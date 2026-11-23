//! The link (`kuvo_box::link`): held to the vectors every implementation runs
//! (`protocol/vectors/noise.json`, `link.json`), and its scenarios against a stand-in relay and a
//! stand-in box on it. Nothing real is reached.

use std::io::{BufRead, Read};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kuvo_box::link::client::words;
use kuvo_box::link::stand_in::{Host, RELAY_URL, Relay};
use kuvo_box::link::{Identity, LinkBox, LinkClient, Timing, WebSocketDial, frames, is_unpaired, keys, noise};
use kuvo_box::transport::{Closer, Request, Response, Socket, SocketMessage, SocketOut, Stream, Transport};
use kuvo_box::{BoxClient, Error, Gate, gate};
use serde_json::{Value, json};

fn vectors(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/vectors").join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))).expect("the vectors are JSON")
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex")).collect()
}

fn hex32(value: &Value) -> [u8; 32] {
    hex(value.as_str().expect("hex text")).try_into().expect("32 bytes")
}

// MARK: Vectors

#[test]
fn the_handshake_and_transport_match_the_noise_vectors() {
    let all = vectors("noise.json");
    let vector = all["vectors"].as_array().unwrap().iter().find(|vector| vector["protocol_name"] == "Noise_IK_25519_AESGCM_SHA256").expect("the IK vector");
    let prologue = hex(vector["init_prologue"].as_str().unwrap());
    let messages = vector["messages"].as_array().unwrap();
    let mut initiator = noise::Initiator::with_ephemeral(hex32(&vector["init_static"]), hex32(&vector["init_remote_static"]), &prologue, hex32(&vector["init_ephemeral"]));
    let mut responder = noise::Responder::with_ephemeral(hex32(&vector["resp_static"]), &prologue, hex32(&vector["resp_ephemeral"]));
    let payload = |index: usize| hex(messages[index]["payload"].as_str().unwrap());
    let ciphertext = |index: usize| hex(messages[index]["ciphertext"].as_str().unwrap());

    let first = initiator.write_message_1(&payload(0)).unwrap();
    assert_eq!(first, ciphertext(0));
    assert_eq!(responder.read_message_1(&first).unwrap(), payload(0));
    let (second, mut answering) = responder.write_message_2(&payload(1)).unwrap();
    assert_eq!(second, ciphertext(1));
    let (read, mut asking) = initiator.read_message_2(&second).unwrap();
    assert_eq!(read, payload(1));
    assert_eq!(asking.hash.to_vec(), hex(vector["handshake_hash"].as_str().unwrap()));
    for index in 2..messages.len() {
        let (from, to) = if index % 2 == 0 { (&mut asking, &mut answering) } else { (&mut answering, &mut asking) };
        let sealed = from.seal(&payload(index));
        assert_eq!(sealed, ciphertext(index), "message {index}");
        assert_eq!(to.open(&sealed).unwrap(), payload(index));
    }
    // Anything that doesn't open ends the session: it's an error, never bytes.
    assert!(asking.open(b"not a sealed message").is_err());
}

#[test]
fn addresses_frames_and_messages_match_the_link_vectors() {
    let link = vectors("link.json");
    for case in link["addresses"].as_array().unwrap() {
        assert_eq!(keys::address_of(&hex32(&case["secret"])).to_vec(), hex(case["address"].as_str().unwrap()));
    }
    for case in link["relay"].as_array().unwrap() {
        let bytes = hex(case["bytes"].as_str().unwrap());
        let name = case["name"].as_str().unwrap();
        let address = || -> [u8; 16] { hex(case["address"].as_str().unwrap()).try_into().unwrap() };
        match (case["from"].as_str().unwrap(), name) {
            ("device", "hello") => assert_eq!(frames::relay_hello(&bytes[1..].try_into().unwrap()), bytes),
            ("device", "send") => assert_eq!(frames::relay_send(&hex(case["to"].as_str().unwrap()).try_into().unwrap(), &hex(case["payload"].as_str().unwrap())), bytes),
            ("relay", "welcome") => assert_eq!(frames::read_relay(&bytes), Some(frames::FromRelay::Welcome(address()))),
            ("relay", "deliver") => assert_eq!(frames::read_relay(&bytes), Some(frames::FromRelay::Deliver { from: address(), payload: hex(case["payload"].as_str().unwrap()) })),
            ("relay", "unreachable") => assert_eq!(frames::read_relay(&bytes), Some(frames::FromRelay::Unreachable(address()))),
            ("relay", "pushed") => assert_eq!(frames::read_relay(&bytes), Some(frames::FromRelay::Pushed { to: address(), status: case["status"].as_u64().unwrap() as u8 })),
            ("relay", "gone") => assert_eq!(frames::read_relay(&bytes), Some(frames::FromRelay::Gone { address: address(), replaced: case["reads"] == "replaced" })),
            // What only a phone or a host sends (register, push): not this client's.
            _ => {}
        }
    }
    for case in link["frames"].as_array().unwrap() {
        let bytes = hex(case["bytes"].as_str().unwrap());
        let (kind, session, body) = frames::read_frame(&bytes).expect("a frame");
        assert_eq!((kind as u64, session as u64, body.to_vec()), (case["kind"].as_u64().unwrap(), case["session"].as_u64().unwrap(), hex(case["body"].as_str().unwrap())));
        assert_eq!(frames::frame(kind, session, body), bytes);
    }
    for case in link["messages"].as_array().unwrap() {
        let bytes = hex(case["bytes"].as_str().unwrap());
        let body = hex(case["body"].as_str().unwrap());
        assert_eq!(frames::encode_message(&case["head"], &body), bytes, "{}", case["head"]);
        let (head, read) = frames::decode_message(&bytes).expect("a message");
        assert_eq!((head, read.to_vec()), (case["head"].clone(), body));
    }
}

