//! The tools relay: Kuvo Code's tools for an agent that runs in the box's
//! sandbox, on 127.0.0.1 over MCP's HTTP, each conversation by a key of its
//! own. On a computer the tools are a program run here, outside the sandbox,
//! one per conversation; on the Mac they're the app's server. Each call is
//! its conversation's, whatever it says: a command in the sandbox could name
//! another's.
//!
//! The address and keys are kept in the box's own folder, so an agent that
//! outlives its resident (held through an update) reaches its tools at the
//! same address with the same key. The next resident keeps only the keys of
//! the conversations whose agents it took up.

mod settings_servers;
mod places;
mod servers;

pub use settings_servers::{EngineApi, Sandbox, SettingsServers};
pub use places::{TOOL_SERVER_NAME, ToolPlaces};
pub use servers::{
    ComputerServer, ComputerServers, Launch, RelayedServer, ServerHost, ServerKind,
};

use futures_util::{StreamExt, future::join_all};
use http::{HeaderValue, Method, Request, Response, StatusCode, header, request::Parts};
use http_body_util::{BodyExt, Full, Limited, StreamBody, combinators::BoxBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use kuvo_os::{
    platform::{Environment, Executable, which},
    process::{ChildGroup, Spawn, Stdio},
};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    convert::Infallible,
    ffi::OsString,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{OnceCell, mpsc, oneshot},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

/// Kuvo Code's tools for a conversation: a program run here, outside the
/// sandbox, one per conversation, which all of its agent's connections
/// share. `key` names how it's set up: a change starts it again.
#[derive(Clone, Debug)]
pub struct Tools {
    pub key: String,
    pub command: PathBuf,
    pub args: Vec<OsString>,
    pub env: Environment,
    /// How long a call may take.
    pub timeout: Duration,
}

/// Says where each conversation's tools are now.
pub trait ToolSource: Send + Sync + 'static {
    fn tools(&self, session: &str) -> Option<Tools>;
}

/// What reaches a conversation's tools through the relay: an agent's thread,
/// say. The key stays while any holds it.
pub type Holder = u64;

/// Kuvo Code's tools as an agent in the sandbox reaches them.
#[derive(Clone, Debug, PartialEq)]
pub struct RelayedTools {
    pub url: String,
    /// The `Authorization` header: the conversation's key.
    pub authorization: String,
    pub timeout: Duration,
}

/// Kuvo Code's tools as an agent is given them.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentTools {
    /// Over MCP's HTTP, with these headers.
    Http {
        key: String,
        url: String,
        headers: Vec<(String, String)>,
        timeout: Duration,
    },
    /// A program the agent starts.
    Stdio {
        key: String,
        command: PathBuf,
        args: Vec<OsString>,
        env: Environment,
        timeout: Duration,
    },
}

