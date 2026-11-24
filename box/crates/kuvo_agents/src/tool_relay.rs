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

use crate::rpc::RpcError;
use futures_util::{StreamExt, future::join_all};
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full, Limited, StreamBody, combinators::BoxBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use kuvo_os::{
    platform::Environment,
    process::{ChildGroup, Spawn, Stdio},
};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    convert::Infallible,
    ffi::OsString,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{OnceCell, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

/// Where a conversation's Kuvo Code tools are.
#[derive(Clone, Debug)]
pub enum Tools {
    /// The app's server: each call goes there with its headers (the app's
    /// password and the conversation), whatever the agent's said.
    Http {
        url: String,
        headers: Vec<(String, String)>,
        timeout: Duration,
    },
    /// A program run here, outside the sandbox, one per conversation, which
    /// all of its agent's connections share. `key` names how it's set up: a
    /// change starts it again.
    Program {
        key: String,
        command: PathBuf,
        args: Vec<OsString>,
        env: Environment,
        timeout: Duration,
    },
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

#[derive(Clone)]
pub struct ToolRelay(Arc<Inner>);

struct Inner {
    record: Option<PathBuf>,
    source: Arc<dyn ToolSource>,
    state: Mutex<State>,
    listening: OnceCell<(u16, JoinHandle<()>)>,
    stop: CancellationToken,
    client: reqwest::Client,
}

#[derive(Default)]
struct State {
    keys: HashMap<String, String>,
    sessions: HashMap<String, String>,
    holders: HashMap<String, HashSet<Holder>>,
    programs: HashMap<String, Arc<Program>>,
}

#[derive(Default, Deserialize, Serialize)]
struct Record {
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    keys: std::collections::BTreeMap<String, String>,
}

const TOOLS: &str = "Kuvo Code's tools";
const BODY_LIMIT: usize = 32 << 20;

type Body = BoxBody<Bytes, io::Error>;

impl ToolRelay {
    /// A relay keeping its address and keys in `record` (none: kept only
    /// while it runs). It listens once something first holds a key.
    pub fn new(record: Option<PathBuf>, source: Arc<dyn ToolSource>) -> io::Result<Self> {
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
        })))
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
                        if state.keys.contains_key(&session) {
                            continue;
                        }
                        state.sessions.insert(key.clone(), session.clone());
                        state.keys.insert(session, key);
                    }
                }
                let listener = match kept.port {
                    Some(port) => match TcpListener::bind(("127.0.0.1", port)).await {
                        Ok(listener) => listener,
                        Err(_) => {
                            // (Taken meanwhile: agents the resident before
                            // started reach their tools once they start again.)
                            eprintln!("The tools relay's port {port} is taken: it listens on another.");
                            TcpListener::bind(("127.0.0.1", 0)).await?
                        }
                    },
                    None => TcpListener::bind(("127.0.0.1", 0)).await?,
                };
                let port = listener.local_addr()?.port();
                let accepting = tokio::spawn(accept(Arc::downgrade(&self.0), listener, self.0.stop.clone()));
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
        state
            .holders
            .entry(session.to_owned())
            .or_default()
            .insert(holder);
        if let Some(key) = state.keys.get(session) {
            return Ok(key.clone());
        }
        let mut secret = [0; 24];
        getrandom::fill(&mut secret).map_err(io::Error::other)?;
        let key = hex::encode(secret);
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
        let timeout = match tools {
            Tools::Http { timeout, .. } | Tools::Program { timeout, .. } => timeout,
        };
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
            let gone = state
                .keys
                .keys()
                .filter(|session| !sessions.contains(*session) && !state.holders.contains_key(*session))
                .cloned()
                .collect::<Vec<_>>();
            for session in gone {
                if let Some(key) = state.keys.remove(&session) {
                    state.sessions.remove(&key);
                }
            }
        }
        self.keep();
    }

    /// `holder` is done with `session`'s tools: once nothing else holds
    /// them, its key goes and its program stops.
    pub fn drop_relayed(&self, session: &str, holder: Holder) {
        let program = {
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
            state.programs.remove(session)
        };
        if let Some(program) = program {
            program.stop();
        }
        self.keep();
    }

    /// Stop listening, and stop the programs. The record stays for the next.
    pub async fn shutdown(&self) {
        self.0.stop.cancel();
        let programs = std::mem::take(&mut self.state().programs);
        for program in programs.into_values() {
            program.stop();
        }
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

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_record(&self) -> Record {
        self.0
            .record
            .as_ref()
            .and_then(|path| files::read(path).ok().flatten())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn keep(&self) {
        let (Some(path), Some((port, _))) = (&self.0.record, self.0.listening.get()) else {
            return;
        };
        let record = Record {
            port: Some(*port),
            keys: self
                .state()
                .keys
                .iter()
                .map(|(session, key)| (session.clone(), key.clone()))
                .collect(),
        };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| files::write_secret(path, &serde_json::to_vec(&record)?));
        if let Err(error) = written {
            eprintln!("The tools relay's record couldn't be written: {error}");
        }
    }
}

