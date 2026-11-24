//! The computer's MCP servers, for every agent, each at `/mcp/<name>`: a
//! command's server started for each of an agent's connections (in the
//! box's sandbox where the host has one), with its keys in its environment;
//! a URL's reached with its headers and sign-in added. The agent is given
//! only the relay's address and its conversation's key.
//!
//! One that can't be reached, or turns down its sign-in, never stops the
//! agent's turn: some agents end their whole session when one server fails
//! to start. The relay connects the agent to a stand-in of its own with no
//! tools whose instructions say why, and the host is asked to connect the
//! server again (renewing the sign-in, or finding it needs one). Once it's
//! set right, its agent is given it at another address and connects afresh.

use super::{
    Answer, Body, Holder, Program, RelayTiming, ToolRelay, connection_of, failure, fresh, full,
    json_reply, parse_error, text,
};
use futures_util::{
    FutureExt, StreamExt,
    future::{BoxFuture, Shared, join_all},
    stream,
};
use http::{HeaderValue, Method, Response, StatusCode, header, request::Parts};
use http_body_util::{StreamBody, combinators::BoxBody};
use hyper::body::{Bytes, Frame};
use kuvo_os::platform::{Environment, Executable, which};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::time::Instant;

/// The connections the relay names for its stand-ins.
const STAND_IN: &str = "unavailable-";

/// One of the computer's servers, as the relay runs or reaches it.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputerServer {
    pub name: String,
    /// How long a call may take.
    pub timeout: Duration,
    pub kind: ServerKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerKind {
    Remote {
        url: String,
        headers: Vec<(String, String)>,
        /// When your sign-in to it runs out, if it's one.
        expires: Option<SystemTime>,
    },
    Local {
        command: Vec<OsString>,
        env: Environment,
        cwd: Option<PathBuf>,
    },
}

/// A conversation's servers, and what tells one set of them from another
/// as agents see them (their names).
#[derive(Clone, Debug, Default)]
pub struct ComputerServers {
    pub servers: Vec<ComputerServer>,
    pub key: String,
}

/// A server as an agent is given it.
#[derive(Clone, Debug, PartialEq)]
pub struct RelayedServer {
    pub name: String,
    pub url: String,
    /// The `Authorization` header: the conversation's key.
    pub authorization: String,
    pub timeout: Duration,
}

/// What a command's server is started as.
pub struct Launch {
    pub command: Executable,
    pub args: Vec<OsString>,
    pub env: Environment,
}

/// Where the computer's servers come from: the engine reads them, renews
/// their sign-ins and connects them again; the sandbox runs their commands.
pub trait ServerHost: Send + Sync + 'static {
    /// The server named `name` for `directory` as the host has it now, its
    /// sign-in renewed if it can be.
    fn renewed<'a>(
        &'a self,
        _directory: &'a Path,
        _name: &'a str,
    ) -> BoxFuture<'a, Option<ComputerServer>> {
        async { None }.boxed()
    }

    /// Connect it again: the host renews your sign-in if it can, or finds it
    /// needs one (and says so where you sign in).
    fn reconnect<'a>(&'a self, _directory: &'a Path, _name: &'a str) -> BoxFuture<'a, ()> {
        async {}.boxed()
    }

    /// How a command's server runs: in the box's sandbox while it's on.
    /// None runs it as it is. An error says why it can't start.
    fn sandboxed(&self, _launch: &Launch, _directory: &Path) -> Result<Option<Launch>, String> {
        Ok(None)
    }
}

pub(super) struct NoHost;
impl ServerHost for NoHost {}

type Asked = Shared<BoxFuture<'static, ()>>;
/// A conversation's server, by name.
type At = (String, String);

#[derive(Default)]
pub(super) struct Servers {
    served: HashMap<String, Served>,
    /// The command servers running, each for one connection: by
    /// conversation, name and connection.
    running: HashMap<(String, String, String), Arc<Program>>,
    stand_ins: HashMap<String, StandIn>,
    /// The host asked to connect each server again, for how it was set up
    /// when it turned down its sign-in: once a setup.
    refusals: HashMap<At, (String, Asked)>,
    /// How many times each conversation's server was set right after the
    /// relay stood in for it.
    set_right: HashMap<String, BTreeMap<String, u32>>,
    brief: HashSet<String>,
    /// Each check underway of a server the relay stood in for, and when the
    /// next is due.
    checks: HashMap<At, Shared<BoxFuture<'static, bool>>>,
    later: HashMap<At, Later>,
}

