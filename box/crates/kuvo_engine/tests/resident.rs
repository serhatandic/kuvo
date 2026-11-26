//! The resident, dispatcher and process owner are real. This executable doubles
//! as OpenCode in the scratch engine folder, with a controllable API and PTY.

use std::{
    convert::Infallible,
    fs, io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use kuvo_engine::{
    ActivityState, AgentCatalog, AgentModel, AgentModelSource, AttachmentConversation,
    AttachmentRead, AttachmentSource, AutoModel, CatalogRead, Engine, LocalSessions, ModelAccount,
    ModelSpeed, Timing, pinned_version,
};
use kuvo_os::process::ProcessIdentity;
use kuvo_server::{
    Caller, DispatchRequest, LineTiming, Pairing, Registry, Reply, Resident, Transport,
    ssh::NoPairings,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle, time};
use tokio_tungstenite::{
    WebSocketStream, connect_async,
    tungstenite::{
        Message, Utf8Bytes,
        client::IntoClientRequest,
        handshake::derive_accept_key,
        protocol::{CloseFrame, Role, frame::coding::CloseCode},
    },
};
use tokio_util::sync::CancellationToken;

const IDLE: Duration = Duration::from_secs(600);
const CHECK: Duration = Duration::from_secs(30);

fn main() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mode = std::env::args().nth(1);
    if mode.as_deref() == Some("serve") {
        executor.block_on(fixture());
    } else if mode.as_deref() == Some("ssh-install") {
        executor.block_on(ssh_install_failure());
    } else {
        executor.block_on(async {
            scenarios().await;
            interrupted_inbox_transfer().await;
            starter_stop_leaves_summary_out_of_queue().await;
            attachment_retention().await;
            local_background_activity().await;
            cancelled_session_writers().await;
            transcript_reads_older_pages().await;
            coding_agent_models().await;
            external_asks().await;
            peer_waiting().await;
            paired_delivery().await;
            resident_answers_promptly_under_mutation_lock_contention().await;
        });
    }
}

async fn ssh_install_failure() {
    let home = tempfile::tempdir().unwrap();
    let installed = install(home.path());
    let _cleanup = Cleanup {
        home: home.path().into(),
        installed,
    };
    let mut env: kuvo_os::platform::Environment = std::env::vars_os()
        .filter(|(key, _)| !key.to_string_lossy().starts_with("KUVO_"))
        .collect();
    env.insert("HOME".into(), home.path().into());
    env.insert("USERPROFILE".into(), home.path().into());
    let engine = Engine::new(home.path().into(), env, Timing::default()).unwrap();
    let fail = || io::Error::other("fixture refused credential import");
    let mut owned = None;
    let result = engine
        .with_install_engine(async {
            owned = ProcessIdentity::read(current(home.path())).unwrap();
            assert!(matches!(
                engine.state().await,
                kuvo_engine::State::Running { .. }
            ));
            Err::<(), _>(fail())
        })
        .await;
    assert_eq!(result.unwrap_err().to_string(), fail().to_string());
    assert!(!home.path().join("run/engine.json").exists());
    assert!(!home.path().join("run/ssh-engine.json").exists());
    assert_eq!(
        owned.unwrap().state(),
        kuvo_os::process::ProcessState::Stale
    );

    engine.with_install_engine(async { Ok(()) }).await.unwrap();
    let earlier = ProcessIdentity::read(current(home.path()))
        .unwrap()
        .unwrap();
    assert_eq!(
        engine
            .with_install_engine(async { Err::<(), _>(fail()) })
            .await
            .unwrap_err()
            .to_string(),
        fail().to_string()
    );
    assert_eq!(current(home.path()), earlier.pid);
    assert_eq!(earlier.state(), kuvo_os::process::ProcessState::Live);
    engine.stop_install_engine().await.unwrap();
    assert_eq!(earlier.state(), kuvo_os::process::ProcessState::Stale);
}

fn accepted<T>(result: Result<T, kuvo_server::Refusal>) -> T {
    result.unwrap_or_else(|refused| {
        panic!(
            "The resident refused the request with {}",
            refused.reply().status
        )
    })
}

struct Server {
    stop: CancellationToken,
    task: JoinHandle<io::Result<()>>,
    dispatcher: kuvo_server::Dispatcher,
    engine: Engine,
    sessions: LocalSessions,
    url: String,
    password: String,
    client: reqwest::Client,
}

impl Server {
    async fn start(home: &Path) -> Self {
        Self::start_with(home, |engine| engine).await
    }

    async fn start_with(home: &Path, configure: impl FnOnce(Engine) -> Engine) -> Self {
        let resident = Resident::bind(home).await.unwrap().unwrap();
        let mut routes = Registry::new();
        let mut env: kuvo_os::platform::Environment = std::env::vars_os()
            .filter(|(key, _)| !key.to_string_lossy().starts_with("KUVO_"))
            .collect();
        env.insert("HOME".into(), home.into());
        env.insert("USERPROFILE".into(), home.into());
        if cfg!(windows) {
            env.insert("=C:".into(), home.into());
        }
        let engine = Engine::new(
            home.to_owned(),
            env,
            Timing {
                idle: IDLE,
                check: CHECK,
            },
        )
        .unwrap();
        let engine = configure(engine);
        let sessions = engine.register(&mut routes);
        let dispatcher = routes.build(LineTiming::default());
        let stop = CancellationToken::new();
        let url = format!("127.0.0.1:{}", resident.record().port);
        let password = resident.record().password.clone();
        let task = tokio::spawn(resident.serve(
            dispatcher.clone(),
            Arc::new(NoPairings),
            stop.clone().cancelled_owned(),
        ));
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        Self {
            stop,
            task,
            dispatcher,
            engine,
            sessions,
            url,
            password,
            client,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("http://{}/{path}", self.url))
            .basic_auth("opencode", Some(&self.password))
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap()
    }

    async fn stop(mut self) {
        self.stop.cancel();
        (&mut self.task).await.unwrap().unwrap();
    }

