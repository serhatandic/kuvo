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
use http::{Request, Response, StatusCode, header};
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use kuvo_engine::{Engine, Timing, pinned_version};
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
    if std::env::args().nth(1).as_deref() == Some("serve") {
        executor.block_on(fixture());
    } else {
        executor.block_on(scenarios());
    }
}

struct Server {
    stop: CancellationToken,
    task: JoinHandle<io::Result<()>>,
    dispatcher: kuvo_server::Dispatcher,
    url: String,
    password: String,
    client: reqwest::Client,
}

impl Server {
    async fn start(home: &Path) -> Self {
        let resident = Resident::bind(home).await.unwrap().unwrap();
        let mut routes = Registry::new();
        let mut env: kuvo_os::platform::Environment = std::env::vars_os()
            .filter(|(key, _)| !key.to_string_lossy().starts_with("KUVO_"))
            .collect();
        env.insert("HOME".into(), home.into());
        env.insert("USERPROFILE".into(), home.into());
        let engine = Engine::new(
            home.to_owned(),
            env,
            Timing {
                idle: IDLE,
                check: CHECK,
            },
        )
        .unwrap();
        engine.register(&mut routes);
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
        drop(answer);
        assert_eq!(
            fs::read_to_string(work.join("last-path")).unwrap(),
            format!("/{forwarded}"),
            "{vector}"
        );
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
        fs::write("last-path", req.uri().path_and_query().unwrap().as_str()).unwrap();
        let text = |header| {
            req.headers()
                .get(header)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        match req.uri().path() {
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
                        if let Message::Close(Some(close)) = message {
                            fs::write(
                                "close",
                                format!("{} {}", u16::from(close.code), close.reason),
                            )
                            .unwrap();
                            socket.flush().await.unwrap();
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
