//! This client's sessions with boxes, over one relay connection (PROTOCOL.md, "Sessions",
//! "Messages in a session"): one Noise session per box, its requests and followed routes told
//! apart by id, as the phone's `LinkClient` does it.
//!
//! Nothing runs on a timer here. The relay connection is made when something is asked and it's
//! down (never more often than its backoff allows), a session when its box is asked something, and
//! a request with no answer is checked on (a ping) by the thread that waits for it. A box that
//! goes away fails what waits on it; what follows a route opens it again by its own backoff
//! ([`crate::follow`]).

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use super::frames::{self, FromRelay, Kind};
use super::keys::{Address, Host, Identity};
use super::log;
use super::noise::{self, Initiator};
use super::relay::{Dial, Incoming, Outgoing};
use crate::error::{Error, Refusal, Result};
use crate::transport::{check_route_version, Request, Socket, SocketMessage, SocketOut, ROUTE_VERSION_HEADER};

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
    /// How long a request safe to send again (a message under its id) is sent again for, from when
    /// it first failed (PROTOCOL.md, "Sending again"): 2 minutes.
    pub repeat_for: Duration,
    /// The wait before its second try, doubling up to `repeat_most` (1, 2, 4, 8 s); a relay
    /// connection made meanwhile sends it at once. (The first goes again at once.)
    pub repeat_first: Duration,
    pub repeat_most: Duration,
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
            repeat_for: Duration::from_secs(120),
            repeat_first: Duration::from_secs(1),
            repeat_most: Duration::from_secs(8),
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

#[derive(Deserialize)]
struct HandshakeHello {
    #[serde(flatten)]
    hello: BoxHello,
    waiting: Option<String>,
    reason: Option<String>,
    title: Option<String>,
    action: Option<crate::error::Offered>,
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
    pub const PAIRING_WAIT: &str = "pairing-wait";
    pub const CLIENT_UPDATE_REQUIRED: &str = "client-update-required";
    pub const PAIRING_READD: &str = "pairing-readd";
}

/// The box turned this device away: it isn't paired there (any more).
pub fn unpaired() -> Error {
    Error::Refused(Refusal { status: 403, message: words::REFUSED.into(), title: None, reason: Some(words::UNPAIRED.into()), older_box: false, action: None })
}

/// Whether `error` is a box turning this device away.
pub fn is_unpaired(error: &Error) -> bool {
    error.refusal().is_some_and(|refusal| refusal.reason.as_deref() == Some(words::UNPAIRED))
}