#[test]
fn a_device_has_an_address_of_its_own_on_each_relay_and_never_shows_its_keys() {
    let identity = Identity::from_bytes([7; 32], [9; 32]);
    assert_ne!(identity.address_on("wss://one.example"), identity.address_on("wss://two.example"));
    assert_eq!(identity.address_on("wss://one.example"), identity.address_on("wss://one.example"));
    // Kept and read back, it's the same device.
    let again = Identity::from_kept(&identity.kept()).expect("read back");
    assert_eq!((again.public(), again.address_on(RELAY_URL)), (identity.public(), identity.address_on(RELAY_URL)));
    // Never its key or secret, wherever it's shown.
    let shown = format!("{identity:?}");
    assert!(!shown.contains(&keys::encode(&[7; 32])) && !shown.contains(&keys::encode(&[9; 32])), "{shown}");
}

// MARK: Scenarios

/// What the stand-in box answers: a few routes, enough to see requests, big bodies and a followed
/// route go through the link whole.
#[derive(Default)]
struct Served {
    seen: Mutex<Vec<Request>>,
    /// The followed route's lines, written by the test; and whether it's been closed.
    ticks: Mutex<Option<Receiver<String>>>,
    closed: Arc<Mutex<bool>>,
    features: Mutex<Option<Vec<String>>>,
    /// What its `kuvo/info` says of this client (`compatibility`), where a test says.
    compatibility: Mutex<Option<Value>>,
    /// Once set, the next "send" it's asked drops this device's relay connection as it's taken,
    /// before its answer can go back (a network drop just then).
    drops: Mutex<Option<(Relay, keys::Address)>>,
    /// A terminal's socket, as the box's engine has it.
    terminal: Arc<Terminal>,
}

/// What a terminal's socket on the box was asked, what was typed in it, and who's told it closed.
#[derive(Default)]
struct Terminal {
    asked: Mutex<Vec<Request>>,
    typed: Mutex<Vec<String>>,
    closed: Mutex<Option<Sender<()>>>,
    shell: Mutex<Option<async_channel::Sender<SocketMessage>>>,
}

/// The engine's side of a terminal's socket: what's typed is echoed back, as a shell does.
struct Shell(Arc<Terminal>);

impl SocketOut for Shell {
    fn send_text(&self, text: &str) -> kuvo_box::Result<()> {
        self.0.typed.lock().unwrap().push(text.to_owned());
        if let Some(shell) = self.0.shell.lock().unwrap().as_ref() {
            let _ = shell.send_blocking(SocketMessage::Text(text.as_bytes().to_vec()));
        }
        Ok(())
    }

    fn close(&self) {
        if let Some(shell) = self.0.shell.lock().unwrap().take() {
            let _ = shell.send_blocking(SocketMessage::Ended(None));
        }
        if let Some(told) = self.0.closed.lock().unwrap().take() {
            let _ = told.send(());
        }
    }
}

impl Served {
    fn new() -> (Arc<Served>, Sender<String>) {
        let (sender, receiver) = channel();
        let served = Served { ticks: Mutex::new(Some(receiver)), ..Served::default() };
        (Arc::new(served), sender)
    }
}

struct Ticks(Receiver<String>, Vec<u8>);

