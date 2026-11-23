//! Shared treatment of stored and streaming conversation summaries.

mod reasoning;

use std::{collections::BTreeSet, sync::Arc};

use serde_json::{Value, json};

pub(crate) fn compaction_page(page: &mut Value) {
    match page.get_mut("data") {
        Some(Value::Array(messages)) => messages.iter_mut().for_each(compaction_message),
        Some(message) => compaction_message(message),
        None => {}
    }
}

fn compaction_message(message: &mut Value) {
    if message["type"] != "compaction" {
        return;
    }
    let status = message["status"].as_str();
    let title = match status {
        Some("running") => "Summarizing the conversation…",
        Some("completed") => "Conversation summarized",
        Some("failed") => "Couldn’t summarize the conversation",
        _ => "Conversation summary",
    };
    if status == Some("running") {
        message["workingTitle"] = json!(title);
    }
    message["title"] = json!(title);
}

pub(crate) fn compaction_event(line: &[u8]) -> Option<Vec<u8>> {
    let payload = line.strip_prefix(b"data:")?;
    if !payload
        .windows(b"session.compaction.".len())
        .any(|part| part == b"session.compaction.")
    {
        return None;
    }
    let mut event: Value = serde_json::from_slice(payload).ok()?;
    let status = match event["type"].as_str()? {
        "session.compaction.started" => "running",
        "session.compaction.ended" => "completed",
        "session.compaction.failed" => "failed",
        _ => return None,
    };
    let data = event.get_mut("data")?.as_object_mut()?;
    let mut message = json!({ "type": "compaction", "status": status });
    compaction_message(&mut message);
    data.insert("title".into(), message["title"].take());
    if let Some(working) = message.get_mut("workingTitle") {
        data.insert("workingTitle".into(), working.take());
    }
    let mut encoded = b"data: ".to_vec();
    serde_json::to_writer(&mut encoded, &event).ok()?;
    encoded.extend_from_slice(if line.ends_with(b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    });
    Some(encoded)
}

#[derive(serde::Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, alias = "properties")]
    data: Value,
}

