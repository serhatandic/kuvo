//! Your own `codex`: where it is, its home (CODEX_HOME), who it's signed in as, and how its
//! app-server starts under a holder.

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use base64::Engine as _;
use kuvo_os::{
    platform::{Account, Environment, Executable, Platform, env_value, which},
    process::Spawn,
};
use kuvo_store::home::Home;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    hold::{HoldKind, Limits, Metadata, Start},
    session_view::Compaction,
};

/// Codex's plugins that drive your computer (ChatGPT's Computer Use), off for every thread here
/// even where your Codex settings turn them on: they click around your Mac with no card to allow it.
const NO_COMPUTER_USE: [&str; 2] = [
    "unified-computer-use@openai-bundled",
    "computer-use@openai-bundled",
];

#[derive(Clone)]
pub(super) struct Program {
    pub home: Home,
    pub holder_binary: PathBuf,
    pub provider: String,
    /// An added account's own CODEX_HOME.
    managed: Option<PathBuf>,
    auth: Arc<AuthReads>,
}

/// Its ChatGPT sign-in, as Codex keeps it.
pub(super) struct Credentials {
    pub token: String,
    pub account: Option<String>,
}

impl Program {
    pub fn new(
        home: Home,
        holder_binary: PathBuf,
        provider: String,
        managed: Option<PathBuf>,
    ) -> Self {
        Self {
            home,
            holder_binary,
            provider,
            managed,
            auth: Arc::default(),
        }
    }

    fn user_home(env: &Environment) -> Option<PathBuf> {
        let platform = Platform::current();
        Account::current().ok()?.home_folder(platform, env).ok()
    }

    /// Your own Codex's folder, which an added account's links to.
    pub fn your_codex(&self) -> Option<PathBuf> {
        let env: Environment = std::env::vars_os().collect();
        Self::user_home(&env).map(|home| home.join(".codex"))
    }

