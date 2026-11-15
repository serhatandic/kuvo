//! Your boxes as one list (`kuvo_box::boxes`): a stand-in for the box on this computer, and
//! stand-in boxes of your other computers on a stand-in relay. What's asserted is what a client
//! shows and may do: which computers it reaches, how each stands, and how it pairs.

use std::io::Read;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kuvo_box::boxes::{Boxes, Options, Pairing, Reach, Timing};
use kuvo_box::link::client::words;
use kuvo_box::link::stand_in::{Host, RELAY_URL, Relay};
use kuvo_box::link::{Keeper, MemoryKeeper, keys};
use kuvo_box::transport::{Closer, Request, Response, Stream, Transport};
use kuvo_box::{Error, gate};
use serde_json::{Value, json};

/// What a stand-in box does when it's asked to pair a device: its status and answer.
type Pairs = Box<dyn Fn(&Value) -> (u16, Value) + Send>;

/// A stand-in box: `kuvo/info`, a list of computers, and whatever else a test gives it to answer.
#[derive(Default)]
struct StandInBox {
    name: String,
    here: bool,
    features: Mutex<Option<Vec<String>>>,
    computers: Mutex<Value>,
    seen: Mutex<Vec<Request>>,
    /// What `POST kuvo/devices/pair` does with the key it's shown, and answers.
    pair: Mutex<Option<Pairs>>,
    /// The followed list's lines, written by the test.
    lines: Mutex<Option<Receiver<String>>>,
}

struct Lines(Receiver<String>, Vec<u8>);

impl Read for Lines {
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

impl Transport for StandInBox {
    fn send(&self, request: &Request) -> kuvo_box::Result<Response> {
        self.seen.lock().unwrap().push(request.clone());
        let json = |status: u16, value: Value| Ok(Response { status, content_type: Some("application/json".into()), body: value.to_string().into_bytes() });
        match (request.method.as_str(), request.route()) {
            ("GET", "kuvo/info") => {
                let features = self.features.lock().unwrap().clone().unwrap_or_else(|| gate::REQUIRED.iter().map(|name| (*name).to_owned()).collect());
                json(200, json!({ "kind": "machine", "name": self.name, "features": features }))
            }
            ("GET", "kuvo/computers") => json(200, self.computers.lock().unwrap().clone()),
            ("POST", "kuvo/devices/pair") => match &*self.pair.lock().unwrap() {
                Some(pair) => {
                    let (status, value) = pair(request.body.as_ref().unwrap_or(&Value::Null));
                    json(status, value)
                }
                None => json(404, json!({})),
            },
            ("GET", "whoami") => json(200, json!({ "name": self.name })),
            _ => json(404, json!({})),
        }
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> kuvo_box::Result<Stream> {
        self.seen.lock().unwrap().push(request.clone());
        hold(Closer::nothing());
        let lines = self.lines.lock().unwrap().take().expect("followed once");
        Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(std::io::BufReader::new(Lines(lines, Vec::new()))), closer: Closer::nothing() })
    }

    fn streams(&self) -> bool {
        true
    }

    fn here(&self) -> bool {
        self.here
    }

    fn describe(&self) -> String {
        self.name.clone()
    }
}

/// A setup: this computer (a MacBook, a member), the main computer (a Mac mini) and a Linux
/// machine, the last two on the relay; and a machine shared with another setup, filed apart.
struct Setup {
    relay: Relay,
    macbook: Arc<StandInBox>,
    mini: (Arc<StandInBox>, Host),
    devbox: (Arc<StandInBox>, Host),
    shared: (Arc<StandInBox>, Host),
    keeper: Arc<MemoryKeeper>,
    kept: tempfile::TempDir,
    lines: Sender<String>,
}

const MACBOOK_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

impl Setup {
    fn new() -> Setup {
        let relay = Relay::new();
        let remote = |name: &str| {
            let served = Arc::new(StandInBox { name: name.into(), ..StandInBox::default() });
            let host = Host::start(&relay, served.clone(), name);
            (served, host)
        };
        let (mini, devbox, shared) = (remote("Mac mini"), remote("devbox"), remote("shared-box"));
        let (lines, receiver) = channel();
        let macbook = Arc::new(StandInBox { name: "MacBook".into(), here: true, lines: Mutex::new(Some(receiver)), ..StandInBox::default() });
        let setup = Setup { relay, macbook, mini, devbox, shared, keeper: Arc::new(MemoryKeeper::default()), kept: tempfile::tempdir().unwrap(), lines };
        *setup.macbook.computers.lock().unwrap() = setup.list();
        // The nearest box vouches for the app to the main computer, and every box of the setup
        // takes the device from the main computer's phones document. Never the shared machine.
        let (mini, devbox) = (setup.mini.1.clone(), setup.devbox.1.clone());
        setup.pairs(move |asked| {
            let key: [u8; 32] = keys::decode_exact(asked["key"].as_str().unwrap()).expect("a key");
            mini.pair(key);
            devbox.pair(key);
            (200, json!({ "paired": true, "pairing": "pair_1" }))
        });
        setup
    }

