//! Antigravity: Google's `agy` CLI (`agy acp`), signed in with your Google account (`agy acp
//! --login`, the code its page ends on pasted back), in a home of Kuvo's own (GEMINI_HOME, never
//! your `~/.gemini`). An account signed in before `agy` keeps running on Google's earlier agent
//! (1.3.0, downloaded then) until it's signed in again.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as Plain},
    time::{Duration, SystemTime},
};

use futures_util::future::BoxFuture;
use kuvo_os::{
    platform::{Environment, Platform},
    process::Spawn,
};
use kuvo_store::home::Home;
use serde_json::{Value, json};

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::AgentKind,
    catalog::Models,
    hold::{Directory, HoldKind, Limits, Metadata, Start},
    program_updates::{Definition, Native},
    rpc::{Failure, Framing, Options, Rpc, RpcError},
    runtime::AdapterCall,
    session::{LimitScope, ToolOutput},
    sign_in::{Attempt, Mode as PageMode, Page, Request, Status, command as sign_in},
};

use super::{
    flavour::{Flavour, Launch, Line, Mode},
    local::{self, Catalog, Listed},
    server::lock,
    tools::{self, Call},
    wire::{Capabilities, PROTOCOL_VERSION, Setup},
};

const INSTALL: &str = "curl -fsSL https://antigravity.google/cli/install.sh | bash";
/// Google's earlier agent, kept for accounts signed in before `agy`.
const EARLIER: &str = "1.3.0";
const EARLIER_PROGRAM: &str = "agy_acp_server.par";
const EARLIER_HARNESS: &str = "localharness_external";
/// What it says on stdout when it wants you to sign in (not ACP).
const SIGN_IN_LINE: &str = "Open the following link to authenticate the ACP server:";
const METHOD: &str = "oauth-personal";
/// How much a model holds before it says (not all are Gemini's 1M).
const CONTEXT: u64 = 1_000_000;
/// Your other Google keys and settings, kept from it: it signs in its own way.
const SCRUBBED: [&str; 21] = [
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GOOGLE_GENAI_USE_VERTEXAI",
    "GCLOUD_PROJECT",
    "CLOUDSDK_CORE_PROJECT",
    "AGY_ACP_CCPA_PROJECT",
    "AGY_ACP_ENABLE_OAUTH",
    "GEMINI_HOME",
    "AGY_ACP_FORCE_FILE_STORAGE",
    "ANTIGRAVITY_HARNESS_PATH",
    "BROWSER",
    "PYTHONUNBUFFERED",
    "ELECTRON_RUN_AS_NODE",
    "AGY_CLI_DISABLE_AUTO_UPDATE",
    "SSH_CONNECTION",
    "KUVO_AGY_PROGRAM",
    "CHROME_CRASHPAD_PIPE_NAME",
];

/// Which program runs an account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Runtime {
    Agy,
    /// Google's earlier agent, for an account signed in before `agy`.
    Earlier,
}

pub(super) struct Antigravity {
    home: Home,
    provider: String,
    /// An added account's own folder.
    managed: Option<PathBuf>,
    /// The holder its checks run under.
    holder: PathBuf,
    feed: Option<crate::feed::Feed>,
    windows: Plain<HashMap<String, u64>>,
    /// Its sign-in file as it was when Google ended the sign-in: until it's new.
    ended: Plain<Option<Option<SystemTime>>>,
}

impl Antigravity {
    pub fn new(
        home: Home,
        provider: String,
        managed: Option<PathBuf>,
        holder: PathBuf,
        feed: Option<crate::feed::Feed>,
    ) -> Self {
        Self {
            home,
            provider,
            managed,
            holder,
            feed,
            windows: Plain::default(),
            ended: Plain::default(),
        }
    }

    /// Its folder among the agents' (where earlier boxes kept it too).
    fn root(&self) -> PathBuf {
        self.home.root().join("agents").join("antigravity")
    }

    fn earlier_folder(&self) -> PathBuf {
        self.root().join(EARLIER)
    }

    /// Its home (GEMINI_HOME): its sign-in, settings and conversations.
    fn profile(&self) -> PathBuf {
        self.managed
            .as_ref()
            .map_or_else(|| self.root().join("profile"), |home| home.join("profile"))
    }

    fn acp_folder(&self) -> PathBuf {
        self.profile().join("antigravity-acp")
    }

    /// The earlier agent's sign-in (Google's 1.3 kept it in a file).
    fn token_file(&self) -> PathBuf {
        self.acp_folder().join("acp_token.json")
    }

    /// `agy`'s sign-in here, once `agy` itself said it's signed in: an ID of Kuvo's own for it.
    fn sign_in_file(&self) -> PathBuf {
        self.acp_folder().join("kuvo-sign-in.json")
    }

    fn models_file(&self) -> PathBuf {
        self.profile().join("kuvo-models.json")
    }

    fn executable(&self, env: &Environment) -> Option<kuvo_os::platform::Executable> {
        local::program("agy", &[".local/bin"], env)
    }

