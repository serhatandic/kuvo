//! Peer routes through real resident processes, private homes, and a loopback relay stand-in.

use std::{collections::HashMap, fs, io, path::Path, process::Stdio, sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use kuvo_peers::{
    relay::{Config, Event, Relay},
    session::{self, BoxHello, ClientHello, ComputerKind},
    setup_lists::{ResidentPeers, system_clock},
};
use kuvo_server::{Caller, LocalOrigin, Transport, ssh::Pairings};
use kuvo_setup::{ManagedDeviceChange, ManagedDeviceGrant, SetupRuntime};
use kuvo_wire::{
    documents::{self, Kind},
    frames,
    keys::{Address, Host, Identity, address_of, encode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::{Mutex, mpsc},
    task::JoinSet,
    time,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

const SESSION: &str = "ses_peers";
const ROOT_SESSION: &str = "ses_root";

fn write(home: &Path, path: &str, value: Value) {
    let file = home.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn read(home: &Path, path: &str) -> Value {
    serde_json::from_slice(&fs::read(home.join(path)).unwrap()).unwrap()
}

fn public(key: u8) -> [u8; 32] {
    Identity::from_bytes([key; 32], [0; 32]).public()
}

fn signing_key(byte: u8) -> String {
    let envelope = documents::sign(&[byte; 32], Kind::Peers, 1, Default::default()).unwrap();
    encode(&documents::verify(envelope).unwrap().origin)
}

fn fixture(home: &Path, url: &str, key: u8, other: Option<u8>) {
    let computer = |key: u8| {
        json!({
            "key": encode(&public(key)), "address": encode(&address_of(&[key + 50; 32])),
            "name": format!("Computer {key}"), "kind": "machine", "sign": signing_key(key),
        })
    };
    let mut link = json!({
        "key": encode(&[key; 32]), "secret": encode(&[key + 50; 32]), "name":format!("Computer {key}"),
        "formatVersion":1, "sign": encode(&[key; 32]), "phones": [],
    });
    if !url.is_empty() {
        link["relay"] = url.into();
    }
    if let Some(other) = other {
        let main = key.min(other);
        let id = format!("set_{}", encode(&[main; 16]));
        let position = json!({"id": id, "epoch": 1, "primary": encode(&public(main))});
        let shared = json!({"membershipAndAccess": {"sequence": 1, "value": {
            "boxes": [computer(main), computer(key.max(other))], "left": [], "moved": [],
            "devices": {"phones": [], "removed": []}, "removalRevisions": {},
        }}});
        let manifest = json!({"id": id, "shared": shared, "archives": []});
        let envelope = documents::sign(
            &[main; 32],
            Kind::SetupCheckpoint,
            1,
            manifest.as_object().unwrap().clone(),
        )
        .unwrap();
        let hash = encode(&Sha256::digest(envelope.d.as_bytes()));
        write(
            home,
            &format!("run/setup-checkpoints/{hash}.json"),
            json!(envelope),
        );
        let cert = json!({"setup": position, "relayTerm": 1, "checkpoint": hash});
        let cert = documents::sign(
            &[main; 32],
            Kind::Leadership,
            1,
            cert.as_object().unwrap().clone(),
        )
        .unwrap();
        link["setup"] = position;
        link["setup"]["managed"] = true.into();
        link["setup"]["checkpointed"] = true.into();
        link["setup"]["role"] = if key == main { "primary" } else { "member" }.into();
        link["setup"]["sign"] = signing_key(main).into();
        link["sharedSetup"] = id.into();
        link["shared"] = shared;
        link["leadership"] = json!({"1": cert});
    }
    write(home, "run/link.json", link);
}

async fn start(home: &Path) -> (Child, Value) {
    let executable =
        std::env::var_os("KUVO_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_kuvo-box").into());
    let mut command = Command::new(executable);
    command.env_clear().env("KUVO_HOME", home);
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot").expect("Windows directory"),
    );
    let mut child = command
        .arg("serve")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let stdout = child.stdout.take().unwrap();
    assert_ne!(
        BufReader::new(stdout).read_line(&mut line).await.unwrap(),
        0,
        "resident exited during startup"
    );
    let ready: Value = serde_json::from_str(&line).unwrap();
    let record = read(home, "run/serve.json");
    assert_eq!(ready["url"], format!("http://127.0.0.1:{}", record["port"]));
    (child, record)
}

async fn request_start(
    record: &Value,
    method: &str,
    path: &str,
    body: Option<Value>,
    app: bool,
) -> (u16, BufReader<TcpStream>) {
    let port = record["port"].as_u64().unwrap() as u16;
    let auth = STANDARD.encode(format!("opencode:{}", record["password"].as_str().unwrap()));
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let body = body.map(|data| data.to_string()).unwrap_or_default();
    let origin = if app { "x-kuvo-from: app\r\n" } else { "" };
    let request = format!(
        "{method} /{path} HTTP/1.0\r\nHost: localhost\r\nAuthorization: Basic {auth}\r\nx-kuvo-route-version: 1\r\n{origin}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).await.unwrap(),
            0,
            "truncated reply headers"
        );
        if line == "\r\n" {
            break;
        }
    }
    (status, reader)
}

async fn request(
    record: &Value,
    method: &str,
    path: &str,
    body: Option<Value>,
    app: bool,
) -> (u16, Value) {
    let (status, mut reader) = request_start(record, method, path, body, app).await;
    let mut bytes = vec![];
    reader.read_to_end(&mut bytes).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn next_line(reader: &mut BufReader<TcpStream>) -> Value {
    let mut line = String::new();
    assert_ne!(
        reader.read_line(&mut line).await.unwrap(),
        0,
        "stream closed before the expected state"
    );
    serde_json::from_str(&line).unwrap()
}

type Destinations = std::sync::Arc<Mutex<HashMap<Address, mpsc::UnboundedSender<Vec<u8>>>>>;

async fn relay(socket: TcpStream, peers: Destinations) -> Option<()> {
    let mut socket = accept_async(socket).await.ok()?;
    let hello = socket.next().await?.ok()?.into_data();
    let secret = hello.strip_prefix(&[1])?.try_into().ok()?;
    let sender = address_of(&secret);
    let (out, mut incoming) = mpsc::unbounded_channel();
    peers.lock().await.insert(sender, out.clone());
    socket
        .send(Message::Binary([&[1], &sender[..]].concat().into()))
        .await
        .ok()?;
    loop {
        tokio::select! {
            Some(bytes) = incoming.recv() => if socket.send(Message::Binary(bytes.into())).await.is_err() { break; },
            message = socket.next() => {
                let Some(Ok(Message::Binary(bytes))) = message else { break; };
                let Some((2, data)) = bytes.split_first() else { continue; };
                let Some((address, payload)) = data.split_at_checked(16) else { continue; };
                let address: Address = address.try_into().ok()?;
                let target = peers.lock().await.get(&address).cloned();
                if target.is_none_or(|target| target.send(frames::relay_send(&sender, payload)).is_err())
                    && socket.send(Message::Binary([&[3], &address[..]].concat().into())).await.is_err() { break; }
            }
        }
    }
    let mut peers = peers.lock().await;
    if peers
        .get(&sender)
        .is_some_and(|current| current.same_channel(&out))
    {
        peers.remove(&sender);
    }
    Some(())
}

async fn relay_socket() -> TcpListener {
    TcpListener::bind(("127.0.0.1", 0)).await.unwrap()
}

fn relay_url(listener: &TcpListener) -> String {
    format!("ws://{}", listener.local_addr().unwrap())
}

fn run_relay(listener: TcpListener) -> JoinSet<io::Result<()>> {
    let mut task = JoinSet::new();
    task.spawn(async move {
        let peers = Destinations::default();
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (socket, _) = accepted?;
                    connections.spawn(relay(socket, peers.clone()));
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
    });
    task
}

async fn stand_in() -> (String, JoinSet<io::Result<()>>) {
    let listener = relay_socket().await;
    (relay_url(&listener), run_relay(listener))
}

async fn leased_relay(socket: TcpStream, peers: Destinations) -> io::Result<()> {
    let mut first = [0];
    if socket.peek(&mut first).await? == 0 {
        return Ok(());
    }
    if first[0] != b'P' {
        relay(socket, peers).await;
        return Ok(());
    }
    let mut request = BufReader::new(socket);
    let mut line = String::new();
    let mut length = 0;
    loop {
        line.clear();
        if request.read_line(&mut line).await? == 0 || line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
    }
    request.read_exact(&mut vec![0; length]).await?;
    let body = json!({
        "lease": {"holder": encode(&public(1)), "sign": signing_key(1),
            "term": 1, "expires": system_clock() + 60_000, "high": 0},
        "remaining": 60_000, "takeoverIn": 80_000,
    })
    .to_string();
    request
        .into_inner()
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
}

#[tokio::test]
async fn a_member_forwards_device_pairing_and_revocation_to_the_main() {
    time::timeout(Duration::from_secs(25), async {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        let listener = relay_socket().await;
        let url = relay_url(&listener);
        let destinations = Destinations::default();
        let mut relay_task = JoinSet::new();
        relay_task.spawn({
            let destinations = destinations.clone();
            async move {
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (socket, _) = match accepted {
                                Ok(connection) => connection,
                                Err(error) => return Err::<(), _>(error),
                            };
                            connections.spawn(leased_relay(socket, destinations.clone()));
                        }
                        _ = connections.join_next(), if !connections.is_empty() => {}
                    }
                }
            }
        });
        let mut residents = Vec::new();
        for (home, own, other) in [(&a, 1, 2), (&b, 2, 1)] {
            fixture(home, &url, own, Some(other));
            let prepared = SetupRuntime::prepare(home, format!("Computer {own}"), false, None)
                .await
                .unwrap();
            let roster = prepared.peer_roster().unwrap();
            let relay = Relay::start(prepared.relay_config().unwrap().unwrap());
            let setup = prepared.start(Some(relay.handle())).unwrap();
            let peers = ResidentPeers::start(
                home,
                roster,
                Some(relay),
                None,
                false,
                Arc::new(system_clock),
            )
            .await
            .unwrap();
            residents.push((setup, peers));
        }
        while !a.join("run/setup-relay-lease.json").exists()
            || !b.join("run/setup-relay-lease.json").exists()
            || destinations.lock().await.len() != 2
        {
            time::sleep(Duration::from_millis(20)).await;
        }
        let main = residents[0].0.handle();
        let member = residents[1].0.handle();
        let caller = Caller::Local(Transport::Http {
            origin: LocalOrigin::App,
            internal: false,
        });
        let key = public(3);
        let change = ManagedDeviceChange::PairDesktop {
            key: encode(&key),
            name: "Desktop from the member".into(),
        };
        let operation = Some("c".repeat(22));
        let saved = member
            .change_devices(&caller, operation.clone(), change.clone())
            .await
            .unwrap();
        let grant = saved.pairing.unwrap();
        let receipt = saved.receipt.unwrap();
        let proof = documents::verify(receipt.proof.clone()).unwrap();
        assert_eq!(encode(&proof.origin), signing_key(1));
        assert_eq!(proof.body["requestingMember"], encode(&public(2)));
        assert_eq!(main.paired(&key).unwrap().id, grant);
        assert_eq!(
            member
                .change_devices(&caller, operation, change)
                .await
                .unwrap()
                .receipt
                .unwrap()
                .proof,
            receipt.proof
        );
        while member.paired(&key).is_none() {
            time::sleep(Duration::from_millis(20)).await;
        }
        member
            .change_devices(
                &caller,
                None,
                ManagedDeviceChange::Revoke {
                    grants: vec![ManagedDeviceGrant {
                        key: encode(&key),
                        pairing: grant,
                    }],
                },
            )
            .await
            .unwrap();
        assert!(main.paired(&key).is_none());
        while member.paired(&key).is_some() {
            time::sleep(Duration::from_millis(20)).await;
        }
        let state = read(&a, "run/link.json");
        assert_eq!(state["shared"]["membershipAndAccess"]["sequence"], 3);
    })
    .await
    .expect("the leased main commits and replicates signed device changes from a member");
}

