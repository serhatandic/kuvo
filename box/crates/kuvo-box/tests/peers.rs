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
    mail::Envelope,
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
    let mut members: Vec<_> = other.into_iter().flat_map(|other| [key, other]).collect();
    members.sort_unstable();
    fixture_roster(home, url, key, &members);
}

fn fixture_roster(home: &Path, url: &str, key: u8, members: &[u8]) {
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
    if let Some(&main) = members.first() {
        let id = format!("set_{}", encode(&[main; 16]));
        let position = json!({"id": id, "epoch": 1, "primary": encode(&public(main))});
        let shared = json!({"membershipAndAccess": {"sequence": 1, "value": {
            "boxes": members.iter().copied().map(computer).collect::<Vec<_>>(), "left": [], "moved": [],
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
    let marker = home.join(kuvo_store::layout::FILE);
    if !marker.exists() {
        kuvo_store::layout::publish(&kuvo_store::home::Home::new(home.to_path_buf()).unwrap())
            .unwrap();
    }
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
    run_relay_with(listener, Destinations::default())
}

fn run_relay_with(listener: TcpListener, peers: Destinations) -> JoinSet<io::Result<()>> {
    let mut task = JoinSet::new();
    task.spawn(async move {
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

fn run_leased(listener: TcpListener, destinations: Destinations) -> JoinSet<io::Result<()>> {
    let mut relay_task = JoinSet::new();
    relay_task.spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (socket, _) = accepted?;
                    connections.spawn(leased_relay(socket, destinations.clone()));
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
    });
    relay_task
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
        let _relay_task = run_leased(listener, destinations.clone());
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

fn host(key: u8) -> Host {
    Host {
        key: public(key),
        address: address_of(&[key + 50; 32]),
    }
}

async fn wait_for_roster(record: &Value, keys: &[String]) {
    loop {
        let (status, value) = request(record, "GET", "kuvo/phones", None, true).await;
        assert_eq!(status, 200);
        let phones = value["phones"].as_array().unwrap();
        if phones.len() == keys.len()
            && keys
                .iter()
                .all(|key| phones.iter().any(|row| row["key"] == *key))
        {
            return;
        }
        time::sleep(Duration::from_millis(20)).await;
    }
}

async fn connecting_client(url: &str, key: u8, peers: Vec<Host>) -> Relay {
    let mut relay = Relay::start(Config {
        url: url.into(),
        grants: HashMap::new(),
        key: [key; 32],
        secret: [key + 50; 32],
        peers,
        legacy_enrollers: vec![],
        hello: BoxHello {
            v: 1,
            name: "Client".into(),
            app: "test".into(),
            kind: ComputerKind::Machine,
            engine: None,
        },
    });
    while !matches!(relay.events.recv().await, Some(Event::Connected(_))) {}
    relay
}

async fn open_desktop(relay: &mut Relay, target: Host, pairing: String) -> Option<session::Id> {
    let id = relay
        .sender
        .start(
            target,
            ClientHello {
                v: 1,
                name: "Managed desktop".into(),
                app: "test".into(),
                client: "desktop".into(),
                pair: Some(pairing),
            },
        )
        .await
        .unwrap();
    loop {
        match relay.events.recv().await {
            Some(Event::Session(session::Event::Opened(got, _))) if got == id => return Some(id),
            Some(Event::Session(session::Event::Ended(got))) if got == id => return None,
            Some(_) => {}
            None => panic!("relay ended before the pairing result"),
        }
    }
}

async fn wait_closed(relay: &mut Relay, id: session::Id) {
    loop {
        match relay.events.recv().await {
            Some(Event::Session(session::Event::Ended(got))) if got == id => return,
            Some(_) => {}
            None => panic!("relay ended before the revoked device closed"),
        }
    }
}

async fn verify_phone_sync(url: &str, key: &str) {
    let mut client = connecting_client(url, 3, vec![host(1)]).await;
    let origin = signing_key(1);
    let id = encode(&[88; 16]);
    let request = json!({"phoneSync":{"id":id,"origin":origin},"at":system_clock()});
    client
        .sender
        .mail(
            host(1),
            kuvo_wire::mail::Envelope(request.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let (mut phones, mut peers) = (None, None);
    loop {
        let event = client
            .events
            .recv()
            .await
            .expect("signed device sync reply");
        let Event::Mail {
            key: sender,
            envelope,
            ..
        } = event
        else {
            continue;
        };
        if sender != public(1) {
            continue;
        }
        for export in envelope
            .0
            .get("docs")
            .and_then(|docs| docs["give"].as_array())
            .into_iter()
            .flatten()
        {
            let doc = documents::verify(serde_json::from_value(export.clone()).unwrap()).unwrap();
            assert_eq!(encode(&doc.origin), origin);
            match doc.kind {
                Kind::Phones => phones = Some(doc),
                Kind::Peers => peers = Some(doc),
                _ => {}
            }
        }
        if let Some(sync) = envelope.0.get("phoneSync") {
            let phones = phones.expect("phone sync includes the signed grant list");
            let peers = peers.expect("phone sync includes the signed peer roster");
            assert_eq!(
                *sync,
                json!({"id":id,"ack":true,
                    "list":format!("{}:{}", phones.n, phones.digest()),
                    "peers":format!("{}:{}", peers.n, peers.digest()),
                })
            );
            assert!(
                phones.body["phones"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["key"] == key)
            );
            assert_eq!(peers.body["boxes"].as_array().unwrap().len(), 3);
            return;
        }
    }
}

#[tokio::test]
async fn a_reconnecting_computer_gets_signed_device_removal_from_a_member_while_main_is_away() {
    time::timeout(Duration::from_secs(35), async {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("main");
        let b = temp.path().join("member");
        let c = temp.path().join("returning");
        let listener = relay_socket().await;
        let url = relay_url(&listener);
        let destinations = Destinations::default();
        let _relay = run_leased(listener, destinations.clone());
        for (home, key) in [(&a, 1), (&b, 2), (&c, 3)] {
            fixture_roster(home, &url, key, &[1, 2, 3]);
        }
        let (mut primary, main) = start(&a).await;
        let (_member, member) = start(&b).await;
        let (mut returning, old) = start(&c).await;
        while !a.join("run/setup-relay-lease.json").is_file()
            || destinations.lock().await.len() != 3
        {
            time::sleep(Duration::from_millis(20)).await;
        }
        let grant_key = encode(&public(4));
        let admin_key = encode(&public(5));
        let pair = |key: &str, name: &str| json!({"key":key,"name":name,"kind":"desktop"});
        let (status, denied) = request(
            &member,
            "POST",
            "kuvo/devices/pair",
            Some(pair(&grant_key, "Member's app")),
            false,
        )
        .await;
        assert_eq!(status, 403, "{denied}");
        let (status, grant) = request(
            &member,
            "POST",
            "kuvo/devices/pair",
            Some(pair(&grant_key, "Member's app")),
            true,
        )
        .await;
        assert_eq!(status, 200, "{grant}");
        let pairing = grant["pairing"].as_str().unwrap().to_owned();
        let (status, admin) = request(
            &main,
            "POST",
            "kuvo/devices/pair",
            Some(pair(&admin_key, "Other desktop")),
            true,
        )
        .await;
        assert_eq!(status, 200, "{admin}");
        let keys = [grant_key.clone(), admin_key.clone()];
        for record in [&main, &member, &old] {
            wait_for_roster(record, &keys).await;
        }

        let mut client = connecting_client(&url, 4, vec![]).await;
        let main_session = open_desktop(&mut client, host(1), pairing.clone())
            .await
            .expect("member's pairing opens the main");
        let member_session = open_desktop(&mut client, host(2), pairing.clone())
            .await
            .expect("the same desktop grant opens the member");
        // This authenticated desktop revokes the other grant over its own paired route.
        let admin_host = host(2);
        let mut member_client = connecting_client(&url, 5, vec![]).await;
        let admin_session = open_desktop(
            &mut member_client,
            admin_host,
            admin["pairing"].as_str().unwrap().into(),
        )
        .await
        .expect("admin device opens the member");
        for (session, expected_name) in [(main_session, "Computer 1"), (member_session, "Computer 2")] {
            let (status, info) = wire_request(
                &mut client,
                session,
                json!({"t":"req","id":299,"p":"kuvo/info","h":{"x-kuvo-route-version":"1"},"end":true}), None,
            ).await;
            assert_eq!(status, 200);
            assert_eq!(info["name"], expected_name);
            let (status, phones) = wire_request(
                &mut client,
                session,
                json!({"t":"req","id":300,"p":"kuvo/phones","h":{"x-kuvo-route-version":"1"},"end":true}), None,
            ).await;
            assert_eq!(status, 200);
            let roster = phones["phones"].as_array().unwrap();
            let own = roster.iter().find(|row| row["key"] == grant_key).unwrap();
            assert_eq!(own["you"], true);
            assert_eq!(own["kind"], "desktop");
            assert_eq!(own["name"], "Member's app");
            let (status, denied) = wire_request(
                &mut client, session,
                json!({"t":"req","id":302,"m":"POST","p":"kuvo/devices/pair","h":{"x-kuvo-route-version":"1"},"end":true}),
                Some(pair(&grant_key, "Member's app")),
            ).await;
            assert_eq!(status, 403);
            assert_eq!(denied["message"], "Only Kuvo Code on this computer adds itself to your devices.");
        }

        let (status, _) = request(&member, "POST", "kuvo/phones/remove",
            Some(json!({"phones":[{"key":grant_key,"pairing":"former"}]})), true).await;
        assert_eq!(status, 200);
        wait_for_roster(&member, &keys).await;

        let old_endpoint = destinations.lock().await[&host(3).address].clone();
        returning.kill().await.unwrap();
        old_endpoint.closed().await;
        verify_phone_sync(&url, &grant_key).await;
        let (status, removed) = wire_request(
            &mut member_client,
            admin_session,
            json!({"t":"req","id":301,"m":"POST","p":"kuvo/phones/remove","h":{"x-kuvo-route-version":"1"},"end":true}),
            Some(json!({"phones":[{"key":grant_key,"pairing":pairing}]})),
        )
        .await;
        assert_eq!(status, 200, "{removed}");
        wait_closed(&mut client, main_session).await;
        wait_for_roster(&member, std::slice::from_ref(&admin_key)).await;
        assert!(
            open_desktop(&mut client, host(2), pairing.clone())
                .await
                .is_none()
        );
        assert!(open_desktop(&mut client, host(1), pairing.clone()).await.is_none());
        let (status, info) = wire_request(
            &mut member_client, admin_session,
            json!({"t":"req","id":303,"p":"kuvo/info","h":{"x-kuvo-route-version":"1"},"end":true}), None,
        ).await;
        assert_eq!(status, 200);
        assert_eq!(info["name"], "Computer 2");
        assert_eq!(
            read(&c, "run/link.json")["shared"]["membershipAndAccess"]["sequence"],
            3
        );
        let primary_endpoint = destinations.lock().await[&host(1).address].clone();
        primary.kill().await.unwrap();
        primary_endpoint.closed().await;

        let (_reconnected, new) = start(&c).await;
        wait_for_roster(&new, std::slice::from_ref(&admin_key)).await;
        let accepted = read(&c, "run/link.json");
        let signed =
            documents::verify(serde_json::from_value(accepted["setupReplica"].clone()).unwrap())
                .unwrap();
        assert_eq!(encode(&signed.origin), signing_key(1));
        assert_eq!(accepted["shared"]["membershipAndAccess"]["sequence"], 4);
        assert!(open_desktop(&mut client, host(3), pairing).await.is_none());
        let (status, refused) = request(&new, "POST", "kuvo/devices/pair", Some(pair(&grant_key, "Member's app")), true).await;
        assert_eq!(status, 409);
        assert_eq!(refused["reason"], "removed");
        let (status, waiting) = request(&new, "POST", "kuvo/devices/pair", Some(pair(&encode(&public(6)), "New app")), true).await;
        assert_eq!(status, 503, "{waiting}");
        assert_eq!(waiting["message"], "Computer 1 isn't answering. This app reaches your other computers once it does.");
        wait_for_roster(&new, std::slice::from_ref(&admin_key)).await;

    })
    .await
    .expect("the returning member gets the signed revocation without its main");
}

#[tokio::test]
async fn another_computers_session_is_in_the_first_state_after_restart_and_stays_visible_away() {
    const ACTIVE: &str = "ses_active";
    time::timeout(Duration::from_secs(25), async {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        let listener = relay_socket().await;
        let url = relay_url(&listener);
        fixture(&a, &url, 1, Some(2));
        fixture(&b, &url, 2, Some(1));
        write(&b, "run/lists/sessions.json", json!({
            "epoch":"b","seq":1,"sessions":{
                SESSION:{"facts":{
                    "kind":"chat","directory":"/scratch","title":"Peer's session",
                    "created":10,"updated":20,"asked":30,"running":true,
                },"place":"engine"},
                ACTIVE:{"facts":{
                    "kind":"chat","directory":"/scratch","title":"Active session",
                    "created":10,"updated":21,"running":true,
                },"place":"engine"}
            },"projects":{},"needsYou":[{"session":SESSION,"asks":{"kind":"question","summary":"asks for a choice"}}]
        }));
        let (mut a_process, a_record) = start(&a).await;
        let key = encode(&public(2));
        let pending = json!([{"id":key,"name":"Computer 2"}]);
        let (status, mut states) = request_start(&a_record, "GET", "kuvo/setup/sessions/list/events", None, false).await;
        assert_eq!(status, 200);
        assert_eq!(next_line(&mut states).await["pendingComputers"], pending);
        for (route, field) in [
            ("kuvo/setup/sessions/events", "sessions"),
            ("kuvo/setup/waiting/events", "waiting"),
            ("kuvo/setup/notices/events", "notices"),
        ] {
            let (status, mut watch) = request_start(&a_record, "GET", route, None, false).await;
            assert_eq!(status, 200, "{route}");
            let state = next_line(&mut watch).await;
            assert_eq!(state["pendingComputers"], pending, "{route}");
            assert_eq!(state[field], json!([]), "{route}");
        }
        let _relay = run_relay(listener);
        let (mut b_process, _) = start(&b).await;
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
        assert_eq!(copy["sessions"].as_array().unwrap().len(), 2);

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
        assert_eq!(first["pendingComputers"], json!([]));
        assert_eq!(first["sessions"][SESSION]["on"], json!({
            "id":key,"name":"Computer 2","away":true,"lastSeen":old,"actions":[],
        }));
        let (status, location) = request(&record, "GET", &format!("kuvo/sessions/{SESSION}/location"), None, false).await;
        assert_eq!(status, 200);
        assert_eq!(location["on"], first["sessions"][SESSION]["on"]);
        let (_, counts) = request(&record, "GET", "kuvo/setup/waiting/counts", None, false).await;
        assert_eq!(counts["dock"], 1);
        assert_eq!(counts["pendingComputers"], json!([]));
        let (status, mut waiting) = request_start(&record, "GET", "kuvo/setup/waiting/events", None, false).await;
        assert_eq!(status, 200);
        let waiting = next_line(&mut waiting).await;
        assert_eq!(waiting["pendingComputers"], json!([]));
        assert_eq!(waiting["waiting"][0]["session"], SESSION);
        assert_eq!(waiting["waiting"][0]["away"], true);

        let (status, computers) = request(&record, "GET", "kuvo/computers", None, false).await;
        assert_eq!(status, 200);
        assert_eq!(computers["computers"][1]["reach"], "away");
        assert_eq!(computers["computers"][1]["working"], 0);
        let (_returned, _) = start(&b).await;
        loop {
            let state = next_line(&mut states).await;
            if state["sessions"][SESSION]["on"]["away"] == false {
                assert_eq!(state["sessions"][SESSION]["on"], json!({
                    "id":key,"name":"Computer 2","away":false,"actions":["open","manage"],
                }));
                break;
            }
        }
        let (status, computers) = request(&record, "GET", "kuvo/computers", None, false).await;
        assert_eq!(status, 200);
        let peers = computers["computers"].as_array().unwrap();
        assert_eq!(peers[0]["reach"], "connected");
        assert_eq!(peers[0]["working"], 0);
        assert_eq!(peers[1]["address"], encode(&address_of(&[52; 32])));
        assert_eq!(peers[1]["relay"], url);
        assert_eq!(peers[1]["reach"], "connected");
        assert_eq!(peers[1]["working"], 1);
        assert_eq!(peers[1]["atWork"], json!([{"id":ACTIVE,"title":"Active session"}]));
    }).await.expect("two boxes share the session and reopen its kept snapshot");
}

#[tokio::test]
async fn computers_show_only_valid_facts_from_the_latest_announcement() {
    time::timeout(Duration::from_secs(15), async {
        let home = tempfile::tempdir().unwrap();
        let listener = relay_socket().await;
        let url = relay_url(&listener);
        fixture_roster(home.path(), &url, 1, &[1, 2, 3]);
        let (_resident, record) = start(home.path()).await;
        let (_, before) = request(&record, "GET", "kuvo/computers", None, false).await;
        for row in &before["computers"].as_array().unwrap()[1..] {
            assert_eq!(row["reach"], "connecting");
            assert!(row.get("working").is_none());
        }
        let (status, mut listing) =
            request_start(&record, "GET", "kuvo/setup/sessions/events", None, false).await;
        assert_eq!(status, 200);
        let pending = |key| json!({"id":encode(&public(key)), "name":format!("Computer {key}")});
        assert_eq!(
            next_line(&mut listing).await["pendingComputers"],
            json!([pending(2), pending(3)])
        );
        let _relay = run_relay(listener);
        let peer = connecting_client(&url, 2, vec![host(1)]).await;
        let at = system_clock();
        let send = |at, computer| {
            let sender = peer.sender.clone();
            async move {
                let mail = json!({
                    "at":at,"agents":{"at":at,"list":[],"features":["line.setup-lists"],"computer":computer}
                });
                sender
                    .mail(host(1), Envelope(mail.as_object().unwrap().clone()))
                    .await
                    .unwrap();
            }
        };
        send(
            at,
            json!({
                "system":"Ubuntu 24.04.1 LTS","engine":"1.19.2","version":"1.0.0","working":2,
                "agents":{"state":"down","why":"outdated","extra":"ignored"},
                "extra":"ignored"
            }),
        )
        .await;
        // An older message can refresh reach, but must never replace computer facts.
        send(at - 1, json!({"version":"old","working":9})).await;
        loop {
            let (_, state) = request(&record, "GET", "kuvo/computers", None, false).await;
            let row = &state["computers"][1];
            if row["reach"] == "connected" {
                assert_eq!(row["version"], "1.0.0");
                assert_eq!(row["engine"], "1.19.2");
                assert_eq!(row["system"], "Ubuntu 24.04.1 LTS");
                assert_eq!(row["working"], 2);
                assert_eq!(row["agents"], json!({"state":"down","why":"outdated"}));
                assert_eq!(row["compatibility"]["state"], "held");
                assert!(row["compatibility"]["missing"].as_array().unwrap().contains(&json!("messages.send")));
                break;
            }
        }
        send(
            at + 1,
            json!({
                "version":"1.0.1","system":"x".repeat(81),"engine":7,"working":10001,
                "agents":{"state":"down","why":"unknown"}
            }),
        )
        .await;
        loop {
            let (_, state) = request(&record, "GET", "kuvo/computers", None, false).await;
            let row = &state["computers"][1];
            if row["version"] == "1.0.1" {
                for omitted in ["system", "engine", "working"] {
                    assert!(row.get(omitted).is_none(), "{row}");
                }
                assert_eq!(row["agents"], json!({"state":"down"}));
                break;
            }
        }
        let peer_file = read(home.path(), "run/peer-agents.json");
        assert_eq!(
            peer_file[encode(&public(2))]["computer"],
            json!({
                "version":"1.0.1","agents":{"state":"down"}
            })
        );
        loop {
            let (_, state) = request(&record, "GET", "kuvo/computers", None, false).await;
            let never_heard = &state["computers"][2];
            if never_heard["reach"] == "away" {
                assert!(never_heard.get("working").is_none());
                break;
            }
        }
        let (status, mut listing) =
            request_start(&record, "GET", "kuvo/setup/sessions/events", None, false).await;
        assert_eq!(status, 200);
        let state = next_line(&mut listing).await;
        assert_eq!(state["sessions"], json!([]));
        assert_eq!(state["pendingComputers"], json!([pending(2)]));
    })
    .await
    .expect("both peer announcements reach the computer list");
}

#[tokio::test]
async fn an_away_worktree_pages_under_its_own_computers_project() {
    time::timeout(Duration::from_secs(12), async {
        let home = tempfile::tempdir().unwrap();
        let listener = relay_socket().await;
        fixture(home.path(), &relay_url(&listener), 1, Some(2));
        let key = encode(&public(2));
        let project = format!("{key}:/scratch/source");
        let old = kuvo_peers::setup_lists::system_clock() - 190_000;
        let ids: Vec<_> = (0..18).map(|i| format!("ses_worktree{i}")).collect();
        let row = |id: &str, i: usize, archived: bool| {
            json!({
                "id": id, "actions": ["open","manage"], "facts": {
                    "kind":"project","directory":"/scratch/worktree","project":"/scratch/source",
                    "title":id,"created":10, "updated":100-i, "archived":archived,
                }
            })
        };
        let sessions: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| row(id, i, false))
            .chain([row("ses_old", 19, true)])
            .collect();
        write(
            home.path(),
            &format!("run/peers/{key}/lists-v1.json"),
            json!({
                "at":old, "seen":old, "sessions":sessions, "projects":[],
                "catalog":kuvo_peers::lists::Catalog::default(),"queues":[],"waiting":[]
            }),
        );
        let (_resident, record) = start(home.path()).await;
        let (status, mut watch) = request_start(
            &record,
            "GET",
            "kuvo/setup/sessions/list/events",
            None,
            false,
        )
        .await;
        assert_eq!(status, 200);
        let first = next_line(&mut watch).await;
        assert_eq!(first["sessions"][&ids[0]]["directory"], "/scratch/worktree");
        assert_eq!(first["sessions"][&ids[0]]["project"], project);
        assert_eq!(first["sessions"][&ids[0]]["on"]["away"], true);
        assert_eq!(first["projects"][&project]["shown"], json!(ids[..6]));
        assert_eq!(first["projects"][&project]["total"], 18);
        assert_eq!(first["projects"][&project]["more"]["count"], 10);
        assert!(
            first["projects"]
                .get(format!("{key}:/scratch/worktree"))
                .is_none()
        );
        assert_eq!(
            first["archivedGroups"],
            json!([
                {"directory":project,"title":"source","kind":"project","sessions":["ses_old"]}
            ])
        );
        let page = |cursor: String| {
            format!("kuvo/sessions/page?project={key}%3A%2Fscratch%2Fsource&cursor={cursor}")
        };
        let (status, _) = request(&record, "GET", &page("s10000.0".into()), None, false).await;
        assert_eq!(status, 400);
        let cursor = first["projects"][&project]["more"]["cursor"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, next) = request(&record, "GET", &page(cursor), None, false).await;
        assert_eq!(status, 200);
        assert_eq!(next["sessions"], json!(ids[..16]));
        assert_eq!(next["more"]["count"], 2);
        let (status, last) = request(
            &record,
            "GET",
            &page(next["more"]["cursor"].as_str().unwrap().into()),
            None,
            false,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            last,
            json!({"project":project,"sessions":ids,"total":18,"more":null})
        );
    })
    .await
    .expect("the kept project and both pages");
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
        legacy_enrollers: vec![],
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

mod work {
    use super::*;
    use kuvo_peers::{
        calls::{
            Admission, Answer, Lead, MailedCall, Permission, RemoteWork, Request, Resource, Rule,
            Tool, WorkFuture,
        },
        directory::{ComputerKey, Member, Roster},
        remote_work::{Outcome, WorkHandle},
        setup_lists::{Reader, ResidentPeers},
        work_rows,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    };

    const NOW: i64 = 1_800_000_000_000;
    const LEAD: &str = "ses_business";
    const MACHINE_LEAD: &str = "ses_machinelead";
    const BILLING: &str = "ses_billing";
    const OTHER: &str = "ses_other";
    const MISSING: &str = "ses_missing";
    const STOPPED: &str = "Stopped Billing";
    const LIST: &str = r#"{"rows":[{"session":"ses_billing","state":"idle"}]}"#;

    // A stand-in for the external session engine. Effects and queued messages use the same
    // durable operation identity across resident restarts.
    struct Engine {
        effects: std::path::PathBuf,
        notes: mpsc::UnboundedSender<(String, String)>,
        ask_first: AtomicBool,
        deny_push: AtomicBool,
        ask_push: AtomicBool,
    }

    impl RemoteWork for Engine {
        fn inspect<'a>(
            &'a self,
            _: ComputerKey,
            call: &'a MailedCall,
        ) -> WorkFuture<'a, Admission> {
            Box::pin(async move {
                if call.request.session() == Some(MISSING) {
                    return Ok(Admission::Reject(format!(
                        "There's no session {MISSING} on this computer."
                    )));
                }
                if !matches!(call.from.as_str(), LEAD | MACHINE_LEAD)
                    || call.request.session().is_some_and(|id| id != BILLING)
                {
                    return Ok(Admission::Refused(Answer::refused(
                        "Only sessions started by this lead",
                    )));
                }
                if call.request.tool == Tool::AnswerAsk {
                    let command = call
                        .request
                        .args
                        .get("ask")
                        .and_then(Value::as_str)
                        .unwrap_or("git status");
                    let local_effect = if self.deny_push.load(Ordering::Relaxed) {
                        Some("deny")
                    } else if self.ask_push.load(Ordering::Relaxed) {
                        Some("ask")
                    } else {
                        None
                    };
                    let local_rules = local_effect
                        .into_iter()
                        .map(|effect| Rule {
                            action: "shell".into(),
                            resource: "git push*".into(),
                            effect: effect.into(),
                        })
                        .collect();
                    return Ok(Admission::Permission(Permission {
                        title: "Billing".into(),
                        sandboxed: false,
                        ask_first: self.ask_first.load(Ordering::Relaxed),
                        checks: vec![Resource {
                            action: "shell".into(),
                            value: command.into(),
                        }],
                        local_rules,
                    }));
                }
                Ok(Admission::Ready)
            })
        }

        fn apply<'a>(&'a self, _: ComputerKey, call: &'a MailedCall) -> WorkFuture<'a, Answer> {
            Box::pin(async move {
                if call.request.tool == Tool::ListSessions
                    && call.request.args.get("wide").and_then(Value::as_bool) == Some(true)
                {
                    let row = json!({
                        "session":BILLING, "title":"職場".repeat(50),
                        "lastReply": "引用 \" 😀".repeat(250), "state":"finished",
                    });
                    return Ok(Answer::rows(vec![row; 90]));
                }
                let text = match call.request.tool {
                    Tool::ListSessions => LIST,
                    Tool::StopSession => STOPPED,
                    Tool::CompactSession => "Summarizing Billing",
                    Tool::AnswerAsk if call.request.args["decision"] == "allow" => "Allowed",
                    Tool::AnswerAsk => "Denied",
                    _ => unreachable!(),
                };
                use std::io::Write;
                let path = self.effects.join(call.id.as_str());
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?
                    .write_all(text.as_bytes())?;
                Ok(Answer {
                    text: text.into(),
                    refused: false,
                })
            })
        }

        fn deliver<'a>(
            &'a self,
            id: &'a kuvo_peers::outbox::Id,
            lead: &'a str,
            answer: &'a Answer,
        ) -> WorkFuture<'a, ()> {
            Box::pin(async move {
                use std::io::Write;
                let path = self.effects.join(id.as_str());
                match fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(path)
                {
                    Ok(mut file) => {
                        file.write_all(answer.text.as_bytes())?;
                        self.notes.send((lead.into(), answer.text.clone())).ok();
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                Ok(())
            })
        }
    }

    fn engine(home: &Path) -> (Arc<Engine>, mpsc::UnboundedReceiver<(String, String)>) {
        let (notes, mailbox) = mpsc::unbounded_channel();
        let effects = home.join("effects");
        fs::create_dir_all(&effects).unwrap();
        (
            Arc::new(Engine {
                effects,
                notes,
                ask_first: false.into(),
                deny_push: false.into(),
                ask_push: false.into(),
            }),
            mailbox,
        )
    }

    fn key(id: u8) -> ComputerKey {
        public(id + 3)
    }

    fn member(id: u8) -> Member {
        Member {
            key: key(id),
            address: Some(address_of(&[id; 32])),
            name: format!("Computer {id}"),
            kind: if id == 1 {
                ComputerKind::Mac
            } else {
                ComputerKind::Machine
            },
        }
    }

    async fn start_work(
        url: &str,
        id: u8,
        home: &Path,
        peers: &[u8],
        now: Arc<AtomicI64>,
        engine: Arc<Engine>,
    ) -> (ResidentPeers, WorkHandle) {
        let members: Vec<_> = peers.iter().copied().map(member).collect();
        let relay = Relay::start(Config {
            key: [id + 3; 32],
            secret: [id; 32],
            url: url.into(),
            grants: HashMap::new(),
            peers: members.iter().filter_map(Member::host).collect(),
            legacy_enrollers: vec![],
            hello: BoxHello {
                v: 1,
                name: member(id).name,
                app: "1".into(),
                kind: member(id).kind,
                engine: None,
            },
        });
        let mut resident = ResidentPeers::start(
            home,
            Roster {
                here: Some(key(id)),
                main: Some(key(1)),
                name: member(id).name,
                members,
            },
            Some(relay),
            None,
            false,
            Arc::new(move || now.load(Ordering::Relaxed)),
        )
        .await
        .unwrap();
        let handle = resident.attach_work(engine).await.unwrap();
        (resident, handle)
    }

    fn request(tool: Tool, args: Value) -> Request {
        Request {
            tool,
            args: args.as_object().unwrap().clone(),
            lead: Lead {
                name: "Business".into(),
                free: true,
                bot: None,
                start: None,
                rules: vec![],
            },
        }
    }

    async fn answer(handle: &WorkHandle, req: Request) -> Answer {
        match handle
            .call(key(2), LEAD.into(), req, false, false)
            .await
            .unwrap()
        {
            Outcome::Answer(answer) => answer,
            other => panic!("expected a work answer, got {other:?}"),
        }
    }

    async fn ready(resident: &ResidentPeers, other: u8, home: &Path) {
        let mut reader = resident.reader();
        loop {
            let path = home.join("run/peer-agents.json");
            if reader.view().relay_connected
                && path.is_file()
                && read(home, "run/peer-agents.json")[encode(&key(other))]["features"]
                    .as_array()
                    .is_some_and(|features| features.iter().any(|name| name == "work.elsewhere"))
            {
                return;
            }
            reader.changed().await.unwrap();
        }
    }

    async fn away(mut reader: Reader, key: ComputerKey, expected: bool) {
        while reader.view().standing(key, reader.now()).away() != expected {
            reader.changed().await.unwrap();
        }
    }

    async fn work_observer(url: &str, id: u8, peer: u8) -> Relay {
        let mut observer = Relay::start(Config {
            url: url.into(),
            key: [id + 3; 32],
            secret: [id; 32],
            grants: HashMap::new(),
            peers: vec![member(peer).host().unwrap()],
            legacy_enrollers: vec![],
            hello: BoxHello {
                v: 1,
                name: "Observer".into(),
                app: "test".into(),
                kind: ComputerKind::Machine,
                engine: None,
            },
        });
        while !matches!(observer.events.recv().await, Some(Event::Connected(_))) {}
        observer
    }

    async fn send_work_mail(observer: &Relay, to: u8, fields: Value) {
        observer
            .sender
            .mail(
                member(to).host().unwrap(),
                kuvo_wire::mail::Envelope(fields.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
    }

    async fn next_rows(observer: &mut Relay) -> Value {
        loop {
            match observer.events.recv().await {
                Some(Event::Mail { envelope, .. }) => {
                    if let Some(rows) = envelope.0.get("rows") {
                        return rows.clone();
                    }
                }
                Some(_) => {}
                None => panic!("the work rows relay stopped"),
            }
        }
    }

    #[tokio::test]
    async fn work_rows_go_only_to_the_leads_computer_retry_until_acknowledged_and_wait_while_away()
    {
        time::timeout(Duration::from_secs(27), async {
            let listener = relay_socket().await;
            let url = relay_url(&listener);
            let peers = Destinations::default();
            let _relay = run_relay_with(listener, peers.clone());
            let b = tempfile::tempdir().unwrap();
            let other_lead = "ses_anotherlead";
            let a_key = encode(&key(1));
            let c_key = encode(&key(3));
            write(
                b.path(),
                "run/work/homes.json",
                json!({
                    (LEAD): {"key":a_key,"at":NOW}, (other_lead):{"key":c_key,"at":NOW},
                }),
            );
            let (engine_b, _) = engine(b.path());
            let now = Arc::new(AtomicI64::new(NOW));
            let mut a = work_observer(&url, 1, 2).await;
            let mut c = work_observer(&url, 3, 2).await;
            let (b_box, work) = start_work(&url, 2, b.path(), &[3, 1], now.clone(), engine_b).await;
            let mut reader = b_box.reader();
            while !reader.view().relay_connected {
                reader.changed().await.unwrap();
            }
            let announce =
                |at| json!({"at":at,"agents":{"at":at,"list":[],"features":["work.elsewhere"]}});
            send_work_mail(&a, 2, announce(NOW)).await;
            send_work_mail(&c, 2, announce(NOW)).await;
            ready(&b_box, 1, b.path()).await;
            ready(&b_box, 3, b.path()).await;
            let snapshot = |title, both| {
                let row = |session| json!([{"session":session,"title":title,"state":"idle"}]);
                let mut leads = json!({(LEAD):row(BILLING)});
                if both {
                    leads[other_lead] = row(OTHER);
                }
                serde_json::from_value(leads).unwrap()
            };
            work.publish(snapshot("First copy", false)).await.unwrap();
            let first = next_rows(&mut a).await;
            assert_eq!(first["leads"][LEAD][0]["title"], "First copy");
            assert_eq!(
                read(b.path(), "run/work/pushed.json")
                    .as_object()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>(),
                [&a_key]
            );

            let ack = json!({"delivered":{"id":first["id"],"ok":true},"at":NOW});
            send_work_mail(&c, 2, ack.clone()).await;
            now.store(NOW + 61_000, Ordering::Relaxed);
            let repeated = next_rows(&mut a).await;
            assert_eq!(repeated["id"], first["id"]);
            assert_eq!(repeated["at"], NOW + 61_000);
            send_work_mail(&a, 2, ack).await;
            while read(b.path(), "run/work/pushed.json")[&a_key]["done"] != true {
                reader.changed().await.unwrap();
            }

            now.store(NOW + 62_000, Ordering::Relaxed);
            work.publish(snapshot("Within the spacing window", true))
                .await
                .unwrap();
            let theirs = next_rows(&mut c).await;
            assert_eq!(
                theirs["leads"][other_lead][0]["title"],
                "Within the spacing window"
            );
            assert_eq!(
                read(b.path(), "run/work/pushed.json")[&a_key]["id"],
                first["id"]
            );

            let destination = peers.lock().await[&member(1).address.unwrap()].clone();
            drop(a);
            destination.closed().await;
            let b_relay = b_box.relay_handle().unwrap();
            b_relay
                .sender
                .mail(
                    member(1).host().unwrap(),
                    kuvo_wire::mail::Envelope(Default::default()),
                )
                .await
                .unwrap();
            let is_away = |home: &Path| {
                fs::read(home.join("run/peer-reach.json"))
                    .ok()
                    .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                    .is_some_and(|copy| copy[&a_key] == NOW + 62_000)
            };
            while !is_away(b.path()) {
                reader.changed().await.unwrap();
            }

            now.store(NOW + 80_000, Ordering::Relaxed);
            work.publish(snapshot("Fresh copy", true)).await.unwrap();
            let theirs = next_rows(&mut c).await;
            assert_eq!(theirs["leads"][other_lead][0]["title"], "Fresh copy");
            assert_eq!(theirs["leads"][LEAD], Value::Null);
            let pushed = read(b.path(), "run/work/pushed.json");
            assert_eq!(pushed[&a_key]["id"], first["id"]);
            assert_eq!(pushed[&a_key]["done"], true);

            let mut a = work_observer(&url, 1, 2).await;
            send_work_mail(&a, 2, announce(NOW + 80_000)).await;
            let returned = next_rows(&mut a).await;
            assert_eq!(returned["leads"][LEAD][0]["title"], "Fresh copy");
            assert_ne!(returned["id"], first["id"]);
        })
        .await
        .expect("rows go only to the owner and retry until its receipt");
    }

    #[tokio::test]
    async fn a_session_has_a_remote_owner_only_while_one_computer_claims_it() {
        use kuvo_peers::directory::RecordScope;

        time::timeout(Duration::from_secs(10), async {
            let listener = relay_socket().await;
            let url = relay_url(&listener);
            let _relay = run_relay(listener);
            let home = tempfile::tempdir().unwrap();
            let (engine, _) = engine(home.path());
            let now = Arc::new(AtomicI64::new(NOW));
            let (resident, _) = start_work(&url, 1, home.path(), &[2, 3], now, engine).await;
            let second = work_observer(&url, 2, 1).await;
            let third = work_observer(&url, 3, 1).await;
            let mut reader = resident.reader();
            let owner = |reader: &Reader, id| {
                reader
                    .view()
                    .owner_of(RecordScope::Session, id, NOW)
                    .and_then(|owner| owner.key)
            };
            let row = |id| json!({"session":id,"name":id,"kind":"session"});
            send_work_mail(
                &second,
                1,
                json!({"at":NOW,
                    "agents":{"at":NOW,"list":[row(SESSION),row("invalid")]}
                }),
            )
            .await;
            while owner(&reader, SESSION).is_none() {
                reader.changed().await.unwrap();
            }
            assert_eq!(owner(&reader, SESSION), Some(key(2)));
            let kept = read(home.path(), "run/peer-agents.json");
            assert_eq!(kept[encode(&key(2))]["list"], json!([row(SESSION)]));

            send_work_mail(
                &second,
                1,
                json!({"at":NOW,"agents":{"at":NOW-1,"list":[]},
                    "setupLists":{"at":NOW,"sessions":[],"projects":[],"queues":[],"waiting":[],
                        "catalog":kuvo_peers::lists::Catalog::default()}
                }),
            )
            .await;
            while !home
                .path()
                .join(format!("run/peers/{}/lists-v1.json", encode(&key(2))))
                .is_file()
            {
                reader.changed().await.unwrap();
            }
            assert_eq!(owner(&reader, SESSION), Some(key(2)));

            send_work_mail(
                &third,
                1,
                json!({"at":NOW,"agents":{"at":NOW,"list":[row(SESSION),row(OTHER)]}}),
            )
            .await;
            while owner(&reader, OTHER).is_none() {
                reader.changed().await.unwrap();
            }
            assert_eq!(owner(&reader, OTHER), Some(key(3)));
            assert_eq!(owner(&reader, SESSION), None);

            send_work_mail(
                &third,
                1,
                json!({"at":NOW+1,"agents":{"at":NOW+1,"list":[]}}),
            )
            .await;
            while owner(&reader, OTHER).is_some() {
                reader.changed().await.unwrap();
            }
            assert_eq!(owner(&reader, SESSION), Some(key(2)));
        })
        .await
        .expect("authenticated announcements update remote ownership in order");
    }

    #[tokio::test]
    async fn peers_announce_changes_answer_refreshes_and_repeat_after_ten_minutes() {
        time::timeout(Duration::from_secs(18), async {
            let listener = relay_socket().await;
            let url = relay_url(&listener);
            let _relay = run_relay(listener);
            let a = tempfile::tempdir().unwrap();
            let (engine_a, _) = engine(a.path());
            let now = Arc::new(AtomicI64::new(NOW));
            // The observing computer sends no startup advertisements of its own. The
            // announcement carrying the attached work feature ends the initial exchange.
            let mut observer = work_observer(&url, 2, 1).await;
            let (a_box, _) = start_work(&url, 1, a.path(), &[2], now.clone(), engine_a).await;
            let b_relay = observer.handle();
            let events = &mut observer.events;
            loop {
                if let Some(Event::Mail { envelope, .. }) = events.recv().await
                    && envelope.0.get("agents").and_then(|entry| entry["features"].as_array())
                        .is_some_and(|features| features.iter().any(|feature| feature == "work.elsewhere")) {
                    assert_eq!(envelope.0.get("ask"), Some(&json!("agents")));
                    break;
                }
            }
            now.store(NOW + 60_001, Ordering::Relaxed);
            loop {
                if let Some(Event::Mail { envelope, key: sender, .. }) = events.recv().await
                    && sender == key(1)
                    && envelope.0.get("at").and_then(Value::as_i64) == Some(NOW + 60_001) {
                    assert!(!envelope.0.contains_key("agents"), "an unchanged announcement waits ten minutes");
                    if envelope.0.contains_key("activity") { break; }
                }
            }

            now.store(NOW + 601_000, Ordering::Relaxed);
            loop {
                if let Some(Event::Mail { envelope, key: sender, .. }) = events.recv().await
                    && sender == key(1)
                    && let Some(agents) = envelope.0.get("agents")
                    && agents["at"] == NOW + 601_000 {
                    assert_eq!(agents["list"], json!([]));
                    break;
                }
            }

            a_box.local(kuvo_peers::lists::Local {
                list: Some(serde_json::from_value(json!({"epoch":"work","seq":1,"projects":{},"sessions":{
                    (LEAD): { "facts": { "kind":"chat","directory":"/scratch","title":"Business", "created":10, "updated":20 },
                        "place":"engine" }
                }})).unwrap()),
                ..Default::default()
            }).await;
            loop {
                if let Some(Event::Mail { envelope, key: sender, .. }) = events.recv().await
                    && sender == key(1)
                    && let Some(agents) = envelope.0.get("agents")
                    && agents["list"][0]["session"] == LEAD {
                    assert_eq!(agents["list"], json!([{"session":LEAD,"name":"Business","kind":"session"}]));
                    assert_eq!(envelope.0.get("at"), Some(&json!(NOW + 601_000)));
                    break;
                }
            }

            let value = json!({"ask":"agents", "at":NOW + 601_000});
            b_relay.sender.mail(member(1).host().unwrap(), kuvo_wire::mail::Envelope(value.as_object().unwrap().clone())).await.unwrap();
            loop {
                if let Some(Event::Mail { envelope, key: sender, .. }) = events.recv().await
                    && sender == key(1)
                    && let Some(agents) = envelope.0.get("agents") {
                    assert_eq!(agents["list"][0]["session"], LEAD);
                    assert!(!envelope.0.contains_key("ask"), "the reply does not ask again");
                    break;
                }
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn a_lead_can_list_summarize_and_stop_work_elsewhere_and_only_answer_within_its_rules() {
        time::timeout(Duration::from_secs(18), async {
            let listener = relay_socket().await;
            let url = relay_url(&listener);
            let _relay = run_relay(listener);
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let (engine_a, _) = engine(a.path());
            let (engine_b, _) = engine(b.path());
            let now = Arc::new(AtomicI64::new(NOW));
            let (a_box, a_work) = start_work(&url, 1, a.path(), &[2], now.clone(), engine_a).await;
            let (b_box, b_work) = start_work(&url, 2, b.path(), &[1], now, engine_b.clone()).await;
            ready(&a_box, 2, a.path()).await;
            ready(&b_box, 1, b.path()).await;

            let list = answer(&a_work, request(Tool::ListSessions, json!({}))).await;
            assert_eq!((list.text.as_str(), list.refused), (LIST, false));
            let from_machine = b_work
                .call(
                    key(1),
                    MACHINE_LEAD.into(),
                    request(Tool::ListSessions, json!({})),
                    false,
                    false,
                )
                .await
                .unwrap();
            let Outcome::Answer(from_machine) = from_machine else {
                panic!("expected a reply from the Mac, got {from_machine:?}");
            };
            assert_eq!(
                (from_machine.text.as_str(), from_machine.refused),
                (LIST, false)
            );
            assert_eq!(
                answer(
                    &a_work,
                    request(Tool::CompactSession, json!({"session":BILLING}))
                )
                .await
                .text,
                "Summarizing Billing"
            );
            let foreign = answer(
                &a_work,
                request(Tool::StopSession, json!({"session":OTHER})),
            )
            .await;
            assert!(foreign.refused);
            assert_eq!(foreign.text, "Only sessions started by this lead");
            assert_eq!(
                answer(
                    &a_work,
                    request(Tool::StopSession, json!({"session":BILLING}))
                )
                .await
                .text,
                STOPPED
            );

            let allow = || {
                request(
                    Tool::AnswerAsk,
                    json!({"session":BILLING,"ask":"git push origin main","decision":"allow"}),
                )
            };
            engine_b.deny_push.store(true, Ordering::Relaxed);
            let blocked_here = answer(&a_work, allow()).await;
            assert!(blocked_here.refused && blocked_here.text.starts_with("You can't allow that"));
            engine_b.deny_push.store(false, Ordering::Relaxed);
            engine_b.ask_push.store(true, Ordering::Relaxed);
            let blocked_by_local_ask = answer(&a_work, allow()).await;
            assert!(
                blocked_by_local_ask.refused
                    && blocked_by_local_ask
                        .text
                        .starts_with("You can't allow that")
            );
            engine_b.ask_push.store(false, Ordering::Relaxed);

            let rule = |effect: &str| Rule {
                action: "shell".into(),
                resource: "*".into(),
                effect: effect.into(),
            };
            let mut lead_asks = allow();
            lead_asks.lead.rules = vec![rule("allow"), rule("ask")];
            let blocked_at_home = answer(&a_work, lead_asks).await;
            assert!(
                blocked_at_home.refused && blocked_at_home.text.starts_with("You can't allow that")
            );

            let mut sandboxed = allow();
            sandboxed.lead.free = false;
            let blocked_sandbox = answer(&a_work, sandboxed).await;
            assert!(blocked_sandbox.refused);
            let mut oversized = allow();
            oversized.lead.rules = vec![rule("allow"); 301];
            assert!(answer(&a_work, oversized).await.refused);

            engine_b.ask_first.store(true, Ordering::Relaxed);
            let ask_first = answer(&a_work, allow()).await;
            assert!(ask_first.refused && ask_first.text.contains("Ask First"));
            assert_eq!(
                answer(
                    &a_work,
                    request(
                        Tool::AnswerAsk,
                        json!({"session":BILLING,"ask":"git push origin main","decision":"deny"})
                    )
                )
                .await
                .text,
                "Denied"
            );
            engine_b.ask_first.store(false, Ordering::Relaxed);
            assert_eq!(answer(&a_work, allow()).await.text, "Allowed");
            assert_eq!(fs::read_dir(&engine_b.effects).unwrap().count(), 5);
        })
        .await
        .unwrap();
    }

    async fn send_wire_call(resident: &ResidentPeers, given: Value) -> (Value, Value) {
        let handle = resident.relay_handle().unwrap();
        let mut events = handle.subscribe().unwrap();
        let id = given["id"].clone();
        let suffix = id.as_str().unwrap().strip_prefix("cal_").unwrap();
        assert!(
            (16..=40).contains(&suffix.len()),
            "invalid call fixture {id}"
        );
        let answer = format!("{}-a", id.as_str().unwrap());
        let envelope =
            kuvo_wire::mail::Envelope(json!({"call":given,"at":NOW}).as_object().unwrap().clone());
        handle
            .sender
            .mail(member(2).host().unwrap(), envelope)
            .await
            .unwrap();
        let (mut receipt, mut letter) = (None, None);
        loop {
            if let kuvo_peers::relay::RelayEvent::Mail {
                envelope,
                key: sender,
                ..
            } = &*events.recv().await.unwrap()
                && *sender == key(2)
            {
                if let Some(got) = envelope.0.get("delivered").filter(|got| got["id"] == id) {
                    if got["ok"] == false {
                        return (got.clone(), Value::Null);
                    }
                    receipt = Some(got.clone());
                }
                if let Some(got) = envelope.0.get("letter").filter(|got| got["id"] == answer) {
                    letter = Some(got.clone());
                }
            }
            if let (Some(receipt), Some(letter)) = (&receipt, &letter) {
                return (receipt.clone(), letter.clone());
            }
        }
    }

    #[tokio::test]
    async fn a_resent_call_keeps_its_answer_after_restart_and_an_expired_allow_has_no_effect() {
        time::timeout(Duration::from_secs(12), async {
            let listener = relay_socket().await;
            let url = relay_url(&listener);
            let peers = Destinations::default();
            let _relay = run_relay_with(listener, peers.clone());
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let (engine_a, _) = engine(a.path());
            let (engine_b, _) = engine(b.path());
            write(b.path(), "run/work/homes.json", json!({"ses_wronghome":{"key":encode(&key(3)), "at":NOW}}));
            let now = Arc::new(AtomicI64::new(NOW));
            let (a_box, _) = start_work(&url, 1, a.path(), &[2], now.clone(), engine_a).await;
            let (b_box, _) = start_work(&url, 2, b.path(), &[1, 3], now.clone(), engine_b.clone()).await;
            ready(&a_box, 2, a.path()).await;
            let given = |id: &str, tool: &str, args: Value, made: i64| json!({
                "id":id,"tool":tool,"from":LEAD,"as":{"name":"Business","rules":[],"free":true},
                "args":args, "made":made, "at":NOW
            });
            let id = "cal_abcdefgh01234567";
            let call = given(id, "stop_session", json!({"session":BILLING}), NOW);
            let (receipt, answer) = send_wire_call(&a_box, call.clone()).await;
            assert_eq!(receipt, json!({"id":id,"ok":true}));
            assert_eq!(answer["note"], json!({
                "from":"Computer 2","words":STOPPED,"line":STOPPED,"delivery":"queue","about":[BILLING],
            }));
            let old = peers.lock().await[&member(2).address.unwrap()].clone();
            drop(b_box);
            old.closed().await;
            let (b_box, _) = start_work(&url, 2, b.path(), &[1, 3], now, engine_b.clone()).await;
            ready(&b_box, 1, b.path()).await;
            assert_eq!(send_wire_call(&a_box, call).await, (receipt, answer));
            assert_eq!(fs::read_to_string(engine_b.effects.join(id)).unwrap(), STOPPED);

            let (receipt, letter) = send_wire_call(&a_box, given("cal_deadline01234567", "answer_ask",
                json!({"session":BILLING,"ask":"git status","decision":"allow","reason":"Check current work"}),
                NOW - 86_400_001)).await;
            assert_eq!(receipt["ok"], true);
            assert_eq!(letter["refused"], true);
            assert!(letter["note"]["words"].as_str().unwrap().contains("away for over a day"));
            assert_eq!(fs::read_dir(&engine_b.effects).unwrap().count(), 1);

            let (_, list) = send_wire_call(&a_box, given("cal_wide000001234567", "list_sessions", json!({"wide":true}), NOW)).await;
            assert_eq!(list["read"], json!({}));
            let text = list["text"].as_str().unwrap();
            assert!(text.len() <= 22_000);
            let answer: Value = serde_json::from_str(text).expect("complete list JSON");
            let rows = answer["rows"].as_array().unwrap();
            assert!(!rows.is_empty() && rows.len() < 90);
            assert!(rows.iter().all(|row| row["title"] == "職場".repeat(50)));

            for sequence in 0..25 {
                let id = format!("cal_throughout_{sequence:08}");
                let (receipt, list) = send_wire_call(&a_box, given(&id, "list_sessions", json!({}), NOW)).await;
                assert_eq!(receipt, json!({"id":id, "ok":true}));
                assert_eq!(list["text"], LIST);
            }
            let (refused, letter) = send_wire_call(&a_box, given("cal_badtools01234567", "prompt_session", json!({}), NOW)).await;
            assert_eq!(refused, json!({
                "id":"cal_badtools01234567","ok":false,
                "why":"That isn't something another computer can do here.",
            }));
            assert!(letter.is_null());

            let mut local = kuvo_peers::lists::Local::default();
            local.ownership.held.insert((kuvo_peers::directory::RecordScope::Session, "ses_ownlead".into()));
            let mut reader = b_box.reader();
            b_box.local(local).await;
            while reader.view().local.ownership.held.is_empty() { reader.changed().await.unwrap(); }
            let stop = |id| given(id, "stop_session", json!({"session":BILLING}), NOW);
            let mut own = stop("cal_local00001234567");
            own["from"] = "ses_ownlead".into();
            assert_eq!(send_wire_call(&a_box, own).await.0, json!({
                "id":"cal_local00001234567","ok":false,
                "why":"That conversation is on this computer: another can't speak for it.",
            }));
            let mut claimed = stop("cal_claim00123456789");
            claimed["from"] = "ses_wronghome".into();
            let (receipt, _) = send_wire_call(&a_box, claimed).await;
            assert_eq!((receipt["ok"].clone(), receipt["retry"].clone()), (json!(false), json!(true)));
            let mut old = stop("cal_stale00123456789");
            old["at"] = (NOW - 172_800_000).into();
            let (receipt, _) = send_wire_call(&a_box, old).await;
            assert_eq!((receipt["ok"].clone(), receipt["retry"].clone()), (json!(false), json!(true)));
            assert_eq!(fs::read_dir(&engine_b.effects).unwrap().count(), 26);

        }).await.unwrap();
    }

    #[test]
    fn an_away_peer_gets_its_oldest_call_at_each_retry_until_it_returns() {
        use kuvo_peers::calls::{Calls, Home, Outgoing, PeerState};
        let run = tempfile::tempdir().unwrap();
        let mut outbox = Calls::open(run.path());
        let enqueue = |outbox: &mut Calls, session, at| {
            outbox
                .enqueue(
                    key(2),
                    LEAD.into(),
                    request(Tool::StopSession, json!({"session":session})),
                    false,
                    false,
                    at,
                )
                .unwrap()
        };
        let first = enqueue(&mut outbox, BILLING, NOW);
        let second = enqueue(&mut outbox, OTHER, NOW + 1);
        let send = |outbox: &mut Calls, now, state| {
            outbox.round(now, true, |_| state, |_| Home::Local).unwrap()
        };
        let id = |mail: &Outgoing| match mail {
            Outgoing::Mail { envelope, .. } => {
                envelope.0["call"]["id"].as_str().unwrap().to_owned()
            }
            Outgoing::Failed(_) => panic!("retry unexpectedly failed"),
        };
        let (one, retry) = send(&mut outbox, NOW + 1, PeerState::Away);
        assert_eq!(one.len(), 1);
        assert_eq!(id(&one[0]), first.as_str());
        assert_eq!(retry, Some(NOW + 60_001));
        let (waiting, retry) = send(&mut outbox, NOW + 2000, PeerState::Away);
        assert!(waiting.is_empty());
        assert_eq!(retry, Some(NOW + 60_001));
        assert_eq!(
            id(&send(&mut outbox, NOW + 60_001, PeerState::Away).0[0]),
            first.as_str()
        );
        outbox.heard(key(2), NOW + 60_002);
        let (back, _) = send(&mut outbox, NOW + 60_002, PeerState::Online);
        assert_eq!(
            back.iter().map(id).collect::<Vec<_>>(),
            [first.as_str(), second.as_str()]
        );
        outbox.finish(&first).unwrap();
        outbox.finish(&second).unwrap();
        let (empty, retry) = send(&mut outbox, NOW + 60_003, PeerState::Online);
        assert!(empty.is_empty());
        assert_eq!(retry, None);
    }

    #[tokio::test]
    async fn an_away_computer_keeps_its_last_work_rows_and_the_stop_reaches_it_on_return() {
        time::timeout(Duration::from_secs(30), async {
            let listener = relay_socket().await;
            let url = relay_url(&listener);
            let peers = Destinations::default();
            let _relay = run_relay_with(listener, peers.clone());
            let a = tempfile::tempdir().unwrap();
            let b = tempfile::tempdir().unwrap();
            let (engine_a, mut mailbox) = engine(a.path());
            let (engine_b, _) = engine(b.path());
            let now = Arc::new(AtomicI64::new(NOW));
            let (a_box, a_work) = start_work(&url, 1, a.path(), &[2], now.clone(), engine_a).await;
            let (b_box, b_work) =
                start_work(&url, 2, b.path(), &[1], now.clone(), engine_b.clone()).await;
            ready(&a_box, 2, a.path()).await;
            ready(&b_box, 1, b.path()).await;
            assert_eq!(
                answer(&a_work, request(Tool::ListSessions, json!({})))
                    .await
                    .text,
                LIST
            );
            let snapshot = serde_json::from_value(
                json!({(LEAD):[{"session":BILLING,"state":"idle","title":"Billing"}]}),
            )
            .unwrap();
            b_work.publish(snapshot).await.unwrap();

            let mut reader = a_box.reader();
            loop {
                let rows = work_rows::for_lead(&reader, LEAD).await.unwrap();
                if let [told] = rows.as_slice() {
                    assert_eq!(told.rows[0].session, BILLING);
                    assert_eq!(told.at, NOW);
                    break;
                }
                reader.changed().await.unwrap();
            }

            let destination = peers.lock().await[&member(2).address.unwrap()].clone();
            drop(b_box);
            destination.closed().await;
            // The list itself encounters the missing relay link. It returns immediately
            // on the relay's refusal, well before the ordinary call timeout.
            let unavailable = time::timeout(
                Duration::from_secs(2),
                a_work.call(key(2), LEAD.into(), request(Tool::ListSessions, json!({})), false, false),
            ).await.expect("the relay reports that the computer is away").unwrap();
            assert!(matches!(unavailable, Outcome::Away { kept: false, .. }));
            let mut changes = a_box.reader();
            let known_away = |home: &Path| {
                fs::read(home.join("run/peer-reach.json")).ok()
                    .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                    .is_some_and(|record| record[encode(&key(2))] == NOW)
            };
            while !known_away(a.path()) {
                changes.changed().await.unwrap();
            }
            now.store(NOW + 65_000, Ordering::Relaxed);
            away(a_box.reader(), key(2), true).await;
            let rows = work_rows::for_lead(&reader, LEAD).await.unwrap();
            assert_eq!(rows[0].standing.last_seen(), Some(NOW));
            let stopped = a_work
                .call(
                    key(2),
                    LEAD.into(),
                    request(Tool::StopSession, json!({"session":BILLING})),
                    true,
                    false,
                )
                .await
                .unwrap();
            assert!(matches!(
                stopped,
                Outcome::Away {
                    kept: true,
                    sent: false,
                    ..
                }
            ));
            assert!(mailbox.try_recv().is_err());
            let offer = fs::read_dir(a.path().join("run/outbox/sending"))
                .unwrap()
                .map(|entry| {
                    read(
                        a.path(),
                        entry
                            .unwrap()
                            .path()
                            .strip_prefix(a.path())
                            .unwrap()
                            .to_str()
                            .unwrap(),
                    )
                })
                .find(|entry| entry["call"]["tool"] == "stop_session")
                .unwrap();
            assert_eq!(offer["from"], LEAD);
            let quiet = a_work.call(key(2), LEAD.into(), request(Tool::StopSession, json!({"session":MISSING})),
                true, true).await.unwrap();
            assert!(matches!(quiet, Outcome::Away { kept: true, sent: false, .. }));
            let doomed = a_work.call(key(2), LEAD.into(), request(Tool::StopSession, json!({"session":MISSING})),
                true, false).await.unwrap();
            assert!(matches!(doomed, Outcome::Away { kept: true, sent: false, .. }));


            now.store(NOW + 80_000, Ordering::Relaxed);
            let (returned, fresh_work) =
                start_work(&url, 2, b.path(), &[1], now.clone(), engine_b.clone()).await;
            away(a_box.reader(), key(2), false).await;
            let mut results = vec![mailbox.recv().await.unwrap(), mailbox.recv().await.unwrap()];
            results.sort();
            assert_eq!(results, [
                (LEAD.into(), format!("[Not done] Your stop_session for {MISSING} on Computer 2: There's no session {MISSING} on this computer.")),
                (LEAD.into(), "[On Computer 2] Stopped Billing".into()),
            ]);
            write(a.path(), "run/outbox/sending/cal_ticket0000000000.json", json!({
                "call": request(Tool::AnswerAsk, json!({"session":BILLING,"decision":"allow"})),
                "peer":encode(&key(2)), "from":LEAD, "to":"", "at":NOW - 86_400_000,
            }));
            let (lead, words) = mailbox.recv().await.unwrap();
            assert_eq!(lead, LEAD);
            assert_eq!(words, format!("[Not done] Your answer_ask for {BILLING} on Computer 2 wasn't done: that computer wasn't back in time. An allow waits a day at most, as what it was for may have changed. Look at what it asks with list_sessions once it's back."));
            assert_eq!(fs::read_dir(&engine_b.effects).unwrap().count(), 2);
            assert!(mailbox.try_recv().is_err());

            // The returned engine's full empty snapshot must clear the old list.
            fresh_work.publish(Default::default()).await.unwrap();
            while !work_rows::for_lead(&reader, LEAD).await.unwrap().is_empty() {
                reader.changed().await.unwrap();
            }
            let origins = read(a.path(), "run/work/homes.json");
            assert_eq!(origins[BILLING]["key"], encode(&key(2)));
            assert_eq!(origins[BILLING]["at"], NOW);
            let relay = returned.relay_handle().unwrap();
            let mut events = relay.subscribe().unwrap();
            let stale_id = "rws_replayed";
            let stale = json!({"at":NOW, "rows":{"id":stale_id,"at":NOW, "leads":{(LEAD):[{"session":BILLING,"title":"Old copy","state":"idle"}]}}});
            relay.sender.mail(member(1).host().unwrap(), kuvo_wire::mail::Envelope(stale.as_object().unwrap().clone())).await.unwrap();
            loop {
                if let kuvo_peers::relay::RelayEvent::Mail { envelope, .. } = &*events.recv().await.unwrap()
                    && let Some(receipt) = envelope.0.get("delivered")
                    && receipt["id"] == stale_id {
                    assert_eq!(receipt["ok"], true);
                    break;
                }
            }
            assert!(work_rows::for_lead(&reader, LEAD).await.unwrap().is_empty());

            // A newer copy still arrives, but only well-formed rows reach the list.
            now.store(NOW + 81_000, Ordering::Relaxed);
            let fresh = json!({"at":NOW+81_000, "rows":{
                "id":"rws_clean_rows", "at":NOW+81_000, "leads":{(LEAD):[
                    {"session":BILLING,"title":"Broken","state":"exploded"},
                    {"session":OTHER,"title":"Two\u{202e} lines\nhere","state":"idle"}
                ]}
            }});
            relay.sender.mail(member(1).host().unwrap(), Envelope(fresh.as_object().unwrap().clone())).await.unwrap();
            loop {
                if let [copy] = work_rows::for_lead(&reader, LEAD).await.unwrap().as_slice() {
                    assert_eq!(copy.rows.len(), 1);
                    assert_eq!(copy.rows[0].session, OTHER);
                    assert_eq!(copy.rows[0].title, "Two lines here");
                    break;
                }
                reader.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }
}