    fn agy_sign_in(&self) -> Option<String> {
        let saved: Value =
            serde_json::from_slice(&std::fs::read(self.sign_in_file()).ok()?).ok()?;
        saved["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    }

    /// Signed in before `agy`, on Google's earlier agent, which still runs it until you sign in
    /// again.
    fn earlier(&self) -> bool {
        self.agy_sign_in().is_none()
            && self.token_file().exists()
            && self.earlier_folder().join(".complete").exists()
    }

    fn runtime(&self) -> Runtime {
        if self.earlier() {
            Runtime::Earlier
        } else {
            Runtime::Agy
        }
    }

    /// The earlier agent's token, by what it keeps: its refresh token, or its access token.
    fn earlier_token(&self) -> Option<String> {
        let raw: Value = serde_json::from_slice(&std::fs::read(self.token_file()).ok()?).ok()?;
        let source = if raw["token"].is_object() {
            &raw["token"]
        } else {
            &raw
        };
        let text = |keys: [&str; 2]| {
            keys.iter().find_map(|key| {
                source[*key]
                    .as_str()
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            })
        };
        text(["refresh_token", "refreshToken"]).or_else(|| text(["access_token", "accessToken"]))
    }

    fn signed_in_mark(&self) -> Option<SystemTime> {
        let file = if self.runtime() == Runtime::Earlier {
            self.token_file()
        } else {
            self.sign_in_file()
        };
        std::fs::metadata(file)
            .and_then(|meta| meta.modified())
            .ok()
    }

    fn told(&self, kind: &str) {
        if let Some(feed) = &self.feed {
            feed.emit(kind, json!({"integrationID": self.provider}));
        }
    }

    /// The model list kept in its home (`kuvo-models.json`, as earlier boxes kept it), for the
    /// sign-in it belongs to.
    fn saved_models(&self) -> Option<Catalog> {
        let saved: Value = serde_json::from_slice(&std::fs::read(self.models_file()).ok()?).ok()?;
        let credential = self.credential()?;
        if saved["credential"] != credential.as_str() {
            return None;
        }
        let models = saved["models"]
            .as_array()?
            .iter()
            .map(|info| {
                Some(Listed {
                    id: info["value"].as_str().filter(|id| !id.is_empty())?.into(),
                    name: info["name"].as_str()?.into(),
                    efforts: Vec::new(),
                    effort: None,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Catalog {
            credential: Some(credential),
            current: saved["current"].as_str().map(str::to_owned),
            models,
        })
    }

    /// Keeps the models a session offered, for the sign-in it ran with: whether it listed them.
    fn save_models(&self, setup: &Setup, credential: Option<&str>) -> bool {
        let (Some(credential), Some(now)) = (credential, self.credential()) else {
            return false;
        };
        if credential != now {
            return false;
        }
        let option = setup.option("model", "model");
        let mut flat: Vec<(String, String)> = Vec::new();
        for entry in option
            .and_then(|option| option["options"].as_array())
            .into_iter()
            .flatten()
        {
            match entry["options"].as_array() {
                Some(inner) => flat.extend(inner.iter().filter_map(|inner| named(inner, "value"))),
                None => flat.extend(named(entry, "value")),
            }
        }
        if flat.is_empty() {
            flat.extend(
                setup.get("models")["availableModels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|info| named(info, "modelId")),
            );
        }
        let empty = |value: &Value| value.as_array().is_some_and(Vec::is_empty);
        let listed = !flat.is_empty()
            || empty(&setup.get("models")["availableModels"])
            || option.is_some_and(|option| empty(&option["options"]));
        if !listed {
            return false;
        }
        let models: Vec<Value> = flat
            .iter()
            .map(|(value, name)| json!({"value": value, "name": name}))
            .collect();
        let saved = json!({"credential": credential, "current": option.map(|option| option["currentValue"].clone()), "models": models});
        let file = self.models_file();
        let changed = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            != Some(saved.clone());
        if changed && kuvo_store::files::write_atomic(&file, saved.to_string().as_bytes()).is_ok() {
            self.told("model.updated");
        }
        true
    }

    /// How it's started: its own home and temporary folder (it unpacks itself there), none of
    /// your other Google keys, its sign-in kept in a file (it thinks it runs over SSH), and no
    /// updating itself unless agents may.
    fn launch_for(&self, runtime: Runtime) -> io::Result<Launch> {
        let acp = self.acp_folder();
        kuvo_store::files::private_folder(&acp.join("tmp"))?;
        let settings = acp.join("settings.json");
        if !settings.exists() {
            kuvo_store::files::write_atomic(&settings, br#"{"auth":{"type":"oauth-personal"}}"#)?;
        }
        let mut random = [0; 8];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let temporary = self
            .home
            .run()
            .join("agents/tmp/antigravity")
            .join(format!("att_{}", hex::encode(random)));
        kuvo_store::files::private_folder(&temporary)?;
        let catcher = sign_in::catcher_script(&temporary)?;
        let mut env: Environment = local::environment();
        env.retain(|name, _| {
            !SCRUBBED.contains(&name.to_string_lossy().to_ascii_uppercase().as_str())
        });
        let url = temporary.join("url");
        for (name, value) in [
            ("GEMINI_HOME", self.profile()),
            ("BROWSER", catcher),
            ("KUVO_URL_FILE", url.clone()),
            ("TMPDIR", temporary.clone()),
        ] {
            env.insert(name.into(), value.into_os_string());
        }
        let command: Vec<PathBuf> = match runtime {
            Runtime::Earlier => {
                let folder = self.earlier_folder();
                env.insert("AGY_ACP_FORCE_FILE_STORAGE".into(), "1".into());
                env.insert("PYTHONUNBUFFERED".into(), "1".into());
                env.insert(
                    "ANTIGRAVITY_HARNESS_PATH".into(),
                    folder.join(EARLIER_HARNESS).into(),
                );
                let mut command = vec![folder.join(EARLIER_PROGRAM), "--nosyslog_on_start".into()];
                if Platform::current() == Platform::Linux {
                    command.push("--uid=".into());
                }
                command
            }
            Runtime::Agy => {
                let Some(agy) = self.executable(&env) else {
                    let _ = std::fs::remove_dir_all(&temporary);
                    return Err(not_installed());
                };
                let real =
                    std::fs::canonicalize(agy.path()).unwrap_or_else(|_| agy.path().to_owned());
                env.insert("SSH_CONNECTION".into(), "127.0.0.1 0 127.0.0.1 0".into());
                env.insert("KUVO_AGY_PROGRAM".into(), real.into());
                if !local::agents_update_themselves(&self.home) {
                    env.insert("AGY_CLI_DISABLE_AUTO_UPDATE".into(), "1".into());
                }
                vec![
                    agy.path().into(),
                    "acp".into(),
                    "--gemini-dir".into(),
                    self.profile(),
                    "--app-data-dir".into(),
                    "antigravity-acp".into(),
                ]
            }
        };
        Ok(Launch {
            start: Start {
                command,
                cwd: Some(temporary.clone()),
                env,
                kind: HoldKind::JsonRpc,
                meta: Metadata::new(),
                limits: Limits::default(),
                portable_requests: None,
            },
            temporary: Some(temporary),
            sign_in_file: Some(url),
            credential: self.credential(),
        })
    }

    /// Asks `agy` to use its sign-in and list this account's models: signed in here only then.
    async fn verify_sign_in(&self) -> io::Result<()> {
        let launch = self.launch_for(Runtime::Agy)?;
        let temporary = launch.temporary.clone();
        let cwd = launch.start.cwd.clone().unwrap_or_default();
        let result = async {
            let controller = Directory::for_home(&self.home).start(&self.holder, launch.start).await?;
            let (rpc, _incoming) = Rpc::over(controller, Options { framing: Framing::JsonRpc2, first_id: 1, filter: Some(Box::new(|line: &str| !line.contains(SIGN_IN_LINE))) });
            let checked = async {
                let wait = Some(Duration::from_secs(120));
                let init = json!({"protocolVersion": PROTOCOL_VERSION, "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": false}, "clientInfo": {"name": "kuvo-code", "version": "1"}});
                rpc.request("initialize", Some(init), wait).await?;
                rpc.request("authenticate", Some(json!({"methodId": METHOD})), wait).await?;
                rpc.request("session/new", Some(json!({"cwd": cwd, "mcpServers": []})), wait).await
            }
            .await;
            rpc.stop().await;
            let setup = checked.map_err(|failure| match failure {
                Failure::Refused(error) if error.message.contains("SUBSCRIPTION_REQUIRED") => io::Error::other("Google says this account needs an Antigravity subscription."),
                Failure::Refused(error) => io::Error::other(format!("Antigravity didn't take the sign-in: {}", error.message)),
                Failure::Closed(said) => {
                    eprintln!("Google's Antigravity agent stopped while signing in:\n{said}");
                    io::Error::other(stopped_words(&said))
                }
                other => io::Error::from(other),
            })?;
            if let Some(session) = setup["sessionId"].as_str() {
                self.remove_session_files(session);
            }
            let mut random = [0; 8];
            getrandom::fill(&mut random).map_err(io::Error::other)?;
            let id = format!("agy_{}", hex::encode(random));
            let saved = json!({"id": id, "at": crate::background::now()});
            kuvo_store::files::write_atomic(&self.sign_in_file(), saved.to_string().as_bytes())?;
            // (Google's earlier agent's sign-in is behind it now.)
            let _ = std::fs::remove_file(self.token_file());
            self.save_models(&Setup::read(&setup), Some(&local::fingerprint(&id)));
            Ok(())
        }
        .await;
        if let Some(temporary) = temporary {
            let _ = std::fs::remove_dir_all(temporary);
        }
        result
    }

    /// A session made only to check the sign-in leaves nothing behind.
    fn remove_session_files(&self, session: &str) {
        let plain = !session.is_empty()
            && session.len() <= 128
            && session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !plain {
            return;
        }
        let acp = self.acp_folder();
        for suffix in [".db", ".db-wal", ".db-shm", ".db-journal", ".meta"] {
            let _ =
                std::fs::remove_file(acp.join("conversations").join(format!("{session}{suffix}")));
        }
        let _ = std::fs::remove_dir_all(acp.join("brain").join(session));
    }

    fn sign_in_options(&self, holder_check: Arc<Self>) -> io::Result<sign_in::Options> {
        let launch = self.launch_for(Runtime::Agy)?;
        let temporary = launch.temporary.clone();
        let mut env = launch.start.env;
        for name in ["DISPLAY", "WAYLAND_DISPLAY"] {
            env.remove(std::ffi::OsStr::new(name));
        }
        let mut command = launch.start.command.into_iter();
        let program = command.next().ok_or_else(not_installed)?;
        let mut spawn = Spawn::new(Path::new(&program), env);
        spawn.args = command
            .map(PathBuf::into_os_string)
            .chain(["--login".into()])
            .collect();
        spawn.cwd = launch.start.cwd;
        let scratch = self.home.run().join("agents/tmp");
        let handle = tokio::runtime::Handle::current();
        let feed = self.feed.clone();
        let provider = self.provider.clone();
        Ok(sign_in::Options {
            spawn,
            temporary: scratch,
            printed_page: true,
            code_prompt: Some("press Enter:"),
            instructions: "Sign in with Google on its page, then paste the code it gives you here.",
            code_instructions: "Sign in with Google on its page, then paste the code it gives you here.",
            user_code: None,
            confirm: Box::new(move || {
                let verified = handle.block_on(holder_check.verify_sign_in());
                if let Some(temporary) = temporary {
                    let _ = std::fs::remove_dir_all(temporary);
                }
                match verified {
                    Ok(()) => {
                        *lock(&holder_check.ended) = None;
                        true
                    }
                    Err(error) => {
                        eprintln!("Antigravity didn't take the sign-in: {error}");
                        false
                    }
                }
            }),
            completed: Box::new(move || {
                if let Some(feed) = feed {
                    feed.emit("model.updated", json!({}));
                    feed.emit("integration.updated", json!({"integrationID": provider}));
                }
            }),
        })
    }

    /// `agy` forgets its sign-in itself (its ACP `logout`), then what Kuvo kept of it goes.
    async fn forget(&self) {
        if self.agy_sign_in().is_some()
            && self.executable(&local::environment()).is_some()
            && let Ok(launch) = self.launch_for(Runtime::Agy)
        {
            let temporary = launch.temporary.clone();
            if let Ok(controller) = Directory::for_home(&self.home)
                .start(&self.holder, launch.start)
                .await
            {
                let (rpc, _incoming) = Rpc::over(
                    controller,
                    Options {
                        framing: Framing::JsonRpc2,
                        first_id: 1,
                        filter: None,
                    },
                );
                let init = json!({"protocolVersion": PROTOCOL_VERSION, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}, "clientInfo": {"name": "kuvo-code", "version": "1"}});
                if let Ok(init) = rpc
                    .request("initialize", Some(init), Some(Duration::from_secs(60)))
                    .await
                    && Capabilities::read(&init).logout
                    && let Err(failure) = rpc
                        .request("logout", Some(json!({})), Some(Duration::from_secs(90)))
                        .await
                {
                    eprintln!("Antigravity couldn't sign out: {failure}");
                }
                rpc.stop().await;
            }
            if let Some(temporary) = temporary {
                let _ = std::fs::remove_dir_all(temporary);
            }
        }
        for file in [self.sign_in_file(), self.token_file(), self.models_file()] {
            let _ = std::fs::remove_file(file);
        }
        self.told("integration.updated");
    }
}

fn named(entry: &Value, key: &str) -> Option<(String, String)> {
    let value = entry[key].as_str().filter(|value| !value.is_empty())?;
    Some((value.into(), entry["name"].as_str().unwrap_or(value).into()))
}

fn not_installed() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "Antigravity needs Google's agy CLI on this computer. Install it in Settings › Accounts ({INSTALL}), then sign in."
        ),
    )
}

/// Why Google's agent stopped while signing in, in a line (what it said goes to the log).
fn stopped_words(said: &str) -> String {
    let last = said.trim().lines().last().unwrap_or("");
    let refused = [
        "PermissionError",
        "Operation not permitted",
        "Permission denied",
        "EACCES",
        "EPERM",
    ];
    if refused.iter().any(|word| last.contains(word)) {
        "Google's Antigravity agent stopped as it started: it wasn't allowed into a folder it needs.".into()
    } else {
        "Google's Antigravity agent stopped before signing in finished.".into()
    }
}

/// A model's name as the picker shows it: its Claude and GPT models named apart from Claude
/// Code's and Codex's own; Gemini's as they are.
fn model_name(name: &str) -> String {
    if name.to_ascii_lowercase().contains("gemini") || name.ends_with("(Antigravity)") {
        name.into()
    } else {
        format!("{name} (Antigravity)")
    }
}

/// Its whole answer is "Usage Limit Reached. You have reached your current quota for this period.
/// Your limit will reset in 6 days, 19 hours.": the plan's limit.
fn limit_reply(text: &str) -> bool {
    let plain: String = text
        .chars()
        .map(|c| if "#*_>`".contains(c) { ' ' } else { c })
        .collect();
    let plain = plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let plain = plain.trim_end_matches('.');
    let Some(rest) = plain.strip_prefix("usage limit reached") else {
        return false;
    };
    let rest = rest.trim_start_matches('.').trim_start();
    let Some(rest) = rest.strip_prefix("you have reached your current quota for this period")
    else {
        return false;
    };
    let rest = rest.trim_start_matches('.').trim_start();
    rest.is_empty()
        || rest
            .strip_prefix("your limit will reset in ")
            .is_some_and(|when| !when.is_empty() && when.len() <= 80 && !when.contains('.'))
}

impl Flavour for Antigravity {
    fn kind(&self) -> AgentKind {
        AgentKind::Antigravity
    }

    fn name(&self) -> &'static str {
        "Antigravity"
    }

    fn probe(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(async move {
            let installed = self.executable(&local::environment()).is_some();
            let status = |installed, signed_in, message: Option<&str>| AccountStatus {
                installed,
                signed_in,
                label: signed_in.then(|| "Google account".into()),
                identity: signed_in.then(|| self.credential()).flatten(),
                label_describes_method: signed_in,
                message: message.map(str::to_owned),
                ended: false,
                unchecked: false,
            };
            let ended = *lock(&self.ended);
            if let Some(mark) = ended {
                if self.signed_in_mark() == mark {
                    return Ok(AccountStatus {
                        ended: true,
                        ..status(
                            installed || self.earlier(),
                            false,
                            Some("Google ended this sign-in. Sign in again."),
                        )
                    });
                }
                *lock(&self.ended) = None;
            }
            // Signed in before `agy`. With `agy` here, it's signed in only once `agy` takes that
            // sign-in (signing in tries that first, then Google's page); without it, it runs on
            // Google's earlier agent until agy is installed.
            if self.earlier() {
                return Ok(if installed {
                    status(
                        true,
                        false,
                        Some("Sign in to move Antigravity to Google's agy."),
                    )
                } else {
                    status(
                        false,
                        true,
                        Some(
                            "Antigravity runs on Google's earlier agent here. Install agy, then sign in to move it there.",
                        ),
                    )
                });
            }
            Ok(match (installed, self.agy_sign_in().is_some()) {
                (false, _) => status(
                    false,
                    false,
                    Some("Install Antigravity's CLI, then sign in here."),
                ),
                (true, false) => status(true, false, Some("Sign in with your Google account.")),
                (true, true) => status(true, true, None),
            })
        })
    }

    /// As its home keeps them, listed when it was signed in or a session last opened: never by
    /// starting it.
    fn models(&self, _refresh: bool) -> AdapterCall<'_, Models> {
        Box::pin(async move {
            let windows = lock(&self.windows).clone();
            let context = |model: &str| windows.get(model).copied().unwrap_or(CONTEXT);
            Ok(match self.saved_models() {
                Some(catalog) => local::models(&catalog, context, model_name),
                None => local::provisional("Antigravity"),
            })
        })
    }

