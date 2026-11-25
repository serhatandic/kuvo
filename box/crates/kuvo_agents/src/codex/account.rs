//! Who Codex is signed in as, signing in (ChatGPT, its device code, an API key), the plan's usage
//! windows as it says them, and its models.

use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, Mutex as Plain,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    account_check::{AccountStatus, CheckFailure},
    catalog::{Model, Models, Speed},
    runtime::AdapterCall,
    session::{Limit, LimitScope, LimitWindow},
    sign_in::{Attempt, Mode, Page, Request, Status},
};

use super::server::{Server, Shared, lock};

const ASK: Duration = Duration::from_secs(30);

pub(super) enum Heard {
    Taken,
    /// The account's, and each turn under way's too.
    Everyone,
    No,
}

#[derive(Default)]
pub(super) struct Account {
    logins: Plain<HashMap<String, Arc<Login>>>,
    /// The plan's usage windows as Codex last said, by limit ("codex" is the plan's).
    rate_limits: Plain<HashMap<String, Value>>,
    /// The plan Codex last said, and who it said it of.
    plan_read: Plain<Option<(Option<String>, String)>>,
    token_plan: Plain<Option<String>>,
    models: Plain<Option<Models>>,
    /// Moves as what Codex listed goes: a listing begun before isn't kept.
    generation: AtomicU64,
    /// This adapter's own, so a client tells its lists from another's.
    prefix: String,
    /// How much each model holds, as Codex said in a turn.
    windows: Plain<HashMap<String, u64>>,
    /// Where it says its sign-in changed, and of which account.
    told: std::sync::OnceLock<(crate::feed::Feed, String)>,
}

