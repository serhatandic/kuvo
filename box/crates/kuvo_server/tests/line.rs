//! The real dispatcher and line, with controlled state and event sources and a paused clock.

use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::BodyExt;
use hyper::body::Bytes;
use kuvo_protocol::routes::{
    get_kuvo_peer_session_computer_session_events as conversation,
    get_kuvo_waiting_events as waiting,
};
use kuvo_server::{
    Access, Caller, DispatchRequest, Dispatcher, JsonLine, LineTiming, LocalOrigin, Refusal,
    Registry, Reply, RouteReply, Transport,
};
use serde_json::{Value, json};
use tokio::{sync::mpsc, time};
use tokio_util::sync::CancellationToken;

fn local(origin: LocalOrigin) -> Caller {
    Caller::Local(Transport::Http {
        origin,
        internal: false,
    })
}

async fn ask(
    dispatch: &Dispatcher,
    method: Method,
    path: String,
    body: Bytes,
    caller: Caller,
) -> Reply {
    dispatch
        .dispatch(
            DispatchRequest {
                method,
                path,
                headers: HeaderMap::new(),
                body,
                caller,
            },
            CancellationToken::new(),
        )
        .await
}

struct Line(Reply);

impl Line {
    async fn open(dispatch: &Dispatcher, watch: Value, query: &str, caller: Caller) -> Self {
        let watch = URL_SAFE_NO_PAD.encode(watch.to_string());
        let reply = ask(
            dispatch,
            Method::GET,
            format!("kuvo/line?watch={watch}{query}"),
            Bytes::new(),
            caller,
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.headers["content-type"], "application/x-ndjson");
        assert_eq!(reply.headers["cache-control"], "no-store");
        Self(reply)
    }

    async fn next(&mut self) -> Option<Value> {
        let frame = time::timeout(Duration::from_secs(12), self.0.body.frame())
            .await
            .expect("line spoke");
        frame.map(|frame| serde_json::from_slice(&frame.unwrap().into_data().unwrap()).unwrap())
    }

    async fn read(&mut self) -> Value {
        self.next().await.expect("an open line")
    }

    async fn hello(&mut self, fresh: bool) -> String {
        let hello = self.read().await;
        assert_eq!(hello["hello"]["fresh"], fresh);
        assert_eq!(hello["hello"]["beat"], 5000);
        assert!(hello.get("n").is_none());
        hello["hello"]["line"].as_str().unwrap().into()
    }
}

async fn change(
    dispatch: &Dispatcher,
    line: &str,
    watch: Value,
    caller: Caller,
) -> (StatusCode, Value) {
    let reply = ask(
        dispatch,
        Method::POST,
        "kuvo/line/watch".into(),
        json!({"line": line, "watch": watch}).to_string().into(),
        caller,
    )
    .await;
    (
        reply.status,
        serde_json::from_slice(&reply.body.collect().await.unwrap().to_bytes()).unwrap(),
    )
}

fn state(n: u64) -> Value {
    json!({"sessions": n, "queue": 0, "bots": 0})
}

struct Sources {
    counts: mpsc::UnboundedReceiver<mpsc::UnboundedSender<u64>>,
    engine: mpsc::UnboundedReceiver<mpsc::UnboundedSender<Bytes>>,
    conversation: mpsc::UnboundedReceiver<mpsc::UnboundedSender<Bytes>>,
}

