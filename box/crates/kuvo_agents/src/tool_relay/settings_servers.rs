//! The computer's MCP servers for a conversation's folder (Settings › MCP
//! Servers and Settings › Integrations, a project's own), read from the same
//! settings files OpenCode reads, in its order, with your sign-ins from its
//! database: OpenCode isn't started to list them. Kuvo Code's own tools aren't
//! among them (each agent is given those anyway), nor those turned off, nor a
//! marketplace app the folder's scope leaves out.

use super::{ComputerServer, ComputerServers, Launch, ServerHost, ServerKind, places::TOOL_SERVER_NAME};
use crate::permissions::{settings_object, turn::project_layers};
use futures_util::{FutureExt, future::BoxFuture};
use http::Method;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// OpenCode's API while it runs, for a server sign-in only (renewing or
/// connecting one again): its JSON answer, or why there's none.
pub trait EngineApi: Send + Sync + 'static {
    fn call<'a>(&'a self, method: Method, path: &'a str, body: Option<Value>, wait: Duration)
    -> BoxFuture<'a, Result<Value, String>>;
}

/// What a command's server runs as while there's no sandbox for it.
pub type Sandbox = Arc<dyn Fn(&Launch, &Path) -> Result<Option<Launch>, String> + Send + Sync>;

/// The servers for each folder, read again at most every few seconds (each
/// agent's turn asks) or at once (a sign-in ran out); the last ones known
/// while a settings file can't be read, none before.
pub struct SettingsServers {
    home: PathBuf,
    engine: Arc<dyn EngineApi>,
    read: Mutex<HashMap<PathBuf, (Instant, ComputerServers)>>,
    sandbox: Option<Sandbox>,
}

const MARKETPLACE_SCOPE: &str = ".kuvo-marketplace.json";
const MARKETPLACE_PREFIX: &str = "marketplace-";
const READ_AGAIN: Duration = Duration::from_secs(5);
const WAIT: Duration = Duration::from_secs(10);

impl SettingsServers {
    /// `home`: the box's. `sandbox` runs a command's server in the box's sandbox while it's on.
    pub fn new(home: PathBuf, engine: Arc<dyn EngineApi>, sandbox: Option<Sandbox>) -> Self {
        Self { home, engine, read: Mutex::default(), sandbox }
    }

    async fn read(&self, directory: &Path) -> Result<ComputerServers, String> {
        let (home, folder) = (self.home.clone(), directory.to_owned());
        let layers = tokio::task::spawn_blocking(move || layers(&home, &folder)).await.map_err(|error| error.to_string())??;
        let mut servers = servers_in(&layers, &marketplace_apps(directory));
        let signing = servers.iter_mut().filter(|server| server.1).map(|server| &mut server.0).collect::<Vec<_>>();
        self.sign_in(signing, directory).await;
        let servers = servers.into_iter().map(|(server, _)| server).collect::<Vec<_>>();
        let mut names = servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>();
        names.sort_unstable();
        let key = names.join(" ");
        Ok(ComputerServers { servers, key })
    }

    /// A URL's server you signed in to in Settings (its maker's sign-in,
    /// which OpenCode keeps and renews): its sign-in added, unless its
    /// settings give one of their own. One that ran out is renewed first,
    /// which starts OpenCode (a sign-in).
    async fn sign_in(&self, servers: Vec<&mut ComputerServer>, directory: &Path) {
        for server in servers {
            let ServerKind::Remote { url, headers, expires: until } = &mut server.kind else { continue };
            if headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("authorization")) {
                continue;
            }
            let integration = integration_id(&server.name, url);
            let mut value = self.access(&integration).await;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as f64;
            if value.as_ref().is_some_and(|(_, expires)| *expires > 0.0 && *expires < now + 60_000.0) {
                // (Briefly: an agent may be waiting for it to start.)
                let connect = located(&format!("/api/experimental/mcp/{}/connect", encode(&server.name)), directory);
                let _ = self.engine.call(Method::POST, &connect, Some(json!({})), WAIT).await;
                value = self.access(&integration).await;
            }
            let Some((access, expires)) = value else { continue };
            headers.push(("Authorization".into(), format!("Bearer {access}")));
            if expires > 0.0 {
                *until = Some(UNIX_EPOCH + Duration::from_millis(expires as u64));
            }
        }
    }

    /// The access token of an integration's active sign-in and when it runs
    /// out (ms), from OpenCode's database, read only.
    async fn access(&self, integration: &str) -> Option<(String, f64)> {
        let database = std::env::var_os("KUVO_ENGINE_DATA")
            .filter(|value| !value.is_empty())
            .map_or_else(|| self.home.join("data/opencode"), PathBuf::from)
            .join("opencode.db");
        let integration = integration.to_owned();
        let value = tokio::task::spawn_blocking(move || -> Option<String> {
            if !database.is_file() {
                return None;
            }
            let connection = kuvo_store::sqlite::open_index(&database, Duration::from_secs(2)).ok()?;
            connection
                .query_row(
                    "SELECT value FROM credential WHERE integration_id = ?1 AND active = 1 ORDER BY time_updated DESC LIMIT 1",
                    [&integration],
                    |row| row.get(0),
                )
                .ok()
        })
        .await
        .ok()??;
        let value: Value = serde_json::from_str(&value).ok()?;
        (value["type"] == "oauth").then_some(())?;
        Some((value["access"].as_str()?.to_owned(), value["expires"].as_f64().unwrap_or(0.0)))
    }
}