async fn accept(relay: std::sync::Weak<Inner>, listener: TcpListener, stop: CancellationToken) {
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
        if request.uri().path().starts_with("/mcp/") {
            // (The computer's own servers are relayed in a later part.)
            return text(StatusCode::NOT_FOUND, "No such server here.");
        }
        let Some(tools) = self.0.source.tools(&session) else {
            return text(StatusCode::SERVICE_UNAVAILABLE, "Kuvo Code's tools aren't here now.");
        };
        let forwarded = request
            .headers()
            .get("x-kuvo-tool-forwarder")
            .is_some_and(|value| value == "1");
        let (parts, incoming) = request.into_parts();
        let body = if parts.method == Method::POST {
            let read = Limited::new(incoming, BODY_LIMIT).collect().await;
            match read.ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes.to_bytes()).ok()) {
                Some(body) => Some(own_session(body, &session, forwarded)),
                None => {
                    return json_reply(
                        StatusCode::BAD_REQUEST,
                        &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
                        None,
                    );
                }
            }
        } else {
            None
        };
        match tools {
            Tools::Http {
                url,
                headers,
                timeout,
            } => self.forward(&parts, &url, &headers, timeout, body).await,
            Tools::Program {
                key,
                command,
                args,
                env,
                timeout,
            } => {
                let Some(body) = body.filter(|_| parts.method == Method::POST) else {
                    let mut reply = text(StatusCode::METHOD_NOT_ALLOWED, "");
                    reply
                        .headers_mut()
                        .insert(header::ALLOW, HeaderValue::from_static("POST"));
                    return reply;
                };
                let program = {
                    let mut state = self.state();
                    match state.programs.get(&session) {
                        Some(program) if !program.stopped() && program.key == key => program.clone(),
                        _ => {
                            if let Some(old) = state.programs.remove(&session) {
                                old.stop();
                            }
                            match Program::start(key, command, args, env, TOOLS) {
                                Ok(program) => {
                                    state.programs.insert(session.clone(), program.clone());
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
                let messages = match &body {
                    Value::Array(all) => all.clone(),
                    one => vec![one.clone()],
                };
                // Each of its connections (Codex has one a thread) by a name
                // of the relay's, given as it starts: their requests are told
                // apart by it.
                let starts = messages.iter().any(|message| message["method"] == "initialize");
                let connection = if starts {
                    let mut name = [0; 12];
                    let _ = getrandom::fill(&mut name);
                    hex::encode(name)
                } else {
                    parts
                        .headers
                        .get("mcp-session-id")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                };
                let answers = join_all(
                    messages
                        .into_iter()
                        .map(|message| program.send(message, timeout, &connection)),
                )
                .await;
                let said = answers.into_iter().flatten().collect::<Vec<_>>();
                let started = starts.then_some(connection.as_str());
                if said.is_empty() {
                    let mut reply = text(StatusCode::ACCEPTED, "");
                    if let Some(connection) = started.and_then(|name| HeaderValue::from_str(name).ok()) {
                        reply.headers_mut().insert("mcp-session-id", connection);
                    }
                    return reply;
                }
                let answer = if body.is_array() {
                    Value::Array(said)
                } else {
                    said.into_iter().next().unwrap_or(Value::Null)
                };
                json_reply(StatusCode::OK, &answer, started)
            }
        }
    }

    /// A request on to the app's server with its headers; what it answers, back.
    async fn forward(
        &self,
        parts: &http::request::Parts,
        url: &str,
        headers: &[(String, String)],
        timeout: Duration,
        body: Option<Value>,
    ) -> Response<Body> {
        let mut sent = self.0.client.request(parts.method.clone(), url);
        for (name, value) in headers {
            sent = sent.header(name, value);
        }
        let accept = parts
            .headers
            .get(header::ACCEPT)
            .cloned()
            .unwrap_or(HeaderValue::from_static("application/json, text/event-stream"));
        sent = sent.header(header::ACCEPT, accept);
        for name in ["content-type", "mcp-session-id", "mcp-protocol-version", "last-event-id"] {
            if let Some(value) = parts.headers.get(name) {
                sent = sent.header(name, value);
            }
        }
        if let Some(body) = &body {
            sent = sent.body(body.to_string());
        }
        // A call has the tool's time; a stream of what comes, as long as it's read.
        if parts.method == Method::POST {
            sent = sent.timeout(timeout);
        }
        let Ok(answer) = sent.send().await else {
            return text(StatusCode::BAD_GATEWAY, "Kuvo Code's tools didn't answer.");
        };
        let mut reply = Response::builder().status(answer.status().as_u16());
        for name in ["content-type", "mcp-session-id"] {
            if let Some(value) = answer.headers().get(name) {
                reply = reply.header(name, value.as_bytes());
            }
        }
        let stream = answer
            .bytes_stream()
            .map(|chunk| chunk.map(Frame::data).map_err(io::Error::other));
        reply
            .body(BoxBody::new(StreamBody::new(stream)))
            .unwrap_or_else(|_| text(StatusCode::BAD_GATEWAY, "Kuvo Code's tools didn't answer."))
    }
}

/// Each call is the relay's conversation's, whatever it names, with an
/// operation of its own (the forwarder's, sent again after a restart, kept).
fn own_session(body: Value, session: &str, forwarded: bool) -> Value {
    let each = |mut message: Value| {
        if message["method"] != "tools/call" || !message["params"].is_object() {
            return message;
        }
        let params = message["params"].as_object_mut().expect("an object");
        let meta = params
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if !meta.is_object() {
            *meta = Value::Object(Map::new());
        }
        let meta = meta.as_object_mut().expect("an object");
        let operation = meta
            .get("kuvo.operation")
            .and_then(Value::as_str)
            .filter(|operation| forwarded && is_uuid(operation))
            .map(str::to_owned)
            .unwrap_or_else(new_uuid);
        meta.insert("ai.opencode/sessionID".into(), json!(session));
        meta.insert("kuvo.operation".into(), json!(operation));
        message
    };
    match body {
        Value::Array(all) => Value::Array(all.into_iter().map(each).collect()),
        one => each(one),
    }
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

fn new_uuid() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

fn text(status: StatusCode, words: &'static str) -> Response<Body> {
    let mut reply = Response::new(full(Bytes::from_static(words.as_bytes())));
    *reply.status_mut() = status;
    reply
}

fn json_reply(status: StatusCode, body: &Value, connection: Option<&str>) -> Response<Body> {
    let mut reply = Response::new(full(Bytes::from(body.to_string())));
    *reply.status_mut() = status;
    reply
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(connection) = connection.and_then(|name| HeaderValue::from_str(name).ok()) {
        reply.headers_mut().insert("mcp-session-id", connection);
    }
    reply
}

fn full(bytes: Bytes) -> Body {
    BoxBody::new(Full::new(bytes).map_err(|never| match never {}))
}

/// A program spoken to over its input and output, which each of its
/// agent's connections share, their requests numbered anew here so theirs
/// never meet.
struct Program {
    key: String,
    name: &'static str,
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
}

impl Program {
    fn start(
        key: String,
        command: PathBuf,
        args: Vec<OsString>,
        env: Environment,
        name: &'static str,
    ) -> io::Result<Arc<Self>> {
        let mut spawn = Spawn::new(command, env);
        spawn.args = args;
        spawn.stdin = Stdio::Pipe;
        spawn.stdout = Stdio::Pipe;
        // (What it says on stderr isn't kept: a server's own can have its keys in it.)
        spawn.stderr = Stdio::Null;
        let mut group = spawn.spawn()?;
        let mut stdin = group.stdin.take().ok_or_else(|| io::Error::other("no input"))?;
        let stdout = group.stdout.take().ok_or_else(|| io::Error::other("no output"))?;
        let (input, mut writes) = mpsc::unbounded_channel::<String>();
        std::thread::Builder::new()
            .name("relay-program-input".into())
            .spawn(move || {
                while let Some(line) = writes.blocking_recv() {
                    if writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_err() {
                        break;
                    }
                }
            })?;
        let (lines, mut read) = mpsc::unbounded_channel::<String>();
        std::thread::Builder::new()
            .name("relay-program-output".into())
            .spawn(move || {
                for line in io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if lines.send(line).is_err() {
                        break;
                    }
                }
            })?;
        let program = Arc::new(Self {
            key,
            name,
            input,
            calls: Mutex::new(Calls {
                next: 1,
                ..Calls::default()
            }),
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

    fn calls(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stopped(&self) -> bool {
        self.calls().stopped
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
        let original = message.get("id").cloned().filter(|id| !id.is_null());
        let Some(original) = original else {
            // A cancel names the request as the program knows it.
            let request = message["params"].get("requestId").cloned();
            let ours = request.and_then(|request| {
                self.calls()
                    .ours
                    .get(&format!("{connection} {request}"))
                    .copied()
            });
            if message["method"] == "notifications/cancelled" && ours.is_none() {
                return None;
            }
            if let Some(ours) = ours {
                message["params"]["requestId"] = json!(ours);
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
                return Some(stopped(&original, self.name));
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
            Ok(Err(_)) => Some(stopped(&original, self.name)),
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
            let _ = done.send(stopped(&original, self.name));
        }
        let group = self
            .group
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
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
    let error = RpcError::new(-32603, message);
    json!({"jsonrpc":"2.0","id":id,"error":{"code":error.code,"message":error.message}})
}

/// Stopped (the relay restarting, say): the forwarder sends it again.
fn stopped(id: &Value, name: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":format!("{name} stopped."),
        "data":{"kuvoRelayRestart":true}}})
}