#[tokio::test]
async fn another_computers_session_is_in_the_first_state_after_restart_and_stays_visible_away() {
    time::timeout(Duration::from_secs(25), async {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        let listener = relay_socket().await;
        let url = relay_url(&listener);
        fixture(&a, &url, 1, Some(2));
        fixture(&b, &url, 2, Some(1));
        write(&b, "run/lists/sessions.json", json!({
            "epoch":"b","seq":1,"sessions":{SESSION:{"facts":{
                "kind":"chat","directory":"/scratch","title":"Peer's session",
                "created":10,"updated":20,"asked":30,"running":true,
            },"place":"engine"}},"projects":{},"needsYou":[{"session":SESSION,"asks":{"kind":"question","summary":"asks for a choice"}}]
        }));
        let (mut a_process, a_record) = start(&a).await;
        let (status, mut states) = request_start(&a_record, "GET", "kuvo/setup/sessions/list/events", None, false).await;
        assert_eq!(status, 200);
        assert_eq!(next_line(&mut states).await["pendingComputers"][0]["name"], "Computer 2");
        let _relay = run_relay(listener);
        let (mut b_process, _) = start(&b).await;
        let key = encode(&public(2));
        loop {
            let state = next_line(&mut states).await;
            if state["sessions"][SESSION]["computer"]["id"] == key {
                assert_eq!(state["needsYou"][0]["asks"]["summary"], "asks for a choice");
                assert_eq!(state["pendingComputers"], json!([]));
                break;
            }
        }
        drop(states);
        let file = format!("run/peers/{key}/lists-v1.json");
        let copy = read(&a, &file);
        assert_eq!(copy["sessions"][0]["id"], SESSION);

        a_process.kill().await.unwrap();
        b_process.kill().await.unwrap();
        // Re-open the real kept copy with both computers silent. Age only the recorded times, so
        // the away scenario is deterministic and does not wait a minute on the wall clock.
        let old = kuvo_peers::setup_lists::system_clock() - 190_000;
        let mut copy = copy;
        copy["seen"] = old.into();
        write(&a, &file, copy);
        write(&a, "run/peer-reach.json", json!({(&key):old + 10}));
        let (_restarted, record) = start(&a).await;
        let (status, mut states) = request_start(&record, "GET", "kuvo/setup/sessions/list/events", None, false).await;
        assert_eq!(status, 200);
        let first = next_line(&mut states).await;
        assert_eq!(first["sessions"][SESSION]["on"], json!({
            "id":key,"name":"Computer 2","away":true,"lastSeen":old,"actions":[],
        }));
        let (status, location) = request(&record, "GET", &format!("kuvo/sessions/{SESSION}/location"), None, false).await;
        assert_eq!(status, 200);
        assert_eq!(location["on"], first["sessions"][SESSION]["on"]);
        let (_, counts) = request(&record, "GET", "kuvo/setup/waiting/counts", None, false).await;
        assert_eq!(counts["dock"], 1);
    }).await.expect("two boxes share the session and reopen its kept snapshot");
}