    fn socket(
        &self,
        path: &str,
        authorized: bool,
    ) -> tokio_tungstenite::tungstenite::handshake::client::Request {
        let mut request = format!("ws://{}/{path}", self.url)
            .into_client_request()
            .unwrap();
        if authorized {
            request
                .headers_mut()
                .insert(header::AUTHORIZATION, auth(&self.password).parse().unwrap());
        }
        request
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn scenarios() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let guard = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let server = Server::start(home).await;
    let record = home.join("run/engine.json");
    let work = home.join("workspace");

    assert_eq!(server.get("api/event").await.status(), 503);
    assert_eq!(
        server
            .get("api/session/active")
            .await
            .json::<Value>()
            .await
            .unwrap(),
        json!({"data": []})
    );
    assert_eq!(
        server
            .client
            .get(format!("http://{}/api/echo", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(server.get("api/%2foutside").await.status(), 403);
    assert!(
        !record.exists(),
        "passive or refused requests woke OpenCode"
    );

    fs::write(home.join("run/stopped"), []).unwrap();
    assert_eq!(
        server.get("api/echo").await.json::<Value>().await.unwrap(),
        json!({"message":"The engine isn't running."})
    );
    assert!(!record.exists());
    fs::create_dir(home.join("run/.updating")).unwrap();
    fs::write(
        home.join("run/.updating/pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    fs::write(home.join("run/update.json"), br#"{"running":"installing"}"#).unwrap();
    for action in ["start", "restart"] {
        assert_eq!(
            server
                .request(reqwest::Method::POST, &format!("kuvo/engine/{action}"))
                .send()
                .await
                .unwrap()
                .status(),
            409
        );
    }
    assert!(!record.exists());
    fs::write(
        home.join("run/update.json"),
        br#"{"running":"downloading"}"#,
    )
    .unwrap();
    assert_eq!(
        server
            .request(reqwest::Method::POST, "kuvo/engine/start")
            .send()
            .await
            .unwrap()
            .status(),
        202
    );

    let request = || {
        server
            .request(reqwest::Method::POST, "api/echo?value=a%2Bb")
            .header("x-opencode-directory", "%2Fmy%20folder")
            .header("last-event-id", "17")
            .header("accept", "application/json")
            .header("x-ignored", "never")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(vec![0, 3, 255])
            .send()
    };
    let (a, b) = tokio::join!(request(), request());
    for reply in [a.unwrap(), b.unwrap()] {
        assert_eq!(reply.status(), 207);
        assert_eq!(reply.headers()["cache-control"], "no-cache");
        assert_eq!(reply.headers()["x-kuvo-route-version"], "1");
        assert!(!reply.headers().contains_key("x-private"));
        assert_eq!(
            reply.json::<Value>().await.unwrap(),
            json!({
                "path": "/api/echo?value=a%2Bb", "directory": "%2Fmy%20folder",
                "last": "17", "accept": "application/json", "type": "application/octet-stream",
                "other": null, "body": [0, 3, 255],
            })
        );
    }
    let initial = current(home);
    assert!(!home.join("run/stopped").exists());
    assert_eq!(fs::read_to_string(work.join("launches")).unwrap(), "1\n");
    fs::remove_file(home.join("run/.updating/pid")).unwrap();
    fs::remove_dir(home.join("run/.updating")).unwrap();

    let mut stream = server.get("api/event").await;
    assert_eq!(stream.headers()["content-type"], "text/event-stream");
    assert_eq!(
        stream.chunk().await.unwrap().unwrap(),
        "data: connected\n\n"
    );

    for (path, authorized, status) in [
        ("api/pty/term/connect", false, 401),
        ("api/echo", true, 403),
        ("api/pty/refused/connect", true, 502),
    ] {
        let error = connect_async(server.socket(path, authorized))
            .await
            .unwrap_err();
        assert!(
            matches!(error, tokio_tungstenite::tungstenite::Error::Http(ref reply) if reply.status() == status),
            "{error}"
        );
    }
    let mut request = server.socket("api/pty/term/connect", true);
    request
        .headers_mut()
        .insert("x-kuvo-route-version", "99".parse().unwrap());
    assert!(
        matches!(connect_async(request).await.unwrap_err(), tokio_tungstenite::tungstenite::Error::Http(ref reply) if reply.status() == 400)
    );
    assert!(!work.join("socket").exists());
    let mut request = server.socket("api/pty/term/connect?cursor=12", true);
    request
        .headers_mut()
        .insert("x-opencode-directory", "%2Fhere".parse().unwrap());
    let (mut socket, response) = connect_async(request).await.unwrap();
    assert_eq!(response.headers()["x-kuvo-route-version"], "1");
    assert_eq!(
        fs::read_to_string(work.join("socket")).unwrap(),
        "/api/pty/term/connect?cursor=12 %2Fhere"
    );
    let text = Message::Text(Utf8Bytes::from("é\n"));
    let binary = Message::Binary(Bytes::from_static(&[0, 255]));
    for message in [text, binary] {
        socket.send(message.clone()).await.unwrap();
        assert_eq!(socket.next().await.unwrap().unwrap(), message);
    }
    let close = CloseFrame {
        code: CloseCode::Library(4012),
        reason: "client closed".into(),
    };
    socket.send(Message::Close(Some(close))).await.unwrap();
    assert!(matches!(
        socket.next().await.unwrap().unwrap(),
        Message::Close(_)
    ));
    until(|| work.join("close").exists(), "upstream close").await;
    assert_eq!(
        fs::read_to_string(work.join("close")).unwrap(),
        "4012 client closed"
    );

    let pairing = |current| {
        Pairing::verified([1; 32], "paired".into(), "test".into(), true, move || {
            current
        })
    };
    let request = |route: &str, current| DispatchRequest {
        method: http::Method::GET,
        path: route.into(),
        headers: [(header::AUTHORIZATION, "Basic forged".parse().unwrap())]
            .into_iter()
            .collect(),
        body: Bytes::new(),
        caller: Caller::Paired {
            transport: Transport::Relay,
            pairing: pairing(current),
        },
    };
    let cancel = CancellationToken::new();
    assert_eq!(
        server
            .dispatcher
            .dispatch(request("api/echo", false), cancel.clone())
            .await
            .status,
        403
    );
    let proxied = server
        .dispatcher
        .dispatch(request("api//echo?&&value=a+b", true), cancel.clone())
        .await;
    assert_eq!(
        proxied.status, 207,
        "the upstream must receive the engine password"
    );
    let body = proxied.body.collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["path"],
        "/api/echo?value=a%20b"
    );
    drop(stream);
    until(
        || work.join("stream-ended").exists(),
        "upstream event stream cancellation",
    )
    .await;
    let vectors: Value =
        serde_json::from_str(include_str!("../../../../protocol/vectors/link.json")).unwrap();
    for vector in vectors["routes"].as_array().unwrap() {
        let Some(forwarded) = vector["forwarded"].as_str() else {
            continue;
        };
        let mut call = request(vector["path"].as_str().unwrap(), true);
        call.method = vector["method"].as_str().unwrap_or("GET").parse().unwrap();
        let answer = server.dispatcher.dispatch(call, cancel.clone()).await;
        assert!(answer.status.is_success(), "{vector}: {}", answer.status);
        assert_eq!(
            answer.headers[header::CACHE_CONTROL].to_str().unwrap(),
            format!("/{forwarded}"),
            "{vector}"
        );
        drop(answer);
    }

    let refusal = server
        .dispatcher
        .open_socket(request("api/pty/term/connect", false), cancel.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(refusal.reply().status, 403);
    let paired_cancel = CancellationToken::new();
    let mut paired = server
        .dispatcher
        .open_socket(request("api/pty/term/connect", true), paired_cancel.clone())
        .await
        .map_err(|refusal| refusal.reply().status)
        .unwrap();
    let key = Message::Text("paired terminal".into());
    paired.send.send(key.clone()).await.unwrap();
    assert_eq!(paired.receive.recv().await, Some(key));
    paired_cancel.cancel();
    assert_eq!(paired.receive.recv().await, None);

    server.stop().await;
    assert!(ProcessIdentity::read(initial).unwrap().is_some());
    let server = Server::start(home).await;
    assert_eq!(server.get("api/echo").await.status(), 207);
    assert_eq!(current(home), initial, "restart started a second OpenCode");

    // Parking waits on the real engine API's busy responses and ignores an open event stream.
    let stream = server.get("api/event").await;
    assert_eq!(stream.status(), 200);
    time::pause();
    // Keep the test's clock still while waiting for real loopback requests.
    let clock = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    fs::write(work.join("busy"), b"form").unwrap();
    time::advance(IDLE + CHECK).await;
    until(|| work.join("busy-checked").exists(), "idle probe").await;
    assert!(record.is_file(), "an unanswered question was cut");
    // Restart with the same engine and no busy work, giving the next
    // resident its own idle interval. The old resident's probe is cancelled.
    drop(stream);
    server.stop().await;
    fs::remove_file(work.join("busy")).unwrap();
    let server = Server::start(home).await;
    let stream = server.get("api/event").await;
    assert_eq!(stream.status(), 200);
    time::advance(IDLE + CHECK).await;
    until(|| !record.exists(), "engine to park").await;
    assert!(ProcessIdentity::read(initial).unwrap().is_none());
    drop(stream);
    clock.abort();
    time::resume();
    assert_eq!(server.get("api/event").await.status(), 503);
    assert_eq!(server.get("api/echo").await.status(), 207);
    assert_ne!(current(home), initial);
    assert_eq!(fs::read_to_string(work.join("launches")).unwrap(), "1\n1\n");

    server.stop().await;
    drop(guard);
    println!("cold start, proxy, PTY, adoption, busy parking and wake passed");
}

fn current(home: &Path) -> u32 {
    let record: Value =
        serde_json::from_slice(&fs::read(home.join("run/engine.json")).unwrap()).unwrap();
    record["pid"].as_u64().unwrap() as u32
}

async fn until(condition: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::task::yield_now().await;
    }
}

fn install(home: &Path) -> PathBuf {
    let path = home
        .join("engine")
        .join(pinned_version())
        .join(if cfg!(windows) {
            "opencode.exe"
        } else {
            "opencode"
        });
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
    path
}

struct Cleanup {
    home: PathBuf,
    installed: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let Ok(bytes) = fs::read(self.home.join("run/engine.json")) else {
            return;
        };
        let record: Value = serde_json::from_slice(&bytes).unwrap();
        let pid = record["pid"].as_u64().unwrap() as u32;
        let token = record["password"]
            .as_str()
            .unwrap()
            .strip_prefix("kuvo-")
            .unwrap();
        let job = format!("kuvo-engine-{token}");
        if let Some(mut child) = kuvo_os::process::AdoptedProcess::open(
            pid,
            record["startIdentity"].as_str(),
            |path| path.canonicalize().ok() == self.installed.canonicalize().ok(),
            Some(&job),
        )
        .unwrap()
        {
            child.stop(Duration::ZERO).unwrap();
        }
    }
}

fn auth(password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("opencode:{password}")))
}

async fn attachment_retention() {
    struct AgentInventory {
        available: std::sync::atomic::AtomicBool,
        carried: std::sync::atomic::AtomicBool,
    }

    impl AttachmentSource for AgentInventory {
        fn conversations(&self) -> AttachmentRead<'_, Vec<AttachmentConversation>> {
            Box::pin(async {
                if self.available.load(std::sync::atomic::Ordering::Relaxed) {
                    Ok(if self.carried.load(std::sync::atomic::Ordering::Relaxed) {
                        vec![AttachmentConversation {
                            id: "ses_carried".into(),
                            ancestors: vec!["ses_carriedFrom".into()],
                        }]
                    } else {
                        Vec::new()
                    })
                } else {
                    Err(io::Error::other("The coding-agent roster is unavailable."))
                }
            })
        }

        fn transcript<'a>(&'a self, session: &'a str) -> AttachmentRead<'a, Vec<Value>> {
            Box::pin(async move {
                assert_eq!(session, "ses_carried");
                Ok(vec![
                    json!({"type":"file","url":"file:///scratch/attachments/ses_carriedFrom/notes.txt"}),
                ])
            })
        }

        fn protected(&self, session: &str) -> bool {
            session == "ses_inMove"
        }
    }

    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let inventory = Arc::new(AgentInventory {
        available: false.into(),
        carried: true.into(),
    });
    let files = home.join("attachments");
    for session in [
        "ses_source",
        "ses_sourceWin",
        "ses_branch",
        "ses_inMove",
        "ses_gone",
        "ses_carriedFrom",
    ] {
        let folder = files.join(session);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("notes.txt"), session).unwrap();
    }
    let legacy = home.join("mac-attachment-copies");
    fs::create_dir_all(legacy.join("ses_gone")).unwrap();
    fs::write(legacy.join("ses_gone/notes.txt"), "Legacy copy.").unwrap();
    let work = home.join("workspace");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("event-watch"), []).unwrap();
    let server = Server::start_with(home, |engine| {
        engine.with_attachment_source(inventory.clone(), Some(legacy.clone()))
    })
    .await;
    assert_eq!(server.get("api/echo").await.status(), 207);
    until(
        || work.join("event-subscribed").exists(),
        "the resident to follow attachment events",
    )
    .await;
    let show = work.join("attachments-list.json");
    let branch = json!({"id":"ses_branch","metadata":{"kuvo":{"branchedFrom":"ses_middle"}}});
    fs::write(
        &show,
        serde_json::to_vec(
            &json!([branch, {"id":"ses_middle","metadata":{"kuvo":{"branchedFrom":"ses_source"}}}]),
        )
        .unwrap(),
    )
    .unwrap();
    fs::write(
        work.join("attachment-words.json"),
        serde_json::to_vec(&json!([
            {"type":"user","text":"file:///scratch/attachments/ses_source/notes.txt"},
            {"type":"user","text":r"C:\scratch\attachments\ses_sourceWin\notes.txt"},
        ]))
        .unwrap(),
    )
    .unwrap();
    assert!(server.engine.reconcile_attachments().await.is_err());
    assert!(files.join("ses_gone/notes.txt").exists());
    inventory
        .available
        .store(true, std::sync::atomic::Ordering::Relaxed);
    fs::write(work.join("attachment-inventory-unavailable"), []).unwrap();
    assert!(server.engine.reconcile_attachments().await.is_err());
    assert!(files.join("ses_gone/notes.txt").exists());
    fs::remove_file(work.join("attachment-inventory-unavailable")).unwrap();
    server.engine.reconcile_attachments().await.unwrap();
    assert!(!files.join("ses_gone").exists());
    assert!(legacy.join("ses_gone/notes.txt").exists());
    assert!(files.join("ses_carriedFrom/notes.txt").exists());
    assert!(files.join("ses_source/notes.txt").exists());
    assert!(files.join("ses_sourceWin/notes.txt").exists());
    assert!(files.join("ses_branch/notes.txt").exists());
    assert!(files.join("ses_inMove/notes.txt").exists());
    fs::write(&show, b"[]").unwrap();
    inventory
        .carried
        .store(false, std::sync::atomic::Ordering::Relaxed);
    server.engine.reconcile_attachments().await.unwrap();
    assert!(!files.join("ses_source").exists());
    assert!(!files.join("ses_sourceWin").exists());
    assert!(!files.join("ses_branch").exists());
    assert!(!files.join("ses_carriedFrom").exists());
    assert!(files.join("ses_inMove/notes.txt").exists());

    let deleted = "ses_deletedOnLine";
    for root in [&files, &legacy] {
        let folder = root.join(deleted);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("notes.txt"), "From the deleted conversation.").unwrap();
    }
    let pending = files.join("pending");
    fs::create_dir_all(&pending).unwrap();
    fs::write(pending.join("draft.txt"), "In a draft.").unwrap();
    let event = format!(
        "data: {}\n\n",
        json!({"type":"session.deleted","data":{"sessionID":deleted}})
    );
    fs::write(work.join("fixture-event"), event).unwrap();
    until(
        || !files.join(deleted).exists() && !legacy.join(deleted).exists(),
        "the deletion event to drop only its attachment copies",
    )
    .await;
    assert!(pending.join("draft.txt").is_file());
    assert!(legacy.join("ses_gone/notes.txt").is_file());
    assert!(files.join("ses_inMove/notes.txt").is_file());
    server.stop().await;
}

