//! A finish held until what its session left running is over (a build, a subagent; not a service
//! it left up): told nothing meanwhile, and looked at again as that ends, as it stops counting at
//! its limit, or every little while, in case its end wasn't heard.

use std::time::Duration;

use serde_json::Value;

use super::{
    Watcher,
    background::{Running, WorkOrder},
    is_archived, started_by,
    told::{ENGINE, Waited},
};

/// How long after what it left running ended a session is looked at again: its agent is woken by
/// the end, and a turn of its own begins within this; else its word goes as it stands.
pub const SETTLE: Duration = Duration::from_secs(10);
/// How often a finish that waits looks whether that's over, in case its end wasn't heard.
pub const WAITING_LOOK: Duration = Duration::from_secs(10 * 60);
/// The first of those looks, sooner: a server just started may not listen yet.
pub const FIRST_LOOK: Duration = Duration::from_secs(60);
/// How soon one that couldn't be looked at is looked at again.
pub const AGAIN: Duration = Duration::from_secs(60);

/// Whether what a session left running holds its finish: any of it but its services, or, for a turn
/// that answered a question, only what began since that was asked.
pub fn holds(still: Option<&Running>, asked: Option<u64>, order: Option<&WorkOrder>) -> bool {
    let Some(work) = still.and_then(Running::awaited) else {
        return false;
    };
    asked.is_none_or(|asked| work.started_since(asked, order))
}

impl Watcher {
    /// Held: told nothing until what it left running is over, however long that takes.
    pub(super) fn wait_for(
        &self,
        session: &str,
        since: u64,
        asked: Option<u64>,
        order: Option<WorkOrder>,
    ) {
        let was = self.kept().waited(session);
        let same = was
            .as_ref()
            .is_some_and(|was| was.asked == asked && was.asked_order == order);
        if !same {
            let at = was.as_ref().map_or(since, |was| was.at);
            let waited = Waited {
                from: ENGINE.into(),
                at,
                asked,
                asked_order: order,
            };
            self.kept().wait(session, &waited);
        }
        self.look_while_waiting(
            session,
            if was.is_some() {
                WAITING_LOOK
            } else {
                FIRST_LOOK
            },
        );
    }

    pub(super) fn look_while_waiting(&self, session: &str, after: Duration) {
        if self.inner.over()
            || !self
                .inner
                .looking
                .lock()
                .expect("looking")
                .insert(session.to_owned())
        {
            return;
        }
        let (watcher, session) = (self.clone(), session.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            watcher
                .inner
                .looking
                .lock()
                .expect("looking")
                .remove(&session);
            let waited = watcher
                .kept()
                .waited(&session)
                .filter(|waited| waited.from == ENGINE);
            let Some(waited) = waited.filter(|_| !watcher.inner.over()) else {
                return;
            };
            let folder = watcher.folder_of(&session).await;
            let still = watcher.inner.background.of(&session, &folder).await;
            if !holds(still.as_ref(), waited.asked, waited.asked_order.as_ref()) {
                let _ = watcher.settle(&session).await;
            }
            if watcher
                .kept()
                .waited(&session)
                .is_some_and(|waited| waited.from == ENGINE)
            {
                watcher.look_while_waiting(&session, WAITING_LOOK);
            }
        });
    }

    async fn folder_of(&self, session: &str) -> String {
        let Some(engine) = self.work().list.engine().running_bot_engine().await else {
            return String::new();
        };
        let data = Self::session(&engine, session).await.unwrap_or_default();
        super::lines::folder_of(&data).to_owned()
    }

    /// One whose finish waits, looked at now: gone, at work again (its end comes back here), or
    /// told as its finish would be. An error where it couldn't be read.
    pub(super) async fn settle(&self, id: &str) -> Result<(), ()> {
        if self.inner.over()
            || self
                .kept()
                .waited(id)
                .is_none_or(|waited| waited.from != ENGINE)
        {
            return Ok(());
        }
        let engine = self
            .work()
            .list
            .engine()
            .running_bot_engine()
            .await
            .ok_or(())?;
        // (Gone, or moved away: nothing to wait for.)
        let Some(session) = Self::session(&engine, id).await else {
            self.kept().let_go_of(id);
            return Ok(());
        };
        let Some(starter) = started_by(&session).map(str::to_owned) else {
            self.kept().let_go_of(id);
            return Ok(());
        };
        if Self::at_work(&engine, id).await {
            return Ok(());
        }
        self.finish(&engine, &session, &starter, None).await;
        Ok(())
    }

    fn look_again(&self, session: String, after: Duration) {
        let watcher = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            watcher
                .inner
                .settling
                .lock()
                .expect("settling")
                .remove(&session);
            // (Couldn't be read now: again in a while, or a command that never ends would never be told.)
            if watcher.settle(&session).await.is_err() && !watcher.inner.over() {
                watcher.look_again(session, AGAIN);
            }
        });
    }

    /// What its sessions left running ended, as an event says: those it held looked at again in a
    /// moment; a bot's session no one is told of shown to it finishing, once it's really done.
    pub(super) fn background_ended(&self) {
        for (id, waited) in self.kept().all_waited() {
            let still = self.inner.background.known(&id);
            if waited.from != ENGINE
                || holds(still.as_ref(), waited.asked, waited.asked_order.as_ref())
            {
                continue;
            }
            if self
                .inner
                .settling
                .lock()
                .expect("settling")
                .insert(id.clone())
            {
                self.look_again(id, SETTLE);
            }
        }
        let done: Vec<(String, String)> = {
            let mut later = self.inner.mark_later.lock().expect("later");
            let done: Vec<String> = later
                .keys()
                .filter(|id| {
                    self.inner
                        .background
                        .known(id)
                        .and_then(|work| work.awaited())
                        .is_none()
                })
                .cloned()
                .collect();
            done.into_iter()
                .filter_map(|id| later.remove_entry(&id))
                .collect()
        };
        let stopped = self.kept().stopped();
        for (id, bot) in done.into_iter().filter(|(id, _)| !stopped.contains_key(id)) {
            self.work().records.note_oversight(&bot, &id, "finished");
        }
    }

    /// A bot's session that finished quietly (no one asked to hear of it): shown to its bot finishing
    /// now, or once what it left running is over.
    pub(super) async fn finished_quietly(&self, session: &Value, bot: &str) {
        let id = session["id"].as_str().unwrap_or_default();
        if is_archived(session) || self.kept().stopped().contains_key(id) {
            return;
        }
        let still = self
            .inner
            .background
            .of(id, super::lines::folder_of(session))
            .await;
        if still.and_then(|work| work.awaited()).is_some() {
            self.inner
                .mark_later
                .lock()
                .expect("later")
                .insert(id.to_owned(), bot.to_owned());
        } else {
            self.work().records.note_oversight(bot, id, "finished");
        }
    }
}
