//! Two stdio pipes and the real resident IPC, using the desktop's Noise initiator and fixture grants.

use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use http::StatusCode;
use kuvo_protocol::routes::{get_kuvo_waiting_events as waiting, post_kuvo_agent_tools as tool};
use kuvo_server::{
    Access, LineTiming, Refusal, Registry, Reply, Resident, RouteReply,
    ssh::{Command, Grant, IdentityRefusal, NoPairings, Pairings, pipe},
};
use kuvo_wire::noise as desktop_noise;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{self, timeout},
};

const BOX: [u8; 32] = [0x08; 32];
const DEVICE: [u8; 32] = [0x09; 32];

fn public(secret: [u8; 32]) -> [u8; 32] {
    x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(secret)).to_bytes()
}

#[derive(Default)]
struct Grants {
    removed: AtomicBool,
}

impl Pairings for Grants {
    fn identity(&self, computer: &str) -> Result<[u8; 32], IdentityRefusal> {
        (computer == URL_SAFE_NO_PAD.encode(public(BOX)))
            .then_some(BOX)
            .ok_or(IdentityRefusal::WrongComputer)
    }

    fn paired(&self, key: &[u8; 32]) -> Option<Grant> {
        (*key == public(DEVICE) && !self.removed.load(Ordering::Relaxed)).then(|| Grant {
            id: "box-test-grant".into(),
            name: "Desktop fixture".into(),
        })
    }
}

struct Server {
    quit: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}

impl Server {
    async fn start(home: &Path, routes: Registry, pairings: Arc<dyn Pairings>) -> Self {
        let dispatcher = routes.build(LineTiming::default());
        let (quit, done) = oneshot::channel();
        let resident = Resident::bind(home).await.unwrap().unwrap();
        let task = tokio::spawn(resident.serve(dispatcher, pairings, async {
            let _ = done.await;
        }));
        Self {
            quit: Some(quit),
            task,
        }
    }

