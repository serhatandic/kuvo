//! The real HTTP server and its discovery files. Only the engine's answers are supplied by fixtures.

use std::{io, path::Path, sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use http::StatusCode;
use hyper::body::Bytes;
use kuvo_server::{
    Dispatcher, LineTiming, Registry, Reply, Resident, ServeRecord, ssh::NoPairings,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time,
};

struct Running {
    record: ServeRecord,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}

impl Running {
    async fn start(home: &Path, dispatcher: Dispatcher) -> Self {
        let server = Resident::bind(home).await.unwrap().unwrap();
        let record =
            serde_json::from_slice(&std::fs::read(home.join("run/serve.json")).unwrap()).unwrap();
        let (shutdown, done) = oneshot::channel();
        let task = tokio::spawn(server.serve(dispatcher, Arc::new(NoPairings), async move {
            let _ = done.await;
        }));
        Self {
            record,
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        (&mut self.task).await.unwrap().unwrap();
    }

    async fn request(
        &self,
        path: &str,
        version: &str,
        headers: &str,
        auth: bool,
    ) -> BufReader<TcpStream> {
        let mut socket = TcpStream::connect(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            self.record.port,
        )))
        .await
        .unwrap();
        let password = if auth {
            format!(
                "Authorization: Basic {}\r\n",
                STANDARD.encode(format!("opencode:{}", self.record.password))
            )
        } else {
            String::new()
        };
        let request =
            format!("GET /{path} HTTP/{version}\r\nHost: 127.0.0.1\r\n{password}{headers}\r\n");
        socket.write_all(request.as_bytes()).await.unwrap();
        BufReader::new(socket)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn head(read: &mut BufReader<TcpStream>) -> String {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        assert!(read.read_line(&mut line).await.unwrap() > 0, "{head:?}");
        head.push_str(&line);
        if line == "\r\n" {
            return head;
        }
    }
}

async fn whole(read: &mut BufReader<TcpStream>) -> Value {
    let mut rest = Vec::new();
    read.read_to_end(&mut rest).await.unwrap();
    serde_json::from_slice(&rest).unwrap()
}

#[tokio::test]
async fn one_resident_owns_both_listeners_and_replaces_only_its_discovery_record() {
    let home = tempfile::tempdir().unwrap();
    let record = home.path().join("run/serve.json");
    let first = Running::start(home.path(), Registry::new().build(LineTiming::default())).await;
    assert_eq!(first.record.format_version, 1);
    assert_eq!(first.record.pid, std::process::id());
    assert!(!first.record.kept);
    assert!(first.record.started_at > 0);
    assert_eq!(first.record.password.len(), 48);
    assert_eq!(
        std::fs::read_to_string(home.path().join("run/link.pid")).unwrap(),
        first.record.pid.to_string()
    );
    assert!(home.path().join("run/socket.key").exists());
    assert!(home.path().join("run/route").is_dir());
    let generation = first.record.generation;
    let password = first.record.password.clone();
    let duplicate = Resident::bind(home.path()).await;
    assert_eq!(
        duplicate.err().unwrap().kind(),
        io::ErrorKind::AlreadyExists
    );
    first.stop().await;
    assert!(!record.exists());
    assert!(!home.path().join("run/link.pid").exists());
    assert!(!home.path().join("run/.client-owner").exists());

    let next = Running::start(home.path(), Registry::new().build(LineTiming::default())).await;
    assert_ne!(next.record.generation, generation);
    assert_ne!(next.record.password, password);
    next.stop().await;
    for marker in ["off", "paused"] {
        let marker = home.path().join("run").join(marker);
        std::fs::write(&marker, b"").unwrap();
        assert!(Resident::bind(home.path()).await.unwrap().is_none());
        assert!(!record.exists());
        std::fs::remove_file(marker).unwrap();
    }
}

#[tokio::test]
async fn loopback_refuses_without_the_password_or_an_understood_path_and_route_version() {
    let home = tempfile::tempdir().unwrap();
    let server = Running::start(home.path(), Registry::new().build(LineTiming::default())).await;
    for (path, auth, version, status) in [
        ("kuvo/line?once=1", false, "", "401"),
        ("kuvo/%2fline", true, "", "403"),
        (
            "kuvo/line?watch=bad",
            true,
            "x-kuvo-route-version: 1\r\n",
            "400",
        ),
        (
            "kuvo/line?once=1",
            true,
            "x-kuvo-route-version: 99\r\n",
            "400",
        ),
        ("kuvo/unknown", true, "", "404"),
    ] {
        let headers = format!("Connection: close\r\n{version}");
        let mut connection = server.request(path, "1.1", &headers, auth).await;
        let reply = head(&mut connection).await;
        assert!(
            reply.starts_with(&format!("HTTP/1.1 {status} ")),
            "{reply:?}"
        );
        if status == "400" {
            assert!(reply.contains("x-kuvo-route-version: 1"));
            let body = whole(&mut connection).await;
            if path.contains("bad") {
                assert_eq!(body, json!({"message":"Name what this line follows."}));
            } else {
                assert_eq!(
                    body,
                    json!({
                        "code":"unsupportedFormatVersion",
                        "message":"Kuvo route format version 99 is unsupported (this build writes 1)."
                    })
                );
            }
        }
    }
    server.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_slow_request_keeps_streaming_http11_alive_without_sending_an_interim_http10_head() {
    let home = tempfile::tempdir().unwrap();
    let mut routes = Registry::new();
    let (entered, mut calls) = mpsc::unbounded_channel();
    routes.engine(move |_, _| {
        let entered = entered.clone();
        async move {
            let (answer, ready) = oneshot::channel();
            entered.send(answer).unwrap();
            ready.await.unwrap()
        }
    });
    let server = Running::start(home.path(), routes.build(LineTiming::default())).await;
    for version in ["1.1", "1.0"] {
        let mut held = server
            .request("api/slow", version, "Connection: close\r\n", true)
            .await;
        let answer = calls.recv().await.unwrap();
        // The line remains responsive while another request is awaiting its area.
        let mut unrelated = server
            .request("kuvo/unknown", "1.1", "Connection: close\r\n", true)
            .await;
        assert!(head(&mut unrelated).await.starts_with("HTTP/1.1 404"));
        if version == "1.1" {
            for _ in 0..2 {
                time::advance(Duration::from_secs(20)).await;
                assert_eq!(head(&mut held).await, "HTTP/1.1 102 Processing\r\n\r\n");
            }
        } else {
            time::advance(Duration::from_secs(40)).await;
        }
        assert!(
            answer
                .send(Reply::json(StatusCode::OK, json!({"done":true})))
                .is_ok()
        );
        assert!(
            head(&mut held)
                .await
                .starts_with(&format!("HTTP/{version} 200"))
        );
        assert_eq!(whole(&mut held).await, json!({"done":true}));
    }
    server.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_busy_clients_http_connection_lives_past_the_advertised_idle_time() {
    let home = tempfile::tempdir().unwrap();
    let server = Running::start(home.path(), Registry::new().build(LineTiming::default())).await;
    let mut http = server.request("kuvo/unknown", "1.1", "", true).await;
    let first = head(&mut http).await;
    assert!(first.contains("keep-alive: timeout=5"));
    let mut body = [0; 2];
    http.read_exact(&mut body).await.unwrap();
    assert_eq!(&body, b"{}");
    time::advance(Duration::from_secs(7)).await;
    let token = STANDARD.encode(format!("opencode:{}", server.record.password));
    http.get_mut().write_all(format!("GET /kuvo/unknown HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic {token}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    assert!(head(&mut http).await.starts_with("HTTP/1.1 404"));
    assert_eq!(whole(&mut http).await, json!({}));
    server.stop().await;
}

#[tokio::test]
async fn a_stream_forwards_changes_and_ends_when_the_client_disconnects() {
    let home = tempfile::tempdir().unwrap();
    let mut routes = Registry::new();
    let (opened, mut openings) = mpsc::unbounded_channel();
    routes.engine(move |_, cancel| {
        let opened = opened.clone();
        async move {
            let (writes, mut input) = mpsc::unbounded_channel::<io::Result<Bytes>>();
            opened.send((writes, cancel)).unwrap();
            Reply::stream(
                StatusCode::OK,
                "text/event-stream",
                async_stream::stream! {
                    while let Some(part) = input.recv().await { yield part; }
                },
            )
        }
    });
    let server = Running::start(home.path(), routes.build(LineTiming::default())).await;
    for fail in [false, true] {
        let mut http = server.request("api/event", "1.1", "", true).await;
        let (writes, cancel) = openings.recv().await.unwrap();
        writes.send(Ok(Bytes::from_static(b"first\n"))).unwrap();
        assert!(head(&mut http).await.contains("transfer-encoding: chunked"));
        let mut chunk = [0; 11];
        http.read_exact(&mut chunk).await.unwrap();
        assert_eq!(&chunk, b"6\r\nfirst\n\r\n");
        if fail {
            writes
                .send(Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "fixture engine stopped",
                )))
                .unwrap();
            let mut rest = Vec::new();
            let _ = http.read_to_end(&mut rest).await;
            assert_eq!(rest, b"", "no final zero chunk on a cut stream");
        }
        drop(http);
        time::timeout(Duration::from_secs(3), cancel.cancelled())
            .await
            .expect("server stops the stream");
    }
    server.stop().await;
}
