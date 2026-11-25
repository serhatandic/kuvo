//! Counts derived from the same local snapshot as the session list.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use kuvo_protocol::routes::{get_kuvo_waiting as read, get_kuvo_waiting_events as events};
use kuvo_server::{Access, JsonLine, Registry, RouteReply};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{Engine, LocalSessions, session_list::Snapshot};

/// Current asking activity reported by peers. The work runtime filters away
/// computers and superseded copies before publishing this snapshot.
#[derive(Clone, PartialEq, Eq)]
pub struct PeerWaiting {
    pub session: String,
    pub line: Vec<String>,
    pub bot: Option<String>,
}

impl Engine {
    pub fn with_peer_waiting(mut self, activity: watch::Receiver<Arc<Vec<PeerWaiting>>>) -> Self {
        self.peer_waiting = Some(activity);
        self
    }
}

#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(rename = "title")]
    _title: String,
    status: Option<String>,
    category: Option<String>,
    #[serde(default)]
    removed: Value,
    #[serde(default)]
    created: Value,
    #[serde(default)]
    updated: Value,
    #[serde(default)]
    sessions: Vec<Linked>,
}

#[derive(Deserialize)]
struct Linked {
    session: String,
}

impl Item {
    fn status(&self) -> &str {
        self.status
            .as_deref()
            .filter(|status| !status.is_empty())
            .unwrap_or(match self.category.as_deref() {
                Some("done") => "done",
                Some("cat_waiting-on-you") => "needs-you",
                _ => "todo",
            })
    }

    fn updated(&self) -> f64 {
        self.updated["at"]
            .as_f64()
            .or_else(|| self.created["at"].as_f64())
            .unwrap_or_default()
    }
}

fn prefixed_id(
    value: &str,
    prefix: &str,
    size: std::ops::RangeInclusive<usize>,
    valid: impl Fn(u8) -> bool,
) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|rest| size.contains(&rest.len()) && rest.bytes().all(valid))
}

fn queue_owner(owner: &str) -> bool {
    owner == "usr_you"
        || prefixed_id(owner, "ses_", 1..=60, |ch| ch.is_ascii_alphanumeric())
        || prefixed_id(owner, "grp_", 1..=60, |ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit()
        })
}

async fn queue_count(folder: &Path, list: Option<&Snapshot>) -> usize {
    let mut items = BTreeMap::<String, Item>::new();
    let Ok(mut files) = tokio::fs::read_dir(folder).await else {
        return 0;
    };
    let mut paths = Vec::new();
    while let Ok(Some(file)) = files.next_entry().await {
        if file
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .is_some_and(queue_owner)
        {
            paths.push(file.path());
        }
    }
    paths.sort();
    for path in paths {
        let Ok(bytes) = tokio::fs::read(path).await else {
            continue;
        };
        let Ok(stored) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let rows = stored["items"].as_array().into_iter().flatten();
        let mut seen = std::collections::BTreeSet::new();
        for item in rows.filter_map(|row| serde_json::from_value::<Item>(row.clone()).ok()) {
            if !prefixed_id(&item.id, "wqi_", 20..=20, |ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-')
            }) || !seen.insert(item.id.clone())
            {
                continue;
            }
            if items
                .get(&item.id)
                .is_none_or(|old| old.updated() <= item.updated())
            {
                items.insert(item.id.clone(), item);
            }
        }
    }
    items
        .values()
        .filter(|item| {
            let status = item.status();
            let open = !matches!(status, "done" | "canceled") && !item.removed["at"].is_number();
            open && (status == "needs-you"
                || list.is_some_and(|list| {
                    item.sessions.iter().any(|linked| {
                        prefixed_id(&linked.session, "ses_", 1..=60, |ch| {
                            ch.is_ascii_alphanumeric()
                        }) && list.asking(&linked.session)
                    })
                }))
        })
        .count()
}

async fn bot_count(folder: &Path, list: Option<&Snapshot>, peers: &[PeerWaiting]) -> usize {
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
            && !session.trim().is_empty()
        {
            let session = session.trim();
            let local = list.is_some_and(|list| list.bot_waits(session, id));
            let away = peers.iter().any(|activity| {
                !list.is_some_and(|list| list.has_local(&activity.session))
                    && (activity.session == session
                        || activity.line.iter().any(|root| root == session)
                        || activity.bot.as_deref() == Some(id))
            });
            if local || away {
                asking += 1;
            }
        }
    }
    asking
}

async fn counts(
    home: &Path,
    list: Option<Arc<Snapshot>>,
    peers: &[PeerWaiting],
) -> read::Status200 {
    let list = list.as_deref();
    let bots = home.join("bots");
    let queues = home.join("run/work-queue/queues");
    let (bots, queue) = tokio::join!(bot_count(&bots, list, peers), queue_count(&queues, list));
    read::Status200 {
        sessions: list.map_or(0, Snapshot::waiting_sessions).into(),
        bots: bots.into(),
        queue: queue.into(),
        dock: None,
        pending_computers: None,
    }
}

impl Engine {
    async fn waiting_list(
        &self,
        list: &mut LocalSessions,
        cancel: &CancellationToken,
    ) -> Option<Arc<Snapshot>> {
        let snapshot = list.snapshot_now();
        if snapshot.is_none()
            && self
                .acquire(crate::lifetime::Demand::Passive)
                .await
                .is_some()
        {
            return list.current(cancel).await.ok();
        }
        snapshot
    }

    /// The Helper reads the same list and counts as the Waiting route, from one snapshot.
    pub async fn waiting_snapshot(
        &self,
        list: &mut LocalSessions,
        cancel: &CancellationToken,
    ) -> (Option<Value>, read::Status200) {
        let snapshot = self.waiting_list(list, cancel).await;
        let peers = self
            .peer_waiting
            .as_ref()
            .map(|activity| activity.borrow().clone())
            .unwrap_or_default();
        let record = snapshot
            .as_ref()
            .and_then(|snapshot| serde_json::to_value(snapshot.as_ref()).ok());
        (record, counts(&self.home, snapshot, &peers).await)
    }
}

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) {
    let session_list = list.clone();
    let here = engine.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let mut list = session_list.clone();
        let engine = here.clone();
        async move {
            let snapshot = engine.waiting_list(&mut list, &call.cancel).await;
            let peers = engine
                .peer_waiting
                .as_ref()
                .map(|activity| activity.borrow().clone())
                .unwrap_or_default();
            Ok(RouteReply::json::<200>(
                counts(&engine.home, snapshot, &peers).await,
            ))
        }
    });

    let home = engine.home.clone();
    let activity = engine.peer_waiting.clone();
    let here = engine.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let home = home.clone();
        let mut list = list.clone();
        let mut activity = activity.clone();
        let engine = here.clone();
        async move {
            let _ = engine.waiting_list(&mut list, &call.cancel).await;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut last = None;
                let mut next = tokio::time::interval(Duration::from_secs(1));
                next.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    let snapshot = list.snapshot_now();
                    let peers = activity.as_ref().map(|activity| activity.borrow().clone()).unwrap_or_default();
                    let current = counts(&home, snapshot, &peers).await;
                    if last.as_ref() != Some(&current) {
                        yield JsonLine::new(current.clone())?;
                        last = Some(current);
                    }
                    tokio::select! {
                        changed = list.changed() => if !changed { break; },
                        changed = async {
                            match &mut activity {
                                Some(activity) => activity.changed().await,
                                None => std::future::pending().await,
                            }
                        } => if changed.is_err() { activity = None; },
                        () = call.cancel.cancelled() => break,
                        _ = next.tick() => {},
                    }
                }
            }))
        }
    });
}
