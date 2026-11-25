use std::{
    ffi::OsStr,
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use kuvo_os::{
    platform::{Environment, Executable, Platform, env_value, which},
    process::{Spawn, Stdio},
};
use kuvo_store::home::Home;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    account_check::AccountStatus,
    sign_in::{Attempt, Request, command as sign_in},
};

use super::model::Catalog;

#[derive(Clone)]
pub(super) struct Program {
    pub home: Home,
    pub holder_binary: PathBuf,
    account: String,
    managed: Option<PathBuf>,
    /// Claude turned this sign-in away in a run, though `claude auth status` may still say
    /// signed in: when its credentials had last been written then. Until they're written again,
    /// the account needs a sign-in.
    turned_away: Arc<Mutex<Option<SystemTime>>>,
    /// Signed in with a key, paid by the token (not a Claude plan), as it last said: unknown
    /// until it's asked.
    uses_key: Arc<AtomicU8>,
    /// Counts updates of Claude Code: a conversation's Claude Code started before the last one
    /// starts again at its next message.
    updates: Arc<AtomicU64>,
    identity: Arc<tokio::sync::Mutex<Option<IdentityCache>>>,
}

struct IdentityCache {
    path: PathBuf,
    stamp: (u64, Option<SystemTime>),
    value: Option<(String, String)>,
}

const UNKNOWN: u8 = 0;
const PLAN: u8 = 1;
const KEY: u8 = 2;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    logged_in: bool,
    #[serde(default)]
    auth_method: String,
    email: Option<String>,
    api_provider: Option<String>,
    subscription_type: Option<String>,
}

#[derive(Deserialize)]
struct Legacy {
    token: String,
}

impl Program {
    pub fn new(
        home: Home,
        holder_binary: PathBuf,
        account: String,
        managed: Option<PathBuf>,
    ) -> Self {
        Self {
            home,
            holder_binary,
            account,
            managed,
            turned_away: Arc::default(),
            uses_key: Arc::default(),
            updates: Arc::default(),
            identity: Arc::default(),
        }
    }

    /// Updates of Claude Code so far.
    pub fn updates(&self) -> u64 {
        self.updates.load(Ordering::SeqCst)
    }

    pub fn updated(&self) {
        self.updates.fetch_add(1, Ordering::SeqCst);
    }

    /// Signed in with an API key: what Claude Code says a turn cost is what it costs. Asked
    /// first if it never has been.
    pub async fn uses_key(&self) -> bool {
        if self.uses_key.load(Ordering::Relaxed) == UNKNOWN {
            let _ = self.probe().await;
        }
        self.uses_key.load(Ordering::Relaxed) == KEY
    }

    /// Claude turned the sign-in away in a run: the account needs one until it's written again.
    pub fn turn_away(&self) {
        let written = self.credentials_written();
        *self
            .turned_away
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(written);
    }