async fn local_background_activity() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let workspace = home.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let rows = [
        json!({"id":"ses_root","title":"Root","location":{"directory":workspace},"metadata":{"kuvo":{"bot":"bot_coordinator"}}}),
        json!({"id":"ses_sub","title":"Child","location":{"directory":workspace},"parentID":"ses_root"}),
        json!({"id":"ses_started","title":"Delegated","metadata":{"kuvo":{"startedBy":"ses_sub"}}}),
        json!({"id":"ses_unrelated","title":"Other"}),
        json!({"id":"ses_idle","title":"Done","parentID":"ses_root"}),
        json!({"id":"ses_archivedTree","title":"Previous work","location":{"directory":workspace},"time":{"archived":1}}),
        json!({"id":"ses_hiddenTree","title":"Internal","location":{"directory":workspace},"metadata":{"kuvo":{"engineFor":"ses_archivedTree"}}}),
    ];
    fs::write(
        home.join("workspace/attachments-list.json"),
        serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();
    fs::write(
        home.join("workspace/active-sessions.json"),
        br#"{"ses_root":true,"ses_sub":true,"ses_started":true,"ses_unrelated":true}"#,
    )
    .unwrap();
    let server = Server::start(home).await;
    assert_eq!(server.get("api/echo").await.status(), 207);
    until(
        || {
            server
                .sessions
                .activity()
                .iter()
                .any(|row| row.session == "ses_started")
        },
        "the active sessions projection",
    )
    .await;
    let current = server.sessions.activity();
    assert!(current.iter().any(|row| row.session == "ses_root"));
    let background: Vec<_> = current
        .iter()
        .filter(|row| row.works_under("ses_root"))
        .map(|row| {
            (
                row.session.as_str(),
                row.title.as_str(),
                row.state,
                row.subagent,
                row.bot.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        background,
        [
            (
                "ses_started",
                "Delegated",
                ActivityState::Working,
                false,
                Some("bot_coordinator")
            ),
            (
                "ses_sub",
                "Child",
                ActivityState::Working,
                true,
                Some("bot_coordinator")
            ),
        ]
    );
    let inventory = server.engine.worktree_inventory().await.unwrap();
    assert_eq!(inventory.len(), 2);
    assert!(inventory.iter().any(|row| row.session == "ses_root"
        && row.folder == workspace
        && row.running
        && !row.archived));
    assert!(inventory.iter().any(|row| row.session == "ses_archivedTree"
        && row.folder == workspace
        && !row.running
        && row.archived));
    fs::write(workspace.join("active-sessions.json"), b"null").unwrap();
    assert!(server.engine.worktree_inventory().await.is_err());
    fs::write(workspace.join("attachment-inventory-unavailable"), []).unwrap();
    assert!(server.engine.worktree_inventory().await.is_err());
    server.stop().await;
}

async fn cancelled_session_writers() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let work = home.join("workspace");
    fs::create_dir_all(&work).unwrap();
    let record = work.join("session-writers.json");
    fs::write(
        &record,
        br#"{"id":"ses_writers","title":"Before","metadata":{"kept":true}}"#,
    )
    .unwrap();
    let started = work.join("session-writer-started");
    let opened = work.join("session-writer-opened");
    let server = Server::start(home).await;
    assert_eq!(server.get("api/echo").await.status(), 207);
    let edit = |action, body: Value, cancel| {
        let request = DispatchRequest {
            method: Method::POST,
            path: format!("kuvo/sessions/ses_writers/{action}"),
            headers: Default::default(),
            body: serde_json::to_vec(&body).unwrap().into(),
            caller: Caller::Local(Transport::Http {
                origin: kuvo_server::LocalOrigin::App,
                internal: false,
            }),
        };
        let dispatcher = server.dispatcher.clone();
        tokio::spawn(async move { dispatcher.dispatch(request, cancel).await.status })
    };
    let abandon = async |cancel: CancellationToken, write: JoinHandle<StatusCode>| {
        cancel.cancel();
        assert_eq!(
            time::timeout(Duration::from_secs(2), write)
                .await
                .unwrap()
                .unwrap(),
            StatusCode::BAD_GATEWAY,
        );
    };

    let first = edit("rename", json!({"title":"After"}), CancellationToken::new());
    until(
        || started.exists(),
        "the first note writer to start its read",
    )
    .await;
    let cancel = CancellationToken::new();
    let waiting = edit("archive", json!({"archived":true}), cancel.clone());
    tokio::task::yield_now().await;
    abandon(cancel, waiting).await;
    fs::write(&opened, []).unwrap();
    assert_eq!(first.await.unwrap(), StatusCode::OK);
    assert_eq!(
        edit("pin", json!({"pinned":true}), CancellationToken::new())
            .await
            .unwrap(),
        StatusCode::OK
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&record).unwrap()).unwrap(),
        json!({
            "id":"ses_writers","title":"After","metadata":{"kept":true,"kuvo.pinned":true},
        })
    );

    fs::remove_file(&started).unwrap();
    fs::remove_file(&opened).unwrap();
    let cancel = CancellationToken::new();
    let reading = edit(
        "rename",
        json!({"title":"Do not write this"}),
        cancel.clone(),
    );
    until(
        || started.exists(),
        "the next note writer to start a held read",
    )
    .await;
    abandon(cancel, reading).await;
    let next = edit("pin", json!({"pinned":false}), CancellationToken::new());
    assert_eq!(
        time::timeout(Duration::from_secs(2), next)
            .await
            .unwrap()
            .unwrap(),
        StatusCode::OK
    );
    fs::write(&opened, []).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&record).unwrap()).unwrap(),
        json!({
            "id":"ses_writers","title":"After","metadata":{"kept":true,"kuvo.pinned":false},
        })
    );
    use kuvo_engine::{NoteEdits, SessionPermission};
    let mut notes = accepted(server.engine.note_writer("ses_writers").await);
    assert!(!accepted(notes.edit(NoteEdits::default()).await));
    assert!(accepted(
        notes
            .edit(NoteEdits {
                kuvo: [
                    ("chat".to_owned(), Some(json!("/chats"))),
                    ("autopilot".to_owned(), Some(json!(true)))
                ]
                .into(),
                ..Default::default()
            })
            .await
    ));
    let mut next = edit("pin", json!({"pinned":true}), CancellationToken::new());
    assert!(
        time::timeout(Duration::from_millis(100), &mut next)
            .await
            .is_err()
    );
    assert!(accepted(
        notes
            .edit(NoteEdits {
                kuvo: [
                    ("autopilot".to_owned(), None),
                    ("approval".to_owned(), Some(json!("acceptEdits")))
                ]
                .into(),
                top: [("kuvo.pinned".to_owned(), None)].into(),
                permissions: Some(vec![SessionPermission::new("edit", "*", "allow")]),
            })
            .await
    ));
    drop(notes);
    assert_eq!(next.await.unwrap(), StatusCode::OK);
    let saved: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(
        saved["metadata"],
        json!({
            "kept": true, "kuvo.pinned": true, "kuvo": { "chat": "/chats", "approval": "acceptEdits" },
        })
    );
    assert_eq!(
        saved["permissions"],
        json!([{ "action":"edit", "resource":"*", "effect":"allow" }])
    );
    server.stop().await;
}

