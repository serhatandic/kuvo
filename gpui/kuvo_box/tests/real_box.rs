//! The Rust link against a REAL box: the box's own link (`kuvo-link.js run`, as a machine runs it,
//! on Bun where there's one) in a scratch home, on the bridge tests' relay stand-in
//! (`box/bridge-tests/real-box-for-clients.js` starts both). Everything else here is held to the
//! shared vectors and a Rust stand-in; this is where a drift between the two implementations (the
//! hello's fields, a frame, a message's head, `kuvo/info`, the list of computers) fails.
//!
//! Needs `node` on the PATH (as the bridge tests do). Nothing real is reached: the box's home is a
//! folder of its own, removed as the test ends, and its process is stopped by its pid.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use kuvo_box::areas::Computers;
use kuvo_box::link::{Host, Identity, LinkBox, LinkClient, Timing, WebSocketDial, keys};
use kuvo_box::transport::{Request, Transport};
use kuvo_box::{BoxClient, Gate, gate};
use serde_json::Value;

/// The real box, until dropped: closing its standard input ends it, and its folder goes.
struct RealBox {
    child: Child,
    said: Value,
}

impl RealBox {
    fn start() -> RealBox {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../box/bridge-tests/real-box-for-clients.js");
        let mut command = Command::new("node");
        command.arg(&script).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        // (No `KUVO_*` of whoever runs the tests reaches the box; the fixture drops them too.)
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KUVO_") {
                command.env_remove(name);
            }
        }
        command.env("KUVO_TEST_WRAPPER_PID", std::process::id().to_string());
        let mut child = command.spawn().expect("node runs the box's fixture (it must be on the PATH, as for the bridge tests)");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("its output")).read_line(&mut line).expect("its first line");
        let said: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("the box didn't start: {line:?}"));
        RealBox { child, said }
    }

    fn text(&self, name: &str) -> &str {
        self.said[name].as_str().unwrap_or_else(|| panic!("the box said no {name}"))
    }
}

impl Drop for RealBox {
    fn drop(&mut self) {
        // Its input ends: it stops the box it started and removes its folder.
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

#[test]
fn the_rust_link_pairs_with_a_real_box_asks_it_and_follows_it() {
    let real = RealBox::start();
    let relay = real.text("relay").to_owned();
    let host = Host::from_text(real.text("key"), real.text("host")).expect("its key and address");
    let identity = Identity::new();
    let client = LinkClient::new(relay.clone(), identity.clone(), "Rust desktop", "0.8.0", Arc::new(WebSocketDial), Timing::default());

    // The handshake, with the pairing token in its hello: the box takes it and says what it is.
    let hello = client.pair(&host, real.text("token")).expect("the real box pairs this device by its token");
    assert_eq!((hello.v, hello.kind.as_str(), hello.name.as_str()), (1, "machine", "real-box"));

    // A request: its `kuvo/info`, read as this client reads it, with every name the gate needs.
    let linked = LinkBox::new(client.clone(), host.clone(), "real-box");
    let boxed = BoxClient::new(LinkBox::new(client.clone(), host.clone(), "real-box"));
    let info = boxed.connect().expect("kuvo/info from the real box");
    assert_eq!((info.kind.as_str(), info.name.as_str()), ("machine", "real-box"));
    let missing: Vec<&str> = gate::REQUIRED.iter().copied().filter(|name| !info.features.iter().any(|has| has == name)).collect();
    assert_eq!(missing, Vec::<&str>::new(), "this build's box has every feature this client requires");
    assert_eq!(boxed.gate(), Gate::Open);

    // The box took it as a desktop, from the hello's `client`, and knows who asks.
    let phones: Value = boxed.get("kuvo/phones").expect("kuvo/phones");
    let me = phones["phones"].as_array().unwrap().iter().find(|phone| phone["you"] == true).expect("this device is listed");
    assert_eq!((me["kind"].as_str(), me["name"].as_str(), me["key"].as_str()), (Some("desktop"), Some("Rust desktop"), Some(keys::encode(&identity.public()).as_str())));

    // A refusal comes back whole, in the box's words: only the app beside a box adds itself.
    let refused = linked.send(&Request::post("kuvo/devices/pair", serde_json::json!({ "key": keys::encode(&Identity::new().public()), "name": "Other", "kind": "desktop" }))).expect("answered");
    assert_eq!(refused.status, 403);

    // An event: the list of computers, followed; its first line says where this box is reached,
    // as this client connects to it (`computers.reach`).
    let stream = boxed.open(&Request::get("kuvo/computers/events"), &|_| true).expect("followed");
    assert_eq!(stream.status, 200);
    let mut reader = stream.reader;
    let mut line = String::new();
    reader.read_line(&mut line).expect("its first line");
    let list: Computers = serde_json::from_str(&line).expect("the list, as this client reads it");
    let here = list.computers.iter().find(|computer| computer.here).expect("the box itself");
    assert_eq!((here.key.as_str(), here.address.as_deref(), here.relay.as_deref()), (real.text("key"), Some(real.text("host")), Some(relay.as_str())));
    // Closed from here, the box stops sending and the session goes on.
    stream.closer.close();
    line.clear();
    assert_eq!(reader.read_line(&mut line).unwrap_or(0), 0);
    boxed.get::<Value>("kuvo/phones").expect("the same session, after the follow was cancelled");

    // Let in without the token from now on: a new session, as after a restart of the app.
    client.let_go();
    assert_eq!(client.hello(&host).expect("paired").name, "real-box");
}