async fn wire_request(
    relay: &mut Relay,
    session: session::Id,
    head: Value,
    body: Option<Value>,
) -> (u16, Value) {
    let request = head["id"].clone();
    relay
        .sender
        .send(
            session,
            session::Message {
                head,
                body: body.map_or_else(Vec::new, |body| serde_json::to_vec(&body).unwrap()),
            },
        )
        .await
        .unwrap();
    let mut status = None;
    let mut bytes = vec![];
    loop {
        match relay.events.recv().await {
            Some(Event::Session(session::Event::Message(id, message)))
                if id == session && message.head["id"] == request =>
            {
                if message.head["t"] == "res" {
                    status = message.head["s"].as_u64();
                    assert_eq!(message.head["h"]["x-kuvo-route-version"], "1");
                }
                bytes.extend(message.body);
                if message.head["end"] == true {
                    return (
                        status.unwrap() as u16,
                        serde_json::from_slice(&bytes).unwrap(),
                    );
                }
            }
            Some(Event::Session(session::Event::Ended(id))) if id == session => {
                panic!("paired session ended before the reply")
            }
            None => panic!("relay stopped before the paired reply"),
            _ => {}
        }
    }
}

async fn desktop(url: String, pairing: String) -> (Relay, session::Id) {
    let mut relay = Relay::start(Config {
        url,
        secret: [99; 32],
        key: [4; 32],
        grants: HashMap::new(),
        peers: vec![],
        hello: BoxHello {
            v: 1,
            kind: ComputerKind::Machine,
            name: "Desktop".into(),
            app: "test".into(),
            engine: None,
        },
    });
    loop {
        if matches!(relay.events.recv().await, Some(Event::Connected(_))) {
            break;
        }
    }
    let session = relay
        .sender
        .start(
            Host {
                key: public(3),
                address: address_of(&[53; 32]),
            },
            ClientHello {
                v: 1,
                name: "Desktop app".into(),
                app: "test".into(),
                client: "desktop".into(),
                pair: Some(pairing),
            },
        )
        .await
        .unwrap();
    loop {
        match relay.events.recv().await {
            Some(Event::Session(session::Event::Opened(id, _))) if id == session => break,
            Some(Event::Session(session::Event::Ended(id))) if id == session => {
                panic!("desktop grant refused")
            }
            None => panic!("relay stopped before the desktop opened"),
            _ => {}
        }
    }
    (relay, session)
}

