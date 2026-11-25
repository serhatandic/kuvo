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
    flavour::{Flavour, Launch, Mode},
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

    /// As its conversations last listed them: OpenCode isn't started to list them.
    fn models(&self, _refresh: bool) -> AdapterCall<'_, Models> {
        Box::pin(async move {
            let file = self.catalog_file();
            let catalog = tokio::task::spawn_blocking(move || Catalog::read(&file, Some(CATALOG)))
                .await
                .map_err(io::Error::other)?;
            Ok(match catalog {
                Some(catalog) => local::models(&catalog, |_| 0, str::to_owned),
                None => Models::default(),
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
                env: program.env.clone(),
                kind: HoldKind::JsonRpc,
                meta: Metadata::new(),
                limits: Limits::default(),
                portable_requests: None,
            },
            temporary: None,
            sign_in_file: None,
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

    /// `provider/model`, as OpenCode lists them.
    fn model_value(&self, provider: &str, id: &str) -> String {
        format!("{provider}/{id}")
    }

    fn reads_own_servers(&self) -> bool {
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