impl Account {
    pub fn new() -> io::Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(io::Error::other)?;
        Ok(Self {
            prefix: hex::encode(bytes),
            ..Self::default()
        })
    }

    /// Says its sign-in changed (who, or the plan) on `feed`, as account `provider`.
    pub fn tell_on(&self, feed: crate::feed::Feed, provider: String) {
        let _ = self.told.set((feed, provider));
    }

    fn changed(&self) {
        if let Some((feed, provider)) = self.told.get() {
            feed.emit(
                "kuvo.agent.account.changed",
                serde_json::json!({ "integrationID": provider }),
            );
        }
    }

    pub fn signing_in(&self) -> bool {
        !lock(&self.logins).is_empty()
    }

    pub fn server_stopped(&self, _server: u64) {}

    /// What's for the account, not a thread: true when it's taken here. The plan's windows go to
    /// the turns under way too.
    pub async fn heard(
        &self,
        shared: &Arc<Shared>,
        server: &Server,
        method: &str,
        params: &Value,
    ) -> Heard {
        match method {
            "account/login/completed" => {
                let login = params["loginId"]
                    .as_str()
                    .and_then(|id| lock(&self.logins).remove(id));
                if let Some(login) = login {
                    let status = if params["success"] == true {
                        Status::Complete
                    } else {
                        Status::Failed {
                            message: params["error"]
                                .as_str()
                                .unwrap_or("Signing in didn't work.")
                                .to_owned(),
                        }
                    };
                    if status == Status::Complete {
                        self.signed_in(shared, server).await;
                    }
                    *lock(&login.status) = status;
                }
                Heard::Taken
            }
            "account/updated" => {
                let identity = shared.program.identity().await;
                // (A new plan for the same sign-in says so itself; anything else is said here.)
                let noted = params["planType"]
                    .as_str()
                    .filter(|plan| *plan != "unknown")
                    .is_some_and(|plan| {
                        *lock(&server.signed_in_as) == identity && self.note_plan(plan, identity)
                    });
                if !noted {
                    self.changed();
                }
                Heard::Taken
            }
            "account/rateLimits/updated" => {
                let identity = shared.program.identity().await;
                let snapshot = &params["rateLimits"];
                if snapshot.is_object() && *lock(&server.signed_in_as) == identity {
                    let id = snapshot["limitId"].as_str().unwrap_or("codex").to_owned();
                    let mut limits = lock(&self.rate_limits);
                    let previous = limits.get(&id).cloned().unwrap_or(Value::Null);
                    // A rolling update can omit the windows or the model name.
                    let mut kept = snapshot.clone();
                    for key in ["normalModelSlug", "primary", "secondary"] {
                        if kept[key].is_null() && !previous[key].is_null() {
                            kept[key] = previous[key].clone();
                        }
                    }
                    limits.insert(id, kept);
                    drop(limits);
                    if let Some(plan) = snapshot["planType"]
                        .as_str()
                        .filter(|plan| *plan != "unknown")
                    {
                        self.note_plan(plan, identity);
                    }
                    return Heard::Everyone;
                }
                Heard::Taken
            }
            // A spawned agent's thread: subagents are row 4.6's.
            "thread/started" => Heard::Taken,
            _ => Heard::No,
        }
    }

    /// The plan Codex said: whether it's news (and if so, said).
    fn note_plan(&self, plan: &str, identity: Option<String>) -> bool {
        let read = Some((identity, plan.to_owned()));
        let mut kept = lock(&self.plan_read);
        if *kept == read {
            return false;
        }
        *kept = read;
        drop(kept);
        self.changed();
        true
    }

    /// Signed in on `server`: what Codex said of the last sign-in goes, and the app-server reads the
    /// new one as it starts again.
    async fn signed_in(&self, shared: &Shared, server: &Server) {
        *lock(&server.signed_in_as) = shared.program.identity().await;
        lock(&self.rate_limits).clear();
        *lock(&self.plan_read) = None;
        *lock(&self.token_plan) = None;
        self.forget_models();
    }

    pub fn forget_models(&self) {
        let mut models = lock(&self.models);
        self.generation.fetch_add(1, Ordering::Relaxed);
        *models = None;
    }

    pub async fn probe(&self, shared: &Arc<Shared>) -> Result<AccountStatus, CheckFailure> {
        let mut answer = AccountStatus {
            installed: false,
            signed_in: false,
            label: None,
            identity: None,
            label_describes_method: false,
            message: None,
            ended: false,
            unchecked: false,
        };
        if !shared.program.installed() {
            answer.message = Some("Install Codex, then sign in here.".into());
            return Ok(answer);
        }
        answer.installed = true;
        let failed = |error: io::Error| match error.kind() {
            io::ErrorKind::TimedOut => CheckFailure::TimedOut,
            _ => CheckFailure::Failed(error.to_string()),
        };
        let server = shared.server().await.map_err(failed)?;
        let identity = shared.program.identity().await;
        let result = server
            .rpc
            .request(
                "account/read",
                Some(json!({})),
                Some(Duration::from_secs(20)),
            )
            .await
            .map_err(|error| failed(error.into()))?;
        if !result.is_object() {
            return Err(CheckFailure::Failed(
                "Codex couldn't say whether it's signed in.".into(),
            ));
        }
        let info = &result["account"];
        if info.is_null() {
            answer.message = Some("Sign in to Codex with ChatGPT or an OpenAI API key.".into());
            return Ok(answer);
        }
        answer.signed_in = true;
        if info["type"] == "chatgpt" {
            let plan = info["planType"].as_str().map(str::to_owned);
            let mut token_plan = lock(&self.token_plan);
            if token_plan.is_some() && *token_plan != plan {
                *lock(&self.plan_read) = None;
            }
            *token_plan = plan.clone();
            drop(token_plan);
            let read = lock(&self.plan_read)
                .clone()
                .filter(|(who, _)| *who == identity)
                .map(|(_, plan)| plan);
            let plan = read
                .or(plan)
                .filter(|plan| plan != "unknown")
                .map(|plan| plan_name(&plan));
            let email = info["email"].as_str().filter(|email| !email.is_empty());
            let plan = plan.map_or_else(|| "ChatGPT".to_owned(), |plan| format!("ChatGPT {plan}"));
            answer.label = Some(match email {
                Some(email) => format!("{email} · {plan}"),
                None => plan,
            });
            answer.label_describes_method = email.is_none();
            answer.identity = identity;
        } else {
            answer.label = Some(match info["type"].as_str() {
                Some("apiKey") => "OpenAI API key".into(),
                Some(other) => other.into(),
                None => info["type"].to_string(),
            });
            answer.label_describes_method = true;
        }
        Ok(answer)
    }

    pub async fn sign_in(
        &self,
        shared: &Arc<Shared>,
        request: Request,
    ) -> io::Result<Arc<dyn Attempt>> {
        let server = shared.server().await?;
        // On another device: a code to enter on ChatGPT's page, as `codex login --device-auth`.
        let device = request.remote && !request.browser_return;
        let kind = if device {
            "chatgptDeviceCode"
        } else {
            "chatgpt"
        };
        let started = server
            .rpc
            .request("account/login/start", Some(json!({"type":kind})), Some(ASK))
            .await?;
        let id = started["loginId"].as_str().unwrap_or_default().to_owned();
        let page = if device {
            let code = started["userCode"].as_str().unwrap_or_default().to_owned();
            Page {
                url: started["verificationUrl"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
                instructions: format!("Open ChatGPT's page, sign in, and enter the code {code}."),
                mode: Mode::Auto,
                accepts_code: false,
                user_code: Some(code),
            }
        } else {
            Page {
                url: started["authUrl"].as_str().unwrap_or_default().into(),
                instructions: "Finish signing in with ChatGPT in your browser.".into(),
                mode: Mode::Auto,
                accepts_code: false,
                user_code: None,
            }
        };
        let login = Arc::new(Login {
            id: id.clone(),
            server: server.clone(),
            status: Plain::new(Status::Pending { page }),
            account: Arc::downgrade(shared),
        });
        lock(&self.logins).insert(id, login.clone());
        Ok(login)
    }

    pub async fn sign_in_with_key(&self, shared: &Arc<Shared>, key: String) -> io::Result<()> {
        let server = shared.server().await?;
        server
            .rpc
            .request(
                "account/login/start",
                Some(json!({"type":"apiKey","apiKey":key})),
                Some(ASK),
            )
            .await?;
        self.signed_in(shared, &server).await;
        Ok(())
    }

    /// Codex's models, as it lists them (with those it hides from its picker, which it still runs):
    /// the ones it listed last while it can't say.
    pub async fn models(&self, shared: &Arc<Shared>, refresh: bool) -> io::Result<Models> {
        let generation = {
            let models = lock(&self.models);
            if let (Some(models), false) = (&*models, refresh) {
                return Ok(models.clone());
            }
            self.generation.load(Ordering::Relaxed)
        };
        match self.list(shared, refresh).await {
            Ok(mut models) => {
                models.generation = Some(format!("{}-{generation}", self.prefix));
                let mut kept = lock(&self.models);
                if self.generation.load(Ordering::Relaxed) == generation {
                    *kept = Some(models.clone());
                }
                Ok(models)
            }
            Err(error) => match (&*lock(&self.models), refresh) {
                (Some(models), false) => Ok(models.clone()),
                _ => Err(error),
            },
        }
    }

    async fn list(&self, shared: &Arc<Shared>, refresh: bool) -> io::Result<Models> {
        let server = shared.server().await?;
        if refresh {
            // model/list reads its cache first: only that file goes, after it started.
            shared.program.forget_model_cache()?;
        }
        let mut models: Vec<Model> = Vec::new();
        let mut defaults = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params = json!({"includeHidden":true});
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let page = server
                .rpc
                .request("model/list", Some(params), Some(ASK))
                .await?;
            for info in page["data"].as_array().into_iter().flatten() {
                let Some(id) = info["model"].as_str().or_else(|| info["id"].as_str()) else {
                    continue;
                };
                let model = Model {
                    id: id.to_owned(),
                    name: info["displayName"].as_str().unwrap_or(id).to_owned(),
                    aliases: Vec::new(),
                    efforts: info["supportedReasoningEfforts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|effort| effort["reasoningEffort"].as_str().map(str::to_owned))
                        .collect(),
                    speeds: speeds(&info["serviceTiers"]),
                    hidden: info["hidden"] == true,
                    retiring: false,
                    provisional: false,
                    context: Some(lock(&self.windows).get(id).copied().unwrap_or(400_000)),
                };
                // Listed in Codex's order: its default first.
                if info["isDefault"] == true {
                    defaults.push(model);
                } else {
                    models.push(model);
                }
            }
            cursor = page["nextCursor"]
                .as_str()
                .filter(|next| !next.is_empty())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        defaults.extend(models);
        Ok(Models {
            generation: None,
            complete: true,
            models: defaults,
        })
    }

    /// How much `model` holds, as Codex said in a turn: the models list it from then on.
    pub fn note_context(&self, model: &str, window: Option<u64>) {
        let Some(window) = window.filter(|window| *window > 0) else {
            return;
        };
        if lock(&self.windows).insert(model.to_owned(), window) != Some(window) {
            self.forget_models();
        }
    }

    /// A usage or rate limit in Codex's error: the plan's, or one model's.
    pub fn limit_of(&self, error: &Value, model: &str) -> Option<Limit> {
        let info = error["codexErrorInfo"].as_str()?;
        if info != "usageLimitExceeded" && info != "rateLimitExceeded" {
            return None;
        }
        let windows =
            |snapshot: &Value| [snapshot["primary"].clone(), snapshot["secondary"].clone()];
        let reached = |snapshot: &Value| {
            snapshot["rateLimitReachedType"]
                .as_str()
                .is_some_and(|kind| !kind.is_empty())
                || windows(snapshot)
                    .iter()
                    .any(|window| window["usedPercent"].as_f64().unwrap_or(0.0) >= 100.0)
        };
        let resets = |snapshot: &Value| {
            windows(snapshot)
                .iter()
                .filter(|window| window["usedPercent"].as_f64().unwrap_or(0.0) >= 100.0)
                .filter_map(|window| window["resetsAt"].as_u64())
                .map(|at| at * 1000)
                .max()
        };
        let limits = lock(&self.rate_limits);
        for (id, snapshot) in limits.iter() {
            if id == "codex" || !reached(snapshot) {
                continue;
            }
            if let Some(slug) = snapshot["normalModelSlug"].as_str()
                && (model == slug || model.starts_with(&format!("{slug}-")))
            {
                return Some(Limit {
                    scope: LimitScope::Model,
                    resets_at: resets(snapshot),
                });
            }
        }
        let plan = limits.get("codex").cloned().unwrap_or(Value::Null);
        if info == "rateLimitExceeded" && !reached(&plan) {
            return Some(Limit {
                scope: LimitScope::Model,
                resets_at: None,
            });
        }
        Some(Limit {
            scope: LimitScope::Provider,
            resets_at: resets(&plan),
        })
    }
}

/// The speeds a model runs at besides its standard one: its service tiers (one called `default`
/// would be standard).
fn speeds(listed: &Value) -> Vec<Speed> {
    let mut out: Vec<Speed> = Vec::new();
    for each in listed.as_array().into_iter().flatten() {
        let Some(id) = each["id"].as_str() else {
            continue;
        };
        let valid = !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'));
        if !valid || id == "default" || out.iter().any(|speed| speed.id == id) {
            continue;
        }
        let text = |key: &str| {
            each[key]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        out.push(Speed {
            id: id.into(),
            name: text("name").unwrap_or(id).into(),
            description: text("description").map(str::to_owned),
        });
    }
    out
}

/// "Plus", "Pro": a ChatGPT plan's name, from Codex's word for it.
pub(super) fn plan_name(kind: &str) -> String {
    kind.split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A Codex usage snapshot's windows: the plan's (`primary`, `secondary`), or a model's own.
pub(super) fn windows(snapshot: &Value) -> Vec<LimitWindow> {
    if !snapshot.is_object() {
        return Vec::new();
    }
    let limit = snapshot["limitId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .unwrap_or("codex");
    let model = (limit != "codex").then(|| {
        snapshot["limitName"]
            .as_str()
            .or_else(|| snapshot["normalModelSlug"].as_str())
            .unwrap_or(limit)
            .to_owned()
    });
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|which| {
            let window = &snapshot[which];
            let used = window["usedPercent"].as_f64()?;
            Some(LimitWindow {
                id: if limit == "codex" {
                    which.into()
                } else {
                    format!("{limit}.{which}")
                },
                used: used / 100.0,
                minutes: window["windowDurationMins"]
                    .as_u64()
                    .filter(|m| *m > 0)
                    .map(|m| m as u32),
                resets_at: window["resetsAt"]
                    .as_u64()
                    .filter(|at| *at > 0)
                    .map(|at| at * 1000),
                model: model.clone().filter(|model| !model.is_empty()),
            })
        })
        .collect()
}

/// A ChatGPT sign-in under way: Codex says how it went.
struct Login {
    id: String,
    server: Arc<Server>,
    status: Plain<Status>,
    account: std::sync::Weak<Shared>,
}

impl Attempt for Login {
    fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    fn submit_code(&self, _code: String) -> AdapterCall<'_, ()> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Codex finishes signing in on ChatGPT's page.",
            ))
        })
    }

    fn cancel(&self) {
        if let Some(shared) = self.account.upgrade() {
            lock(&shared.account.logins).remove(&self.id);
        }
        let rpc = self.server.rpc.clone();
        let id = self.id.clone();
        tokio::spawn(async move {
            let _ = rpc
                .request(
                    "account/login/cancel",
                    Some(json!({"loginId":id})),
                    Some(ASK),
                )
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_is_named_as_chatgpt_names_it() {
        assert_eq!(plan_name("plus"), "Plus");
        assert_eq!(plan_name("pro_lite"), "Pro Lite");
    }

    #[test]
    fn the_windows_are_the_plans_or_a_models_own() {
        let plan =
            windows(&json!({"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":10}}));
        assert_eq!(plan[0].id, "primary");
        assert_eq!(
            (plan[0].used, plan[0].minutes, plan[0].resets_at),
            (0.42, Some(300), Some(10_000))
        );
        let model = windows(
            &json!({"limitId":"gpt-x","limitName":"GPT X","secondary":{"usedPercent":100}}),
        );
        assert_eq!(
            (model[0].id.as_str(), model[0].model.as_deref()),
            ("gpt-x.secondary", Some("GPT X"))
        );
    }

    #[test]
    fn a_model_at_its_own_limit_waits_alone() {
        let account = Account::default();
        lock(&account.rate_limits).insert(
            "gpt-x".into(),
            json!({"normalModelSlug":"gpt-x","primary":{"usedPercent":100,"resetsAt":7}}),
        );
        let error = json!({"codexErrorInfo":"usageLimitExceeded"});
        let limit = account.limit_of(&error, "gpt-x-mini").unwrap();
        assert_eq!(
            (limit.scope, limit.resets_at),
            (LimitScope::Model, Some(7000))
        );
        assert_eq!(
            account.limit_of(&error, "gpt-y").unwrap().scope,
            LimitScope::Provider
        );
        let rate = json!({"codexErrorInfo":"rateLimitExceeded"});
        assert_eq!(
            account.limit_of(&rate, "gpt-y").unwrap().scope,
            LimitScope::Model
        );
        assert!(account.limit_of(&json!({}), "gpt-y").is_none());
    }
}
