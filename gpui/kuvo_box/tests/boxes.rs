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
use kuvo_box::link::{Keeper, Kept, MemoryKeeper, keys};
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
                let features = self.features.lock().unwrap().clone().unwrap_or_else(|| gate::REQUIRED.iter().copied().chain(["update.fleet"]).map(str::to_owned).collect());
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
/// machine, the last two on the relay.
struct Setup {
    relay: Relay,
    macbook: Arc<StandInBox>,
    mini: (Arc<StandInBox>, Host),
    devbox: (Arc<StandInBox>, Host),
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
        let (mini, devbox) = (remote("Mac mini"), remote("devbox"));
        let (lines, receiver) = channel();
        let macbook = Arc::new(StandInBox { name: "MacBook".into(), here: true, lines: Mutex::new(Some(receiver)), ..StandInBox::default() });
        let setup = Setup { relay, macbook, mini, devbox, keeper: Arc::new(MemoryKeeper::default()), kept: tempfile::tempdir().unwrap(), lines };
        *setup.macbook.computers.lock().unwrap() = setup.list();
        // The nearest box vouches for the app to the main computer, and every box of the setup
        // takes the device from the main computer's phones document.
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
        json!({ "computers": [
            { "key": MACBOOK_KEY, "name": "MacBook", "kind": "mac", "here": true, "reach": "connected", "via": "here", "address": "AAECAwQFBgcICQoLDA0ODw", "relay": RELAY_URL },
            row("Mac mini", "mac", &self.mini.1),
            row("devbox", "machine", &self.devbox.1),
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
    assert_eq!(reach(&boxes), [("MacBook".into(), Reach::Nearest), ("Mac mini".into(), Reach::Connecting), ("devbox".into(), Reach::Connecting)]);
    assert!(setup.mini.1.hellos().is_empty());

    boxes.settle();
    assert_eq!(boxes.pairing(), Pairing::Paired);
    assert_eq!(reach(&boxes), [("MacBook".into(), Reach::Nearest), ("Mac mini".into(), Reach::Connected), ("devbox".into(), Reach::Connected)]);
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
fn a_computer_confirming_its_list_shows_waiting_for_an_already_paired_app() {
    let setup = Setup::new();
    let waiting = "Checking this app with your other computers…";
    setup.mini.1.set_pairing_wait(Some(waiting));
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let host = setup.mini.1.key_text();
    assert_eq!(boxes.check(&host), Some(Reach::Waiting(waiting.into())));
    assert_eq!(boxes.pairing(), Pairing::Paired);
    setup.mini.1.set_pairing_wait(None);
    assert_eq!(boxes.check(&host), Some(Reach::Connected));
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
fn a_cached_pairing_checks_again_but_a_readd_requires_the_users_action() {
    let setup = Setup::new();
    let checking = "Checking this app with your other computers…";
    setup.pairs(move |_| (503, json!({"reason":"pairing-wait", "message":checking})));
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    assert_eq!(boxes.pairing(), Pairing::Checking(checking.into()));
    let first_key = setup.pair_asks()[0]["key"].clone();

    setup.pairs(|_| (409, json!({"reason":"pairing-readd", "title":"This app needs to be paired again",
        "message":"The computer that added this app was removed. Pair this app again from one of your computers.",
        "action":{"label":"Pair Again…", "route":"kuvo/devices/pair"}})));
    boxes.settle();
    let Pairing::Readd(refusal) = boxes.pairing() else { panic!("the box asked for Pair Again") };
    assert_eq!(refusal.title.as_deref(), Some("This app needs to be paired again"));
    assert_eq!(refusal.pair_again_label(), Some("Pair Again…"));
    assert_eq!(setup.pair_asks().len(), 2);
    boxes.settle();
    assert_eq!(setup.pair_asks().len(), 2);

    setup.pairs(|_| (200, json!({"paired":true})));
    assert_eq!(boxes.pair_again(), Pairing::Paired);
    assert_ne!(setup.pair_asks().last().unwrap()["key"], first_key);
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
fn a_beta35_member_refuses_new_desktop_work_until_the_accepted_pack_is_reported() {
    let setup = Setup::new();
    let beta35 = gate::REQUIRED.iter().copied().map(str::to_owned).collect();
    *setup.devbox.0.features.lock().unwrap() = Some(beta35);
    let mut list = setup.list();
    list["computers"][2]["fleetUpdate"] = json!({
        "current": "0.8.0-beta.35", "target": "0.8.0-beta.36", "state": "behind", "canRetry": false, "requiresUpdate": true
    });
    *setup.macbook.computers.lock().unwrap() = list.clone();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let row = boxes.rows().into_iter().find(|row| row.key() == setup.devbox.1.key_text()).unwrap();
    let member = boxes.nearest().unwrap().computer(&row.computer).unwrap();
    let write = Request::post("api/session/ses_live/message", json!({}));
    assert!(matches!(member.send(&write), Err(Error::Refused(refusal))
        if refusal.status == 503 && refusal.reason.as_deref() == Some("computer-update-required")));
    assert!(!setup.devbox.0.seen.lock().unwrap().iter().any(|request| request.path == write.path));
    let approval = Request::post("api/session/ses_live/permission/ask_one", json!({}));
    assert!(member.send(&approval).is_ok());
    assert!(setup.devbox.0.seen.lock().unwrap().iter().any(|request| request.path == approval.path));

    let stop = Request::post("kuvo/sessions/ses_live/stop", json!({}));
    assert!(member.send(&stop).is_ok());
    assert!(setup.devbox.0.seen.lock().unwrap().iter().any(|request| request.path == stop.path));

    list["computers"][2]["fleetUpdate"]["current"] = json!("0.8.0-beta.37");
    list["computers"][2]["fleetUpdate"]["requiresUpdate"] = json!(false);
    *setup.macbook.computers.lock().unwrap() = list;
    boxes.read_list().unwrap();
    assert!(member.send(&write).is_ok());
    assert!(setup.devbox.0.seen.lock().unwrap().iter().any(|request| request.path == write.path));
}

#[test]
fn an_old_nearest_box_bootstraps_the_list_then_uses_the_updated_peers_fleet_state() {
    let setup = Setup::new();
    let beta35 = gate::REQUIRED.iter().copied().map(str::to_owned).collect();
    *setup.macbook.features.lock().unwrap() = Some(beta35);
    let mut list = setup.list();
    list["computers"][0]["here"] = json!(false);
    list["computers"][1]["here"] = json!(true);
    list["computers"][0]["fleetUpdate"] = json!({
        "current": "0.8.0-beta.35", "target": "0.8.0-beta.36", "state": "behind", "canRetry": false, "requiresUpdate": true
    });
    list["fleetUpdate"] = json!({
        "target": "0.8.0-beta.36", "stage": "behind", "done": 1, "behind": 2, "updating": 0, "failed": 0, "dismissed": false, "failures": []
    });
    *setup.mini.0.computers.lock().unwrap() = list;
    *setup.devbox.0.features.lock().unwrap() = Some(gate::BASELINE.iter().map(|feature| (*feature).to_owned()).collect());
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    boxes.read_list().unwrap();
    assert_eq!(boxes.fleet_update().unwrap().target, "0.8.0-beta.36");
    assert_eq!(boxes.nearest_key().as_deref(), Some(MACBOOK_KEY));
    let refuses_work = || matches!(boxes.nearest().unwrap().send(&Request::post("kuvo/sessions/start", json!({}))),
        Err(Error::Refused(refusal)) if refusal.reason.as_deref() == Some("computer-update-required"));
    assert!(refuses_work());

    setup.mini.1.stop();
    assert!(boxes.check(&setup.mini.1.key_text()).is_some_and(|reach| reach != Reach::Connected));
    boxes.read_list().unwrap();
    assert_eq!(boxes.fleet_update().unwrap().target, "0.8.0-beta.36");
    assert!(refuses_work());
}

#[test]
fn the_kept_list_opens_the_app_with_your_computers_and_holds_nothing_secret() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let path = setup.kept.path().join("desktop/boxes.json");
    let kept = std::fs::read_to_string(&path).expect("kept");
    // Public facts only: never this device's key or secret.
    let Kept::Found(secret) = setup.keeper.read() else { panic!("its key is in the keeper") };
    for part in secret.split('.') {
        assert!(!kept.contains(part));
    }
    assert!(kept.contains("Mac mini"));
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

/// A keeper whose key can't be read now (a locked keychain), though one is kept.
struct Locked(MemoryKeeper, std::sync::atomic::AtomicBool);

impl Keeper for Locked {
    fn read(&self) -> Kept {
        if self.1.load(std::sync::atomic::Ordering::SeqCst) { Kept::Unreadable } else { self.0.read() }
    }

    fn write(&self, kept: &str) -> bool {
        self.0.write(kept)
    }

    fn forget(&self) {
        self.0.forget()
    }
}

#[test]
fn a_key_that_cant_be_read_now_is_never_replaced_by_a_new_one() {
    let setup = Setup::new();
    let keeper = Arc::new(Locked(MemoryKeeper::default(), std::sync::atomic::AtomicBool::new(false)));
    let boxes_with = |keeper: Arc<Locked>| Boxes::new(Options { nearest: Some(setup.macbook.clone() as Arc<dyn Transport>), keeper, dial: setup.relay.dial(), name: "MacBook".into(), app: "0.8.0".into(), kept: None, timing: Setup::timing() });
    let first = boxes_with(keeper.clone());
    first.read_list().unwrap();
    first.settle();
    let kept = keeper.0.read();
    assert_eq!(setup.pair_asks().len(), 1);

    // The next start, the keychain is locked: nothing's paired, and the key kept isn't written over.
    keeper.1.store(true, std::sync::atomic::Ordering::SeqCst);
    let locked = boxes_with(keeper.clone());
    locked.read_list().unwrap();
    locked.settle();
    assert_eq!(setup.pair_asks().len(), 1, "no new key was paired");
    assert_eq!(keeper.0.read(), kept, "the key kept is as it was");
    assert!(matches!(locked.pairing(), Pairing::Refused(_)));
    assert!(locked.client(&setup.mini.1.key_text()).is_none());
}

#[test]
fn a_computer_dropped_later_is_let_go_and_its_session_ended() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let devbox = named(&boxes, "devbox");
    let held = boxes.client(&devbox).unwrap();
    held.get::<Value>("whoami").unwrap();
    let before = boxes.client_made(&devbox);
    assert!(before.is_some());

    // The list drops it.
    let mut list = setup.list();
    list["computers"].as_array_mut().unwrap().remove(2);
    *setup.macbook.computers.lock().unwrap() = list;
    boxes.read_list().unwrap();
    assert!(boxes.client(&devbox).is_none() && boxes.client_made(&devbox).is_none());
    assert!(!reach(&boxes).iter().any(|(name, _)| name == "devbox"));
    // Its session was ended there, not left open: devbox sees a new hello only if asked again.
    let hellos = setup.devbox.1.hellos().len();
    let _ = held.get::<Value>("whoami");
    assert_eq!(setup.devbox.1.hellos().len(), hellos + 1, "the session it had was closed");
}

#[test]
fn a_computer_that_keeps_turning_the_app_away_doesnt_make_it_ask_to_pair_over_and_over() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    // devbox stops taking the device (its switch is off), while the nearest box still says "paired".
    setup.devbox.1.remove(&keys::decode_exact(setup.pair_asks()[0]["key"].as_str().unwrap()).unwrap());
    setup.pairs(|_| (200, json!({ "paired": true, "pairing": "pair_1" })));
    let _ = whoami(&boxes, "devbox");
    for _ in 0..5 {
        boxes.settle();
    }
    assert_eq!(reach(&boxes)[2], ("devbox".to_owned(), Reach::NotPaired));
    assert_eq!(setup.pair_asks().len(), 2, "asked once more, to learn whether it was removed, then no more");
}

#[test]
fn paired_again_every_computer_gets_a_new_client() {
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let mini = named(&boxes, "Mac mini");
    let before = boxes.client_made(&mini);
    assert_eq!(boxes.pair_again(), Pairing::Paired);
    assert_ne!(boxes.client_made(&mini), before, "what held the old key's client sees it's been replaced");
    boxes.settle();
    assert_eq!(whoami(&boxes, "Mac mini").unwrap()["name"], "Mac mini");
}

#[cfg(unix)]
#[test]
fn what_it_keeps_is_yours_alone_even_where_a_wider_file_was_there_already() {
    use std::os::unix::fs::PermissionsExt;
    let made = tempfile::tempdir().unwrap();
    let folder = made.path().join("desktop");
    let path = folder.join("identity");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(&path, "something longer that was there before").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    kuvo_box::boxes::write_private(&path, b"kept").unwrap();
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!((mode(&path), mode(&folder)), (0o600, 0o700));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
}

#[test]
fn the_list_taken_again_while_a_message_goes_keeps_the_computers_client_and_session() {
    // The list's follow brings it every few seconds, each computer's other facts changed (whether
    // it's connected, its version, what's at work): a send on its way there is never cut off.
    let setup = Setup::new();
    let boxes = setup.boxes();
    boxes.read_list().unwrap();
    boxes.settle();
    let devbox = named(&boxes, "devbox");
    let held = boxes.client(&devbox).unwrap();
    held.get::<Value>("whoami").unwrap();
    let (before, hellos) = (boxes.client_made(&devbox), setup.devbox.1.hellos().len());
    let mut list = setup.list();
    list["computers"][2]["reach"] = json!("connecting");
    list["computers"][2]["version"] = json!("0.8.9");
    list["computers"][2]["working"] = json!(2);
    *setup.macbook.computers.lock().unwrap() = list;
    boxes.read_list().unwrap();
    held.get::<Value>("whoami").unwrap();
    assert_eq!(boxes.client_made(&devbox), before, "the same client");
    assert_eq!(setup.devbox.1.hellos().len(), hellos, "on the same session");
}
