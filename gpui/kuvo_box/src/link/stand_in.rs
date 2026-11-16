//! A relay and a box on it, standing in for the real ones in tests and in the app's `--stand-in`:
//! the relay passes what devices send each other and says `unreachable` and `gone` as the real one
//! does; the host answers sessions as a box's link does, serving the routes of any [`Transport`]
//! it's given. All in this process (channels), or the relay on `127.0.0.1` for the WebSocket itself.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};

use super::frames::{self, FromRelay, Kind};
use super::keys::{self, Address, Identity};
use super::noise::{self, Responder};
use super::relay::{Dial, Incoming, Outgoing, memory_connection};
use crate::transport::{Closer, Request, Transport};

struct Connected {
    id: u64,
    out: Box<dyn Outgoing>,
    /// The devices that sent this connection something: told `gone` when it ends.
    heard: HashSet<Address>,
}

#[derive(Default)]
struct RelayState {
    next: u64,
    at: HashMap<Address, Connected>,
    down: bool,
    dials: usize,
    turned_away: usize,
}

/// The stand-in relay. Cheap to clone: clones share it.
#[derive(Clone, Default)]
pub struct Relay {
    state: Arc<Mutex<RelayState>>,
}

impl Relay {
    pub fn new() -> Relay {
        Relay::default()
    }

    fn state(&self) -> MutexGuard<'_, RelayState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How a device reaches it, in this process.
    pub fn dial(&self) -> Arc<dyn Dial> {
        Arc::new(self.clone())
    }

    /// Whether it takes connections (`true`: none, as with the network down).
    pub fn set_down(&self, down: bool) {
        self.state().down = down;
    }

    /// How many times a device tried to connect while it was down.
    pub fn turned_away(&self) -> usize {
        self.state().turned_away
    }

    /// How many times a device has connected to it.
    pub fn dials(&self) -> usize {
        self.state().dials
    }

    /// Whether a device is connected at `address`.
    pub fn has(&self, address: &Address) -> bool {
        self.state().at.contains_key(address)
    }

    /// Ends the connection at `address`, as a network drop does; the devices it talked to hear `gone`.
    pub fn drop_device(&self, address: &Address) {
        let mut state = self.state();
        if let Some(connected) = state.at.remove(address) {
            Self::tell_gone(&mut state, address, connected.heard, false);
        }
    }

    /// Ends every connection.
    pub fn drop_all(&self) {
        self.state().at.clear();
    }

    fn tell_gone(state: &mut RelayState, address: &Address, heard: HashSet<Address>, replaced: bool) {
        for device in heard {
            if let Some(other) = state.at.get_mut(&device) {
                let _ = other.out.send([&[7u8][..], address, &[if replaced { 0 } else { 1 }]].concat());
            }
        }
    }

    /// Serves one device's connection until it ends.
    pub fn serve(&self, mut out: Box<dyn Outgoing>, mut incoming: Box<dyn Incoming>) {
        let relay = self.clone();
        let _ = std::thread::Builder::new().name("stand-in-relay".into()).spawn(move || {
            let Ok(hello) = incoming.next() else { return };
            let Some(secret) = hello.strip_prefix(&[1u8]).and_then(|secret| <[u8; 32]>::try_from(secret).ok()) else { return };
            let address = keys::address_of(&secret);
            let id = {
                let mut state = relay.state();
                state.next += 1;
                let id = state.next;
                if out.send([&[1u8][..], &address].concat()).is_err() {
                    return;
                }
                // A second connection at the address replaces the first.
                if let Some(old) = state.at.insert(address, Connected { id, out, heard: HashSet::new() }) {
                    Self::tell_gone(&mut state, &address, old.heard, true);
                }
                id
            };
            while let Ok(message) = incoming.next() {
                let mut state = relay.state();
                if state.at.get(&address).is_none_or(|connected| connected.id != id) {
                    break;
                }
                let (Some(2), Some(to)) = (message.first().copied(), message.get(1..17).and_then(|to| Address::try_from(to).ok())) else { continue };
                let delivered = state.at.get_mut(&to).is_some_and(|target| {
                    target.heard.insert(address);
                    target.out.send([&[2u8][..], &address, &message[17..]].concat()).is_ok()
                });
                if !delivered && let Some(sender) = state.at.get_mut(&address) {
                    let _ = sender.out.send([&[3u8][..], &to].concat());
                }
            }
            let mut state = relay.state();
            if state.at.get(&address).is_some_and(|connected| connected.id == id)
                && let Some(connected) = state.at.remove(&address)
            {
                Self::tell_gone(&mut state, &address, connected.heard, false);
            }
        });
    }