struct Later {
    setup: String,
    next: Instant,
    wait: Duration,
}

#[derive(Clone)]
struct Served {
    directory: PathBuf,
    servers: HashMap<String, ComputerServer>,
}

struct StandIn {
    session: String,
    name: String,
    setup: String,
    why: String,
    sign_in: bool,
    retired: bool,
}

/// How a server is set up: its address and headers (your sign-in), or its
/// command and environment.
fn setup_of(server: &ComputerServer) -> String {
    match &server.kind {
        ServerKind::Remote { url, headers, .. } => format!("{url:?} {headers:?}"),
        ServerKind::Local { command, env, cwd } => format!("{command:?} {env:?} {cwd:?}"),
    }
}

impl Servers {
    pub(super) fn forget(&mut self, session: &str) -> Vec<Arc<Program>> {
        self.served.remove(session);
        self.stand_ins.retain(|_, stand_in| stand_in.session != session);
        self.refusals.retain(|(at, _), _| at != session);
        self.set_right.remove(session);
        self.brief.remove(session);
        self.checks.retain(|(at, _), _| at != session);
        self.later.retain(|(at, _), _| at != session);
        let stopping = self
            .running
            .keys()
            .filter(|(at, _, _)| at == session)
            .cloned()
            .collect::<Vec<_>>();
        stopping
            .into_iter()
            .filter_map(|at| self.running.remove(&at))
            .collect()
    }

    pub(super) fn stop_all(&mut self) -> Vec<Arc<Program>> {
        self.running.drain().map(|(_, program)| program).collect()
    }

    /// The command servers no connection has used for `idle`, let go of.
    pub(super) fn idle(&mut self, idle: Duration) -> Vec<Arc<Program>> {
        let mut stopping = Vec::new();
        self.running.retain(|_, program| {
            let used = !program.unused_for(idle);
            if !used {
                stopping.push(program.clone());
            }
            used
        });
        stopping
    }

    /// Not there: checked again later, a few minutes on, then each time
    /// twice as long, up to the most (unless it's set up otherwise now).
    fn check_later(&mut self, at: At, setup: String, timing: &RelayTiming) {
        let wait = match self.later.get(&at) {
            Some(before) if before.setup == setup => (before.wait * 2).min(timing.check_at_most),
            _ => timing.check_every,
        };
        self.later.insert(at, Later { setup, next: Instant::now() + wait, wait });
    }

    /// Stand-ins for servers gone now, or (turned down its sign-in) set up
    /// otherwise since, retired.
    fn retire_changed(&mut self, session: &str, found: &ComputerServers) {
        let retiring = self
            .stand_ins
            .values()
            .filter(|stand_in| stand_in.session == session && !stand_in.retired)
            .filter(|stand_in| {
                let now = found.servers.iter().find(|each| each.name == stand_in.name);
                now.is_none_or(|now| stand_in.sign_in && setup_of(now) != stand_in.setup)
            })
            .map(|stand_in| stand_in.name.clone())
            .collect::<HashSet<_>>();
        for name in retiring {
            self.retire(session, &name);
        }
    }

    /// A server set right: its stand-ins are retired, and its agent is
    /// given it at another address.
    fn retire(&mut self, session: &str, name: &str) {
        let mut any = false;
        for stand_in in self.stand_ins.values_mut() {
            if stand_in.session == session && stand_in.name == name && !stand_in.retired {
                stand_in.retired = true;
                any = true;
            }
        }
        if any {
            *self
                .set_right
                .entry(session.to_owned())
                .or_default()
                .entry(name.to_owned())
                .or_default() += 1;
        }
    }