async fn transcript_reads_older_pages() {
    use kuvo_engine::{TranscriptError, TranscriptScope};

    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let work = home.join("workspace");
    fs::create_dir_all(&work).unwrap();
    let retain = |file, value: Value| {
        fs::write(work.join(file), serde_json::to_vec(&value).unwrap()).unwrap()
    };
    let session = "ses_transcript";
    retain("transcript-session.json", json!({"id":session}));
    let mut rows: Vec<_> = (1..=42).map(|at| json!({
        "id":format!("msg_{at}"), "type":"user", "text":format!("Paragraph {at} {}",
        if at % 14 == 0 { format!("mentions Café. {}", "words ".repeat(440)) } else { "words ".repeat(445) }),
    })).collect();
    let recent = rows.split_off(22);
    retain(
        "transcript-latest.json",
        json!({"data":recent.into_iter().rev().collect::<Vec<_>>(), "cursor":{"previous":"older page"}}),
    );
    retain(
        "transcript-older.json",
        json!({"data":rows.into_iter().rev().collect::<Vec<_>>()}),
    );
    let server = Server::start(home).await;
    assert_eq!(server.get("api/echo").await.status(), 207);
    let cancel = CancellationToken::new();
    let read = |query| {
        server
            .engine
            .transcript(session, "Release", query, TranscriptScope::Peer, &cancel)
    };

    let whole = read(None).await.unwrap();
    assert!(whole.starts_with("# Release\n\n**User**\n\nParagraph 1 "));
    assert!(whole.contains("[… the middle is left out; read it with query, to find a part …]"));
    assert!(whole.contains("Paragraph 42 mentions Café."));
    assert!(whole.len() < 20_200);
    let found = read(Some("CAFE")).await.unwrap();
    assert!(found.starts_with("3 places mention \"CAFE\""));
    assert!(found.contains("Paragraph 41"));
    assert!(found.contains("Paragraph 42 mentions Café."));
    assert!(!found.contains("Paragraph 14 mentions Café."));
    assert!(
        read(Some("missing"))
            .await
            .unwrap()
            .starts_with("Nothing in it mentions")
    );

    retain(
        "transcript-session.json",
        json!({"id":session, "revert":{"messageID":"msg_41"}}),
    );
    let reverted = read(None).await.unwrap();
    assert!(reverted.contains("Paragraph 40"));
    assert!(!reverted.contains("Paragraph 41"));
    assert!(!reverted.contains("Paragraph 42"));
    fs::write(work.join("transcript-unavailable"), []).unwrap();
    assert!(
        read(None)
            .await
            .unwrap()
            .starts_with("Some older messages couldn't be read:")
    );
    retain(
        "transcript-session.json",
        json!({"id":session, "revert":{"messageID":"msg_3"}}),
    );
    assert!(matches!(
        read(None).await,
        Err(TranscriptError::Unavailable(_))
    ));
    fs::remove_file(work.join("transcript-unavailable")).unwrap();
    retain("transcript-session.json", json!({"id":session}));
    for reason in ["auto", "manual"] {
        retain(
            "transcript-latest.json",
            json!({"data":[
                {"id":"msg_idle","type":"idle","outcome":"failed","error":{"message":"The engine stopped."}},
                {"id":"msg_summary","type":"compaction","reason":reason,"status":"failed"},
                {"id":"msg_first","type":"user","text":"Tidy the notes."},
            ]}),
        );
        let summary = read(None).await.unwrap();
        assert!(summary.contains("_Couldn't summarize the conversation._"));
        assert_eq!(
            summary.contains("It stopped with an error: The engine stopped."),
            reason == "auto"
        );
    }
    cancel.cancel();
    assert!(matches!(read(None).await, Err(TranscriptError::Cancelled)));
    server.stop().await;
}