#[tokio::test]
async fn a_local_app_pairs_once_and_removal_survives_a_resident_restart() {
    time::timeout(Duration::from_secs(15), async {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".kuvo-code-adding"), []).unwrap();
        let home = root.path().join("box");
        let (url, _relay) = stand_in().await;
        fixture(&home, &url, 3, None);
        let key = encode(&public(4));
        let body = json!({"kind":"desktop","key":key,"name":"Desktop app"});
        let (mut child, record) = start(&home).await;
        let (status, _) = request(
            &record,
            "POST",
            "kuvo/devices/pair",
            Some(body.clone()),
            false,
        )
        .await;
        assert_eq!(status, 403);
        let (status, reply) = request(
            &record,
            "POST",
            "kuvo/devices/pair",
            Some(body.clone()),
            true,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(reply["paired"], true);
        let pairing = reply["pairing"].as_str().unwrap();
        assert_eq!(pairing.len(), 16);
        assert_eq!(
            request(
                &record,
                "POST",
                "kuvo/devices/pair",
                Some(body.clone()),
                true
            )
            .await,
            (200, reply.clone())
        );
        let (_, devices) = request(&record, "GET", "kuvo/phones", None, false).await;
        assert_eq!(devices["phones"].as_array().unwrap().len(), 1);
        assert_eq!(devices["phones"][0]["kind"], "desktop");
        assert_eq!(devices["phones"][0]["name"], "Desktop app");
        assert_eq!(devices["phones"][0]["pairing"], pairing);
        let remove = |key: &str| Some(json!({"phones":[{"key":key,"pairing":pairing}]}));
        let (status, _) = request(
            &record,
            "POST",
            "kuvo/phones/remove",
            remove(&encode(&public(3))),
            true,
        )
        .await;
        assert_eq!(status, 400, "a computer key cannot be revoked as a device");
        child.kill().await.unwrap();
        let (mut child, _) = start(&home).await;
        let (mut desktop, session) = desktop(url, pairing.into()).await;
        let (status, listed) = wire_request(&mut desktop, session, json!({
            "t":"req","id":1,"m":"GET","p":"kuvo/phones","h":{"X-Kuvo-Route-Version":"1"},
        }), None).await;
        assert_eq!(status, 200);
        assert_eq!(listed["phones"][0]["you"], true);
        let (status, _) = wire_request(&mut desktop, session, json!({
            "t":"req","id":2,"m":"GET","p":"kuvo/phones","h":{"X-Kuvo-Route-Version":"2","x-kuvo-route-version":"1"},
        }), None).await;
        assert_eq!(status, 400);
        let removed = wire_request(&mut desktop, session, json!({
            "t":"req","id":3,"m":"POST","p":"kuvo/phones/remove","h":{"x-kuvo-route-version":"1"},
        }), remove(&key)).await;
        assert_eq!(removed, (200, json!({})));
        loop {
            match desktop.events.recv().await {
                Some(Event::Session(session::Event::Ended(id))) if id == session => break,
                None => panic!("relay stopped before revoking the session"),
                _ => {}
            }
        }
        child.kill().await.unwrap();
        let (_restarted, record) = start(&home).await;
        assert_eq!(
            request(&record, "GET", "kuvo/phones", None, false).await,
            (200, json!({"phones":[]}))
        );
        let (status, blocked) =
            request(&record, "POST", "kuvo/devices/pair", Some(body), true).await;
        assert_eq!(status, 409);
        assert_eq!(blocked["reason"], "removed");
    })
    .await
    .expect("pairing and removal complete in the scratch home");
}

