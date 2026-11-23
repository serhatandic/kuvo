//! The local composer's durable handoff and receipt feed.

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use http::StatusCode;
use http_body_util::BodyExt;
use kuvo_protocol::routes::{
    get_kuvo_data_messages_operation as receipt,
    get_kuvo_data_messages_operation_events as receipt_events,
    get_kuvo_sessions_id_delivery as read, get_kuvo_sessions_id_delivery_events as events,
    post_kuvo_sessions_id_delivery as save,
};
use kuvo_server::{
    Access, Caller, JsonLine, LocalOrigin, Refusal, Registry, RouteCall, RouteReply, Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time,
};

use crate::{
    Engine, LocalSessions, disk, queue, sessions,
    typed::{self, Draft},
};

const MAX_PENDING: usize = 200;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Outcome {
    Pending {
        message: String,
    },
    Committed {
        #[serde(rename = "acceptedAt")]
        accepted_at: u64,
    },
    Interrupted {
        message: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
struct Receipt {
    operation: String,
    owner: String,
    #[serde(flatten)]
    outcome: Outcome,
}

#[derive(Clone, Serialize, Deserialize)]
struct Operation {
    setup: String,
    record: String,
    operation: String,
    owner: String,
    body: Value,
    draft: bool,
    delivery: bool,
    submitter: Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default)]
    format_version: u32,
    record: String,
    setup: String,
    digest: String,
    receipt: Receipt,
    at: u64,
    draft: bool,
    delivery: bool,
    submitter: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued_delivery: Option<Operation>,
}

fn hash(mut value: Value) -> String {
    value.sort_all_objects();
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.to_string()))
}

fn operation(session: &str, id: &str) -> String {
    hash(json!(["delivery-v1", "", session, id]))
}

fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn store_error(_: io::Error) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This computer couldn’t save the message receipt. Try again in a moment.",
    )
}

fn gone() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "There’s no such message operation here.",
    )
}

fn local<T>(call: &RouteCall<T>) -> Result<(), Refusal> {
    if !typed::author(&call.request.caller).is_ok_and(|author| author == "computer") {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "Open this from a paired Kuvo Code client.",
        ));
    }
    let computer = crate::query(&call.request.path, "computer").unwrap_or_else(|| "here".into());
    if computer == "here" {
        return Ok(());
    }
    if !valid_key(&computer) {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Name the conversation’s computer.",
        ));
    }
    Err(Refusal::new(
        StatusCode::NOT_FOUND,
        "There’s no such computer in this setup.",
    ))
}

#[derive(Clone)]
struct Delivery {
    store: PathBuf,
    name: String,
    wake: mpsc::Sender<()>,
    changes: watch::Receiver<u64>,
    _task: Arc<Task>,
}

struct Task(JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn file(root: &Path, family: &str, key: &str) -> PathBuf {
    root.join(family).join(format!("{key}.json"))
}

fn local_record(saved: &Saved, key: &str) -> bool {
    saved.format_version <= 1
        && saved.setup.is_empty()
        && saved.delivery
        && saved.receipt.operation == key
        && saved.receipt.owner == "here"
        && saved.submitter["kind"] == "computer"
}

async fn lookup(root: &Path, key: &str) -> io::Result<Option<Saved>> {
    if !valid_key(key) {
        return Ok(None);
    }
    for family in ["message-receipts", "outbox"] {
        if let Some(saved) = disk::read::<Saved>(&file(root, family, key)).await? {
            if saved.format_version > 1 {
                return Err(io::Error::other("Unsupported message receipt format."));
            }
            return Ok(local_record(&saved, key).then_some(saved));
        }
    }
    Ok(None)
}

async fn pending(root: &Path) -> io::Result<Vec<(Saved, Operation)>> {
    let mut list = match tokio::fs::read_dir(root.join("outbox")).await {
        Ok(list) => list,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut saved = Vec::new();
    while let Some(entry) = list.next_entry().await? {
        let name = entry.file_name();
        let Some(key) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|key| valid_key(key))
        else {
            continue;
        };
        let Some(found) = lookup(root, key).await.ok().flatten() else {
            continue;
        };
        if let Some(op) = found.queued_delivery.clone()
            && matches!(found.receipt.outcome, Outcome::Pending { .. })
            && op.setup.is_empty()
            && op.operation == key
            && op.owner == "here"
            && op.record == found.record
            && op.submitter["kind"] == "computer"
            && op.delivery
            && op.draft
            && op.body["id"]
                .as_str()
                .is_some_and(|id| operation(&found.record, id) == key)
            && sessions::valid_id(&found.record)
        {
            saved.push((found, op));
        }
    }
    saved.sort_by(|(a, _), (b, _)| {
        a.at.cmp(&b.at)
            .then(a.receipt.operation.cmp(&b.receipt.operation))
    });
    Ok(saved)
}

async fn finish(
    engine: &Engine,
    list: &LocalSessions,
    root: &Path,
    mut saved: Saved,
    op: Operation,
) -> io::Result<bool> {
    let Ok(draft) = serde_json::from_value::<Draft>(op.body) else {
        return Ok(false);
    };
    let id = draft.id.clone();
    let caller = Caller::Local(Transport::Http {
        origin: LocalOrigin::App,
        internal: true,
    });
    let (status, response) =
        match typed::submit(engine, list, &op.record, draft, &caller, "computer").await {
            Ok(response) => (StatusCode::OK, response),
            Err(error) => {
                let response = error.reply();
                if response.status.is_server_error() {
                    return Ok(false);
                }
                let status = response.status;
                let body = response
                    .body
                    .collect()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_slice(&body.to_bytes()).ok())
                    .unwrap_or_default();
                (status, body)
            }
        };
    saved.receipt.outcome = if status == StatusCode::OK && response["message"] == id {
        Outcome::Committed {
            accepted_at: response["acceptedAt"].as_u64().unwrap_or_else(disk::now),
        }
    } else {
        let message = response["answer"]["error"]
            .as_str()
            .or_else(|| response["message"].as_str())
            .unwrap_or("The message could not be sent.")
            .to_owned();
        Outcome::Interrupted { message }
    };
    saved.queued_delivery = None;
    let key = &saved.receipt.operation;
    disk::durable(file(root, "message-receipts", key), saved.clone()).await?;
    let _ = tokio::fs::remove_file(file(root, "outbox", key)).await;
    Ok(true)
}

