//! Read the local OpenCode catalog and apply the conversation's model rules.

use std::{collections::BTreeSet, time::Duration};

use http::{HeaderMap, Method, StatusCode};
use kuvo_protocol::routes::get_kuvo_sessions_models as route;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Engine, api, disk,
    lifetime::{Demand, Lease},
    sessions::{self, Session},
};

const CATALOG_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    variant: Option<String>,
}

#[derive(Deserialize)]
struct Listed {
    #[serde(flatten)]
    identity: ModelRef,
    name: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    enabled: Option<bool>,
}

impl Listed {
    fn name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.identity.id)
    }
}

#[derive(Deserialize)]
struct Provider {
    id: String,
}

#[derive(Deserialize)]
struct Account {
    id: String,
    name: Option<String>,
    #[serde(default)]
    methods: Vec<MethodEntry>,
    #[serde(default)]
    connections: Vec<Value>,
}

impl Account {
    fn name(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }

    fn shown(&self) -> Value {
        json!({ "id": self.id, "name": self.name() })
    }
}

#[derive(Deserialize)]
struct MethodEntry {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: String,
}

struct Plan {
    name: &'static str,
    agent: &'static str,
    provider: &'static str,
}

impl Plan {
    fn of(provider: &str) -> Option<Self> {
        match provider {
            "openai" => Some(Self {
                name: "ChatGPT",
                agent: "Codex",
                provider: "codex-cli",
            }),
            "xai" => Some(Self {
                name: "SuperGrok",
                agent: "Grok",
                provider: "grok-cli",
            }),
            _ => None,
        }
    }
}

struct Catalog {
    providers: Vec<Provider>,
    models: Vec<Listed>,
    accounts: Vec<Account>,
    // An absent reply and a reply that says there is no default carry different evidence.
    fallback: Option<Option<ModelRef>>,
}

impl Catalog {
    async fn read(engine: &Engine, lease: &Lease, directory: &str) -> Option<Self> {
        let client = &engine.upstream;
        let mut headers = HeaderMap::new();
        if !directory.is_empty() {
            headers.insert(
                "x-opencode-directory",
                api::segment(directory).parse().ok()?,
            );
        }
        let record = &lease.record;
        let (providers, models, accounts, fallback) = tokio::join!(
            client.get_data::<Vec<Value>>(record, "api/provider", &headers, CATALOG_WAIT),
            client.get_data::<Vec<Value>>(record, "api/model", &headers, CATALOG_WAIT),
            client.get_data::<Vec<Value>>(record, "api/integration", &headers, CATALOG_WAIT),
            client.get_data::<Value>(record, "api/model/default", &headers, CATALOG_WAIT),
        );
        let providers: Vec<Provider> = entries(providers.ok()?);
        let models: Vec<Listed> = entries(models.ok()?);
        if providers.is_empty() || !models.iter().any(|model| model.enabled != Some(false)) {
            return None;
        }
        Some(Self {
            providers,
            models,
            accounts: entries(accounts.ok()?),
            fallback: fallback
                .ok()
                .map(|value| serde_json::from_value(value).ok()),
        })
    }

    fn named(&self, model: &ModelRef) -> Option<&Listed> {
        let of_provider = || {
            self.models
                .iter()
                .filter(|entry| entry.identity.provider == model.provider)
        };
        of_provider()
            .find(|entry| entry.identity.id == model.id)
            .or_else(|| of_provider().find(|entry| entry.aliases.contains(&model.id)))
    }

    fn readiness(&self, model: Option<ModelRef>, names: &Names) -> Option<Value> {
        let Some(model) = model else {
            return Some(json!({
                "model": null, "runs": false, "missing": "model", "choose": true,
                "refusal": "Choose a model to send it.",
                "action": action("choose", "Choose a Model", "Choose a model")
            }));
        };
        if matches!(
            model.provider.split('@').next()?,
            "auto" | "claude-code" | "codex-cli" | "cursor" | "grok-cli" | "antigravity"
        ) {
            return None;
        }
        let named = self.named(&model);
        let listed = named.filter(|entry| entry.enabled != Some(false));
        let account = self
            .accounts
            .iter()
            .find(|account| account.id == model.provider);
        let ready = if model.provider == "opencode" {
            listed.is_some() || account.is_none_or(|account| !account.connections.is_empty())
        } else {
            self.providers
                .iter()
                .any(|provider| provider.id == model.provider)
                || account.is_none_or(|account| {
                    !account.connections.is_empty() || account.methods.is_empty()
                })
        };
        if ready {
            if let Some(listed) = listed {
                return Some(json!({ "model": model, "name": listed.name(), "runs": true }));
            }
            if !self.models.iter().any(|entry| {
                entry.identity.provider == model.provider && entry.enabled != Some(false)
            }) {
                return None;
            }
            let name = named
                .map(Listed::name)
                .or_else(|| names.get(&model))
                .unwrap_or(&model.id);
            let refusal = account.map_or_else(
                || format!("{name} is no longer available: choose another model."),
                |account| {
                    format!(
                        "{name} is no longer among {}’s models: choose another model.",
                        account.name()
                    )
                },
            );
            let mut answer = json!({ "model": model, "runs": false, "missing": "gone", "choose": true,
                "refusal": refusal, "action": action("choose", "Choose a Model", "Choose another model") });
            if let Some(account) = account {
                answer["account"] = account.shown();
            }
            return Some(answer);
        }
        let account = account?;
        let name = listed
            .map(Listed::name)
            .or_else(|| names.get(&model))
            .unwrap_or(&model.id);
        let methods: Vec<_> = account
            .methods
            .iter()
            .filter(|method| match method.kind.as_str() {
                "oauth" => {
                    !(model.provider == "openai" && method.id.starts_with("chatgpt")
                        || model.provider == "xai" && method.id == "device")
                }
                "key" | "external" => true,
                _ => false,
            })
            .collect();
        let key = !methods.is_empty() && methods.iter().all(|method| method.kind == "key");
        let connects = !key
            && methods.iter().any(|method| method.kind == "external")
            && !methods.iter().any(|method| method.kind == "oauth");
        let provider = account.name();
        let mut answer = json!({ "model": model, "runs": false, "account": account.shown() });
        if let Some(listed) = listed {
            answer["name"] = json!(listed.name());
        }
        let (refusal, action) = if key {
            answer["missing"] = json!("key");
            let key = format!("{} {provider} key", article(provider));
            if let Some(plan) = Plan::of(&model.provider) {
                let with = format!("{} {} plan", article(plan.name), plan.name);
                answer["plan"] =
                    json!({ "name": plan.name, "agent": plan.agent, "provider": plan.provider });
                (
                    format!(
                        "{name} needs {key}: add one, or choose another model (with {with}, one under {}).",
                        plan.agent
                    ),
                    action(
                        "addKey",
                        "Add Key",
                        format!(
                            "Add {key} (with {with}, choose a model under {} instead)",
                            plan.agent
                        ),
                    ),
                )
            } else {
                (
                    format!("{name} needs {key}: add one, or choose another model."),
                    action("addKey", "Add Key", format!("Add {key}")),
                )
            }
        } else if connects {
            answer["missing"] = json!("signIn");
            answer["connects"] = json!(true);
            (
                format!("{name} needs {provider} connected: connect it, or choose another model."),
                action("signIn", "Connect", format!("Connect {provider}")),
            )
        } else {
            answer["missing"] = json!("signIn");
            (
                format!("{name} needs a sign-in to {provider}: sign in, or choose another model."),
                action("signIn", "Sign In", format!("Sign in to {provider}")),
            )
        };
        answer["refusal"] = json!(refusal);
        answer["action"] = action;
        Some(answer)
    }
}