/// An agent's environment with the relay's loopback addresses kept off any
/// proxy it's given: where a proxy is set, `NO_PROXY` and `no_proxy` name
/// them too, beside what they named already.
pub fn unproxied_loopback(env: &mut Environment) {
    const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
    let proxied = env.iter().any(|(name, value)| {
        let name = name.to_string_lossy().to_ascii_lowercase();
        matches!(name.as_str(), "http_proxy" | "https_proxy" | "all_proxy") && !value.is_empty()
    });
    if !proxied {
        return;
    }
    // (Windows names are one whatever their case: one of them there.)
    let keys: &[&str] = if cfg!(windows) { &["NO_PROXY"] } else { &["NO_PROXY", "no_proxy"] };
    for &key in keys {
        let same = |name: &std::ffi::OsString| match cfg!(windows) {
            true => name.to_string_lossy().eq_ignore_ascii_case(key),
            false => name.to_str() == Some(key),
        };
        let found = env
            .iter()
            .find(|(name, _)| same(name))
            .map(|(_, value)| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        env.retain(|name, _| !same(name));
        let mut hosts: Vec<String> = found
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(str::to_owned)
            .collect();
        for host in LOOPBACK {
            if !hosts.iter().any(|kept| kept == host) {
                hosts.push(host.to_owned());
            }
        }
        env.insert(key.into(), hosts.join(",").into());
    }
}

/// How long the forwarder waits for a relay that's restarting, on top of a call's time.
const FORWARDER_WAITS: Duration = Duration::from_secs(80);

/// How long a server has to start for an agent that gives up its whole
/// session over one (ACP's), a command's longer as it may fetch its package
/// the first time; how long an agent's next message waits to hear whether
/// one that wasn't there answers now (the check goes on without it); how
/// often one that isn't there is checked, first a few minutes on, then
/// twice as long each time, up to the most; and how long a command's server
/// no connection uses runs.
#[derive(Clone, Copy, Debug)]
pub struct RelayTiming {
    pub starts: Duration,
    pub command_starts: Duration,
    pub check: Duration,
    pub check_every: Duration,
    pub check_at_most: Duration,
    pub idle: Duration,
}

impl Default for RelayTiming {
    fn default() -> Self {
        Self {
            starts: Duration::from_secs(15),
            command_starts: Duration::from_secs(25),
            check: Duration::from_secs(1),
            check_every: Duration::from_secs(3 * 60),
            check_at_most: Duration::from_secs(30 * 60),
            idle: Duration::from_secs(60 * 60),
        }
    }
}

#[derive(Clone)]
pub struct ToolRelay(Arc<Inner>);

struct Inner {
    record: Option<PathBuf>,
    source: Arc<dyn ToolSource>,
    state: Mutex<State>,
    listening: OnceCell<(u16, JoinHandle<()>)>,
    stop: CancellationToken,
    client: reqwest::Client,
    host: Arc<dyn ServerHost>,
}

#[derive(Default)]
struct State {
    keys: HashMap<String, String>,
    sessions: HashMap<String, String>,
    holders: HashMap<String, HashSet<Holder>>,
    programs: HashMap<String, Arc<Program>>,
    servers: servers::Servers,
    timing: RelayTiming,
    /// The box's program, whose `mcp` forwards an agent's tools to the
    /// relay and waits for it through an update.
    forwarder: Option<PathBuf>,
}

#[derive(Default, Deserialize, Serialize)]
struct Record {
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

const TOOLS: &str = "Kuvo Code's tools";
const BODY_LIMIT: usize = 32 << 20;

type Body = BoxBody<Bytes, io::Error>;

impl ToolRelay {
    /// A relay keeping its address and keys in `record` (none: kept only
    /// while it runs). It listens once something first holds a key.
    pub fn new(record: Option<PathBuf>, source: Arc<dyn ToolSource>) -> io::Result<Self> {
        Self::with_host(record, source, Arc::new(servers::NoHost))
    }

    /// A relay for Kuvo Code's tools and the computer's servers, which
    /// `host` reads, renews and runs.
    pub fn with_host(
        record: Option<PathBuf>,
        source: Arc<dyn ToolSource>,
        host: Arc<dyn ServerHost>,
    ) -> io::Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                // Only where the server itself moved it on its own site: a
                // sign-in page elsewhere isn't its answer, and its keys
                // aren't for another site.
                let same_site = attempt.previous().last().is_some_and(|before| {
                    let next = attempt.url();
                    before.host_str() == next.host_str()
                        && (before.scheme() == next.scheme() && before.port() == next.port()
                            || before.scheme() == "http" && next.scheme() == "https")
                });
                let moved = matches!(attempt.status().as_u16(), 307 | 308);
                if moved && same_site && attempt.previous().len() <= 3 {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(io::Error::other)?;
        Ok(Self(Arc::new(Inner {
            record,
            source,
            state: Mutex::new(State::default()),
            listening: OnceCell::new(),
            stop: CancellationToken::new(),
            client,
            host,
        })))
    }

    pub fn set_timing(&self, timing: RelayTiming) {
        self.state().timing = timing;
    }

    /// Where the box keeps agents through its updates (a computer's):
    /// `program mcp` forwards to the relay.
    pub fn set_forwarder(&self, program: Option<PathBuf>) {
        self.state().forwarder = program;
    }

    /// Kuvo Code's tools as `session`'s agent is given them, held by
    /// `holder`: through the relay for one in the sandbox (`fenced`), and by
    /// the forwarder where the box keeps agents through its updates; as
    /// they are otherwise. None until the box knows where they are.
    pub async fn agent_tools(&self, session: &str, holder: Holder, fenced: bool) -> io::Result<Option<AgentTools>> {
        let Some(Tools { key, command, args, env, timeout }) = self.0.source.tools(session) else {
            return Ok(None);
        };
        if !fenced {
            return Ok(Some(AgentTools::Stdio { key, command, args, env, timeout }));
        }
        let forwarder = self.state().forwarder.clone();
        let Some(relayed) = self.relayed_tools(session, holder).await? else {
            return Ok(None);
        };
        let RelayedTools { url, authorization, timeout } = relayed;
        let Some(command) = forwarder else {
            let headers = vec![("Authorization".into(), authorization)];
            return Ok(Some(AgentTools::Http { key, url, headers, timeout }));
        };
        let env = [
            ("KUVO_TOOL_RELAY", url),
            ("KUVO_TOOL_RELAY_KEY", authorization),
            ("KUVO_TOOL_TIMEOUT", timeout.as_millis().to_string()),
        ];
        Ok(Some(AgentTools::Stdio {
            key: format!("forward:{key}"),
            command,
            args: vec!["mcp".into()],
            env: env.into_iter().map(|(name, value)| (name.into(), value.into())).collect(),
            timeout: timeout + FORWARDER_WAITS,
        }))
    }

    /// Kuvo Code's tools for a conversation straight from the relay, over HTTP (an agent with
    /// one process for every conversation, where a program per conversation would be many).
    pub async fn http_tools(&self, session: &str, holder: Holder) -> io::Result<Option<AgentTools>> {
        let Some(Tools { key, .. }) = self.0.source.tools(session) else {
            return Ok(None);
        };
        let Some(RelayedTools { url, authorization, timeout }) = self.relayed_tools(session, holder).await? else {
            return Ok(None);
        };
        Ok(Some(AgentTools::Http { key, url, headers: vec![("Authorization".into(), authorization)], timeout }))
    }

    /// The relay's port, listening from then on: the one it had before when
    /// that's free, with the keys it gave then.
    pub async fn port(&self) -> io::Result<u16> {
        let (port, _) = self
            .0
            .listening
            .get_or_try_init(|| async {
                let kept = self.read_record();
                {
                    let mut state = self.state();
                    for (session, key) in kept.keys {
                        if !state.keys.contains_key(&session) {
                            state.sessions.insert(key.clone(), session.clone());
                            state.keys.insert(session, key);
                        }
                    }
                }
                let any = ("127.0.0.1", 0);
                let listener = match kept.port {
                    Some(port) => match TcpListener::bind(("127.0.0.1", port)).await {
                        Ok(listener) => listener,
                        Err(_) => {
                            // (Taken meanwhile: agents the resident before
                            // started reach their tools once they start again.)
                            eprintln!("The tools relay's port {port} is taken: it listens on another.");
                            TcpListener::bind(any).await?
                        }
                    },
                    None => TcpListener::bind(any).await?,
                };
                let port = listener.local_addr()?.port();
                let relay = Arc::downgrade(&self.0);
                tokio::spawn(sweep(relay.clone(), self.0.stop.clone()));
                let accepting = tokio::spawn(accept(relay, listener, self.0.stop.clone()));
                Ok::<_, io::Error>((port, accepting))
            })
            .await?;
        let port = *port;
        self.keep();
        Ok(port)
    }

    /// `holder` reaches `session`'s tools through the relay: its key.
    pub async fn hold(&self, session: &str, holder: Holder) -> io::Result<String> {
        self.port().await?;
        let mut state = self.state();
        state.holders.entry(session.to_owned()).or_default().insert(holder);
        if let Some(key) = state.keys.get(session) {
            return Ok(key.clone());
        }
        let key = fresh(24);
        state.keys.insert(session.to_owned(), key.clone());
        state.sessions.insert(key.clone(), session.to_owned());
        drop(state);
        self.keep();
        Ok(key)
    }

    /// Kuvo Code's tools as an agent in the sandbox reaches them for
    /// `session`, held by `holder` until it drops them; none until the box
    /// knows where they are.
    pub async fn relayed_tools(
        &self,
        session: &str,
        holder: Holder,
    ) -> io::Result<Option<RelayedTools>> {
        let Some(tools) = self.0.source.tools(session) else {
            return Ok(None);
        };
        let key = self.hold(session, holder).await?;
        let timeout = tools.timeout;
        Ok(Some(RelayedTools {
            url: format!("http://127.0.0.1:{}/mcp", self.port().await?),
            authorization: format!("Bearer {key}"),
            timeout,
        }))
    }

    /// Whether this conversation's key belongs to this resident.
    pub fn uses(&self, session: &str) -> bool {
        self.state().keys.contains_key(session)
    }

    /// As a resident starts: keys kept for conversations none of whose
    /// agents it took up from the one before go.
    pub fn keep_keys(&self, sessions: &BTreeSet<String>) {
        {
            let mut state = self.state();
            let State { keys, sessions: by_key, holders, .. } = &mut *state;
            keys.retain(|session, key| {
                let kept = sessions.contains(session) || holders.contains_key(session);
                if !kept {
                    by_key.remove(key);
                }
                kept
            });
        }
        self.keep();
    }

    /// `holder` is done with `session`'s tools: once nothing else holds
    /// them, its key goes and its program stops.
    pub fn drop_relayed(&self, session: &str, holder: Holder) {
        let stopping = {
            let mut state = self.state();
            let Some(held) = state.holders.get_mut(session) else {
                return;
            };
            held.remove(&holder);
            if !held.is_empty() {
                return;
            }
            state.holders.remove(session);
            if let Some(key) = state.keys.remove(session) {
                state.sessions.remove(&key);
            }
            let mut stopping = state.servers.forget(session);
            stopping.extend(state.programs.remove(session));
            stopping
        };
        stopping.iter().for_each(|program| program.stop());
        self.keep();
    }

    /// Stop listening, and stop the programs. The record stays for the next.
    pub async fn shutdown(&self) {
        self.0.stop.cancel();
        let programs = {
            let mut state = self.state();
            let mut all = std::mem::take(&mut state.programs).into_values().collect::<Vec<_>>();
            all.extend(state.servers.stop_all());
            all
        };
        programs.iter().for_each(|program| program.stop());
        if let Some((_, accepting)) = self.0.listening.get() {
            accepting.abort();
        }
        // (The listener is the accept loop's: wait until it's let go of.)
        for _ in 0..100 {
            if self.0.listening.get().is_none_or(|(_, task)| task.is_finished()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn timing(&self) -> RelayTiming {
        self.state().timing
    }

    fn read_record(&self) -> Record {
        let path = self.0.record.as_ref();
        let bytes = path.and_then(|path| files::read(path).ok().flatten());
        bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    fn keep(&self) {
        let (Some(path), Some((port, _))) = (&self.0.record, self.0.listening.get()) else {
            return;
        };
        let keys = self.state().keys.clone().into_iter().collect();
        let record = Record { port: Some(*port), keys };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| files::write_secret(path, &serde_json::to_vec(&record)?));
        if let Err(error) = written {
            eprintln!("The tools relay's record couldn't be written: {error}");
        }
    }
}

async fn accept(relay: Weak<Inner>, listener: TcpListener, stop: CancellationToken) {
    loop {
        let socket = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok((socket, _)) => socket,
                Err(_) => continue,
            },
        };
        let Some(inner) = relay.upgrade() else { return };
        let relay = ToolRelay(inner);
        let stop = stop.clone();
        tokio::spawn(async move {
            let connection = http1::Builder::new().serve_connection(
                TokioIo::new(socket),
                service_fn(move |request| {
                    let relay = relay.clone();
                    async move { Ok::<_, Infallible>(relay.answer(request).await) }
                }),
            );
            tokio::pin!(connection);
            tokio::select! {
                _ = &mut connection => {}
                () = stop.cancelled() => {}
            }
        });
    }
}

/// A command's server no connection has used for a while stops (an agent
/// that started again has a new one), as the engine stops its own.
async fn sweep(relay: Weak<Inner>, stop: CancellationToken) {
    loop {
        let Some(every) = relay.upgrade().map(|inner| ToolRelay(inner).timing().idle / 12) else {
            return;
        };
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(every) => {}
        }
        let Some(inner) = relay.upgrade() else { return };
        let relay = ToolRelay(inner);
        let idle = relay.timing().idle;
        relay.state().servers.idle(idle).iter().for_each(|program| program.stop());
    }
}

impl ToolRelay {
    async fn answer(&self, request: Request<Incoming>) -> Response<Body> {
        let key = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|key| !key.is_empty() && !key.contains(char::is_whitespace));
        let Some(session) = key.and_then(|key| self.state().sessions.get(key).cloned()) else {
            return text(StatusCode::UNAUTHORIZED, "Not allowed.");
        };
        let target = match request.uri().path().strip_prefix("/mcp/") {
            Some(named) => match percent_encoding::percent_decode_str(named).decode_utf8() {
                Ok(name) => Target::Server(name.into_owned()),
                Err(_) => return text(StatusCode::NOT_FOUND, "No such server here."),
            },
            None => match self.0.source.tools(&session) {
                Some(tools) => Target::Tools(tools),
                None => return text(StatusCode::SERVICE_UNAVAILABLE, "Kuvo Code's tools aren't here now."),
            },
        };
        let forwarded = request
            .headers()
            .get("x-kuvo-tool-forwarder")
            .is_some_and(|value| value == "1");
        let (parts, incoming) = request.into_parts();
        let body = if parts.method == Method::POST {
            match Limited::new(incoming, BODY_LIMIT).collect().await {
                Ok(bytes) => Some(bytes.to_bytes()),
                Err(_) => return text(StatusCode::PAYLOAD_TOO_LARGE, "Too much at once."),
            }
        } else {
            None
        };
        match target {
            Target::Server(name) => self.answer_server(parts, body, &session, &name).await,
            Target::Tools(tools) => self.answer_tools(parts, body, &session, forwarded, tools).await,
        }
    }

    async fn answer_tools(
        &self,
        parts: Parts,
        body: Option<Bytes>,
        session: &str,
        forwarded: bool,
        tools: Tools,
    ) -> Response<Body> {
        let body = match body.map(|bytes| serde_json::from_slice::<Value>(&bytes)) {
            Some(Ok(body)) => own_session(body, session, forwarded),
            Some(Err(_)) => return parse_error(),
            None => {
                let mut reply = text(StatusCode::METHOD_NOT_ALLOWED, "");
                reply.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
                return reply;
            }
        };
        let Tools { key, command, args, env, timeout } = tools;
        let program = {
            let mut state = self.state();
            match state.programs.get(session) {
                Some(program) if !program.stopped() && program.key == key => program.clone(),
                _ => {
                    if let Some(old) = state.programs.remove(session) {
                        old.stop();
                    }
                    // (By its name on the PATH, as the engine starts it.)
                    let command = which(command.as_os_str(), &env, &[]).unwrap_or(Executable::Native(command));
                    match Program::start(key, Launch { command, args, env }, None, TOOLS.into()) {
                        Ok(program) => {
                            state.programs.insert(session.to_owned(), program.clone());
                            program
                        }
                        Err(error) => {
                            eprintln!("Kuvo Code's tools couldn't start: {error}");
                            return text(StatusCode::BAD_GATEWAY, "Kuvo Code's tools didn't start.");
                        }
                    }
                }
            }
        };
        let messages = servers::messages(Some(&body));
        // Each of its connections (Codex has one a thread) by a name of the
        // relay's, given as it starts: their requests are told apart by it.
        let starts = messages.iter().any(|message| message["method"] == "initialize");
        let connection = if starts { fresh(12) } else { connection_of(&parts) };
        let answers =
            join_all(messages.into_iter().map(|message| program.send(message, timeout, &connection))).await;
        servers::replies(&body, answers, starts.then_some(connection.as_str()))
    }

    /// A request on to a server over HTTP with `headers`: what it answers.
    /// A call has `timeout`, a stream of what comes as long as it's read; a
    /// start is given up at `deadline` (and untied once it has started).
    async fn send_on(
        &self,
        parts: &Parts,
        url: &str,
        headers: &[(String, String)],
        body: Option<Bytes>,
        timeout: Option<Duration>,
        deadline: Option<Instant>,
    ) -> Answer {
        let mut request = self.0.client.request(parts.method.clone(), url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let accept = parts.headers.get(header::ACCEPT).cloned();
        let accept = accept.unwrap_or(HeaderValue::from_static("application/json, text/event-stream"));
        request = request.header(header::ACCEPT, accept);
        for key in ["content-type", "mcp-session-id", "mcp-protocol-version", "last-event-id"] {
            if let Some(value) = parts.headers.get(key) {
                request = request.header(key, value);
            }
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        if let Some(timeout) = timeout.filter(|_| parts.method == Method::POST) {
            request = request.timeout(timeout);
        }
        let sent = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, request.send()).await.ok(),
            None => Some(request.send().await),
        };
        let Some(Ok(response)) = sent else { return Answer::failed() };
        Answer {
            status: response.status().as_u16(),
            headers: ["content-type", "mcp-session-id"]
                .into_iter()
                .filter_map(|key| Some((key, response.headers().get(key)?.as_bytes().to_vec())))
                .collect(),
            response: Some(response),
        }
    }
}

/// What a request is for: one of the computer's servers, or Kuvo Code's tools.
enum Target {
    Server(String),
    Tools(Tools),
}

/// A server's answer as it came.
struct Answer {
    status: u16,
    headers: Vec<(&'static str, Vec<u8>)>,
    response: Option<reqwest::Response>,
}

impl Answer {
    fn failed() -> Self {
        Self { status: 502, headers: Vec::new(), response: None }
    }

    fn reply(&self, body: Body) -> Response<Body> {
        let mut reply = Response::builder().status(self.status);
        for (key, value) in &self.headers {
            reply = reply.header(*key, value.as_slice());
        }
        reply.body(body).unwrap_or_else(|_| text(StatusCode::BAD_GATEWAY, ""))
    }

    fn into_reply(mut self, failed: &'static str) -> Response<Body> {
        match self.response.take() {
            Some(response) => self.reply(stream_of(response)),
            None => text(StatusCode::BAD_GATEWAY, failed),
        }
    }
}

fn stream_of(response: reqwest::Response) -> Body {
    let chunks = response.bytes_stream().map(|chunk| chunk.map(Frame::data).map_err(io::Error::other));
    BoxBody::new(StreamBody::new(chunks))
}

/// Each call is the relay's conversation's, whatever it names, with an
/// operation of its own (the forwarder's, sent again after a restart, kept).
fn own_session(body: Value, session: &str, forwarded: bool) -> Value {
    let each = |mut message: Value| {
        let call = message["method"] == "tools/call";
        let Some(params) = message["params"].as_object_mut().filter(|_| call) else {
            return message;
        };
        let meta = params.entry("_meta").or_insert_with(|| Value::Object(Map::new()));
        if !meta.is_object() {
            *meta = Value::Object(Map::new());
        }
        let operation = meta["kuvo.operation"].as_str().filter(|operation| {
            forwarded
                && operation.len() == 36
                && operation.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
        });
        let operation = operation.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
        meta["ai.opencode/sessionID"] = json!(session);
        meta["kuvo.operation"] = json!(operation);
        message
    };
    match body {
        Value::Array(all) => Value::Array(all.into_iter().map(each).collect()),
        one => each(one),
    }
}

/// A name of the relay's own, hard to guess.
fn fresh(bytes: usize) -> String {
    let mut name = vec![0; bytes];
    let _ = getrandom::fill(&mut name);
    hex::encode(name)
}

fn connection_of(parts: &Parts) -> String {
    let connection = parts.headers.get("mcp-session-id").and_then(|value| value.to_str().ok());
    connection.unwrap_or_default().to_owned()
}

fn text(status: StatusCode, words: &'static str) -> Response<Body> {
    let mut reply = Response::new(full(Bytes::from_static(words.as_bytes())));
    *reply.status_mut() = status;
    reply
}

fn json_reply(status: StatusCode, body: &Value, connection: Option<&str>) -> Response<Body> {
    let mut reply = Response::new(full(Bytes::from(body.to_string())));
    *reply.status_mut() = status;
    let headers = reply.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(connection) = connection.and_then(|name| HeaderValue::from_str(name).ok()) {
        headers.insert("mcp-session-id", connection);
    }
    reply
}

fn parse_error() -> Response<Body> {
    let error = json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}});
    json_reply(StatusCode::BAD_REQUEST, &error, None)
}