async fn external_asks() {
    use kuvo_engine::{
        AskResult, ExternalAsk, ExternalAskKind, ExternalAsks, ExternalCard, ExternalDecision,
        ExternalReply, ExternalWaiting,
    };
    use kuvo_server::Refusal;
    use tokio::sync::watch;

    struct Owner(watch::Sender<Arc<Vec<ExternalWaiting>>>);
    impl ExternalAsks for Owner {
        fn list<'a>(
            &'a self,
            sessions: &'a [String],
            viewer: &'a DispatchRequest,
        ) -> AskResult<'a, Vec<ExternalAsk>> {
            Box::pin(async move {
                let choices = json!([{"answer":"once","title":"Allow Once","help":null,
                    "named":null,"placeholder":null,"spoken":null}]);
                let seen = self.0.borrow();
                Ok(seen.iter().filter(|ask| sessions.contains(&ask.session)).map(|ask| {
                    let card = match ask.kind {
                        ExternalAskKind::Sandbox => ExternalCard::Sandbox(serde_json::from_value(json!({
                            "kind":"sandbox","id":ask.id,"session":ask.session,"title":"Outside the sandbox",
                            "asker":"Runner","message":"A command needs your answer.","command":"printf test",
                            "error":null,"why":"A test command","stopped":[],"choices":choices,
                        })).unwrap()),
                        ExternalAskKind::Vault => ExternalCard::Vault(serde_json::from_value(json!({
                            "kind":"vault","id":ask.id,"session":ask.session,"title":"Use a vault item",
                            "need":"use","message":null,"note":null,"site":null,
                            "choices":if viewer.headers.contains_key("x-test-vault-proof") { choices.clone() } else { json!([]) },
                        })).unwrap()),
                    };
                    ExternalAsk { at: ask.at, card }
                }).collect())
            })
        }
        fn reply<'a>(&'a self, answer: ExternalReply<'a>) -> AskResult<'a, ()> {
            Box::pin(async move {
                assert!(matches!(answer.decision, ExternalDecision::Once));
                assert_eq!(answer.session, "ses_asks");
                if matches!(answer.kind, ExternalAskKind::Vault) {
                    assert!(answer.request.headers.contains_key("x-test-vault-proof"));
                }
                let changed = self.0.send_if_modified(|rows| {
                    let rows = Arc::make_mut(rows);
                    let before = rows.len();
                    rows.retain(|ask| ask.id != answer.id);
                    rows.len() != before
                });
                if changed {
                    Ok(())
                } else {
                    Err(Refusal::new(
                        StatusCode::NOT_FOUND,
                        "It isn’t waiting any more.",
                    ))
                }
            })
        }
        fn waiting(&self) -> watch::Receiver<Arc<Vec<ExternalWaiting>>> {
            self.0.subscribe()
        }
    }
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let seen = Arc::new(vec![
        ExternalWaiting {
            id: "sb_test".into(),
            session: "ses_asks".into(),
            at: 2,
            kind: ExternalAskKind::Sandbox,
        },
        ExternalWaiting {
            id: "vault_test".into(),
            session: "ses_asks".into(),
            at: 1,
            kind: ExternalAskKind::Vault,
        },
    ]);
    let (updates, _) = watch::channel(seen);
    let source = Arc::new(Owner(updates));
    let server = Server::start_with(home, |engine| engine.with_external_asks(source)).await;
    assert_eq!(server.get("api/echo").await.status(), 207);
    fs::write(
        home.join("workspace/attachments-list.json"),
        br#"[{"id":"ses_asks","title":"Test","location":{"directory":""}}]"#,
    )
    .unwrap();
    let route = "kuvo/sessions/ses_asks/asks";
    let cards = server.get(route).await.json::<Value>().await.unwrap();
    assert_eq!(cards["asks"][0]["id"], "sb_test");
    assert_eq!(cards["asks"][1]["choices"], json!([]));
    let answer = |id: &str| {
        server
            .request(reqwest::Method::POST, &format!("{route}/{id}"))
            .header("x-kuvo-from", "app")
            .json(&json!({"answer":"once"}))
    };
    assert_eq!(answer("vault_test").send().await.unwrap().status(), 403);
    assert_eq!(answer("sb_test").send().await.unwrap().status(), 200);
    assert_eq!(
        answer("vault_test")
            .header("x-test-vault-proof", "yes")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        server.get(route).await.json::<Value>().await.unwrap(),
        json!({"asks":[]})
    );
    server.stop().await;
}

async fn coding_agent_models() {
    struct CodingAgent;
    impl AgentModelSource for CodingAgent {
        fn catalog(&self, refresh: bool) -> CatalogRead<'_, AgentCatalog> {
            Box::pin(async move {
                let account =
                    |provider: &str, id: &str, name: &str, label: Option<&str>| ModelAccount {
                        provider: provider.into(),
                        kind: "codex".into(),
                        id: id.into(),
                        name: name.into(),
                        label: label.map(str::to_owned),
                        signed_in_as: None,
                        plan: Some("Pro".into()),
                        installed: true,
                        signed_in: true,
                        complete: id != "default",
                    };
                Ok(AgentCatalog {
                    accounts: vec![
                        account("codex-cli", "default", "Codex", None),
                        account("codex-cli@work", "work", "Codex · Work", Some("Work")),
                    ],
                    models: vec![AgentModel {
                        provider: "codex-cli@work".into(),
                        id: "listed".into(),
                        name: "Listed".into(),
                        aliases: vec!["default".into()],
                        efforts: vec!["high".into()],
                        hidden: false,
                        retiring: false,
                        provisional: false,
                        speeds: vec![ModelSpeed {
                            id: "priority".into(),
                            name: "Fast".into(),
                            description: None,
                        }],
                    }],
                    auto: Some(vec![AutoModel {
                        id: "picked".into(),
                        name: "Picked".into(),
                        entry_count: 1,
                    }]),
                    generations: [(
                        "codex-cli@work".into(),
                        if refresh { "fresh" } else { "kept" }.into(),
                    )]
                    .into(),
                    unavailable: vec!["codex-cli".into()],
                })
            })
        }
        fn session_folder<'a>(&'a self, session: &'a str) -> CatalogRead<'a, Option<String>> {
            Box::pin(async move { Ok((session == "ses_agents").then(String::new)) })
        }
    }
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    fs::create_dir_all(home.join("box")).unwrap();
    fs::write(
        home.join("box/models.json"),
        br#"{"speeds":{"codex-cli@work":"priority"}}"#,
    )
    .unwrap();
    let server = Server::start_with(home, |engine| {
        engine.with_agent_model_source(Arc::new(CodingAgent))
    })
    .await;
    let reply = server.get("kuvo/models/offered?session=ses_agents&refresh=1&provider=codex-cli%40work&model=default&variant=high").await;
    assert_eq!(reply.status(), 200);
    assert_eq!(
        reply.json::<Value>().await.unwrap(),
        json!({
            "models":[
                {"providerID":"codex-cli@work","id":"listed","name":"Listed","side":"agents","aliases":["default"],"efforts":["high"],"speeds":[{"id":"priority","name":"Fast"}]},
                {"providerID":"auto","id":"picked","name":"Picked","side":"agents","aliases":[],"efforts":[]}
            ],
            "providers":[
                {"id":"codex-cli@work","name":"Codex · Work","label":"Work","speed":"priority","account":{"agent":"codex","id":"work","agentName":"Codex","name":"Work","plan":"Pro"}},
                {"id":"auto","name":"Custom Auto"}
            ],
            "known":{"engine":false,"agents":true},"starred":[],
            "generations":{"codex-cli@work":"fresh"},"listing":["codex-cli"],"unavailable":[{"id":"codex-cli"}],
            "starting":{"providerID":"codex-cli@work","id":"listed","variant":"high"},"scoped":true
        })
    );
    server.stop().await;
}