impl Delivery {
    fn start(engine: Engine, list: LocalSessions, name: String) -> Self {
        let store = engine.home.join("run/owned-data");
        let root = store.clone();
        let (wake, mut nudge) = mpsc::channel::<()>(1);
        let (updates, changes) = watch::channel(0u64);
        let task = tokio::spawn(async move {
            let mut tick = time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => {},
                    received = nudge.recv() => {
                        if received.is_none() { break; }
                        updates.send_modify(|change| *change = change.wrapping_add(1));
                    },
                }
                let Ok(waiting) = pending(&root).await else {
                    continue;
                };
                for (saved, op) in waiting {
                    // A failure at the head can recover on the next tick without reordering.
                    match finish(&engine, &list, &root, saved, op).await {
                        Ok(true) => updates.send_modify(|change| *change = change.wrapping_add(1)),
                        _ => break,
                    }
                }
            }
        });
        Self {
            store,
            name,
            wake,
            changes,
            _task: Arc::new(Task(task)),
        }
    }

    async fn save(
        &self,
        engine: &Engine,
        session: &str,
        draft: Draft,
        body: Value,
    ) -> Result<Saved, Refusal> {
        let key = operation(session, &draft.id);
        let submitter = json!({"kind":"computer"});
        let digest = hash(json!({"owner":"here","body":body,"submitter":submitter}));
        let _writer = engine
            .decision_writer
            .acquire()
            .await
            .expect("the decision writer stays open");
        if let Some(prior) = lookup(&self.store, &key).await.map_err(store_error)? {
            if prior.digest == digest {
                return Ok(prior);
            }
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "That message id is already used.",
            ));
        }
        let rows = pending(&self.store).await.map_err(store_error)?;
        if rows.len() >= MAX_PENDING {
            return Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Too many messages are waiting for their computers.",
            ));
        }
        let at = rows.last().map_or(disk::now(), |(prior, _)| {
            disk::now().max(prior.at.saturating_add(1))
        });
        let receipt = Receipt {
            operation: key.clone(),
            owner: "here".into(),
            outcome: Outcome::Pending {
                message: format!("Unsent. Delivered when {} accepts it.", self.name),
            },
        };
        let op = Operation {
            setup: String::new(),
            record: session.to_owned(),
            operation: key.clone(),
            owner: "here".into(),
            body,
            draft: true,
            delivery: true,
            submitter: submitter.clone(),
        };
        let saved = Saved {
            format_version: 1,
            record: session.to_owned(),
            setup: String::new(),
            digest,
            receipt,
            at,
            draft: true,
            delivery: true,
            submitter,
            queued_delivery: Some(op),
        };
        disk::durable(file(&self.store, "outbox", &key), saved.clone())
            .await
            .map_err(store_error)?;
        let _ = self.wake.try_send(());
        Ok(saved)
    }

    async fn receipt(&self, key: &str) -> Result<Receipt, Refusal> {
        let saved = lookup(&self.store, key)
            .await
            .map_err(store_error)?
            .ok_or_else(gone)?;
        Ok(saved.receipt)
    }

    async fn queue(&self, engine: Option<&Engine>, session: &str) -> Result<Value, Refusal> {
        let mut shown = if let Some(engine) = engine {
            queue::snapshot(engine, session, true).await?
        } else {
            json!({"paused":false,"held":[],"items":[]})
        };
        let mut items = shown["items"]
            .as_array_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        let ids = items
            .iter()
            .filter_map(|row| row["id"].as_str().map(str::to_owned))
            .collect::<std::collections::BTreeSet<_>>();
        for (_, op) in pending(&self.store)
            .await
            .map_err(store_error)?
            .into_iter()
            .filter(|(_, op)| op.record == session)
        {
            let id = &op.body["id"];
            if id.as_str().is_some_and(|id| ids.contains(id)) {
                continue;
            }
            let words = op.body["quotedText"]
                .as_str()
                .or_else(|| op.body["text"].as_str())
                .unwrap_or_default();
            let files = op.body["message"]["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file["name"].as_str())
                .collect::<Vec<_>>();
            items.push(json!({"id":id,"words":words,"files":files,"from":null,"on":"box","delivery":"queue","held":false}));
        }
        shown["items"] = json!(items);
        Ok(shown)
    }
}