    /// Listens on `127.0.0.1`, for a device that connects by WebSocket: its `ws://` address.
    #[cfg(unix)]
    pub fn listen(&self) -> io::Result<String> {
        use std::os::fd::AsRawFd;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let url = format!("ws://127.0.0.1:{}", listener.local_addr()?.port());
        let relay = self.clone();
        std::thread::Builder::new().name("stand-in-relay-listen".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                if relay.state().down {
                    continue;
                }
                relay.state().dials += 1;
                let fd = stream.as_raw_fd();
                let Ok(socket) = tungstenite::accept(stream) else { continue };
                if socket.get_ref().set_nonblocking(true).is_err() {
                    continue;
                }
                if let Ok((out, incoming)) = super::relay::pump(socket, fd, false) {
                    relay.serve(out, incoming);
                }
            }
        })?;
        Ok(url)
    }
}

impl Dial for Relay {
    fn dial(&self, _url: &str) -> io::Result<(Box<dyn Outgoing>, Box<dyn Incoming>)> {
        {
            let mut state = self.state();
            if state.down {
                state.turned_away += 1;
                return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "the stand-in relay is down"));
            }
            state.dials += 1;
        }
        let (device, relay) = memory_connection();
        self.serve(relay.0, relay.1);
        Ok(device)
    }
}

struct HostSession {
    number: u32,
    transport: noise::Transport,
    /// Requests whose body is still coming, and what closes those under way.
    incoming: HashMap<u64, (Value, Vec<u8>)>,
    running: HashMap<u64, Closer>,
}

struct HostState {
    out: Option<Box<dyn Outgoing>>,
    paired: HashSet<[u8; 32]>,
    tokens: HashSet<String>,
    sessions: HashMap<Address, HostSession>,
    hellos: Vec<Value>,
    quiet: bool,
    connection: u64,
}

struct HostInner {
    identity: Identity,
    relay: Relay,
    served: Arc<dyn Transport>,
    name: String,
    state: Mutex<HostState>,
}

/// A stand-in box on the stand-in relay: it lets in the devices paired with it and answers them
/// with `served`'s routes.
#[derive(Clone)]
pub struct Host {
    inner: Arc<HostInner>,
}

/// The relay's address every stand-in device says it's at.
pub const RELAY_URL: &str = "wss://relay.stand-in.invalid";

impl Host {
    /// Starts one on `relay`, connected.
    pub fn start(relay: &Relay, served: Arc<dyn Transport>, name: &str) -> Host {
        let host = Host {
            inner: Arc::new(HostInner {
                identity: Identity::new(),
                relay: relay.clone(),
                served,
                name: name.to_owned(),
                state: Mutex::new(HostState { out: None, paired: HashSet::new(), tokens: HashSet::new(), sessions: HashMap::new(), hellos: Vec::new(), quiet: false, connection: 0 }),
            }),
        };
        host.connect();
        host
    }

    fn state(&self) -> MutexGuard<'_, HostState> {
        self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Its key and where it is, as its `kuvo/computers` row would say.
    pub fn facts(&self) -> keys::Host {
        keys::Host { key: self.inner.identity.public(), address: self.inner.identity.address_on(RELAY_URL) }
    }

    pub fn key_text(&self) -> String {
        keys::encode(&self.facts().key)
    }

    pub fn address_text(&self) -> String {
        keys::encode(&self.facts().address)
    }

    /// Lets the device with this static key in.
    pub fn pair(&self, key: [u8; 32]) {
        self.state().paired.insert(key);
    }

    /// Takes a pairing token, once.
    pub fn take_token(&self, token: &str) {
        self.state().tokens.insert(token.to_owned());
    }

    /// Removes the device: its sessions end, told.
    pub fn remove(&self, key: &[u8; 32]) {
        let mut state = self.state();
        state.paired.remove(key);
        let sessions: Vec<(Address, u32)> = state.sessions.iter().map(|(address, session)| (*address, session.number)).collect();
        state.sessions.clear();
        for (address, number) in sessions {
            Self::send(&mut state, &address, &frames::frame(Kind::Close, number, &[]));
        }
    }