#[tokio::test]
async fn kept_activity_and_notices_keep_their_origin_and_dock_deduplicates_a_bot() {
    time::timeout(Duration::from_secs(10), async {
        let home = tempfile::tempdir().unwrap();
        let (url, _relay) = stand_in().await;
        fixture(home.path(), &url, 3, Some(4));
        let key = encode(&public(4));
        let now = kuvo_peers::setup_lists::system_clock();
        let seen = now - 190_000;
        write(home.path(), &format!("run/peers/{key}/lists-v1.json"), json!({
            "at":seen,"seen":seen,"projects":[],"queues":[],
            "sessions":[{"id":SESSION,"facts":{"kind":"bot","bot":"bot_one","created":1,"updated":2,"asked":3,"directory":"/scratch"},"actions":["open"]}],
            "waiting":[],"catalog":{"agents":[],"offered":{"known":{"engine":false,"agents":false},"models":[]}},
            "bots":{"bot_one":{"session":SESSION,"now":{"working":0,"asking":1,"state":"needsYou","since":3}}},
        }));
        write(home.path(), &format!("run/peers/{key}/notices-v1.json"), json!({
            "at":seen,"seen":seen,"telling":true,"notices":[{"id":"ntc_fixed","at":seen,"notice":{"category":"question"}}]
        }));
        write(home.path(), &format!("run/activity/{key}.json"), json!({
            "at":seen,"heard":seen,"rows":[{"session":SESSION,"line":[ROOT_SESSION],"title":"Delegated","state":"working","subagent":true}],
        }));
        write(home.path(), "run/peer-reach.json", json!({(&key):seen}));
        // Keep relay traffic out of this scenario so the fixture stays stable.
        let mut link = read(home.path(), "run/link.json");
        link.as_object_mut().unwrap().remove("relay");
        write(home.path(), "run/link.json", link);
        let (_child, record) = start(home.path()).await;
        let (status, mut stream) = request_start(&record, "GET", "kuvo/setup/notices/events", None, false).await;
        assert_eq!(status, 200);
        let first = next_line(&mut stream).await;
        assert_eq!(first["notices"], json!([{
            "id":"ntc_fixed","at":seen,"notice":{"category":"question"},"computer":{"id":key,"name":"Computer 4"},
            "away":true,"lastSeen":seen,
        }]));
        assert_eq!(first["origins"][1]["telling"], false);
        let (_, counts) = request(&record, "GET", "kuvo/setup/waiting/counts", None, false).await;
        assert_eq!((counts["sessions"].as_u64(), counts["bots"].as_u64(), counts["dock"].as_u64()), (Some(1), Some(1), Some(1)));

        let bootstrap = kuvo_peers::devices::Devices::load(home.path(), "Here".into(), kuvo_peers::session::ComputerKind::Machine).await.unwrap();
        let peers = kuvo_peers::setup_lists::ResidentPeers::start(
            home.path(), bootstrap.roster, None, None, false, std::sync::Arc::new(move || now),
        ).await.unwrap();
        let view = peers.reader().view();
        assert_eq!(serde_json::to_value(view.activity(Some(ROOT_SESSION), None, true, now)).unwrap(), json!({
            "working":0,"asking":0,"sessions":[],"lastSeenSessions":[
                {"session":SESSION,"title":"Delegated","state":"working","computer":key,"subagent":true,"lastSeen":seen}
            ],
        }));
    }).await.expect("kept views stream immediately");
}

