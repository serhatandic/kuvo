//! Settle work that was active before categories were migrated to statuses.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

const BACKGROUND_LIFE: u64 = 6 * 3_600_000;

impl Writer {
    pub(super) async fn settle_migration(&self, now: i64) -> Result<bool, Error> {
        let model = self.store.root.clone();
        let pending = tokio::task::spawn_blocking(move || {
            migration::migrate(&model)?;
            migration::read_started(&model)
        })
        .await
        .map_err(io::Error::other)??;
        if pending.is_empty() {
            return Ok(true);
        }

        let Ok(mut working) = self.engine.work_queue_active_sessions().await else {
            return Ok(false);
        };
        let guard = self.lock().await.map_err(hand::hand_error)?;
        let here = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            working.extend(background(&here.store, now));
            here.settle_started(&working, now)
        })
        .await
        .map_err(io::Error::other)??;
        Ok(true)
    }

    fn settle_started(&self, working: &BTreeSet<String>, now: i64) -> Result<(), Error> {
        let pending = migration::read_started(&self.store.root)?;
        if pending.is_empty() {
            return Ok(());
        }
        let (all, compaction) = self.changes(now).map_err(hand::hand_error)?;
        let mut docs = BTreeMap::new();
        let mut order = place(&all, None, "in-progress", false);
        for item in all.iter().filter(|item| {
            pending.contains(&item.id)
                && item.removed.is_none()
                && matches!(item.status.as_str(), "todo" | "backlog" | "waiting")
                && item.sessions.iter().any(|link| {
                    link["session"]
                        .as_str()
                        .is_some_and(|id| working.contains(id))
                })
        }) {
            let book = self
                .document(&mut docs, &item.owner, now)
                .map_err(hand::hand_error)?;
            let row = book
                .items
                .iter_mut()
                .find(|row| row["id"] == item.id)
                .expect("loaded queue item");
            row["status"] = "in-progress".into();
            row["order"] = order.into();
            order += 1.0;
            let who = json!({"source":"kuvo","name":"Kuvo Code"});
            append_event(
                row,
                json!({"at":now,"by":who,"kind":"moved","from":item.status,"to":"in-progress"}),
            );
        }
        for book in docs.into_values() {
            book.save(&self.store)?;
        }
        compaction.finish(self, now).map_err(hand::hand_error)?;
        migration::forget_started(&self.store.root)
    }
}

fn background(store: &Store, now: i64) -> BTreeSet<String> {
    let root = store.root.parent().expect("Work Queue under run");
    let path = root.join("work").join("background.json");
    let Some(places) = files::read(&path)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Fields>(&bytes).ok())
    else {
        return BTreeSet::new();
    };
    let mut active = BTreeSet::new();
    for place in places.values() {
        let Some(sessions) = place["sessions"].as_object() else {
            continue;
        };
        for (id, work) in sessions {
            let running = [
                ("commands", "started"),
                ("subagents", "since"),
                ("tasks", "since"),
            ]
            .into_iter()
            .any(|(field, moment)| {
                store::array(work.get(field)).iter().any(|entry| {
                    entry[moment]
                        .as_i64()
                        .is_some_and(|at| now.saturating_sub(at) < BACKGROUND_LIFE as i64)
                })
            });
            if running {
                active.insert(id.clone());
            }
        }
    }
    active
}