async fn peer_waiting() {
    use kuvo_engine::PeerWaiting;
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("bots/bot_scout");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("bot.json"),
        br#"{"kuvo":1,"id":"bot_scout","name":"Scout"}"#,
    )
    .unwrap();
    fs::write(folder.join("session"), b"ses_root").unwrap();
    let seen = Arc::new(vec![
        PeerWaiting {
            session: "ses_delegated".into(),
            line: vec!["ses_root".into()],
            bot: None,
        },
        PeerWaiting {
            session: "ses_child".into(),
            line: Vec::new(),
            bot: Some("bot_scout".into()),
        },
    ]);
    let (updates, activity) = tokio::sync::watch::channel(seen);
    let server = Server::start_with(home.path(), |engine| engine.with_peer_waiting(activity)).await;
    let waiting = || async {
        server
            .get("kuvo/waiting")
            .await
            .json::<Value>()
            .await
            .unwrap()
    };
    assert_eq!(waiting().await, json!({"sessions":0,"bots":1,"queue":0}));
    updates.send_replace(Arc::new(Vec::new()));
    assert_eq!(waiting().await, json!({"sessions":0,"bots":0,"queue":0}));
    server.stop().await;
}

async fn fixture() {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("launches")
        .unwrap();
    writeln!(file, "1").unwrap();
    let password = std::env::var("OPENCODE_PASSWORD").unwrap();
    assert_ne!(password, "");
    let authorization: Arc<str> = auth(&password).into();
    let port = std::env::args().nth(3).unwrap().parse::<u16>().unwrap();
    let server = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    loop {
        let (stream, _) = server.accept().await.unwrap();
        let auth = authorization.clone();
        tokio::spawn(async move {
            let _ = http1::Builder::new()
                .serve_connection(
                    TokioIo::new(stream),
                    service_fn(move |req| {
                        let auth = auth.clone();
                        async move { Ok::<_, Infallible>(fixture_reply(req, &auth).await) }
                    }),
                )
                .with_upgrades()
                .await;
        });
    }
}

async fn paired_delivery() {
    use kuvo_engine::{
        ComputerKey, DeviceAccess, OwnedRequest, OwnerScope, PeerReply, SessionOwnership,
    };
    use kuvo_server::Refusal;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Authority {
        held: AtomicBool,
        replaced: AtomicBool,
    }

    impl SessionOwnership for Authority {
        fn scope(&self, _: &str, issuer: Option<ComputerKey>) -> Result<OwnerScope, Refusal> {
            Ok(OwnerScope {
                setup: Some("set_AAAAAAAAAAAAAAAAAAAAAA".into()),
                here: Some([3; 32]),
                owner: None,
                receipt_issuer: issuer,
                held: self.held.load(Ordering::Relaxed),
            })
        }

        fn current_pairing(&self, key: &ComputerKey, pairing: &str) -> DeviceAccess {
            let expected = if self.replaced.load(Ordering::Relaxed) {
                "new"
            } else {
                "old"
            };
            if *key == [17; 32] && pairing == expected {
                DeviceAccess::Allowed
            } else {
                DeviceAccess::Revoked
            }
        }

        fn request(&self, _: ComputerKey, _: OwnedRequest, _: Duration) -> PeerReply<'_> {
            panic!("a draft addressed here must use the local handoff")
        }
    }

    let home = tempfile::tempdir().unwrap();
    let authority = Arc::new(Authority {
        held: true.into(),
        replaced: false.into(),
    });
    let server = Server::start_with(home.path(), |engine| {
        engine.with_session_ownership(authority.clone())
    })
    .await;
    let call = |method: Method, route: String, device, pairing: &str, body: Value| {
        let id = pairing.to_owned();
        let authority = authority.clone();
        DispatchRequest {
            method,
            path: route,
            headers: Default::default(),
            body: serde_json::to_vec(&body).unwrap().into(),
            caller: Caller::Paired {
                transport: Transport::Relay,
                pairing: Pairing::verified(
                    device,
                    id.clone(),
                    "paired author".into(),
                    true,
                    move || authority.current_pairing(&device, &id) == DeviceAccess::Allowed,
                ),
            },
        }
    };
    let cancel = CancellationToken::new();
    let saved = server
        .dispatcher
        .dispatch(
            call(
                Method::POST,
                "kuvo/sessions/ses_paired/delivery".into(),
                [17; 32],
                "old",
                json!({"id":"msg_paired","text":"Before pairing changed","message":{}}),
            ),
            cancel.clone(),
        )
        .await;
    assert_eq!(saved.status, 202);
    let saved: Value =
        serde_json::from_slice(&saved.body.collect().await.unwrap().to_bytes()).unwrap();
    let receipt = format!(
        "kuvo/data/messages/{}",
        saved["operation"].as_str().unwrap()
    );
    let read = |pairing| call(Method::GET, receipt.clone(), [17; 32], pairing, Value::Null);
    authority.replaced.store(true, Ordering::Relaxed);
    authority.held.store(false, Ordering::Relaxed);
    assert_eq!(
        server
            .dispatcher
            .dispatch(read("old"), cancel.clone())
            .await
            .status,
        403
    );
    let settled = time::timeout(Duration::from_secs(15), async {
        loop {
            let result = server
                .dispatcher
                .dispatch(read("new"), cancel.clone())
                .await;
            assert_eq!(result.status, 200);
            let receipt: Value =
                serde_json::from_slice(&result.body.collect().await.unwrap().to_bytes()).unwrap();
            if receipt["status"] == "interrupted" {
                break receipt;
            }
            time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        settled["message"]
            .as_str()
            .unwrap()
            .contains("no longer has access")
    );
    assert!(!home.path().join("run/engine.json").exists());
    let installed = install(home.path());
    let _cleanup = Cleanup {
        home: home.path().to_owned(),
        installed,
    };
    let work = home.path().join("workspace");
    fs::create_dir_all(&work).unwrap();
    assert_eq!(server.get("api/echo").await.status(), 207);
    let permission = "api/session/ses_paired/permission/per_live/reply";
    let permission_for = |pairing| {
        call(
            Method::POST,
            permission.into(),
            [17; 32],
            pairing,
            json!({"decision":"once"}),
        )
    };
    assert_eq!(
        server
            .dispatcher
            .dispatch(permission_for("old"), cancel.clone())
            .await
            .status,
        403
    );
    let started = work.join("consent-reading");
    let proceed = work.join("consent-continue");
    for (status, revoke_pairing) in [(403, true), (409, false)] {
        let dispatch = server.dispatcher.clone();
        let request = permission_for("new");
        let waiting = tokio::spawn(async move {
            dispatch
                .dispatch(request, CancellationToken::new())
                .await
                .status
        });
        until(|| started.exists(), "the live permission read").await;
        if revoke_pairing {
            authority.replaced.store(false, Ordering::Relaxed);
        } else {
            authority.held.store(true, Ordering::Relaxed);
        }
        fs::write(&proceed, []).unwrap();
        assert_eq!(waiting.await.unwrap(), status);
        assert!(!work.join("consent-replied").exists());
        fs::remove_file(&started).unwrap();
        fs::remove_file(&proceed).unwrap();
        authority.replaced.store(true, Ordering::Relaxed);
        authority.held.store(false, Ordering::Relaxed);
    }
    fs::write(&proceed, []).unwrap();
    assert_eq!(
        server
            .dispatcher
            .dispatch(permission_for("new"), cancel)
            .await
            .status,
        200
    );
    assert!(work.join("consent-replied").exists());

    server.stop().await;
    println!(
        "revoked delivery and permissions were fenced, including during a live permission read"
    );
}

// The old inbox is only on the fixture engine; the saved prefix uses the resident's real files.
async fn interrupted_inbox_transfer() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let work = home.join("workspace");
    fs::create_dir_all(&work).unwrap();
    let entries = ["A", "B", "C"].map(|word| {
        json!({ "id": format!("msg_{word}"), "type": "user", "delivery": "queue", "payload": { "text": word } })
    });
    fs::write(work.join("inbox-mode"), []).unwrap();
    fs::write(
        work.join("inbox.json"),
        serde_json::to_vec(&entries).unwrap(),
    )
    .unwrap();
    let server = Server::start(home).await;
    assert_eq!(server.get("api/session?limit=1").await.status(), 200);

    assert!(
        server.engine.keep_inboxes().await.is_err(),
        "incomplete inventory must defer the switch"
    );
    assert!(!home.join("run/queues.json").exists());
    fs::write(work.join("complete-inventory"), []).unwrap();
    fs::write(work.join("refuse-B"), []).unwrap();
    assert!(server.engine.keep_inboxes().await.is_err());

    let session = "kuvo/sessions/ses_inbox";
    let queue = server
        .get(&format!("{session}/queue"))
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(queue["waiting"][0]["id"], "msg_A");
    assert_eq!(queue["waiting"][1]["id"], "msg_B");
    let sent = server
        .request(reqwest::Method::POST, &format!("{session}/send"))
        .json(&json!({ "id":"msg_D", "text":"D" }))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 200);
    assert_eq!(
        sent.json::<Value>().await.unwrap(),
        json!({ "waits": true })
    );
    assert!(!work.join("delivered").exists());

    server.stop().await;
    let server = Server::start(home).await;
    assert_eq!(server.get("api/session?limit=1").await.status(), 200);
    fs::remove_file(work.join("refuse-B")).unwrap();
    assert_eq!(server.engine.keep_inboxes().await.unwrap(), 2);
    let waiting = home.join("run/waiting/msg_A.json");
    fs::write(work.join("acceptance-unknown"), []).unwrap();
    assert_eq!(server.engine.release_waiting().await.unwrap(), 0);
    assert!(waiting.is_file());
    assert!(!work.join("delivered").exists());
    fs::remove_file(work.join("acceptance-unknown")).unwrap();
    assert_eq!(server.engine.release_waiting().await.unwrap(), 4);
    assert!(!waiting.exists());
    assert_eq!(server.engine.release_waiting().await.unwrap(), 0);
    assert_eq!(
        fs::read_to_string(work.join("delivered")).unwrap(),
        "A\nB\nC\nD\n"
    );
    let queue = server
        .get(&format!("{session}/queue"))
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(queue, json!({ "paused": false, "held": [] }));
    server.stop().await;
}