    /// Reached for real: its stand-ins from before are retired, without its
    /// agent being given it afresh.
    fn reached(&mut self, session: &str, name: &str) {
        for stand_in in self.stand_ins.values_mut() {
            if stand_in.session == session && stand_in.name == name {
                stand_in.retired = true;
            }
        }
        self.later.remove(&(session.to_owned(), name.to_owned()));
    }
}

impl ToolRelay {
    /// The computer's servers for `session`, whose commands run in
    /// `directory`, as its agent reaches them through the relay, held by
    /// `holder`. `brief`: its agent gives up its whole session when one
    /// doesn't start, so their starts are brief.
    pub async fn relayed_servers(
        &self,
        session: &str,
        holder: Holder,
        directory: &Path,
        found: &ComputerServers,
        brief: bool,
    ) -> io::Result<Vec<RelayedServer>> {
        {
            let mut state = self.state();
            if brief {
                state.servers.brief.insert(session.to_owned());
            }
            state.servers.retire_changed(session, found);
            if found.servers.is_empty() {
                state.servers.served.remove(session);
                return Ok(Vec::new());
            }
        }
        let key = self.hold(session, holder).await?;
        let port = self.port().await?;
        let mut state = self.state();
        state.servers.served.insert(
            session.to_owned(),
            Served {
                directory: directory.to_owned(),
                servers: found
                    .servers
                    .iter()
                    .map(|each| (each.name.clone(), each.clone()))
                    .collect(),
            },
        );
        let times = state.servers.set_right.get(session);
        Ok(found
            .servers
            .iter()
            .map(|each| {
                let again = times
                    .and_then(|times| times.get(&each.name))
                    .map_or(String::new(), |times| format!("?again={times}"));
                RelayedServer {
                    name: each.name.clone(),
                    url: format!(
                        "http://127.0.0.1:{port}/mcp/{}{again}",
                        percent_encoding::utf8_percent_encode(&each.name, percent_encoding::NON_ALPHANUMERIC)
                    ),
                    authorization: format!("Bearer {key}"),
                    timeout: each.timeout,
                }
            })
            .collect())
    }

    /// What tells the servers an agent is given now from those it was
    /// given: one the relay stood in for that's set right since counts as
    /// another, so its agent connects to it afresh. One whose sign-in was
    /// turned down is set right by another; one that couldn't be reached,
    /// or start, once it answers, which is checked now (briefly: one that
    /// doesn't answer at once is checked on, for the next message).
    pub async fn servers_key(&self, session: &str, found: &ComputerServers) -> String {
        let (waiting, check) = {
            let mut state = self.state();
            let timing = state.timing;
            let directory = state.servers.served.get(session).map(|served| served.directory.clone());
            let servers = &mut state.servers;
            servers.retire_changed(session, found);
            let names = servers
                .stand_ins
                .values()
                .filter(|stand_in| stand_in.session == session && !stand_in.retired && !stand_in.sign_in)
                .map(|stand_in| stand_in.name.clone())
                .collect::<HashSet<_>>();
            let mut waiting = Vec::new();
            for now in found.servers.iter().filter(|each| names.contains(&each.name)) {
                let at = (session.to_owned(), now.name.clone());
                if let Some(check) = servers.checks.get(&at) {
                    waiting.push(check.clone());
                    continue;
                }
                // (Not again so soon, unless it's set up otherwise now: your
                // change may be what it needed.)
                let setup = setup_of(now);
                let later = servers.later.get(&at);
                if later.is_some_and(|later| later.setup == setup && Instant::now() < later.next) {
                    continue;
                }
                let (relay, now, directory, key) = (self.clone(), now.clone(), directory.clone(), at.clone());
                let check = async move {
                    let started = relay.answers(&now, &key.0, directory.as_deref()).await;
                    let mut state = relay.state();
                    state.servers.checks.remove(&key);
                    if started {
                        state.servers.later.remove(&key);
                        state.servers.retire(&key.0, &key.1);
                    } else {
                        state.servers.check_later(key, setup, &timing);
                    }
                    started
                }
                .boxed()
                .shared();
                tokio::spawn(check.clone());
                servers.checks.insert(at, check.clone());
                waiting.push(check);
            }
            (waiting, timing.check)
        };
        if !waiting.is_empty() {
            let _ = tokio::time::timeout(check, join_all(waiting)).await;
        }
        let state = self.state();
        match state.servers.set_right.get(session).filter(|times| !times.is_empty()) {
            Some(times) => format!("{} {}", found.key, json!(times.iter().collect::<Vec<_>>())),
            None => found.key.clone(),
        }
    }

