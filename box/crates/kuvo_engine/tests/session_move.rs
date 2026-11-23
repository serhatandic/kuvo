// The real engine lease and session transfer API against a loopback OpenCode stand-in.

use std::{collections::HashMap, io, sync::Arc};

use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use kuvo_engine::{Engine, Timing};
use kuvo_store::files;
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::Mutex};

#[tokio::test]
async fn a_move_exports_every_child_and_can_undo_only_its_own_import() {
    let folder = tempfile::tempdir().unwrap();
    let home = folder.path();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let record =
        json!({"pid":std::process::id(),"port":port,"password":"fixture","version":"fixture"});
    files::write_atomic(
        &home.join("run/engine.json"),
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        let table = Arc::new(Mutex::new(HashMap::new()));
        while let Ok((socket, _)) = listener.accept().await {
            let table = table.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request| serve(request, table.clone()));
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(socket), service)
                    .await;
            });
        }
    });
    let engine = Engine::new(home.to_owned(), Default::default(), Timing::default()).unwrap();
    let movement = engine.session_move().await.unwrap();
    let tree = movement.export_tree("ses_root").await.unwrap();
    let ids: Vec<_> = tree
        .iter()
        .map(|session| session["info"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["ses_root", "ses_child1", "ses_child2", "ses_grandchild"]
    );
    assert_eq!(
        tree[3]["messages"],
        json!([{"id":"msg_saved","text":"kept"}])
    );

    let chats = home.join("chats");
    let old = "mov_one:1:hash";
    let new = "mov_two:2:hash";
    let first = json!({"info":{"id":"ses_first","metadata":{"kuvo":{"chat":"/old","archived":true}}},"messages":[]});
    movement.import(&first, &chats, old).await.unwrap();
    assert!(!movement.discard_import("ses_first", new).await.unwrap());
    let response = reqwest::get(format!("http://127.0.0.1:{port}/api/session/ses_first"))
        .await
        .unwrap();
    let bytes = response.bytes().await.unwrap();
    let saved: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        saved["data"]["metadata"],
        json!({
            "kuvo": {"chat": chats.to_str().unwrap(), "archived":true}, "kuvo.moveImport":old,
        })
    );
    assert!(movement.discard_import("ses_first", old).await.unwrap());
    assert!(!movement.discard_import("ses_first", old).await.unwrap());
    assert_eq!(
        reqwest::get(format!("http://127.0.0.1:{port}/api/session/ses_first"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let uncertain = json!({"info": {"id":"ses_uncertain"},"messages":[]});
    assert!(movement.import(&uncertain, &chats, new).await.is_err());
    drop(movement);
    drop(engine);
    let restarted = Engine::new(home.to_owned(), Default::default(), Timing::default()).unwrap();
    let recovered = restarted.session_move().await.unwrap();
    assert!(
        recovered
            .discard_import("ses_uncertain", new)
            .await
            .unwrap()
    );
    server.abort();
}

async fn serve(
    request: Request<Incoming>,
    imports: Arc<Mutex<HashMap<String, Value>>>,
) -> io::Result<Response<Full<Bytes>>> {
    let path = request.uri().path();
    let reply = |status, body: Value| {
        Ok(Response::builder()
            .status(status)
            .body(Full::new(Bytes::from(serde_json::to_vec(&body).unwrap())))
            .unwrap())
    };
    if path == "/api/session" && request.uri().query().is_some() {
        let query = request.uri().query().unwrap();
        let rows = if query.contains("parentID=ses_root") {
            if query.contains("cursor=next%2Fpage") {
                json!({"data":[{"id":"ses_child2"}]})
            } else {
                json!({"data":[{"id":"ses_child1"}],"cursor":{"next":"next/page"}})
            }
        } else if query.contains("parentID=ses_child2") {
            json!({"data":[{"id":"ses_grandchild"}]})
        } else {
            json!({"data":[]})
        };
        return reply(StatusCode::OK, rows);
    }
    if let Some(id) = path
        .strip_prefix("/api/experimental/session/")
        .and_then(|suffix| suffix.strip_suffix("/export"))
    {
        let parent = match id {
            "ses_child1" | "ses_child2" => Some("ses_root"),
            "ses_grandchild" => Some("ses_child2"),
            _ => None,
        };
        return reply(
            StatusCode::OK,
            json!({"data":{
                "info":{"id":id,"parentID":parent},"messages":[{"id":"msg_saved","text":"kept"}],
            }}),
        );
    }
    if path == "/api/experimental/session/import" && request.method() == Method::POST {
        let body = request.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let id = value["info"]["id"].as_str().unwrap().to_owned();
        let lost = id == "ses_uncertain";
        imports.lock().await.insert(id, value["info"].clone());
        if lost {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "reply interrupted",
            ));
        }
        return reply(StatusCode::OK, json!({"data":true}));
    }
    if let Some(id) = path.strip_prefix("/api/session/") {
        let mut imports = imports.lock().await;
        let found = if request.method() == Method::DELETE {
            imports.remove(id)
        } else {
            imports.get(id).cloned()
        };
        return match found {
            Some(info) => reply(StatusCode::OK, json!({"data":info})),
            None => reply(StatusCode::NOT_FOUND, json!({"message":"missing"})),
        };
    }
    reply(StatusCode::OK, json!({"data":[]}))
}
