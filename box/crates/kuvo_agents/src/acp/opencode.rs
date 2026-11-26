//! OpenCode, the pinned program Kuvo Code downloads (never one of your own), as a coding agent:
//! `opencode acp`, one process for every OpenCode conversation on the computer. Its accounts are
//! its own providers (`opencode-go`, `anthropic`…), its MCP servers the computer's settings it
//! reads itself, and it never offers its plan mode.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::future::BoxFuture;
use kuvo_os::platform::Environment;
use kuvo_store::home::Home;
use serde_json::{Map, Value, json};

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::AgentKind,
    catalog::Models,
    hold::{HoldKind, Limits, Metadata, Start},
    program_updates::Definition,
    rpc::Rpc,
    runtime::AdapterCall,
    sign_in::{Attempt, Request},
};

use super::{
    flavour::{Flavour, Launch, Mode, OwnAnswer, OwnRequest, Question},
    local::{self, Catalog, Listed},
    wire::{Capabilities, Setup},
};

/// What the box runs OpenCode as: the pinned program and its environment.
#[derive(Clone)]
pub struct OpenCodeProgram {
    pub executable: PathBuf,
    pub env: Environment,
}

/// Its catalog's sign-in: it has none of its own (its providers' are its settings').
const CATALOG: &str = "opencode";
/// Its own mode that only plans: never offered.
const PLAN: &str = "plan";
const WAIT: Option<Duration> = Some(Duration::from_secs(30));
/// OpenCode asks the box before the tools Ask First is about, and the box answers by the
/// conversation's rules, as the engine applies them (a project's own settings can't let them
/// through unasked). Its other tools keep OpenCode's own defaults, as on the engine.
const ASKS: &str = r#"{"permission":{"edit":"ask","write":"ask","patch":"ask","shell":"ask","bash":"ask","webfetch":"ask","external_directory":"ask"}}"#;

pub(super) struct OpenCode {
    home: Home,
    provider: String,
    program: Option<OpenCodeProgram>,
    feed: Option<crate::feed::Feed>,
}

impl OpenCode {
    pub fn new(
        home: Home,
        provider: String,
        program: Option<OpenCodeProgram>,
        feed: Option<crate::feed::Feed>,
    ) -> Self {
        Self {
            home,
            provider,
            program,
            feed,
        }
    }

    fn installed(&self) -> Option<&OpenCodeProgram> {
        self.program
            .as_ref()
            .filter(|program| program.executable.is_file())
    }

    /// Whether the picker has OpenCode's models without it: a list the box keeps of them, or its
    /// own data from a run before (the engine side's kept list, `kuvo_engine::models::kept`).
    fn listed_elsewhere(&self) -> bool {
        let data = std::env::var_os("KUVO_ENGINE_DATA")
            .filter(|value| !value.is_empty())
            .map_or_else(|| self.home.engine_data(), PathBuf::from);
        self.home.run().join("engine/catalog.json").is_file() || data.join("opencode.db").is_file()
    }

    fn catalog_file(&self) -> PathBuf {
        Catalog::file(&self.home, "opencode", &self.provider)
    }

    async fn set_option(rpc: &Rpc, session: &str, option: &str, value: &str) -> io::Result<()> {
        let params = json!({"sessionId": session, "configId": option, "value": value});
        rpc.request("session/set_config_option", Some(params), WAIT)
            .await?;
        Ok(())
    }
}

/// An option's current value.
fn current<'a>(setup: &'a Setup, id: &str) -> Option<&'a str> {
    setup.option(id, "")?["currentValue"].as_str()
}

fn not_set_up() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "Kuvo Code hasn't set OpenCode up on this computer yet.",
    )
}

impl Flavour for OpenCode {
    fn kind(&self) -> AgentKind {
        AgentKind::OpenCode
    }