    fn sign_in(&self, _request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        Box::pin(async move {
            let checker = Arc::new(Self::new(
                self.home.clone(),
                self.provider.clone(),
                self.managed.clone(),
                self.holder.clone(),
                self.feed.clone(),
            ));
            if self.executable(&local::environment()).is_none() {
                return Err(not_installed());
            }
            // Signed in before `agy`: `agy` is asked to take that sign-in first.
            if self.earlier() {
                return Ok(Takeover::start(checker));
            }
            let options = self.sign_in_options(checker)?;
            sign_in::start(options).await
        })
    }

    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            self.forget().await;
            Ok(())
        })
    }

    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            self.forget().await;
            Ok(())
        })
    }

    /// Google's own updater (`agy update`).
    fn program(&self) -> Option<Definition> {
        Some(Definition {
            name: "Antigravity's CLI".into(),
            executable: self.executable(&local::environment())?.path().to_owned(),
            npm_package: None,
            native: Some(Native {
                args: vec!["update".into()],
                markers: vec![String::new()],
                dist_tag: None,
            }),
        })
    }

    fn credential(&self) -> Option<String> {
        self.agy_sign_in()
            .or_else(|| self.earlier_token())
            .map(|secret| local::fingerprint(&secret))
    }

    fn launch_key(&self, _mode: &Mode, _directory: &Path, _model: &str) -> io::Result<String> {
        Ok(
            json!({"runtime": format!("{:?}", self.runtime()), "credential": self.credential()})
                .to_string(),
        )
    }

    fn launch(&self, _mode: &Mode, _directory: &Path, _model: &str) -> io::Result<Launch> {
        if self.executable(&local::environment()).is_none() && !self.earlier() {
            return Err(not_installed());
        }
        if !self.earlier() && self.agy_sign_in().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Sign in to Antigravity in Settings › Accounts first.",
            ));
        }
        self.launch_for(self.runtime())
    }

    fn version(&self) -> Option<String> {
        Some(match self.runtime() {
            Runtime::Agy => "agy".into(),
            Runtime::Earlier => EARLIER.into(),
        })
    }

    fn file_system(&self) -> bool {
        true
    }

    fn reads_unsaid(&self) -> bool {
        true
    }

    fn auth_method(&self, _: &Capabilities) -> Option<String> {
        Some(METHOD.into())
    }

    fn authenticates_first(&self) -> bool {
        true
    }

    fn resumes(&self) -> bool {
        true
    }

    /// Google's quotas are each model's own.
    fn limit_scope(&self) -> LimitScope {
        LimitScope::Model
    }

    /// Its access lasts an hour, renewed from the sign-in it keeps: one that asks for a new
    /// sign-in partway is started again, which renews it.
    fn line(&self, line: &str) -> Line {
        if line.contains(SIGN_IN_LINE) {
            Line::AsksSignIn
        } else {
            Line::Protocol
        }
    }

    /// `agy` asked to use a sign-in it no longer has.
    fn sign_in_refused(&self, error: &RpcError) -> bool {
        error.code == -32000
            || error.message.contains("No browser is available here")
            || error.message.contains("Authentication required")
    }

    fn sign_in_ended(&self) {
        *lock(&self.ended) = Some(self.signed_in_mark());
        self.told("integration.updated");
    }

    fn limit_reply(&self, text: &str) -> bool {
        limit_reply(text)
    }

    fn select_model<'a>(
        &'a self,
        rpc: &'a Rpc,
        session: &'a str,
        model: &'a str,
        _variant: Option<&'a str>,
        setup: &'a mut Setup,
        credential: Option<&'a str>,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            if model == local::DEFAULT_MODEL || setup.get("kuvoModel") == model {
                return Ok(());
            }
            let params = json!({"sessionId": session, "configId": "model", "value": model});
            rpc.request(
                "session/set_config_option",
                Some(params),
                Some(Duration::from_secs(30)),
            )
            .await?;
            setup.set("kuvoModel", model);
            self.save_models(setup, credential);
            Ok(())
        })
    }

    /// Its yolo mode for Approve All, default otherwise (its plan mode only for the box's own
    /// read-only runs). One that can't
    /// be made to ask, while it's in yolo, doesn't start.
    fn select_mode<'a>(
        &'a self,
        rpc: &'a Rpc,
        session: &'a str,
        mode: &'a Mode,
        setup: &'a mut Setup,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let option = setup.option("mode", "mode").cloned();
            let values: Vec<String> = option
                .as_ref()
                .and_then(|option| option["options"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["value"].as_str().map(str::to_owned))
                .collect();
            let wanted = if mode.read_only && values.iter().any(|value| value == "plan") {
                "plan"
            } else if mode.approve_all {
                "yolo"
            } else {
                "default"
            };
            let current = setup
                .get("kuvoMode")
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    option
                        .as_ref()
                        .and_then(|option| option["currentValue"].as_str().map(str::to_owned))
                });
            if current.as_deref() == Some(wanted) {
                return Ok(());
            }
            let cant_ask = || {
                io::Error::other(
                    "Antigravity couldn't be set to ask before it acts, so it didn't start. Try again, or pick Approve All for this session.",
                )
            };
            let Some(option) =
                option.filter(|_| values.is_empty() || values.iter().any(|value| value == wanted))
            else {
                return if wanted != "yolo" && current.as_deref() == Some("yolo") {
                    Err(cant_ask())
                } else {
                    Ok(())
                };
            };
            let params = json!({"sessionId": session, "configId": option["id"], "value": wanted});
            if rpc
                .request(
                    "session/set_config_option",
                    Some(params),
                    Some(Duration::from_secs(30)),
                )
                .await
                .is_err()
            {
                return if wanted != "yolo" {
                    Err(cant_ask())
                } else {
                    Ok(())
                };
            }
            setup.set("kuvoMode", wanted);
            Ok(())
        })
    }

    /// Its start of subagents, which its own record shows.
    fn hides(&self, call: &Call) -> bool {
        let named = format!(
            "{} {}",
            tools::title(call),
            call.get("rawInput")
                .map_or("", |raw| raw["name"].as_str().unwrap_or(""))
        );
        named
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|word| word == "start_subagent" || word == "invoke_subagent")
    }

    /// An image it made is saved in its folder for the conversation, not handed back: the newest
    /// by the name it was given, or made since the turn began.
    fn made_files(&self, call: &Call, session: &str, since_ms: u64) -> Vec<ToolOutput> {
        if !tools::title(call)
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|word| word == "generate_image")
        {
            return Vec::new();
        }
        let raw = call.get("rawInput").cloned().unwrap_or(Value::Null);
        generated_image(
            &self.acp_folder().join("brain").join(session),
            &raw,
            since_ms,
        )
        .into_iter()
        .collect()
    }

    fn note_context(&self, model: &str, size: u64) {
        if lock(&self.windows).insert(model.into(), size) != Some(size) {
            self.told("model.updated");
        }
    }

    fn note_setup(&self, setup: &Setup, credential: Option<&str>) {
        self.save_models(setup, credential);
    }

    fn record_mark(&self, session: &str) -> Option<i64> {
        Some(
            self.steps(session, false)
                .iter()
                .map(|step| step.index)
                .max()
                .unwrap_or(-1),
        )
    }

    /// Its durable record can finish a prompt whose ACP answer never comes: the latest step, after
    /// this prompt's input, is a finished answer with no tools whose words are the ones it said.
    fn record_answer(
        &self,
        session: &str,
        mark: i64,
        sent_ms: u64,
        said: &str,
        said_length: usize,
    ) -> Option<i64> {
        if said_length == 0 {
            return None;
        }
        let steps = self.steps(session, false);
        let input = steps
            .iter()
            .rev()
            .find(|step| step.kind == "USER_INPUT" && step.index > mark)?;
        let sent_second = sent_ms / 1000 * 1000;
        let created = input
            .created
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .map(|at| at.as_millisecond());
        if created.is_none_or(|at| at < sent_second as i64) {
            return None;
        }
        let latest = steps.iter().max_by_key(|step| step.index)?;
        if latest.index <= input.index
            || latest.kind != "PLANNER_RESPONSE"
            || latest.status != "DONE"
            || latest.tools
        {
            return None;
        }
        let content = if latest.truncated {
            self.steps(session, true)
                .into_iter()
                .find(|step| step.index == latest.index)?
                .content
        } else {
            latest.content.clone()
        };
        (content.len() == said_length && content.ends_with(said)).then_some(latest.index)
    }
}

