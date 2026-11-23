//! Read the local OpenCode catalog and apply the conversation's model rules.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use http::{HeaderMap, Method, StatusCode};
use kuvo_protocol::routes::{
    get_kuvo_models_offered as offered, get_kuvo_sessions_models as route,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Engine, LocalSessions, api, disk,
    lifetime::{Demand, Lease},
    model_preferences,
    sessions::{self, Session},
};

const CATALOG_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) variant: Option<String>,
}

#[derive(Serialize)]
pub struct ModelSpeed {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct Listed {
    #[serde(flatten)]
    identity: ModelRef,
    name: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    enabled: Option<bool>,
    #[serde(default)]
    variants: Vec<Value>,
    #[serde(default)]
    speeds: Vec<Value>,
    #[serde(default)]
    time: Value,
    #[serde(default)]
    cost: Value,
    status: Option<String>,
    hidden: Option<bool>,
}

impl Listed {
    fn name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.identity.id)
    }

    fn retiring(&self) -> bool {
        self.status.as_deref() == Some("deprecated")
    }

    fn free(&self) -> bool {
        if self.identity.provider != "opencode" {
            return false;
        }
        let costs = self
            .cost
            .as_array()
            .map_or_else(|| std::slice::from_ref(&self.cost), Vec::as_slice);
        (!costs.is_empty()
            && costs.iter().all(|cost| {
                cost["input"].as_f64() == Some(0.0) && cost["output"].as_f64() == Some(0.0)
            }))
            || self
                .name()
                .rsplit_once(char::is_whitespace)
                .is_some_and(|(_, end)| end.eq_ignore_ascii_case("free"))
    }

    fn picker_row(&self) -> Value {
        let mut seen = BTreeSet::new();
        let speeds = self
            .speeds
            .iter()
            .filter_map(|speed| {
                let id = speed["id"]
                    .as_str()
                    .filter(|id| model_preferences::speed_id(id) && seen.insert(*id))?;
                let text = |key| {
                    speed[key]
                        .as_str()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                };
                let mut row = json!({"id": id, "name": text("name").unwrap_or(id)});
                if let Some(description) = text("description") {
                    row["description"] = json!(description);
                }
                Some(row)
            })
            .collect::<Vec<_>>();
        let mut model = json!({
            "providerID": self.identity.provider, "id": self.identity.id,
            "name": self.name(), "side": "engine", "aliases": self.aliases,
            "efforts": self.variants.iter().filter_map(|variant| variant["id"].as_str()).collect::<Vec<_>>(),
        });
        if self.retiring() {
            model["retiring"] = json!(true);
        }
        if self.hidden == Some(true) {
            model["hidden"] = json!(true);
        }
        if self.free() {
            model["free"] = json!(true);
        }
        if !speeds.is_empty() {
            model["speeds"] = json!(speeds);
        }
        model
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
            fallback: fallback.ok().map(|value| {
                serde_json::from_value::<ModelRef>(value)
                    .ok()
                    .map(|mut model| {
                        model.variant = None;
                        model
                    })
            }),
        })
    }

    fn available(&self, provider: &str, listed: bool) -> bool {
        let account = self.account(provider);
        let signed_in = account.is_some_and(|account| !account.connections.is_empty());
        if provider == "opencode" && account.is_some() {
            listed || signed_in
        } else {
            self.providers.iter().any(|ready| ready.id == provider)
                || signed_in
                || account.is_none_or(|account| account.methods.is_empty())
        }
    }

    fn account(&self, provider: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .rev()
            .find(|account| account.id == provider)
    }

    fn offered(&self) -> Vec<&Listed> {
        const FEATURED: &[&str] = &[
            "openai",
            "github-copilot",
            "anthropic",
            "google",
            "xai",
            "openrouter",
            "opencode",
            "deepseek",
            "mistral",
            "groq",
        ];
        let rank = |id: &str| {
            FEATURED
                .iter()
                .position(|featured| id == *featured)
                .unwrap_or(FEATURED.len())
        };
        let mut rows = self
            .models
            .iter()
            .filter(|model| {
                model.enabled != Some(false) && self.available(&model.identity.provider, true)
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| {
            let provider_a = &a.identity.provider;
            let provider_b = &b.identity.provider;
            rank(provider_a)
                .cmp(&rank(provider_b))
                .then_with(|| provider_a.encode_utf16().cmp(provider_b.encode_utf16()))
                .then_with(|| {
                    b.time["released"]
                        .as_f64()
                        .unwrap_or_default()
                        .total_cmp(&a.time["released"].as_f64().unwrap_or_default())
                })
        });
        rows
    }

    fn starting(&self, wanted: Option<ModelRef>, offered: &[&Listed]) -> Option<Option<ModelRef>> {
        let runnable = |model: &ModelRef| {
            self.named(model).filter(|listed| {
                listed.enabled != Some(false)
                    && !listed.retiring()
                    && self.available(&model.provider, true)
            })
        };
        if let Some(mut wanted) = wanted {
            if let Some(listed) = runnable(&wanted) {
                wanted.id.clone_from(&listed.identity.id);
                return Some(Some(wanted));
            }
            let known = self
                .providers
                .iter()
                .any(|provider| provider.id == wanted.provider)
                || self.account(&wanted.provider).is_some();
            let still_loading = !self.models.iter().any(|model| {
                model.identity.provider == wanted.provider && model.enabled != Some(false)
            });
            if known && still_loading && self.available(&wanted.provider, false) {
                return Some(Some(wanted));
            }
        }
        let fallback = self.fallback.as_ref()?;
        if let Some(fallback) = fallback
            && runnable(fallback).is_some()
        {
            return Some(Some(fallback.clone()));
        }
        Some(
            offered
                .iter()
                .find(|model| !model.retiring() && model.hidden != Some(true))
                .map(|model| ModelRef {
                    variant: None,
                    ..model.identity.clone()
                }),
        )
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
        if agent_provider(&model.provider) {
            return None;
        }
        let named = self.named(&model);
        let listed = named.filter(|entry| entry.enabled != Some(false));
        let account = self.account(&model.provider);
        let ready = self.available(&model.provider, listed.is_some());
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

fn agent_provider(provider: &str) -> bool {
    provider == "auto"
        || matches!(
            provider.split('@').next(),
            Some("claude-code" | "codex-cli" | "cursor" | "grok-cli" | "antigravity")
        )
}

fn absolute_folder(text: &str) -> Option<PathBuf> {
    let path = Path::new(text);
    if !path.is_absolute() {
        return None;
    }
    let mut folder = PathBuf::new();
    for component in path.components() {
        if component == Component::ParentDir {
            folder.pop();
        } else {
            folder.push(component);
        }
    }
    Some(folder)
}

async fn known_folder(engine: &Engine, list: &LocalSessions, text: &str) -> Option<PathBuf> {
    let folder = absolute_folder(text)?;
    let matches = |text: &str| absolute_folder(text).as_ref() == Some(&folder);
    let projects = disk::read::<Value>(&engine.home.join("box/projects.json"))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let worktrees = disk::read::<Value>(&engine.home.join("box/worktrees.json"))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let project = projects["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|project| {
            project["removed"] != true
                && project["directory"].is_string()
                && (project["directory"].as_str().is_some_and(matches)
                    || project["folders"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .any(matches))
        });
    let worktree = worktrees["worktrees"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|tree| {
            tree["project"].is_string()
                && tree["branch"].is_string()
                && [tree["root"].as_str(), tree["folder"].as_str()]
                    .into_iter()
                    .flatten()
                    .any(matches)
        });
    if (project || worktree || list.knows_folder(matches))
        && tokio::fs::metadata(&folder)
            .await
            .is_ok_and(|entry| entry.is_dir())
    {
        return Some(folder);
    }
    None
}

enum PickerPlace {
    Engine(Box<Session>),
    Agents(String),
}

async fn picker_place(
    engine: &Engine,
    lease: Option<&Lease>,
    id: &str,
) -> Result<PickerPlace, Refusal> {
    let mut unavailable = false;
    if let Some(lease) = lease {
        match engine
            .call::<Session>(lease, Method::GET, &sessions::path(id), None)
            .await
        {
            Ok(session) => return Ok(PickerPlace::Engine(Box::new(session))),
            Err(error) => unavailable = !error.missing(),
        }
    }
    if let Some(source) = &engine.agent_models {
        match source.session_folder(id).await {
            Ok(Some(folder)) => return Ok(PickerPlace::Agents(folder)),
            Ok(None) => {}
            Err(_) => unavailable = true,
        }
    }
    Err(if unavailable {
        Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The engine isn't answering. Try again in a moment.",
        )
    } else {
        Refusal::new(
            StatusCode::NOT_FOUND,
            "This conversation is no longer available.",
        )
    })
}

async fn picker(
    engine: &Engine,
    list: &LocalSessions,
    path: &str,
) -> Result<RouteReply<offered::Spec>, Refusal> {
    let query = |key| {
        crate::query(path, key)
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    let refresh = query("refresh").as_deref() == Some("1");
    let lease = engine.acquire(Demand::Start).await;
    if lease.is_none() && engine.agent_models.is_none() {
        return Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The engine isn't running.",
        ));
    }
    let place = match query("session") {
        Some(id) => Some(picker_place(engine, lease.as_ref(), &id).await?),
        None => None,
    };
    let folder = match (place.as_ref(), query("folder")) {
        (None, Some(f)) => known_folder(engine, list, &f).await,
        _ => None,
    };
    let directory = match &place {
        Some(PickerPlace::Engine(session)) => session.directory(),
        Some(PickerPlace::Agents(folder)) => folder,
        None => folder.as_deref().and_then(Path::to_str).unwrap_or_default(),
    };
    let read_engine = async {
        let lease = lease.as_ref()?;
        if refresh {
            crate::model_refresh::revalidate_picker(engine, lease, directory).await;
        }
        Catalog::read(engine, lease, directory).await
    };
    let read_agents = async { engine.agent_models.as_ref()?.catalog(refresh).await.ok() };
    let (catalog, agents, starred, speeds) = tokio::join!(
        read_engine,
        read_agents,
        model_preferences::picked_stars(engine),
        model_preferences::picked_speeds(engine),
    );
    let engine_models = catalog.as_ref().map(Catalog::offered).unwrap_or_default();
    let mut models = engine_models
        .iter()
        .map(|model| model.picker_row())
        .collect::<Vec<_>>();
    if let Some(agents) = &agents {
        models.extend(agents.models());
    }
    let mut seen = BTreeSet::new();
    let providers = models
        .iter()
        .filter_map(|model| {
            let provider = model["providerID"].as_str()?;
            if !seen.insert(provider) {
                return None;
            }
            let mut row = if model["side"] == "agents" {
                agents.as_ref()?.provider(provider)
            } else {
                let name = catalog
                    .as_ref()
                    .and_then(|catalog| catalog.account(provider))
                    .map_or(provider, Account::name);
                json!({"id": provider, "name": name})
            };
            if let Some(speed) = speeds.get(provider).and_then(Value::as_str) {
                row["speed"] = json!(speed);
            }
            Some(row)
        })
        .collect::<Vec<_>>();
    let wanted = query("provider")
        .zip(query("model"))
        .map(|(provider, id)| ModelRef {
            provider,
            id,
            variant: query("variant"),
        });
    let (wanted, agent_choice) = match wanted {
        Some(model) if agent_provider(&model.provider) => {
            let choice = match &agents {
                Some(agents) => agents.starting(model),
                None => Some(model),
            };
            (None, choice)
        }
        other => (other, None),
    };
    let starting = agent_choice.map(Some).or_else(|| {
        catalog
            .as_ref()?
            .starting(wanted, &engine_models)
            .map(|model| model.or_else(|| agents.as_ref().and_then(crate::AgentCatalog::first)))
    });
    let defaults = query("for").as_deref() == Some("defaults");
    if defaults || matches!(place, Some(PickerPlace::Engine(_))) {
        models.retain(|model| {
            model["side"] == "engine"
                && (!defaults || (model["retiring"] != true && model["hidden"] != true))
        });
    }
    let mut answer = json!({
        "models": models, "starred": starred,
        "known": {"engine": catalog.is_some(), "agents": agents.is_some()},
    });
    if let Some(agents) = &agents {
        answer["generations"] = json!(agents.generations);
        if !agents.unavailable.is_empty() {
            answer["unavailable"] = json!(
                agents
                    .unavailable
                    .iter()
                    .map(|id| json!({"id": id}))
                    .collect::<Vec<_>>()
            );
        }
        let listing = agents.listing();
        if !listing.is_empty() {
            answer["listing"] = json!(listing);
        }
    }
    if let Some(starting) = starting {
        if starting.is_none() {
            answer["startChoice"] =
                json!({"label": "Choose a Model", "refusal": "Choose a model to send it."});
        }
        answer["starting"] = json!(starting);
    }
    if defaults || place.is_some() {
        answer["scoped"] = json!(true);
    }
    if let Some(PickerPlace::Engine(session)) = place
        && !defaults
    {
        let note = if session
            .notes()
            .is_some_and(|notes| notes.get("bot").is_some_and(Value::is_string))
        {
            "To use a coding agent in this bot, change its model in Edit Bot."
        } else {
            "This conversation uses provider models. Start a new conversation to use a coding agent’s models."
        };
        answer["note"] = json!(note);
    }
    // Existing clients read the account detail carried beside a provider's base wire fields.
    RouteReply::json_with::<200>(sessions::typed(answer)?, [("providers", json!(providers))])
        .map_err(|_| {
            Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The model picker could not be read.",
            )
        })
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

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) {
    let here = engine.clone();
    routes.register::<offered::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        let list = list.clone();
        async move { picker(&here, &list, &call.request.path).await }
    });
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