impl Read for Ticks {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.1.is_empty() {
            match self.0.recv() {
                Ok(line) => self.1 = line.into_bytes(),
                Err(_) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.1.len());
        buffer[..count].copy_from_slice(&self.1[..count]);
        self.1.drain(..count);
        Ok(count)
    }
}

impl Transport for Served {
    fn send(&self, request: &Request) -> kuvo_box::Result<Response> {
        self.seen.lock().unwrap().push(request.clone());
        let json = |status: u16, value: Value| Ok(Response { status, content_type: Some("application/json".into()), body: value.to_string().into_bytes() });
        match request.route() {
            "kuvo/info" => {
                let features = self.features.lock().unwrap().clone().unwrap_or_else(|| gate::REQUIRED.iter().map(|name| (*name).to_owned()).collect());
                let mut info = json!({ "kind": "machine", "name": "devbox", "features": features, "chats": "/home/me/chats" });
                if let Some(compatibility) = self.compatibility.lock().unwrap().clone() {
                    info["compatibility"] = compatibility;
                }
                json(200, info)
            }
            "echo" => json(200, json!({ "method": request.method, "path": request.path, "headers": request.headers, "body": request.body })),
            "big" => json(200, json!({ "text": "x".repeat(200_000) })),
            "send" => {
                if let Some((relay, device)) = self.drops.lock().unwrap().take() {
                    relay.drop_device(&device);
                }
                json(200, json!({}))
            }
            _ => json(404, json!({})),
        }
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> kuvo_box::Result<Stream> {
        self.seen.lock().unwrap().push(request.clone());
        let closed = self.closed.clone();
        let closer = Closer::new(move || *closed.lock().unwrap() = true);
        hold(closer.clone());
        let ticks = self.ticks.lock().unwrap().take().expect("followed once");
        Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(std::io::BufReader::new(Ticks(ticks, Vec::new()))), closer })
    }

    fn socket(&self, request: &Request) -> kuvo_box::Result<Socket> {
        self.terminal.asked.lock().unwrap().push(request.clone());
        if request.route() != "api/pty/pty_1/connect" {
            return Err(Error::Unreachable("The engine closed it.".into()));
        }
        let (shell, incoming) = async_channel::unbounded();
        // What it printed so far, then its note on where that got to.
        shell.send_blocking(SocketMessage::Text(b"\x1b[1;32m~/app\x1b[0m $ ".to_vec())).unwrap();
        shell.send_blocking(SocketMessage::Binary(b"\0{\"cursor\":21}".to_vec())).unwrap();
        *self.terminal.shell.lock().unwrap() = Some(shell);
        Ok(Socket { incoming, outgoing: Arc::new(Shell(self.terminal.clone())) })
    }

    fn streams(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        "devbox".into()
    }
}

struct World {
    relay: Relay,
    host: Host,
    served: Arc<Served>,
    ticks: Sender<String>,
    identity: Identity,
}

impl World {
    /// A relay, a box on it, and this device paired with the box.
    fn new() -> World {
        let relay = Relay::new();
        let (served, ticks) = Served::new();
        let host = Host::start(&relay, served.clone(), "devbox");
        let identity = Identity::new();
        host.pair(identity.public());
        World { relay, host, served, ticks, identity }
    }

    fn client(&self, timing: Timing) -> LinkClient {
        LinkClient::new(RELAY_URL, self.identity.clone(), "Kuvo on MacBook", "0.8.0", self.relay.dial(), timing)
    }

    /// Follows the box's ticks: as a followed route does, it says how things are now as it opens.
    fn follow(&self, linked: &LinkBox) -> Stream {
        self.ticks.send("{\"at\":0}\n".into()).unwrap();
        linked.open(&Request::get("ticks/events"), &|_| true).expect("followed")
    }

    fn linked(&self, timing: Timing) -> (LinkClient, LinkBox) {
        let client = self.client(timing);
        (client.clone(), LinkBox::new(client, self.host.facts(), "devbox"))
    }
}

/// Never waits to try the relay again, so a test says when it's tried.
fn at_once() -> Timing {
    Timing { retry_first: Duration::ZERO, retry_most: Duration::ZERO, ..Timing::default() }
}