/// Signing in an account from before `agy`: `agy` is asked to use the sign-in it has; only if it
/// can't does Google's page come, as for any sign-in.
struct Takeover {
    checking: Plain<Status>,
    page: Plain<Option<Arc<dyn Attempt>>>,
    cancelled: std::sync::atomic::AtomicBool,
}

impl Takeover {
    fn start(agent: Arc<Antigravity>) -> Arc<dyn Attempt> {
        let page = Page {
            url: String::new(),
            instructions: "Checking your Google sign-in with agy.".into(),
            mode: PageMode::Auto,
            accepts_code: true,
            user_code: None,
        };
        let takeover = Arc::new(Self {
            checking: Plain::new(Status::Pending { page }),
            page: Plain::default(),
            cancelled: Default::default(),
        });
        let own = takeover.clone();
        tokio::spawn(async move {
            let Err(error) = agent.verify_sign_in().await else {
                *lock(&own.checking) = Status::Complete;
                agent.told("model.updated");
                agent.told("integration.updated");
                return;
            };
            eprintln!("agy didn't take Antigravity's earlier sign-in: {error}");
            if own.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let started = match agent.sign_in_options(agent.clone()) {
                Ok(options) => sign_in::start(options).await,
                Err(error) => Err(error),
            };
            match started {
                Ok(page) => *lock(&own.page) = Some(page),
                Err(error) => {
                    *lock(&own.checking) = Status::Failed {
                        message: error.to_string(),
                    }
                }
            }
        });
        takeover
    }
}