    /// When the sign-in was last written: signing in, or renewing it. Claude Code keeps it in
    /// `.credentials.json` in its folder, or on a Mac in the Keychain, by the folder's name.
    fn credentials_written(&self) -> SystemTime {
        let env: Environment = std::env::vars_os().collect();
        let platform = Platform::current();
        let own = self.managed.clone().or_else(|| {
            env_value(platform, &env, "CLAUDE_CONFIG_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        });
        let folder = own.clone().or_else(|| {
            kuvo_os::platform::Account::current()
                .and_then(|account| account.home_folder(platform, &env))
                .ok()
                .map(|home| home.join(".claude"))
        });
        let file = folder
            .and_then(|folder| fs::metadata(folder.join(".credentials.json")).ok())
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        // Tests never read the computer's Keychain.
        if !cfg!(target_os = "macos")
            || env_value(platform, &env, "KUVO_TEST_PATH_ONLY") == Some(OsStr::new("1"))
        {
            return file;
        }
        let service = match own {
            Some(own) => {
                let digest = Sha256::digest(own.as_os_str().as_encoded_bytes());
                format!("Claude Code-credentials-{}", &hex::encode(digest)[..8])
            }
            None => "Claude Code-credentials".into(),
        };
        file.max(keychain_written(&service).unwrap_or(SystemTime::UNIX_EPOCH))
    }

    fn token_file(&self) -> PathBuf {
        self.home
            .root()
            .join("agents/claude/tokens")
            .join(format!("{}.json", self.account))
    }

    fn token(&self) -> Option<String> {
        let saved = fs::read(self.token_file()).ok()?;
        let legacy: Legacy = serde_json::from_slice(&saved).ok()?;
        (!legacy.token.is_empty()).then_some(legacy.token)
    }

    fn forget_token(&self) -> io::Result<()> {
        let path = self.token_file();
        match fs::remove_file(&path) {
            Ok(()) => {
                if let Some(folder) = path.parent() {
                    let _ = fs::remove_dir(folder);
                }
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub fn environment(&self, legacy: bool) -> Environment {
        let mut env: Environment = std::env::vars_os().collect();
        env.retain(|name, _| !name.eq_ignore_ascii_case("CLAUDE_CODE_OAUTH_TOKEN"));
        if legacy && let Some(token) = self.token() {
            env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), token.into());
        }
        if let Some(home) = &self.managed {
            env.retain(|name, _| !name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"));
            env.insert("CLAUDE_CONFIG_DIR".into(), home.as_os_str().into());
        }
        env.insert(
            "CLAUDE_CODE_TMPDIR".into(),
            self.scratch().join("claude").into(),
        );
        let updates = fs::read(self.home.settings().join("agents-update-themselves.json"))
            .is_ok_and(|bytes| matches!(serde_json::from_slice::<bool>(&bytes), Ok(true)));
        if !updates || env.contains_key(OsStr::new("KUVO_AGENTS_REMOTE")) {
            env.insert("DISABLE_AUTOUPDATER".into(), "1".into());
        }
        // It reaches the relay's servers on this computer: never through a proxy.
        crate::tool_relay::unproxied_loopback(&mut env);
        env
    }

    pub fn config_home(&self, directory: &Path) -> Option<PathBuf> {
        let env: Environment = std::env::vars_os().collect();
        let platform = Platform::current();
        if let Some(managed) = &self.managed {
            return Some(managed.clone());
        }
        if let Some(config) =
            env_value(platform, &env, "CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty())
        {
            return Some(directory.join(config));
        }
        let account = kuvo_os::platform::Account::current().ok()?;
        let personal = account.home_folder(platform, &env).ok()?;
        Some(personal.join(".claude"))
    }

    /// Where Claude Code keeps who's signed in (`.claude.json`): its folder when it's given one,
    /// else your home.
    fn record_home(&self) -> Option<PathBuf> {
        if let Some(managed) = &self.managed {
            return Some(managed.clone());
        }
        let env: Environment = std::env::vars_os().collect();
        let platform = Platform::current();
        if let Some(config) = env_value(platform, &env, "CLAUDE_CONFIG_DIR")
            .filter(|value| !value.to_string_lossy().trim().is_empty())
        {
            return Some(PathBuf::from(config));
        }
        let account = kuvo_os::platform::Account::current().ok()?;
        account.home_folder(platform, &env).ok()
    }

    /// Its sign-in, as a fingerprint of who it is: the same through Claude Code's renewals, which
    /// Kuvo never sees, and nothing of the account in it. None when no one's signed in that way.
    pub async fn sign_in_mark(&self) -> Option<String> {
        let (mark, _) = self.plan_sign_in().await?;
        Some(mark)
    }

    pub async fn plan_sign_in(&self) -> Option<(String, String)> {
        let path = self.record_home()?.join(".claude.json");
        let stamp = tokio::fs::metadata(&path).await.ok()?;
        let stamp = (stamp.len(), stamp.modified().ok());
        let identity = {
            let mut cache = self.identity.lock().await;
            if let Some(kept) = cache
                .as_ref()
                .filter(|c| c.path == path && c.stamp == stamp)
            {
                kept.value.clone()
            } else {
                let raw = tokio::fs::read(&path).await.ok()?;
                let parsed = (|| {
                    let record: serde_json::Value = serde_json::from_slice(&raw).ok()?;
                    let account = &record["oauthAccount"];
                    let uuid = account["accountUuid"].as_str()?.trim().to_lowercase();
                    if uuid.is_empty() {
                        return None;
                    }
                    let organization = account["organizationUuid"]
                        .as_str()
                        .map(|id| id.trim().to_lowercase())
                        .unwrap_or_default();
                    let raw = format!("anthropic:{uuid}|{organization}");
                    let mark = hex::encode(&Sha256::digest(&raw)[..8]);
                    Some((mark, raw))
                })();
                *cache = Some(IdentityCache {
                    path,
                    stamp,
                    value: parsed.clone(),
                });
                parsed
            }
        }?;
        // The identity record outlives some sign-outs. Accounts' local status
        // also requires the CLI's actual OAuth credential (file or Keychain).
        let program = self.clone();
        tokio::task::spawn_blocking(move || {
            let status = program.local_status(&program.environment(false));
            if !status.logged_in || status.auth_method != "claude.ai" {
                return None;
            }
            let turned_away = *program
                .turned_away
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if turned_away.is_some_and(|at| program.credentials_written() <= at) {
                return None;
            }
            Some(identity)
        })
        .await
        .ok()
        .flatten()
    }

    pub fn scratch(&self) -> PathBuf {
        self.home.run().join("agents/tmp")
    }

    pub fn executable(&self, env: &Environment) -> Option<Executable> {
        let platform = Platform::current();
        let mut folders = Vec::new();
        let user_home = if platform == Platform::Windows {
            "USERPROFILE"
        } else {
            "HOME"
        };
        if let Some(home) = env_value(platform, env, user_home).map(Path::new) {
            folders.push(home.join(".local/bin"));
            folders.push(home.join(".claude/local"));
        }
        if env_value(platform, env, "KUVO_TEST_PATH_ONLY") != Some(OsStr::new("1")) {
            folders.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
        }
        which(OsStr::new("claude"), env, &folders)
    }

    pub fn kept_version(&self) -> Option<String> {
        let env = self.environment(false);
        let executable = self.executable(&env)?;
        crate::program_updates::kept_installed_version(
            executable.path(),
            &self.home.run().join("agents/program-cache"),
        )
    }

    /// The catalog is produced only during a turn or sign-in, not by the picker.
    pub fn catalog_file(&self) -> PathBuf {
        self.home
            .run()
            .join("agents/claude/catalogs")
            .join(format!("{}.json", self.account))
    }

    fn local_status(&self, env: &Environment) -> Status {
        let record: serde_json::Value = self
            .record_home()
            .and_then(|folder| fs::read(folder.join(".claude.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let account = &record["oauthAccount"];
        let config = self.config_home(self.home.root());
        let read = |name: &str| -> serde_json::Value {
            config
                .as_ref()
                .and_then(|folder| fs::read(folder.join(name)).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        };
        let credentials = read(".credentials.json");
        let settings = read("settings.json");
        let configured = |name| {
            env_value(Platform::current(), env, name).is_some_and(|value| !value.is_empty())
                || settings["env"][name]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
        };
        let key = configured("ANTHROPIC_API_KEY")
            || configured("ANTHROPIC_AUTH_TOKEN")
            || settings["apiKeyHelper"]
                .as_str()
                .is_some_and(|helper| !helper.is_empty());
        let cloud = [
            ("CLAUDE_CODE_USE_BEDROCK", "bedrock"),
            ("CLAUDE_CODE_USE_VERTEX", "vertex"),
            ("CLAUDE_CODE_USE_FOUNDRY", "foundry"),
        ]
        .into_iter()
        .find_map(|(name, service)| configured(name).then(|| service.to_owned()));
        let oauth = account.is_object() && self.credentials_written() != SystemTime::UNIX_EPOCH;
        Status {
            logged_in: key || cloud.is_some() || oauth,
            auth_method: if key || cloud.is_some() || account["organizationType"] == "api" {
                "console_api_key".into()
            } else if oauth {
                "claude.ai".into()
            } else {
                "none".into()
            },
            email: account["emailAddress"].as_str().map(str::to_owned),
            api_provider: cloud,
            subscription_type: credentials["claudeAiOauth"]["subscriptionType"]
                .as_str()
                .or_else(|| account["subscriptionType"].as_str())
                .map(str::to_owned),
        }
    }

    pub async fn probe(&self) -> io::Result<(AccountStatus, bool)> {
        let program = self.clone();
        tokio::task::spawn_blocking(move || program.probe_blocking()).await?
    }

    fn probe_blocking(&self) -> io::Result<(AccountStatus, bool)> {
        let env = self.environment(false);
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
        if self.executable(&env).is_none() {
            answer.message = Some("Install Claude Code, then sign in here.".into());
            return Ok((answer, false));
        }
        let status = self.local_status(&env);
        answer.installed = true;
        answer.signed_in = status.logged_in;
        let key = !status.auth_method.is_empty() && status.auth_method != "claude.ai";
        self.uses_key
            .store(if key { KEY } else { PLAN }, Ordering::Relaxed);
        let legacy = self.token().is_some();
        if status.logged_in {
            let mut label = status.email.clone().or_else(|| {
                status
                    .api_provider
                    .filter(|provider| provider != "firstParty")
            });
            if let Some(plan) = status.subscription_type.filter(|plan| !plan.is_empty()) {
                let mut characters = plan.chars();
                let title = characters
                    .next()
                    .expect("nonempty plan")
                    .to_uppercase()
                    .to_string()
                    + characters.as_str();
                let plan = format!("{title} plan");
                label = Some(label.map_or_else(|| plan.clone(), |who| format!("{who} · {plan}")));
            }
            answer.label = Some(label.unwrap_or_else(|| "Claude Code".into()));
            answer.label_describes_method = status.email.is_none();
            answer.identity = status.email;
            if legacy {
                self.forget_token()?;
            }
        } else if legacy {
            answer.signed_in = true;
            answer.label = Some("Claude plan".into());
            answer.label_describes_method = true;
            answer.message = Some("Sign in again to see your plan's usage.".into());
        } else {
            answer.message = Some(
                "Sign in to Claude Code with your Claude plan or an Anthropic Console account."
                    .into(),
            );
        }
        let mut turned_away = self
            .turned_away
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(written) = *turned_away {
            if self.credentials_written() > written {
                *turned_away = None;
            } else if answer.signed_in {
                answer.signed_in = false;
                answer.ended = true;
                answer.label = None;
                answer.identity = None;
                answer.label_describes_method = false;
                answer.message = Some("Claude ended this sign-in. Sign in again.".into());
            }
        }
        Ok((answer, status.logged_in && legacy))
    }

    pub async fn login(
        &self,
        request: Request,
        models: Catalog,
    ) -> io::Result<std::sync::Arc<dyn Attempt>> {
        let method = match request.method.as_str() {
            "claudeai" => "--claudeai",
            "console" => "--console",
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "That isn't a Claude Code sign-in method.",
                ));
            }
        };
        let program = self.clone();
        let confirm = self.clone();
        let home = self.home.root().to_owned();
        let handle = tokio::runtime::Handle::current();
        let options = tokio::task::spawn_blocking(move || {
            let env = program.environment(false);
            let executable = program.executable(&env).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Install Claude Code first.")
            })?;
            if let Some(managed) = &program.managed {
                kuvo_store::files::private_folder(managed)?;
            }
            let mut spawn = Spawn::new(executable.path(), env);
            spawn.executable = executable;
            spawn.cwd = Some(home);
            spawn.args = ["auth", "login", method]
                .into_iter()
                .map(Into::into)
                .collect();
            Ok::<_, io::Error>(sign_in::Options {
                spawn,
                temporary: program.scratch(),
                printed_page: request.remote && !request.browser_return,
                code_prompt: Some("Paste code here if prompted >"),
                instructions: "Finish signing in on Claude’s page.",
                code_instructions: "Sign in on Claude’s page, then paste its code here.",
                confirm: Box::new(move || {
                    let env = confirm.environment(false);
                    confirm.executable(&env).is_some_and(|executable| {
                        native_status(executable, env, confirm.home.root())
                            .is_ok_and(|status| status.logged_in)
                    })
                }),
                completed: Box::new(move || {
                    let _ = program.forget_token();
                    handle.block_on(async {
                        models.clear().await;
                        // This is explicitly a sign-in, so it may ask Claude for its models.
                        let _ = models.refresh(program.clone()).await;
                    });
                }),
            })
        })
        .await??;
        sign_in::start(options).await
    }

    pub async fn logout(&self) -> io::Result<()> {
        let program = self.clone();
        tokio::task::spawn_blocking(move || {
            program.forget_token()?;
            let env = program.environment(false);
            if let Some(executable) = program.executable(&env) {
                let (success, _) =
                    command(executable, env, &["auth", "logout"], program.home.root())?;
                if !success {
                    return Err(io::Error::other("Claude Code couldn't sign out."));
                }
            }
            Ok(())
        })
        .await?
    }
}

/// The Keychain item's modification date, read from its attributes (never its secret).
fn keychain_written(service: &str) -> Option<SystemTime> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.contains("\"mdat\"<timedate>="))?;
    let stamp = line.split('"').nth(3)?;
    let digits = stamp
        .strip_suffix("Z\\000")
        .or_else(|| stamp.strip_suffix('Z'))?;
    let digits = digits.get(..14)?;
    let time: jiff::civil::DateTime =
        jiff::civil::DateTime::strptime("%Y%m%d%H%M%S", digits).ok()?;
    let at = time.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp();
    Some(SystemTime::from(at))
}

fn native_status(executable: Executable, env: Environment, cwd: &Path) -> io::Result<Status> {
    let (_, stdout) = command(executable, env, &["auth", "status", "--json"], cwd)?;
    serde_json::from_slice(&stdout)
        .map_err(|_| io::Error::other("Claude Code couldn't say whether it's signed in."))
}

/// The timeout owns the whole process tree, including descendants holding stdout open.
fn command(
    executable: Executable,
    env: Environment,
    args: &[&str],
    cwd: &Path,
) -> io::Result<(bool, Vec<u8>)> {
    let mut spawn = Spawn::new(executable.path(), env);
    spawn.executable = executable;
    spawn.args = args.iter().map(Into::into).collect();
    spawn.cwd = Some(cwd.to_owned());
    spawn.stdout = Stdio::Pipe;
    let mut child = spawn.spawn()?;
    let stdout = child.stdout.take().expect("requested stdout pipe");
    let read = thread::Builder::new()
        .name("claude-auth-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout.take(1_000_001).read_to_end(&mut bytes)?;
            if bytes.len() > 1_000_000 {
                return Err(io::Error::other(
                    "Claude Code sent too much account output.",
                ));
            }
            Ok(bytes)
        })?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !child.has_exited()? {
        if Instant::now() >= deadline {
            child.stop(Duration::from_secs(1), |_| {})?;
            let _ = read.join();
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        thread::sleep(Duration::from_millis(20));
    }
    let status = child.wait()?;
    let bytes = read
        .join()
        .map_err(|_| io::Error::other("Claude account reader stopped."))??;
    Ok((status.success(), bytes))
}
