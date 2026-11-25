//! Grok's CLI: your own `grok`, signed in with `grok login` (SuperGrok, or an xAI key in
//! XAI_API_KEY), its conversations in `grok agent stdio` processes speaking ACP with `x.ai`
//! extensions: its background tasks, its own end of a prompt, its questions and its plan.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as Plain,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::future::BoxFuture;
use kuvo_os::{platform::Environment, process::Spawn};
use kuvo_store::home::Home;
use serde_json::{Map, Value, json};

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::AgentKind,
    catalog::Models,
    hold::{HoldKind, Limits, Metadata, Start},
    program_updates::{Definition, Native},
    rpc::Rpc,
    runtime::AdapterCall,
    session_view::Compaction,
    sign_in::{Attempt, Request, command as sign_in},
};

use super::{
    flavour::{Extension, Flavour, Launch, Mode, OwnAnswer, OwnRequest, Question, Task},
    local::{self, Catalog, Listed},
    server::lock,
    wire::{Capabilities, Setup},
};

const INSTALL: &str = "curl -fsSL https://x.ai/cli/install.sh | bash";
/// How much a Grok model holds before it says.
const CONTEXT: u64 = 256_000;
/// The efforts every Grok model has, before a session says.
const EFFORTS: [&str; 3] = ["low", "medium", "high"];

pub(super) struct Grok {
    home: Home,
    provider: String,
    /// An added account's own GROK_HOME.
    managed: Option<PathBuf>,
    feed: Option<crate::feed::Feed>,
    prompts: AtomicU64,
    /// How much each model holds, as Grok said in a turn.
    windows: Plain<HashMap<String, u64>>,
}

impl Grok {
    pub fn new(
        home: Home,
        provider: String,
        managed: Option<PathBuf>,
        feed: Option<crate::feed::Feed>,
    ) -> Self {
        Self {
            home,
            provider,
            managed,
            feed,
            prompts: AtomicU64::new(0),
            windows: Plain::default(),
        }
    }

    fn environment(&self) -> Environment {
        let mut env = local::environment();
        if !local::agents_update_themselves(&self.home) {
            env.insert("GROK_DISABLE_AUTOUPDATER".into(), "1".into());
        }
        if let Some(home) = &self.managed {
            env.retain(|name, _| !name.eq_ignore_ascii_case("GROK_HOME"));
            env.insert("GROK_HOME".into(), home.as_os_str().into());
        }
        env
    }

    fn executable(&self, env: &Environment) -> Option<kuvo_os::platform::Executable> {
        local::program("grok", &[".grok/bin", ".local/bin"], env)
    }

    /// Its home (GROK_HOME): an added account's own folder, else yours.
    fn grok_home(&self, env: &Environment) -> Option<PathBuf> {
        if let Some(home) = &self.managed {
            return Some(home.clone());
        }
        let user = local::user_home(env);
        match local::variable(env, "GROK_HOME") {
            Some(set) if set == "~" => user,
            Some(set) => match set.strip_prefix("~/") {
                Some(rest) => user.map(|home| home.join(rest)),
                None => Some(set.into()),
            },
            None => user.map(|home| home.join(".grok")),
        }
    }

    fn api_key(env: &Environment) -> Option<String> {
        local::variable(env, "XAI_API_KEY")
    }

    /// The sign-in Grok keeps in its home: the key of its first entry that has one.
    fn kept_key(&self, env: &Environment) -> Option<String> {
        let bytes = std::fs::read(self.grok_home(env)?.join("auth.json")).ok()?;
        let entries: Map<String, Value> = serde_json::from_slice(&bytes).ok()?;
        entries.values().find_map(|entry| {
            entry["key"]
                .as_str()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
        })
    }

    fn catalog_file(&self) -> PathBuf {
        Catalog::file(&self.home, "grok", &self.provider)
    }

    fn context(&self, model: &str) -> u64 {
        lock(&self.windows).get(model).copied().unwrap_or(CONTEXT)
    }

    fn told(&self, kind: &str) {
        if let Some(feed) = &self.feed {
            feed.emit(kind, json!({"program": "grok"}));
        }
    }

    /// The share of the model's window Grok summarizes at, from "Summarize long conversations at".
    fn threshold(&self, model: &str) -> Option<u64> {
        let Compaction::At(tokens) = Compaction::read(self.home.root()) else {
            return None;
        };
        let window = lock(&self.windows).get(model).copied()?;
        Some((u64::from(tokens) * 100 / window).clamp(1, 85))
    }