    /// Its home (CODEX_HOME): an added account's own, or yours.
    pub fn codex_home(&self) -> Option<PathBuf> {
        if let Some(managed) = &self.managed {
            return Some(managed.clone());
        }
        let env: Environment = std::env::vars_os().collect();
        env_value(Platform::current(), &env, "CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.your_codex())
    }

    pub fn added(&self) -> bool {
        self.managed.is_some()
    }

    pub fn environment(&self) -> Environment {
        let mut env: Environment = std::env::vars_os().collect();
        env.entry("CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into())
            .or_insert_with(|| "kuvo-code".into());
        if let Some(home) = &self.managed {
            env.retain(|name, _| !name.eq_ignore_ascii_case("CODEX_HOME"));
            env.insert("CODEX_HOME".into(), home.as_os_str().into());
        }
        // It reaches the relay's servers on this computer: never through a proxy.
        crate::tool_relay::unproxied_loopback(&mut env);
        env
    }

    /// Its maker's installer's places, else your PATH.
    pub fn executable(&self, env: &Environment) -> Option<Executable> {
        let platform = Platform::current();
        let mut folders: Vec<PathBuf> = Vec::new();
        if env_value(platform, env, "KUVO_TEST_PATH_ONLY") != Some(OsStr::new("1")) {
            folders.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
        }
        if let Some(home) = Self::user_home(env) {
            folders.push(home.join(".local/bin"));
            folders.push(home.join(".codex/bin"));
        }
        which(OsStr::new("codex"), env, &folders)
    }

    pub fn installed(&self) -> bool {
        self.executable(&self.environment()).is_some()
    }

    /// Where its app-server runs outside a session's folder: the box's own scratch folder.
    pub fn scratch(&self) -> PathBuf {
        self.home.run().join("agents/tmp")
    }

    /// The computer's "Summarize long conversations at", as its app-server starts with it.
    pub fn compaction(&self) -> String {
        Compaction::read(self.home.root()).id()
    }

    /// Its app-server, as the holder starts it (`codex app-server`), for all of this account's
    /// conversations.
    pub fn launch(&self, compaction: &str) -> io::Result<Start> {
        if let (Some(home), Some(yours)) = (&self.managed, self.your_codex()) {
            shadow_home(home, &yours);
        }
        let env = self.environment();
        let executable = self.executable(&env).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Install Codex first: curl -fsSL https://chatgpt.com/codex/install.sh | sh",
            )
        })?;
        let mut command = vec![executable.path().to_owned(), "app-server".into()];
        for plugin in NO_COMPUTER_USE {
            command.extend([
                "-c".into(),
                format!("plugins.{plugin}.enabled=false").into(),
            ]);
        }
        // Its own updater off, unless you let the agents update themselves.
        let updates = fs::read(self.home.settings().join("agents-update-themselves.json"))
            .is_ok_and(|bytes| matches!(serde_json::from_slice::<bool>(&bytes), Ok(true)));
        if !updates {
            command.extend(["-c".into(), "check_for_update_on_startup=false".into()]);
        }
        let scratch = self.scratch();
        kuvo_store::files::private_folder(&scratch)?;
        let meta = json!({
            "provider": self.provider, "kind": "codex", "key": "", "compaction": compaction,
            "fence": "", "fenced": false, "turns": {},
            // (Until it's initialized: one whose resident went before then is of no use to the next.)
            "initialized": false,
        });
        Ok(Start {
            command,
            cwd: Some(scratch),
            env,
            kind: HoldKind::JsonRpc,
            meta: meta.as_object().cloned().unwrap_or_else(Metadata::new),
            limits: Limits::default(),
            portable_requests: None,
        })
    }

    /// Native sign-in metadata, without starting Codex or renewing its credentials. The claims
    /// are for display only, never authorization. API keys are not returned from this method.
    pub async fn account_info(&self) -> Option<(Value, Option<String>)> {
        let saved = self.auth.read(self.codex_home()?, read_auth).await?;
        if let Some(credentials) = oauth(&saved) {
            let access = claims(&credentials.token).unwrap_or_default();
            let id = saved["tokens"]["id_token"]
                .as_str()
                .and_then(claims)
                .unwrap_or_default();
            let auth = "https://api.openai.com/auth";
            let profile = "https://api.openai.com/profile";
            let email = id["email"]
                .as_str()
                .or_else(|| access[profile]["email"].as_str());
            let plan = id[auth]["chatgpt_plan_type"]
                .as_str()
                .or_else(|| access[auth]["chatgpt_plan_type"].as_str());
            let identity = chatgpt_identity(&credentials.token, credentials.account.as_deref());
            return Some((
                json!({"type":"chatgpt","email":email,"planType":plan}),
                identity,
            ));
        }
        saved["OPENAI_API_KEY"]
            .as_str()
            .filter(|key| !key.is_empty())?;
        Some((json!({"type":"apiKey"}), None))
    }

    pub fn catalog_file(&self) -> PathBuf {
        self.home
            .run()
            .join("agents/codex/catalogs")
            .join(format!("{}.json", self.provider))
    }

    /// Its ChatGPT sign-in: `auth.json`, or the Keychain's or keyring's item where its settings
    /// keep it there. Never an API key's, and never renewed here.
    pub async fn credentials(&self) -> Option<Credentials> {
        oauth(&self.auth.read(self.codex_home()?, read_auth).await?)
    }

    /// Who it's signed in as with ChatGPT (`chatgpt:<account>/<user>`), from its token's own claims.
    pub async fn identity(&self) -> Option<String> {
        let credentials = self.credentials().await?;
        chatgpt_identity(&credentials.token, credentials.account.as_deref())
    }

    /// `codex logout`, for its home: Codex forgets the sign-in tied to it, the Keychain's too.
    pub async fn logout(&self) -> io::Result<()> {
        let program = self.clone();
        tokio::task::spawn_blocking(move || {
            let env = program.environment();
            let Some(executable) = program.executable(&env) else {
                return Ok(());
            };
            let mut spawn = Spawn::new(executable.path(), env);
            spawn.priority = kuvo_os::process::Priority::Background;
            spawn.executable = executable;
            spawn.args = vec!["logout".into()];
            spawn.cwd = Some(program.home.root().to_owned());
            let mut child = spawn.spawn()?;
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while !child.has_exited()? {
                if std::time::Instant::now() >= deadline {
                    child.stop(Duration::from_secs(1), |_| {})?;
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Codex couldn't sign out.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if child.wait()?.success() {
                Ok(())
            } else {
                Err(io::Error::other("Codex couldn't sign out."))
            }
        })
        .await?
    }

    /// Removes Codex's cache of its models, so the next list asks OpenAI (it doesn't follow a link
    /// or remove a folder).
    pub fn forget_model_cache(&self) -> io::Result<()> {
        let Some(home) = self.codex_home() else {
            return Ok(());
        };
        match fs::remove_file(home.join("models_cache.json")) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// A concurrent account status and plan read share one native credential lookup.
/// Only overlapping callers keep its result; a later sign-in check sees the store again.
#[derive(Default)]
struct AuthReads {
    active: Mutex<Option<(PathBuf, Weak<tokio::sync::OnceCell<Option<Value>>>)>>,
}
impl AuthReads {
    async fn read(
        &self,
        home: PathBuf,
        load: impl FnOnce(&Path) -> Option<Value> + Send + 'static,
    ) -> Option<Value> {
        let cell = {
            let mut active = self.active.lock().expect("Codex auth read");
            if let Some(cell) = active
                .as_ref()
                .filter(|(path, _)| path == &home)
                .and_then(|(_, cell)| cell.upgrade())
            {
                cell
            } else {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                *active = Some((home.clone(), Arc::downgrade(&cell)));
                cell
            }
        };
        cell.get_or_init(|| async move {
            tokio::task::spawn_blocking(move || load(&home))
                .await
                .ok()
                .flatten()
        })
        .await
        .clone()
    }
}

fn read_auth(home: &Path) -> Option<Value> {
    read_auth_using(home, keychain)
}

fn read_auth_using(home: &Path, lookup: impl FnOnce(&str) -> Option<Value>) -> Option<Value> {
    let mode = fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|settings| {
            settings.lines().find_map(|line| {
                let value = line.trim().strip_prefix("cli_auth_credentials_store")?;
                let value = value.trim_start().strip_prefix('=')?.trim();
                let value = value.trim_matches(|quote| quote == '"' || quote == '\'');
                matches!(value, "keyring" | "auto").then(|| value.to_owned())
            })
        });
    if mode.is_some()
        && let Some(saved) = lookup(&keychain_account(home))
    {
        return Some(saved);
    }
    if mode.as_deref() == Some("keyring") {
        return None;
    }
    serde_json::from_slice(&fs::read(home.join("auth.json")).ok()?).ok()
}

fn oauth(saved: &Value) -> Option<Credentials> {
    let key = saved["OPENAI_API_KEY"]
        .as_str()
        .is_some_and(|key| !key.is_empty());
    if saved["auth_mode"] == "apikey" || (saved["auth_mode"].is_null() && key) {
        return None;
    }
    let tokens = &saved["tokens"];
    let token = tokens["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())?;
    Some(Credentials {
        token: token.to_owned(),
        account: tokens["account_id"].as_str().map(str::to_owned),
    })
}

/// The Keychain's (or keyring's) item Codex keeps its sign-in in, by its home.
fn keychain_account(home: &Path) -> String {
    let canonical = fs::canonicalize(home).unwrap_or_else(|_| home.to_owned());
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    format!("cli|{}", &hex::encode(digest)[..16])
}
fn keychain(account: &str) -> Option<Value> {
    let env: Environment = std::env::vars_os().collect();
    // Tests never read the computer's Keychain.
    if env_value(Platform::current(), &env, "KUVO_TEST_PATH_ONLY") == Some(OsStr::new("1")) {
        return None;
    }
    let output = if cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "Codex Auth",
                "-a",
                account,
                "-w",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?
    } else if cfg!(target_os = "linux") {
        let secret = which(OsStr::new("secret-tool"), &env, &[])?;
        std::process::Command::new(secret.path())
            .args([
                "lookup",
                "application",
                "rust-keyring",
                "service",
                "Codex Auth",
                "username",
                account,
                "target",
                "default",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?
    } else {
        return None;
    };
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).ok())
        .flatten()
        .filter(Value::is_object)
}

/// What a JSON Web Token says of itself, never checked: only to tell who it's for.
fn claims(token: &str) -> Option<Value> {
    let part = token.trim().split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok().filter(Value::is_object)
}

pub(super) fn chatgpt_identity(token: &str, account: Option<&str>) -> Option<String> {
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_lowercase)
    };
    let auth = claims(token).map(|claims| claims["https://api.openai.com/auth"].clone());
    let auth = auth.as_ref();
    let found = auth
        .and_then(|auth| text(&auth["chatgpt_account_id"]))
        .or_else(|| account.and_then(|id| text(&Value::from(id))))?;
    // (A workspace's members share its account: each one's own limits.)
    let user =
        auth.and_then(|auth| text(&auth["chatgpt_user_id"]).or_else(|| text(&auth["user_id"])));
    Some(match user {
        Some(user) => format!("chatgpt:{found}/{user}"),
        None => format!("chatgpt:{found}"),
    })
}