fn fixture(timing: LineTiming) -> (Dispatcher, Sources) {
    let (counts, counts_rx) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();
    let (peer, peer_rx) = mpsc::unbounded_channel();
    let mut registry = Registry::new();
    registry.register::<waiting::Spec, _, _>(Access::Any, move |_| {
        let counts = counts.clone();
        async move {
            let (out, mut input) = mpsc::unbounded_channel();
            counts.send(out).unwrap();
            Ok(RouteReply::ndjson::<200>(async_stream::stream! {
                while let Some(n) = input.recv().await {
                    let value = waiting::Status200 {
                        sessions: serde_json::Number::from(n),
                        queue: 0.into(), bots: 0.into(), dock: None, pending_computers: None,
                    };
                    yield JsonLine::new(value);
                }
            }))
        }
    });
    registry.register::<conversation::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let peer = peer.clone();
        async move {
            assert_eq!(call.param("computer"), Some("remote"));
            let (out, input) = mpsc::unbounded_channel();
            peer.send(out).unwrap();
            Ok(RouteReply::stream::<200>(
                "text/event-stream",
                chunks(input),
            ))
        }
    });
    registry.engine(move |request, _| {
        let events = events.clone();
        async move {
            let (out, input) = mpsc::unbounded_channel();
            assert_eq!(request.path, "api/event");
            events.send(out).unwrap();
            Reply::stream(StatusCode::OK, "text/event-stream", chunks(input))
        }
    });
    (
        registry.build(timing),
        Sources {
            counts: counts_rx,
            engine: events_rx,
            conversation: peer_rx,
        },
    )
}

fn chunks(
    mut input: mpsc::UnboundedReceiver<Bytes>,
) -> impl futures_core::Stream<Item = std::io::Result<Bytes>> {
    async_stream::stream! {
        while let Some(bytes) = input.recv().await { yield Ok(bytes); }
    }
}

#[tokio::test(start_paused = true)]
async fn carries_two_event_sources_and_a_list_as_one_numbered_line() {
    let (dispatch, mut sources) = fixture(LineTiming::default());
    let mut line = Line::open(&dispatch, json!({
        "engine":"api/event", "agent":"kuvo/peer-session/remote/ses_one/events", "wait":"kuvo/waiting/events",
    }), "", local(LocalOrigin::Mac)).await;
    line.hello(true).await;
    let events = sources.engine.recv().await.unwrap();
    events
        .send(Bytes::from_static(b"data: {\r\ndata: \"word\": \"\xc3"))
        .unwrap();
    events.send(Bytes::from_static(b"\xa9\"}\r\n\r\n")).unwrap();
    assert_eq!(
        line.read().await,
        json!({"n":1,"t":"engine","d":{"word":"é"}})
    );
    sources
        .conversation
        .recv()
        .await
        .unwrap()
        .send(Bytes::from_static(b"data: 7\n\n"))
        .unwrap();
    assert_eq!(line.read().await, json!({"n":2,"t":"agent","d":7}));
    let counts = sources.counts.recv().await.unwrap();
    counts.send(4).unwrap();
    assert_eq!(line.read().await, json!({"n":3,"t":"wait","d":state(4)}));
    counts.send(4).unwrap();
    counts.send(5).unwrap();
    assert_eq!(line.read().await, json!({"n":4,"t":"wait","d":state(5)}));
}

#[tokio::test(start_paused = true)]
async fn replays_after_the_last_seen_number_and_keeps_live_topics() {
    let (dispatch, mut sources) = fixture(LineTiming::default());
    let watch = json!({"wait":"kuvo/waiting/events"});
    let caller = local(LocalOrigin::Direct);
    let mut line = Line::open(&dispatch, watch.clone(), "", caller.clone()).await;
    let id = line.hello(true).await;
    let counts = sources.counts.recv().await.unwrap();
    counts.send(1).unwrap();
    assert_eq!(line.read().await["n"], 1);
    drop(line);
    counts.send(2).unwrap();
    counts.send(3).unwrap();
    let mut line = Line::open(&dispatch, watch, &format!("&line={id}&after=1"), caller).await;
    assert_eq!(line.hello(false).await, id);
    for n in [2, 3, 4] {
        if n == 4 {
            counts.send(n).unwrap();
        }
        assert_eq!(line.read().await, json!({"n":n,"t":"wait","d":state(n)}));
    }
}

