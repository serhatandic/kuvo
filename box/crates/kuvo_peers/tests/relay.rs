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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut relay = Relay::start(config(format!("ws://{}", listener.local_addr().unwrap())));
        let mut test = Self(
            accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap(),
        );
        assert_eq!(test.binary().await, frames::relay_hello(&RUST_SECRET));
        let address = keys::address_of(&RUST_SECRET);
        test.send([&[1], &address[..]].concat()).await;
        assert!(matches!(relay.events.recv().await, Some(Event::Connected(got)) if got == address));
        (relay, test)
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