impl Attempt for Takeover {
    fn status(&self) -> Status {
        match lock(&self.page).as_ref() {
            Some(page) => page.status(),
            None => lock(&self.checking).clone(),
        }
    }

    fn submit_code(&self, code: String) -> AdapterCall<'_, ()> {
        let page = lock(&self.page).clone();
        Box::pin(async move {
            match page {
                Some(page) => page.submit_code(code).await,
                None => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Google's page isn't ready yet: wait for it, then paste its code.",
                )),
            }
        })
    }

    fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(page) = lock(&self.page).as_ref() {
            page.cancel();
        }
    }
}

/// One step of Antigravity's record of a conversation (`brain/<id>/.system_generated/logs`).
struct Step {
    index: i64,
    kind: String,
    status: String,
    created: Option<String>,
    content: String,
    tools: bool,
    truncated: bool,
}

impl Antigravity {
    fn steps(&self, session: &str, full: bool) -> Vec<Step> {
        let name = if full {
            "transcript_full.jsonl"
        } else {
            "transcript.jsonl"
        };
        let file = self
            .acp_folder()
            .join("brain")
            .join(session)
            .join(".system_generated/logs")
            .join(name);
        let Ok(text) = std::fs::read_to_string(file) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| {
                let step: Value = serde_json::from_str(line).ok()?;
                Some(Step {
                    index: step["step_index"].as_i64()?,
                    kind: step["type"].as_str().unwrap_or("").into(),
                    status: step["status"].as_str().unwrap_or("").into(),
                    created: step["created_at"].as_str().map(str::to_owned),
                    content: step["content"].as_str().unwrap_or("").into(),
                    tools: step["tool_calls"]
                        .as_array()
                        .is_some_and(|calls| !calls.is_empty()),
                    truncated: step["truncated_fields"]
                        .as_array()
                        .is_some_and(|fields| fields.iter().any(|field| field == "content")),
                })
            })
            .collect()
    }
}