#[tokio::test(start_paused = true)]
async fn starts_fresh_when_resume_position_is_evicted_expired_or_from_another_process() {
    let timing = LineTiming {
        replay_bytes: 1,
        ..LineTiming::default()
    };
    let (dispatch, mut sources) = fixture(timing);
    let watch = json!({"wait":"kuvo/waiting/events"});
    let caller = local(LocalOrigin::Direct);
    let mut line = Line::open(&dispatch, watch.clone(), "", caller.clone()).await;
    let id = line.hello(true).await;
    let counts = sources.counts.recv().await.unwrap();
    for n in [1, 2, 3] {
        counts.send(n).unwrap();
        assert_eq!(line.read().await["n"], n);
    }
    drop(line);
    let mut line = Line::open(
        &dispatch,
        watch.clone(),
        &format!("&line={id}&after=1"),
        caller.clone(),
    )
    .await;
    let fresh = line.hello(true).await;
    assert_ne!(fresh, id);
    sources.counts.recv().await.unwrap().send(4).unwrap();
    assert_eq!(line.read().await, json!({"n":1,"t":"wait","d":state(4)}));
    drop(line);
    assert_eq!(
        change(&dispatch, &fresh, watch.clone(), caller.clone())
            .await
            .0,
        StatusCode::OK
    );
    time::advance(timing.retention).await;
    assert_eq!(
        change(&dispatch, &fresh, watch.clone(), caller.clone()).await,
        (
            StatusCode::NOT_FOUND,
            json!({"fresh":true,"message":"Open the line again."})
        )
    );
    let (restarted, _) = fixture(timing);
    let mut line = Line::open(
        &restarted,
        json!({}),
        &format!("&line={fresh}&after=1"),
        caller,
    )
    .await;
    assert_ne!(line.hello(true).await, fresh);
}

#[tokio::test(start_paused = true)]
async fn a_topic_refused_under_the_lines_identity_ends_while_other_topics_continue() {
    let (dispatch, mut sources) = fixture(LineTiming::default());
    let mut line = Line::open(
        &dispatch,
        json!({
            "locked":"kuvo/peer-session/remote/ses_x/events",
            "wait":"kuvo/waiting/events",
        }),
        "",
        local(LocalOrigin::Direct),
    )
    .await;
    line.hello(true).await;
    let words = "Open this from a paired Kuvo Code client.";
    assert_eq!(
        line.read().await,
        json!({"n":1,"t":"locked","refused":{"status":403,"message":words,"said":{"message":words}}})
    );
    let counts = sources.counts.recv().await.unwrap();
    counts.send(1).unwrap();
    assert_eq!(line.read().await, json!({"n":2,"t":"wait","d":state(1)}));
    assert_eq!(line.read().await, json!({"beat":2}));
}

#[tokio::test(start_paused = true)]
async fn transient_refusals_retry_and_an_event_reopen_reports_a_gap() {
    let mut registry = Registry::new();
    let (opened, mut opens) = mpsc::unbounded_channel();
    registry.engine(move |_, _| {
        let opened = opened.clone();
        async move {
            let (send, reply) = tokio::sync::oneshot::channel();
            opened.send(send).unwrap();
            reply.await.unwrap()
        }
    });
    let dispatch = registry.build(LineTiming::default());
    let mut line = Line::open(
        &dispatch,
        json!({"events":"api/event"}),
        "",
        local(LocalOrigin::Direct),
    )
    .await;
    line.hello(true).await;
    for _ in 0..2 {
        let answer = opens.recv().await.unwrap();
        assert!(
            answer
                .send(Reply::json(
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({"message":"Starting."})
                ))
                .is_ok()
        );
    }
    assert_eq!(line.read().await["refused"]["status"], 503);
    for (gap, n) in [(2, 3), (4, 5)] {
        let answer = opens.recv().await.unwrap();
        assert!(
            answer
                .send(Reply::bytes(
                    StatusCode::OK,
                    "text/event-stream",
                    "data: 9\n\n"
                ))
                .is_ok()
        );
        assert_eq!(line.read().await, json!({"n":gap,"t":"events","gap":true}));
        assert_eq!(line.read().await, json!({"n":n,"t":"events","d":9}));
    }
}

