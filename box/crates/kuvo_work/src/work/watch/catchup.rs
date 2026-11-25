//! What happened while the box was down (or while the engine couldn't be heard): each followed
//! session that finished, failed or asked since the box last heard of any, and hasn't been told,
//! told as it starts. Only once it could look at them all: until then, looked at again a minute on,
//! from the same time. A session it couldn't read doesn't hold back the others: they're told, and
//! it's looked at again (a few times, on a few starts). And answers given after their tool stopped
//! waiting, told to their conversations as the box starts and every few minutes.

use std::time::Duration;

use http::Method;
use kuvo_engine::BotEngine;
use serde_json::{Value, json};

use super::{
    Watcher,
    held::{AGAIN, FIRST_LOOK},
    is_archived, started_by,
    told::{ENGINE, Note, now},
    turns::{self, BeganBy},
    wakes_of,
};

const WAIT: Duration = Duration::from_secs(10);
// How many times a catch-up that couldn't read some of the sessions is tried again, before they're
// let go of until the box starts the next time; and on how many starts in a row, before for good.
const TRIES: u32 = 10;
const UNREAD_STARTS: u64 = 3;
// How often answers given late that couldn't be told yet are tried again.
const LATE_AGAIN: Duration = Duration::from_secs(300);

pub(super) fn ids_of(active: &Value) -> Vec<String> {
    match active {
        Value::Array(ids) => ids
            .iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect(),
        Value::Object(ids) => ids.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

impl Watcher {
    /// As the box starts: what happened while it was down, and late answers, told; tried again as
    /// it couldn't be.
    pub(super) fn catch_up(&self, before: Option<Value>) {
        let watcher = self.clone();
        tokio::spawn(async move {
            let mut tries = 0;
            loop {
                let caught = watcher.catch_up_once(before.as_ref(), &mut tries).await;
                for (id, waited) in watcher.kept().all_waited() {
                    if waited.from == ENGINE {
                        watcher.look_while_waiting(&id, FIRST_LOOK);
                    }
                }
                if caught || watcher.inner.over() {
                    break;
                }
                tokio::time::sleep(AGAIN).await;
            }
        });
        let late = self.clone();
        tokio::spawn(async move {
            loop {
                late.tell_late_answers().await;
                tokio::time::sleep(LATE_AGAIN).await;
                if late.inner.over() {
                    return;
                }
            }
        });
    }

    /// Answered after their tool stopped waiting, and not told yet: told now.
    async fn tell_late_answers(&self) {
        let asks = self.work().kept_asks();
        let untold = asks.iter().filter(|each| {
            each["answered"].is_object() && each["late"] == true && each["toldLate"] != true
        });
        for ask in untold.filter_map(|ask| ask["id"].as_str()) {
            self.work().tell_late(ask).await;
        }
    }

    /// One look; whether it's done (nothing left to look at again).
    async fn catch_up_once(&self, before: Option<&Value>, tries: &mut u32) -> bool {
        let Some(before) = before else {
            self.inner.caught();
            self.inner.background.start().await;
            return true;
        };
        let Some(engine) = self.work().list.engine().running_bot_engine().await else {
            return false;
        };
        let since = before["since"].as_u64().unwrap_or(0);
        // (What changes from here on is looked at the next time, if it isn't heard of as it happens.)
        let looked = now();
        let (Some(listed), Ok(active)) = (
            work_sessions(&engine).await,
            engine
                .call(Method::GET, "api/session/active", None, WAIT)
                .await,
        ) else {
            return false;
        };
        self.inner.background.start().await;
        let working = ids_of(&active);
        let ours = |key: &str| -> Vec<String> {
            let each = before[key].as_object().into_iter().flatten();
            each.filter(|(_, each)| each["from"] == ENGINE)
                .map(|(id, _)| id.clone())
                .collect()
        };
        let was_held = [ours("held"), ours("waiting")].concat();
        let updated = |data: &Value| data["time"]["updated"].as_f64().map_or(0, |at| at as u64);
        let followed: Vec<&Value> = listed
            .iter()
            .filter(|data| started_by(data).is_some() && !wakes_of(data).is_empty())
            .filter(|data| {
                updated(data) > since || was_held.iter().any(|id| data["id"] == id.as_str())
            })
            .collect();
        // One waited for that isn't listed now (deleted, moved, put away): looked at, and let go of.
        for id in ours("waiting") {
            if !listed.iter().any(|data| data["id"] == id.as_str()) {
                let _ = self.settle(&id).await;
            }
        }
        let mut unread = Vec::new();
        for data in &followed {
            if self
                .tell_what_happened(&engine, data, &working)
                .await
                .is_err()
            {
                unread.push(*data);
            }
        }
        if !unread.is_empty() {
            *tries += 1;
            if *tries < TRIES {
                return false;
            }
        }
        // Let go of for now: from just before them, looked at again as the box starts the next time;
        // one it couldn't read on a few starts in a row, for good.
        let starts = self
            .kept()
            .told()
            .map(|told| told["unread"].clone())
            .unwrap_or_default();
        let count = |data: &Value| {
            starts[data["id"].as_str().unwrap_or_default()]
                .as_u64()
                .unwrap_or(0)
        };
        let held: Vec<&Value> = unread
            .into_iter()
            .filter(|data| count(data) + 1 < UNREAD_STARTS)
            .collect();
        if held.is_empty() {
            self.inner.caught();
        }
        let until = held
            .iter()
            .map(|data| updated(data).saturating_sub(1))
            .fold(looked, u64::min);
        let holding = self.inner.holding.lock().expect("holding").clone();
        self.kept().change_told(
            |told| {
                let mut counts = told["unread"].as_object().cloned().unwrap_or_default();
                for data in &followed {
                    counts.shift_remove(data["id"].as_str().unwrap_or_default());
                }
                for data in &held {
                    counts.insert(
                        data["id"].as_str().unwrap_or_default().to_owned(),
                        json!(count(data) + 1),
                    );
                }
                let told = told.as_object_mut().expect("told");
                if counts.is_empty() {
                    told.shift_remove("unread");
                } else {
                    told.insert("unread".into(), json!(counts));
                }
                // Held here as the box stopped, and looked at now: told, nothing to tell, or held again.
                if let Some(kept) = told.get_mut("held").and_then(Value::as_object_mut) {
                    for id in &was_held {
                        let again = holding.contains(id)
                            || working.contains(id)
                            || held.iter().any(|data| data["id"] == id.as_str());
                        if !again {
                            kept.shift_remove(id);
                        }
                    }
                    if kept.is_empty() {
                        told.shift_remove("held");
                    }
                }
            },
            Some(ENGINE),
            until,
        );
        held.is_empty()
    }

    /// What one followed session did that its starter hasn't been told; an error where it can't be
    /// read.
    async fn tell_what_happened(
        &self,
        engine: &BotEngine,
        data: &Value,
        working: &[String],
    ) -> Result<(), ()> {
        let (id, starter) = (
            data["id"].as_str().unwrap_or_default(),
            started_by(data).unwrap_or_default(),
        );
        let was = self.kept().session_told(id);
        let mut unread_asks = false;
        if wakes_of(data).contains(&"asks") {
            let (permissions, questions, whole) = asks_in(engine, id).await;
            unread_asks = !whole;
            for permission in permissions
                .iter()
                .filter(|each| !was.asks.iter().any(|ask| each["id"] == ask.as_str()))
            {
                let line = self.work().permission_line(data, permission);
                self.tell(starter, line, true);
                self.note_told(
                    id,
                    Note {
                        ask: permission["id"].as_str().map(str::to_owned),
                        ..Note::default()
                    },
                );
            }
            for question in questions
                .iter()
                .filter(|each| !was.asks.iter().any(|ask| each["id"] == ask.as_str()))
            {
                let title = data["title"]
                    .as_str()
                    .filter(|title| !title.is_empty())
                    .unwrap_or("New Session");
                let (text, ask) = (
                    question_text(question),
                    question["id"].as_str().unwrap_or_default(),
                );
                let line = format!(
                    "[Session asks] \"{title}\" (address {id}) asks: {text} (ask {ask}). Answer with answer_ask (answer), or tell the user in a line and carry on."
                );
                self.tell(starter, line, true);
                self.note_told(
                    id,
                    Note {
                        ask: Some(ask.to_owned()),
                        ..Note::default()
                    },
                );
            }
        }
        self.tell_if_finished(engine, data, working).await?;
        // (Questions it couldn't read: it isn't past them, though it's told of the rest.)
        if unread_asks { Err(()) } else { Ok(()) }
    }

    async fn tell_if_finished(
        &self,
        engine: &BotEngine,
        data: &Value,
        working: &[String],
    ) -> Result<(), ()> {
        let (id, starter) = (
            data["id"].as_str().unwrap_or_default(),
            started_by(data).unwrap_or_default(),
        );
        if !wakes_of(data).contains(&"finished") || working.iter().any(|each| each == id) {
            return Ok(());
        }
        let was = self.kept().session_told(id);
        let last = turns::last_reply(engine, id).await.ok_or(())?;
        // Its finish was waiting as the box stopped: looked at as it would have been.
        if self
            .kept()
            .waited(id)
            .is_some_and(|waited| waited.from == ENGINE)
        {
            self.finish(engine, data, starter, Some(last)).await;
            return Ok(());
        }
        // (Its work's, past a summary after it, whose own run says how that went.)
        let outcome = turns::work_outcome(&last.whole, &data["outcome"]);
        let failed = outcome == "failed";
        let reply = last
            .id
            .clone()
            .filter(|reply| Some(reply) != was.reply.as_ref());
        // A turn that failed before it replied: told by its end, once.
        let before_told = was
            .at
            .is_some_and(|at| last.turn_at.unwrap_or(u64::MAX) <= at);
        let failed_bare = failed
            && reply.is_none()
            && last.turn.is_some()
            && last.turn != was.turn
            && !before_told;
        if reply.is_none() && !failed_bare {
            return Ok(());
        }
        let noted = |reply: Option<String>, turn: Option<String>| Note {
            reply,
            turn,
            ..Note::default()
        };
        // Told as it finished, its reply unread then: this one was there by then.
        let written = last
            .at
            .or(data["time"]["updated"].as_f64().map(|at| at as u64))
            .unwrap_or(u64::MAX);
        if reply.is_some() && was.at.is_some_and(|at| written <= at) {
            self.note_told(id, noted(reply, None));
            return Ok(());
        }
        // Stopped (by you, or cut short): nothing to tell, as while the box runs.
        if outcome == "interrupted" {
            self.note_told(id, noted(reply, None));
            return Ok(());
        }
        if !failed {
            self.finish(engine, data, starter, Some(last)).await;
            return Ok(());
        }
        if is_archived(data)
            || self.stays_stopped(id, Some(&last))
            || last.began.kind == BeganBy::You
        {
            self.note_told(id, noted(reply, last.turn.clone()));
            return Ok(());
        }
        let line = self.failed_line_of(engine, data, None).await;
        self.tell(starter, line, false);
        self.note_told(id, noted(reply, last.turn.clone()));
        Ok(())
    }
}

/// A question as its lead is told it: its one field's words, else its title.
pub(super) fn question_text(form: &Value) -> String {
    let shown = |field: &&Value| {
        field["hidden"] != true && field["when"].as_array().is_none_or(Vec::is_empty)
    };
    let fields: Vec<&Value> = form["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(shown)
        .collect();
    let one = match fields.as_slice() {
        [field] if field["type"] == "string" => Some(*field),
        _ => None,
    };
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let one = one.and_then(|field| text(&field["description"]).or_else(|| text(&field["title"])));
    one.or_else(|| text(&form["title"]))
        .unwrap_or_else(|| "a question".into())
}

/// Every session here that's someone's work (started by another conversation), whole: none when a
/// page can't be read.
pub(super) async fn work_sessions(engine: &BotEngine) -> Option<Vec<Value>> {
    work_sessions_as(engine, false).await
}

/// As `work_sessions`, those put away too where `archived`.
pub(super) async fn work_sessions_as(engine: &BotEngine, archived: bool) -> Option<Vec<Value>> {
    let theirs = |data: &Value| {
        data["id"].is_string()
            && data["parentID"].is_null()
            && (archived || !is_archived(data))
            && started_by(data).is_some()
    };
    let mut found = Vec::new();
    let mut query = Some("limit=200&order=desc&parentID=null".to_owned());
    for _ in 0..15 {
        let Some(asked) = query.take() else {
            break;
        };
        let path = format!("api/session?{asked}");
        // (None of the engine's while it isn't running.)
        let page = match engine.call_whole(Method::GET, &path, WAIT).await {
            Ok(page) => page,
            Err(_) if !engine.running() => break,
            Err(_) => return None,
        };
        let listed = page["data"].as_array().cloned()?;
        let next = page["cursor"]["next"].as_str().map(super::encoded);
        query = next
            .filter(|_| listed.len() == 200)
            .map(|next| format!("limit=200&parentID=null&cursor={next}"));
        found.extend(listed.into_iter().filter(theirs));
    }
    // (The coding agents' conversations too: someone's work as the engine's are.)
    let agents = engine
        .agent_sessions()
        .await
        .into_iter()
        .map(|(data, _)| data);
    found.extend(agents.filter(theirs));
    let created = |data: &Value| data["time"]["created"].as_f64().unwrap_or(0.0);
    found.sort_by(|a, b| created(b).total_cmp(&created(a)));
    Some(found)
}

/// What a session (or one of its subagents) asks now: its permissions and questions, and whether
/// its questions could all be read.
pub(super) async fn asks_in(engine: &BotEngine, session: &str) -> (Vec<Value>, Vec<Value>, bool) {
    let mut askers = vec![session.to_owned()];
    let mut at = 0;
    while at < askers.len() && at < 40 && !(at > 0 && askers.len() > 30) {
        let path = format!(
            "api/session?limit=50&order=desc&parentID={}",
            super::encoded(&askers[at])
        );
        let listed = engine
            .call(Method::GET, &path, None, WAIT)
            .await
            .unwrap_or_default();
        for child in listed.as_array().into_iter().flatten() {
            let id = child["id"].as_str().filter(|id| {
                child["parentID"] == askers[at].as_str() && !askers.iter().any(|each| each == id)
            });
            if let Some(id) = id {
                askers.push(id.to_owned());
            }
        }
        at += 1;
    }
    let (mut permissions, mut questions, mut whole) = (Vec::new(), Vec::new(), true);
    for asker in &askers {
        let at = format!("api/session/{}", super::encoded(asker));
        let listed = engine
            .call(Method::GET, &format!("{at}/permission"), None, WAIT)
            .await
            .unwrap_or_default();
        // (Each with who asks it: the session, or a subagent of it.)
        let asked = |mut each: Value| {
            each["asker"] = Value::from(asker.as_str());
            each
        };
        let listed = listed.as_array().cloned().unwrap_or_default();
        permissions.extend(
            listed
                .into_iter()
                .filter(|each| each["id"].is_string())
                .map(asked),
        );
        match engine
            .call(Method::GET, &format!("{at}/form"), None, WAIT)
            .await
        {
            Ok(Value::Array(forms)) => questions.extend(
                forms
                    .into_iter()
                    .filter(|each| each["id"].is_string())
                    .map(asked),
            ),
            _ => whole = false,
        }
    }
    (permissions, questions, whole)
}
