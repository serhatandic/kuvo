//! The tools a conversation runs its sessions with (PROTOCOL.md, "Work"), as this box answers them
//! for it: where each session it started stands (list_sessions), one stopped (stop_session), one
//! summarized to free up its context (compact_session). Every box answers them alike: here for one
//! of its own conversations, and for a lead on another computer through its calls (`elsewhere`).

use std::{collections::HashSet, time::Duration};

use http::Method;
use kuvo_engine::{BotEngine, NoteEdits};
use serde_json::{Map, Value, json};

use super::{
    catchup::{asks_in, ids_of, question_text, work_sessions},
    encoded,
    lines::{describe_permission, folder_of},
    started_by,
    turns::{last_turn_end, past_summaries, reply_in, turn_in, work_outcome},
    works_for,
};
use crate::{bots::here::BotHere, work::Work};

const WAIT: Duration = Duration::from_secs(10);

/// What a tool says back, and whether that's a refusal.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub text: String,
    pub refused: bool,
}

fn said(text: impl Into<String>) -> Said {
    Said {
        text: text.into(),
        refused: false,
    }
}

fn refused(text: impl Into<String>) -> Said {
    Said {
        text: text.into(),
        refused: true,
    }
}

fn title_of(data: &Value) -> &str {
    data["title"]
        .as_str()
        .filter(|title| !title.is_empty())
        .unwrap_or("New Session")
}

/// The conversations above a session, its starter's starter first (`kuvo.line`); one from before
/// lines, its starter.
pub fn line_of(data: &Value) -> Vec<&str> {
    let Some(line) = data["metadata"]["kuvo"]["line"].as_array() else {
        return started_by(data).into_iter().collect();
    };
    let sessions = line.iter().filter_map(Value::as_str).filter(|each| {
        each.strip_prefix("ses_").is_some_and(|rest| {
            (1..=60).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    });
    let sessions: Vec<&str> = sessions.collect();
    sessions[sessions.len().saturating_sub(20)..].to_vec()
}

/// The sessions under `root` (those it started, theirs, and so on), from `all`: up their starters
/// here, and their lines, for one whose line crosses computers.
pub fn under<'a>(root: &str, all: &'a [Value]) -> Vec<&'a Value> {
    let by_id = |id: &str| all.iter().find(|each| each["id"] == id);
    let is_under = |each: &Value| {
        let mut at = Some(each);
        for _ in 0..20 {
            let Some(data) = at else {
                return false;
            };
            if line_of(data).contains(&root) {
                return true;
            }
            let Some(above) = started_by(data) else {
                return false;
            };
            at = by_id(above);
        }
        false
    };
    all.iter().filter(|each| is_under(each)).collect()
}

/// Every session here that's someone's work, the latest first; none where a page can't be read.
pub async fn all_work(engine: &BotEngine) -> Option<Vec<Value>> {
    work_sessions(engine).await
}

