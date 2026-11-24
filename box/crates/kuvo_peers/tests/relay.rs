//! The Rust socket and session owner over loopback WebSockets. The stand-in emits the relay's
//! responses so disconnection, grants, and malformed frames can be tested deterministically.

use std::{collections::HashMap, time::Duration};

use futures_util::{SinkExt, StreamExt};
use kuvo_peers::{
    relay::{Config, Event, Relay, RelayEvent},
    session::{self, BoxHello, ClientHello, ComputerKind, Message},
};
use kuvo_wire::{
    frames::{self, Kind},
    keys::{self, Address, Host, Identity},
    mail::Envelope,
    noise,
};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message as Ws};

const RUST_KEY: [u8; 32] = [11; 32];
const RUST_SECRET: [u8; 32] = [21; 32];
const PEER_KEY: [u8; 32] = [12; 32];
const PEER_ADDRESS: Address = [42; 16];

fn public(key: [u8; 32]) -> [u8; 32] {
    Identity::from_bytes(key, [0; 32]).public()
}

fn hello() -> ClientHello {
    ClientHello {
        v: 1,
        name: "Paired desktop".into(),
        app: "1".into(),
        client: "desktop".into(),
        pair: None,
    }
}

fn config(url: String) -> Config {
    Config {
        url,
        secret: RUST_SECRET,
        key: RUST_KEY,
        hello: BoxHello {
            v: 1,
            kind: ComputerKind::Machine,
            name: "Rust".into(),
            app: "1".into(),
            engine: None,
        },
        grants: HashMap::from([(public(PEER_KEY), "first".into())]),
        peers: vec![Host {
            key: public(PEER_KEY),
            address: PEER_ADDRESS,
        }],
    }
}

struct TestRelay(WebSocketStream<TcpStream>);

impl TestRelay {
    async fn connect() -> (Relay, Self) {
        Self::connect_with(|_| {}).await
    }

    async fn connect_with(change: impl FnOnce(&mut Config)) -> (Relay, Self) {
        let (mut relay, test, address) = Self::welcome(change).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Connected(got)) if got == address));
        (relay, test)
    }

    async fn welcome(change: impl FnOnce(&mut Config)) -> (Relay, Self, Address) {
        let (relay, test, address, _) = Self::reconnecting(change).await;
        (relay, test, address)
    }

    async fn reconnecting(change: impl FnOnce(&mut Config)) -> (Relay, Self, Address, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = config(format!("ws://{}", listener.local_addr().unwrap()));
        change(&mut config);
        let secret = config.secret;
        let relay = Relay::start(config);
        let test = Self::accept(&listener, secret).await;
        (relay, test, keys::address_of(&secret), listener)
    }

    async fn accept(listener: &TcpListener, secret: [u8; 32]) -> Self {
        let mut test = Self(
            accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap(),
        );
        assert_eq!(test.binary().await, frames::relay_hello(&secret));
        let address = keys::address_of(&secret);
        test.send([&[1], &address[..]].concat()).await;
        test
    }

    async fn send(&mut self, bytes: Vec<u8>) {
        self.0.send(Ws::Binary(bytes.into())).await.unwrap();
    }

    async fn receive(&mut self, address: Address, kind: Kind, number: u32, body: &[u8]) {
        self.send(frames::relay_send(
            &address,
            &frames::frame(kind, number, body),
        ))
        .await;
    }

    async fn binary(&mut self) -> Vec<u8> {
        self.0.next().await.unwrap().unwrap().into_data().to_vec()
    }

    async fn frame(&mut self) -> (Kind, u32, Vec<u8>) {
        let sent = self.binary().await;
        assert_eq!(sent[0], 2);
        assert_eq!(sent[1..17], PEER_ADDRESS);
        let (kind, number, body) = frames::read_frame(&sent[17..]).unwrap();
        (kind, number, body.to_vec())
    }

    async fn moving(&mut self) -> serde_json::Value {
        self.mailed("moving").await["moves"].clone()
    }

    async fn move_step(&mut self, op: &str) -> serde_json::Value {
        loop {
            let movement = self.next_move().await;
            if movement["op"] == op {
                return movement;
            }
        }
    }

    async fn next_move(&mut self) -> serde_json::Value {
        self.mailed("move").await
    }

    async fn mailed(&mut self, field: &str) -> serde_json::Value {
        loop {
            let sent = self.binary().await;
            if sent.get(1..17) != Some(PEER_ADDRESS.as_slice()) {
                continue;
            }
            if let Some((Kind::Mail, _, bytes)) = frames::read_frame(&sent[17..]) {
                let mail = Envelope::open(bytes, &PEER_KEY, &public(RUST_KEY)).unwrap();
                if let Some(value) = mail.0.get(field) {
                    return value.clone();
                }
            }
        }
    }

    async fn paired(&mut self, number: u32) -> noise::Transport {
        let mut ik = noise::Initiator::new(PEER_KEY, public(RUST_KEY), noise::PROLOGUE);
        let payload = serde_json::to_vec(&hello()).unwrap();
        let first = ik.write_message_1(&payload).unwrap();
        self.receive(PEER_ADDRESS, Kind::Handshake1, number, &first)
            .await;
        let (kind, reply, body) = self.frame().await;
        assert_eq!((kind, reply), (Kind::Handshake2, number));
        let (reply, transport) = ik.read_message_2(&body).unwrap();
        assert_eq!(
            serde_json::from_slice::<BoxHello>(&reply).unwrap().name,
            "Rust"
        );
        transport
    }
}

#[tokio::test]
async fn only_paired_keys_open_and_the_first_transport_message_confirms_the_device() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut relay, mut test) = TestRelay::connect().await;
        let mut stranger = noise::Initiator::new([23; 32], public(RUST_KEY), noise::PROLOGUE);
        let bytes = stranger.write_message_1(&serde_json::to_vec(&hello()).unwrap()).unwrap();
        test.receive(PEER_ADDRESS, Kind::Handshake1, 1, &bytes).await;
        assert_eq!(test.frame().await, (Kind::Close, 1, vec![]));

        let mut transport = test.paired(2).await;
        let Some(Event::Session(session::Event::Accepted(id, device))) = relay.events.recv().await else { panic!("paired device") };
        assert_eq!(device.key, public(PEER_KEY));
        assert_eq!(device.grant, "first");
        let plain = frames::encode_message(&json!({"t":"ping"}), &[]);
        test.receive(PEER_ADDRESS, Kind::Transport, 2, &transport.seal(&plain)).await;
        let (kind, number, pong) = test.frame().await;
        assert_eq!((kind, number), (Kind::Transport, 2));
        let pong = transport.open(&pong).unwrap();
        assert_eq!(frames::decode_message(&pong).unwrap().0, json!({"t":"pong"}));
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Confirmed(got))) if got == id));

        let request = frames::encode_message(&json!({"t":"req","id":1,"p":"kuvo/info","end":true}), b"payload");
        test.receive(PEER_ADDRESS, Kind::Transport, 2, &transport.seal(&request)).await;
        let Some(Event::Session(session::Event::Message(got, message))) = relay.events.recv().await else { panic!("request") };
        assert_eq!(got, id);
        assert_eq!(message.head["p"], "kuvo/info");
        assert_eq!(message.body, b"payload");

        relay.sender.trust(HashMap::from([(device.key, "replacement".into())]), vec![]).await.unwrap();
        assert_eq!(test.frame().await, (Kind::Close, 2, vec![]));
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Ended(got))) if got == id));
    }).await.unwrap();
}

#[tokio::test]
async fn stale_and_tampered_transport_frames_are_closed() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut relay, mut test) = TestRelay::connect().await;
        test.receive(PEER_ADDRESS, Kind::Transport, 6, b"old").await;
        assert_eq!(test.frame().await, (Kind::Close, 6, vec![]));

        let mut transport = test.paired(7).await;
        let Some(Event::Session(session::Event::Accepted(id, _))) = relay.events.recv().await else { panic!("paired device") };
        let mut sealed = transport.seal(&frames::encode_message(&json!({ "t":"ping" }), &[]));
        sealed[0] ^= 1;
        test.receive(PEER_ADDRESS, Kind::Transport, 7, &sealed).await;
        assert_eq!(test.frame().await, (Kind::Close, 7, vec![]));
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Ended(got))) if got == id));
    }).await.unwrap();
}

#[tokio::test]
async fn only_authenticated_mail_from_the_roster_is_delivered() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut relay, mut test) = TestRelay::connect().await;
        let handle = relay.handle();
        let mut observer = handle.subscribe().expect("active relay subscription");
        let value = json!({"at":42,"agents":{"list":[]}});
        let mail = Envelope(value.as_object().unwrap().clone());
        for (key, marker) in [([51; 32], 0), (PEER_KEY, 1)] {
            let bytes = mail.seal(&key, &public(RUST_KEY)).unwrap();
            // A peer's known key remains valid when its advertised relay address changes.
            test.send(frames::relay_send(&[marker; 16], &bytes)).await;
        }
        let Some(Event::Mail {
            from,
            key,
            envelope,
        }) = relay.events.recv().await
        else {
            panic!("known sender's mail")
        };
        assert_eq!((from, key, &envelope), ([1; 16], public(PEER_KEY), &mail));
        let observed = observer.recv().await.expect("same authenticated event");
        let RelayEvent::Mail {
            from,
            key,
            envelope,
        } = observed.as_ref()
        else {
            panic!("observer must receive the authenticated peer's mail first");
        };
        assert_eq!((*from, *key, envelope), ([1; 16], public(PEER_KEY), &mail));
        handle
            .sender
            .mail(
                Host {
                    key: public(PEER_KEY),
                    address: PEER_ADDRESS,
                },
                mail.clone(),
            )
            .await
            .unwrap();
        let (kind, number, body) = test.frame().await;
        assert_eq!((kind, number), (Kind::Mail, 0));
        assert_eq!(
            Envelope::open(&body, &PEER_KEY, &public(RUST_KEY)).unwrap(),
            mail
        );
        drop(relay);
        assert!(matches!(
            observer.recv().await,
            Err(tokio::sync::broadcast::error::RecvError::Closed)
        ));
        assert!(handle.subscribe().is_none());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn relay_replacement_retries_an_opening_handshake_and_ends_a_live_session() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut relay, mut test) = TestRelay::connect().await;
        let outgoing = relay.sender.start(Host { key: public(PEER_KEY), address: PEER_ADDRESS }, hello()).await.unwrap();
        let (kind, number, initial) = test.frame().await;
        assert_eq!((kind, number), (Kind::Handshake1, outgoing.number));
        test.send([&[7][..], &PEER_ADDRESS, &[0]].concat()).await;
        let (kind, number, restarted) = test.frame().await;
        assert_eq!(kind, Kind::Handshake1);
        assert_ne!(outgoing.number, number);
        assert_ne!(initial, restarted);
        let Some(Event::Session(session::Event::Reopening { before, after })) = relay.events.recv().await else { panic!("opening restarted") };
        assert_eq!(before, outgoing);
        assert_eq!(after.number, number);
        assert!(matches!(relay.events.recv().await, Some(Event::Gone { replaced: true, .. })));

        let mut respond = noise::Responder::new(PEER_KEY, noise::PROLOGUE);
        respond.read_message_1(&restarted).unwrap();
        let (second, _) = respond.write_message_2(&serde_json::to_vec(&config(String::new()).hello).unwrap()).unwrap();
        test.receive(PEER_ADDRESS, Kind::Handshake2, number, &second).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Opened(id, _))) if id == after));

        test.send([&[7][..], &PEER_ADDRESS, &[1]].concat()).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Ended(id))) if id == after));
        assert!(matches!(relay.events.recv().await, Some(Event::Gone { replaced: false, .. })));
        assert!(relay.sender.send(after, Message { head: json!({"t":"ping"}), body: vec![] }).await.is_err());
    }).await.unwrap();
}