#[test]
fn a_paired_device_asks_a_box_on_another_computer_and_says_its_a_desktop() {
    let world = World::new();
    let (_, linked) = world.linked(at_once());
    let asked = Request::post("echo?x=1", json!({ "words": "hello" })).header("idempotency-key", "msg_1");
    let answer = linked.send(&asked).expect("answered");
    let read: Value = serde_json::from_slice(&answer.body).unwrap();
    assert_eq!((answer.status, answer.content_type.as_deref()), (200, Some("application/json")));
    assert_eq!(read["method"], "POST");
    assert_eq!(read["path"], "echo?x=1");
    assert_eq!(read["body"], json!({ "words": "hello" }));
    assert_eq!(read["headers"], json!([["idempotency-key", "msg_1"], ["x-kuvo-route-version", "1"]]));
    // What it said as its session opened: a desktop, so the box doesn't take it for a phone.
    let hellos = world.host.hellos();
    assert_eq!(hellos.len(), 1, "one session, however many requests");
    assert_eq!((hellos[0]["client"].as_str(), hellos[0]["name"].as_str(), hellos[0].get("pair")), (Some("desktop"), Some("Kuvo on MacBook"), None));
    linked.send(&Request::get("echo")).expect("again, on the same session");
    assert_eq!(world.host.hellos().len(), 1);
    // The box that isn't on this computer is never taken for the one that is.
    assert!(!linked.here() && linked.streams());
}

#[test]
fn a_body_too_big_for_one_message_arrives_whole_both_ways() {
    let world = World::new();
    let (_, linked) = world.linked(at_once());
    let answer = linked.send(&Request::get("big")).expect("answered");
    let read: Value = serde_json::from_slice(&answer.body).expect("whole");
    assert_eq!(read["text"].as_str().map(str::len), Some(200_000));
    let long = "é".repeat(150_000);
    let answer = linked.send(&Request::post("echo", json!({ "text": long }))).expect("answered");
    let read: Value = serde_json::from_slice(&answer.body).expect("whole");
    assert_eq!(read["body"]["text"].as_str(), Some(long.as_str()));
}

#[test]
fn a_device_that_isnt_paired_is_turned_away_and_one_removed_loses_its_session() {
    let world = World::new();
    let stranger = LinkClient::new(RELAY_URL, Identity::new(), "Someone", "0.8.0", world.relay.dial(), at_once());
    let refused = LinkBox::new(stranger, world.host.facts(), "devbox").send(&Request::get("kuvo/info")).expect_err("turned away");
    assert!(is_unpaired(&refused), "{refused:?}");
    assert!(world.served.seen.lock().unwrap().is_empty(), "nothing a stranger asks reaches the box's routes");

    let (_, linked) = world.linked(at_once());
    linked.send(&Request::get("kuvo/info")).expect("paired: answered");
    world.host.remove(&world.identity.public());
    let after = linked.send(&Request::get("kuvo/info")).expect_err("removed");
    assert!(is_unpaired(&after), "{after:?}");
}

#[test]
fn a_followed_route_comes_line_by_line_and_closing_it_stops_it_on_the_box() {
    let world = World::new();
    let (_, linked) = world.linked(at_once());
    let stream = world.follow(&linked);
    assert_eq!(stream.status, 200);
    let mut reader = stream.reader;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line, "{\"at\":0}\n", "as it is now, as it opens");
    for tick in ["{\"at\":1}\n", "{\"at\":2}\n"] {
        world.ticks.send(tick.to_owned()).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, tick);
    }
    stream.closer.close();
    line.clear();
    assert_eq!(reader.read_line(&mut line).unwrap(), 0, "closed from here, it ends");
    // The box hears it's closed, on the same session: a request after it is answered in order.
    linked.send(&Request::get("echo")).unwrap();
    assert!(*world.served.closed.lock().unwrap(), "the box stopped following too");
}

#[test]
fn a_box_that_started_again_is_asked_again_and_its_gate_is_read_anew() {
    let world = World::new();
    let (client, linked) = world.linked(at_once());
    let host = world.host.facts();
    let boxed = BoxClient::new(linked);
    boxed.connect().expect("its info");
    assert_eq!(boxed.gate(), Gate::Open);
    world.ticks.send("{\"at\":0}\n".into()).unwrap();
    let stream = boxed.open(&Request::get("ticks/events"), &|_| true).expect("followed");
    assert_eq!(client.sessions_over(&host), 0);

    // It starts again, an older version's now: what was followed ends as a failure (so what
    // follows it opens it again), and the next read finds a new session and reads its gate anew.
    world.host.stop();
    *world.served.features.lock().unwrap() = Some(vec!["sessions.list".into()]);
    *world.served.compatibility.lock().unwrap() = Some(json!({ "state": "held", "missing": ["messages.send"] }));
    world.host.connect();
    let mut reader = stream.reader;
    let mut line = String::new();
    let failed = (0..2).any(|_| reader.read_line(&mut line).is_err());
    assert!(failed, "the followed route failed, it didn't just end");
    boxed.send(&Request::get("echo")).expect("still asked: the box answers");
    assert!(boxed.gate().is_held(), "held, as it says now");
    assert_eq!(client.sessions_over(&host), 1);
    assert!(world.served.seen.lock().unwrap().iter().any(|request| request.route() == "echo"));
}

