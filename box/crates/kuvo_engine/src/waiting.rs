//! Counts derived from the same local snapshot as the session list.

use std::{collections::BTreeMap, io, path::Path, sync::Arc, time::Duration};

use kuvo_protocol::routes::{get_kuvo_waiting as read, get_kuvo_waiting_events as events};
use kuvo_server::{Access, JsonLine, Registry, RouteReply};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Engine, LocalSessions, session_list::Snapshot, sessions::typed};

#[derive(Deserialize)]
struct Item {
    id: String,
    status: String,
    #[serde(default)]
    removed: Value,
    #[serde(default)]
    updated: Value,
    #[serde(default)]
    sessions: Vec<Linked>,
}

#[derive(Deserialize)]
struct Linked {
    session: String,
}

async fn queue_count(folder: &Path, list: Option<&Snapshot>) -> usize {
    let mut items = BTreeMap::<String, Item>::new();
    let Ok(mut files) = tokio::fs::read_dir(folder).await else {
        return 0;
    };
    while let Ok(Some(file)) = files.next_entry().await {
        if file.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(file.path()).await else {
            continue;
        };
        let Ok(stored) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let rows = stored["items"].as_array().into_iter().flatten();
        for item in rows.filter_map(|row| serde_json::from_value::<Item>(row.clone()).ok()) {
            let at = item.updated["at"].as_u64().unwrap_or_default();
            if items
                .get(&item.id)
                .is_none_or(|old| old.updated["at"].as_u64().unwrap_or_default() <= at)
            {
                items.insert(item.id.clone(), item);
            }
        }
    }
    items
        .values()
        .filter(|item| {
            let open = !matches!(item.status.as_str(), "done" | "canceled")
                && matches!(item.removed, Value::Null | Value::Bool(false));
            open && (item.status == "needs-you"
                || list.is_some_and(|list| {
                    item.sessions
                        .iter()
                        .any(|linked| list.asking(&linked.session))
                }))
        })
        .count()
}

async fn bot_count(folder: &Path, list: Option<&Snapshot>) -> usize {
    let Some(list) = list else {
        return 0;
    };
    let Ok(mut folders) = tokio::fs::read_dir(folder).await else {
        return 0;
    };
    let mut asking = 0;
    while let Ok(Some(bot)) = folders.next_entry().await {
        let name = bot.file_name();
        let Some(id) = name.to_str() else {
            continue;
        };
        if !id.strip_prefix("bot_").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|ch| ch.is_ascii_alphanumeric())
        }) {
            continue;
        }
        let stored = tokio::fs::read(bot.path().join("bot.json"))
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if let Some(stored) = stored
            && stored["kuvo"] == 1
            && stored["id"] == id
            && stored["name"].is_string()
            && let Ok(session) = tokio::fs::read_to_string(bot.path().join("session")).await
            && list.asking(session.trim())
        {
            asking += 1;
        }
    }
    asking
}

async fn counts(home: &Path, list: Option<Arc<Snapshot>>) -> Value {
    let list = list.as_deref();
    let bots = home.join("bots");
    let queues = home.join("run/work-queue/queues");
    let (bots, queue) = tokio::join!(bot_count(&bots, list), queue_count(&queues, list));
    json!({"sessions":list.map_or(0, Snapshot::waiting_sessions),"bots":bots,"queue":queue})
}

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) {
    let home = engine.home.clone();
    let session_list = list.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |_| {
        let home = home.clone();
        let list = session_list.snapshot_now();
        async move { Ok(RouteReply::json::<200>(typed(counts(&home, list).await)?)) }
    });

    let home = engine.home.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let home = home.clone();
        let mut list = list.clone();
        async move {
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut last = Value::Null;
                let mut next = tokio::time::interval(Duration::from_secs(1));
                next.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    let snapshot = list.snapshot_now();
                    let current = counts(&home, snapshot).await;
                    if current != last {
                        let value: events::Status200 = serde_json::from_value(current.clone()).map_err(io::Error::other)?;
                        yield JsonLine::new(value)?;
                        last = current;
                    }
                    tokio::select! {
                        changed = list.changed() => if !changed { break; },
                        () = call.cancel.cancelled() => break,
                        _ = next.tick() => {},
                    }
                }
            }))
        }
    });
}