    fn mode_args(mode: &Mode) -> (Vec<&'static str>, Vec<&'static str>) {
        if mode.read_only {
            (vec!["--permission-mode", "plan"], vec![])
        } else if mode.approve_all {
            (vec![], vec!["--always-approve"])
        } else {
            (vec!["--permission-mode", "default"], vec![])
        }
    }

    fn command(&self, request: &Request) -> io::Result<sign_in::Options> {
        let env = self.environment();
        let executable = self.executable(&env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Install Grok's CLI first: {INSTALL}"),
            )
        })?;
        let mut spawn = Spawn::new(executable.path(), env);
        spawn.executable = executable;
        spawn.args = if request.remote {
            vec!["login".into(), "--device-auth".into()]
        } else {
            vec!["login".into()]
        };
        spawn.cwd = Some(self.home.root().to_owned());
        let scratch = self.home.run().join("agents/tmp");
        kuvo_store::files::private_folder(&scratch)?;
        let confirm = Self {
            home: self.home.clone(),
            provider: self.provider.clone(),
            managed: self.managed.clone(),
            feed: None,
            prompts: AtomicU64::new(0),
            windows: Plain::default(),
        };
        let feed = self.feed.clone();
        Ok(sign_in::Options {
            spawn,
            temporary: scratch,
            printed_page: request.remote,
            code_prompt: None,
            instructions: if request.remote {
                "Open Grok's page, sign in, and enter the code it shows here."
            } else {
                "Finish signing in to Grok in your browser."
            },
            code_instructions: "Open Grok's page, sign in, and enter the code it shows here.",
            user_code: Some(device_code),
            confirm: Box::new(move || confirm.credential().is_some()),
            completed: Box::new(move || {
                if let Some(feed) = feed {
                    feed.emit("integration.updated", json!({"program": "grok"}));
                }
            }),
        })
    }
}

/// The code `grok login --device-auth` prints: groups of capitals and digits joined by dashes.
fn device_code(said: &str) -> Option<String> {
    said.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find_map(|word| {
            let groups: Vec<_> = word.split('-').collect();
            let code = groups.len() >= 2
                && groups.iter().all(|group| {
                    group.len() >= 4
                        && group
                            .bytes()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                });
            code.then(|| word.to_owned())
        })
}

impl Flavour for Grok {
    fn kind(&self) -> AgentKind {
        AgentKind::Grok
    }

