//! This client's sessions with boxes, over one relay connection (PROTOCOL.md, "Sessions",
//! "Messages in a session"): one Noise session per box, its requests and followed routes told
//! apart by id, as the phone's `LinkClient` does it.
//!
//! Nothing runs on a timer here. The relay connection is made when something is asked and it's
//! down (never more often than its backoff allows), a session when its box is asked something, and
//! a request with no answer is checked on (a ping) by the thread that waits for it. A box that
//! goes away fails what waits on it; what follows a route opens it again by its own backoff
//! ([`crate::follow`]).

use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use super::frames::{self, FromRelay, Kind};
use super::keys::{Address, Host, Identity};
use super::noise::{self, Initiator};
use super::relay::{Dial, Incoming, Outgoing};
use crate::error::{Error, Refusal, Result};
use crate::transport::Request;

/// How long the link waits, as the phone's does (KuvoLink's `Link`).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// For the relay's welcome, once connected.
    pub welcome: Duration,
    /// For a box's answer to a handshake.
    pub handshake: Duration,
    /// How long a request waits with nothing from its box before the box is checked on.
    pub answer_within: Duration,
    /// How long a check may take: a computer under a heavy load answers late, and is still there.
    pub check: Duration,
    /// After the relay couldn't be reached: not tried again before this, doubling up to `retry_most`.
    pub retry_first: Duration,
    pub retry_most: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            welcome: Duration::from_secs(15),
            handshake: Duration::from_secs(15),
            answer_within: Duration::from_secs(5),
            check: Duration::from_secs(15),
            retry_first: Duration::from_millis(500),
            retry_most: Duration::from_secs(30),
        }
    }
}

/// What a box says of itself as a session opens.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BoxHello {
    pub v: u64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub app: String,
}

/// Why a box couldn't be asked, in words a client shows.
pub mod words {
    pub const RELAY: &str = "Couldn’t connect. Check that you’re online, then try again.";
    pub const OFFLINE: &str = "It may be asleep or offline.";
    pub const TIMED_OUT: &str = "It didn’t answer.";
    pub const CLOSED: &str = "The connection closed.";
    pub const REFUSED: &str = "It turned this app away.";
    pub const VERSION: &str = "It runs a version this app doesn’t know. Update both.";
    /// A refusal's `reason` when the box doesn't take this device (never paired, or removed).
    pub const UNPAIRED: &str = "unpaired";
}

/// The box turned this device away: it isn't paired there (any more).
pub fn unpaired() -> Error {
    Error::Refused(Refusal { status: 403, message: words::REFUSED.into(), reason: Some(words::UNPAIRED.into()), older_box: false })
}

/// Whether `error` is a box turning this device away.
pub fn is_unpaired(error: &Error) -> bool {
    error.refusal().is_some_and(|refusal| refusal.reason.as_deref() == Some(words::UNPAIRED))
}

/// A piece of an answer, as it comes.
enum Piece {
    Head { status: u16, content_type: Option<String> },
    Body(Vec<u8>),
    End,
    Failed(Error),
}

enum Phase {
    Shaking(Option<Initiator>),
    Open(noise::Transport, BoxHello),
}

struct Session {
    /// Which of this client's sessions it is, counted: what waits on it tells it from a later one.
    token: u64,
    number: u32,
    key: [u8; 32],
    pair: Option<String>,
    phase: Phase,
    next: u64,
    answers: HashMap<u64, Sender<Piece>>,
    heard: Instant,
    checking: Option<Instant>,
}

enum Connection {
    Down,
    Dialing,
    Welcoming { id: u64, out: Box<dyn Outgoing> },
    Up { id: u64, out: Box<dyn Outgoing> },
}

struct State {
    connection: Connection,
    connections: u64,
    /// When the relay last couldn't be reached, and how many times in a row.
    failed: Option<(Instant, u32)>,
    sessions: HashMap<Address, Session>,
    tokens: u64,
    /// How many sessions with each box have ended: after one, the box may be another version's.
    over: HashMap<Address, u64>,
    /// Why each box's last session ended, by its token, for what waited on its handshake.
    ended: HashMap<Address, (u64, Error)>,
}