    /// Whether a server that wasn't there starts now, as an agent's start
    /// would have it: connected to, and let go.
    async fn answers(&self, entry: &ComputerServer, session: &str, directory: Option<&Path>) -> bool {
        let timing = self.timing();
        let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"kuvo-code","version":"1"}}});
        let ServerKind::Remote { url, headers, .. } = &entry.kind else {
            let Some(Ok(program)) = directory.map(|directory| self.start_server(entry, session, directory))
            else {
                return false;
            };
            let answer = program.send(initialize, entry.timeout.min(timing.command_starts), "check").await;
            program.stop();
            return answer.is_some_and(|answer| answer["result"].is_object() && answer.get("error").is_none());
        };
        let deadline = Instant::now() + entry.timeout.min(timing.starts);
        let mut request = self.0.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let request = request
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(initialize.to_string());
        let Ok(Ok(response)) = tokio::time::timeout_at(deadline, request.send()).await else {
            return false;
        };
        let (status, at) = (response.status(), response.url().clone());
        let connection = response.headers().get("mcp-session-id").cloned();
        let started = status.is_success()
            && matches!(tokio::time::timeout_at(deadline, read_start(Some(response), &json!(1))).await, Ok(Ok(_)));
        if let Some(connection) = connection {
            let mut left = self.0.client.delete(at).header("mcp-session-id", connection);
            for (key, value) in headers {
                left = left.header(key, value);
            }
            tokio::spawn(left.timeout(timing.starts).send());
        }
        started
    }

    pub(super) async fn answer_server(
        &self,
        parts: Parts,
        body: Option<Bytes>,
        session: &str,
        name: &str,
    ) -> Response<Body> {
        let (served, entry) = {
            let state = self.state();
            let served = state.servers.served.get(session).cloned();
            let entry = served.as_ref().and_then(|served| served.servers.get(name).cloned());
            match (served, entry) {
                (Some(served), Some(entry)) => (served, entry),
                _ => return text(StatusCode::NOT_FOUND, "No such server here."),
            }
        };
        let connection = connection_of(&parts);
        if connection.starts_with(STAND_IN) {
            // One of the relay's own, never passed on to the server, which
            // never gave it.
            let why = {
                let state = self.state();
                match state.servers.stand_ins.get(&connection) {
                    Some(stand_in) if stand_in.session != session || stand_in.name != name => {
                        return text(StatusCode::NOT_FOUND, "No such connection here.");
                    }
                    Some(stand_in) => stand_in.why.clone(),
                    None => format!("The {name} server isn't connected in this conversation now"),
                }
            };
            return self.answer_stand_in(&parts, body, &connection, &why);
        }
        let sent = body.as_ref().and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
        match entry.kind {
            ServerKind::Remote { .. } => {
                self.answer_remote(parts, body, sent, session, name, served, entry).await
            }
            ServerKind::Local { .. } => {
                self.answer_local(parts, sent, session, name, &served.directory, &entry, &connection)
                    .await
            }
        }
    }

    fn answer_stand_in(
        &self,
        parts: &Parts,
        body: Option<Bytes>,
        connection: &str,
        why: &str,
    ) -> Response<Body> {
        if parts.method == Method::DELETE {
            self.state().servers.stand_ins.remove(connection);
            return text(StatusCode::OK, "");
        }
        if parts.method != Method::POST {
            return not_allowed();
        }
        let Some(body) = body.and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) else {
            return parse_error();
        };
        stand_in_answers(&body, why, None, false)
    }

    #[allow(clippy::too_many_arguments)]
    async fn answer_remote(
        &self,
        parts: Parts,
        body: Option<Bytes>,
        sent: Option<Value>,
        session: &str,
        name: &str,
        served: Served,
        entry: ComputerServer,
    ) -> Response<Body> {
        let messages = messages(sent.as_ref());
        let initialize = messages.iter().find(|message| message["method"] == "initialize").cloned();
        let brief = initialize.is_some() && self.state().servers.brief.contains(session);
        let start_in = if brief { entry.timeout.min(self.timing().starts) } else { entry.timeout };
        let deadline = Instant::now() + start_in;
        let host = self.0.host.clone();
        let renewing = || {
            let host = host.clone();
            let directory = served.directory.clone();
            let name = name.to_owned();
            async move {
                let renewed = if brief {
                    tokio::time::timeout_at(deadline, host.renewed(&directory, &name)).await.ok().flatten()
                } else {
                    host.renewed(&directory, &name).await
                };
                renewed.filter(|server| matches!(server.kind, ServerKind::Remote { .. }))
            }
        };
        // Your sign-in to it running out (in a long turn): renewed first.
        let mut current = entry.clone();
        if let ServerKind::Remote { expires: Some(expires), .. } = &entry.kind
            && *expires < SystemTime::now() + Duration::from_secs(60)
            && let Some(renewed) = renewing().await
        {
            current = renewed;
        }
        let limit = |initialize: bool| if initialize { None } else { Some(entry.timeout) };
        let mut answer = self.send_remote(&parts, &current, body.clone(), limit(initialize.is_some()), deadline).await;
        let at = (session.to_owned(), name.to_owned());
        if answer.status == 401 && matches!(current.kind, ServerKind::Remote { expires: Some(_), .. })
            && let Some(again) = renewing().await.filter(|again| setup_of(again) != setup_of(&current))
        {
            current = again;
            answer = self.send_remote(&parts, &current, body.clone(), limit(initialize.is_some()), deadline).await;
        }
        if answer.status == 401 {
            // The host connects it again, once for how it's set up now.
            let setup = setup_of(&current);
            let asked = {
                let mut state = self.state();
                match state.servers.refusals.get(&at) {
                    Some((known, asked)) if *known == setup => asked.clone(),
                    _ => {
                        let host = host.clone();
                        let directory = served.directory.clone();
                        let named = name.to_owned();
                        let asked = async move { host.reconnect(&directory, &named).await }.boxed().shared();
                        tokio::spawn(asked.clone());
                        state.servers.refusals.insert(at.clone(), (setup, asked.clone()));
                        asked
                    }
                }
            };
            if initialize.is_none() {
                asked.clone().await;
                if let Some(again) = renewing().await.filter(|again| setup_of(again) != setup_of(&current)) {
                    current = again;
                    answer = self.send_remote(&parts, &current, body.clone(), limit(false), deadline).await;
                }
                if answer.status == 401 {
                    self.state().servers.refusals.insert(at, (setup_of(&current), asked));
                }
            }
        }
        if let Some(initialize) = initialize {
            // Read before the agent has it: what isn't a start is a stand-in's.
            let started = if (200..300).contains(&answer.status) {
                match tokio::time::timeout_at(deadline, read_start(answer.response.take(), &initialize["id"])).await {
                    Ok(Ok(start)) => Ok(start),
                    Ok(Err(failed)) => Err(why_not(name, answer.status, Some(&failed))),
                    Err(_) => Err(why_not(name, 502, None)),
                }
            } else {
                Err(why_not(name, answer.status, None))
            };
            return match started {
                Ok(body) => {
                    self.state().servers.reached(session, name);
                    answer.reply(body)
                }
                Err(why) => self.stand_in_for(session, name, &setup_of(&current), &why, sent.as_ref(), answer.status == 401),
            };
        }
        // A call it turns down partway: its answer says why, as a tool's error.
        if !messages.is_empty() && failing(answer.status) {
            let sent = sent.unwrap_or(Value::Null);
            return stand_in_answers(&sent, &why_not(name, answer.status, None), None, true);
        }
        if parts.method == Method::GET && failing(answer.status) {
            return not_allowed();
        }
        answer.into_reply("The server didn't answer.")
    }

    /// To a URL's server as it's set up: a start is given up at `deadline`.
    async fn send_remote(
        &self,
        parts: &Parts,
        server: &ComputerServer,
        body: Option<Bytes>,
        timeout: Option<Duration>,
        deadline: Instant,
    ) -> Answer {
        let ServerKind::Remote { url, headers, .. } = &server.kind else {
            return Answer::failed();
        };
        let deadline = timeout.is_none().then_some(deadline);
        self.send_on(parts, url, headers, body, timeout, deadline).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn answer_local(
        &self,
        parts: Parts,
        sent: Option<Value>,
        session: &str,
        name: &str,
        directory: &Path,
        entry: &ComputerServer,
        connection: &str,
    ) -> Response<Body> {
        let at = (session.to_owned(), name.to_owned(), connection.to_owned());
        if parts.method == Method::DELETE {
            if let Some(program) = self.state().servers.running.remove(&at) {
                program.stop();
            }
            return text(StatusCode::OK, "");
        }
        if parts.method != Method::POST {
            return not_allowed();
        }
        let Some(body) = sent else { return parse_error() };
        let all = messages(Some(&body));
        let Some(initialize) = all.iter().find(|message| message["method"] == "initialize").cloned() else {
            let program = self.state().servers.running.get(&at).cloned();
            // (One that stopped, or a connection it doesn't know: the agent connects again.)
            let Some(program) = program.filter(|program| !program.stopped()) else {
                return text(StatusCode::NOT_FOUND, "No such connection here.");
            };
            let answers =
                join_all(all.into_iter().map(|message| program.send(message, entry.timeout, connection))).await;
            return replies(&body, answers, None);
        };
        // Each connection a server of its own, as an agent that starts it
        // itself has it.
        let program = match self.start_server(entry, session, directory) {
            Ok(program) => program,
            Err(why) => return self.stand_in_for(session, name, &setup_of(entry), &why, Some(&body), false),
        };
        let fresh = fresh(12);
        let at = (session.to_owned(), name.to_owned(), fresh.clone());
        self.state().servers.running.insert(at.clone(), program.clone());
        let brief = self.state().servers.brief.contains(session);
        let start_in = if brief { entry.timeout.min(self.timing().command_starts) } else { entry.timeout };
        let answers = join_all(all.into_iter().map(|message| {
            let time = if message == initialize { start_in } else { entry.timeout };
            program.send(message, time, &fresh)
        }))
        .await;
        let started = answers
            .iter()
            .flatten()
            .find(|answer| answer["id"] == initialize["id"])
            .cloned();
        // (One that ends, or says no, before it's started: the agent goes on
        // without it.)
        match started {
            Some(answer) if answer.get("error").is_none() => {
                self.state().servers.reached(session, name);
                replies(&body, answers, Some(&fresh))
            }
            other => {
                program.stop();
                self.state().servers.running.remove(&at);
                let said = other
                    .as_ref()
                    .and_then(|answer| answer["error"]["message"].as_str())
                    .map(|message| format!(": {message}"))
                    .unwrap_or_default();
                let why = format!("The {name} server couldn't start{said}");
                self.stand_in_for(session, name, &setup_of(entry), &why, Some(&body), false)
            }
        }
    }

    /// A command's server for one of a conversation's connections, or why
    /// it can't start.
    fn start_server(&self, entry: &ComputerServer, session: &str, directory: &Path) -> Result<Arc<Program>, String> {
        let ServerKind::Local { command, env, cwd } = &entry.kind else {
            return Err(format!("The {} server can't start.", entry.name));
        };
        let Some((named, args)) = command.split_first() else {
            return Err(format!("The {} server has no command.", entry.name));
        };
        let mut environment: Environment = std::env::vars_os().collect();
        environment.extend(env.clone());
        environment.insert("OPENCODE_SESSION_ID".into(), session.into());
        // By its name on the PATH, as a shell finds it.
        let program = which(named, &environment, &[]).ok_or_else(|| {
            format!("The {} server can't start: {} isn't installed here.", entry.name, named.to_string_lossy())
        })?;
        let launch = Launch { command: program, args: args.to_vec(), env: environment };
        let launch = self.0.host.sandboxed(&launch, directory)?.unwrap_or(launch);
        let cwd = cwd.clone().unwrap_or_else(|| directory.to_owned());
        Program::start(String::new(), launch, Some(cwd), format!("The {} server", entry.name))
            .map_err(|error| format!("The {} server couldn't start: {error}", entry.name))
    }

    /// A server that couldn't be reached as an agent connected to it: a
    /// stand-in of the relay's own, with no tools, whose instructions say why.
    fn stand_in_for(&self, session: &str, name: &str, setup: &str, why: &str, sent: Option<&Value>, sign_in: bool) -> Response<Body> {
        let connection = format!("{STAND_IN}{}", fresh(12));
        let (session, name, setup) = (session.to_owned(), name.to_owned(), setup.to_owned());
        let mut state = self.state();
        let timing = state.timing;
        // (This start was a check of its own: the next, a few minutes on.)
        let at = (session.clone(), name.clone());
        if !sign_in && !state.servers.later.contains_key(&at) {
            state.servers.check_later(at, setup.clone(), &timing);
        }
        let why_now = why.to_owned();
        let stand_in = StandIn { session, name, setup, why: why_now, sign_in, retired: false };
        state.servers.stand_ins.insert(connection.clone(), stand_in);
        drop(state);
        stand_in_answers(sent.unwrap_or(&Value::Null), why, Some(&connection), false)
    }
}