pub(crate) fn watch(
    engine: crate::Engine,
    refresh: tokio::sync::mpsc::WeakSender<()>,
    inventory_changed: tokio::sync::watch::Sender<u64>,
    ask_updates: tokio::sync::watch::Sender<Arc<crate::asks::Book>>,
    models_changed: tokio::sync::mpsc::Sender<()>,
    thinking: tokio::sync::watch::Sender<Arc<BTreeSet<String>>>,
    stop: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    use futures_util::StreamExt;
    use http::Method;
    use tokio::time::{Duration, sleep};

    let attachments = crate::attachment_retention::watch(engine.clone());
    tokio::spawn(async move {
        let mut states = engine.states();
        let mut asks = crate::asks::Book::new();
        let mut reasoning = reasoning::Reasoning::default();
        let mut generation = None;
        let mut queues = crate::queue::QueueEvents::default();
        let mut scan = None::<tokio::task::JoinHandle<()>>;
        loop {
            let Some(keep) = refresh.upgrade() else {
                break;
            };
            if keep.is_closed() {
                break;
            }
            // Retain no sender while the feed is open; the list owns its lifetime.
            drop(keep);
            let Some(lease) = engine.acquire(crate::lifetime::Demand::Passive).await else {
                tokio::select! {
                    _ = states.changed() => {},
                    _ = sleep(Duration::from_secs(2)) => {},
                }
                continue;
            };
            states.borrow_and_update();
            let pin = (lease.record.pid, lease.record.port);
            if generation.is_none() {
                asks = crate::asks::restore(&engine, pin).await;
                if !asks.is_empty() {
                    ask_updates.send_replace(std::sync::Arc::new(asks.clone()));
                }
            } else if generation != Some(pin) {
                asks.clear();
                reasoning = reasoning::Reasoning::default();
                thinking.send_replace(Arc::new(BTreeSet::new()));
                queues = crate::queue::QueueEvents::default();
                ask_updates.send_replace(std::sync::Arc::new(asks.clone()));
            }
            let new_generation = generation != Some(pin);
            if new_generation {
                crate::asks::checkpoint(&engine, pin, &asks).await;
                let _ = models_changed.try_send(());
            }
            let request = engine
                .upstream
                .request(&lease.record, Method::GET, "api/event");
            let connected = tokio::time::timeout(crate::api::WAIT, request.send()).await;
            if let Ok(Ok(response)) = connected
                && response.status().is_success()
            {
                engine.catalogs.invalidate(false);
                if new_generation {
                    generation = Some(pin);
                    if let Some(previous) = scan.take() {
                        previous.abort();
                    }
                    let engine = engine.clone();
                    let stopped = stop.clone();
                    scan = Some(tokio::spawn(async move {
                        let Some(lease) = engine
                            .acquire(crate::lifetime::Demand::Passive)
                            .await
                            .filter(|lease| (lease.record.pid, lease.record.port) == pin)
                        else {
                            return;
                        };
                        tokio::select! {
                            () = stopped.cancelled() => {},
                            result = crate::queue::scan(&engine, &lease) => if let Err(error) = result {
                                eprintln!("Checking the engine's saved queues: {error}");
                            }
                        }
                    }));
                }
                if let Some(attachments) = &attachments {
                    let _ = attachments.send(None);
                }
                let mut chunks = response.bytes_stream();
                let mut line = Vec::new();
                loop {
                    let next = tokio::select! {
                        next = chunks.next() => next,
                        () = async {
                            match reasoning.next() {
                                Some(deadline) => tokio::time::sleep_until(deadline).await,
                                None => std::future::pending().await,
                            }
                        } => {
                            reasoning.advance();
                            thinking.send_replace(Arc::new(reasoning.visible()));
                            continue;
                        },
                        _ = states.changed() => break,
                        _ = sleep(Duration::from_secs(60)) => break,
                    };
                    let Some(Ok(chunk)) = next else {
                        break;
                    };
                    let Some(list) = refresh.upgrade().filter(|list| !list.is_closed()) else {
                        return;
                    };
                    for part in chunk.split_inclusive(|byte| *byte == b'\n') {
                        line.extend_from_slice(part);
                        if line.len() > 4 * 1024 * 1024 {
                            return;
                        }
                        if !line.ends_with(b"\n") {
                            continue;
                        }
                        if let Some(event) = line
                            .strip_prefix(b"data:")
                            .and_then(|data| serde_json::from_slice::<Event>(data).ok())
                        {
                            engine.catalogs.event(&event.kind, &event.data);
                            if event.kind == "session.deleted"
                                && let Some(attachments) = &attachments
                                && let Some(session) = event.data["sessionID"].as_str()
                            {
                                let _ = attachments.send(Some(session.to_owned()));
                            }
                            if crate::model_refresh::work_ended(&event.kind) {
                                let _ = models_changed.try_send(());
                            }
                            if crate::asks::note(&mut asks, &event.kind, &event.data) {
                                ask_updates.send_replace(std::sync::Arc::new(asks.clone()));
                                crate::asks::checkpoint(&engine, pin, &asks).await;
                            }
                            if changes_inventory(&event.kind) {
                                inventory_changed
                                    .send_modify(|revision| *revision = revision.wrapping_add(1));
                                let _ = list.try_send(());
                            }
                            if let Some(session) = event.data["sessionID"]
                                .as_str()
                                .or_else(|| event.data["info"]["id"].as_str())
                                .filter(|id| crate::sessions::valid_id(id))
                            {
                                if reasoning.hear(session, &event.kind) {
                                    thinking.send_replace(Arc::new(reasoning.visible()));
                                }
                                queues.hear(
                                    &engine,
                                    session,
                                    &event.kind,
                                    &event.data,
                                    stop.clone(),
                                );
                            }
                        }
                        line.clear();
                    }
                }
            }
            sleep(Duration::from_millis(250)).await;
        }
    })
}

fn changes_inventory(kind: &str) -> bool {
    matches!(
        kind,
        "session.created"
            | "session.deleted"
            | "session.renamed"
            | "session.updated"
            | "session.metadata.updated"
            | "session.moved"
            | "session.viewed"
            | "session.revert.staged"
            | "session.revert.cleared"
            | "session.revert.committed"
            | "session.shell.started"
            | "session.shell.ended"
            | "session.idle"
            | "session.execution.started"
            | "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted"
    )
}