struct Inner {
    relay: String,
    identity: Identity,
    secret: [u8; 32],
    address: Address,
    name: String,
    app: String,
    dial: Arc<dyn Dial>,
    timing: Timing,
    state: Mutex<State>,
    changed: Condvar,
}

/// This device on one relay. Cheap to clone: clones share the connection.
#[derive(Clone)]
pub struct LinkClient {
    inner: Arc<Inner>,
}

fn random_number() -> u32 {
    let mut bytes = [0u8; 4];
    loop {
        getrandom::fill(&mut bytes).expect("the system gives random bytes");
        let number = u32::from_be_bytes(bytes);
        if number != 0 {
            return number;
        }
    }
}

impl LinkClient {
    /// `name`: what this device is called in the list of paired devices; `app`: its version.
    pub fn new(relay: impl Into<String>, identity: Identity, name: impl Into<String>, app: impl Into<String>, dial: Arc<dyn Dial>, timing: Timing) -> LinkClient {
        let relay = relay.into();
        let secret = identity.secret_on(&relay);
        LinkClient {
            inner: Arc::new(Inner {
                address: super::keys::address_of(&secret),
                secret,
                relay,
                identity,
                name: name.into(),
                app: app.into(),
                dial,
                timing,
                state: Mutex::new(State { connection: Connection::Down, connections: 0, failed: None, sessions: HashMap::new(), tokens: 0, over: HashMap::new(), ended: HashMap::new() }),
                changed: Condvar::new(),
            }),
        }
    }

    pub fn relay(&self) -> &str {
        &self.inner.relay
    }

