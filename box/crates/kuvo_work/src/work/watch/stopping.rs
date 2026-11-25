//! Stopping someone's work for the user (Stop on its row, Stop All), rather than for its lead
//! (stop_session): what it left running and its wake-ups end with its turn, and its lead is told
//! once that the user stopped it, if it still waited on it, so it never waits on a session that
//! won't finish now (PROTOCOL.md, "Work › Background work › Stopping"). Nothing more comes from it
//! until it's written to again.

use std::{collections::HashSet, time::Duration};

use http::{Method, StatusCode};
use kuvo_engine::{BotEngine, WorkStopped, WorkStops};
use kuvo_server::Refusal;
use serde_json::Value;

use super::{
    background::Running,
    catchup::{asks_in, ids_of, work_sessions},
    encoded, is_archived, started_by,
    told::Kept,
    wakes_of, works_for,
};
use crate::{bots::here::BotHere, work::Work};

const WAIT: Duration = Duration::from_secs(10);

fn session_id(session: &str) -> bool {
    (1..=100).contains(&session.len())
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// The sessions at work in the engine now.
async fn working_now(engine: &BotEngine) -> HashSet<String> {
    let active = engine.call(Method::GET, "api/session/active", None, WAIT);
    active
        .await
        .map(|active| ids_of(&active))
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Whether a session (or one of its subagents) asks something now.
async fn asking_now(engine: &BotEngine, session: &str) -> bool {
    let (permissions, questions, _) = asks_in(engine, session).await;
    !permissions.is_empty() || !questions.is_empty()
}

/// A command a session left running ended: its process group signalled, as Stop on its row does,
/// else the engine asked to end it.
async fn end_command(engine: &BotEngine, job: &Value, folder: &str) {
    #[cfg(unix)]
    if let Some(pid) = job["pid"].as_i64().filter(|pid| *pid > 1)
        && let Ok(group) = i32::try_from(pid)
        // SAFETY: a signal to the process group the engine says leads this command.
        && unsafe { libc::kill(-group, libc::SIGTERM) } == 0
    {
        return;
    }
    let path = format!(
        "api/shell/{}",
        encoded(job["id"].as_str().unwrap_or_default())
    );
    let _ = match folder {
        "" => engine.call(Method::DELETE, &path, None, WAIT).await,
        folder => engine.call_in(Method::DELETE, &path, folder, WAIT).await,
    };
}

impl Work {
    pub(super) fn kept(&self) -> Kept {
        Kept::new(&self.home.join("run"))
    }

    fn session_wakes(&self, session: &str) -> std::path::PathBuf {
        self.home.join("wakes").join(session)
    }

    /// Whether a session has wake-ups or watches of its own waiting.
    pub fn has_wake_ups(&self, session: &str) -> bool {
        session_id(session)
            && ["wakes", "watches"].iter().any(|kind| {
                std::fs::read_dir(self.session_wakes(session).join(kind))
                    .is_ok_and(|mut names| names.next().is_some())
            })
    }

    // The wake-ups and watches a session keeps of its own (not a bot's, whose are its bot's).
    fn end_wake_ups(&self, session: &str) {
        if session_id(session) {
            for kind in ["wakes", "watches"] {
                let _ = std::fs::remove_dir_all(self.session_wakes(session).join(kind));
            }
        }
    }

    /// The sessions at work now: in a turn, or with something they left running.
    pub(super) async fn at_work_now(
        &self,
        engine: &BotEngine,
    ) -> (HashSet<String>, HashSet<String>) {
        let turns = working_now(engine).await;
        let mut working = turns.clone();
        let file = self.home.join("run/work/background.json");
        let kept = kuvo_store::files::text_of(&file)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_default();
        let places = kept
            .as_object()
            .into_iter()
            .flat_map(|places| places.values());
        for place in places {
            let sessions = place["sessions"].as_object().into_iter().flatten();
            for (id, work) in sessions {
                let work = serde_json::from_value::<Running>(work.clone()).ok();
                if work.is_some_and(|work| work.counting(super::told::now()).is_some()) {
                    working.insert(id.clone());
                }
            }
        }
        (working, turns)
    }

    // What it left running, ended: its commands and its subagents.
    async fn end_background(&self, engine: &BotEngine, session: &str) {
        // (A coding agents' conversation: through its agent.)
        if engine.stop_agent_background(session).await.is_some() {
            return;
        }
        let path = format!("api/session/{}/background", encoded(session));
        if engine.call(Method::DELETE, &path, None, WAIT).await.is_ok() {
            return;
        }
        let file = self.home.join("run/work/background.json");
        let kept = kuvo_store::files::text_of(&file)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_default();
        let work: Running =
            serde_json::from_value(kept["engine"]["sessions"][session].clone()).unwrap_or_default();
        for command in &work.commands {
            // (The engine knows a command only in its own folder.)
            let folder = command.folder.as_deref().unwrap_or_default();
            let path = format!("api/shell/{}", encoded(&command.id));
            let job = match folder {
                "" => engine.call(Method::GET, &path, None, WAIT).await,
                folder => engine.call_in(Method::GET, &path, folder, WAIT).await,
            };
            if let Some(job) = job.ok().filter(|job| job["status"] == "running") {
                end_command(engine, &job, folder).await;
            }
        }
        for helper in &work.subagents {
            let path = format!("api/session/{}/interrupt", encoded(&helper.id));
            let _ = engine.call(Method::POST, &path, None, WAIT).await;
        }
    }

    /// Stop for a session someone started: its turn (`holds` as Stop's), what it left running, and
    /// its wake-ups and watches. Noted first: the end of a command stopped here isn't news.
    pub async fn stop_work(
        &self,
        engine: &BotEngine,
        session: &str,
        holds: bool,
        items: Vec<String>,
    ) -> Result<Value, Refusal> {
        self.kept().note_stopped(session);
        self.end_wake_ups(session);
        let state = self.list.engine().stop_work(session, holds, items).await?;
        self.end_background(engine, session).await;
        Ok(state)
    }

    /// Who's to hear that the user stopped `data`: its starter, when it follows its finishes and
    /// still waits on it (a turn under way, its finish held or not sent yet, a wake-up of its own,
    /// or a question); none for one stopped already and not set going since. `known`: whether it's
    /// in a turn and whether it asks, as found already. Asked before it's stopped.
    async fn lead_waiting_on(
        &self,
        engine: &BotEngine,
        data: &Value,
        known: Option<(bool, bool)>,
    ) -> Option<String> {
        let id = data["id"].as_str().unwrap_or_default();
        let starter = started_by(data)?;
        let kept = self.kept();
        if !wakes_of(data).contains(&"finished")
            || is_archived(data)
            || kept.stopped().contains_key(id)
        {
            return None;
        }
        let told = kept.told().unwrap_or_default();
        let held = [&told["waiting"][id], &told["held"][id]]
            .iter()
            .any(|kept| super::truthy(kept));
        let address = format!("(address {id})");
        let to_be_told = kept
            .waiting_for(starter)
            .is_some_and(|waiting| waiting.lines.iter().any(|line| line.contains(&address)));
        let waits = match known {
            Some((turn, asking)) => turn || asking,
            None => working_now(engine).await.contains(id) || asking_now(engine, id).await,
        };
        let early = held || to_be_told || self.has_wake_ups(id);
        (early || waits).then(|| starter.to_owned())
    }

    /// Stop on a conversation someone started: as `stop_work`, its lead told the user stopped it.
    /// None for one that isn't someone's work.
    async fn stop_conversation(
        &self,
        session: &str,
        items: Vec<String>,
    ) -> Option<Result<Value, Refusal>> {
        let engine = self.list.engine().running_bot_engine().await?;
        let path = format!("api/session/{}", encoded(session));
        let data = engine.call(Method::GET, &path, None, WAIT).await.ok()?;
        started_by(&data)?;
        let lead = self.lead_waiting_on(&engine, &data, None).await;
        let state = self.stop_work(&engine, session, true, items).await;
        if state.is_ok() {
            if let Some(lead) = lead {
                self.tell_starters(&[(lead, self.stopped_line(&data))]);
            }
            // (Stopped at rest, its row changes all the same: a bot's work bar shows it stopped.)
            if let Some(bot) = works_for(&data) {
                self.records.note_changed(bot);
            }
        }
        Some(state)
    }

    /// A bot's work stopped (Stop All): each session of it at work, asking or waiting on a timer,
    /// and the bot's own turn, since it would only start them again. Each lead hears once, in one
    /// message, which you stopped; the bot itself as it goes on, its queue held until then. How
    /// many were stopped.
    pub async fn stop_bot_work(&self, bot: &str) -> Result<usize, Refusal> {
        let Some(here) = BotHere::read(&self.bots, bot) else {
            return Err(Refusal::new(
                StatusCode::NOT_FOUND,
                "There's no such bot here.",
            ));
        };
        let refused = || Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.");
        let engine = self
            .list
            .engine()
            .running_bot_engine()
            .await
            .ok_or_else(refused)?;
        let (own, engine_here) = (here.session.as_str(), self.list.engine());
        // A due reset mustn't start the bot while its sessions are stopped.
        let _ = engine_here.pause_for_user(own, true).await;
        let all = work_sessions(&engine).await.unwrap_or_default();
        let work = all.iter().filter(|data| works_for(data) == Some(bot));
        let (working, turns) = self.at_work_now(&engine).await;
        let (mut stopped, mut told) = (0, Vec::new());
        for data in work {
            let id = data["id"].as_str().unwrap_or_default();
            let turn = turns.contains(id);
            let asking = !turn && asking_now(&engine, id).await;
            if !working.contains(id) && !self.has_wake_ups(id) && !asking {
                continue;
            }
            let lead = self
                .lead_waiting_on(&engine, data, Some((turn, asking)))
                .await;
            // (What waits is held only where a turn stopped: with none running, its next message
            // just goes.)
            if self
                .stop_work(&engine, id, turn || asking, Vec::new())
                .await
                .is_err()
            {
                continue;
            }
            stopped += 1;
            if let Some(lead) = lead {
                told.push((lead, self.stopped_line(data)));
            }
        }
        if told.iter().any(|(starter, _)| starter == own) {
            let _ = engine_here.pause_for_user(own, false).await;
        }
        self.forget_wakes(own);
        if working.contains(own) {
            let _ = self
                .stop_work(&engine, own, turns.contains(own), Vec::new())
                .await;
        }
        // Stopped leads hear from their own lead; the bot still hears as it goes on.
        let marked = self.kept().stopped();
        told.retain(|(starter, _)| starter == own || !marked.contains_key(starter));
        self.tell_starters(&told);
        self.records.note_changed(bot);
        Ok(stopped)
    }
}

/// Stop on a row, as `Work::stop_conversation`.
pub struct Stops(pub Work);

impl WorkStops for Stops {
    fn stop<'a>(&'a self, session: &'a str, items: Vec<String>) -> WorkStopped<'a> {
        Box::pin(self.0.stop_conversation(session, items))
    }
}