    async fn stop(mut self) {
        self.quit.take().unwrap().send(()).unwrap();
        (&mut self.task).await.unwrap().unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Entry {
    stdin: DuplexStream,
    stdout: BufReader<DuplexStream>,
    task: JoinHandle<io::Result<()>>,
}

impl Entry {
    fn start(home: &Path, command: Command) -> Self {
        let (stdin, from) = tokio::io::duplex(65536);
        let (into, stdout) = tokio::io::duplex(65536);
        let home = home.to_owned();
        let task = tokio::spawn(async move { pipe(&home, command, from, into).await });
        Self {
            stdin,
            stdout: BufReader::new(stdout),
            task,
        }
    }

    async fn send(&mut self, value: Value) {
        self.stdin
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn read(&mut self) -> Value {
        read_json(&mut self.stdout).await
    }

    async fn rest(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(10), self.stdout.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        bytes
    }

    async fn finish(mut self) {
        assert!(self.rest().await.is_empty());
        (&mut self.task).await.unwrap().unwrap();
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_json(input: &mut BufReader<impl AsyncRead + Unpin>) -> Value {
    let mut line = String::new();
    timeout(Duration::from_secs(10), input.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line:?}"))
}

async fn hello(
    entry: &mut Entry,
    secret: [u8; 32],
    computer: &str,
) -> (Value, Option<desktop_noise::Transport>) {
    let mut noise = desktop_noise::Initiator::new(secret, public(BOX), b"kuvo-ssh v1");
    let ask = noise
        .write_message_1(br#"{"v":1,"client":"desktop"}"#)
        .unwrap();
    entry
        .send(json!({"computer":computer,"hello":URL_SAFE_NO_PAD.encode(ask)}))
        .await;
    let head = entry.read().await;
    if let Some(answer) = head["hello"].as_str() {
        let (payload, transport) = noise
            .read_message_2(&URL_SAFE_NO_PAD.decode(answer).unwrap())
            .unwrap();
        (serde_json::from_slice(&payload).unwrap(), Some(transport))
    } else {
        (head, None)
    }
}

async fn addressed(entry: &mut Entry, given: Value) {
    let (head, transport) = hello(entry, DEVICE, &URL_SAFE_NO_PAD.encode(public(BOX))).await;
    assert_eq!(head, json!({"allowed":true}));
    let sealed = transport.unwrap().seal(given.to_string().as_bytes());
    entry
        .send(json!({"request":URL_SAFE_NO_PAD.encode(sealed)}))
        .await;
}

async fn legacy(home: &Path, given: Value) -> Value {
    let mut call = Entry::start(
        home,
        Command::Route {
            internal: false,
            internal_path: None,
        },
    );
    call.send(given).await;
    call.stdin.shutdown().await.unwrap();
    let answer = call.read().await;
    call.finish().await;
    answer
}

#[tokio::test]
async fn addressed_ssh_checks_computer_and_pairing_and_preserves_large_binary_requests() {
    let home = tempfile::tempdir().unwrap();
    let mut routes = Registry::new();
    routes.engine(|request, _| async move {
        assert_eq!(request.path, "api/echo");
        assert_eq!(request.caller.pairing().unwrap().id, "box-test-grant");
        assert_eq!(request.headers["x-opencode-directory"], "/fixture");
        Reply::bytes(StatusCode::OK, "application/octet-stream", request.body)
    });
    let server = Server::start(home.path(), routes, Arc::new(Grants::default())).await;
    let mut wrong = Entry::start(home.path(), Command::RouteTo);
    let (refusal, transport) = hello(&mut wrong, DEVICE, &"A".repeat(43)).await;
    assert!(transport.is_none());
    assert_eq!(refusal["status"], 409);
    assert_eq!(refusal["routeVersion"], "1");
    wrong.finish().await;

    let mut unpaired = Entry::start(home.path(), Command::RouteTo);
    let (greeting, _) = hello(&mut unpaired, [7; 32], &URL_SAFE_NO_PAD.encode(public(BOX))).await;
    assert_eq!(greeting, json!({"allowed":false}));
    assert_eq!(unpaired.read().await["status"], 403);
    unpaired.finish().await;

    let bytes: Vec<u8> = (0..150_000).map(|i| i as u8).collect();
    for streaming in [true, false] {
        let mut call = Entry::start(home.path(), Command::RouteTo);
        addressed(
            &mut call,
            json!({
                "path":"api/echo","method":"POST","stream":streaming,
                "bodyBase64":STANDARD.encode(&bytes),
                "headers":{"X-OpenCode-Directory":"/fixture","X-Kuvo-Route-Version":"1"}
            }),
        )
        .await;
        if !streaming {
            call.stdin.shutdown().await.unwrap();
        }
        let head = call.read().await;
        assert_eq!(head["status"], 200);
        assert_eq!(head["computer"], URL_SAFE_NO_PAD.encode(public(BOX)));
        assert_eq!(head["routeVersion"], "1");
        assert_eq!(head["type"], "application/octet-stream");
        if streaming {
            assert_eq!(call.rest().await, bytes);
        } else {
            assert_eq!(
                STANDARD
                    .decode(head["bodyBase64"].as_str().unwrap())
                    .unwrap(),
                bytes
            );
        }
        call.finish().await;
    }
    server.stop().await;
}

#[tokio::test(start_paused = true)]
async fn addressed_streams_close_when_stdin_ends_or_the_device_grant_is_revoked() {
    let home = tempfile::tempdir().unwrap();
    let grants = Arc::new(Grants::default());
    let mut routes = Registry::new();
    let (opened, mut openings) = mpsc::unbounded_channel();
    routes.engine(move |_, cancel| {
        let opened = opened.clone();
        async move {
            opened.send(cancel).unwrap();
            Reply::stream(
                StatusCode::OK,
                "text/event-stream",
                futures_util::stream::pending::<io::Result<hyper::body::Bytes>>(),
            )
        }
    });
    let server = Server::start(home.path(), routes, grants.clone()).await;
    for revoke in [false, true] {
        let mut call = Entry::start(home.path(), Command::RouteTo);
        addressed(&mut call, json!({"path":"api/event","stream":true})).await;
        let token = openings.recv().await.unwrap();
        assert_eq!(call.read().await["status"], 200);
        if revoke {
            grants.removed.store(true, Ordering::Relaxed);
            time::advance(Duration::from_secs(2)).await;
        } else {
            call.stdin.shutdown().await.unwrap();
        }
        call.finish().await;
        assert!(token.is_cancelled());
    }
    server.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_tool_call_queues_through_startup_and_keeps_its_operation_after_admission_is_cut() {
    let home = tempfile::tempdir().unwrap();
    let (observed, mut calls) = mpsc::unbounded_channel();
    let routes = |restarting: bool| {
        let observed = observed.clone();
        let mut registry = Registry::new();
        registry.register::<tool::Spec, _, _>(Access::Internal, move |call| {
            let observed = observed.clone();
            async move {
                let body: Value = serde_json::from_slice(&call.request.body).unwrap();
                observed
                    .send((body["operation"].clone(), call.cancel))
                    .unwrap();
                if restarting {
                    std::future::pending().await
                } else {
                    Err(Refusal::new(
                        StatusCode::CONFLICT,
                        "Fixture has already accepted this call.",
                    ))
                }
            }
        });
        registry
    };
    let mut queued = Entry::start(
        home.path(),
        Command::Route {
            internal: true,
            internal_path: None,
        },
    );
    queued
        .send(json!({"path":"kuvo/agent-tools","method":"POST","body":{
            "name":"send_message","arguments":{"to":"ses_fixture","text":"Check it."}
        }}))
        .await;
    queued.stdin.shutdown().await.unwrap();
    time::advance(Duration::from_secs(6)).await;
    assert!(!queued.task.is_finished());
    let server = Server::start(home.path(), routes(true), Arc::new(NoPairings)).await;
    let (first, cancel) = calls.recv().await.unwrap();
    assert_eq!(first.as_str().unwrap().len(), 36);
    server.stop().await;
    assert!(cancel.is_cancelled());
    let server = Server::start(home.path(), routes(false), Arc::new(NoPairings)).await;
    let (retry, _) = calls.recv().await.unwrap();
    assert_eq!(retry, first);
    assert_eq!(queued.read().await["status"], 409);
    queued.finish().await;
    server.stop().await;
}

#[tokio::test]
async fn legacy_ssh_validates_versions_preserves_authority_and_can_read_a_line_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let mut routes = Registry::new();
    routes.register::<waiting::Spec, _, _>(Access::LocalApp, |_| async {
        Ok(RouteReply::ndjson::<200>(futures_util::stream::iter([
            kuvo_server::JsonLine::new(waiting::Status200 {
                sessions: 2.into(),
                bots: 0.into(),
                queue: 0.into(),
                dock: None,
                pending_computers: None,
            }),
        ])))
    });
    let server = Server::start(home.path(), routes, Arc::new(NoPairings)).await;
    for (headers, code) in [
        (
            json!({"X-Kuvo-Route-Version":"2"}),
            "unsupportedFormatVersion",
        ),
        (
            json!({"X-Kuvo-Route-Version":"1","x-kuvo-route-version":"2"}),
            "unsupportedFormatVersion",
        ),
        (
            json!({"Accept":"application/json","accept":"text/plain"}),
            "conflictingHeader",
        ),
    ] {
        let answer = legacy(
            home.path(),
            json!({"path":"kuvo/waiting/events","headers":headers}),
        )
        .await;
        assert_eq!(answer["status"], 400);
        assert_eq!(answer["routeVersion"], "1");
        assert_eq!(
            serde_json::from_str::<Value>(answer["body"].as_str().unwrap()).unwrap()["code"],
            code
        );
    }
    let forged = legacy(
        home.path(),
        json!({
            "path":"kuvo/waiting/events","fromApp":true,"served":true,
            "headers":{"x-kuvo-from":"app","Accept":"application/json","accept":"application/json"},
        }),
    )
    .await;
    assert_eq!(forged["status"], 403);

    let watch = URL_SAFE_NO_PAD.encode(br#"{"waiting":"kuvo/waiting/events"}"#);
    let snapshot = legacy(
        home.path(),
        json!({"path":format!("kuvo/line?watch={watch}&once=1")}),
    )
    .await;
    let answer: Vec<Value> = snapshot["body"]
        .as_str()
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(answer.len(), 2);
    assert_eq!(answer[0]["hello"]["fresh"], true);
    assert_eq!(answer[1]["refused"]["status"], 403);
    server.stop().await;
}