    /// What each device said as its session opened.
    pub fn hellos(&self) -> Vec<Value> {
        self.state().hellos.clone()
    }

    /// Whether it answers what it's sent (`true`: nothing, as a computer whose connection died
    /// without closing).
    pub fn set_quiet(&self, quiet: bool) {
        self.state().quiet = quiet;
    }

    /// Off the relay, as a computer that slept or stopped: its sessions go with its connection.
    pub fn stop(&self) {
        {
            let mut state = self.state();
            state.connection += 1;
            state.out = None;
            for (_, session) in state.sessions.drain() {
                for closer in session.running.values() {
                    closer.close();
                }
            }
        }
        self.inner.relay.drop_device(&self.facts().address);
    }

    /// On the relay (again), with no session kept from before.
    pub fn connect(&self) {
        let secret = self.inner.identity.secret_on(RELAY_URL);
        let Ok((mut out, mut incoming)) = self.inner.relay.dial().dial(RELAY_URL) else { return };
        let _ = out.send(frames::relay_hello(&secret));
        // (Its welcome, read before anything's served, so it's there once this returns.)
        let _ = incoming.next();
        let connection = {
            let mut state = self.state();
            state.connection += 1;
            state.out = Some(out);
            state.connection
        };
        let host = self.clone();
        let _ = std::thread::Builder::new().name("stand-in-host".into()).spawn(move || {
            while let Ok(bytes) = incoming.next() {
                if host.state().connection != connection {
                    break;
                }
                host.received(&bytes);
            }
        });
    }

    fn send(state: &mut HostState, to: &Address, payload: &[u8]) {
        if let Some(out) = &mut state.out {
            let _ = out.send(frames::relay_send(to, payload));
        }
    }

    fn seal(&self, to: &Address, number: u32, plain: &[u8]) {
        let mut state = self.state();
        let Some(session) = state.sessions.get_mut(to).filter(|session| session.number == number) else { return };
        let framed = frames::frame(Kind::Transport, number, &session.transport.seal(plain));
        Self::send(&mut state, to, &framed);
    }