/// The image a generate_image call saved (`<image_name>_<time>.jpg`), the newest; one saved since
/// the turn began when it isn't named.
fn generated_image(folder: &Path, input: &Value, since_ms: u64) -> Option<ToolOutput> {
    let stem = [&input["image_name"], &input["ImageName"]]
        .into_iter()
        .find_map(Value::as_str)
        .map(|stem| serde_json::from_str::<String>(stem).unwrap_or_else(|_| stem.to_owned()))
        .unwrap_or_default();
    let mime = |name: &str| match name.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    };
    let since = SystemTime::UNIX_EPOCH + Duration::from_millis(since_ms);
    let newest = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let mime = mime(&name)?;
            if !stem.is_empty()
                && !name.starts_with(&format!("{stem}_"))
                && !name.starts_with(&format!("{stem}."))
            {
                return None;
            }
            let at = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (!stem.is_empty() || at >= since).then_some((at, name, mime))
        })
        .max_by_key(|(at, _, _)| *at)?;
    let (_, name, mime) = newest;
    Some(ToolOutput::File {
        uri: crate::pictures::file_url(&folder.join(&name).to_string_lossy()),
        mime: mime.into(),
        name: Some(name),
    })
}

/// Releases of the earlier agent installed beside the one kept: each is removed once that one is
/// whole and no running holder may still use it.
pub async fn forget_old_releases(home: &Home) {
    let held: Vec<Option<String>> = match Directory::for_home(home).records().await {
        Ok(records) => records
            .into_iter()
            .filter(|record| record.meta.get("kind").and_then(Value::as_str) == Some("antigravity"))
            .map(|record| {
                let version = record.meta.get("version").and_then(Value::as_str);
                version.map(str::to_owned)
            })
            .collect(),
        Err(error) => return eprintln!("Antigravity's holders: {error}"),
    };
    let root = home.root().join("agents").join("antigravity");
    if let Err(error) = tokio::task::spawn_blocking(move || forget_releases(&root, &held))
        .await
        .map_err(io::Error::other)
        .and_then(|done| done)
    {
        eprintln!("Antigravity's earlier releases: {error}");
    }
}