#[tokio::test]
async fn the_relay_disappearing_is_reported_and_the_client_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let mut relay = Relay::start(config(url));
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut test = TestRelay(
            accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap(),
        );
        test.binary().await;
        test.0.close(None).await.unwrap();
        assert!(matches!(
            relay.events.recv().await,
            Some(Event::Disconnected(_))
        ));

        let mut test = TestRelay(
            accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap(),
        );
        assert_eq!(test.binary().await, frames::relay_hello(&RUST_SECRET));
        let address = keys::address_of(&RUST_SECRET);
        test.send([&[1], &address[..]].concat()).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Connected(got)) if got == address));
    })
    .await
    .unwrap();
}

/// Run after scripts/build-built.sh and installing the relay deps, with KUVO_RS15_BUN set.
#[tokio::test]
#[ignore = "live TypeScript box and local production relay"]
async fn typescript_and_rust_exchange_a_paired_request_and_an_outbox_letter() {
    use kuvo_peers::outbox::{Delivered, Id, Outbox, Settlement};
    use std::{
        process::Stdio,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        process::Command,
    };

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let homes = tempfile::tempdir().unwrap();
    let bun = std::env::var_os("KUVO_RS15_BUN").expect("set KUVO_RS15_BUN");
    let home = homes.path().join("ts-peer");
    let ts_run = home.join("run");
    std::fs::create_dir_all(&ts_run).unwrap();

    let mut relay_process = Command::new(&bun)
        .arg(root.join("box/test/fixtures/rust-relay-peer.ts"))
        .current_dir(&root)
        .env_clear()
        .env("HOME", &home)
        .env("KUVO_HOME", &home)
        .env("TMPDIR", &home)
        .env("BUN_BE_BUN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(relay_process.stdout.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(15), stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let url = line.trim();
    assert!(url.starts_with("ws://127.0.0.1:"));

    let ts_secret = [22; 32];
    let rust_key = keys::encode(&public(RUST_KEY));
    // A desktop grant and a computer's peer key are separate identities in TypeScript.
    let device_key = [31; 32];
    let device_secret = [32; 32];
    let peer = Host {
        key: public(PEER_KEY),
        address: keys::address_of(&ts_secret),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let letter_id = "ltr_live_check";
    let state = json!({
        "key": keys::encode(&PEER_KEY),
        "secret": keys::encode(&ts_secret),
        "sign": keys::encode(&[23; 32]),
        "name": "TypeScript",
        "relay": url,
        "mac": rust_key,
        "macs": [rust_key],
        "phones": [{ "key": keys::encode(&public(device_key)), "name": "Rust desktop", "from": "pair", "kind": "desktop" }],
        "legacy": { (&rust_key): { "at": now, "peers": [{
            "key": rust_key, "address": keys::encode(&keys::address_of(&RUST_SECRET)),
            "name": "Rust", "kind": "mac"
        }] } }
    });
    std::fs::write(
        ts_run.join("link.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    // Simulate a letter the TypeScript box accepted before its reply was lost. Its real mail
    // handler must decrypt the resend and acknowledge that delivery without an engine.
    std::fs::write(
        ts_run.join("letters-seen.json"),
        serde_json::to_vec(&json!({ (letter_id): now })).unwrap(),
    )
    .unwrap();
    let mut typescript = Command::new(bun)
        .arg(root.join("box/dist/kuvo-link.js"))
        .arg("run")
        .current_dir(&home)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("KUVO_HOME", &home)
        .env("TMPDIR", &home)
        .env("BUN_BE_BUN", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut startup = BufReader::new(typescript.stderr.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(line) = startup.next_line().await.unwrap() {
            if line.contains("\"event\":\"relay welcomed\"") {
                return;
            }
            eprintln!("{line}");
        }
        panic!("TypeScript box closed before the relay welcomed it")
    })
    .await
    .unwrap();

    let mut device = config(url.to_owned());
    device.key = device_key;
    device.secret = device_secret;
    let mut rust = Relay::start(device);

    tokio::time::timeout(Duration::from_secs(30), async {
        assert!(matches!(
            rust.events.recv().await,
            Some(Event::Connected(_))
        ));
        let session = rust.sender.start(peer.clone(), hello()).await.unwrap();
        loop {
            match rust.events.recv().await.unwrap() {
                Event::Session(session::Event::Opened(id, reply)) => {
                    assert_eq!(id, session);
                    assert_eq!(reply.name, "TypeScript");
                    break;
                }
                Event::Session(session::Event::Ended(id)) => {
                    panic!("TypeScript closed handshake: {id:?}")
                }
                _ => {}
            }
        }
        rust.sender
            .send(
                session,
                Message {
                    head: json!({"t":"req","id":1,"m":"GET","p":"kuvo/info","h":{},"end":true}),
                    body: vec![],
                },
            )
            .await
            .unwrap();
        let Some(Event::Session(session::Event::Message(id, head))) = rust.events.recv().await
        else {
            panic!("TypeScript response head")
        };
        assert_eq!(id, session);
        assert_eq!(head.head["s"], 200);
        let mut data = head.body;
        let mut end = head.head["end"] == true;
        while !end {
            let Some(Event::Session(session::Event::Message(id, piece))) = rust.events.recv().await
            else {
                panic!("TypeScript response body")
            };
            assert_eq!(id, session);
            data.extend(piece.body);
            end = piece.head["end"] == true;
        }
        let info: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(info["name"], "TypeScript");
        assert_eq!(info["kind"], "machine");
        let mut settings = config(url.to_owned());
        settings.peers = vec![peer.clone()];
        let mut rust = Relay::start(settings);
        assert!(matches!(
            rust.events.recv().await,
            Some(Event::Connected(_))
        ));

        let run = homes.path().join("rust-peer/run");
        let mut outbox = Outbox::open(&run).unwrap();
        let id: Id = letter_id.to_owned().try_into().unwrap();
        let letter = serde_json::from_value(
            json!({"to":"ses_test","text":"A letter through the relay","at":now}),
        )
        .unwrap();
        assert!(outbox.enqueue(&id, &letter).unwrap());
        drop(outbox);
        let mut outbox = Outbox::open(&run).unwrap();
        let wire = outbox.attempt(&id, &peer.key, now).unwrap().unwrap();
        rust.sender.mail(peer.clone(), wire).await.unwrap();
        loop {
            if let Event::Mail { key, envelope, .. } = rust.events.recv().await.unwrap()
                && let Some(value) = envelope.0.get("delivered")
            {
                let receipt: Delivered = serde_json::from_value(value.clone()).unwrap();
                assert_eq!(key, peer.key);
                assert_eq!(receipt.id, id);
                assert_eq!(
                    outbox.settle(&key, &receipt, now).unwrap(),
                    Settlement::Delivered
                );
                break;
            }
        }
        assert!(outbox.due(now + 60_000, true).unwrap().is_empty());
    })
    .await
    .unwrap();

    typescript.kill().await.unwrap();
    relay_process.stdin.take();
    let status = tokio::time::timeout(Duration::from_secs(5), relay_process.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn activity_from_other_addresses_does_not_evict_a_live_session() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut relay, mut test) = TestRelay::connect().await;
        let mut transport = test.paired(3).await;
        let Some(Event::Session(session::Event::Accepted(id, _))) = relay.events.recv().await else {
            panic!("paired device");
        };
        let other = [25; 16];
        test.send([&[7][..], &other, &[0]].concat()).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Gone { address, .. }) if address == other));
        let request = frames::encode_message(&json!({"t":"req","id":6,"m":"GET","p":"kuvo/info"}), &[]);
        test.receive(PEER_ADDRESS, Kind::Transport, 3, &transport.seal(&request)).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Confirmed(got))) if got == id));
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Message(got, _))) if got == id));

        test.send([&[3][..], &PEER_ADDRESS].concat()).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Session(session::Event::Ended(got))) if got == id));
        assert!(matches!(relay.events.recv().await, Some(Event::Unreachable(address)) if address == PEER_ADDRESS));
        // An unknown frame gets a close. No close got there when the relay said unreachable.
        test.receive(PEER_ADDRESS, Kind::Transport, 99, b"old").await;
        assert_eq!(test.frame().await, (Kind::Close, 99, vec![]));
    }).await.unwrap();
}

#[tokio::test]
async fn a_destination_replays_only_the_committed_bundles_receipt() {
    use kuvo_peers::{
        directory::{Member, Roster},
        moves::receipt::{Bundle, Decided, Decision, Payloads, Receipts},
        setup_lists::ResidentPeers,
    };
    use kuvo_wire::keys::encode;
    use std::sync::Arc;

    tokio::time::timeout(Duration::from_secs(8), async {
        let home = tempfile::tempdir().unwrap();
        let receipt = Decision {
            decided: Decided::Switched,
            at: 10,
            bundle: Bundle { generation: Some(2), hash: Some("P".repeat(43)) },
            peer: Some(encode(&public(PEER_KEY))),
            payloads: Some(Payloads { sessions: vec![], queue: vec![] }),
            last: None,
        };
        Receipts::open(&home.path().join("run"))
            .record("mov_committed", &receipt, &["ses_committed".into()]).unwrap();
        let (relay, mut wire) = TestRelay::connect().await;
        let _resident = ResidentPeers::start(
            home.path(), Roster {
                here: Some(public(RUST_KEY)),
                main: Some(public(RUST_KEY)),
                name: "Destination".into(),
                members: vec![Member {
                    key: public(PEER_KEY), address: Some(PEER_ADDRESS), name: "Source".into(),
                    kind: ComputerKind::Machine,
                }],
            }, Some(relay), None, false, Arc::new(|| 12),
        ).await.unwrap();

        // The accepted sender tries a different generation and a move for which there is
        // no decision. The following valid retry is the first reply that can prove ownership.
        for (id, generation, hash) in [
            ("mov_committed", 1, "P".repeat(43)),
            ("mov_committed", 2, "Q".repeat(43)),
            ("mov_missing", 2, "P".repeat(43)),
            ("mov_committed", 2, "P".repeat(43)),
        ] {
            let value = json!({"at": 12, "move": {"op": "switch", "safeSwitch": true, "id": id, "generation": generation, "hash": hash}});
            let bytes = Envelope(value.as_object().unwrap().clone()).seal(&PEER_KEY, &public(RUST_KEY)).unwrap();
            wire.send(frames::relay_send(&PEER_ADDRESS, &bytes)).await;
        }
        loop {
            let (kind, _, body) = wire.frame().await;
            if kind != Kind::Mail {
                continue;
            }
            let opened = Envelope::open(&body, &PEER_KEY, &public(RUST_KEY)).unwrap();
            if let Some(proof) = opened.0.get("move") {
                assert_eq!(*proof, json!({
                    "op": "switched", "id": "mov_committed", "generation": 2,
                    "hash": "P".repeat(43), "verifiedPayloads": true,
                }));
                break;
            }
        }
    }).await.unwrap();
}