/// A fingerprint of a secret: who's signed in, never the secret.
pub(crate) fn fingerprint(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))[..16].to_owned()
}

/// An added account's Codex folder: your own Codex's settings, instructions, skills and history in
/// it as links, but its own sign-in and its own list of models.
fn shadow_home(home: &Path, own: &Path) {
    const KEPT: [&str; 2] = ["auth.json", "models_cache.json"];
    if kuvo_store::files::private_folder(home).is_err() {
        return;
    }
    let wanted: Vec<_> = fs::read_dir(own)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name())
                .filter(|name| !KEPT.iter().any(|kept| name == kept))
                .collect()
        })
        .unwrap_or_default();
    for name in &wanted {
        let link = home.join(name);
        let target = own.join(name);
        match fs::symlink_metadata(&link) {
            Ok(found) if found.file_type().is_symlink() => {
                if fs::read_link(&link).is_ok_and(|to| to == target) {
                    continue;
                }
                let _ = fs::remove_file(&link);
            }
            // Something of its own there already: kept.
            Ok(_) => continue,
            Err(_) => {}
        }
        let _ = symlink(&target, &link);
    }
    // Links to what's gone from yours: gone too.
    for entry in fs::read_dir(home).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_symlink())
            && !wanted.contains(&entry.file_name())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(claims: Value) -> String {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("e30.{body}.sig")
    }

    #[test]
    fn a_chatgpt_sign_in_is_its_account_and_its_member() {
        let auth = "https://api.openai.com/auth";
        let both =
            token(json!({ auth: { "chatgpt_account_id": "Acc", "chatgpt_user_id": "User" } }));
        assert_eq!(
            chatgpt_identity(&both, None).as_deref(),
            Some("chatgpt:acc/user")
        );
        let account = token(json!({}));
        assert_eq!(
            chatgpt_identity(&account, Some("acc")).as_deref(),
            Some("chatgpt:acc")
        );
        assert_eq!(chatgpt_identity("not a token", None), None);
    }

    #[test]
    fn an_api_key_sign_in_has_no_chatgpt_credentials() {
        assert!(oauth(&json!({"auth_mode":"apikey","tokens":{"access_token":"x"}})).is_none());
        assert!(oauth(&json!({"OPENAI_API_KEY":"sk"})).is_none());
        let found = oauth(&json!({"tokens":{"access_token":"x","account_id":"a"}})).unwrap();
        assert_eq!(
            (found.token.as_str(), found.account.as_deref()),
            ("x", Some("a"))
        );
    }

    #[test]
    fn a_keyring_store_uses_the_canonical_folder_and_never_falls_back_to_a_file() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(
            folder.path().join("config.toml"),
            "cli_auth_credentials_store = \"keyring\"\n",
        )
        .unwrap();
        fs::write(
            folder.path().join("auth.json"),
            r#"{"tokens":{"access_token":"file-fixture","account_id":"file"}}"#,
        )
        .unwrap();
        let key = format!(
            "cli|{}",
            &hex::encode(Sha256::digest(
                fs::canonicalize(folder.path())
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
            ))[..16]
        );
        let saved = read_auth_using(folder.path(), |account| {
            assert_eq!(account, key);
            Some(json!({"tokens":{"access_token":"keyring-fixture","account_id":"keyring"}}))
        })
        .unwrap();
        assert_eq!(oauth(&saved).unwrap().account.as_deref(), Some("keyring"));
        assert!(read_auth_using(folder.path(), |_| None).is_none());
        fs::write(
            folder.path().join("config.toml"),
            "cli_auth_credentials_store = \"auto\"\n",
        )
        .unwrap();
        assert_eq!(
            oauth(&read_auth_using(folder.path(), |_| Some(saved.clone())).unwrap())
                .unwrap()
                .account
                .as_deref(),
            Some("keyring")
        );
        assert_eq!(
            oauth(&read_auth_using(folder.path(), |_| None).unwrap())
                .unwrap()
                .account
                .as_deref(),
            Some("file")
        );
    }

    #[tokio::test]
    async fn overlapping_auth_reads_share_one_lookup_but_the_next_read_is_fresh() {
        let reader = AuthReads::default();
        let home = PathBuf::from("fixture");
        let (begun, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let first = reader.read(home.clone(), move |_| {
            begun.send(()).unwrap();
            wait.recv().unwrap();
            Some(json!({"account":"first"}))
        });
        let second = reader.read(home.clone(), |_| panic!("A pending lookup must be shared"));
        let unlock = async {
            started.await.unwrap();
            release.send(()).unwrap();
        };
        let (first, second, ()) = tokio::join!(first, second, unlock);
        assert_eq!(first, second);
        assert_eq!(first.unwrap()["account"], "first");
        assert_eq!(
            reader
                .read(home, |_| Some(json!({"account":"next"})))
                .await
                .unwrap()["account"],
            "next"
        );
    }
}