impl ServerHost for SettingsServers {
    fn servers<'a>(&'a self, directory: &'a Path, fresh: bool) -> BoxFuture<'a, ComputerServers> {
        async move {
            let known = self.read.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(directory).cloned();
            if let Some((at, servers)) = &known
                && !fresh
                && at.elapsed() < READ_AGAIN
            {
                return servers.clone();
            }
            match self.read(directory).await {
                Ok(servers) => {
                    let mut read = self.read.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    read.insert(directory.to_owned(), (Instant::now(), servers.clone()));
                    servers
                }
                Err(error) => {
                    eprintln!("Kuvo could not read the MCP servers for {}: {error}", directory.display());
                    known.map(|(_, servers)| servers).unwrap_or_default()
                }
            }
        }
        .boxed()
    }

    /// For the conversation's folder and OpenCode's own: it renews your
    /// sign-in if it can, or finds it can't (and Settings › Integrations
    /// says it needs signing in again).
    fn reconnect<'a>(&'a self, directory: &'a Path, name: &'a str) -> BoxFuture<'a, ()> {
        async move {
            let connect = format!("/api/experimental/mcp/{}/connect", encode(name));
            let wait = Duration::from_secs(30);
            for path in [located(&connect, directory), connect] {
                let _ = self.engine.call(Method::POST, &path, Some(json!({})), wait).await;
            }
        }
        .boxed()
    }

    fn sandboxed(&self, launch: &Launch, directory: &Path) -> Result<Option<Launch>, String> {
        self.sandbox.as_ref().map_or(Ok(None), |sandbox| sandbox(launch, directory))
    }
}

/// The settings OpenCode reads for a folder, in its order: this computer's
/// (`config/opencode`, then on a machine the file the box hands it), then the
/// project's, from its repository's top down. Each `mcp.servers`.
fn layers(home: &Path, folder: &Path) -> Result<Vec<Value>, String> {
    let mut files = vec![home.join("config/opencode/opencode.json"), home.join("config/opencode/opencode.jsonc")];
    let mac = std::fs::read_to_string(home.join("box/kind")).is_ok_and(|kind| kind.trim() == "mac");
    if !mac {
        files.extend(["box/opencode.json", "keys/settings.json"].map(|file| home.join(file)).into_iter().find(|file| file.is_file()));
    }
    files.extend(project_layers(folder));
    let mut layers = Vec::new();
    for file in files {
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{}: {error}", file.display())),
        };
        let text = substitute(&text, file.parent().unwrap_or(folder));
        let settings = settings_object(text.as_bytes()).map_err(|error| format!("{}: {error}", file.display()))?;
        layers.push(settings["mcp"]["servers"].clone());
    }
    Ok(layers)
}

