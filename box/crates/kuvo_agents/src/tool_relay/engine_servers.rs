//! The computer's MCP servers as its engine has them for a conversation's
//! folder (Settings › MCP Servers and Settings › Integrations, a project's
//! own), keys in, read through the engine's own API. Kuvo Code's own tools
//! aren't among them (each agent is given those anyway), nor those turned
//! off, nor a marketplace app the folder's scope leaves out.

use super::{ComputerServer, ComputerServers, Launch, ServerHost, ServerKind, places::TOOL_SERVER_NAME};
use futures_util::{FutureExt, future::BoxFuture};
use http::Method;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The engine's API while it runs: its JSON answer, or why there's none
/// (it isn't running: never started for this).
pub trait EngineApi: Send + Sync + 'static {
    fn call<'a>(&'a self, method: Method, path: &'a str, body: Option<Value>, wait: Duration)
    -> BoxFuture<'a, Result<Value, String>>;
}

/// What a command's server runs as while there's no sandbox for it.
pub type Sandbox = Arc<dyn Fn(&Launch, &Path) -> Result<Option<Launch>, String> + Send + Sync>;

/// The engine's servers for each folder, read again at most every few
/// seconds (each agent's turn asks) or at once (a sign-in ran out); the
/// last ones known while the engine can't say, none before.
pub struct EngineServers {
    engine: Arc<dyn EngineApi>,
    read: Mutex<HashMap<PathBuf, (Instant, ComputerServers)>>,
    sandbox: Option<Sandbox>,
}

const MARKETPLACE_SCOPE: &str = ".kuvo-marketplace.json";
const MARKETPLACE_PREFIX: &str = "marketplace-";
const READ_AGAIN: Duration = Duration::from_secs(5);
const WAIT: Duration = Duration::from_secs(10);

impl EngineServers {
    /// `sandbox` runs a command's server in the box's sandbox while it's on.
    pub fn new(engine: Arc<dyn EngineApi>, sandbox: Option<Sandbox>) -> Self {
        Self { engine, read: Mutex::default(), sandbox }
    }

    async fn read(&self, directory: &Path) -> Result<ComputerServers, String> {
        let answer = self.engine.call(Method::GET, &located("/api/config", directory), None, WAIT).await?;
        let mut servers = servers_in(&answer, &marketplace_apps(directory));
        let signing = servers.iter_mut().filter(|server| server.1).map(|server| &mut server.0).collect::<Vec<_>>();
        self.sign_in(signing, directory).await;
        let servers = servers.into_iter().map(|(server, _)| server).collect::<Vec<_>>();
        let mut names = servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>();
        names.sort_unstable();
        let key = names.join(" ");
        Ok(ComputerServers { servers, key })
    }

    /// A URL's server you signed in to in Settings (its maker's sign-in,
    /// which the engine keeps and renews): its sign-in added, unless its
    /// settings give one of their own. One that ran out is renewed first.
    async fn sign_in(&self, servers: Vec<&mut ComputerServer>, directory: &Path) {
        let at = |path: &str| located(path, directory);
        let mut signing = servers
            .into_iter()
            .filter(|server| match &server.kind {
                ServerKind::Remote { headers, .. } => {
                    !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                }
                ServerKind::Local { .. } => false,
            })
            .collect::<Vec<_>>();
        if signing.is_empty() {
            return;
        }
        let listed = self.engine.call(Method::GET, &at("/api/mcp"), None, WAIT).await.unwrap_or_default();
        let integrations = data(&listed)
            .iter()
            .filter_map(|server| Some((server["name"].as_str()?.to_owned(), server["integrationID"].as_str()?.to_owned())))
            .collect::<HashMap<_, _>>();
        if !signing.iter().any(|server| integrations.contains_key(server.name.as_str())) {
            return;
        }
        let mut list = self.credentials().await;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as f64;
        for server in &mut signing {
            let Some(integration) = integrations.get(server.name.as_str()) else { continue };
            let mut value = access_for(&list, integration);
            if value.as_ref().is_some_and(|(_, expires)| *expires > 0.0 && *expires < now + 60_000.0) {
                // (Briefly: an agent may be waiting for it to start.)
                let connect = at(&format!("/api/experimental/mcp/{}/connect", encode(&server.name)));
                let _ = self.engine.call(Method::POST, &connect, Some(json!({})), WAIT).await;
                list = self.credentials().await;
                value = access_for(&list, integration);
            }
            let (Some((access, expires)), ServerKind::Remote { headers, expires: until, .. }) = (value, &mut server.kind)
            else {
                continue;
            };
            headers.push(("Authorization".into(), format!("Bearer {access}")));
            if expires > 0.0 {
                *until = Some(UNIX_EPOCH + Duration::from_millis(expires as u64));
            }
        }
    }

    async fn credentials(&self) -> Vec<Value> {
        let answer = self.engine.call(Method::GET, "/api/credential", None, WAIT).await;
        answer.ok().map(|answer| answer["data"].as_array().cloned().unwrap_or_default()).unwrap_or_default()
    }
}

impl ServerHost for EngineServers {
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
                Err(_) => known.map(|(_, servers)| servers).unwrap_or_default(),
            }
        }
        .boxed()
    }

    /// For the conversation's folder and the engine's own: it renews your
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

/// The servers in the engine's settings, in the order it reads them (this
/// computer's, then the folder's), a later one's in place of an earlier
/// one's of the same name; each with whether it's one you sign in to.
fn servers_in(answer: &Value, apps: &Option<Vec<String>>) -> Vec<(ComputerServer, bool)> {
    let mut named = Map::new();
    for layer in data(answer) {
        if let Some(servers) = layer["info"]["mcp"]["servers"].as_object() {
            named.extend(servers.clone());
        }
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

fn data(answer: &Value) -> Vec<Value> {
    answer.as_array().or(answer["data"].as_array()).cloned().unwrap_or_default()
}

/// The access token of an integration's active sign-in, and when it runs out (ms).
fn access_for(list: &[Value], integration: &str) -> Option<(String, f64)> {
    let found = list.iter().find(|each| {
        each["integrationID"] == integration && each["active"].as_bool() == Some(true) && each["value"]["type"] == "oauth"
            && each["value"]["access"].is_string()
    })?;
    let expires = found["value"]["expires"].as_f64().unwrap_or(0.0);
    Some((found["value"]["access"].as_str()?.to_owned(), expires))
}

/// An engine's path for a conversation's folder.
fn located(path: &str, directory: &Path) -> String {
    format!("{path}?location%5Bdirectory%5D={}", encode(&directory.to_string_lossy()))
}

fn encode(name: &str) -> String {
    percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC).to_string()
}