    fn pairs(&self, pair: impl Fn(&Value) -> (u16, Value) + Send + 'static) {
        *self.macbook.pair.lock().unwrap() = Some(Box::new(pair));
    }

    /// `kuvo/computers` as the MacBook's box says it.
    fn list(&self) -> Value {
        let row = |name: &str, kind: &str, host: &Host| json!({ "key": host.key_text(), "name": name, "kind": kind, "reach": "connected", "via": "relay", "address": host.address_text(), "relay": RELAY_URL });
        let mut shared = row("shared-box", "machine", &self.shared.1);
        shared["apart"] = json!(true);
        shared["shared"] = json!(true);
        json!({ "computers": [
            { "key": MACBOOK_KEY, "name": "MacBook", "kind": "mac", "here": true, "reach": "connected", "via": "here", "address": "AAECAwQFBgcICQoLDA0ODw", "relay": RELAY_URL },
            row("Mac mini", "mac", &self.mini.1),
            row("devbox", "machine", &self.devbox.1),
            shared,
        ] })
    }

    fn timing() -> Timing {
        let mut timing = Timing { away_after: Duration::ZERO, pair_again: Duration::ZERO, look_again: Duration::ZERO, ..Timing::default() };
        timing.link.retry_first = Duration::ZERO;
        timing.link.retry_most = Duration::ZERO;
        timing
    }

    fn boxes_with(&self, nearest: bool, timing: Timing) -> Boxes {
        Boxes::new(Options {
            nearest: nearest.then(|| self.macbook.clone() as Arc<dyn Transport>),
            keeper: self.keeper.clone(),
            dial: self.relay.dial(),
            name: "MacBook".into(),
            app: "0.8.0".into(),
            kept: Some(self.kept.path().join("desktop/boxes.json")),
            timing,
        })
    }

    fn boxes(&self) -> Boxes {
        self.boxes_with(true, Setup::timing())
    }

    fn pair_asks(&self) -> Vec<Value> {
        self.macbook.seen.lock().unwrap().iter().filter(|request| request.route() == "kuvo/devices/pair").filter_map(|request| request.body.clone()).collect()
    }
}

fn reach(boxes: &Boxes) -> Vec<(String, Reach)> {
    boxes.rows().into_iter().map(|row| (row.computer.name.clone(), row.reach)).collect()
}

fn named(boxes: &Boxes, name: &str) -> String {
    boxes.rows().into_iter().find(|row| row.computer.name == name).expect("listed").computer.key
}

fn whoami(boxes: &Boxes, name: &str) -> kuvo_box::Result<Value> {
    boxes.client(&named(boxes, name)).expect("a client").get("whoami")
}

#[test]
fn from_a_member_computer_the_app_pairs_through_its_own_box_and_reaches_the_others() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    // Before it's paired, nothing's asked of another computer.
    assert_eq!(reach(&boxes), [("MacBook".into(), Reach::Nearest), ("Mac mini".into(), Reach::Connecting), ("devbox".into(), Reach::Connecting), ("shared-box".into(), Reach::Apart)]);
    assert!(setup.mini.1.hellos().is_empty());

    boxes.settle();
    assert_eq!(boxes.pairing(), Pairing::Paired);
    assert_eq!(reach(&boxes), [("MacBook".into(), Reach::Nearest), ("Mac mini".into(), Reach::Connected), ("devbox".into(), Reach::Connected), ("shared-box".into(), Reach::Apart)]);
    // It asked its own box, once, with its public key, as a desktop.
    let asks = setup.pair_asks();
    assert_eq!(asks.len(), 1);
    assert_eq!((asks[0]["kind"].as_str(), asks[0]["name"].as_str()), (Some("desktop"), Some("MacBook")));
    assert_eq!(keys::decode(asks[0]["key"].as_str().unwrap()).map(|key| key.len()), Some(32));

    // A session on the main computer is asked of the main computer's own box, directly.
    assert_eq!(whoami(&boxes, "Mac mini").unwrap()["name"], "Mac mini");
    assert_eq!(whoami(&boxes, "devbox").unwrap()["name"], "devbox");
    assert_eq!(setup.mini.1.hellos()[0]["client"], "desktop");
    // This computer's own box is the nearest: asked locally, though the list says where it is on
    // the relay too.
    assert_eq!(whoami(&boxes, "MacBook").unwrap()["name"], "MacBook");
    assert!(boxes.client(MACBOOK_KEY).unwrap().here());
    // One box's client finds another's, as a settings picker does.
    let mini = boxes.rows().into_iter().find(|row| row.computer.name == "Mac mini").unwrap().computer;
    assert_eq!(boxes.nearest().unwrap().computer(&mini).unwrap().get::<Value>("whoami").unwrap()["name"], "Mac mini");
}