/// `held`: the version each running holder says it runs (none: it doesn't say, so it may be any).
fn forget_releases(root: &Path, held: &[Option<String>]) -> io::Result<()> {
    if !root.join(EARLIER).join(".complete").exists() {
        return Ok(());
    }
    let release = |name: &str| {
        let (numbers, label) = name.split_once('-').unwrap_or((name, ""));
        let parts: Vec<&str> = numbers.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            && (!name.contains('-')
                || !label.is_empty()
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte)))
    };
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == EARLIER || !release(&name) {
            continue;
        }
        if held
            .iter()
            .all(|using| using.as_deref().is_some_and(|using| using != name))
        {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_earlier_release_stays_installed_through_its_last_holder() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        let (old, current) = (root.join("1.2.0"), root.join(EARLIER));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(root.join("profile")).unwrap();
        let held = [Some("1.2.0".to_owned())];
        std::fs::write(current.join(".complete"), "").unwrap();
        forget_releases(root, &held).unwrap();
        assert!(old.exists() && current.exists());
        // Its holder gone, the one kept not yet whole: still there.
        std::fs::remove_file(current.join(".complete")).unwrap();
        forget_releases(root, &[]).unwrap();
        assert!(old.exists());
        // A holder that doesn't say which it runs keeps them all.
        std::fs::write(current.join(".complete"), "").unwrap();
        forget_releases(root, &[None]).unwrap();
        assert!(old.exists());
        forget_releases(root, &[Some("agy".into())]).unwrap();
        assert!(!old.exists() && current.exists() && root.join("profile").exists());
    }

    #[test]
    fn its_whole_answer_saying_its_limit_is_the_limit() {
        assert!(limit_reply(
            "**Usage Limit Reached.** You have reached your current quota for this period. Your limit will reset in 6 days, 19 hours."
        ));
        assert!(limit_reply(
            "Usage limit reached. You have reached your current quota for this period"
        ));
        assert!(!limit_reply(
            "Usage limit reached. You have reached your current quota for this period. Also, here's the code."
        ));
        assert!(!limit_reply("The usage limit reached its peak."));
    }

    #[test]
    fn its_claude_and_gpt_models_are_named_apart() {
        assert_eq!(model_name("Gemini 4 Hydrogen"), "Gemini 4 Hydrogen");
        assert_eq!(
            model_name("Claude Opus 5.5"),
            "Claude Opus 5.5 (Antigravity)"
        );
    }

    #[test]
    fn a_made_image_is_the_newest_by_its_name() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("cat_1.jpg"), b"x").unwrap();
        std::fs::write(folder.path().join("dog_1.png"), b"x").unwrap();
        let found = generated_image(folder.path(), &json!({"image_name": "\"cat\""}), 0);
        assert!(
            matches!(found, Some(ToolOutput::File { name: Some(name), .. }) if name == "cat_1.jpg")
        );
        assert!(generated_image(folder.path(), &json!({}), u64::MAX / 2).is_none());
    }
}