    /// Where this device is on its relay.
    pub fn address(&self) -> Address {
        self.inner.address
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many sessions with `host` have ended so far: once one has, the box there may be
    /// another version's (it started again), and is read anew before it's asked anything else.
    pub fn sessions_over(&self, host: &Host) -> u64 {
        self.state().over.get(&host.address).copied().unwrap_or(0)
    }

    /// Whether a session with `host` is open now.
    pub fn is_open(&self, host: &Host) -> bool {
        self.state().sessions.get(&host.address).is_some_and(|session| matches!(session.phase, Phase::Open(..)))
    }

    /// What `host` said of itself, opening a session if there's none.
    pub fn hello(&self, host: &Host) -> Result<BoxHello> {
        let state = self.session(self.state(), host, None)?;
        match state.sessions.get(&host.address).map(|session| &session.phase) {
            Some(Phase::Open(_, hello)) => Ok(hello.clone()),
            _ => Err(Error::Unreachable(words::CLOSED.into())),
        }
    }

    /// Pairs with `host` by a pairing link's `token` (PROTOCOL.md, "Pairing"): a session opened
    /// showing it. A session there already is ended first.
    pub fn pair(&self, host: &Host, token: &str) -> Result<BoxHello> {
        let mut state = self.state();
        self.end(&mut state, &host.address, Error::Unreachable(words::CLOSED.into()), true);
        let state = self.session(state, host, Some(token.to_owned()))?;
        match state.sessions.get(&host.address).map(|session| &session.phase) {
            Some(Phase::Open(_, hello)) => Ok(hello.clone()),
            _ => Err(Error::Unreachable(words::CLOSED.into())),
        }
    }

    /// Ends the session with `host`, telling it: this client no longer talks to that box.
    pub fn close(&self, host: &Host) {
        let mut state = self.state();
        self.end(&mut state, &host.address, Error::Unreachable(words::CLOSED.into()), true);
    }

    /// Lets the relay connection go, with every session on it (the computer woke, the network
    /// changed: it may have died without closing). The next thing asked connects again.
    pub fn let_go(&self) {
        let mut state = self.state();
        self.drop_connection(&mut state, None);
    }

    // MARK: The relay connection

    /// With the relay connection up, making it if it's down.
    fn connected<'a>(&'a self, mut state: MutexGuard<'a, State>) -> Result<MutexGuard<'a, State>> {
        let inner = &self.inner;
        let unreachable = || Error::Unreachable(words::RELAY.into());
        loop {
            match &state.connection {
                Connection::Up { .. } => return Ok(state),
                Connection::Dialing | Connection::Welcoming { .. } => {
                    let (next, timed_out) = inner.changed.wait_timeout(state, inner.timing.welcome * 2).unwrap_or_else(|poisoned| poisoned.into_inner());
                    state = next;
                    if timed_out.timed_out() && !matches!(state.connection, Connection::Up { .. }) {
                        return Err(unreachable());
                    }
                }
                Connection::Down => {
                    if let Some((at, tries)) = state.failed {
                        let wait = inner.timing.retry_first.saturating_mul(1 << tries.saturating_sub(1).min(8)).min(inner.timing.retry_most);
                        if at.elapsed() < wait {
                            return Err(unreachable());
                        }
                    }
                    state.connection = Connection::Dialing;
                    drop(state);
                    let dialed = inner.dial.dial(&inner.relay);
                    state = self.state();
                    let fail = |state: &mut State| {
                        state.connection = Connection::Down;
                        state.failed = Some((Instant::now(), state.failed.map_or(1, |(_, tries)| tries + 1)));
                        inner.changed.notify_all();
                    };
                    let Ok((mut out, incoming)) = dialed else {
                        fail(&mut state);
                        return Err(unreachable());
                    };
                    if out.send(frames::relay_hello(&inner.secret)).is_err() {
                        fail(&mut state);
                        return Err(unreachable());
                    }
                    state.connections += 1;
                    let id = state.connections;
                    state.connection = Connection::Welcoming { id, out };
                    self.read(id, incoming);
                    let deadline = Instant::now() + inner.timing.welcome;
                    loop {
                        match &state.connection {
                            Connection::Up { id: up, .. } if *up == id => break,
                            Connection::Welcoming { id: waiting, .. } if *waiting == id => {
                                let left = deadline.saturating_duration_since(Instant::now());
                                if left.is_zero() {
                                    fail(&mut state);
                                    return Err(unreachable());
                                }
                                state = inner.changed.wait_timeout(state, left).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
                            }
                            // It ended before its welcome (the reader noted the failure).
                            _ => return Err(unreachable()),
                        }
                    }
                }
            }
        }
    }

    /// Reads connection `id` on a thread of its own until it ends.
    fn read(&self, id: u64, mut incoming: Box<dyn Incoming>) {
        let client = self.clone();
        let spawned = std::thread::Builder::new().name("kuvo-link".into()).spawn(move || {
            while let Ok(bytes) = incoming.next() {
                client.received(id, &bytes);
            }
            let mut state = client.state();
            client.drop_connection(&mut state, Some(id));
        });
        if spawned.is_err() {
            let mut state = self.state();
            self.drop_connection(&mut state, Some(id));
        }
    }

    /// The connection (`id`, or whichever it is) is over: every session on it with it.
    fn drop_connection(&self, state: &mut State, id: Option<u64>) {
        let current = match &state.connection {
            Connection::Welcoming { id, .. } | Connection::Up { id, .. } => Some(*id),
            _ => None,
        };
        if current.is_none() || id.is_some_and(|id| Some(id) != current) {
            return;
        }
        // One that never got its welcome counts against the next try; one that held doesn't.
        if matches!(state.connection, Connection::Welcoming { .. }) {
            state.failed = Some((Instant::now(), state.failed.map_or(1, |(_, tries)| tries + 1)));
        }
        state.connection = Connection::Down;
        let addresses: Vec<Address> = state.sessions.keys().copied().collect();
        for address in addresses {
            self.end(state, &address, Error::Unreachable(words::RELAY.into()), false);
        }
        self.inner.changed.notify_all();
    }

    /// Sends `payload` to `to` on the relay; a connection that won't take it is over.
    fn relay_send(&self, state: &mut State, to: &Address, payload: &[u8]) -> bool {
        let sent = match &mut state.connection {
            Connection::Up { out, .. } => out.send(frames::relay_send(to, payload)).is_ok(),
            _ => false,
        };
        if !sent {
            self.drop_connection(state, None);
        }
        sent
    }

    // MARK: Sessions

    /// With a session with `host` open, opening one if there's none.
    fn session<'a>(&'a self, mut state: MutexGuard<'a, State>, host: &Host, pair: Option<String>) -> Result<MutexGuard<'a, State>> {
        let inner = &self.inner;
        let mut mine: Option<(u64, Instant)> = None;
        loop {
            state = self.connected(state)?;
            let found = state.sessions.get(&host.address).map(|session| (session.token, session.key == host.key, matches!(session.phase, Phase::Open(..))));
            match (found, mine) {
                // Another box's key at that address: never that session.
                (Some((_, false, _)), _) => self.end(&mut state, &host.address, Error::Unreachable(words::CLOSED.into()), true),
                (Some((_, true, true)), _) => return Ok(state),
                (Some((token, true, false)), _) => {
                    let since = match mine {
                        Some((waiting, since)) if waiting == token => since,
                        _ => Instant::now(),
                    };
                    mine = Some((token, since));
                    let left = inner.timing.handshake.saturating_sub(since.elapsed());
                    if left.is_zero() {
                        self.end(&mut state, &host.address, Error::Unreachable(words::TIMED_OUT.into()), false);
                        return Err(Error::Unreachable(words::TIMED_OUT.into()));
                    }
                    state = inner.changed.wait_timeout(state, left).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
                }
                (None, Some((token, _))) => {
                    // The one waited on ended: why.
                    return Err(match state.ended.get(&host.address) {
                        Some((ended, error)) if *ended == token => error.clone(),
                        _ => Error::Unreachable(words::CLOSED.into()),
                    });
                }
                (None, None) => {
                    state.tokens += 1;
                    let token = state.tokens;
                    let mut session = Session { token, number: random_number(), key: host.key, pair: pair.clone(), phase: Phase::Shaking(None), next: 1, answers: HashMap::new(), heard: Instant::now(), checking: None };
                    let message = self.handshake(&mut session);
                    let number = session.number;
                    state.sessions.insert(host.address, session);
                    mine = Some((token, Instant::now()));
                    if !self.relay_send(&mut state, &host.address, &frames::frame(Kind::Handshake1, number, &message)) {
                        return Err(Error::Unreachable(words::RELAY.into()));
                    }
                }
            }
        }
    }

    /// The handshake's first message for `session`, which keeps what reads the answer.
    fn handshake(&self, session: &mut Session) -> Vec<u8> {
        let inner = &self.inner;
        let mut initiator = Initiator::new(inner.identity.private(), session.key, noise::PROLOGUE);
        let mut hello = json!({ "v": 1, "name": inner.name, "app": inner.app, "client": "desktop" });
        if let Some(token) = &session.pair {
            hello["pair"] = Value::String(token.clone());
        }
        let message = initiator.write_message_1(hello.to_string().as_bytes());
        session.phase = Phase::Shaking(Some(initiator));
        message
    }

    /// Ends the session with the box at `address`: what waits on it hears `error`.
    fn end(&self, state: &mut State, address: &Address, error: Error, tell: bool) {
        let Some(session) = state.sessions.remove(address) else { return };
        for (_, answer) in session.answers {
            let _ = answer.send(Piece::Failed(error.clone()));
        }
        state.ended.insert(*address, (session.token, error));
        if matches!(session.phase, Phase::Open(..)) {
            *state.over.entry(*address).or_insert(0) += 1;
        }
        if tell && matches!(session.phase, Phase::Open(..)) {
            self.relay_send(state, address, &frames::frame(Kind::Close, session.number, &[]));
        }
        self.inner.changed.notify_all();
    }

    /// Something the relay sent on connection `id`.
    fn received(&self, id: u64, bytes: &[u8]) {
        let inner = &self.inner;
        let Some(message) = frames::read_relay(bytes) else { return };
        let mut state = self.state();
        match (&state.connection, &message) {
            (Connection::Welcoming { id: waiting, .. }, FromRelay::Welcome(address)) if *waiting == id => {
                if *address != inner.address {
                    return self.drop_connection(&mut state, Some(id));
                }
                if let Connection::Welcoming { id, out } = std::mem::replace(&mut state.connection, Connection::Down) {
                    state.connection = Connection::Up { id, out };
                }
                state.failed = None;
                inner.changed.notify_all();
                return;
            }
            (Connection::Up { id: up, .. }, _) if *up == id => {}
            _ => return,
        }
        match message {
            FromRelay::Deliver { from, payload } => self.delivered(&mut state, from, &payload),
            FromRelay::Unreachable(address) => self.end(&mut state, &address, Error::Unreachable(words::OFFLINE.into()), false),
            FromRelay::Gone { address, .. } => {
                // A handshake on its way may have reached the box's old connection, which won't
                // answer, or its new one: sent again under a new number, the new one answers that.
                let shaking = state.sessions.get(&address).is_some_and(|session| matches!(session.phase, Phase::Shaking(_)));
                if shaking {
                    let again = state.sessions.get_mut(&address).map(|session| {
                        session.number = random_number();
                        (session.number, self.handshake(session))
                    });
                    if let Some((number, message)) = again {
                        self.relay_send(&mut state, &address, &frames::frame(Kind::Handshake1, number, &message));
                    }
                } else {
                    self.end(&mut state, &address, Error::Unreachable(words::CLOSED.into()), false);
                }
            }
            FromRelay::Welcome(_) | FromRelay::Pushed { .. } => {}
        }
    }

    fn delivered(&self, state: &mut State, from: Address, payload: &[u8]) {
        let Some((kind, number, body)) = frames::read_frame(payload) else { return };
        let Some(session) = state.sessions.get_mut(&from) else { return };
        if session.number != number {
            return;
        }
        match kind {
            Kind::Handshake2 => {
                let Phase::Shaking(initiator) = &mut session.phase else { return };
                let Some(initiator) = initiator.take() else { return };
                let opened = initiator.read_message_2(body).ok().and_then(|(payload, transport)| Some((serde_json::from_slice::<BoxHello>(&payload).ok()?, transport)));
                match opened {
                    Some((hello, transport)) if hello.v == 1 => {
                        session.phase = Phase::Open(transport, hello);
                        session.heard = Instant::now();
                        self.inner.changed.notify_all();
                    }
                    Some(_) => {
                        // (Open, so it's told: a close.)
                        self.relay_send(state, &from, &frames::frame(Kind::Close, number, &[]));
                        self.end(state, &from, Error::Unreachable(words::VERSION.into()), false);
                    }
                    None => self.end(state, &from, Error::Unreachable(words::CLOSED.into()), false),
                }
            }
            Kind::Transport => {
                let Phase::Open(transport, _) = &mut session.phase else { return };
                let Ok(plain) = transport.open(body) else {
                    return self.end(state, &from, Error::Unreachable(words::CLOSED.into()), true);
                };
                session.heard = Instant::now();
                session.checking = None;
                let Some((head, body)) = frames::decode_message(&plain) else { return };
                let id = head.get("id").and_then(Value::as_u64);
                let ends = head.get("end").and_then(Value::as_bool) == Some(true);
                match (head.get("t").and_then(Value::as_str), id) {
                    (Some(kind @ ("res" | "data")), Some(id)) => {
                        let Some(answer) = session.answers.get(&id) else { return };
                        let mut gone = false;
                        if kind == "res" {
                            let status = head.get("s").and_then(Value::as_u64).unwrap_or(0) as u16;
                            let content_type = head.get("h").and_then(|headers| headers.get("content-type")).and_then(Value::as_str).map(str::to_owned);
                            gone |= answer.send(Piece::Head { status, content_type }).is_err();
                        }
                        if !body.is_empty() {
                            gone |= answer.send(Piece::Body(body.to_vec())).is_err();
                        }
                        if ends {
                            let _ = answer.send(Piece::End);
                        }
                        if ends || gone {
                            session.answers.remove(&id);
                        }
                    }
                    (Some("cancel"), Some(id)) => {
                        if let Some(answer) = session.answers.remove(&id) {
                            let _ = answer.send(Piece::Failed(Error::Unreachable(words::CLOSED.into())));
                        }
                    }
                    _ => {}
                }
            }
            // Before it opened: the box doesn't take this device. After: it ended it (a restart, a
            // removal), and what was under way is gone.
            Kind::Close => {
                let error = if matches!(session.phase, Phase::Shaking(_)) { unpaired() } else { Error::Unreachable(words::CLOSED.into()) };
                self.end(state, &from, error, false);
            }
            Kind::Handshake1 | Kind::Mail => {}
        }
    }

    /// Seals `plain` on the session at `address` and sends it.
    fn seal(&self, state: &mut State, address: &Address, plain: &[u8]) -> bool {
        let Some(session) = state.sessions.get_mut(address) else { return false };
        let Phase::Open(transport, _) = &mut session.phase else { return false };
        let framed = frames::frame(Kind::Transport, session.number, &transport.seal(plain));
        self.relay_send(state, address, &framed)
    }

    // MARK: Requests

    /// Sends `request` to `host`: its answer, to read as it comes.
    pub fn request(&self, host: &Host, request: &Request) -> Result<Answer> {
        let mut state = self.session(self.state(), host, None)?;
        let session = state.sessions.get_mut(&host.address).ok_or_else(|| Error::Unreachable(words::CLOSED.into()))?;
        let (id, token) = (session.next, session.token);
        session.next += 1;
        let (sender, receiver) = channel();
        session.answers.insert(id, sender);
        let mut headers = serde_json::Map::new();
        for (name, value) in &request.headers {
            headers.insert(name.clone(), Value::String(value.clone()));
        }
        let body = match &request.body {
            Some(body) => {
                headers.entry("content-type").or_insert_with(|| Value::String("application/json".into()));
                serde_json::to_vec(body).map_err(|error| Error::BadAnswer(error.to_string()))?
            }
            None => Vec::new(),
        };
        let head = json!({ "t": "req", "id": id, "m": request.method, "p": request.path, "h": Value::Object(headers) });
        let handle = Handle { client: self.clone(), address: host.address, token, id };
        let Some(pieces) = frames::pieces(&head, &body, true) else {
            handle.cancel();
            return Err(Error::BadAnswer("That’s too long to send.".into()));
        };
        for piece in pieces {
            if !self.seal(&mut state, &host.address, &piece) {
                return Err(Error::Unreachable(words::RELAY.into()));
            }
        }
        Ok(Answer { handle, receiver, ended: false })
    }

    /// A request there has had no answer for a while: its box is checked on (a ping), and one that
    /// doesn't answer the check in time has its session ended, with everything on it.
    fn check(&self, address: &Address, token: u64) -> Result<()> {
        let timing = self.inner.timing;
        let mut state = self.state();
        // What was sent is still going out (a picture, on a slow connection): its answer can't have
        // come yet, and the box isn't asked whether it's there behind it.
        let sending = matches!(&state.connection, Connection::Up { out, .. } if out.busy());
        let Some(session) = state.sessions.get_mut(address).filter(|session| session.token == token) else {
            return Err(Error::Unreachable(words::CLOSED.into()));
        };
        if sending {
            session.heard = Instant::now();
            session.checking = None;
            return Ok(());
        }
        match session.checking {
            Some(since) if since.elapsed() >= timing.check => {
                self.end(&mut state, address, Error::Unreachable(words::TIMED_OUT.into()), true);
                Err(Error::Unreachable(words::TIMED_OUT.into()))
            }
            Some(_) => Ok(()),
            None if session.heard.elapsed() >= timing.answer_within => {
                session.checking = Some(Instant::now());
                if self.seal(&mut state, address, &frames::encode_message(&json!({ "t": "ping" }), &[])) { Ok(()) } else { Err(Error::Unreachable(words::RELAY.into())) }
            }
            None => Ok(()),
        }
    }
}

/// Which request an answer is to: what ends it from any thread.
#[derive(Clone)]
pub struct Handle {
    client: LinkClient,
    address: Address,
    token: u64,
    id: u64,
}

impl Handle {
    /// Ends the request: the box is told to stop, and what reads its answer sees its end.
    pub fn cancel(&self) {
        let mut state = self.client.state();
        let Some(session) = state.sessions.get_mut(&self.address).filter(|session| session.token == self.token) else { return };
        if session.answers.remove(&self.id).is_some() && matches!(session.phase, Phase::Open(..)) {
            self.client.seal(&mut state, &self.address, &frames::encode_message(&json!({ "t": "cancel", "id": self.id }), &[]));
        }
    }
}

/// A request's answer, as it comes.
pub struct Answer {
    handle: Handle,
    receiver: Receiver<Piece>,
    ended: bool,
}

impl Answer {
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// The next piece, checking on the box while nothing comes (`checked`), or just waiting (a
    /// followed route is quiet while nothing changes).
    fn next(&mut self, checked: bool) -> Result<Piece> {
        let closed = || Error::Unreachable(words::CLOSED.into());
        if self.ended {
            return Ok(Piece::End);
        }
        let piece = loop {
            if !checked {
                break self.receiver.recv().map_err(|_| closed())?;
            }
            match self.receiver.recv_timeout(self.handle.client.inner.timing.answer_within) {
                Ok(piece) => break piece,
                Err(RecvTimeoutError::Disconnected) => return Err(closed()),
                Err(RecvTimeoutError::Timeout) => self.handle.client.check(&self.handle.address, self.handle.token)?,
            }
        };
        match piece {
            Piece::Failed(error) => {
                self.ended = true;
                Err(error)
            }
            Piece::End => {
                self.ended = true;
                Ok(Piece::End)
            }
            piece => Ok(piece),
        }
    }