/// Read a server's answer to `initialize` until it says whether it started:
/// why it didn't, or the answer to hand on (a stream goes on as it comes).
async fn read_start(response: Option<reqwest::Response>, id: &Value) -> Result<Body, String> {
    let Some(response) = response else { return Err("no answer".into()) };
    let stream_of_events = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|kind| kind.contains("text/event-stream"));
    let not_one = "its answer isn't an MCP server's";
    if !stream_of_events {
        let text = response.text().await.map_err(|_| "no answer".to_owned())?;
        return match start_of(&text, false, id) {
            Some(Ok(())) => Ok(full(Bytes::from(text))),
            Some(Err(failed)) => Err(failed),
            None => Err(not_one.into()),
        };
    }
    let mut chunks = response.bytes_stream();
    let mut read = Vec::<Bytes>::new();
    let mut text = String::new();
    loop {
        let next = chunks.next().await;
        let done = match next {
            Some(Ok(chunk)) => {
                text.push_str(&String::from_utf8_lossy(&chunk));
                read.push(chunk);
                false
            }
            Some(Err(_)) => return Err("no answer".into()),
            None => true,
        };
        match start_of(&text, true, id) {
            Some(Err(failed)) => return Err(failed),
            None if done => return Err(not_one.into()),
            None => continue,
            Some(Ok(())) => {
                let rest = chunks.map(|chunk| chunk.map(Frame::data).map_err(io::Error::other));
                let kept = stream::iter(read.into_iter().map(|chunk| Ok(Frame::data(chunk))));
                return Ok(BoxBody::new(StreamBody::new(kept.chain(rest))));
            }
        }
    }
}