#[test]
fn a_computer_filed_apart_is_never_connected_to() {
    let setup = Setup::new();
    // Even were its box to take this device (it must not), the client never opens a session there.
    setup.shared.1.pair([1; 32]);
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    boxes.settle();
    assert!(boxes.client(&setup.shared.1.key_text()).is_none());
    assert!(setup.shared.1.hellos().is_empty(), "no session was ever opened with the shared machine");
    assert!(setup.shared.0.seen.lock().unwrap().is_empty());
    assert_eq!(reach(&boxes).last().unwrap(), &("shared-box".to_owned(), Reach::Apart));
}

#[test]
fn a_device_thats_removed_stays_out_until_you_pair_it_again_yourself() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let first = setup.pair_asks()[0]["key"].as_str().unwrap().to_owned();
    assert_eq!(whoami(&boxes, "Mac mini").unwrap()["name"], "Mac mini");

    // Removed from another device: every computer turns it away, and its own box won't vouch
    // for that key again.
    let key: [u8; 32] = keys::decode_exact(&first).unwrap();
    setup.mini.1.remove(&key);
    setup.devbox.1.remove(&key);
    setup.pairs(|_| (409, json!({ "message": "This app was removed from your devices.", "reason": "removed" })));
    assert!(kuvo_box::link::is_unpaired(&whoami(&boxes, "Mac mini").unwrap_err()));
    boxes.settle();
    boxes.settle();
    assert_eq!(boxes.pairing(), Pairing::Removed("This app was removed from your devices.".into()));
    assert_eq!(reach(&boxes)[1], ("Mac mini".to_owned(), Reach::NotPaired));
    // It never makes itself a new key to get back in.
    assert!(setup.pair_asks().iter().all(|ask| ask["key"] == first.as_str()));
    boxes.settle();
    assert!(setup.pair_asks().len() <= 2, "it doesn't keep asking once it's told it was removed");

    // On your word: a new key, paired anew.
    let (mini, devbox) = (setup.mini.1.clone(), setup.devbox.1.clone());
    setup.pairs(move |asked| {
        let key: [u8; 32] = keys::decode_exact(asked["key"].as_str().unwrap()).unwrap();
        mini.pair(key);
        devbox.pair(key);
        (200, json!({ "paired": true, "pairing": "pair_2" }))
    });
    assert_eq!(boxes.pair_again(), Pairing::Paired);
    boxes.settle();
    assert_ne!(setup.pair_asks().last().unwrap()["key"], first.as_str());
    assert_eq!(whoami(&boxes, "Mac mini").unwrap()["name"], "Mac mini");
}

#[test]
fn while_the_main_computer_isnt_answering_the_app_waits_and_says_so_in_the_boxs_words() {
    let setup = Setup::new();
    setup.pairs(|_| (503, json!({ "message": "Mac mini isn’t answering. This app reaches your other computers once it does." })));
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    assert_eq!(boxes.pairing(), Pairing::Waiting("Mac mini isn’t answering. This app reaches your other computers once it does.".into()));
    assert!(setup.mini.1.hellos().is_empty() && setup.devbox.1.hellos().is_empty(), "nothing's asked over the link before it's paired");

    // A box that refuses says why (the switch is off there).
    setup.pairs(|_| (409, json!({ "message": "“Let my devices connect” is off on Mac mini." })));
    boxes.settle();
    assert_eq!(boxes.pairing(), Pairing::Refused("“Let my devices connect” is off on Mac mini.".into()));

    // One from before this says it needs an update, never a raw 404.
    *setup.macbook.pair.lock().unwrap() = None;
    boxes.settle();
    assert!(matches!(boxes.pairing(), Pairing::Refused(words) if words.contains("needs an update")));
}