/// The sessions at work in the engine now.
pub async fn working_now(engine: &BotEngine) -> HashSet<String> {
    let active = engine
        .call(Method::GET, "api/session/active", None, WAIT)
        .await;
    active
        .map(|active| ids_of(&active))
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// What a session (or one of its subagents) asks now, in words, with the ask's id.
async fn asking_now(engine: &BotEngine, session: &str) -> Option<(String, String)> {
    let (permissions, questions, _) = asks_in(engine, session).await;
    if let Some(permission) = permissions.first() {
        let id = permission["id"].as_str().unwrap_or_default().to_owned();
        return Some((id, describe_permission(permission)));
    }
    let question = questions.first()?;
    let id = question["id"].as_str().unwrap_or_default().to_owned();
    Some((id, format!("Asks: {}", question_text(question))))
}

async fn last_messages(engine: &BotEngine, session: &str, limit: u32) -> Vec<Value> {
    let path = format!(
        "api/session/{}/message?limit={limit}&order=desc",
        encoded(session)
    );
    let page = engine.call(Method::GET, &path, None, WAIT).await;
    page.ok()
        .and_then(|page| page.as_array().cloned())
        .unwrap_or_default()
}

/// How full a session's context is now, as the app's gauge reads it: what its last step sent and
/// got back; `summarized` where it was summarized since. None where it hasn't said.
async fn context_of(engine: &BotEngine, session: &str) -> Option<Value> {
    for message in last_messages(engine, session, 20).await {
        if message["type"] == "compaction" && message["status"] == "completed" {
            return Some(json!({ "summarized": true }));
        }
        let tokens = &message["tokens"];
        if message["type"] != "assistant" || !tokens.is_object() {
            continue;
        }
        let count = |value: &Value| value.as_u64().unwrap_or(0);
        let used = count(&tokens["input"])
            + count(&tokens["cache"]["read"])
            + count(&tokens["cache"]["write"])
            + count(&tokens["output"]);
        return Some(json!({ "used": used }));
    }
    None
}

/// What a session said in its latest turn of work, past a summary asked for after it.
fn reply_of_last_turn(page: &[Value]) -> Option<String> {
    let work = past_summaries(page);
    let (from, before) = turn_in(&work);
    reply_in(&work[from..before.unwrap_or(work.len())])
}

impl Work {
    /// The bot a conversation is, by its session.
    fn bot_of_conversation(&self, session: &str) -> Option<BotHere> {
        let names = crate::wakes::folder::names(&self.bots).into_iter();
        names
            .filter_map(|id| BotHere::read(&self.bots, &id))
            .find(|bot| bot.session == session)
    }

    /// A change to the work of the bot a session works for, for clients to read it again; and,
    /// done by that bot itself, what it did, for clients to show it looking after its sessions.
    fn work_changed(&self, data: &Value, did: Option<(&str, &str)>) {
        let Some(bot) = works_for(data) else {
            return;
        };
        self.records.note_changed(bot);
        if let Some((by, kind)) = did
            && self
                .bot_of_conversation(by)
                .is_some_and(|here| here.id == bot)
            && let Some(id) = data["id"].as_str()
        {
            self.records.note_oversight(bot, id, kind);
        }
    }

    /// A session of someone's work as clients and its lead see it: where it stands now (`working`,
    /// what it asks), and, with `replies`, its last reply's first lines.
    pub async fn describe_work(
        &self,
        engine: &BotEngine,
        data: &Value,
        working: &HashSet<String>,
        replies: bool,
    ) -> Value {
        let id = data["id"].as_str().unwrap_or_default();
        let folder = folder_of(data);
        let asking = asking_now(engine, id).await;
        let read = replies && !working.contains(id);
        let page = if read {
            last_messages(engine, id, 6).await
        } else {
            Vec::new()
        };
        let outcome = if read {
            work_outcome(&page, &data["outcome"])
        } else {
            data["outcome"].clone()
        };
        // Stopped at rest (#428): its last turn, before the stop, isn't how it stands.
        let stopped_at = self.kept().stopped().get(id).and_then(Value::as_f64);
        let ended_at = last_turn_end(&page)
            .and_then(|end| end["time"]["created"].as_f64())
            .or_else(|| data["time"]["idle"].as_f64())
            .unwrap_or(0.0);
        let at_rest = stopped_at.is_some_and(|at| ended_at < at);
        let ended = match outcome.as_str() {
            Some("succeeded") => "finished",
            Some("failed") => "failed",
            Some("interrupted") => "stopped",
            _ if super::truthy(&data["time"]["idle"]) => "finished",
            _ => "idle",
        };
        let state = if asking.is_some() {
            "asking"
        } else if working.contains(id) {
            "working"
        } else if at_rest {
            "stopped"
        } else {
            ended
        };
        let before_stop = at_rest && state == "stopped";
        let as_said = if before_stop { ended } else { state };
        let reply = match () {
            () if !replies || state == "working" => None,
            () if matches!(as_said, "stopped" | "failed") => reply_of_last_turn(&page),
            () if read => reply_in(&page),
            () => reply_in(&last_messages(engine, id, 6).await),
        };
        let mut row = Map::new();
        row.insert("session".into(), json!(id));
        row.insert("title".into(), json!(title_of(data)));
        row.insert("folder".into(), json!(folder));
        let chats = self.home.join("chats");
        if std::path::Path::new(folder) != chats
            && let Some(project) = self.project_name(data)
        {
            row.insert("project".into(), json!(project));
        }
        row.insert(
            "startedBy".into(),
            json!(started_by(data).unwrap_or_default()),
        );
        let depth = data["metadata"]["kuvo"]["depth"]
            .as_u64()
            .filter(|depth| *depth > 0);
        row.insert("depth".into(), json!(depth.unwrap_or(1)));
        let model = &data["model"];
        if let (Some(model_id), Some(provider)) =
            (model["id"].as_str(), model["providerID"].as_str())
        {
            let mut shown = json!({ "id": model_id, "providerID": provider });
            if let Some(variant) = model["variant"]
                .as_str()
                .filter(|variant| !variant.is_empty())
            {
                shown["variant"] = json!(variant);
            }
            row.insert("model".into(), shown);
        }
        if let Some(mode) = data["agent"].as_str().filter(|mode| !mode.is_empty()) {
            row.insert("mode".into(), json!(mode));
        }
        row.insert("state".into(), json!(state));
        if let Some((ask, what)) = asking {
            row.insert("asks".into(), json!(what));
            row.insert("askID".into(), json!(ask));
        }
        let started = &data["time"]["created"];
        row.insert(
            "started".into(),
            if started.is_number() {
                started.clone()
            } else {
                json!(0)
            },
        );
        let tokens = &data["tokens"];
        let count = |value: &Value| value.as_u64().unwrap_or(0);
        let total =
            count(&tokens["input"]) + count(&tokens["output"]) + count(&tokens["reasoning"]);
        row.insert("tokens".into(), json!(total));
        if let Some(cost) = data["cost"].as_f64().filter(|cost| *cost > 0.0) {
            row.insert("cost".into(), json!(cost));
        }
        if let Some(reply) = reply {
            row.insert("lastReply".into(), json!(reply));
            if before_stop {
                row.insert("replyBeforeStop".into(), json!(true));
            }
        }
        Value::Object(row)
    }

    /// The rows of `lead`'s sessions here, the latest first, at most `most`, as list_sessions
    /// shows them: with how full each one's context is. None where they can't be read now.
    pub async fn live_rows(
        &self,
        engine: &BotEngine,
        lead: &str,
        most: usize,
    ) -> Option<Vec<Value>> {
        let all = work_sessions(engine).await?;
        let mine: Vec<&Value> = under(lead, &all).into_iter().take(most).collect();
        if mine.is_empty() {
            return Some(Vec::new());
        }
        let (working, _) = self.at_work_now(engine).await;
        let mut rows = Vec::new();
        for data in mine {
            let mut row = self.describe_work(engine, data, &working, true).await;
            if let Some(context) = context_of(engine, data["id"].as_str().unwrap_or_default()).await
            {
                row["context"] = context;
            }
            rows.push(row);
        }
        Some(rows)
    }

    /// One of `lead`'s sessions by its address, or what to say when it isn't one.
    async fn one_of_mine(
        &self,
        engine: &BotEngine,
        lead: &str,
        session: Option<&str>,
        doing: &str,
    ) -> Result<Value, Said> {
        let Some(session) = session.map(str::trim).filter(|session| !session.is_empty()) else {
            return Err(refused(
                "Say which session (session), by its address from list_sessions.",
            ));
        };
        let all = work_sessions(engine).await.unwrap_or_default();
        if let Some(found) = under(lead, &all)
            .into_iter()
            .find(|each| each["id"] == session)
        {
            return Ok(found.clone());
        }
        let title = match all.iter().find(|each| each["id"] == session) {
            Some(any) => Some(title_of(any).to_owned()),
            None => {
                let path = format!("api/session/{}", encoded(session));
                let found = engine.call(Method::GET, &path, None, WAIT).await;
                found
                    .ok()
                    .filter(|found| found.is_object())
                    .map(|found| title_of(&found).to_owned())
            }
        };
        let Some(title) = title else {
            return Err(refused(format!(
                "There's no session {session} here. Use list_sessions to see yours."
            )));
        };
        Err(refused(if doing == "answer" {
            format!("\"{title}\" isn't one of your sessions: only the user answers what it asks.")
        } else {
            format!("\"{title}\" isn't yours to {doing}: ask the user.")
        }))
    }

    /// stop_session: one of `lead`'s sessions stopped (what waits for it held), and put away if it
    /// says so.
    pub async fn stop_session_tool(
        &self,
        engine: &BotEngine,
        lead: &str,
        args: &Map<String, Value>,
    ) -> Said {
        let session = args.get("session").and_then(Value::as_str);
        let data = match self.one_of_mine(engine, lead, session, "stop").await {
            Ok(data) => data,
            Err(said) => return said,
        };
        let (id, title) = (data["id"].as_str().unwrap_or_default(), title_of(&data));
        if let Err(refusal) = self.stop_work(engine, id, false, Vec::new()).await {
            let why = refusal_words(refusal).await;
            return refused(format!("Couldn't stop \"{title}\": {why}"));
        }
        self.work_changed(&data, Some((lead, "stopped")));
        if args.get("archive") != Some(&Value::Bool(true)) {
            return said(format!("Stopped \"{title}\"."));
        }
        let archived = engine
            .rewrite(id, |_| {
                let top = [("kuvo.archived".to_owned(), Some(json!(true)))].into();
                Some(NoteEdits {
                    top,
                    ..NoteEdits::default()
                })
            })
            .await;
        if archived.is_ok() {
            crate::wakes::folder::hold_archived(&self.home.join("wakes"), id, true);
        }
        said(format!("Stopped and archived \"{title}\"."))
    }

    /// compact_session: one of `lead`'s sessions summarizes its conversation to free up its
    /// context, as its `/compact` does.
    pub async fn compact_session_tool(
        &self,
        engine: &BotEngine,
        lead: &str,
        args: &Map<String, Value>,
    ) -> Said {
        let session = args.get("session").and_then(Value::as_str);
        let data = match self.one_of_mine(engine, lead, session, "compact").await {
            Ok(data) => data,
            Err(said) => return said,
        };
        let (id, title) = (data["id"].as_str().unwrap_or_default(), title_of(&data));
        let working = working_now(engine).await.contains(id);
        let path = format!("api/session/{}/compact", encoded(id));
        let answer = match engine
            .call(Method::POST, &path, Some(&json!({})), WAIT)
            .await
        {
            Ok(answer) => answer,
            Err(error) => return refused(format!("Couldn't summarize \"{title}\": {}", error.0)),
        };
        self.work_changed(&data, None);
        // (The coding agents' bridge says; the engine takes it at its next step.)
        let state = answer["state"].as_str();
        if state == Some("waiting") {
            return said(format!(
                "\"{title}\" will be summarized when its turn ends."
            ));
        }
        if working && state != Some("compacting") {
            return said(format!("\"{title}\" will be summarized at its next step."));
        }
        said(format!(
            "Summarizing \"{title}\" now. list_sessions shows its context size again after its next step."
        ))
    }

    /// Whether a conversation is here.
    pub async fn has_session_here(&self, engine: &BotEngine, session: &str) -> bool {
        let path = format!("api/session/{}", encoded(session));
        let found = engine.call(Method::GET, &path, None, WAIT).await;
        found.is_ok_and(|found| found["id"] == session)
    }
}

/// What a refusal says, in words.
pub async fn refusal_words(refusal: kuvo_server::Refusal) -> String {
    use http_body_util::BodyExt;
    let body = refusal
        .reply()
        .body
        .collect()
        .await
        .map(|body| body.to_bytes());
    let said: Value = body
        .ok()
        .and_then(|body| serde_json::from_slice(&body).ok())
        .unwrap_or_default();
    said["message"]
        .as_str()
        .unwrap_or("it couldn't be done now")
        .to_owned()
}