#[test]
fn a_computer_thats_off_says_so_and_is_reached_again_once_its_back() {
    let world = World::new();
    let (_, linked) = world.linked(at_once());
    linked.send(&Request::get("kuvo/info")).expect("there");
    world.host.stop();
    let away = linked.send(&Request::get("kuvo/info")).expect_err("off");
    assert_eq!(away, Error::Unreachable(words::OFFLINE.into()));
    world.host.connect();
    linked.send(&Request::get("kuvo/info")).expect("back");
}

#[test]
fn with_the_relay_down_nothing_is_asked_and_its_not_tried_again_before_its_wait() {
    let world = World::new();
    world.relay.set_down(true);
    let patient = Timing { retry_first: Duration::from_secs(3600), retry_most: Duration::from_secs(3600), ..Timing::default() };
    let (_, linked) = world.linked(patient);
    let before = world.relay.dials();
    assert_eq!(linked.send(&Request::get("kuvo/info")), Err(Error::Unreachable(words::RELAY.into())));
    world.relay.set_down(false);
    // Asked again at once (a follow that retries, a view that opens): no second try before the wait.
    assert_eq!(linked.send(&Request::get("kuvo/info")), Err(Error::Unreachable(words::RELAY.into())));
    assert_eq!(world.relay.dials(), before, "the relay wasn't dialed again");

    // One that tries at once finds it back.
    let (_, eager) = world.linked(at_once());
    world.relay.set_down(true);
    assert!(eager.send(&Request::get("kuvo/info")).is_err());
    world.relay.set_down(false);
    eager.send(&Request::get("kuvo/info")).expect("back");
}

#[test]
fn the_relay_connection_dropping_fails_what_was_followed_and_the_next_read_connects_again() {
    let world = World::new();
    let (client, linked) = world.linked(at_once());
    let stream = world.follow(&linked);
    world.relay.drop_device(&client.address());
    let mut reader = stream.reader;
    let mut line = String::new();
    // (What had come before it dropped is still read; then it fails.)
    let failed = (0..2).any(|_| reader.read_line(&mut line).is_err());
    assert!(failed, "the followed route failed, it didn't just end");
    let dials = world.relay.dials();
    linked.send(&Request::get("kuvo/info")).expect("connected again");
    assert_eq!(world.relay.dials(), dials + 1);
}

#[test]
fn a_box_that_stops_answering_is_checked_on_and_then_said_not_to_answer() {
    let world = World::new();
    let quick = Timing { answer_within: Duration::from_millis(20), check: Duration::from_millis(20), ..at_once() };
    let (_, linked) = world.linked(quick);
    linked.send(&Request::get("kuvo/info")).expect("there");
    // Its connection died without closing: it takes frames and never answers them.
    world.host.set_quiet(true);
    assert_eq!(linked.send(&Request::post("echo", json!({}))), Err(Error::Unreachable(words::TIMED_OUT.into())));
}

#[test]
fn a_device_pairs_with_a_link_s_token_once() {
    let world = World::new();
    world.host.take_token("tok_once");
    let newcomer = Identity::new();
    let client = LinkClient::new(RELAY_URL, newcomer.clone(), "Laptop", "0.8.0", world.relay.dial(), at_once());
    let hello = client.pair(&world.host.facts(), "tok_once").expect("paired");
    assert_eq!(hello.name, "devbox");
    assert_eq!(world.host.hellos().last().unwrap()["pair"], "tok_once");
    // Paired now: let in without the token.
    client.let_go();
    LinkBox::new(client, world.host.facts(), "devbox").send(&Request::get("kuvo/info")).expect("paired");
    // The token's used up: another device showing it is turned away.
    let other = LinkClient::new(RELAY_URL, Identity::new(), "Other", "0.8.0", world.relay.dial(), at_once());
    assert!(is_unpaired(&other.pair(&world.host.facts(), "tok_once").expect_err("used up")));
}

#[test]
fn the_same_goes_through_a_real_websocket() {
    let world = World::new();
    let url = world.relay.listen().expect("listening on 127.0.0.1");
    // (The box is at the address its own relay URL gives it; the device's is by the URL it dials.)
    let client = LinkClient::new(url, world.identity.clone(), "Kuvo on MacBook", "0.8.0", Arc::new(WebSocketDial), at_once());
    let linked = LinkBox::new(client, world.host.facts(), "devbox");
    let answer = linked.send(&Request::get("big")).expect("answered over the socket");
    assert_eq!(serde_json::from_slice::<Value>(&answer.body).unwrap()["text"].as_str().map(str::len), Some(200_000));
    let stream = world.follow(&linked);
    let mut reader = stream.reader;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line, "{\"at\":0}\n");
}