#[tokio::test]
async fn staged_move_parts_survive_a_restart_and_only_acknowledge_the_bound_bundle() {
    use kuvo_peers::{
        directory::{Member, Roster},
        moves::transfer::{BundleSize, IncomingBundle, PART_BYTES},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    tokio::time::timeout(Duration::from_secs(8), async {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("run/moves/mov_copy.json");
        let staged = home.path().join("run/moves/mov_copy");
        let mut data = vec![b'a'; PART_BYTES as usize];
        data.push(b'b');
        let hash = encode(&Sha256::digest(&data));
        let source = encode(&public(PEER_KEY));
        let dest = encode(&public(RUST_KEY));
        let journal = json!({
            "formatVersion":1, "id":"mov_copy", "thing":"ses_copy", "kind":"chat", "name":"Copy",
            "role":"to", "stage":"copying", "peer":source, "from":source, "to":dest,
            "sessions":[], "started":1, "size":data.len(), "sent":0, "generation":2, "hash":hash,
            "safeSwitch":true, "createdSessions":[],
        });
        files::write_atomic(&path, &serde_json::to_vec(&journal).unwrap()).unwrap();
        let roster = || Roster {
            here: Some(public(RUST_KEY)), main: Some(public(RUST_KEY)), name: "Destination".into(),
            members: vec![Member {
                key: public(PEER_KEY), address: Some(PEER_ADDRESS), name: "Source".into(),
                kind: ComputerKind::Machine,
            }],
        };
        for (index, part) in data.chunks(PART_BYTES as usize).enumerate() {
            let (relay, mut wire) = TestRelay::connect().await;
            let resident = ResidentPeers::start(home.path(), roster(), Some(relay), None, false, Arc::new(|| 12))
                .await.unwrap();
            // The same sender may still have an old generation in flight.
            for generation in [1, 2] {
                let packet = json!({"at":12, "move": {
                    "id":"mov_copy", "op":"part", "generation":generation, "hash":hash,
                    "i":index, "of":2, "d":encode(part),
                }});
                let bytes = Envelope(packet.as_object().unwrap().clone())
                    .seal(&PEER_KEY, &public(RUST_KEY)).unwrap();
                wire.send(frames::relay_send(&PEER_ADDRESS, &bytes)).await;
            }
            loop {
                let (kind, _, body) = wire.frame().await;
                if kind == Kind::Mail {
                    let opened = Envelope::open(&body, &PEER_KEY, &public(RUST_KEY)).unwrap();
                    if let Some(reply) = opened.0.get("move") {
                        assert_eq!(*reply, json!({"id":"mov_copy", "op":"got", "generation":2, "hash":hash, "i":index}));
                        break;
                    }
                }
            }
            assert_eq!(files::read(&staged.join("got")).unwrap().unwrap(), if index == 0 { &b"0\n"[..] } else { &b"0\n1\n"[..] });
            drop(resident);
        }
        let complete = IncomingBundle::open(&staged, BundleSize::new(data.len() as u64, &hash).unwrap()).unwrap();
        assert_eq!(files::read(complete.verify().unwrap()).unwrap().unwrap(), data);
        let saved: serde_json::Value = serde_json::from_slice(&files::read(&path).unwrap().unwrap()).unwrap();
        assert_eq!(saved["sent"], data.len());
        assert_eq!(saved["createdSessions"], json!([]));
        assert!(saved.get("activation").is_none());
    }).await.unwrap();
}

#[tokio::test]
async fn live_stream_parts_and_temporary_reads_leave_saved_peer_reach_alone() {
    use kuvo_peers::{
        directory::{Member, Roster},
        moves::receipt::{Bundle, Decided, Decision, Receipts},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use std::sync::Arc;

    tokio::time::timeout(Duration::from_secs(8), async {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        let second_key = [15; 32];
        let second = Host {
            key: public(second_key),
            address: [43; 16],
        };
        let original = encode(&public(PEER_KEY));
        let returning = encode(&second.key);
        let paths = ["peer-reach.json", "peer-reach-checked.json"].map(|name| run.join(name));
        let before = json!({(&original): 10, (&returning): 11});
        for path in &paths {
            files::write_atomic(path, &serde_json::to_vec(&before).unwrap()).unwrap();
        }
        Receipts::open(&run)
            .record(
                "mov_confirmed",
                &Decision {
                    decided: Decided::Switched,
                    at: 11,
                    bundle: Bundle::default(),
                    peer: Some(returning),
                    payloads: None,
                    last: None,
                },
                &[],
            )
            .unwrap();
        let (relay, mut wire) = TestRelay::connect_with(|config| {
            config.peers.push(second.clone());
        })
        .await;
        let _resident = ResidentPeers::start(
            home.path(),
            Roster {
                here: Some(public(RUST_KEY)),
                main: Some(public(RUST_KEY)),
                name: "Home".into(),
                members: [
                    (public(PEER_KEY), PEER_ADDRESS),
                    (second.key, second.address),
                ]
                .into_iter()
                .map(|(key, address)| Member {
                    key,
                    address: Some(address),
                    name: encode(&key),
                    kind: ComputerKind::Machine,
                })
                .collect(),
            },
            Some(relay),
            None,
            false,
            Arc::new(|| 12),
        )
        .await
        .unwrap();

        for (key, address, packet) in [
            (
                PEER_KEY,
                PEER_ADDRESS,
                json!({"workQueue": {"memory": true}}),
            ),
            (
                PEER_KEY,
                PEER_ADDRESS,
                json!({"sessionFollow": {"part": {}}}),
            ),
            (
                second_key,
                second.address,
                json!({"move": {"op": "decision?", "id": "mov_confirmed"}}),
            ),
        ] {
            let bytes = Envelope(packet.as_object().unwrap().clone())
                .seal(&key, &public(RUST_KEY))
                .unwrap();
            wire.send(frames::relay_send(&address, &bytes)).await;
        }

        // The second peer's receipt confirms both earlier packets have passed
        // through the resident. Only this ordinary mail clears its reach entry.
        loop {
            let bytes = wire.binary().await;
            if bytes.get(1..17) != Some(&second.address) {
                continue;
            }
            let Some((Kind::Mail, _, body)) = frames::read_frame(&bytes[17..]) else {
                continue;
            };
            let mail = Envelope::open(body, &second_key, &public(RUST_KEY)).unwrap();
            if mail.0.get("move").is_some() {
                break;
            }
        }
        for path in paths {
            let saved: serde_json::Value =
                serde_json::from_slice(&files::read(&path).unwrap().unwrap()).unwrap();
            assert_eq!(saved, json!({(&original): 10}));
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn owner_forwarding_binds_the_peer_to_the_accepted_setup_and_operation() {
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Action, Admission, Handle, Reply, Request, SetupState, Submitter},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use std::{collections::BTreeMap, sync::Arc};

    tokio::time::timeout(Duration::from_secs(8), async {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        let here = public(RUST_KEY);
        let peer = public(PEER_KEY);
        let former = public([14; 32]);
        let setup = format!("set_{}", encode(&[1; 16]));
        let operation = encode(&[2; 32]);
        let session = "ses_forwarded";
        let (relay, mut test) = TestRelay::connect().await;
        let resident = ResidentPeers::start(home.path(), Roster {
            here: Some(here), main: Some(here), name: "Owner".into(),
            members: vec![Member { key: peer, address: Some(PEER_ADDRESS), name: "Peer".into(), kind: ComputerKind::Machine }],
        }, Some(relay), None, false, Arc::new(|| 12)).await.unwrap();
        let devices = Devices::load(home.path(), "Owner".into(), ComputerKind::Machine).await.unwrap().devices;
        let accepted = SetupState {
            id: Some(setup.clone()), replacements: BTreeMap::from([(former, here)]),
            grants_current: true, grants_disabled: false,
        };
        let owner = Handle::new(resident.reader(), resident.relay_handle(), devices, move || accepted.clone());
        let mut incoming = owner.incoming().unwrap();
        // The move arrived after the peer reader took its initial snapshot.
        let moved = json!([{"session":session,"key":encode(&here),"at":10,"arrived":true,"formatVersion":1}]);
        files::write_atomic(&run.join("moves/moved.json"), &serde_json::to_vec(&moved).unwrap()).unwrap();
        let scope = owner.scope(session, Some(former)).unwrap();
        assert_eq!(scope.here, Some(here));
        assert_eq!(scope.receipt_issuer, Some(here));
        assert!(scope.owner.unwrap().local);

        let request = Request {
            setup: setup.clone(), session: session.into(), operation: operation.clone(), submitter: Submitter::Computer,
            action: Action::Submit { draft: json!({"id":"msg_forwarded"}) },
        };
        let nonce = encode(&[9; 16]);
        for (target, membership) in [(peer, setup.clone()), (here, format!("set_{}", encode(&[3; 16]))), (here, setup.clone())] {
            let packet = json!({"at":12,"ownerRequest":{"to":encode(&target),"nonce":nonce,"request":{
                "setup":membership,"session":session,"operation":operation,"submitter":{"kind":"computer"},
                "action":{"kind":"submit","draft":{"id":"msg_forwarded"}},
            }}});
            let bytes = Envelope(packet.as_object().unwrap().clone()).seal(&PEER_KEY, &here).unwrap();
            test.send(frames::relay_send(&PEER_ADDRESS, &bytes)).await;
        }
        let accepted = incoming.recv().await.unwrap();
        assert_eq!(accepted.peer(), peer);
        assert_eq!(*accepted.request(), request);
        assert_eq!(owner.admit(&accepted).unwrap(), Admission::Allowed);
        let reply = Reply { status: 200, body: json!({"confirmed":true}) };
        owner.reply(&accepted, reply.clone()).await.unwrap();
        loop {
            let (kind, _, body) = test.frame().await;
            if kind != Kind::Mail { continue; }
            let mail = Envelope::open(&body, &PEER_KEY, &here).unwrap();
            if let Some(packet) = mail.0.get("workQueue").and_then(|queue| queue.get("ownerReply")) {
                assert_eq!(packet, &json!({"to":encode(&peer),"nonce":nonce,"setup":setup,"operation":operation,"reply":reply}));
                break;
            }
        }

        // A switch fences a queued effect immediately, while the old receipt
        // can still be queried at its accepted replacement issuer.
        let move_record = json!({"id":"mov_forwarded","thing":session,"role":"from","stage":"switching",
            "peer":encode(&peer),"sessions":[session],"started":12,"formatVersion":1});
        files::write_atomic(&run.join("moves/mov_forwarded.json"), &serde_json::to_vec(&move_record).unwrap()).unwrap();
        assert_eq!(owner.admit(&accepted).unwrap(), Admission::OwnerChanged);
        assert_eq!(owner.scope(session, Some(former)).unwrap().owner.unwrap().key, Some(peer));

        let mut outcome = request;
        outcome.action = Action::Outcome { issuer: former, query: json!({"handle":"issued"}) };
        let client = owner.clone();
        let asking = tokio::spawn(async move { client.request(peer, outcome, Duration::from_secs(2)).await });
        loop {
            let (kind, _, body) = test.frame().await;
            if kind != Kind::Mail { continue; }
            let mail = Envelope::open(&body, &PEER_KEY, &here).unwrap();
            if let Some(packet) = mail.0.get("workQueue").and_then(|queue| queue.get("ownerRequest")) {
                assert_eq!(packet["to"], encode(&peer));
                for (key, body) in [(encode(&[6; 32]), json!({"forged":true})), (operation, reply.body.clone())] {
                    let answer = json!({"at":12,"ownerReply":{"to":encode(&here),"nonce":packet["nonce"],"setup":setup,
                        "operation":key,"reply":{"status":200,"body":body}}});
                    let bytes = Envelope(answer.as_object().unwrap().clone()).seal(&PEER_KEY, &here).unwrap();
                    test.send(frames::relay_send(&PEER_ADDRESS,&bytes)).await;
                }
                break;
            }
        }
        assert_eq!(asking.await.unwrap().unwrap(), reply);
    }).await.unwrap();
}

#[tokio::test]
async fn owner_submission_recovery_is_advertised_when_attached() {
    use kuvo_peers::{
        directory::{Member, Roster},
        setup_lists::ResidentPeers,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    async fn features(test: &mut TestRelay) -> serde_json::Value {
        loop {
            let (kind, _, body) = test.frame().await;
            if kind == Kind::Mail {
                let mail = Envelope::open(&body, &PEER_KEY, &public(RUST_KEY)).unwrap();
                if let Some(features) = mail
                    .0
                    .get("agents")
                    .and_then(|agents| agents.get("features"))
                {
                    return features.clone();
                }
            }
        }
    }

    tokio::time::timeout(Duration::from_secs(8), async {
        let home = tempfile::tempdir().unwrap();
        let (relay, mut test) = TestRelay::connect().await;
        let clock = Arc::new(AtomicI64::new(12));
        let resident = ResidentPeers::start(
            home.path(),
            Roster {
                here: Some(public(RUST_KEY)),
                main: Some(public(RUST_KEY)),
                name: "Owner".into(),
                members: vec![Member {
                    key: public(PEER_KEY),
                    address: Some(PEER_ADDRESS),
                    name: "Peer".into(),
                    kind: ComputerKind::Machine,
                }],
            },
            Some(relay),
            None,
            false,
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::Relaxed))
            },
        )
        .await
        .unwrap();
        let recovery = json!("typed.submission-recovery");
        assert!(
            !features(&mut test)
                .await
                .as_array()
                .unwrap()
                .contains(&recovery)
        );
        clock.store(13, Ordering::Relaxed);
        resident.advertise_engine_submissions().await.unwrap();
        while !features(&mut test)
            .await
            .as_array()
            .unwrap()
            .contains(&recovery)
        {}
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn owner_forwarding_carries_the_full_draft_in_each_direction() {
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Action, Admission, Error, Handle, Reply, Request, SetupState, Submitter},
        setup_lists::ResidentPeers,
    };
    use kuvo_wire::keys::encode;
    use std::sync::Arc;

    const OTHER_SECRET: [u8; 32] = [29; 32];
    let here = public(RUST_KEY);
    let peer = public(PEER_KEY);
    let here_address = keys::address_of(&RUST_SECRET);
    let peer_address = keys::address_of(&OTHER_SECRET);
    let (left_relay, mut left_wire) = TestRelay::connect_with(|config| {
        config.peers[0].address = peer_address;
    })
    .await;
    let (right_relay, mut right_wire) = TestRelay::connect_with(|config| {
        config.secret = OTHER_SECRET;
        config.key = PEER_KEY;
        config.peers = vec![Host {
            key: here,
            address: here_address,
        }];
    })
    .await;
    let capture = std::env::var_os("KUVO_OWNER_PARTS_TS");
    let (recorded, mut frames_seen) = tokio::sync::mpsc::unbounded_channel();
    let relay = tokio::spawn(async move {
        loop {
            let (mut frame, from, to, wire) = tokio::select! {
                frame = left_wire.binary() => (frame, here_address, peer_address, &mut right_wire),
                frame = right_wire.binary() => (frame, peer_address, here_address, &mut left_wire),
            };
            if frame.first() != Some(&2) {
                continue;
            }
            assert_eq!(&frame[1..17], &to);
            if capture.is_some() {
                let origin = u8::from(from == peer_address);
                recorded.send((origin, encode(&frame))).unwrap();
            }
            frame[1..17].copy_from_slice(&from);
            wire.send(frame).await;
        }
    });

    let setup = format!("set_{}", encode(&[3; 16]));
    async fn resident(
        key: [u8; 32],
        remote: Host,
        relay: Relay,
        setup: &str,
    ) -> (tempfile::TempDir, ResidentPeers, Handle) {
        let home = tempfile::tempdir().unwrap();
        let roster = Roster {
            here: Some(key),
            main: Some(key),
            name: "This computer".into(),
            members: vec![Member {
                key: remote.key,
                address: Some(remote.address),
                name: "Other computer".into(),
                kind: ComputerKind::Machine,
            }],
        };
        let resident = ResidentPeers::start(
            home.path(),
            roster,
            Some(relay),
            None,
            false,
            Arc::new(|| 123),
        )
        .await
        .unwrap();
        let devices = Devices::load(home.path(), "This computer".into(), ComputerKind::Machine)
            .await
            .unwrap()
            .devices;
        let accepted = SetupState {
            id: Some(setup.into()),
            ..SetupState::default()
        };
        let handle = Handle::new(
            resident.reader(),
            resident.relay_handle(),
            devices,
            move || accepted.clone(),
        );
        (home, resident, handle)
    }
    let (left_home, _left_resident, left) = resident(
        here,
        Host {
            key: peer,
            address: peer_address,
        },
        left_relay,
        &setup,
    )
    .await;
    let (right_home, _right_resident, right) = resident(
        peer,
        Host {
            key: here,
            address: here_address,
        },
        right_relay,
        &setup,
    )
    .await;
    let mut right_incoming = right.incoming().unwrap();
    let mut left_incoming = left.incoming().unwrap();
    assert_eq!(left.members()[0].key, Some(peer));
    assert_eq!(left.members()[0].name, "Other computer");

    for (session, home, key) in [
        ("ses_addressed_draft", right_home.path(), peer),
        ("ses_scoped_draft", left_home.path(), here),
    ] {
        let moved = json!([{"session":session,"key":encode(&key),"at":123,"arrived":true,"formatVersion":1}]);
        kuvo_store::files::write_atomic(
            &home.join("run/moves/moved.json"),
            &serde_json::to_vec(&moved).unwrap(),
        )
        .unwrap();
    }

    for (session, size, addressed, source, destination, from, to, inbox) in [
        (
            "ses_addressed_draft",
            5 * 1024 * 1024,
            true,
            &left,
            &right,
            here,
            peer,
            &mut right_incoming,
        ),
        (
            "ses_scoped_draft",
            128 * 1024,
            false,
            &right,
            &left,
            peer,
            here,
            &mut left_incoming,
        ),
    ] {
        let action = |extra| {
            let prefix = "data:application/octet-stream;base64,";
            let mut draft = json!({"id":"msg_draft","text":"","message":{
                "files":[{"name":"example.bin","uri":prefix}]
            }});
            let room = size - kuvo_wire::json::ordinary(&draft).len();
            draft["message"]["files"][0]["uri"] =
                format!("{prefix}{}", "AAAA".repeat(room / 4)).into();
            draft["text"] = "x".repeat(room % 4 + extra).into();
            assert_eq!(kuvo_wire::json::ordinary(&draft).len(), size + extra);
            if addressed {
                Action::Deliver {
                    message: json!({"draft":draft,"deliveryMode":"addressed"}),
                }
            } else {
                Action::Submit { draft }
            }
        };
        let request = Request {
            setup: setup.clone(),
            session: session.into(),
            operation: encode(&[5; 32]),
            submitter: Submitter::Computer,
            action: action(1),
        };
        let wait = Duration::from_secs(20);
        assert!(matches!(
            source.request(to, request.clone(), wait).await,
            Err(Error::TooLarge)
        ));
        let request = Request {
            action: action(0),
            ..request
        };
        let response = Reply {
            status: 202,
            body: json!({"receipt": "r".repeat(128 * 1024)}),
        };
        let receiving = async {
            let incoming = inbox.recv().await.unwrap();
            assert_eq!(incoming.peer(), from);
            assert_eq!(incoming.request(), &request);
            assert_eq!(destination.admit(&incoming).unwrap(), Admission::Allowed);
            destination
                .reply(&incoming, response.clone())
                .await
                .unwrap();
        };
        let asking = source.request(to, request.clone(), wait);
        let (reply, ()) = tokio::time::timeout(wait, async { tokio::join!(asking, receiving) })
            .await
            .unwrap();
        assert_eq!(reply.unwrap(), response);
    }
    relay.abort();
    assert!(relay.await.unwrap_err().is_cancelled());
    if let Some(bundle) = std::env::var_os("KUVO_OWNER_PARTS_TS") {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("frames.json");
        let captured = std::iter::from_fn(|| frames_seen.try_recv().ok()).collect::<Vec<_>>();
        kuvo_store::files::write_atomic(&file, &serde_json::to_vec(&captured).unwrap()).unwrap();
        let mut reference = std::process::Command::new("node");
        reference.env_clear();
        for name in ["PATH", "SYSTEMROOT", "WINDIR", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                reference.env(name, value);
            }
        }
        reference
            .env("HOME", directory.path())
            .env("USERPROFILE", directory.path())
            .env("KUVO_HOME", directory.path().join("box"));
        let result = reference
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/owner_parts.mjs"
            ))
            .arg(bundle)
            .arg(file)
            .status()
            .unwrap();
        assert!(result.success());
    }
}

#[tokio::test]
async fn forwarded_pairing_waits_for_current_grants_after_restart() {
    use kuvo_peers::{
        devices::{DeviceChange, DeviceFuture, DeviceRoster, Devices, ManagedDevices, Revocation},
        directory::Roster,
        ownership::{Handle, PairingState, SetupState},
        setup_lists::ResidentPeers,
    };
    use kuvo_server::{
        Caller,
        ssh::{Grant, IdentityRefusal, Pairings},
    };
    use kuvo_wire::keys::encode;
    use std::sync::Arc;

    struct Accepted(Arc<DeviceRoster>);
    impl Pairings for Accepted {
        fn identity(&self, _: &str) -> Result<[u8; 32], IdentityRefusal> {
            Err(IdentityRefusal::Unavailable)
        }
        fn paired(&self, _: &[u8; 32]) -> Option<Grant> {
            None
        }
    }
    impl ManagedDevices for Accepted {
        fn device_roster(&self) -> Arc<DeviceRoster> {
            self.0.clone()
        }
        fn permits_pairing(&self) -> bool {
            false
        }
        fn change<'a>(&'a self, _: &'a Caller, _: DeviceChange) -> DeviceFuture<'a> {
            Box::pin(async { Err(kuvo_peers::devices::Error::Disabled) })
        }
    }

    let folder = tempfile::tempdir().unwrap();
    let home = folder.path();
    let key = public([32; 32]);
    let here = public(RUST_KEY);
    let restart = || async {
        ResidentPeers::start(
            home,
            Roster {
                here: Some(here),
                main: Some(public(PEER_KEY)),
                name: "Member".into(),
                members: vec![],
            },
            None,
            None,
            false,
            Arc::new(|| 12),
        )
        .await
        .unwrap()
    };
    let grant = |token: &str| {
        serde_json::from_value(json!({"key":encode(&key),"kind":"desktop","pairing":token}))
            .unwrap()
    };
    let open = |grants: DeviceRoster, ready: bool| async move {
        let resident = restart().await;
        let devices = Devices::load(home, "Member".into(), ComputerKind::Machine)
            .await
            .unwrap()
            .devices
            .with_managed(Arc::new(Accepted(Arc::new(grants))));
        let handle = Handle::new(resident.reader(), None, devices, move || SetupState {
            id: Some(format!("set_{}", encode(&[1; 16]))),
            grants_current: ready,
            ..SetupState::default()
        });
        (resident, handle)
    };
    let (previous, before) = open(
        DeviceRoster {
            devices: vec![grant("old")],
            removed: vec![],
        },
        true,
    )
    .await;
    assert_eq!(before.current_pairing(&key, "old"), PairingState::Allowed);
    drop(previous);
    let (_stale, after) = open(
        DeviceRoster {
            devices: vec![grant("old")],
            removed: vec![],
        },
        false,
    )
    .await;
    assert_eq!(after.current_pairing(&key, "old"), PairingState::Checking);
    let current = DeviceRoster {
        devices: vec![grant("new")],
        removed: vec![Revocation {
            key: encode(&key),
            pairing: "old".into(),
        }],
    };
    let (_restarted, fresh) = open(current, true).await;
    assert_eq!(fresh.current_pairing(&key, "old"), PairingState::Revoked);
    assert_eq!(fresh.current_pairing(&key, "new"), PairingState::Allowed);
}

#[tokio::test]
async fn a_quiet_peers_move_disappears_from_a_followed_list() {
    use http::{HeaderMap, Method, StatusCode};
    use http_body_util::BodyExt;
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        setup_lists::ResidentPeers,
    };
    use kuvo_server::{Caller, DispatchRequest, LineTiming, LocalOrigin, Registry, Transport};
    use kuvo_wire::keys::encode;
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    use tokio_util::sync::CancellationToken;

    async fn next(body: &mut kuvo_server::Body) -> Value {
        let data = body.frame().await.unwrap().unwrap().into_data().unwrap();
        serde_json::from_slice(&data).unwrap()
    }

    tokio::time::timeout(Duration::from_secs(12), async {
        let home = tempfile::tempdir().unwrap();
        let stranger = [31; 32];
        let stranger_address = [32; 16];
        let (relay, mut wire) = TestRelay::connect_with(|config| {
            config.peers.push(Host {
                key: public(stranger),
                address: stranger_address,
            });
        })
        .await;
        let here = public(RUST_KEY);
        let peer = public(PEER_KEY);
        let other = public([23; 32]);
        let clock = Arc::new(AtomicI64::new(1_000_000));
        let resident = ResidentPeers::start(
            home.path(),
            Roster {
                here: Some(here),
                main: Some(here),
                name: "Observer".into(),
                members: vec![
                    Member {
                        key: peer,
                        address: Some(PEER_ADDRESS),
                        name: "Source".into(),
                        kind: ComputerKind::Machine,
                    },
                    Member {
                        key: other,
                        address: None,
                        name: "Destination".into(),
                        kind: ComputerKind::Machine,
                    },
                ],
            },
            Some(relay),
            None,
            false,
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::Relaxed))
            },
        )
        .await
        .unwrap();
        let devices = Devices::load(home.path(), "Observer".into(), ComputerKind::Machine)
            .await
            .unwrap()
            .devices;
        let mut routes = Registry::new();
        kuvo_peers::routes::register(&mut routes, resident.reader(), devices);
        let dispatcher = routes.build(LineTiming::default());
        let line = dispatcher
            .dispatch(
                DispatchRequest {
                    method: Method::GET,
                    path: "kuvo/moves/events".into(),
                    headers: HeaderMap::new(),
                    body: Default::default(),
                    caller: Caller::Local(Transport::Http {
                        origin: LocalOrigin::App,
                        internal: false,
                    }),
                },
                CancellationToken::new(),
            )
            .await;
        assert_eq!(line.status, StatusCode::OK);
        let mut body = line.body;
        assert!(next(&mut body).await.get("elsewhere").is_none());
        let move_notice = json!({
            "id": "mov_quiet", "kind": "chat", "thing": "ses_quiet", "name": "Notes",
            "from": encode(&peer), "to": encode(&other), "role": "from", "stage": "copying",
            "sessions": ["ses_quiet", 42], "sent": "unfinished",
        });
        let mut changed = move_notice.clone();
        changed["stage"] = "switching".into();
        let mut invalid = move_notice.clone();
        invalid["stage"] = "flying".into();
        let mut own_move = move_notice.clone();
        own_move["from"] = encode(&here).into();
        let unknown = json!({"at":10_000, "moving":{"moves":[move_notice.clone()]}});
        let unknown = Envelope(unknown.as_object().unwrap().clone())
            .seal(&stranger, &here)
            .unwrap();
        wire.send(frames::relay_send(&stranger_address, &unknown))
            .await;
        for (index, moves) in [
            json!([move_notice]),
            json!("all of them"),
            json!([invalid, own_move, changed]),
        ]
        .into_iter()
        .enumerate()
        {
            // The sender's clock changes, while the observing computer keeps receiving mail.
            let at = if index == 0 {
                10_000
            } else {
                -3_600_000 + index as i64
            };
            let given = json!({"at": at, "moving": {"moves": moves}});
            let bytes = Envelope(given.as_object().unwrap().clone())
                .seal(&PEER_KEY, &here)
                .unwrap();
            wire.send(frames::relay_send(&PEER_ADDRESS, &bytes)).await;
            if index != 1 {
                let list = next(&mut body).await;
                assert_eq!(list["elsewhere"].as_array().unwrap().len(), 1);
                let row = &list["elsewhere"][0];
                assert_eq!(row["box"], encode(&peer));
                assert_eq!(row["moves"].as_array().unwrap().len(), 1);
                let shown = &row["moves"][0];
                assert_eq!(shown["id"], "mov_quiet");
                assert_eq!(
                    shown["stage"],
                    if index == 0 { "copying" } else { "switching" }
                );
                assert_eq!(shown["sessions"], json!(["ses_quiet"]));
                assert!(shown.get("sent").is_none());
            }
        }
        clock.store(1_180_000, Ordering::Relaxed);
        let expired = next(&mut body).await;
        assert!(expired.get("elsewhere").is_none());
        assert_eq!(expired["moves"], json!([]));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn move_snapshots_refresh_on_a_minute_and_on_request() {
    use kuvo_peers::{
        directory::{Member, Roster},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    tokio::time::timeout(Duration::from_secs(55), async {
        let home = tempfile::tempdir().unwrap();
        let (relay, mut wire, _, listener) = TestRelay::reconnecting(|_| {}).await;
        let here = public(RUST_KEY);
        let peer = public(PEER_KEY);
        let clock = Arc::new(AtomicI64::new(1_000_000));
        let resident = ResidentPeers::start(
            home.path(),
            Roster {
                here: Some(here),
                main: Some(here),
                name: "Receiver".into(),
                members: vec![Member {
                    key: peer,
                    address: Some(PEER_ADDRESS),
                    name: "Sender".into(),
                    kind: ComputerKind::Machine,
                }],
            },
            Some(relay),
            None,
            false,
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::Relaxed))
            },
        )
        .await
        .unwrap();
        assert_eq!(wire.moving().await, json!([]));

        let path = home.path().join("run/moves/mov_report.json");
        let mut record = json!({
            "formatVersion":1, "id":"mov_report", "thing":"ses_report", "kind":"chat",
            "name":"Notes", "from":encode(&peer), "to":encode(&here), "peer":encode(&peer),
            "role":"to", "stage":"copying", "sessions":["ses_report"], "started":1_000_000,
            "size":100, "sent":10,
        });
        let save = |record: &Value| {
            files::write_atomic(&path, &serde_json::to_vec(record).unwrap()).unwrap()
        };
        save(&record);
        assert_eq!(wire.moving().await[0]["sent"], 10);

        record["sent"] = 50.into();
        save(&record);
        clock.store(1_059_999, Ordering::Relaxed);
        let check = Duration::from_millis(3500);
        assert!(
            tokio::time::timeout(check, wire.moving()).await.is_err(),
            "More bytes alone do not publish a move"
        );
        clock.store(1_060_000, Ordering::Relaxed);
        assert_eq!(wire.moving().await[0]["sent"], 50);

        let ask = json!({"at":1_060_001, "ask":"agents"});
        let bytes = Envelope(ask.as_object().unwrap().clone())
            .seal(&PEER_KEY, &here)
            .unwrap();
        wire.send(frames::relay_send(&PEER_ADDRESS, &bytes)).await;
        assert_eq!(wire.moving().await[0]["id"], "mov_report");

        record["stage"] = "switching".into();
        save(&record);
        assert_eq!(wire.moving().await[0]["stage"], "switching");
        files::remove_file_synced(&path).unwrap();
        assert_eq!(wire.moving().await, json!([]));
        clock.store(1_120_000, Ordering::Relaxed);
        assert!(
            tokio::time::timeout(check, wire.moving()).await.is_err(),
            "The clear is published once"
        );

        // A disconnected relay rejects the publication. Keeping the wall
        // clock steady distinguishes retrying it from a minute's refresh.
        let mut reader = resident.reader();
        drop(wire);
        while reader.view().relay_connected {
            reader.changed().await.unwrap();
        }
        record["stage"] = "copying".into();
        save(&record);
        tokio::time::sleep(check).await;
        let mut wire = TestRelay::accept(&listener, RUST_SECRET).await;
        assert_eq!(wire.moving().await[0]["stage"], "copying");

        drop(wire);
        while reader.view().relay_connected {
            reader.changed().await.unwrap();
        }
        files::remove_file_synced(&path).unwrap();
        tokio::time::sleep(check).await;
        let mut wire = TestRelay::accept(&listener, RUST_SECRET).await;
        assert_eq!(wire.moving().await, json!([]));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_saved_letter_retries_each_minute_until_its_recipient_replies() {
    use kuvo_engine::{Engine, Timing};
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Handle, SetupState},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::{Host, encode};
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    let home = tempfile::tempdir().unwrap();
    let run = home.path().join("run");
    let here = public(RUST_KEY);
    let peer = public(PEER_KEY);
    let other = public([23; 32]);
    let other_address = [7; 16];
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let id = "ltr_waitingAgain";
    let pending = run.join(format!("outbox/sending/{id}.json"));
    let waiting = run.join(format!("replies/{id}.wait"));
    let reply = run.join(format!("replies/{id}.json"));
    let text = "😀".repeat(30_000);
    let letter =
        json!({"to":"ses_destination","peer":encode(&peer),"text":text,"at":1_000_000,"wait":true});
    files::write_atomic(&pending, &serde_json::to_vec(&letter).unwrap()).unwrap();
    files::write_atomic(&run.join(format!("replies/{id}.alive")), b"").unwrap();

    let (relay, mut wire, _) = TestRelay::welcome(|config| {
        config.peers.push(Host {
            key: other,
            address: other_address,
        })
    })
    .await;
    let mut resident = ResidentPeers::start(
        home.path(),
        Roster {
            here: Some(here),
            main: Some(here),
            name: "Source".into(),
            members: vec![
                Member {
                    key: peer,
                    address: Some(PEER_ADDRESS),
                    name: "Recipient".into(),
                    kind: ComputerKind::Machine,
                },
                Member {
                    key: other,
                    address: Some(other_address),
                    name: "Another computer".into(),
                    kind: ComputerKind::Machine,
                },
            ],
        },
        Some(relay),
        None,
        false,
        {
            let clock = clock.clone();
            Arc::new(move || clock.load(Ordering::Relaxed))
        },
    )
    .await
    .unwrap();
    let devices = Devices::load(home.path(), "Source".into(), ComputerKind::Machine)
        .await
        .unwrap()
        .devices;
    let ownership = Handle::new(
        resident.reader(),
        resident.relay_handle(),
        devices,
        SetupState::default,
    );
    let engine = Engine::new(
        home.path().to_owned(),
        Default::default(),
        Timing::default(),
    )
    .unwrap();
    resident.attach_moves(engine, ownership).await.unwrap();
    let sealed = |fields: Value, key, address| {
        let bytes = Envelope(fields.as_object().unwrap().clone())
            .seal(&key, &here)
            .unwrap();
        frames::relay_send(&address, &bytes)
    };

    let first = tokio::time::timeout(Duration::from_secs(10), wire.mailed("letter"))
        .await
        .unwrap();
    assert_eq!(first["id"], id);
    assert_eq!(first["wait"], true);
    let prefix = first["text"].as_str().unwrap();
    assert!(text.starts_with(prefix) && !prefix.is_empty() && prefix.len() <= 24_000);
    tokio::time::pause();
    clock.store(1_059_000, Ordering::Relaxed);
    assert!(
        tokio::time::timeout(Duration::from_secs(4), wire.mailed("letter"))
            .await
            .is_err()
    );
    wire.send(sealed(json!({"at":1_059_000,"delivered":{"id":id,"ok":false,"why":"The engine is restarting.","retry":true}}), PEER_KEY, PEER_ADDRESS)).await;
    wire.send(sealed(
        json!({"at":1_059_000,"delivered":{"id":id,"ok":true}}),
        [23; 32],
        other_address,
    ))
    .await;
    clock.store(1_061_000, Ordering::Relaxed);
    tokio::time::advance(Duration::from_secs(58)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(6), wire.mailed("letter"))
            .await
            .unwrap(),
        first
    );
    assert!(pending.exists());
    assert!(!waiting.exists());

    wire.send(vec![0]).await;
    clock.store(1_122_000, Ordering::Relaxed);
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(6), wire.mailed("letter"))
            .await
            .unwrap(),
        first
    );
    wire.send(sealed(
        json!({"at":1_122_000,"delivered":{"id":id,"ok":true}}),
        PEER_KEY,
        PEER_ADDRESS,
    ))
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !waiting.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!pending.exists());
    assert!(!reply.exists());
    wire.send(sealed(json!({"at":1_122_000,"letter":{"id":"ltr_answerAfterRetries","from":"ses_destination","to":"ses_source","text":"Done.","answers":id,"at":1_122_000}}), PEER_KEY, PEER_ADDRESS)).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !reply.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let answer: Value = serde_json::from_slice(&std::fs::read(reply).unwrap()).unwrap();
    assert_eq!(answer["text"], "Done.");
    assert!(!waiting.exists());

    let unheard = "ltr_beforeReceipt";
    let pending = run.join(format!("outbox/sending/{unheard}.json"));
    let reply = run.join(format!("replies/{unheard}.json"));
    let waiting = run.join(format!("replies/{unheard}.wait"));
    files::write_atomic(&pending, &serde_json::to_vec(&letter).unwrap()).unwrap();
    files::write_atomic(&run.join(format!("replies/{unheard}.alive")), b"").unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(6), wire.mailed("letter"))
            .await
            .unwrap()["id"],
        unheard
    );
    wire.send(sealed(json!({"at":1_122_000,"letter":{"id":"ltr_answerWithoutReceipt","from":"ses_destination","to":"ses_source","text":"Done without a receipt.","answers":unheard,"at":1_122_000}}), PEER_KEY, PEER_ADDRESS)).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !reply.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let answer: Value = serde_json::from_slice(&std::fs::read(reply).unwrap()).unwrap();
    assert_eq!(answer["text"], "Done without a receipt.");
    assert!(!pending.exists());
    assert!(!waiting.exists());
}