fn key<T>(call: &RouteCall<T>) -> Result<String, Refusal> {
    call.param("operation")
        .filter(|key| valid_key(key))
        .map(str::to_owned)
        .ok_or_else(gone)
}

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) {
    let name = std::fs::read(engine.home.join("run/link.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|link| link["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "this computer".into());
    let delivery = Delivery::start(engine.clone(), list, name);
    let store = delivery.clone();
    let here = engine.clone();
    routes.register::<save::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        let here = here.clone();
        async move {
            local(&call)?;
            let session = sessions::id(&call)?;
            let draft = typed::read_draft(&call)?;
            let body: Value = serde_json::from_slice(&call.request.body).map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, "Send a draft with its stable id, text and message fields."))?;
            let files = body["message"].get("files");
            let inline = |file: &Value| {
                file["uri"].as_str().is_some_and(|uri| uri.starts_with("file:") || uri.starts_with("data:"))
                    || file["url"].as_str().is_some_and(|url| url.starts_with("data:"))
            };
            let stable = draft.id.strip_prefix("msg_").is_some_and(|id| {
                (1..=80).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
            if !stable || body["side"] == true || files.is_some_and(|files| !files.as_array().is_some_and(|files| files.iter().all(inline))) {
                return Err(Refusal::new(StatusCode::BAD_REQUEST, "Send a draft with its stable id, text and message fields. Attachments for another computer need to fit in a 5 MiB draft."));
            }
            let saved = store.save(&here, &session, draft, body.clone()).await?;
            match saved.receipt.outcome {
                Outcome::Pending { .. } => Ok(RouteReply::json::<202>(sessions::typed(serde_json::to_value(saved.receipt).map_err(io::Error::other).map_err(store_error)?)?)),
                Outcome::Committed { accepted_at } => Ok(RouteReply::json::<200>(sessions::typed(json!({
                    "answer":{"send":{"text":body["text"],"quoted":true}},"message":body["id"],"acceptedAt":accepted_at
                }))?)),
                Outcome::Interrupted { message } => Err(Refusal::json(StatusCode::CONFLICT, json!({"message":message,"ambiguous":true}))),
            }
        }
    });
    let store = delivery.clone();
    routes.register::<receipt::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        async move {
            local(&call).map_err(|_| gone())?;
            let receipt = store.receipt(&key(&call)?).await?;
            Ok(RouteReply::json::<200>(sessions::typed(
                serde_json::to_value(receipt)
                    .map_err(io::Error::other)
                    .map_err(store_error)?,
            )?))
        }
    });
    let store = delivery.clone();
    routes.register::<receipt_events::Spec, _, _>(Access::Any, move |call| {
        let mut store = store.clone();
        async move {
            local(&call).map_err(|_| gone())?;
            let key = key(&call)?;
            let initial = store.receipt(&key).await?;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut current = initial;
                let mut last = Value::Null;
                loop {
                    let value = serde_json::to_value(&current).map_err(io::Error::other)?;
                    if value != last {
                        yield JsonLine::new(serde_json::from_value(value.clone()).map_err(io::Error::other)?)?;
                        last = value;
                    }
                    tokio::select! {
                        changed = store.changes.changed() => if changed.is_err() { break; },
                        () = call.cancel.cancelled() => break,
                    }
                    let Ok(next) = store.receipt(&key).await else { break; };
                    current = next;
                }
            }))
        }
    });
    let store = delivery.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        async move {
            local(&call)?;
            let snapshot = store.queue(None, &sessions::id(&call)?).await?;
            Ok(RouteReply::json::<200>(sessions::typed(snapshot)?))
        }
    });
    let here = engine.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let mut store = delivery.clone();
        let here = here.clone();
        async move {
            local(&call)?;
            let session = sessions::id(&call)?;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut fresh = store.queue(None, &session).await.map_err(|error| io::Error::other(error.reply().status.to_string()))?;
                let mut last = Value::Null;
                let mut tick = time::interval(Duration::from_secs(1));
                loop {
                    if fresh != last {
                        yield JsonLine::new(serde_json::from_value(fresh.clone()).map_err(io::Error::other)?)?;
                        last = fresh.clone();
                    }
                    tokio::select! {
                        changed = store.changes.changed() => if changed.is_err() { break; },
                        () = call.cancel.cancelled() => break,
                        _ = tick.tick() => {},
                    }
                    if let Ok(queue) = store.queue(Some(&here), &session).await { fresh = queue; }
                }
            }))
        }
    });
}