fn entries<T: serde::de::DeserializeOwned>(values: Vec<Value>) -> Vec<T> {
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

fn action(do_what: &str, label: &str, help: impl AsRef<str>) -> Value {
    json!({ "do": do_what, "label": label, "help": help.as_ref() })
}

fn article(word: &str) -> &'static str {
    let first = word.as_bytes().first().copied().unwrap_or_default();
    let vowel = if word.as_bytes().get(1).is_some_and(u8::is_ascii_uppercase)
        && first.is_ascii_uppercase()
    {
        b"AEFHILMNORSX".contains(&first)
    } else {
        b"aeiou".contains(&first.to_ascii_lowercase()) || word.starts_with("xA")
    };
    if vowel { "an" } else { "a" }
}

#[derive(Default, Deserialize, Serialize)]
struct Names {
    names: Vec<(String, String, String)>,
}

impl Names {
    fn get(&self, model: &ModelRef) -> Option<&str> {
        self.names
            .iter()
            .rev()
            .find(|(provider, id, _)| provider == &model.provider && id == &model.id)
            .map(|(_, _, name)| name.as_str())
    }
}

pub(crate) async fn readiness(engine: &Engine, lease: &Lease, session: &Session) -> Option<Value> {
    let catalog = Catalog::read(engine, lease, session.directory()).await?;
    let model = session
        .extra
        .get("model")
        .and_then(|model| serde_json::from_value(model.clone()).ok())
        .map(Some)
        .or(catalog.fallback.clone())?;
    let path = engine.home.join("box/model-names.json");
    let _turn = engine.turn("model-names").await;
    let mut names = disk::read::<Names>(&path)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let answer = catalog.readiness(model.clone(), &names)?;
    if let Some(model) = model
        && answer["runs"] == true
        && let Some(name) = answer["name"].as_str()
        && names.get(&model) != Some(name)
    {
        names
            .names
            .retain(|(provider, id, _)| provider != &model.provider || id != &model.id);
        names
            .names
            .push((model.provider, model.id, name.to_owned()));
        if names.names.len() > 2000 {
            names.names.drain(..names.names.len() - 2000);
        }
        let _ = disk::durable(path, names).await;
    }
    Some(answer)
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let engine = engine.clone();
    routes.register::<route::Spec, _, _>(Access::Any, move |call| {
        let engine = engine.clone();
        async move {
            let ids: BTreeSet<_> = crate::query(&call.request.path, "sessions")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|id| sessions::valid_id(id))
                .map(str::to_owned)
                .collect();
            if ids.is_empty() || ids.len() > 200 {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Name the sessions: sessions=<id>,<id>… (200 at most).",
                ));
            }
            let lease = engine.acquire(Demand::Passive).await.ok_or_else(|| {
                Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.")
            })?;
            let rows = futures_util::future::join_all(ids.into_iter().map(|id| {
                let (engine, lease) = (&engine, &lease);
                async move {
                    let session = match engine
                        .call::<Session>(lease, Method::GET, &sessions::path(&id), None)
                        .await
                    {
                        Ok(session) => session,
                        Err(error) if error.missing() => return Ok(None),
                        Err(_) => {
                            return Err(Refusal::new(
                                StatusCode::SERVICE_UNAVAILABLE,
                                "The engine isn't answering. Try again in a moment.",
                            ));
                        }
                    };
                    Ok(readiness(engine, lease, &session)
                        .await
                        .map(|answer| (session.id, answer)))
                }
            }))
            .await;
            let mut sessions = serde_json::Map::new();
            for row in rows {
                sessions.extend(row?);
            }
            Ok(RouteReply::json::<200>(sessions::typed(
                json!({ "sessions": sessions }),
            )?))
        }
    });
}