#[tokio::test]
async fn incomplete_and_complete_copies_wait_on_different_deadlines() {
    use kuvo_engine::{Engine, Timing};
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Handle, SetupState},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    fn waiting(moves: &Value, id: &str) -> bool {
        moves
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .unwrap()["waiting"]
            == true
    }

    tokio::time::timeout(Duration::from_secs(300), async {
        let home = tempfile::tempdir().unwrap();
        let (relay, mut wire) = TestRelay::connect().await;
        let here = public(RUST_KEY);
        let peer = public(PEER_KEY);
        let clock = Arc::new(AtomicI64::new(1_000_000));
        let folder = home.path().join("run/moves");
        let hash = encode(&[9; 32]);
        for (id, complete) in [("mov_partial", false), ("mov_complete", true)] {
            let kept = json!({
                "formatVersion": 1, "id": id, "kind": "chat", "thing": "ses_notice", "name": id,
                "from": encode(&peer), "to": encode(&here), "peer": encode(&peer),
                "role": "to", "stage": "copying", "sessions": ["ses_notice"], "started": 1_000_000,
                "complete": complete, "setUpAt": complete.then_some(1_000_000), "generation": 1, "hash": hash,
            });
            files::write_atomic(&folder.join(format!("{id}.json")), &serde_json::to_vec(&kept).unwrap())
                .unwrap();
        }
        let mut resident = ResidentPeers::start(
            home.path(),
            Roster {
                here: Some(here),
                main: Some(here),
                name: "Receiver".into(),
                members: vec![Member {
                    key: peer,
                    address: Some(PEER_ADDRESS),
                    name: "Sender".into(),
                    kind: ComputerKind::Machine,
                }],
            },
            Some(relay),
            None,
            false,
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::Relaxed))
            },
        )
        .await
        .unwrap();
        let devices = Devices::load(home.path(), "Receiver".into(), ComputerKind::Machine)
            .await
            .unwrap()
            .devices;
        let owner = Handle::new(resident.reader(), resident.relay_handle(), devices, SetupState::default);
        let engine = Engine::new(home.path().to_owned(), Default::default(), Timing::default()).unwrap();
        resident.attach_moves(engine, owner).await.unwrap();

        let initial = tokio::time::timeout(Duration::from_secs(8), wire.moving())
            .await
            .unwrap();
        assert!(!waiting(&initial, "mov_partial"));
        assert!(!waiting(&initial, "mov_complete"));
        tokio::time::pause();
        // Keep the relay connection alive. Only this move's peer is quiet.
        clock.store(1_060_000, Ordering::Relaxed);
        wire.send(vec![0]).await;
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(!waiting(&wire.moving().await, "mov_partial"));

        clock.store(1_120_000, Ordering::Relaxed);
        wire.send(vec![0]).await;
        tokio::time::advance(Duration::from_secs(60)).await;
        let partial_waiting = loop {
            let seen = wire.moving().await;
            if waiting(&seen, "mov_partial") {
                break seen;
            }
        };
        assert!(!waiting(&partial_waiting, "mov_complete"));

        clock.store(1_180_000, Ordering::Relaxed);
        wire.send(vec![0]).await;
        tokio::time::advance(Duration::from_secs(60)).await;
        let both = loop {
            let seen = wire.moving().await;
            if waiting(&seen, "mov_complete") {
                break seen;
            }
        };
        assert!(waiting(&both, "mov_partial"));
        let heard = json!({"at": 1_180_000, "move": {
            "id":"mov_partial", "op":"working", "generation":1, "hash":hash,
        }});
        let bytes = Envelope(heard.as_object().unwrap().clone())
            .seal(&PEER_KEY, &here).unwrap();
        wire.send(frames::relay_send(&PEER_ADDRESS, &bytes)).await;
        let heard_again = loop {
            let seen = wire.moving().await;
            if !waiting(&seen, "mov_partial") {
                break seen;
            }
        };
        assert!(waiting(&heard_again, "mov_complete"));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn switch_retry_survives_wall_clock_changes() {
    use kuvo_engine::{Engine, Timing};
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Handle, SetupState},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    tokio::time::timeout(Duration::from_secs(20 * 60), async {
        let home = tempfile::tempdir().unwrap();
        let (relay, mut wire) = TestRelay::connect().await;
        let here = public(RUST_KEY);
        let peer = public(PEER_KEY);
        let clock = Arc::new(AtomicI64::new(1_000_000));
        let id = "mov_clockSwitch";
        let session = "ses_clockSwitch";
        let hash = encode(&[13; 32]);
        let journal = home.path().join(format!("run/moves/{id}.json"));
        let wake = home.path().join("wakes").join(session);
        files::write_atomic(&wake.join("paused"), format!("moving:{id}").as_bytes()).unwrap();
        let saved = json!({
            "formatVersion": 1, "id": id, "kind": "chat", "thing": session,
            "name": "Clock switch", "from": encode(&here), "to": encode(&peer), "peer": encode(&peer),
            "role": "from", "stage": "switching", "step": "switch", "sessions": [session],
            "started": 920_000, "switchSince": 950_000, "sentAt": 1_050_000, "generation": 1, "hash": hash,
            "folder":wake,
        });
        files::write_atomic(&journal, &serde_json::to_vec(&saved).unwrap()).unwrap();
        let mut resident = ResidentPeers::start(
            home.path(),
            Roster {
                here: Some(here),
                main: Some(here),
                name: "Source".into(),
                members: vec![Member {
                    key: peer,
                    address: Some(PEER_ADDRESS),
                    name: "Destination".into(),
                    kind: ComputerKind::Machine,
                }],
            },
            Some(relay),
            None,
            false,
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::Relaxed))
            },
        )
        .await
        .unwrap();
        let devices = Devices::load(home.path(), "Source".into(), ComputerKind::Machine)
            .await
            .unwrap()
            .devices;
        let owner = Handle::new(resident.reader(), resident.relay_handle(), devices, SetupState::default);
        let engine = Engine::new(home.path().to_owned(), Default::default(), Timing::default()).unwrap();
        resident.attach_moves(engine, owner).await.unwrap();

        let first = wire.move_step("switch").await;
        assert_eq!(first["id"], id);
        assert_eq!(first["hash"], hash);
        assert_eq!(first["generation"], 1);
        let sent_at = || serde_json::from_slice::<Value>(&std::fs::read(&journal).unwrap()).unwrap()["sentAt"].clone();
        assert_eq!(sent_at(), 1_000_000);

        tokio::time::pause();
        clock.store(1_000_001, Ordering::Relaxed);
        assert!(tokio::time::timeout(Duration::from_secs(10), wire.move_step("switch")).await.is_err());
        assert_eq!(sent_at(), 1_000_000);
        wire.send(vec![0]).await;
        tokio::time::advance(Duration::from_secs(52)).await;
        assert_eq!(wire.move_step("switch").await, first);
        assert_eq!(sent_at(), 1_000_001);
        assert!(tokio::time::timeout(Duration::from_secs(1), wire.move_step("switch")).await.is_err());

        clock.store(990_000, Ordering::Relaxed);
        wire.send(vec![0]).await;
        assert_eq!(tokio::time::timeout(Duration::from_secs(6), wire.move_step("switch")).await.unwrap(), first);
        assert_eq!(sent_at(), 990_000);

        let proof = |generation, never_switched| {
            let envelope = json!({"at": clock.load(Ordering::Relaxed), "move": {
                "id":id, "op":"dropped", "generation":generation, "hash":hash, "neverSwitched":never_switched,
            }});
            let bytes = Envelope(envelope.as_object().unwrap().clone()).seal(&PEER_KEY, &here).unwrap();
            frames::relay_send(&PEER_ADDRESS, &bytes)
        };
        for minute in 1..=11 {
            clock.store(990_000 + minute * 61_000, Ordering::Relaxed);
            wire.send(vec![0]).await;
            tokio::time::advance(Duration::from_secs(61)).await;
            assert_eq!(tokio::time::timeout(Duration::from_secs(6), wire.next_move()).await.unwrap(), first);
            assert!(journal.exists());
            assert!(wake.join("paused").exists());
        }
        wire.send(proof(2, true)).await;
        wire.send(proof(1, false)).await;
        wire.send(vec![0]).await;
        tokio::time::advance(Duration::from_secs(61)).await;
        assert_eq!(tokio::time::timeout(Duration::from_secs(6), wire.next_move()).await.unwrap(), first);
        assert!(wake.join("paused").exists());
        wire.send(proof(1, true)).await;
        tokio::time::timeout(Duration::from_secs(6), async {
            while journal.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert!(!wake.join("paused").exists());
        let failed: Value = serde_json::from_slice(&std::fs::read(home.path().join(format!("run/moves/{id}.outcome.json"))).unwrap()).unwrap();
        assert_eq!(failed["status"], 409);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn dropped_copies_retry_each_minute_then_each_hour() {
    use kuvo_engine::{Engine, Timing};
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Handle, SetupState},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        sync::mpsc,
    };

    const MINUTE: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(3600);
    const NOW: i64 = 10 * 24 * 3_600_000;
    let home = tempfile::tempdir().unwrap();
    let here = public(RUST_KEY);
    let peer = public(PEER_KEY);
    let hash = encode(&[17; 32]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let run = home.path().join("run");
    let endpoint =
        json!({"pid":std::process::id(),"port":port,"password":"fixture","version":"fixture"});
    files::write_atomic(
        &run.join("engine.json"),
        &serde_json::to_vec(&endpoint).unwrap(),
    )
    .unwrap();
    let copies = [
        ("mov_recentDrop", "ses_recentDrop", NOW - 5000),
        (
            "mov_overdueDrop",
            "ses_overdueDrop",
            NOW - 8 * 24 * 3_600_000,
        ),
    ];
    for (id, session, since) in copies {
        let row = json!({
            "formatVersion":1, "id":id, "kind":"chat", "thing":session, "name":session,
            "from":encode(&peer), "to":encode(&here), "peer":encode(&peer), "role":"to",
            "stage":"copying", "sessions":[session], "complete":true, "setUpAt":since,
            "dropping":true, "dropSince":since, "started":since, "safeSwitch":true,
            "twoComputers":true, "generation":1, "hash":hash, "createdSessions":[session],
        });
        files::write_atomic(
            &run.join("moves").join(format!("{id}.json")),
            &serde_json::to_vec(&row).unwrap(),
        )
        .unwrap();
    }
    let (delete, mut deletions) = mpsc::unbounded_channel();
    let hashes = hash.clone();
    let server = tokio::spawn(async move {
        let mut attempted = std::collections::HashSet::new();
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut head = Vec::new();
            let mut buffer = [0u8; 1024];
            while !head.ends_with(b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                head.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8_lossy(&head);
            let mut parts = request.split_whitespace();
            let (Some(method), Some(route)) = (parts.next(), parts.next()) else {
                continue;
            };
            let session = route.rsplit('/').next().unwrap();
            let Some((id, session, _)) = copies.iter().find(|(_, found, _)| *found == session)
            else {
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}").await;
                continue;
            };
            let (status, body) = if method == "DELETE" {
                let first = attempted.insert(*session);
                delete.send(*session).unwrap();
                (
                    if first {
                        "503 Service Unavailable"
                    } else {
                        "200 OK"
                    },
                    json!({}),
                )
            } else {
                (
                    "200 OK",
                    json!({"data":{"id":session, "metadata":{"kuvo.moveImport":format!("{id}:1:{hashes}")}}}),
                )
            };
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let (relay, _wire) = TestRelay::connect().await;
    let mut resident = ResidentPeers::start(
        home.path(),
        Roster {
            here: Some(here),
            main: Some(here),
            name: "Destination".into(),
            members: vec![Member {
                key: peer,
                address: Some(PEER_ADDRESS),
                name: "Source".into(),
                kind: ComputerKind::Machine,
            }],
        },
        Some(relay),
        None,
        false,
        Arc::new(|| NOW),
    )
    .await
    .unwrap();
    let devices = Devices::load(home.path(), "Destination".into(), ComputerKind::Machine)
        .await
        .unwrap()
        .devices;
    let owner = Handle::new(
        resident.reader(),
        resident.relay_handle(),
        devices,
        SetupState::default,
    );
    let engine = Engine::new(
        home.path().to_owned(),
        Default::default(),
        Timing::default(),
    )
    .unwrap();
    let probe = tokio::time::timeout(Duration::from_secs(5), engine.session_move())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        probe.lookup("ses_recentDrop").await.unwrap().unwrap()["id"],
        "ses_recentDrop"
    );
    drop(probe);
    resident.attach_moves(engine, owner).await.unwrap();

    let mut first = vec![];
    for _ in copies {
        first.push(
            tokio::time::timeout(Duration::from_secs(10), deletions.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    first.sort_unstable();
    assert_eq!(first, ["ses_overdueDrop", "ses_recentDrop"]);
    tokio::time::pause();
    assert!(
        tokio::time::timeout(MINUTE - Duration::from_secs(2), deletions.recv())
            .await
            .is_err()
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(6), deletions.recv())
            .await
            .unwrap(),
        Some("ses_recentDrop")
    );
    tokio::time::advance(HOUR - 2 * MINUTE).await;
    assert!(deletions.try_recv().is_err());
    assert_eq!(
        tokio::time::timeout(2 * MINUTE, deletions.recv())
            .await
            .unwrap(),
        Some("ses_overdueDrop")
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        for (id, _, _) in copies {
            let journal = run.join("moves").join(format!("{id}.json"));
            while journal.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    })
    .await
    .unwrap();
    server.abort();
}

#[tokio::test]
async fn a_quiet_peer_keeps_the_original_until_the_transfer_deadline() {
    source_deadline(SourceDeadline::Parts).await;
}

#[tokio::test]
async fn a_slow_preparation_releases_the_original_at_the_move_deadline() {
    source_deadline(SourceDeadline::Preparation).await;
}

#[tokio::test]
async fn a_quiet_peer_can_retry_a_second_offer_until_the_transfer_deadline() {
    source_deadline(SourceDeadline::SecondOffer).await;
}

#[tokio::test]
async fn an_undriven_move_can_wait_half_an_hour_without_a_hold_time() {
    source_deadline(SourceDeadline::Undriven).await;
}

#[tokio::test]
async fn a_part_rejected_by_the_disconnected_relay_goes_on_the_next_round() {
    source_deadline(SourceDeadline::RejectedPart).await;
}

#[tokio::test]
async fn a_legacy_setup_can_try_an_unannounced_peer_as_a_destination() {
    source_deadline(SourceDeadline::UnknownPeer).await;
}

#[derive(PartialEq)]
enum SourceDeadline {
    Parts,
    Preparation,
    SecondOffer,
    Undriven,
    RejectedPart,
    UnknownPeer,
}

async fn source_deadline(deadline: SourceDeadline) {
    use kuvo_engine::{Engine, Timing};
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Handle, SetupState},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let home = tempfile::tempdir().unwrap();
    let run = home.path().join("run");
    let here = public(RUST_KEY);
    let peer = public(PEER_KEY);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let session = "ses_waitingSource";
    let id = "mov_waitingSource";
    let wake = home.path().join("wakes").join(session);
    std::fs::create_dir_all(&wake).unwrap();
    std::fs::write(wake.join("answer"), b"keep this").unwrap();
    let endpoint =
        json!({"pid":std::process::id(),"port":port,"password":"fixture","version":"fixture"});
    files::write_atomic(
        &run.join("engine.json"),
        &serde_json::to_vec(&endpoint).unwrap(),
    )
    .unwrap();
    let journal = run.join("moves").join(format!("{id}.json"));
    let row = json!({
        "formatVersion":1, "id":id, "kind":"chat", "thing":session, "name":"Quiet chat",
        "from":encode(&here), "to":encode(&peer), "peer":encode(&peer), "role":"from",
        "stage":"holding", "step":"check", "sessions":[session], "started":1_000_000,
        "twoComputers":true,
    });
    if deadline != SourceDeadline::UnknownPeer {
        files::write_atomic(&journal, &serde_json::to_vec(&row).unwrap()).unwrap();
    }
    let (exported, mut exports) = tokio::sync::mpsc::unbounded_channel();
    let export_allowed = Arc::new(tokio::sync::Semaphore::new(0));
    let permit = export_allowed.clone();
    let changed = Arc::new(AtomicBool::new(false));
    let needs_second_offer = changed.clone();
    let hold_check = Arc::new(AtomicBool::new(false));
    let blocks_check = hold_check.clone();
    let slow_exports = deadline == SourceDeadline::Preparation;
    let chats = home.path().join("chats");
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let mut socket = BufReader::new(socket);
            let mut request = String::new();
            if socket.read_line(&mut request).await.unwrap() == 0 {
                continue;
            }
            let mut header = String::new();
            while socket.read_line(&mut header).await.unwrap() > 2 {
                header.clear();
            }
            let route = request.split_whitespace().nth(1).unwrap_or_default();
            assert!(!request.starts_with("DELETE"));
            if route == "/api/session/active" && blocks_check.swap(false, Ordering::Relaxed) {
                exported.send(()).unwrap();
                permit.acquire().await.unwrap().forget();
            }
            let body = if let Some(session_id) = route
                .strip_prefix("/api/experimental/session/")
                .and_then(|route| route.strip_suffix("/export"))
            {
                if slow_exports {
                    exported.send(()).unwrap();
                    permit.acquire().await.unwrap().forget();
                }
                let mut info = json!({"id":session_id, "title":"Quiet chat"});
                if session_id != session {
                    info["parentID"] = session.into();
                }
                let messages = if changed.load(Ordering::Relaxed) {
                    json!([{"info":{"id":"msg_newLetter","role":"user"},"parts":[{"type":"text","text":"I found one more thing."}]}])
                } else {
                    json!([])
                };
                json!({"data":{"info":info,"messages":messages}})
            } else if slow_exports
                && route.starts_with(&format!("/api/session?parentID={session}&"))
            {
                json!({"data":[{"id":"ses_slowerOne"},{"id":"ses_slowerTwo"},{"id":"ses_slowerThree"}]})
            } else if route == format!("/api/session/{session}") {
                json!({"data":{"id":session,"title":"Quiet chat","directory":chats}})
            } else {
                json!({"data":[]})
            };
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let (relay, mut wire, _, listener) = TestRelay::reconnecting(|_| {}).await;
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let mut resident = ResidentPeers::start(
        home.path(),
        Roster {
            here: Some(here),
            main: Some(here),
            name: "Source".into(),
            members: vec![Member {
                key: peer,
                address: Some(PEER_ADDRESS),
                name: "Destination".into(),
                kind: ComputerKind::Machine,
            }],
        },
        Some(relay),
        None,
        false,
        {
            let clock = clock.clone();
            Arc::new(move || clock.load(Ordering::Relaxed))
        },
    )
    .await
    .unwrap();
    let devices = Devices::load(home.path(), "Source".into(), ComputerKind::Machine)
        .await
        .unwrap()
        .devices;
    let owner = Handle::new(
        resident.reader(),
        resident.relay_handle(),
        devices,
        SetupState::default,
    );
    let engine = Engine::new(
        home.path().to_owned(),
        Default::default(),
        Timing::default(),
    )
    .unwrap();
    let outcome = run.join("moves").join(format!("{id}.outcome.json"));
    if deadline == SourceDeadline::Undriven {
        let mut reader = resident.reader();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !reader.view().relay_connected {
                reader.changed().await.unwrap();
            }
            drop(wire);
            while reader.view().relay_connected {
                reader.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        resident.attach_moves(engine, owner).await.unwrap();
        tokio::time::pause();
        clock.store(1_000_000 + 29 * 60_000, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(journal.exists(), "The move was retired before half an hour");
        assert!(!outcome.exists());
        assert!(!wake.join("paused").exists());

        clock.store(1_000_000 + 31 * 60_000, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !outcome.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let failed: Value = serde_json::from_slice(&std::fs::read(outcome).unwrap()).unwrap();
        assert_eq!(failed["status"], 409);
        assert!(
            failed["message"]
                .as_str()
                .unwrap()
                .contains("took too long")
        );
        assert!(!wake.join("paused").exists());
        assert_eq!(std::fs::read(wake.join("answer")).unwrap(), b"keep this");
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }
    resident.attach_moves(engine.clone(), owner).await.unwrap();
    if deadline == SourceDeadline::UnknownPeer {
        use http::{HeaderMap, Method, StatusCode};
        use http_body_util::BodyExt;
        use kuvo_server::{Caller, DispatchRequest, LineTiming, LocalOrigin, Registry, Transport};
        use tokio_util::sync::CancellationToken;

        let mut reader = resident.reader();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !reader.view().relay_connected {
                reader.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let mut registry = Registry::new();
        resident.register_moves(&mut registry, engine);
        let dispatcher = registry.build(LineTiming::default());
        let place = dispatcher
            .dispatch(
                DispatchRequest {
                    method: Method::GET,
                    path: format!("kuvo/moves/places?kind=chat&id={session}"),
                    headers: HeaderMap::new(),
                    body: Default::default(),
                    caller: Caller::Local(Transport::Http {
                        origin: LocalOrigin::App,
                        internal: false,
                    }),
                },
                CancellationToken::new(),
            )
            .await;
        assert_eq!(place.status, StatusCode::OK);
        let body = place.body.collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["places"][0]["key"], encode(&peer));
        assert_eq!(body["places"][0]["ok"], true);
        let reply = dispatcher
            .dispatch(
                DispatchRequest {
                    method: Method::POST,
                    path: "kuvo/moves".into(),
                    headers: HeaderMap::new(),
                    body: json!({"kind":"chat","id":session,"to":encode(&peer),"receipt":true})
                        .to_string()
                        .into(),
                    caller: Caller::Local(Transport::Http {
                        origin: LocalOrigin::App,
                        internal: false,
                    }),
                },
                CancellationToken::new(),
            )
            .await;
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        let offer = tokio::time::timeout(Duration::from_secs(5), wire.move_step("offer"))
            .await
            .unwrap();
        assert_eq!(offer["thing"], session);
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }

    let mail = |movement: Value| {
        let packet = json!({"at":clock.load(Ordering::Relaxed), "move":movement});
        let bytes = Envelope(packet.as_object().unwrap().clone())
            .seal(&PEER_KEY, &here)
            .unwrap();
        frames::relay_send(&PEER_ADDRESS, &bytes)
    };
    let check = tokio::time::timeout(Duration::from_secs(10), wire.move_step("check"))
        .await
        .unwrap();
    assert_eq!(check["id"], id);
    wire.send(mail(json!({"id":id,"op":"checked","ok":true})))
        .await;
    if slow_exports {
        tokio::time::timeout(Duration::from_secs(10), exports.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(wake.join("paused")).unwrap(),
            format!("moving:{id}")
        );
        tokio::time::pause();
        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(500)).await;
            export_allowed.add_permits(1);
            tokio::time::timeout(Duration::from_secs(2), exports.recv())
                .await
                .unwrap()
                .unwrap();
        }
        tokio::time::advance(Duration::from_secs(301)).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !outcome.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("The move did not stop after thirty minutes of preparation");
        let failed: Value = serde_json::from_slice(&std::fs::read(outcome).unwrap()).unwrap();
        assert_eq!(failed["status"], 409);
        assert_eq!(
            failed["message"],
            "Quiet chat took too long to get ready to move on this machine, so it stayed here. Try again in a moment."
        );
        assert!(!wake.join("paused").exists());
        assert_eq!(std::fs::read(wake.join("answer")).unwrap(), b"keep this");
        assert!(!run.join("moves").join(id).join("bundle.tgz").exists());
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }
    let offer = tokio::time::timeout(Duration::from_secs(10), wire.move_step("offer"))
        .await
        .unwrap();
    let step = |op| {
        json!({"id":id,"op":op,"hash":offer["hash"],"generation":offer["generation"],
        "safeSwitch":true,"verifyPayloads":true,"typedAttachmentPaths":true})
    };
    if deadline == SourceDeadline::RejectedPart {
        hold_check.store(true, Ordering::Relaxed);
        wire.send(mail(
            json!({"id":"mov_blockingCheck","op":"check","kind":"chat","name":"Check"}),
        ))
        .await;
        tokio::time::timeout(Duration::from_secs(10), exports.recv())
            .await
            .unwrap()
            .unwrap();
        // The resident receives ready while its queue is occupied. It can
        // only act on ready once the real relay connection has closed.
        wire.send(mail(step("ready"))).await;
        drop(wire);
        let mut reader = resident.reader();
        tokio::time::timeout(Duration::from_secs(10), async {
            while reader.view().relay_connected {
                reader.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        export_allowed.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let row: Value = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
                if row["step"] == "parts" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let mut wire = tokio::time::timeout(
            Duration::from_secs(7),
            TestRelay::accept(&listener, RUST_SECRET),
        )
        .await
        .unwrap();
        let part = tokio::time::timeout(Duration::from_secs(5), wire.move_step("part"))
            .await
            .unwrap();
        assert_eq!(part["i"], 0);
        assert_eq!(part["hash"], offer["hash"]);
        let mut got = step("got");
        got["i"] = 0.into();
        wire.send(mail(got)).await;
        wire.send(mail(step("arrived"))).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), wire.move_step("switch"))
                .await
                .unwrap()["hash"],
            offer["hash"]
        );
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }
    wire.send(mail(step("ready"))).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), wire.move_step("part"))
            .await
            .unwrap()["i"],
        0
    );

    if deadline == SourceDeadline::SecondOffer {
        let mut got = step("got");
        got["i"] = 0.into();
        wire.send(mail(got)).await;
        needs_second_offer.store(true, Ordering::Relaxed);
        wire.send(mail(step("arrived"))).await;
        let second = tokio::time::timeout(Duration::from_secs(10), wire.move_step("offer"))
            .await
            .unwrap();
        assert_eq!(second["generation"], 2);
        assert_ne!(second["hash"], offer["hash"]);
        tokio::time::pause();
        for seconds in [61, 122] {
            clock.store(1_000_000 + seconds * 1000, Ordering::Relaxed);
            wire.send(vec![0]).await;
            tokio::time::advance(Duration::from_secs(61)).await;
            let retried = tokio::time::timeout(Duration::from_secs(12), wire.move_step("offer"))
                .await
                .unwrap();
            assert_eq!(retried, second);
            assert!(!outcome.exists());
        }
    } else {
        let waiting = |shown: &Value| {
            shown
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .map(|row| row["waiting"] == true)
        };
        tokio::time::pause();
        clock.store(1_094_000, Ordering::Relaxed);
        wire.send(vec![0]).await;
        tokio::time::advance(Duration::from_secs(94)).await;
        tokio::time::timeout(Duration::from_secs(12), async {
            while waiting(&wire.moving().await) != Some(true) {}
        })
        .await
        .unwrap();
        wire.send(mail(step("working"))).await;
        tokio::time::timeout(Duration::from_secs(12), async {
            while waiting(&wire.moving().await) != Some(false) {}
        })
        .await
        .unwrap();
    }

    clock.store(1_699_000, Ordering::Relaxed);
    wire.send(vec![0]).await;
    tokio::time::advance(Duration::from_secs(605)).await;
    tokio::time::timeout(Duration::from_secs(12), async {
        while !outcome.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let failed: Value = serde_json::from_slice(&std::fs::read(outcome).unwrap()).unwrap();
    assert_eq!(failed["status"], 503);
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .contains("stopped answering")
    );
    let parsed: Value = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    assert_eq!(parsed["step"], "drop");
    assert!(!wake.join("paused").exists());
    assert_eq!(std::fs::read(wake.join("answer")).unwrap(), b"keep this");
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn a_destination_deadline_retires_every_import_and_its_wake_folder() {
    destination_import(false).await;
}

#[tokio::test]
async fn an_older_setup_replaces_a_copy_after_its_in_flight_import_finishes() {
    destination_import(true).await;
}

async fn destination_import(replacement: bool) {
    use flate2::{Compression, write::GzEncoder};
    use kuvo_engine::{Engine, Timing};
    use kuvo_peers::{
        devices::Devices,
        directory::{Member, Roster},
        ownership::{Handle, SetupState},
        setup_lists::ResidentPeers,
    };
    use kuvo_store::files;
    use kuvo_wire::keys::encode;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        sync::{Semaphore, mpsc},
        task::JoinSet,
    };

    let home = tempfile::tempdir().unwrap();
    let run = home.path().join("run");
    let session = "ses_slowArrival";
    let id = "mov_slowArrival";
    let here = public(RUST_KEY);
    let peer = public(PEER_KEY);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let endpoint =
        json!({"pid":std::process::id(),"port":port,"password":"fixture","version":"fixture"});
    files::write_atomic(
        &run.join("engine.json"),
        &serde_json::to_vec(&endpoint).unwrap(),
    )
    .unwrap();

    // Each accepted import replies within the engine's own allowance. Only
    // the move's whole-copy deadline can interrupt the last accepted import.
    let engine_sessions = Arc::new(Mutex::new(BTreeMap::<String, Value>::new()));
    let database = engine_sessions.clone();
    let accepted = Arc::new(Semaphore::new(0));
    let respond = accepted.clone();
    let (imported, mut imports) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let mut clients = JoinSet::new();
        while let Ok((socket, _)) = listener.accept().await {
            let (database, respond, imported) =
                (database.clone(), respond.clone(), imported.clone());
            while let Some(result) = clients.try_join_next() {
                result.unwrap();
            }
            clients.spawn(async move {
                let mut socket = BufReader::new(socket);
                let mut request = String::new();
                if socket.read_line(&mut request).await.unwrap() == 0 { return; }
                let mut length = 0;
                let mut header = String::new();
                while socket.read_line(&mut header).await.unwrap() > 2 {
                    if let Some((name, value)) = header.split_once(':') && name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap();
                    }
                    header.clear();
                }
                let route = request.split_whitespace().nth(1).unwrap_or_default();
                let (status, body) = if request.starts_with("POST /api/experimental/session/import ") {
                    let mut body = vec![0; length];
                    socket.read_exact(&mut body).await.unwrap();
                    let entry: Value = serde_json::from_slice(&body).unwrap();
                    let key = entry["info"]["id"].as_str().unwrap().to_owned();
                    database.lock().unwrap().insert(key.clone(), entry["info"].clone());
                    imported.send(key).unwrap();
                    respond.acquire().await.unwrap().forget();
                    ("200 OK", json!({}))
                } else if let Some(key) = route.strip_prefix("/api/session/").filter(|key| key.starts_with("ses_")) {
                    if request.starts_with("DELETE ") {
                        assert!(database.lock().unwrap().remove(key).is_some());
                        ("200 OK", json!({}))
                    } else if let Some(found) = database.lock().unwrap().get(key).cloned() {
                        ("200 OK", json!({"data":found}))
                    } else {
                        ("404 Not Found", json!({}))
                    }
                } else {
                    ("200 OK", json!({"data":[]}))
                };
                let body = body.to_string();
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });

    let (relay, mut wire, _) = TestRelay::welcome(|_| {}).await;
    let mut resident = ResidentPeers::start(
        home.path(),
        Roster {
            here: Some(here),
            main: Some(here),
            name: "Destination".into(),
            members: vec![Member {
                key: peer,
                address: Some(PEER_ADDRESS),
                name: "Source".into(),
                kind: ComputerKind::Machine,
            }],
        },
        Some(relay),
        None,
        false,
        Arc::new(|| 1_000_000),
    )
    .await
    .unwrap();
    let devices = Devices::load(home.path(), "Destination".into(), ComputerKind::Machine)
        .await
        .unwrap()
        .devices;
    let ownership = Handle::new(
        resident.reader(),
        resident.relay_handle(),
        devices,
        SetupState::default,
    );
    let engine = Engine::new(
        home.path().to_owned(),
        Default::default(),
        Timing::default(),
    )
    .unwrap();
    resident.attach_moves(engine, ownership).await.unwrap();

    let mut exports = vec![json!({"info":{"id":session,"title":"Arrival"},"messages":[]})];
    if !replacement {
        exports.extend((1..25).map(
            |child| json!({"info":{"id":format!("ses_child{child}"),"parentID":session},"messages":[]}),
        ));
    }
    let manifest =
        json!({"kind":"chat","thing":session,"name":"Arrival","exports":exports,"folder":session})
            .to_string();
    let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o600);
    header.set_size(manifest.len() as u64);
    archive
        .append_data(&mut header, "move.json", manifest.as_bytes())
        .unwrap();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o700);
    header.set_size(0);
    archive.append_data(&mut header, session, &[][..]).unwrap();
    let contents = archive.into_inner().unwrap().finish().unwrap();
    let hash = encode(&Sha256::digest(&contents));
    let send = |movement: Value| {
        let packet = json!({"at":1_000_000, "move":movement});
        let bytes = Envelope(packet.as_object().unwrap().clone())
            .seal(&PEER_KEY, &here)
            .unwrap();
        frames::relay_send(&PEER_ADDRESS, &bytes)
    };
    let mut offer = json!({"id":id,"op":"offer","kind":"chat","thing":session,"name":"Arrival","generation":1,"hash":hash,"size":contents.len(),"safeSwitch":true});
    if !replacement {
        offer["twoComputers"] = true.into();
    }
    wire.send(send(offer.clone())).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), wire.move_step("ready"))
            .await
            .unwrap()["id"],
        id
    );
    wire.send(send(
        json!({"id":id,"op":"part","generation":1,"hash":hash,"i":0,"of":1,"d":encode(&contents)}),
    ))
    .await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), imports.recv())
            .await
            .unwrap()
            .unwrap(),
        session
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), wire.move_step("working"))
            .await
            .unwrap()["id"],
        id
    );
    let wake = home.path().join("wakes").join(session);
    assert_eq!(
        std::fs::read_to_string(wake.join("paused")).unwrap(),
        format!("moving:{id}")
    );
    let journal = |id| run.join("moves").join(format!("{id}.json"));
    if replacement {
        let next = "mov_replacementCopy";
        offer["id"] = next.into();
        wire.send(send(offer)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(200), wire.move_step("ready"))
                .await
                .is_err()
        );
        assert_eq!(engine_sessions.lock().unwrap().len(), 1);
        accepted.add_permits(1);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), wire.move_step("ready"))
                .await
                .unwrap()["id"],
            next
        );
        assert!(engine_sessions.lock().unwrap().is_empty());
        assert!(!journal(id).exists());
        assert!(journal(next).exists());
        assert!(!wake.exists());
        wire.send(send(
            json!({"id":next,"op":"drop","generation":1,"hash":hash}),
        ))
        .await;
        let dropped = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let proof = wire.move_step("dropped").await;
                if proof["id"] == next {
                    break proof;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(dropped["neverSwitched"], true);
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }

    tokio::time::pause();
    for child in 1..=23 {
        wire.send(vec![0]).await;
        tokio::time::advance(Duration::from_secs(75)).await;
        accepted.add_permits(1);
        let imported = tokio::time::timeout(Duration::from_secs(1), imports.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(imported, format!("ses_child{child}"));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), wire.move_step("working"))
                .await
                .unwrap()["id"],
            id
        );
    }
    assert_eq!(engine_sessions.lock().unwrap().len(), 24);
    wire.send(vec![0]).await;
    tokio::time::advance(Duration::from_secs(76)).await;
    let failed = tokio::time::timeout(Duration::from_secs(10), wire.move_step("failed"))
        .await
        .unwrap();
    assert_eq!(failed["id"], id);
    assert_eq!(failed["hash"], hash);
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .contains("too long to set up")
    );
    let journal = journal(id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while journal.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!wake.exists());
    assert!(engine_sessions.lock().unwrap().is_empty());
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