/// A server's answer to `initialize` in what it's sent so far: why it isn't
/// a start (its words, cut short), a start, or nothing yet.
fn start_of(text: &str, events: bool, id: &Value) -> Option<Result<(), String>> {
    let mut messages = Vec::new();
    let mut add = |data: &str| {
        if let Ok(parsed) = serde_json::from_str::<Value>(data) {
            match parsed {
                Value::Array(all) => messages.extend(all),
                one => messages.push(one),
            }
        }
    };
    if events {
        let normalized = text.replace("\r\n", "\n");
        for event in normalized.split("\n\n") {
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            if !data.is_empty() {
                add(&data);
            }
        }
    } else {
        add(text);
    }
    let answer = messages.into_iter().find(|message| {
        &message["id"] == id && (message.get("result").is_some() || message.get("error").is_some())
    })?;
    if answer.get("error").is_some() || !answer["result"].is_object() {
        let said = answer["error"]["message"].as_str().map(str::trim).unwrap_or("it said no");
        let said = if said.chars().count() > 200 {
            format!("{}…", said.chars().take(200).collect::<String>())
        } else {
            said.to_owned()
        };
        return Some(Err(said));
    }
    Some(Ok(()))
}

/// Turned down: its sign-in or key refused, or what was asked not allowed.
fn refused(status: u16) -> bool {
    status == 401 || status == 403
}