fn full(bytes: Bytes) -> Body {
    BoxBody::new(Full::new(bytes).map_err(|never| match never {}))
}

/// A program spoken to over its input and output, which each of its
/// agent's connections share, their requests numbered anew here so theirs
/// never meet.
struct Program {
    key: String,
    name: String,
    input: mpsc::UnboundedSender<String>,
    calls: Mutex<Calls>,
    group: Mutex<Option<ChildGroup>>,
}

#[derive(Default)]
struct Calls {
    next: i64,
    waiting: HashMap<i64, (Value, oneshot::Sender<Value>)>,
    /// Each connection's request, as the program knows it (for a cancel).
    ours: HashMap<String, i64>,
    stopped: bool,
    /// When a connection last used it.
    used: Option<Instant>,
}

impl Program {
    fn start(key: String, launch: Launch, cwd: Option<PathBuf>, name: String) -> io::Result<Arc<Self>> {
        let Launch { command, args, mut env } = launch;
        // (Windows keeps each drive's folder in variables named `=C:`: not
        // a program's to be given.)
        env.retain(|name, _| !name.is_empty() && !name.as_encoded_bytes().contains(&b'='));
        let spawn = Spawn {
            executable: command,
            args,
            cwd,
            env,
            stdin: Stdio::Pipe,
            stdout: Stdio::Pipe,
            // (What it says on stderr isn't kept: a server's own can have its keys in it.)
            stderr: Stdio::Null,
        };
        let mut group = spawn.spawn()?;
        let mut stdin = group.stdin.take().ok_or_else(|| io::Error::other("no input"))?;
        let stdout = group.stdout.take().ok_or_else(|| io::Error::other("no output"))?;
        let (input, mut writes) = mpsc::unbounded_channel::<String>();
        std::thread::Builder::new().name("relay-program-input".into()).spawn(move || {
            while let Some(line) = writes.blocking_recv() {
                if writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_err() {
                    break;
                }
            }
        })?;
        let (lines, mut read) = mpsc::unbounded_channel::<String>();
        std::thread::Builder::new().name("relay-program-output".into()).spawn(move || {
            let mut said = io::BufReader::new(stdout).lines();
            while let Some(Ok(line)) = said.next() {
                if lines.send(line).is_err() {
                    break;
                }
            }
        })?;
        let calls = Calls { next: 1, used: Some(Instant::now()), ..Calls::default() };
        let program = Arc::new(Self {
            key,
            name,
            input,
            calls: Mutex::new(calls),
            group: Mutex::new(Some(group)),
        });
        let reading = Arc::downgrade(&program);
        tokio::spawn(async move {
            while let Some(line) = read.recv().await {
                let Some(program) = reading.upgrade() else { return };
                program.heard(&line);
            }
            if let Some(program) = reading.upgrade() {
                program.stop();
            }
        });
        Ok(program)
    }