#[test]
fn a_message_whose_answer_a_dropped_connection_lost_is_sent_again_once_under_its_id_and_answered() {
    let world = World::new();
    let (client, linked) = world.linked(at_once());
    linked.send(&Request::get("kuvo/info")).expect("there");
    // The box takes it, and the connection drops before its answer comes back.
    *world.served.drops.lock().unwrap() = Some((world.relay.clone(), client.address()));
    let message = Request::post("send", json!({ "id": "msg_1", "words": "hi", "files": [{ "uri": "data:image/png;base64,AAAA" }] })).header("idempotency-key", "msg_1");
    let answer = linked.send(&message).expect("answered: not said to have failed");
    assert_eq!(answer.status, 200);
    let sends: Vec<Request> = world.served.seen.lock().unwrap().iter().filter(|request| request.route() == "send").cloned().collect();
    assert_eq!(sends.len(), 2, "sent again, once, under the same id (its box takes it once)");
    assert_eq!(sends[0], sends[1]);

    // What has no id of its own isn't sent twice: it says it couldn't connect.
    *world.served.drops.lock().unwrap() = Some((world.relay.clone(), client.address()));
    let plain = linked.send(&Request::post("send", json!({ "words": "hi" })));
    assert_eq!(plain, Err(Error::Unreachable(words::RELAY.into())));
    assert_eq!(world.served.seen.lock().unwrap().iter().filter(|request| request.route() == "send").count(), 3);
}

/// The stand-in relay's connection, with what the test says of whether what was handed to it is
/// still going out, and how often the link asked.
struct Slow {
    relay: Relay,
    busy: Arc<std::sync::atomic::AtomicBool>,
    asked: Arc<std::sync::atomic::AtomicUsize>,
}

struct SlowOut {
    out: Box<dyn kuvo_box::link::relay::Outgoing>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    asked: Arc<std::sync::atomic::AtomicUsize>,
}