    fn name(&self) -> &'static str {
        "Grok"
    }

    /// From what Grok keeps of its sign-in: Grok isn't started.
    fn probe(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(async move {
            let env = self.environment();
            let installed = self.executable(&env).is_some();
            let key = Self::api_key(&env).is_some();
            let signed_in = installed && (key || self.kept_key(&env).is_some());
            let message = if !installed {
                Some("Install Grok's CLI, then sign in here.".into())
            } else if !signed_in {
                Some("Sign in to Grok with your SuperGrok plan.".into())
            } else {
                None
            };
            Ok(AccountStatus {
                installed,
                signed_in,
                label: signed_in.then(|| if key { "xAI API key" } else { "grok.com" }.into()),
                identity: self.credential(),
                label_describes_method: signed_in,
                message,
                ended: false,
                unchecked: false,
            })
        })
    }

    /// As a conversation's turn last listed them: Grok isn't started to list them.
    fn models(&self, _refresh: bool) -> AdapterCall<'_, Models> {
        Box::pin(async move {
            let credential = self.credential();
            let file = self.catalog_file();
            let catalog =
                tokio::task::spawn_blocking(move || Catalog::read(&file, credential.as_deref()))
                    .await
                    .map_err(io::Error::other)?;
            Ok(match catalog {
                Some(catalog) => {
                    local::models(&catalog, |model| self.context(model), str::to_owned)
                }
                None => {
                    let mut models = local::provisional("Grok");
                    for model in &mut models.models {
                        model.efforts = EFFORTS.map(String::from).to_vec();
                        model.context = Some(CONTEXT);
                    }
                    models
                }
            })
        })
    }

    fn sign_in(&self, request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        Box::pin(async move {
            let options = self.command(&request)?;
            sign_in::start(options).await
        })
    }

    /// `grok logout`. A key from your shell's environment keeps it signed in: said so.
    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            let env = self.environment();
            let key = Self::api_key(&env).is_some();
            if let Some(executable) = self.executable(&env) {
                let cwd = self.home.root().to_owned();
                let done = tokio::task::spawn_blocking(move || {
                    local::run(executable, env, &["logout"], &cwd)
                })
                .await
                .map_err(io::Error::other)?;
                if !key && !done.unwrap_or(false) {
                    return Err(io::Error::other("Grok couldn't sign out."));
                }
            }
            self.told("integration.updated");
            if key {
                return Err(io::Error::other(
                    "Grok stays signed in with XAI_API_KEY from your shell’s environment. Remove it there to sign out.",
                ));
            }
            Ok(())
        })
    }

    /// An added account signs out as it goes, before its folder goes.
    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            let env = self.environment();
            if self.managed.is_some()
                && let Some(executable) = self.executable(&env)
            {
                let cwd = self.home.root().to_owned();
                let _ = tokio::task::spawn_blocking(move || {
                    local::run(executable, env, &["logout"], &cwd)
                })
                .await;
            }
            Ok(())
        })
    }

    /// Its own installer puts it in `~/.grok`: `grok update`; npm's otherwise.
    fn program(&self) -> Option<Definition> {
        let env = self.environment();
        Some(Definition {
            name: "Grok's CLI".into(),
            executable: self.executable(&env)?.path().to_owned(),
            npm_package: Some("@xai-official/grok".into()),
            native: Some(Native {
                args: vec!["update".into()],
                markers: vec!["/.grok/".into()],
                dist_tag: None,
            }),
        })
    }

    fn credential(&self) -> Option<String> {
        let env = self.environment();
        Self::api_key(&env)
            .map(|key| format!("key:{}", local::fingerprint(&key)))
            .or_else(|| self.kept_key(&env).map(|key| local::fingerprint(&key)))
    }

    fn launch_key(&self, mode: &Mode, _directory: &Path, model: &str) -> io::Result<String> {
        Ok(json!({"mode": Self::mode_args(mode), "credential": self.credential(), "threshold": self.threshold(model)}).to_string())
    }

    fn launch(&self, mode: &Mode, _directory: &Path, model: &str) -> io::Result<Launch> {
        let mut env = self.environment();
        let executable = self.executable(&env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Install Grok's CLI first: {INSTALL}"),
            )
        })?;
        if let Some(percent) = self.threshold(model) {
            env.insert(
                "GROK_AUTO_COMPACT_THRESHOLD_PERCENT".into(),
                percent.to_string().into(),
            );
        }
        let (before, after) = Self::mode_args(mode);
        let mut command: Vec<PathBuf> = vec![executable.path().into()];
        command.extend(before.into_iter().map(Into::into));
        command.push("agent".into());
        command.extend(after.into_iter().map(Into::into));
        command.push("stdio".into());
        let scratch = self.home.run().join("agents/tmp");
        kuvo_store::files::private_folder(&scratch)?;
        Ok(Launch {
            start: Start {
                command,
                cwd: Some(scratch),
                env,
                kind: HoldKind::JsonRpc,
                meta: Metadata::new(),
                limits: Limits::default(),
                portable_requests: None,
            },
            temporary: None,
            sign_in_file: None,
            credential: self.credential(),
        })
    }

    fn initialize_meta(&self) -> Option<Value> {
        Some(json!({"clientType": "extension"}))
    }

    fn auth_method(&self, _: &Capabilities) -> Option<String> {
        Some(
            if Self::api_key(&self.environment()).is_some() {
                "xai.api_key"
            } else {
                "cached_token"
            }
            .into(),
        )
    }

    /// It can't go on after a stop.
    fn restarts_after_stop(&self) -> bool {
        true
    }

    fn select_model<'a>(
        &'a self,
        rpc: &'a Rpc,
        session: &'a str,
        model: &'a str,
        variant: Option<&'a str>,
        setup: &'a mut Setup,
        _credential: Option<&'a str>,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            if model != local::DEFAULT_MODEL && setup.get("models")["currentModelId"] != model {
                rpc.request(
                    "session/set_model",
                    Some(json!({"sessionId": session, "modelId": model})),
                    Some(std::time::Duration::from_secs(30)),
                )
                .await?;
                if let Some(models) = setup.0.get_mut("models").and_then(Value::as_object_mut) {
                    models.insert("currentModelId".into(), model.into());
                }
            }
            // Its reasoning effort: the one you picked, or (after one was) the model's own.
            let Some(option) = setup.option("reasoning_effort", "thought_level").cloned() else {
                return Ok(());
            };
            let described = setup.get("models")["availableModels"]
                .as_array()
                .and_then(|models| models.iter().find(|info| info["modelId"] == model))
                .and_then(|info| info["_meta"]["reasoningEffort"].as_str())
                .map(str::to_owned);
            let chosen = setup.get("kuvoEffort").as_str().map(str::to_owned);
            let wanted = match variant {
                Some(variant) => Some(variant.to_owned()),
                None if chosen.is_some() => {
                    described.or_else(|| setup.get("kuvoEffortBefore").as_str().map(str::to_owned))
                }
                None => None,
            };
            let key = format!("{model}:{}", variant.unwrap_or(""));
            let Some(wanted) = wanted else { return Ok(()) };
            if chosen.as_deref() == Some(key.as_str()) {
                return Ok(());
            }
            if chosen.is_none() && setup.get("kuvoEffortBefore").is_null() {
                setup.set("kuvoEffortBefore", option["currentValue"].clone());
            }
            let params = json!({"sessionId": session, "configId": option["id"], "value": wanted});
            rpc.request(
                "session/set_config_option",
                Some(params),
                Some(std::time::Duration::from_secs(30)),
            )
            .await?;
            if variant.is_some() {
                setup.set("kuvoEffort", key);
            } else {
                setup.0.remove("kuvoEffort");
            }
            Ok(())
        })
    }

    fn prompt_meta(&self) -> Option<Value> {
        let id = format!(
            "kuvo-prompt-{}",
            self.prompts.fetch_add(1, Ordering::Relaxed) + 1
        );
        Some(json!({"promptId": id, "requestId": id}))
    }

    fn extension(&self, method: &str, params: &Value) -> Option<Extension> {
        match method.trim_start_matches('_') {
            "x.ai/session/update" if params["update"]["sessionUpdate"] == "background_tasks" => {
                let tasks = params["update"]["tasks"].as_array()?;
                let text = |task: &Value, key: &str| task[key].as_str().map(str::to_owned);
                let tasks = tasks
                    .iter()
                    .filter(|task| task["task_id"].is_string() && task["status"] == "running")
                    .map(|task| Task {
                        id: text(task, "task_id").unwrap_or_default(),
                        command: text(task, "command"),
                        description: text(task, "description"),
                        kind: text(task, "kind"),
                        started: task["started_at"]
                            .as_str()
                            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                            .map(|at| at.as_millisecond().max(0) as u64),
                        output_file: text(task, "output_file"),
                    })
                    .collect();
                Some(Extension::Background {
                    session: params["sessionId"].as_str().map(str::to_owned),
                    tasks,
                    replayed: params["_meta"]["isReplay"] == true,
                })
            }
            "x.ai/session/prompt_complete" => {
                let prompt = params["promptId"].as_str().map(str::to_owned);
                let stop = params["stopReason"]
                    .as_str()
                    .unwrap_or("end_turn")
                    .to_owned();
                let error = match stop.as_str() {
                    "rate_limit" => Some("You've reached your Grok usage limit for now.".into()),
                    "error" => Some(
                        params["agentResult"]
                            .as_str()
                            .unwrap_or("Grok stopped with an error.")
                            .into(),
                    ),
                    _ => None,
                };
                let own = prompt
                    .as_deref()
                    .is_some_and(|prompt| prompt.starts_with("task-completed-"));
                Some(Extension::PromptComplete {
                    own,
                    stop,
                    error,
                    prompt,
                })
            }
            _ => None,
        }
    }

    fn own_request(&self, method: &str, params: &Value) -> Option<OwnRequest> {
        let call = if params["params"].is_object() {
            &params["params"]
        } else {
            params
        };
        match method.trim_start_matches('_') {
            "x.ai/ask_user_question" => Some(OwnRequest::Questions {
                tool_call: call["toolCallId"].as_str().map(str::to_owned),
                questions: call["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, question)| Question {
                        id: question["id"]
                            .as_str()
                            .map_or_else(|| format!("q{index}"), str::to_owned),
                        question: question["question"].as_str().unwrap_or("").into(),
                        header: question["header"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| Some(format!("Question {}", index + 1))),
                        options: question["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|option| {
                                Some((
                                    option["label"].as_str()?.to_owned(),
                                    option["description"].as_str().map(str::to_owned),
                                ))
                            })
                            .collect(),
                        multiple: question["multiSelect"] == true,
                    })
                    .collect(),
            }),
            "x.ai/exit_plan_mode" => Some(OwnRequest::ExitPlan {
                plan: ["plan", "planText", "content"].iter().find_map(|key| {
                    call[*key]
                        .as_str()
                        .filter(|plan| !plan.trim().is_empty())
                        .map(str::to_owned)
                }),
            }),
            _ => None,
        }
    }

    fn answer_own(&self, request: &OwnRequest, answer: OwnAnswer) -> Value {
        match request {
            OwnRequest::Questions { questions, .. } => {
                let (values, words) = match answer {
                    OwnAnswer::Answered(values) => (values, None),
                    OwnAnswer::Declined(None) => return json!({"outcome": "cancelled"}),
                    // (Turned down by its box with words: that's what it hears.)
                    OwnAnswer::Declined(words) => (Map::new(), words),
                };
                let answers: Map<String, Value> = questions
                    .iter()
                    .map(|question| {
                        let said = match values.get(&question.id) {
                            Some(Value::Array(list)) => list
                                .iter()
                                .map(|value| {
                                    value
                                        .as_str()
                                        .map_or_else(|| value.to_string(), str::to_owned)
                                })
                                .collect(),
                            Some(Value::Null) | None => words.clone().into_iter().collect(),
                            Some(value) => vec![
                                value
                                    .as_str()
                                    .map_or_else(|| value.to_string(), str::to_owned),
                            ],
                        };
                        (question.question.clone(), json!(said))
                    })
                    .collect();
                json!({"outcome": "accepted", "answers": answers})
            }
            OwnRequest::ExitPlan { .. } => {
                json!({"outcome": "abandoned", "feedback": "The plan is shown to the user. Stop here and wait: they'll switch out of plan mode and ask you to build it when they're ready."})
            }
        }
    }

    fn note_context(&self, model: &str, size: u64) {
        let changed = lock(&self.windows).insert(model.into(), size) != Some(size);
        if changed {
            self.told("model.updated");
        }
    }

    /// Keeps the models (and their efforts) a session listed, for the picker.
    fn note_setup(&self, setup: &Setup, credential: Option<&str>) {
        let models = setup.get("models");
        let listed: Vec<Listed> = models["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|info| {
                let id = info["modelId"].as_str()?.to_owned();
                let levels = info["_meta"]["reasoningEfforts"].as_array();
                let efforts = levels.map_or_else(
                    || EFFORTS.map(String::from).to_vec(),
                    |levels| {
                        levels
                            .iter()
                            .filter_map(|level| level["value"].as_str().map(str::to_owned))
                            .collect()
                    },
                );
                let effort = levels
                    .and_then(|levels| levels.iter().find(|level| level["default"] == true))
                    .and_then(|level| level["value"].as_str())
                    .or_else(|| info["_meta"]["reasoningEffort"].as_str())
                    .map(str::to_owned);
                Some(Listed {
                    name: info["name"].as_str().unwrap_or(&id).to_owned(),
                    id,
                    efforts,
                    effort,
                })
            })
            .collect();
        if listed.is_empty() {
            return;
        }
        let catalog = Catalog {
            credential: credential.map(str::to_owned),
            current: models["currentModelId"].as_str().map(str::to_owned),
            models: listed,
        };
        if catalog.write(&self.catalog_file()) {
            self.told("model.updated");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_device_code_it_prints() {
        assert_eq!(
            device_code("Enter the code ABCD-1234 at https://x.ai/device"),
            Some("ABCD-1234".into())
        );
        assert_eq!(device_code("no code here, just-words"), None);
    }

    #[test]
    fn its_mode_is_in_its_command() {
        assert_eq!(
            Grok::mode_args(&Mode {
                approve_all: true,
                read_only: false
            }),
            (vec![], vec!["--always-approve"])
        );
        assert_eq!(
            Grok::mode_args(&Mode {
                approve_all: true,
                read_only: true
            })
            .0,
            vec!["--permission-mode", "plan"]
        );
        assert_eq!(
            Grok::mode_args(&Mode::default()).0,
            vec!["--permission-mode", "default"]
        );
    }
}