async fn starter_stop_leaves_summary_out_of_queue() {
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let work = home.join("workspace");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("inbox-mode"), []).unwrap();
    fs::write(work.join("event-watch"), []).unwrap();
    let inbox = work.join("inbox.json");
    fs::write(
        &inbox,
        br#"[{"id":"msg_summary","type":"compaction","delivery":"steer"}]"#,
    )
    .unwrap();
    let server = Server::start(home).await;
    assert_eq!(server.get("api/session/ses_inbox").await.status(), 200);
    until(
        || work.join("event-subscribed").exists(),
        "the resident to follow execution events",
    )
    .await;
    accepted(server.engine.stop_for_starter("ses_inbox").await);
    assert!(work.join("interrupted").is_file());
    let stopped = server.get("kuvo/sessions/ses_inbox/queue").await;
    assert_eq!(
        stopped.json::<Value>().await.unwrap(),
        json!({ "paused": false, "held": [] })
    );
    let event = json!({"type":"session.execution.interrupted","data":{"sessionID":"ses_inbox","reason":"user"}});
    fs::write(work.join("fixture-event"), format!("data: {event}\n\n")).unwrap();
    until(
        || fs::read_to_string(&inbox).is_ok_and(|rows| rows == "[]"),
        "the stopped summary to be removed",
    )
    .await;
    let stopped = server.get("kuvo/sessions/ses_inbox/queue").await;
    assert_eq!(
        stopped.json::<Value>().await.unwrap(),
        json!({ "paused": false, "held": [] })
    );
    server.stop().await;
}