/// Not the server's answer: turned down, sent elsewhere, or failing.
fn failing(status: u16) -> bool {
    refused(status) || (300..400).contains(&status) || status >= 500
}

/// Why a server's tools aren't there, or a call to it failed, as the agent
/// is told it.
fn why_not(name: &str, status: u16, started: Option<&str>) -> String {
    if status == 401 {
        return format!("The {name} server turned down its sign-in, so its tools aren't available for now. If the user asks for something from it, tell them to sign in to it again in Kuvo Code (Marketplace › Apps, or its key in Settings › MCP Servers), and carry on with the rest");
    }
    if status == 403 {
        return format!("The {name} server turned this down (HTTP 403): its sign-in or key may not be allowed to do it");
    }
    if let Some(started) = started {
        return format!("The {name} server couldn't start ({started}), so its tools aren't available for now");
    }
    let what = if status == 502 {
        "no answer".to_owned()
    } else if (300..400).contains(&status) {
        format!("HTTP {status}: it sent elsewhere, perhaps to sign in")
    } else {
        format!("HTTP {status}")
    };
    format!("The {name} server couldn't be reached ({what}), so its tools aren't available for now")
}

pub(super) fn messages(sent: Option<&Value>) -> Vec<Value> {
    match sent {
        Some(Value::Array(all)) => all.clone(),
        Some(one) => vec![one.clone()],
        None => Vec::new(),
    }
}