    fn received(&self, bytes: &[u8]) {
        let mut state = self.state();
        if state.quiet {
            return;
        }
        match frames::read_relay(bytes) {
            Some(FromRelay::Deliver { from, payload }) => {
                let Some((kind, number, body)) = frames::read_frame(&payload) else { return };
                match kind {
                    Kind::Handshake1 => {
                        let mut responder = Responder::new(self.inner.identity.private(), noise::PROLOGUE);
                        let hello: Option<Value> = responder.read_message_1(body).ok().and_then(|hello| serde_json::from_slice(&hello).ok());
                        let Some(hello) = hello else { return Self::send(&mut state, &from, &frames::frame(Kind::Close, number, &[])) };
                        let shown = hello.get("pair").and_then(Value::as_str).is_some_and(|token| state.tokens.remove(token));
                        if shown {
                            state.paired.insert(responder.rs);
                        }
                        if hello.get("v").and_then(Value::as_u64) != Some(1) || !state.paired.contains(&responder.rs) {
                            return Self::send(&mut state, &from, &frames::frame(Kind::Close, number, &[]));
                        }
                        state.hellos.push(hello);
                        let (message, transport) = responder.write_message_2(json!({ "v": 1, "kind": "machine", "name": self.inner.name, "app": "0.8.0", "engine": null }).to_string().as_bytes());
                        state.sessions.insert(from, HostSession { number, transport, incoming: HashMap::new(), running: HashMap::new() });
                        Self::send(&mut state, &from, &frames::frame(Kind::Handshake2, number, &message));
                    }
                    Kind::Transport => {
                        let Some(session) = state.sessions.get_mut(&from).filter(|session| session.number == number) else {
                            return Self::send(&mut state, &from, &frames::frame(Kind::Close, number, &[]));
                        };
                        let Ok(plain) = session.transport.open(body) else {
                            state.sessions.remove(&from);
                            return Self::send(&mut state, &from, &frames::frame(Kind::Close, number, &[]));
                        };
                        let Some((head, body)) = frames::decode_message(&plain) else { return };
                        let id = head.get("id").and_then(Value::as_u64).unwrap_or(0);
                        match head.get("t").and_then(Value::as_str) {
                            Some("req") if head.get("end").and_then(Value::as_bool) == Some(false) => {
                                session.incoming.insert(id, (head, body.to_vec()));
                            }
                            Some("req") => {
                                drop(state);
                                self.run(from, number, id, &head, body.to_vec());
                            }
                            Some("data") => {
                                let Some((_, sofar)) = session.incoming.get_mut(&id) else { return };
                                sofar.extend_from_slice(body);
                                if head.get("end").and_then(Value::as_bool) == Some(true)
                                    && let Some((head, body)) = session.incoming.remove(&id)
                                {
                                    drop(state);
                                    self.run(from, number, id, &head, body);
                                }
                            }
                            Some("cancel") => {
                                if let Some(closer) = session.running.remove(&id) {
                                    closer.close();
                                }
                            }
                            Some("ping") => {
                                drop(state);
                                self.seal(&from, number, &frames::encode_message(&json!({ "t": "pong" }), &[]));
                            }
                            _ => {}
                        }
                    }
                    Kind::Close => {
                        if let Some(session) = state.sessions.remove(&from) {
                            for closer in session.running.values() {
                                closer.close();
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(FromRelay::Unreachable(address) | FromRelay::Gone { address, .. }) => {
                if let Some(session) = state.sessions.remove(&address) {
                    for closer in session.running.values() {
                        closer.close();
                    }
                }
            }
            _ => {}
        }
    }

    /// Answers one request with `served`'s answer, on a thread of its own: a followed route as it
    /// comes, until it's cancelled.
    fn run(&self, from: Address, number: u32, id: u64, head: &Value, body: Vec<u8>) {
        let mut request = Request {
            method: head.get("m").and_then(Value::as_str).unwrap_or("GET").to_owned(),
            path: head.get("p").and_then(Value::as_str).unwrap_or("").to_owned(),
            headers: Vec::new(),
            body: serde_json::from_slice(&body).ok(),
        };
        for (name, value) in head.get("h").and_then(Value::as_object).into_iter().flatten() {
            if name != "content-type"
                && let Some(value) = value.as_str()
            {
                request.headers.push((name.clone(), value.to_owned()));
            }
        }
        let host = self.clone();
        let _ = std::thread::Builder::new().name("stand-in-host-request".into()).spawn(move || {
            let reply = |head: Value, body: &[u8], end: bool| {
                for piece in frames::pieces(&head, body, end).unwrap_or_default() {
                    host.seal(&from, number, &piece);
                }
            };
            let headers = |content_type: Option<String>| json!({ "content-type": content_type.unwrap_or_else(|| "application/json".into()) });
            let follows = request.route().ends_with("/events") || request.route() == "api/event";
            if !follows {
                match host.inner.served.send(&request) {
                    Ok(response) => reply(json!({ "t": "res", "id": id, "s": response.status, "h": headers(response.content_type) }), &response.body, true),
                    Err(error) => reply(json!({ "t": "res", "id": id, "s": 502, "h": { "content-type": "text/plain" } }), error.to_string().as_bytes(), true),
                }
                return;
            }
            let hold = |closer: Closer| {
                let mut state = host.state();
                match state.sessions.get_mut(&from).filter(|session| session.number == number) {
                    Some(session) => {
                        session.running.insert(id, closer);
                        true
                    }
                    None => false,
                }
            };
            match host.inner.served.open(&request, &hold) {
                Ok(mut stream) => {
                    let mut first = Some(json!({ "t": "res", "id": id, "s": stream.status, "h": headers(stream.content_type.clone()) }));
                    loop {
                        let read = match stream.reader.fill_buf() {
                            Ok(bytes) if !bytes.is_empty() => bytes.to_vec(),
                            _ => break,
                        };
                        stream.reader.consume(read.len());
                        reply(first.take().unwrap_or_else(|| json!({ "t": "data", "id": id })), &read, false);
                    }
                    reply(first.take().unwrap_or_else(|| json!({ "t": "data", "id": id })), &[], true);
                }
                Err(error) => reply(json!({ "t": "res", "id": id, "s": 502, "h": { "content-type": "text/plain" } }), error.to_string().as_bytes(), true),
            }
            host.state().sessions.get_mut(&from).map(|session| session.running.remove(&id));
        });
    }
}