async fn fixture_reply(mut req: Request<Incoming>, auth: &str) -> Response<kuvo_server::Body> {
    let mut answer = Reply::json(StatusCode::OK, json!({"data": []}));
    if !req
        .headers()
        .get(header::AUTHORIZATION)
        .is_some_and(|given| given == auth)
    {
        answer = Reply::json(
            StatusCode::UNAUTHORIZED,
            json!({"message": "engine password required"}),
        );
    } else {
        let route = req.uri().path_and_query().unwrap().as_str().to_owned();
        let text = |header| {
            req.headers()
                .get(header)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        if Path::new("inbox-mode").exists()
            && !(req.uri().path() == "/api/event" && Path::new("event-watch").exists())
        {
            let reply = inbox_fixture(req).await;
            let mut response = Response::new(reply.body);
            *response.status_mut() = reply.status;
            *response.headers_mut() = reply.headers;
            return response;
        }
        match req.uri().path() {
            "/api/session/ses_paired/permission" if route.ends_with("?live=1") => {
                fs::write("consent-reading", []).unwrap();
                until(
                    || Path::new("consent-continue").exists(),
                    "the test to finish the live permission read",
                )
                .await;
                answer = Reply::json(
                    StatusCode::OK,
                    json!({"data":[{"id":"per_live","action":"edit","resources":["notes.txt"]}]}),
                );
            }
            "/api/session/ses_paired/permission/per_live/reply" if req.method() == Method::POST => {
                fs::write("consent-replied", []).unwrap();
                answer = Reply::json(StatusCode::OK, json!({}));
            }
            "/api/session/ses_writers" => {
                let file = "session-writers.json";
                let mut saved: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
                if req.method() == Method::PATCH {
                    let patch: serde_json::Map<String, Value> = serde_json::from_slice(
                        &req.into_body().collect().await.unwrap().to_bytes(),
                    )
                    .unwrap();
                    saved.as_object_mut().unwrap().extend(patch);
                    fs::write(file, serde_json::to_vec(&saved).unwrap()).unwrap();
                } else if !Path::new("session-writer-started").exists() {
                    fs::write("session-writer-started", []).unwrap();
                    until(
                        || Path::new("session-writer-opened").exists(),
                        "the test to release the held note read",
                    )
                    .await;
                }
                answer = Reply::json(StatusCode::OK, json!({ "data": saved }));
            }
            "/api/session/ses_transcript" => {
                let row: Value =
                    serde_json::from_slice(&fs::read("transcript-session.json").unwrap()).unwrap();
                answer = Reply::json(StatusCode::OK, json!({"data":row}));
            }
            "/api/session/ses_transcript/message" => {
                let older = req
                    .uri()
                    .query()
                    .is_some_and(|query| query.contains("cursor=older%20page"));
                if older && Path::new("transcript-unavailable").exists() {
                    answer = Reply::json(
                        StatusCode::SERVICE_UNAVAILABLE,
                        json!({"message":"The oldest messages aren't ready."}),
                    );
                } else {
                    let file = if older {
                        "transcript-older.json"
                    } else {
                        "transcript-latest.json"
                    };
                    let row: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
                    answer = Reply::json(StatusCode::OK, row);
                }
            }
            "/api/session/active" if Path::new("active-sessions.json").exists() => {
                let rows: Value =
                    serde_json::from_slice(&fs::read("active-sessions.json").unwrap()).unwrap();
                answer = Reply::json(StatusCode::OK, json!({"data":rows}));
            }
            "/api/session" if Path::new("attachment-inventory-unavailable").exists() => {
                answer = Reply::json(StatusCode::SERVICE_UNAVAILABLE, json!({}));
            }
            "/api/session" if Path::new("attachments-list.json").exists() => {
                let mut rows: Vec<Value> =
                    serde_json::from_slice(&fs::read("attachments-list.json").unwrap()).unwrap();
                if let Some(parent) = req.uri().query().and_then(|query| {
                    query
                        .split('&')
                        .find_map(|field| field.strip_prefix("parentID="))
                }) {
                    rows.retain(|row| row["parentID"] == parent);
                }
                answer = Reply::json(StatusCode::OK, json!({"data":rows}));
            }
            "/api/session/ses_branch/message" | "/api/session/ses_middle/message"
                if Path::new("attachment-words.json").exists() =>
            {
                let rows: Value =
                    serde_json::from_slice(&fs::read("attachment-words.json").unwrap()).unwrap();
                answer = Reply::json(StatusCode::OK, json!({"data":rows}));
            }
            "/api/session/ses_asks" if Path::new("attachments-list.json").exists() => {
                answer = Reply::json(StatusCode::OK, json!({"data":{"id":"ses_asks"}}));
            }
            "/api/echo" => {
                let mut value = json!({
                    "path": req.uri().path_and_query().unwrap().as_str(),
                    "directory": text("x-opencode-directory"), "last": text("last-event-id"),
                    "accept": text("accept"), "type": text("content-type"), "other": text("x-ignored"),
                });
                value["body"] = json!(req.into_body().collect().await.unwrap().to_bytes().to_vec());
                answer = Reply::json(StatusCode::MULTI_STATUS, value);
                answer
                    .headers
                    .insert("cache-control", "no-cache".parse().unwrap());
                answer
                    .headers
                    .insert("x-private", "discard".parse().unwrap());
            }
            "/api/event" => {
                answer
                    .headers
                    .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
                let stream = async_stream::stream! {
                    let _ended = StreamEnded;
                    yield Ok(Frame::data(Bytes::from_static(b"data: connected\n\n")));
                    if Path::new("event-watch").exists() {
                        fs::write("event-subscribed", []).unwrap();
                        until(|| Path::new("fixture-event").exists(), "the test to publish an engine event").await;
                        let event = fs::read("fixture-event").unwrap();
                        yield Ok(Frame::data(Bytes::from(event)));
                    }
                    std::future::pending::<()>().await;
                };
                answer.body = StreamBody::new(stream).boxed_unsync();
            }
            "/api/debug/location" => {
                let directory = std::env::current_dir().unwrap();
                answer = Reply::json(StatusCode::OK, json!([{"directory": directory}]));
            }
            "/api/form" if Path::new("busy").exists() => {
                fs::write("busy-checked", []).unwrap();
                answer = Reply::json(StatusCode::OK, json!({"data": [{"id": "question"}]}));
            }
            "/api/pty/term/connect" => {
                let directory = text("x-opencode-directory").unwrap_or_default();
                fs::write(
                    "socket",
                    format!("{} {directory}", req.uri().path_and_query().unwrap()),
                )
                .unwrap();
                let accept = derive_accept_key(req.headers()[header::SEC_WEBSOCKET_KEY].as_bytes());
                let upgrade = hyper::upgrade::on(&mut req);
                tokio::spawn(async {
                    let mut socket = WebSocketStream::from_raw_socket(
                        TokioIo::new(upgrade.await.unwrap()),
                        Role::Server,
                        None,
                    )
                    .await;
                    while let Some(Ok(message)) = socket.next().await {
                        if let Message::Close(close) = message {
                            if let Some(close) = close {
                                use std::io::Write;
                                let mut record = tempfile::NamedTempFile::new_in(".").unwrap();
                                write!(record, "{} {}", u16::from(close.code), close.reason)
                                    .unwrap();
                                record.persist("close").unwrap();
                            }
                            let _ = socket.flush().await;
                            break;
                        }
                        socket.send(message).await.unwrap();
                    }
                });
                answer.status = StatusCode::SWITCHING_PROTOCOLS;
                answer
                    .headers
                    .insert(header::UPGRADE, "websocket".parse().unwrap());
                answer
                    .headers
                    .insert(header::CONNECTION, "Upgrade".parse().unwrap());
                answer
                    .headers
                    .insert(header::SEC_WEBSOCKET_ACCEPT, accept.parse().unwrap());
            }
            "/api/pty/refused/connect" => answer.status = StatusCode::NOT_FOUND,
            _ => {}
        }
        answer
            .headers
            .entry(header::CACHE_CONTROL)
            .or_insert_with(|| route.parse().unwrap());
    }
    let mut response = Response::new(answer.body);
    *response.status_mut() = answer.status;
    *response.headers_mut() = answer.headers;
    response
}

struct StreamEnded;

impl Drop for StreamEnded {
    fn drop(&mut self) {
        fs::write("stream-ended", []).unwrap();
    }
}

async fn inbox_fixture(req: Request<Incoming>) -> Reply {
    let route = req.uri().path().to_owned();
    let tail = |base| route.strip_prefix(base).map(str::to_owned);
    let session =
        json!({ "id": "ses_inbox", "title": "Inbox", "time": { "created": 1, "updated": 1 } });
    let empty = || Reply::json(StatusCode::OK, json!({ "data": [] }));
    let missing = || Reply::json(StatusCode::NOT_FOUND, json!({}));
    let mut inbox: Vec<Value> = serde_json::from_slice(&fs::read("inbox.json").unwrap()).unwrap();
    match (req.method().as_str(), route.as_str()) {
        ("GET", "/api/session") => {
            let query = req.uri().query().unwrap_or_default();
            if query.contains("archived=true") {
                return empty();
            }
            if query.contains("cursor=") {
                return if Path::new("complete-inventory").exists() {
                    Reply::json(StatusCode::OK, json!({ "data": [session] }))
                } else {
                    Reply::json(StatusCode::OK, json!({}))
                };
            }
            if query.contains("limit=100") {
                let rows = (0..100)
                    .map(|index| json!({ "id": format!("ses_page{index}") }))
                    .collect::<Vec<_>>();
                return Reply::json(
                    StatusCode::OK,
                    json!({ "data": rows, "cursor": { "next": "page 2" } }),
                );
            }
            empty()
        }
        ("GET", "/api/session/ses_inbox") => {
            Reply::json(StatusCode::OK, json!({ "data": session }))
        }
        ("GET", "/api/session/ses_inbox/inbox") => {
            Reply::json(StatusCode::OK, json!({ "data": inbox }))
        }
        ("GET", "/api/session/active") => empty(),
        ("POST", "/api/session/ses_inbox/interrupt") => {
            fs::write("interrupted", []).unwrap();
            Reply::json(StatusCode::OK, json!({}))
        }
        ("GET", _) if route.ends_with("/inbox") => empty(),
        ("GET", _) if route.starts_with("/api/session/ses_inbox/message/") => {
            let id = tail("/api/session/ses_inbox/message/").unwrap();
            let found = fs::read_to_string("delivered-ids")
                .unwrap_or_default()
                .lines()
                .any(|saved| saved == id);
            if found {
                Reply::json(StatusCode::OK, json!({ "data": { "id": id } }))
            } else {
                missing()
            }
        }
        ("DELETE", _) if route.starts_with("/api/session/ses_inbox/inbox/") => {
            let id = tail("/api/session/ses_inbox/inbox/").unwrap();
            if id == "msg_B" && Path::new("refuse-B").exists() {
                return Reply::json(StatusCode::SERVICE_UNAVAILABLE, json!({}));
            }
            let before = inbox.len();
            inbox.retain(|row| row["id"] != id);
            fs::write("inbox.json", serde_json::to_vec(&inbox).unwrap()).unwrap();
            if before == inbox.len() {
                missing()
            } else {
                Reply::json(StatusCode::OK, json!({}))
            }
        }
        ("POST", "/api/session/ses_inbox/prompt") => {
            use std::io::Write;
            let given: Value =
                serde_json::from_slice(&req.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            if Path::new("acceptance-unknown").exists() {
                return Reply::json(StatusCode::OK, json!({}));
            }
            let mut delivered = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("delivered")
                .unwrap();
            writeln!(delivered, "{}", given["text"].as_str().unwrap()).unwrap();
            let mut ids = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("delivered-ids")
                .unwrap();
            writeln!(ids, "{}", given["id"].as_str().unwrap()).unwrap();
            Reply::json(StatusCode::OK, json!({}))
        }
        _ => missing(),
    }
}

async fn resident_answers_promptly_under_mutation_lock_contention() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let installed = install(home);
    let _cleanup = Cleanup {
        home: home.to_owned(),
        installed,
    };
    let lock = home.join("run/owned-data/.mutations.lock");
    fs::create_dir_all(&lock).unwrap();
    let server = Server::start(home).await;
    let started = std::time::Instant::now();
    let reply = server.get("kuvo/nonexistent").await;
    let elapsed = started.elapsed();
    assert_eq!(reply.status(), StatusCode::NOT_FOUND);
    assert!(
        elapsed < Duration::from_millis(250),
        "Resident request waited {elapsed:?} while the mutation lock was held"
    );
    let _ = fs::remove_dir_all(&lock);
    server.stop().await;
}