/// A piece of an answer, as it comes.
enum Piece {
    Head { status: u16, content_type: Option<String> },
    Body(Vec<u8>),
    /// A socket's binary message (`h.type = "binary"`: PROTOCOL.md, "Sockets").
    Binary(Vec<u8>),
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
    /// What the box has had for sure, in bytes handed to the relay connection (what went before a
    /// request it answered, or a ping): a check waits longer for what may still be on its way.
    arrived: u64,
    /// Requests with no answer yet, and the pings on their way: how much had been handed over once
    /// each had gone.
    unanswered: HashMap<u64, u64>,
    pings: VecDeque<u64>,
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
    /// Bytes handed to relay connections so far.
    handed: u64,
    /// The messages being sent again, by id, and those of them Stop took back.
    repeating: std::collections::HashSet<String>,
    taken_back: std::collections::HashSet<String>,
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
                state: Mutex::new(State { connection: Connection::Down, connections: 0, failed: None, sessions: HashMap::new(), tokens: 0, over: HashMap::new(), ended: HashMap::new(), handed: 0, repeating: Default::default(), taken_back: Default::default() }),
                changed: Condvar::new(),
            }),
        }
    }

    pub fn relay(&self) -> &str {
        &self.inner.relay
    }

    pub fn timing(&self) -> Timing {
        self.inner.timing
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
        let state = self.session(self.state(), host, None, false)?;
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
        let state = self.session(state, host, Some(token.to_owned()), false)?;
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
        if !matches!(state.connection, Connection::Down) {
            log::note(format!("relay {}: let go (the computer woke, the network changed, or this device was let go)", self.inner.relay));
        }
        self.drop_connection(&mut state, None);
    }

    /// How many relay connections have been made so far: one made after a number says the relay's
    /// back ([`LinkClient::wait_for_relay`]).
    pub fn connections(&self) -> u64 {
        self.state().connections
    }

    /// A message under `key` is being sent again (`true`), or isn't any more: while it is, Stop
    /// can take it back ([`LinkClient::take_back`]).
    pub fn repeating(&self, key: &str, now: bool) {
        let mut state = self.state();
        if now {
            state.repeating.insert(key.to_owned());
        } else {
            state.repeating.remove(key);
            state.taken_back.remove(key);
        }
    }

    /// Stop: the messages under `keys` being sent again are taken back.
    pub fn take_back(&self, keys: &[String]) {
        let mut state = self.state();
        for key in keys {
            if state.repeating.contains(key) {
                state.taken_back.insert(key.clone());
            }
        }
        self.inner.changed.notify_all();
    }

    pub fn is_taken_back(&self, key: &str) -> bool {
        self.state().taken_back.contains(key)
    }

    /// Waits until a relay connection after the `after`th is up, the message under `key` is taken
    /// back, or `until`: whether the relay's up.
    pub fn wait_for_relay(&self, after: u64, until: Instant, key: &str) -> bool {
        let mut state = self.state();
        loop {
            if state.connections > after && matches!(state.connection, Connection::Up { .. }) {
                return true;
            }
            if state.taken_back.contains(key) {
                return false;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self.inner.changed.wait_timeout(state, left).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    // MARK: The relay connection

    /// With the relay connection up, making it if it's down. `eager`: now, even within the wait
    /// after a try that failed (a message sent again, which someone waits on).
    fn connected<'a>(&'a self, mut state: MutexGuard<'a, State>, eager: bool) -> Result<MutexGuard<'a, State>> {
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
                    if let Some((at, tries)) = state.failed.filter(|_| !eager) {
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
                    let (mut out, incoming) = match dialed {
                        Ok(halves) => halves,
                        Err(error) => {
                            log::note(format!("relay {}: couldn’t connect ({error})", inner.relay));
                            fail(&mut state);
                            return Err(unreachable());
                        }
                    };
                    if out.send(frames::relay_hello(&inner.secret)).is_err() {
                        log::note(format!("relay {}: couldn’t say hello", inner.relay));
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
                                    log::note(format!("relay {}: connected, no welcome within {:?}", inner.relay, inner.timing.welcome));
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
            let why = loop {
                match incoming.next() {
                    Ok(bytes) => client.received(id, &bytes),
                    Err(error) => break error.to_string(),
                }
            };
            let mut state = client.state();
            let current = matches!(&state.connection, Connection::Welcoming { id: now, .. } | Connection::Up { id: now, .. } if *now == id);
            if current {
                let sessions = state.sessions.values().map(|session| session.answers.len()).collect::<Vec<_>>();
                log::note(format!("relay {}: connection {id} lost: {why}; {} session(s) with {} request(s) under way end", client.inner.relay, sessions.len(), sessions.iter().sum::<usize>()));
            }
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
        let message = frames::relay_send(to, payload);
        let length = message.len() as u64;
        let sent = match &mut state.connection {
            Connection::Up { out, .. } => out.send(message).is_ok(),
            _ => false,
        };
        if sent {
            state.handed += length;
        } else {
            if matches!(state.connection, Connection::Up { .. }) {
                log::note(format!("relay {}: the connection wouldn’t take what was sent", self.inner.relay));
            }
            self.drop_connection(state, None);
        }
        sent
    }

    // MARK: Sessions

    /// With a session with `host` open, opening one if there's none.
    fn session<'a>(&'a self, mut state: MutexGuard<'a, State>, host: &Host, pair: Option<String>, eager: bool) -> Result<MutexGuard<'a, State>> {
        let inner = &self.inner;
        let mut mine: Option<(u64, Instant)> = None;
        loop {
            state = self.connected(state, eager)?;
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
                        log::note(format!("{}: no answer to the handshake within {:?}", address_text(&host.address), inner.timing.handshake));
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
                    let mut session = Session { token, number: random_number(), key: host.key, pair: pair.clone(), phase: Phase::Shaking(None), next: 1, answers: HashMap::new(), heard: Instant::now(), checking: None, arrived: state.handed, unanswered: HashMap::new(), pings: VecDeque::new() };
                    let message = self.handshake(&mut session)?;
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
    fn handshake(&self, session: &mut Session) -> crate::Result<Vec<u8>> {
        let inner = &self.inner;
        let mut initiator = inner.identity.initiate(session.key, noise::PROLOGUE);
        let mut hello = json!({ "v": 1, "name": inner.name, "app": inner.app, "client": "desktop" });
        if let Some(token) = &session.pair {
            hello["pair"] = Value::String(token.clone());
        }
        let message = initiator.write_message_1(hello.to_string().as_bytes())
            .map_err(|_| Error::BadAnswer("Invalid computer key.".into()))?;
        session.phase = Phase::Shaking(Some(initiator));
        Ok(message)
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
            FromRelay::Unreachable(address) => {
                if state.sessions.contains_key(&address) {
                    log::note(format!("{}: the relay says it isn’t connected", address_text(&address)));
                }
                self.end(&mut state, &address, Error::Unreachable(words::OFFLINE.into()), false)
            }
            FromRelay::Gone { address, .. } => {
                // A handshake on its way may have reached the box's old connection, which won't
                // answer, or its new one: sent again under a new number, the new one answers that.
                let shaking = state.sessions.get(&address).is_some_and(|session| matches!(session.phase, Phase::Shaking(_)));
                if shaking {
                    let again = state.sessions.get_mut(&address).map(|session| {
                        session.number = random_number();
                        (session.number, self.handshake(session))
                    });
                    match again {
                        Some((number, Ok(message))) => {
                            self.relay_send(&mut state, &address, &frames::frame(Kind::Handshake1, number, &message));
                        }
                        Some((_, Err(error))) => self.end(&mut state, &address, error, false),
                        None => {}
                    }
                } else {
                    if state.sessions.contains_key(&address) {
                        log::note(format!("{}: its relay connection went or was replaced (it started again, or reconnected)", address_text(&address)));
                    }
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
                let opened = initiator.read_message_2(body).ok().and_then(|(payload, transport)| Some((serde_json::from_slice::<HandshakeHello>(&payload).ok()?, transport)));
                match opened {
                    Some((HandshakeHello { hello, waiting: Some(message), reason, title, action }, _)) if hello.v == 1 => {
                        let readd = reason.as_deref() == Some(words::PAIRING_READD);
                        let update = reason.as_deref() == Some(words::CLIENT_UPDATE_REQUIRED);
                        let refusal = Refusal { status: if update { 426 } else if readd { 409 } else { 503 }, message, title,
                            reason: Some(if update { words::CLIENT_UPDATE_REQUIRED } else if readd { words::PAIRING_READD } else { words::PAIRING_WAIT }.into()),
                            older_box: false, action };
                        self.end(state, &from, Error::Refused(refusal), false);
                    }
                    Some((HandshakeHello { hello, .. }, transport)) if hello.v == 1 => {
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
                    log::note(format!("{}: a message that couldn’t be opened; its session ends", address_text(&from)));
                    return self.end(state, &from, Error::Unreachable(words::CLOSED.into()), true);
                };
                session.heard = Instant::now();
                session.checking = None;
                let Some((head, body)) = frames::decode_message(&plain) else { return };
                let id = head.get("id").and_then(Value::as_u64);
                let ends = head.get("end").and_then(Value::as_bool) == Some(true);
                match (head.get("t").and_then(Value::as_str), id) {
                    (Some(kind @ ("res" | "data")), Some(id)) => {
                        // It got there: what was handed over before it did too.
                        if let Some(at) = session.unanswered.remove(&id) {
                            session.arrived = session.arrived.max(at);
                        }
                        if kind == "res" {
                            let version = head.get("h").and_then(|headers| headers.get(ROUTE_VERSION_HEADER));
                            if let Err(error) = check_route_version(version.map(|value| value.as_str().unwrap_or(""))) {
                                if let Some(answer) = session.answers.remove(&id) {
                                    let _ = answer.send(Piece::Failed(error));
                                }
                                return;
                            }
                        }
                        let Some(answer) = session.answers.get(&id) else { return };
                        let mut gone = false;
                        if kind == "res" {
                            let status = head.get("s").and_then(Value::as_u64).unwrap_or(0) as u16;
                            let content_type = head.get("h").and_then(|headers| headers.get("content-type")).and_then(Value::as_str).map(str::to_owned);
                            gone |= answer.send(Piece::Head { status, content_type }).is_err();
                        }
                        if !body.is_empty() {
                            let binary = head.get("h").and_then(|headers| headers.get("type")).and_then(Value::as_str) == Some("binary");
                            gone |= answer.send(if binary { Piece::Binary(body.to_vec()) } else { Piece::Body(body.to_vec()) }).is_err();
                        }
                        if ends {
                            let _ = answer.send(Piece::End);
                        }
                        if ends || gone {
                            session.answers.remove(&id);
                        }
                    }
                    (Some("pong"), _) => {
                        if let Some(at) = session.pings.pop_front() {
                            session.arrived = session.arrived.max(at);
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
                log::note(format!("{}: the box closed the session ({})", address_text(&from), if matches!(session.phase, Phase::Shaking(_)) { "it doesn’t take this device" } else { "it started again, or ended it" }));
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
        self.request_now(host, request, false)
    }

    /// As [`LinkClient::request`]; `eager`: the relay is dialed now even within its wait after a
    /// try that failed (a message sent again, which someone waits on).
    pub fn request_now(&self, host: &Host, request: &Request, eager: bool) -> Result<Answer> {
        let mut state = self.session(self.state(), host, None, eager)?;
        let session = state.sessions.get_mut(&host.address).ok_or_else(|| Error::Unreachable(words::CLOSED.into()))?;
        let (id, token) = (session.next, session.token);
        session.next += 1;
        let (sender, receiver) = channel();
        session.answers.insert(id, sender);
        let mut headers = serde_json::Map::new();
        for (name, value) in request.wire_headers() {
            headers.insert(name.to_owned(), Value::String(value.to_owned()));
        }
        let body = match &request.body {
            Some(body) => serde_json::to_vec(body).map_err(|error| Error::BadAnswer(error.to_string()))?,
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
        let handed = state.handed;
        if let Some(session) = state.sessions.get_mut(&host.address).filter(|session| session.token == token) {
            session.unanswered.insert(id, handed);
        }
        Ok(Answer { handle, receiver, ended: false })
    }

    /// Opens a socket on `host`'s engine (PROTOCOL.md, "Sockets"): the request, with `upgrade:
    /// websocket` and no body, is answered `101` once the engine's socket is open (another status,
    /// with its words, if it won't open); then each `data` message for it is one message on the
    /// socket, both ways. Its messages are read on a thread of its own, which ends with it.
    pub fn socket(&self, host: &Host, request: &Request) -> Result<Socket> {
        let mut asked = request.clone().header("upgrade", "websocket");
        asked.body = None;
        let mut answer = self.request(host, &asked)?;
        let (status, _) = answer.head()?;
        if status != 101 {
            let body = answer.body().unwrap_or_default();
            return Err(Error::Refused(socket_refusal(status, &body)));
        }
        let handle = answer.handle();
        let (sender, receiver) = async_channel::unbounded();
        let spawned = std::thread::Builder::new().name("kuvo-link-socket".into()).spawn(move || {
            loop {
                let message = match answer.next(false, None) {
                    Ok(Piece::Body(bytes)) => SocketMessage::Text(bytes),
                    Ok(Piece::Binary(bytes)) => SocketMessage::Binary(bytes),
                    Ok(Piece::End) => SocketMessage::Ended(None),
                    Ok(Piece::Head { .. } | Piece::Failed(_)) => continue,
                    Err(error) => SocketMessage::Ended(Some(error.to_string())),
                };
                let ended = matches!(message, SocketMessage::Ended(_));
                // (No one reads it any more: dropping the answer closes it, telling the box.)
                if sender.send_blocking(message).is_err() || ended {
                    return;
                }
            }
        });
        if spawned.is_err() {
            handle.cancel();
            return Err(Error::Unreachable(words::CLOSED.into()));
        }
        Ok(Socket { incoming: receiver, outgoing: Arc::new(LinkSocket { handle }) })
    }

    /// A request there has had no answer for a while: its box is checked on (a ping), and one that
    /// doesn't answer the check in time has its session ended, with everything on it.
    fn check(&self, address: &Address, token: u64) -> Result<()> {
        let timing = self.inner.timing;
        let mut state = self.state();
        // What was sent is still going out (a picture, on a slow connection): its answer can't have
        // come yet, and the box isn't asked whether it's there behind it.
        let sending = matches!(&state.connection, Connection::Up { out, .. } if out.busy());
        let handed = state.handed;
        let Some(session) = state.sessions.get_mut(address).filter(|session| session.token == token) else {
            return Err(Error::Unreachable(words::CLOSED.into()));
        };
        if sending {
            session.heard = Instant::now();
            session.checking = None;
            return Ok(());
        }
        // What's been handed over that the box hasn't said it had may still be on its way, ahead of
        // the ping (in the system's buffers, on a slow connection): a second more for each 64 KB.
        let behind = handed.saturating_sub(session.arrived);
        let allowed = timing.check + super::relay::behind(behind);
        match session.checking {
            Some(since) if since.elapsed() >= allowed => {
                log::note(format!("{}: no answer to a check within {allowed:?} ({behind} bytes it hadn’t said it had); its session ends", address_text(address)));
                self.end(&mut state, address, Error::Unreachable(words::TIMED_OUT.into()), true);
                Err(Error::Unreachable(words::TIMED_OUT.into()))
            }
            Some(_) => Ok(()),
            None if session.heard.elapsed() >= timing.answer_within => {
                session.checking = Some(Instant::now());
                let sealed = self.seal(&mut state, address, &frames::encode_message(&json!({ "t": "ping" }), &[]));
                let handed = state.handed;
                if let Some(session) = state.sessions.get_mut(address) {
                    session.pings.push_back(handed);
                }
                if sealed { Ok(()) } else { Err(Error::Unreachable(words::RELAY.into())) }
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
    /// Sends `body` as one `data` message on the request (a socket's message: PROTOCOL.md,
    /// "Sockets"), while it's under way.
    pub fn data(&self, body: &[u8]) -> Result<()> {
        let message = frames::encode_message(&json!({ "t": "data", "id": self.id }), body);
        if message.len() > frames::MAX_MESSAGE {
            return Err(Error::BadAnswer("That’s too long to send.".into()));
        }
        let mut state = self.client.state();
        let under_way = state.sessions.get(&self.address).is_some_and(|session| session.token == self.token && session.answers.contains_key(&self.id) && matches!(session.phase, Phase::Open(..)));
        if !under_way {
            return Err(Error::Unreachable(words::CLOSED.into()));
        }
        if self.client.seal(&mut state, &self.address, &message) { Ok(()) } else { Err(Error::Unreachable(words::RELAY.into())) }
    }

    /// Ends the request: the box is told to stop, and what reads its answer sees its end.
    pub fn cancel(&self) {
        let mut state = self.client.state();
        let Some(session) = state.sessions.get_mut(&self.address).filter(|session| session.token == self.token) else { return };
        session.unanswered.remove(&self.id);
        if session.answers.remove(&self.id).is_some() && matches!(session.phase, Phase::Open(..)) {
            self.client.seal(&mut state, &self.address, &frames::encode_message(&json!({ "t": "cancel", "id": self.id }), &[]));
        }
    }
}

/// What sends on a socket opened over the link, and closes it.
struct LinkSocket {
    handle: Handle,
}

impl SocketOut for LinkSocket {
    fn send_text(&self, text: &str) -> Result<()> {
        self.handle.data(text.as_bytes())
    }

    fn close(&self) {
        self.handle.cancel();
    }
}

/// Why a socket didn't open, as its host said: a `502` it couldn't open carries a line of text
/// saying why (the engine isn't running, it closed it), which is shown; anything else is read as
/// any refusal.
fn socket_refusal(status: u16, body: &[u8]) -> Refusal {
    let said = std::str::from_utf8(body).ok().map(str::trim).filter(|text| !text.is_empty() && text.chars().count() <= 300 && !text.contains('\n') && !text.starts_with(['<', '{']));
    match said {
        Some(text) if status == 502 => Refusal { status, message: text.to_owned(), title: None, reason: None, older_box: false, action: None },
        _ => Refusal::from_answer(status, body),
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
    fn next(&mut self, checked: bool, idle: Option<Duration>) -> Result<Piece> {
        let closed = || Error::Unreachable(words::CLOSED.into());
        if self.ended {
            return Ok(Piece::End);
        }
        let piece = loop {
            if !checked {
                break match idle {
                    Some(idle) => self.receiver.recv_timeout(idle).map_err(|error| match error {
                        RecvTimeoutError::Disconnected => closed(),
                        RecvTimeoutError::Timeout => Error::Unreachable(words::TIMED_OUT.into()),
                    }),
                    None => self.receiver.recv().map_err(|_| closed()),
                }?;
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
        match self.next(true, None)? {
            Piece::Head { status, content_type } => Ok((status, content_type)),
            _ => Err(Error::BadAnswer("The answer came without its head.".into())),
        }
    }

    /// The rest of its body, whole.
    pub fn body(&mut self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            match self.next(true, None)? {
                Piece::Body(bytes) => body.extend_from_slice(&bytes),
                Piece::End => return Ok(body),
                Piece::Head { .. } | Piece::Binary(_) | Piece::Failed(_) => {}
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
            match self.answer.next(false, None) {
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
                Err(Error::Unreachable(why)) if why == words::TIMED_OUT => {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, why));
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

/// A box's address on the relay, for the log: its first characters, enough to tell boxes apart.
fn address_text(address: &Address) -> String {
    let text = super::keys::encode(address);
    format!("box {}", &text[..text.len().min(8)])
}