    /// Its status and what its body is, waiting for them.
    pub fn head(&mut self) -> Result<(u16, Option<String>)> {
        match self.next(true)? {
            Piece::Head { status, content_type } => Ok((status, content_type)),
            _ => Err(Error::BadAnswer("The answer came without its head.".into())),
        }
    }

    /// The rest of its body, whole.
    pub fn body(&mut self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            match self.next(true)? {
                Piece::Body(bytes) => body.extend_from_slice(&bytes),
                Piece::End => return Ok(body),
                Piece::Head { .. } | Piece::Failed(_) => {}
            }
        }
    }

    /// Its body as a reader: bytes as they come, the end at its end (or once it's cancelled), an
    /// error if the session goes under it.
    pub fn reader(self) -> AnswerReader {
        AnswerReader { answer: self, left: Vec::new(), at: 0 }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        if !self.ended {
            self.handle.cancel();
        }
    }
}

pub struct AnswerReader {
    answer: Answer,
    left: Vec<u8>,
    at: usize,
}

impl Read for AnswerReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.at >= self.left.len() {
            match self.answer.next(false) {
                Ok(Piece::Body(bytes)) => {
                    self.left = bytes;
                    self.at = 0;
                }
                Ok(Piece::End) => return Ok(0),
                Ok(_) => {}
                // Cancelled from here (its sender let go): its end, not a failure.
                Err(Error::Unreachable(why)) if why == words::CLOSED && !self.answer.ended => {
                    self.answer.ended = true;
                    return Ok(0);
                }
                Err(error) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, error.to_string())),
            }
        }
        let count = buffer.len().min(self.left.len() - self.at);
        buffer[..count].copy_from_slice(&self.left[self.at..self.at + count]);
        self.at += count;
        Ok(count)
    }
}