#[tokio::test(start_paused = true)]
async fn a_quiet_event_source_reopens_and_an_idle_line_keeps_beating() {
    let (dispatch, mut sources) = fixture(LineTiming {
        events_quiet: Duration::from_secs(6),
        ..LineTiming::default()
    });
    let mut line = Line::open(
        &dispatch,
        json!({"events":"api/event"}),
        "",
        local(LocalOrigin::Direct),
    )
    .await;
    line.hello(true).await;
    let quiet = sources.engine.recv().await.unwrap();
    assert_eq!(line.read().await, json!({"beat":0}));
    let next = sources.engine.recv().await.unwrap();
    assert!(quiet.is_closed());
    next.send(Bytes::from_static(b"data: true\n\n")).unwrap();
    assert_eq!(line.read().await, json!({"n":1,"t":"events","gap":true}));
    assert_eq!(line.read().await, json!({"n":2,"t":"events","d":true}));
}

#[tokio::test(start_paused = true)]
async fn a_watch_changes_an_open_line_only_for_its_original_caller() {
    let (dispatch, mut sources) = fixture(LineTiming::default());
    let caller = local(LocalOrigin::App);
    let watch = json!({"wait":"kuvo/waiting/events"});
    let mut line = Line::open(&dispatch, json!({}), "", caller.clone()).await;
    let id = line.hello(true).await;
    assert_eq!(
        change(&dispatch, &id, watch.clone(), local(LocalOrigin::Mac))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        change(&dispatch, &id, watch.clone(), caller.clone()).await,
        (StatusCode::OK, json!({"line":id}))
    );
    let counts = sources.counts.recv().await.unwrap();
    counts.send(2).unwrap();
    assert_eq!(line.read().await["d"], state(2));
    assert_eq!(
        change(&dispatch, &id, json!({}), caller).await.0,
        StatusCode::OK
    );
    counts.closed().await;
    assert_eq!(line.read().await, json!({"beat":1}));
}

#[tokio::test(start_paused = true)]
async fn once_emits_one_snapshot_for_each_topic_and_finishes() {
    let (dispatch, mut sources) = fixture(LineTiming::default());
    let mut line = Line::open(
        &dispatch,
        json!({"a":"kuvo/waiting/events","b":"kuvo/waiting/events"}),
        "&once=1",
        local(LocalOrigin::Direct),
    )
    .await;
    let id = line.hello(true).await;
    let a = sources.counts.recv().await.unwrap();
    let b = sources.counts.recv().await.unwrap();
    a.send(1).unwrap();
    let first = line.read().await;
    for n in 2..100 {
        a.send(n).unwrap();
    }
    b.send(9).unwrap();
    let second = line.read().await;
    assert_ne!(first["t"], second["t"]);
    assert_eq!(first["d"], state(1));
    assert_eq!(second["d"], state(9));
    assert!(line.next().await.is_none());
    assert_eq!(
        change(&dispatch, &id, json!({}), local(LocalOrigin::Direct))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(start_paused = true)]
async fn rejects_a_recursive_watch_and_wrong_methods() {
    let mut registry = Registry::new();
    registry.register::<waiting::Spec, _, _>(Access::Local, |_| async {
        Err(Refusal::new(StatusCode::NOT_FOUND, "No state."))
    });
    let dispatch = registry.build(LineTiming::default());
    let caller = local(LocalOrigin::Direct);
    assert_eq!(
        change(
            &dispatch,
            "unknown",
            json!({"loop":"kuvo/line/watch"}),
            caller.clone()
        )
        .await,
        (
            StatusCode::BAD_REQUEST,
            json!({"message":"Name what this line follows."})
        )
    );
    let reply = ask(
        &dispatch,
        Method::POST,
        "kuvo/line".into(),
        Bytes::new(),
        caller,
    )
    .await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(reply.body.collect().await.unwrap().to_bytes(), "{}");
}
