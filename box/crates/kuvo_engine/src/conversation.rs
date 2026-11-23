//! Shared treatment of stored and streaming conversation summaries.

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
    stop: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    use futures_util::StreamExt;
    use http::Method;
    use tokio::time::{Duration, sleep};

    tokio::spawn(async move {
        let mut states = engine.states();
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
            let request = engine
                .upstream
                .request(&lease.record, Method::GET, "api/event");
            let connected = tokio::time::timeout(crate::api::WAIT, request.send()).await;
            if let Ok(Ok(response)) = connected
                && response.status().is_success()
            {
                let mut chunks = response.bytes_stream();
                let mut line = Vec::new();
                loop {
                    let next = tokio::select! {
                        next = chunks.next() => next,
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
                            if event.kind.starts_with("session.")
                                && !event.kind.starts_with("session.inbox.")
                            {
                                let _ = list.try_send(());
                            }
                            if let Some(session) = event.data["sessionID"]
                                .as_str()
                                .or_else(|| event.data["info"]["id"].as_str())
                                .filter(|id| crate::sessions::valid_id(id))
                            {
                                crate::queue::engine_event(
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