    fn name(&self) -> &'static str {
        "OpenCode"
    }

    /// Ready once its program is here: its sign-ins are its providers', each its own.
    fn probe(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(async move {
            let installed = self.installed().is_some();
            Ok(AccountStatus {
                installed,
                signed_in: installed,
                label: None,
                identity: None,
                label_describes_method: false,
                message: (!installed).then(|| not_set_up().to_string()),
                ended: false,
                unchecked: false,
            })
        })
    }

    /// None of its own: its providers' models are the picker's as OpenCode lists them. Only
    /// where it never ran (nothing of it listed), the one it picks itself, as any agent before
    /// it has listed its models: choosing it starts OpenCode.
    fn models(&self, _refresh: bool) -> AdapterCall<'_, Models> {
        Box::pin(async move {
            let installed = self.installed().is_some();
            Ok(match installed && !self.listed_elsewhere() {
                true => local::provisional("OpenCode"),
                false => Models::default(),
            })
        })
    }

    fn sign_in(&self, _request: Request) -> AdapterCall<'_, std::sync::Arc<dyn Attempt>> {
        Box::pin(async {
            Err(io::Error::other(
                "OpenCode signs in to each of its providers, in Settings › Accounts.",
            ))
        })
    }

    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    /// Pinned with the box: updated with it, never on its own.
    fn program(&self) -> Option<Definition> {
        None
    }

    fn credential(&self) -> Option<String> {
        None
    }

    /// One process for every conversation: modes are each session's own.
    fn launch_key(&self, _mode: &Mode, _directory: &Path, _model: &str) -> io::Result<String> {
        Ok("opencode".into())
    }

    fn launch(&self, _mode: &Mode, _directory: &Path, _model: &str) -> io::Result<Launch> {
        let program = self.installed().ok_or_else(not_set_up)?;
        let scratch = self.home.run().join("agents/tmp");
        kuvo_store::files::private_folder(&scratch)?;
        Ok(Launch {
            start: Start {
                command: vec![program.executable.clone(), "acp".into()],
                cwd: Some(scratch),
                env: {
                    let mut env = program.env.clone();
                    env.insert("OPENCODE_CONFIG_CONTENT".into(), ASKS.into());
                    env
                },
                kind: HoldKind::JsonRpc,
                meta: Metadata::new(),
                limits: Limits::default(),
                portable_requests: None,
            },
            temporary: None,
            credential: None,
        })
    }

    /// Its questions as forms, and its summaries as they go.
    fn client_capabilities(&self) -> Map<String, Value> {
        json!({"elicitation": {"form": {}}, "session": {"compaction": {}}})
            .as_object()
            .cloned()
            .unwrap_or_default()
    }

    /// Its sign-in is a terminal's (`opencode auth login`): never asked for here.
    fn auth_method(&self, _: &Capabilities) -> Option<String> {
        None
    }

    fn resumes(&self) -> bool {
        true
    }

    /// `provider/model`, as OpenCode lists them (its own default: the one it picks).
    fn model_value(&self, provider: &str, id: &str) -> String {
        match provider == AgentKind::OpenCode.provider() {
            true => id.to_owned(),
            false => format!("{provider}/{id}"),
        }
    }

    fn reads_own_servers(&self) -> bool {
        true
    }

    fn answers_usage(&self) -> bool {
        true
    }

    fn titles_itself(&self) -> bool {
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
            if model == local::DEFAULT_MODEL {
                return Ok(());
            }
            if current(setup, "model") != Some(model) {
                Self::set_option(rpc, session, "model", model).await?;
                setup.set("kuvoModel", model);
            }
            // The effort picked, else the model's own.
            let effort = variant.unwrap_or("default");
            if setup.option("effort", "").is_some() && current(setup, "effort") != Some(effort) {
                Self::set_option(rpc, session, "effort", effort).await?;
            }
            Ok(())
        })
    }

    /// Never its plan mode: a session that's in it (from before) goes on building.
    fn select_mode<'a>(
        &'a self,
        rpc: &'a Rpc,
        session: &'a str,
        mode: &'a Mode,
        setup: &'a mut Setup,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            if mode.read_only {
                return Err(io::Error::other(
                    "OpenCode can't run read-only here yet.",
                ));
            }
            if current(setup, "mode") == Some(PLAN) {
                Self::set_option(rpc, session, "mode", "build").await?;
            }
            Ok(())
        })
    }

    /// Its question tool's form (`q0`, with `q0_custom` for words of your own): its questions.
    fn own_request(&self, method: &str, params: &Value) -> Option<OwnRequest> {
        let fields = params["requestedSchema"]["properties"].as_object();
        let opencode = method == "elicitation/create"
            && params["mode"] == "form"
            && fields.is_some_and(|fields| fields.keys().any(|key| key == "q0"));
        if !opencode {
            return None;
        }
        let questions = fields?
            .iter()
            .filter(|(key, _)| !key.ends_with("_custom"))
            .map(|(key, field)| {
                let multiple = field["type"] == "array";
                let choices = if multiple { &field["items"]["anyOf"] } else { &field["oneOf"] };
                Question {
                    id: key.clone(),
                    question: field["description"].as_str().unwrap_or_default().into(),
                    header: field["title"].as_str().map(str::to_owned),
                    options: choices
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|choice| {
                            let label = choice["const"].as_str()?.to_owned();
                            Some((label, choice["description"].as_str().map(str::to_owned)))
                        })
                        .collect(),
                    multiple,
                }
            })
            .collect();
        Some(OwnRequest::Questions {
            tool_call: params["toolCallId"].as_str().map(str::to_owned),
            questions,
        })
    }

    /// Your choices as its form takes them; words of your own in their `_custom` field.
    fn answer_own(&self, request: &OwnRequest, answer: OwnAnswer) -> Value {
        let (OwnRequest::Questions { questions, .. }, OwnAnswer::Answered(values)) = (request, answer)
        else {
            return json!({"action": "decline"});
        };
        let mut content = Map::new();
        for question in questions {
            let offered = |value: &Value| {
                question.options.iter().any(|(label, _)| Some(label.as_str()) == value.as_str())
            };
            let (chosen, own): (Vec<Value>, Vec<Value>) = match values.get(&question.id) {
                Some(Value::Array(list)) => list.iter().cloned().partition(offered),
                Some(value) if !value.is_null() => [value.clone()].into_iter().partition(offered),
                _ => continue,
            };
            if question.multiple {
                content.insert(question.id.clone(), Value::Array(chosen));
            } else if let Some(one) = chosen.into_iter().next() {
                content.insert(question.id.clone(), one);
            }
            let own: Vec<&str> = own.iter().filter_map(Value::as_str).collect();
            if !own.is_empty() {
                content.insert(format!("{}_custom", question.id), own.join(", ").into());
            }
        }
        json!({"action": "accept", "content": content})
    }

    /// Keeps the models a session listed (`provider/model`), for when OpenCode isn't running.
    fn note_setup(&self, setup: &Setup, _credential: Option<&str>) {
        let Some(option) = setup.option("model", "model") else {
            return;
        };
        let models: Vec<Listed> = option["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|choice| {
                let id = choice["value"].as_str()?.to_owned();
                Some(Listed {
                    name: choice["name"].as_str().unwrap_or(&id).to_owned(),
                    id,
                    efforts: Vec::new(),
                    effort: None,
                })
            })
            .collect();
        if models.is_empty() {
            return;
        }
        let catalog = Catalog {
            credential: Some(CATALOG.into()),
            current: option["currentValue"].as_str().map(str::to_owned),
            models,
        };
        if catalog.write(&self.catalog_file())
            && let Some(feed) = &self.feed
        {
            feed.emit("model.updated", json!({"program": "opencode"}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OpenCode agent in a home of its own, removed with the folder kept beside it.
    fn opencode() -> (tempfile::TempDir, OpenCode) {
        let home = tempfile::tempdir().unwrap();
        let agent = OpenCode::new(Home::new(home.path().to_owned()).unwrap(), "opencode-agent".into(), None, None);
        (home, agent)
    }

    /// The form OpenCode 2.2.6's question tool sends over ACP.
    fn asked() -> Value {
        json!({"mode": "form", "toolCallId": "call_1", "message": "Questions", "requestedSchema": {"type": "object", "properties": {
            "q0": {"type": "string", "title": "Size", "description": "Which size?", "oneOf": [
                {"const": "Small", "title": "Small", "description": "A little"}, {"const": "Large", "title": "Large", "description": "A lot"}]},
            "q0_custom": {"type": "string", "title": "Size (other)", "description": "Type your own answer"},
            "q1": {"type": "array", "title": "Colors", "description": "Which colors?", "items": {"anyOf": [
                {"const": "Red", "title": "Red"}, {"const": "Blue", "title": "Blue"}]}},
            "q1_custom": {"type": "string"}}, "required": []}})
    }

    #[test]
    fn its_question_form_is_its_questions_and_your_answers_go_back_in_its_fields() {
        let (_home, agent) = opencode();
        let Some(request @ OwnRequest::Questions { .. }) = agent.own_request("elicitation/create", &asked()) else {
            panic!("its questions");
        };
        let OwnRequest::Questions { tool_call, questions } = &request else { unreachable!() };
        assert_eq!(tool_call.as_deref(), Some("call_1"));
        assert_eq!(questions.len(), 2);
        assert_eq!((questions[0].id.as_str(), questions[0].question.as_str(), questions[0].multiple), ("q0", "Which size?", false));
        assert_eq!(questions[0].options[1], ("Large".into(), Some("A lot".into())));
        assert!(questions[1].multiple);
        let answers = json!({"q0": "Medium", "q1": ["Blue", "Green"]}).as_object().cloned().unwrap();
        assert_eq!(
            agent.answer_own(&request, OwnAnswer::Answered(answers)),
            json!({"action": "accept", "content": {"q0_custom": "Medium", "q1": ["Blue"], "q1_custom": "Green"}})
        );
        assert_eq!(agent.answer_own(&request, OwnAnswer::Declined(None)), json!({"action": "decline"}));
    }

    #[test]
    fn another_servers_form_isnt_its_question() {
        let form = json!({"mode": "form", "requestedSchema": {"properties": {"token": {"type": "string"}}}});
        let (_home, agent) = opencode();
        assert!(agent.own_request("elicitation/create", &form).is_none());
        assert!(agent.own_request("session/request_permission", &asked()).is_none());
    }

    #[test]
    fn its_models_are_named_provider_and_model_and_one_process_carries_every_conversation() {
        let (_home, agent) = opencode();
        assert_eq!(agent.model_value("opencode-go", "deepseek-v4.1-flash"), "opencode-go/deepseek-v4.1-flash");
        let (plain, approving) = (Mode::default(), Mode { approve_all: true, read_only: false });
        assert_eq!(
            agent.launch_key(&plain, Path::new("/a"), "x/y").unwrap(),
            agent.launch_key(&approving, Path::new("/b"), "z/w").unwrap()
        );
        assert!(agent.reads_own_servers());
    }

    #[test]
    fn it_asks_the_box_before_its_tools() {
        let asks: Value = serde_json::from_str(ASKS).unwrap();
        for tool in ["edit", "write", "patch", "shell", "bash", "webfetch", "external_directory"] {
            assert_eq!(asks["permission"][tool], "ask", "{tool}");
        }
        // (Its subagents, questions and the rest as OpenCode has them.)
        assert!(asks["permission"]["*"].is_null() && asks["permission"]["subagent"].is_null());
    }
}