impl kuvo_box::link::relay::Outgoing for SlowOut {
    fn send(&mut self, message: Vec<u8>) -> std::io::Result<()> {
        self.out.send(message)
    }
    fn busy(&self) -> bool {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl kuvo_box::link::relay::Dial for Slow {
    fn dial(&self, url: &str) -> std::io::Result<(Box<dyn kuvo_box::link::relay::Outgoing>, Box<dyn kuvo_box::link::relay::Incoming>)> {
        let (out, incoming) = self.relay.dial().dial(url)?;
        Ok((Box::new(SlowOut { out, busy: self.busy.clone(), asked: self.asked.clone() }), incoming))
    }
}

#[test]
fn a_request_still_going_out_on_a_slow_connection_isnt_given_up_on_until_its_gone() {
    use std::sync::atomic::Ordering;
    let world = World::new();
    let slow = Slow { relay: world.relay.clone(), busy: Arc::new(false.into()), asked: Arc::new(0.into()) };
    let (busy, asked) = (slow.busy.clone(), slow.asked.clone());
    let quick = Timing { answer_within: Duration::from_millis(5), check: Duration::from_millis(5), ..at_once() };
    let client = LinkClient::new(RELAY_URL, world.identity.clone(), "Kuvo on MacBook", "0.8.0", Arc::new(slow), quick);
    let linked = LinkBox::new(client, world.host.facts(), "devbox");
    linked.send(&Request::get("kuvo/info")).expect("there");
    // A picture going out: nothing comes back meanwhile (the box hasn't had it whole yet).
    world.host.set_quiet(true);
    busy.store(true, Ordering::SeqCst);
    let (done, finished) = channel();
    std::thread::spawn(move || done.send(linked.send(&Request::post("echo", json!({ "text": "x" })))).unwrap());
    // Checked many times over (each check's own wait many times over), never given up on.
    let before = asked.load(Ordering::SeqCst);
    while asked.load(Ordering::SeqCst) < before + 20 {
        std::thread::yield_now();
    }
    assert!(finished.try_recv().is_err(), "still waiting while it goes out");
    // Gone, and still nothing back: then it's said not to answer.
    busy.store(false, Ordering::SeqCst);
    assert_eq!(finished.recv().unwrap(), Err(Error::Unreachable(words::TIMED_OUT.into())));
}

#[test]
fn a_message_sent_while_the_network_is_down_goes_once_the_relay_is_back_and_its_box_takes_it_once() {
    // A Wi-Fi drop of more than a moment (#144, the desktop app's "Not sent. Couldn't connect"): the
    // message waits, and goes as soon as a relay connection is made, within its two minutes.
    let world = World::new();
    // (Only the relay coming back sends it again here, never a wait running out.)
    let patient = Timing { repeat_first: Duration::from_secs(3600), repeat_most: Duration::from_secs(3600), repeat_for: Duration::from_secs(3600), ..at_once() };
    let (client, linked) = world.linked(patient);
    let host = world.host.facts();
    linked.send(&Request::get("kuvo/info")).expect("there");
    world.relay.set_down(true);
    world.relay.drop_device(&client.address());
    // (Once this device has noticed: nothing goes on the connection that was.)
    while client.is_open(&host) {
        std::thread::yield_now();
    }
    let message = Request::post("send", json!({ "id": "msg_2", "words": "hi" })).header("idempotency-key", "msg_2");
    let (done, finished) = channel();
    let sender = std::thread::spawn(move || done.send(linked.send(&message)).unwrap());
    // It tried, and at once again, and waits.
    while world.relay.turned_away() < 2 {
        std::thread::yield_now();
    }
    assert!(finished.try_recv().is_err(), "not said to have failed while it waits");
    // The network's back: the next connection made (a follow coming back, a view reading) sends it.
    world.relay.set_down(false);
    let reader = LinkBox::new(client.clone(), host.clone(), "devbox");
    let read = std::thread::spawn(move || reader.send(&Request::get("kuvo/info")));
    assert_eq!(finished.recv().unwrap().map(|answer| answer.status), Ok(200));
    sender.join().unwrap();
    let _ = read.join().unwrap();
    assert_eq!(world.served.seen.lock().unwrap().iter().filter(|request| request.route() == "send").count(), 1, "it reached the box once");
}

#[test]
fn a_message_that_cant_go_says_so_once_its_two_minutes_are_up_or_stop_takes_it_back() {
    let world = World::new();
    let host = world.host.facts();
    // Its two minutes up (here at once): it says it couldn't connect, as it did before.
    let (client, linked) = world.linked(Timing { repeat_for: Duration::ZERO, ..at_once() });
    linked.send(&Request::get("kuvo/info")).expect("there");
    world.relay.set_down(true);
    world.relay.drop_device(&client.address());
    while client.is_open(&host) {
        std::thread::yield_now();
    }
    let message = |id: &str| Request::post("send", json!({ "id": id })).header("idempotency-key", id);
    assert_eq!(linked.send(&message("msg_4")).map(|answer| answer.status), Err(Error::Unreachable(words::RELAY.into())));
    assert_eq!(world.relay.turned_away(), 1, "tried once, not again past its time");

    // Stop takes back one that never went: it says so, and never reaches the box after it.
    let (client, linked) = world.linked(Timing { repeat_first: Duration::from_secs(3600), repeat_most: Duration::from_secs(3600), repeat_for: Duration::from_secs(3600), ..at_once() });
    let turned_away = world.relay.turned_away();
    let (done, finished) = channel();
    let stopping = LinkBox::new(client.clone(), host.clone(), "devbox");
    let sender = std::thread::spawn(move || done.send(linked.send(&message("msg_5"))).unwrap());
    while world.relay.turned_away() < turned_away + 2 {
        std::thread::yield_now();
    }
    stopping.take_back(&["msg_5".to_owned()]);
    assert_eq!(finished.recv().unwrap().map(|answer| answer.status), Err(Error::Unreachable(kuvo_box::link::linked::STOPPED.into())));
    sender.join().unwrap();
    world.relay.set_down(false);
    LinkBox::new(client, host, "devbox").send(&Request::get("kuvo/info")).expect("back");
    assert!(world.served.seen.lock().unwrap().iter().all(|request| request.route() != "send"), "neither reached the box");
}

#[test]
fn a_message_to_a_box_that_turns_this_device_away_or_isnt_on_the_relay_isnt_sent_again() {
    let world = World::new();
    let (client, linked) = world.linked(Timing { repeat_for: Duration::from_secs(3600), ..at_once() });
    let host = world.host.facts();
    linked.send(&Request::get("kuvo/info")).expect("there");
    // Removed there: turned away at once, never sent again.
    world.host.remove(&world.identity.public());
    let refused = linked.send(&Request::post("send", json!({})).header("idempotency-key", "msg_6")).expect_err("turned away");
    assert!(is_unpaired(&refused), "{refused:?}");
    // A computer that isn't on the relay, which it never went to: said at once, as before.
    world.host.pair(world.identity.public());
    world.host.stop();
    while client.is_open(&host) {
        std::thread::yield_now();
    }
    let away = linked.send(&Request::post("send", json!({})).header("idempotency-key", "msg_7"));
    assert_eq!(away.map(|answer| answer.status), Err(Error::Unreachable(words::OFFLINE.into())));
    assert!(world.served.seen.lock().unwrap().iter().all(|request| request.route() != "send"));
}

#[test]
fn a_message_sent_just_after_the_connection_dropped_waits_for_its_box_to_be_read_again_too() {
    // The connection drops, so the box may have started again (another version's): it's read again
    // before it's asked anything. With the network still down, that read waits as the message does,
    // rather than failing it with "Couldn't connect" before it was ever tried.
    let world = World::new();
    let patient = Timing { repeat_first: Duration::from_secs(3600), repeat_most: Duration::from_secs(3600), repeat_for: Duration::from_secs(3600), ..at_once() };
    let (client, linked) = world.linked(patient);
    let host = world.host.facts();
    let boxed = BoxClient::new(linked);
    boxed.connect().expect("its info");
    world.relay.set_down(true);
    world.relay.drop_device(&client.address());
    while client.is_open(&host) {
        std::thread::yield_now();
    }
    let (done, finished) = channel();
    let sender = std::thread::spawn(move || done.send(boxed.send(&Request::post("send", json!({ "id": "msg_8" })).header("idempotency-key", "msg_8"))).unwrap());
    while world.relay.turned_away() < 2 {
        std::thread::yield_now();
    }
    world.relay.set_down(false);
    let reader = LinkBox::new(client, host, "devbox");
    let read = std::thread::spawn(move || reader.send(&Request::get("echo")));
    assert_eq!(finished.recv().unwrap().map(|answer| answer.status), Ok(200));
    sender.join().unwrap();
    let _ = read.join().unwrap();
    let seen = world.served.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|request| request.route() == "send").count(), 1);
    assert_eq!(seen.iter().filter(|request| request.route() == "kuvo/info").count(), 2, "read again before it went");
}