#[test]
fn a_computer_thats_off_is_away_and_back_once_it_answers_again() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    setup.devbox.1.stop();
    assert_eq!(whoami(&boxes, "devbox"), Err(Error::Unreachable(words::OFFLINE.into())));
    boxes.settle();
    assert_eq!(reach(&boxes)[2], ("devbox".to_owned(), Reach::Away(words::OFFLINE.into())));
    // The others are as they were.
    assert_eq!(reach(&boxes)[1], ("Mac mini".to_owned(), Reach::Connected));
    setup.devbox.1.connect();
    boxes.settle();
    assert_eq!(reach(&boxes)[2], ("devbox".to_owned(), Reach::Connected));
}

#[test]
fn a_computer_that_just_stopped_answering_isnt_called_away_at_once() {
    let setup = Setup::new();
    let mut patient = Setup::timing();
    patient.away_after = Duration::from_secs(3600);
    let boxes = setup.boxes_with(true, patient);
    boxes.read_list().unwrap();
    boxes.settle();
    setup.devbox.1.stop();
    let _ = whoami(&boxes, "devbox");
    boxes.settle();
    assert_eq!(reach(&boxes)[2], ("devbox".to_owned(), Reach::Connecting));
}

#[test]
fn an_older_box_on_another_computer_is_held_and_sent_nothing_but_what_its_allowed() {
    let setup = Setup::new();
    *setup.devbox.0.features.lock().unwrap() = Some(vec!["sessions.list".into()]);
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    assert!(matches!(&reach(&boxes)[2].1, Reach::Held { missing } if !missing.is_empty()));
    assert!(matches!(whoami(&boxes, "devbox"), Err(Error::NeedsUpdating { .. })));
    assert!(setup.devbox.0.seen.lock().unwrap().iter().all(|request| request.route() == "kuvo/info"));
    // Updated, it's let go at the next look.
    *setup.devbox.0.features.lock().unwrap() = None;
    let mut soon = Setup::timing();
    soon.follow.held_again = Duration::ZERO;
    let again = setup.boxes_with(true, soon);
    again.read_list().unwrap();
    again.settle();
    assert_eq!(reach(&again)[2], ("devbox".to_owned(), Reach::Connected));
}

#[test]
fn the_kept_list_opens_the_app_with_your_computers_and_holds_nothing_secret() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let path = setup.kept.path().join("desktop/boxes.json");
    let kept = std::fs::read_to_string(&path).expect("kept");
    // Public facts only: never this device's key or secret, nor a computer that isn't yours.
    let secret = setup.keeper.read().expect("its key is in the keeper");
    for part in secret.split('.') {
        assert!(!kept.contains(part));
    }
    assert!(kept.contains("Mac mini") && !kept.contains("shared-box"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    // With no box on this computer answering, the app still opens with your computers, reaches
    // them directly, and reads the list from one of them.
    *setup.mini.0.computers.lock().unwrap() = setup.list();
    let alone = setup.boxes_with(false, Setup::timing());
    assert_eq!(reach(&alone).iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["MacBook", "Mac mini", "devbox"]);
    alone.settle();
    assert_eq!(whoami(&alone, "Mac mini").unwrap()["name"], "Mac mini");
    assert!(setup.pair_asks().len() == 1, "it doesn't pair again: it's the same device");
}

#[test]
fn started_it_follows_the_list_and_reaches_a_computer_added_later() {
    let setup = Setup::new();
    // At first only this computer; then the main computer joins.
    let only = json!({ "computers": [setup.list()["computers"][0].clone()] });
    setup.lines.send(format!("{only}\n")).unwrap();
    let boxes = setup.boxes();
    let changed = boxes.changed();
    boxes.start();
    while reach(&boxes) != [("MacBook".to_owned(), Reach::Nearest)] {
        changed.recv_blocking().unwrap();
    }
    assert_eq!(boxes.pairing(), Pairing::NotAsked, "alone, there's nothing to pair for");
    assert!(setup.pair_asks().is_empty());
    setup.lines.send(format!("{}\n", setup.list())).unwrap();
    while reach(&boxes).get(1) != Some(&("Mac mini".to_owned(), Reach::Connected)) {
        changed.recv_blocking().unwrap();
    }
    assert_eq!(whoami(&boxes, "Mac mini").unwrap()["name"], "Mac mini");
    boxes.stop();
}
