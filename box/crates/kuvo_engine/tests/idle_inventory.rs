//! The resident session list follows OpenCode's live event line. The external engine is a
//! loopback HTTP stand-in; advancing Tokio's clock must not ask it for another whole list.
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{
    Request, Response,
    body::{Bytes, Frame, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use kuvo_engine::{Engine, LocalSessions, Timing};
use kuvo_server::Registry;
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::broadcast, time};

async fn contains(list: &mut LocalSessions, id: &str) {
    loop {
        if list.latest_record().is_some_and(|record| {
            record["sessions"].get(id).is_some()
                && record["pending"].as_array().is_none_or(Vec::is_empty)
        }) {
            return;
        }
        assert!(list.changed().await, "the local session list stopped");
    }
}

#[tokio::test]
async fn an_idle_list_only_reads_again_when_the_engine_tells_it_something_changed() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().to_string_lossy().into_owned();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let changed = Arc::new(AtomicUsize::new(0));
    let (events, _) = broadcast::channel::<Value>(8);
    let server = tokio::spawn({
        let changed = changed.clone();
        let events = events.clone();
        async move {
            while let Ok((socket, _)) = listener.accept().await {
                let (changed, events, directory) =
                    (changed.clone(), events.clone(), directory.clone());
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<Incoming>| {
                        let (changed, events, directory) =
                            (changed.clone(), events.clone(), directory.clone());
                        async move {
                            let reply: Response<UnsyncBoxBody<Bytes, Infallible>> = if request
                                .uri()
                                .path()
                                == "/api/event"
                            {
                                let mut incoming = events.subscribe();
                                let stream = async_stream::stream! {
                                    yield Ok(Frame::data(Bytes::from_static(b"data: {\"type\":\"server.connected\"}\n\n")));
                                    while let Ok(message) = incoming.recv().await {
                                        yield Ok(Frame::data(Bytes::from(format!("data: {message}\n\n"))));
                                    }
                                };
                                Response::builder()
                                    .header("content-type", "text/event-stream")
                                    .body(StreamBody::new(stream).boxed_unsync())
                                    .unwrap()
                            } else {
                                let value = if request.uri().path() == "/api/session" {
                                    let id = match changed.load(Ordering::Acquire) {
                                        0 => "ses_one",
                                        1 => "ses_ready",
                                        _ => "ses_two",
                                    };
                                    json!({"data":[{"id":id,"title":id,"location":{"directory":directory},"time":{"created":1,"updated":1}}]})
                                } else if request.uri().path() == "/api/session/active" {
                                    json!({"data":{}})
                                } else {
                                    json!({"data":[]})
                                };
                                Response::builder()
                                    .header("content-type", "application/json")
                                    .body(Full::new(Bytes::from(value.to_string())).boxed_unsync())
                                    .unwrap()
                            };
                            Ok::<_, Infallible>(reply)
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), service)
                        .await;
                });
            }
        }
    });
    std::fs::create_dir_all(home.path().join("run")).unwrap();
    // The conformance adapter uses this same legacy delegated-engine record. It is borrowed,
    // never signalled, and points only at the loopback test server in this process.
    std::fs::write(
        home.path().join("run/engine.json"),
        json!({"pid":std::process::id(),"port":port,"password":"fixture","version":"0.0.0"})
            .to_string(),
    )
    .unwrap();
    let mut env = kuvo_os::platform::Environment::new();
    env.insert("HOME".into(), home.path().into());
    env.insert("USERPROFILE".into(), home.path().into());
    let engine = Engine::new(
        home.path().to_owned(),
        env,
        Timing {
            idle: Duration::from_secs(600),
            check: Duration::from_secs(300),
        },
    )
    .unwrap();
    let mut list = engine.register(&mut Registry::new());
    time::timeout(Duration::from_secs(10), contains(&mut list, "ses_one"))
        .await
        .unwrap();
    // Keep a feed subscription before changing the engine's answer, so it is not a startup race.
    time::timeout(Duration::from_secs(10), async {
        while events.receiver_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Finish the initial inventory round and its startup follow before testing idle behavior.
    changed.store(1, Ordering::Release);
    events
        .send(json!({"type":"session.updated","data":{"sessionID":"ses_ready"}}))
        .unwrap();
    time::timeout(Duration::from_secs(10), contains(&mut list, "ses_ready"))
        .await
        .unwrap();
    // Allow queued startup requests and file-watch batches to settle before the silent edit.
    time::sleep(Duration::from_millis(500)).await;
    changed.store(2, Ordering::Release);
    time::pause();
    time::advance(Duration::from_secs(61)).await;
    time::resume();
    assert!(
        time::timeout(Duration::from_millis(800), contains(&mut list, "ses_two"))
            .await
            .is_err(),
        "the resident re-read the whole list without an inventory change"
    );
    events
        .send(json!({"type":"session.updated","data":{"sessionID":"ses_two"}}))
        .unwrap();
    time::timeout(Duration::from_secs(10), contains(&mut list, "ses_two"))
        .await
        .unwrap();
    // The old row must have gone, not merely appeared beside the changed inventory.
    assert!(
        list.latest_record().unwrap()["sessions"]
            .get("ses_one")
            .is_none()
    );
    drop(list);
    drop(engine);
    server.abort();
}