#[test]
fn a_terminal_on_another_computer_carries_its_output_and_your_keys_over_the_relay() {
    use kuvo_box::pty::{PtyError, PtyOutput};
    let world = World::new();
    let (_, linked) = world.linked(at_once());
    let client = BoxClient::new(linked);
    let terminal = client.connect_pty("pty_1", Some("/home/me/app")).expect("its socket opened");
    let asked = world.served.terminal.asked.lock().unwrap()[0].clone();
    assert_eq!((asked.method.as_str(), asked.path.as_str()), ("GET", "api/pty/pty_1/connect"));
    assert!(asked.headers.contains(&("x-opencode-directory".into(), "%2Fhome%2Fme%2Fapp".into())), "in its folder: {:?}", asked.headers);
    // What it printed so far, as it printed it; its note on where that got to is kept, not shown.
    assert_eq!(terminal.next_blocking(), Some(PtyOutput::Bytes(b"\x1b[1;32m~/app\x1b[0m $ ".to_vec())));
    terminal.write("ls -la\r").unwrap();
    assert_eq!(terminal.next_blocking(), Some(PtyOutput::Bytes(b"ls -la\r".to_vec())));
    assert_eq!(terminal.cursor(), 21);
    assert_eq!(world.served.terminal.typed.lock().unwrap().as_slice(), ["ls -la\r"]);
    // Closed here: the box closes the engine's socket.
    let (told, closed) = channel();
    *world.served.terminal.closed.lock().unwrap() = Some(told);
    terminal.close();
    closed.recv().expect("the box was told");

    // One that won't open says why, as its box said it.
    let refused = client.connect_pty("pty_gone", None).err().expect("refused");
    assert_eq!(refused, PtyError::Failed("The engine closed it.".into()));
}

#[test]
fn a_terminal_whose_shell_exits_says_so_and_nothing_comes_after() {
    use kuvo_box::pty::PtyOutput;
    let world = World::new();
    let (_, linked) = world.linked(at_once());
    let terminal = BoxClient::new(linked).connect_pty("pty_1", None).expect("its socket opened");
    assert!(matches!(terminal.next_blocking(), Some(PtyOutput::Bytes(_))));
    // The shell exits: the engine closes its socket.
    let shell = world.served.terminal.shell.lock().unwrap().take().expect("open");
    shell.send_blocking(SocketMessage::Ended(None)).unwrap();
    assert_eq!(terminal.next_blocking(), Some(PtyOutput::Ended(None)));
    assert_eq!(terminal.next_blocking(), None);
    assert!(terminal.write("too late").is_err());
}