#[tokio::test]
async fn a_restarting_release_keeps_its_session_actions_and_cannot_be_chosen_for_a_start() {
    time::timeout(Duration::from_secs(10), async {
        let home = tempfile::tempdir().unwrap();
        fixture(home.path(), "", 5, Some(6));
        let key = encode(&public(6));
        let now = kuvo_peers::setup_lists::system_clock();
        let seen = now - 190_000;
        let catalog = json!({
            "agents":[{"id":"codex-cli","name":"Codex"}],
            "offered":{"known":{"engine":false,"agents":true},"models":[]},
        });
        write(home.path(), "run/lists/runs-on.json", catalog.clone());
        write(home.path(), "run/lists/sessions.json", json!({
            "epoch":"here","seq":1,"projects":{},"sessions":{
                "ses_here":{"place":"engine","facts":{"kind":"chat","created":1,"updated":2,"directory":"/scratch"}},
                "ses_switched":{"place":"engine","facts":{"kind":"chat","created":1,"updated":2,"directory":"/scratch"}},
            },
        }));
        write(home.path(), &format!("run/peers/{key}/lists-v1.json"), json!({
            "at":seen,"seen":seen,"catalog":catalog,"projects":[],"queues":[],"waiting":[],
            "sessions":[{"id":SESSION,"facts":{"kind":"chat","created":1,"updated":2,"directory":"/scratch"},"actions":["open","manage"]}],
        }));
        write(home.path(), "run/peer-reach.json", json!({(&key): seen}));
        write(home.path(), "run/peer-agents.json", json!({
            (&key): {"at":seen,"heard":seen,"list":[],"features":["line.setup-lists","line.peer-conversation"],
                "computer":{"fleetUpdate":{"state":"ready"}},
                "owns":format!("{}{}", kuvo_peers::directory::owned_digest("ses_onlyAnnouncement"),
                    kuvo_peers::directory::owned_digest("ses_here"))},
        }));
        write(home.path(), "run/moves/mov_switch.json", json!({
            "thing":"ses_switched","sessions":["ses_switched"],"peer":key,"role":"from",
            "stage":"switching","started":seen,"formatVersion":1,
        }));
        let messages = json!([{"id":"msg_fixed","role":"assistant","text":"I kept this page."}]);
        write(home.path(), &format!("run/peers/{key}/conversations/{SESSION}.json"), json!({
            "at":seen,"data":messages,
        }));
        let (_child, record) = start(home.path()).await;

        let (status, mut rows) = request_start(&record, "GET", "kuvo/setup/sessions/list/events", None, false).await;
        assert_eq!(status, 200);
        let first = next_line(&mut rows).await;
        assert_eq!(first["sessions"][SESSION]["on"], json!({
            "id":key,"name":"Computer 6","away":false,"updating":true,"actions":["open","manage"],
        }));

        let (status, mut runs) = request_start(&record, "GET", "kuvo/setup/runs-on/events", None, false).await;
        assert_eq!(status, 200);
        let first = next_line(&mut runs).await;
        assert_eq!(first["computers"][0]["here"], true);
        assert_eq!(first["computers"][0]["pending"], true);
        assert_eq!(first["computers"][0]["agents"], catalog["agents"]);
        assert_eq!(first["computers"][1]["away"], true);
        assert_eq!(first["computers"][1]["updating"], true);
        assert_eq!(first["computers"][1]["agents"], catalog["agents"]);

        let location = |id| format!("kuvo/sessions/{id}/location");
        let (status, announced) = request(&record, "GET", &location("ses_onlyAnnouncement"), None, false).await;
        assert_eq!(status, 200);
        assert_eq!(announced["on"]["id"], key);
        assert_eq!(request(&record, "GET", &location("ses_absent"), None, false).await.0, 404);
        let (status, here) = request(&record, "GET", &location("ses_here"), None, false).await;
        assert_eq!((status, here), (200, json!({"place":"engine","running":false})));
        let (status, moved) = request(&record, "GET", &location("ses_switched"), None, false).await;
        assert_eq!(status, 200);
        assert_eq!(moved["on"]["id"], key);
        let path = format!("kuvo/peer-session/{key}/{SESSION}/messages");
        assert_eq!(request(&record, "GET", &path, None, true).await, (200, json!({
            "data":messages,"away":{"computer":{"id":key,"name":"Computer 6"},"updating":true},
        })));
    }).await.expect("the kept catalog and conversation page are usable while the peer updates");
}