/// OpenCode's variables in its settings: `{env:NAME}` (empty when unset) and
/// `{file:path}` (relative to the file's folder; not on a `//` line), as JSON
/// string contents.
fn substitute(text: &str, folder: &Path) -> String {
    let quoted = |value: &str| json!(value).to_string().trim_matches('"').to_owned();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("{env:").into_iter().chain(rest.find("{file:")).min() {
        let Some(length) = rest[at..].find('}') else { break };
        let (before, reference) = (&rest[..at], &rest[at..at + length + 1]);
        out.push_str(before);
        let line = out.rsplit('\n').next().unwrap_or_default();
        if let Some(name) = reference.strip_prefix("{env:") {
            out.push_str(&quoted(&std::env::var(&name[..name.len() - 1]).unwrap_or_default()));
        } else if line.trim_start().starts_with("//") {
            out.push_str(reference);
        } else {
            let path = &reference[6..reference.len() - 1];
            let path = match path.strip_prefix("~/") {
                Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
                None => folder.join(path),
            };
            out.push_str(&quoted(std::fs::read_to_string(path).unwrap_or_default().trim()));
        }
        rest = &rest[at + length + 1..];
    }
    out.push_str(rest);
    out
}

/// OpenCode's name for a URL server's sign-in.
fn integration_id(name: &str, url: &str) -> String {
    let digest = Sha1::new().chain_update(name).chain_update([0]).chain_update(url).finalize();
    format!("mcp_{}", &hex::encode(digest)[..16])
}

/// The servers in the settings, in the order they're read, a later one's in
/// place of an earlier one's of the same name; each with whether it's one
/// you sign in to.
fn servers_in(layers: &[Value], apps: &Option<Vec<String>>) -> Vec<(ComputerServer, bool)> {
    let mut named = Map::new();
    for servers in layers.iter().filter_map(Value::as_object) {
        named.extend(servers.clone());
    }
    let strings = |value: &Value| {
        let all = value.as_object().into_iter().flatten();
        all.filter_map(|(name, text)| Some((name.clone(), text.as_str()?.to_owned()))).collect::<Vec<_>>()
    };
    named
        .iter()
        .filter(|(name, entry)| {
            let left_out = name.strip_prefix(MARKETPLACE_PREFIX).zip(apps.as_ref()).is_some_and(|(app, apps)| !apps.iter().any(|one| one == app));
            !left_out
                && name.as_str() != TOOL_SERVER_NAME
                && good_name(name)
                && entry.is_object()
                && entry["disabled"] != true
                && entry["enabled"] != false
        })
        .filter_map(|(name, entry)| {
            let timeout = entry["timeout"]["execution"].as_f64().filter(|time| *time > 0.0).unwrap_or(300_000.0);
            let timeout = Duration::from_millis(timeout as u64);
            let kind = match entry["type"].as_str() {
                Some("local") => {
                    let command = entry["command"].as_array()?;
                    let command = command.iter().map(|part| part.as_str().map(Into::into)).collect::<Option<Vec<_>>>()?;
                    let cwd = entry["cwd"].as_str().map(PathBuf::from).filter(|cwd| cwd.is_absolute());
                    let env = strings(&entry["environment"]).into_iter().map(|(name, value)| (name.into(), value.into()));
                    (!command.is_empty()).then(|| ServerKind::Local { command, env: env.collect(), cwd })?
                }
                Some("remote") => {
                    let url = entry["url"].as_str().filter(|url| url.starts_with("http://") || url.starts_with("https://"))?;
                    ServerKind::Remote { url: url.into(), headers: strings(&entry["headers"]), expires: None }
                }
                _ => return None,
            };
            let signs_in = matches!(kind, ServerKind::Remote { .. }) && entry["oauth"] != false;
            Some((ComputerServer { name: name.clone(), timeout, kind }, signs_in))
        })
        .collect()
}

/// A server's name as every agent's settings take it (Codex's are dotted
/// keys, Claude's tool names have no dots).
fn good_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 64
        && bytes.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// The marketplace apps a folder's scope keeps (none: all of them).
fn marketplace_apps(directory: &Path) -> Option<Vec<String>> {
    let scope: Value = serde_json::from_slice(&std::fs::read(directory.join(MARKETPLACE_SCOPE)).ok()?).ok()?;
    let strings = |value: &Value| value.as_array()?.iter().map(|one| one.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>();
    let whole = ["request", "agent", "version"].iter().all(|key| scope[key].is_string()) && strings(&scope["skills"]).is_some();
    strings(&scope["apps"]).filter(|_| whole)
}

/// OpenCode's path for a conversation's folder.
fn located(path: &str, directory: &Path) -> String {
    format!("{path}?location%5Bdirectory%5D={}", encode(&directory.to_string_lossy()))
}

fn encode(name: &str) -> String {
    percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC).to_string()
}