/// What a server that isn't there answers: it starts, it has no tools, and
/// anything asked of it says why. Or (`failing`) what one connected for real
/// that fails partway answers.
fn stand_in_answers(sent: &Value, reason: &str, connection: Option<&str>, failing: bool) -> Response<Body> {
    let why = format!("{}.", reason.trim_end_matches(|c: char| c == '.' || c.is_whitespace()));
    let answers = messages(Some(sent))
        .into_iter()
        .map(|message| {
            let id = message.get("id").filter(|id| !id.is_null())?.clone();
            let result = |value: Value| json!({"jsonrpc":"2.0","id":id,"result":value});
            let method = message["method"].as_str().unwrap_or_default();
            if failing && method != "tools/call" {
                return Some(failure(&id, &why));
            }
            Some(match method {
                "initialize" => result(json!({
                    "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "unavailable", "version": "1"},
                    "instructions": why,
                })),
                "ping" => result(json!({})),
                "tools/list" => result(json!({"tools": []})),
                "resources/list" => result(json!({"resources": []})),
                "resources/templates/list" => result(json!({"resourceTemplates": []})),
                "prompts/list" => result(json!({"prompts": []})),
                "tools/call" => result(json!({"content":[{"type":"text","text":why}],"isError":true})),
                _ => failure(&id, &why),
            })
        })
        .collect();
    replies(sent, answers, connection)
}

/// The answers to a message (or several), as MCP's HTTP sends them: none
/// (notifications) is a 202.
pub(super) fn replies(body: &Value, answers: Vec<Option<Value>>, connection: Option<&str>) -> Response<Body> {
    let said = answers.into_iter().flatten().collect::<Vec<_>>();
    if said.is_empty() {
        let mut reply = text(StatusCode::ACCEPTED, "");
        if let Some(connection) = connection.and_then(|name| HeaderValue::from_str(name).ok()) {
            reply.headers_mut().insert("mcp-session-id", connection);
        }
        return reply;
    }
    let answer = if body.is_array() {
        Value::Array(said)
    } else {
        said.into_iter().next().unwrap_or(Value::Null)
    };
    json_reply(StatusCode::OK, &answer, connection)
}

fn not_allowed() -> Response<Body> {
    let mut reply = text(StatusCode::METHOD_NOT_ALLOWED, "");
    reply.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST, DELETE"));
    reply
}