    fn calls(&self) -> MutexGuard<'_, Calls> {
        self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stopped(&self) -> bool {
        self.calls().stopped
    }

    fn unused_for(&self, idle: Duration) -> bool {
        self.calls().used.is_some_and(|used| used.elapsed() > idle)
    }

    fn write(&self, message: &Value) {
        let _ = self.input.send(message.to_string());
    }

    fn heard(&self, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return;
        };
        // What it asks the agent isn't passed on: it's told so at once.
        if message["method"].is_string() {
            if !message["id"].is_null() {
                self.write(&json!({"jsonrpc":"2.0","id":message["id"],
                    "error":{"code":-32601,"message":"Kuvo Code doesn't pass this on to the agent."}}));
            }
            return;
        }
        let Some(id) = message["id"].as_i64() else { return };
        let Some((original, done)) = self.calls().waiting.remove(&id) else {
            return;
        };
        let mut answer = message;
        answer["id"] = original;
        let _ = done.send(answer);
    }

    /// A connection's message; a request's answer (none for a notification).
    async fn send(&self, mut message: Value, timeout: Duration, connection: &str) -> Option<Value> {
        self.calls().used = Some(Instant::now());
        let Some(original) = message.get("id").cloned().filter(|id| !id.is_null()) else {
            // A cancel names the request as the program knows it.
            let request = message["params"].get("requestId");
            let ours = request.and_then(|request| self.calls().ours.get(&format!("{connection} {request}")).copied());
            match ours {
                Some(ours) => message["params"]["requestId"] = json!(ours),
                None if message["method"] == "notifications/cancelled" => return None,
                None => {}
            }
            if !self.stopped() {
                self.write(&message);
            }
            return None;
        };
        let known = format!("{connection} {original}");
        let (done, answered) = oneshot::channel();
        let id = {
            let mut calls = self.calls();
            if calls.stopped {
                return Some(stopped(&original, &self.name));
            }
            let id = calls.next;
            calls.next += 1;
            calls.ours.insert(known.clone(), id);
            calls.waiting.insert(id, (original.clone(), done));
            id
        };
        message["id"] = json!(id);
        self.write(&message);
        let answer = tokio::time::timeout(timeout, answered).await;
        self.calls().ours.remove(&known);
        match answer {
            Ok(Ok(answer)) => Some(answer),
            Ok(Err(_)) => Some(stopped(&original, &self.name)),
            Err(_) => {
                self.calls().waiting.remove(&id);
                self.write(&json!({"jsonrpc":"2.0","method":"notifications/cancelled",
                    "params":{"requestId":id,"reason":"Took too long."}}));
                Some(failure(&original, &format!("{} took too long.", self.name)))
            }
        }
    }

    fn stop(&self) {
        let waiting = {
            let mut calls = self.calls();
            if calls.stopped {
                return;
            }
            calls.stopped = true;
            std::mem::take(&mut calls.waiting)
        };
        for (_, (original, done)) in waiting {
            let _ = done.send(stopped(&original, &self.name));
        }
        let group = self.group.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(mut group) = group {
            // (The group it made, what it started too.)
            std::thread::spawn(move || {
                let _ = group.stop(Duration::from_secs(2), |_| {});
            });
        }
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        self.stop();
    }
}

fn failure(id: &Value, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":message}})
}

/// Stopped (the relay restarting, say): the forwarder sends it again.
fn stopped(id: &Value, name: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":format!("{name} stopped."),
        "data":{"kuvoRelayRestart":true}}})
}

#[cfg(test)]
mod loopback_tests {
    use super::*;

    #[test]
    fn a_proxied_agent_reaches_the_relay_directly() {
        let mut env: Environment = [("HTTPS_PROXY", "http://proxy:3128"), ("NO_PROXY", "corp.example,localhost")]
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        unproxied_loopback(&mut env);
        assert_eq!(env.get(std::ffi::OsStr::new("NO_PROXY")).unwrap(), "corp.example,localhost,127.0.0.1,::1");
        if !cfg!(windows) {
            assert_eq!(env.get(std::ffi::OsStr::new("no_proxy")).unwrap(), "localhost,127.0.0.1,::1");
        }
        let mut plain = Environment::new();
        unproxied_loopback(&mut plain);
        assert!(plain.is_empty());
    }
}
