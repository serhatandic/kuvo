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
    pub delivery: Option<&'static str>,
}

fn said(text: impl Into<String>) -> Said {
    Said {
        text: text.into(),
        refused: false,
        delivery: None,
    }
}

fn refused(text: impl Into<String>) -> Said {
    Said {
        text: text.into(),
        refused: true,
        delivery: None,
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

/// As `all_work`, those put away too.
pub async fn all_work_archived(engine: &BotEngine) -> Option<Vec<Value>> {
    super::catchup::work_sessions_as(engine, true).await
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
    pub(crate) fn bot_of_conversation(&self, session: &str) -> Option<BotHere> {
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
                let found = engine.session_data(session).await;
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
        let found = engine.session_data(session).await;
        found.is_ok_and(|found| found["id"] == session)
    }
}

/// What a session asks, that an answer is for.
pub enum Ask {
    Permission(Value),
    Question(Value),
}

/// The answers kept on a session, at most this many (the latest).
const ANSWERS_KEPT: usize = 40;

/// The question a form asks in one field, by that field's key; none for one of more parts.
fn question_key(form: &Value) -> Option<&str> {
    let shown = |field: &&Value| {
        field["hidden"] != true && field["when"].as_array().is_none_or(Vec::is_empty)
    };
    let fields: Vec<&Value> = form["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(shown)
        .collect();
    match fields.as_slice() {
        [field] if field["type"] == "string" => field["key"].as_str(),
        _ => None,
    }
}

impl Work {
    /// What one of `lead`'s sessions asks that `args` answers, before anything's answered: or what
    /// to say where it can't be.
    pub async fn ask_to_answer(
        &self,
        engine: &BotEngine,
        lead: &str,
        args: &Map<String, Value>,
    ) -> Result<(Value, Ask), Said> {
        let session = args.get("session").and_then(Value::as_str);
        let data = self.one_of_mine(engine, lead, session, "answer").await?;
        let title = title_of(&data).to_owned();
        let text = |field: &str| {
            args.get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
        };
        let ask = text("ask");
        if ask.is_empty() {
            return Err(refused(
                "Say which ask (ask), by its id from the [Session asks] message or list_sessions.",
            ));
        }
        if text("reason").is_empty() {
            return Err(refused(
                "Say why, in a line (reason): the user sees it with your answer.",
            ));
        }
        let id = data["id"].as_str().unwrap_or_default();
        let (permissions, questions, _) = asks_in(engine, id).await;
        if let Some(permission) = permissions.into_iter().find(|each| each["id"] == ask) {
            return Ok((data, Ask::Permission(permission)));
        }
        if let Some(question) = questions.into_iter().find(|each| each["id"] == ask) {
            return Ok((data, Ask::Question(question)));
        }
        Err(refused(format!(
            "\"{title}\" doesn't ask {ask} now: it was answered, or it moved on."
        )))
    }

    /// Why nothing it asks may be allowed for the user: it's in Ask First (for now, or as it's set)
    /// as it stands here now, whatever the lead's mode.
    pub fn ask_first_refusal(&self, data: &Value) -> Option<String> {
        let approvals = crate::approval::Approvals::new(&self.home, self.settings.is_mac());
        let id = data["id"].as_str().unwrap_or_default();
        let bot = self.bot_of_conversation(id).is_some() || works_for(data).is_some();
        let mode = approvals.of(data, bot, approvals.since(), approvals.default_approval());
        if mode.effective != crate::approval::Approval::AskFirst {
            return None;
        }
        let title = title_of(data);
        let state = match mode.held {
            Some(held) => format!(
                "\"{title}\" asks first for now ({})",
                held.why.trim_end_matches('.')
            ),
            None => format!("\"{title}\" is in Ask First"),
        };
        Some(format!(
            "You can't allow that: {state}, so the user answers every ask it makes, whatever mode you're in. The user sees the ask on \"{title}\" and answers it there. Tell them in a line why it's needed if it matters, and carry on with the rest. You can still deny it."
        ))
    }

    /// What an allow of `permission` is judged by, as `action` and what: a command's parts as the
    /// engine matches each and what it lists; a file's paths as written, and, outside the
    /// session's folders, as working outside the project, where it's written and where it is.
    pub async fn allow_checks(
        &self,
        engine: &BotEngine,
        data: &Value,
        permission: &Value,
    ) -> Vec<(String, String)> {
        let action = permission["action"].as_str().unwrap_or_default().to_owned();
        let mut asked: Vec<String> = permission["resources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        if asked.is_empty() {
            asked.push("*".into());
        }
        let input = call_input(engine, permission).await;
        let mut checks: Vec<(String, String)> = Vec::new();
        if action == "shell" || action == "bash" {
            let command = input.as_ref().and_then(|input| input["command"].as_str());
            for part in command
                .map(super::super::policy::parts_of)
                .unwrap_or_default()
            {
                checks.push((action.clone(), part));
            }
        }
        let files = super::super::policy::FILES.contains(&action.as_str());
        let named: Vec<String> = match &input {
            Some(input) if files => ["filePath", "file_path", "path", "file"]
                .iter()
                .filter_map(|field| input[*field].as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        // (A path the tool names is judged for its `\\` only, which the engine reads as a `/`: the
        // peers refuse that for a file tool.)
        let marked = named.iter().filter(|each| each.contains('\\'));
        for each in asked.iter().chain(marked) {
            checks.push((action.clone(), each.clone()));
        }
        if files {
            let folder = folder_of(data);
            let mut folders: Vec<std::path::PathBuf> = Vec::new();
            if !folder.is_empty() {
                folders.push(folder.into());
                let projects = kuvo_services::git::read_projects(&self.home).unwrap_or_default();
                let listed = projects
                    .iter()
                    .find(|each| each.get("directory").and_then(Value::as_str) == Some(folder));
                let more = listed
                    .and_then(|each| each.get("folders"))
                    .and_then(Value::as_array);
                folders.extend(
                    more.into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(Into::into),
                );
            }
            let from = folders.first().cloned().unwrap_or_else(|| "/".into());
            for each in asked.iter().chain(&named).filter(|each| *each != "*") {
                if !(std::path::Path::new(each).is_absolute() || !folders.is_empty()) {
                    continue;
                }
                let full = super::super::policy::resolved(&from, each);
                let real = super::super::policy::real_of(&full);
                let inside = folders.iter().any(|folder| {
                    super::super::policy::within(&real, &super::super::policy::real_of(folder))
                });
                if !inside {
                    for at in [&full, &real] {
                        checks.push((
                            "external_directory".into(),
                            at.to_string_lossy().into_owned(),
                        ));
                    }
                }
            }
        }
        checks
    }

    /// answer_ask, once it's been checked: what one of `lead`'s sessions asks answered, by `by`
    /// (the bot it is, or its name), and kept on the session with why.
    pub async fn answer_ask_tool(
        &self,
        engine: &BotEngine,
        lead: &str,
        by: Value,
        args: &Map<String, Value>,
    ) -> Said {
        let (data, ask) = match self.ask_to_answer(engine, lead, args).await {
            Ok(found) => found,
            Err(said) => return said,
        };
        let (id, title) = (
            data["id"].as_str().unwrap_or_default(),
            title_of(&data).to_owned(),
        );
        let text = |field: &str| {
            args.get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_owned()
        };
        let reason = text("reason");
        let at = super::told::now();
        let (kept, answered) = match ask {
            Ask::Question(question) => {
                let answer = text("answer");
                let asked = question_text(&question);
                if answer.is_empty() {
                    return refused(format!(
                        "\"{title}\" asks: {asked} Answer it in words (answer)."
                    ));
                }
                let Some(key) = question_key(&question) else {
                    return refused(format!(
                        "\"{title}\"'s question has more than one part: tell the user, who answers it in the session."
                    ));
                };
                let asker = question["asker"].as_str().unwrap_or(id);
                let form = question["id"].as_str().unwrap_or_default();
                let path = format!(
                    "api/session/{}/form/{}/reply",
                    encoded(asker),
                    encoded(form)
                );
                let body = json!({ "answer": { key: answer } });
                if let Err(error) = engine.call(Method::POST, &path, Some(&body), WAIT).await {
                    return refused(format!("Couldn't answer it: {}", error.0));
                }
                let kept = json!({ "ask": form, "what": format!("Asks: {asked}"), "decision": "answer", "reason": format!("{answer} ({reason})"), "by": by, "at": at });
                (kept, format!("Answered \"{title}\"."))
            }
            Ask::Permission(permission) => {
                let decision = text("decision");
                if decision != "allow" && decision != "deny" {
                    return refused("Say allow or deny (decision).");
                }
                let resources: Vec<&str> = permission["resources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let shown: Vec<&str> = resources
                    .iter()
                    .copied()
                    .filter(|each| *each != "*")
                    .collect();
                let shown = if shown.is_empty() {
                    permission["action"].as_str().unwrap_or_default().to_owned()
                } else {
                    shown.join(", ")
                };
                let asker = permission["asker"].as_str().unwrap_or(id);
                let ask = permission["id"].as_str().unwrap_or_default();
                let path = format!(
                    "api/session/{}/permission/{}/reply",
                    encoded(asker),
                    encoded(ask)
                );
                let name = by["name"].as_str().unwrap_or_default();
                let body = if decision == "allow" {
                    json!({ "decision": "once" })
                } else {
                    json!({ "decision": "reject", "message": format!("{name}: {reason}") })
                };
                if let Err(error) = engine.call(Method::POST, &path, Some(&body), WAIT).await {
                    return refused(format!("Couldn't answer it: {}", error.0));
                }
                let mut kept = json!({ "ask": ask, "what": describe_permission(&permission), "decision": decision, "reason": reason, "by": by, "at": at });
                if let Some(call) = permission["source"]["id"].as_str() {
                    kept["call"] = json!(call);
                }
                let answered = if decision == "allow" {
                    format!("Allowed: {shown}.")
                } else {
                    format!("Denied: {shown}. \"{title}\" is told why.")
                };
                (kept, answered)
            }
        };
        let _ = engine
            .rewrite(id, |now| {
                let before = now["metadata"]["kuvo"]["answers"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let mut answers: Vec<Value> = before.into_iter().chain([kept]).collect();
                let over = answers.len().saturating_sub(ANSWERS_KEPT);
                answers.drain(..over);
                let kuvo = [("answers".to_owned(), Some(Value::Array(answers)))].into();
                Some(NoteEdits {
                    kuvo,
                    ..NoteEdits::default()
                })
            })
            .await;
        self.work_changed(&data, Some((lead, "answered")));
        said(answered)
    }
}

impl Work {
    /// list_projects: the projects here, with where each is, whether it's a git repository, and
    /// who made it; `place`: this computer, by its name.
    pub async fn list_projects_tool(&self, place: &str) -> Said {
        let home = self.settings.account();
        let projects = kuvo_services::git::read_projects(&self.home).unwrap_or_default();
        let projects: Vec<_> = projects
            .iter()
            .filter(|each| each.get("removed") != Some(&Value::Bool(true)))
            .collect();
        let folder = match self.settings.projects_folder() {
            Some(root) => format!(" New ones go in {}.", super::super::projects::shown_path(&root.to_string_lossy(), home)),
            None => " No projects folder is picked yet: the user picks one the first time you make a project.".into(),
        };
        if projects.is_empty() {
            return said(format!("There are no projects on {place} yet.{folder}"));
        }
        let mut lines = Vec::new();
        for each in projects.into_iter().take(60) {
            let directory = each
                .get("directory")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let inside = super::super::projects::git(
                std::path::Path::new(directory),
                &["rev-parse", "--is-inside-work-tree"],
                &[],
            )
            .await;
            let git = if inside.as_deref() == Some("true") {
                "git"
            } else {
                "not git"
            };
            let base = std::path::Path::new(directory)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = each
                .get("name")
                .and_then(Value::as_str)
                .map_or(base, str::to_owned);
            let made = each
                .get("madeBy")
                .and_then(|by| by["name"].as_str())
                .map(|by| format!(", made by {by}"))
                .unwrap_or_default();
            lines.push(format!(
                "- {name}: {} ({git}){made}",
                super::super::projects::shown_path(directory, home)
            ));
        }
        said(format!(
            "Projects on {place}:\n{}\n{}",
            lines.join("\n"),
            folder.trim()
        ))
    }

    /// list_models: the models a session can be started on here, by the one rule every client's
    /// picker offers by; those that can't answer now apart, at its end. `place`: this computer.
    pub async fn list_models_tool(&self, place: &str) -> Said {
        use super::super::unanswering::{ModelRef, hold_of, hold_words};
        let offered = self.list.engine().offered_models().await;
        let shown = |model: &&Value| model["retiring"] != true && model["hidden"] != true;
        let models: Vec<&Value> = offered.models.iter().filter(shown).collect();
        let mut notes = Vec::new();
        if offered.engine == Some(false) {
            notes.push(format!("The engine on {place} can't say which of its models run yet: try list_models again in a moment."));
        }
        if offered.agents == Some(false) {
            notes.push(format!("The coding agents on {place} can't say which of their models run yet: try list_models again in a moment."));
        }
        if models.is_empty() {
            return said(if notes.is_empty() {
                format!(
                    "There's no model with an account on {place}. Ask the user to sign in to one in Settings › Accounts."
                )
            } else {
                notes.join("\n")
            });
        }
        let text =
            |model: &Value, field: &str| model[field].as_str().unwrap_or_default().to_owned();
        let strings = |model: &Value, field: &str| -> Vec<String> {
            model[field]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        let held = self.unanswering.now();
        let held_by = |model: &Value, holds: &[_]| {
            let mut reference = ModelRef::new(&text(model, "providerID"), &text(model, "id"));
            reference.free = Some(model["free"] == true);
            hold_of(&reference, &strings(model, "aliases"), holds).is_some()
        };
        let hold = |model: &Value| held_by(model, &held).then_some(());
        let line = |model: &Value| {
            let efforts = strings(model, "efforts");
            let efforts = if efforts.is_empty() {
                String::new()
            } else {
                format!(", efforts: {}", efforts.join(", "))
            };
            format!(
                "- {}/{} ({}){efforts}",
                text(model, "providerID"),
                text(model, "id"),
                text(model, "name")
            )
        };
        let answering: Vec<String> = models
            .iter()
            .filter(|model| hold(model).is_none())
            .map(|model| line(model))
            .collect();
        let mut holds: Vec<String> = Vec::new();
        for each in &held {
            if models
                .iter()
                .any(|model| held_by(model, std::slice::from_ref(each)))
            {
                holds.push(format!("- {}", hold_words(each, &self.home)));
            }
        }
        let list = if answering.is_empty() {
            "No model here can answer now: tell the user why, from below.".to_owned()
        } else {
            format!(
                "Models you can start a session on, as provider/model (give it to start_session as model, and an effort as effort):\n{}",
                answering.join("\n")
            )
        };
        let whole = if holds.is_empty() {
            list
        } else {
            format!(
                "{list}\n\nLeft out, as a session on them just failed:\n{}",
                holds.join("\n")
            )
        };
        said(if notes.is_empty() {
            whole
        } else {
            format!("{whole}\n\n{}", notes.join("\n"))
        })
    }

    /// Every session under `lead` here that's working, waiting on an answer or on a timer of its
    /// own, made before `before`, stopped (a bot's Stop All, from the computer it lives on): how
    /// many.
    pub async fn stop_line_work(&self, engine: &BotEngine, lead: &str, before: f64) -> usize {
        let all = work_sessions(engine).await.unwrap_or_default();
        let made = |data: &Value| data["time"]["created"].as_f64().unwrap_or(0.0);
        let work: Vec<&Value> = under(lead, &all)
            .into_iter()
            .filter(|data| made(data) <= before)
            .collect();
        if work.is_empty() {
            return 0;
        }
        let (working, turns) = self.at_work_now(engine).await;
        let mut stopped = 0;
        for data in work {
            let id = data["id"].as_str().unwrap_or_default();
            let turn = turns.contains(id);
            let asking = !turn && asking_now(engine, id).await.is_some();
            if !working.contains(id) && !self.has_wake_ups(id) && !asking {
                continue;
            }
            if self
                .stop_work(engine, id, turn || asking, Vec::new())
                .await
                .is_ok()
            {
                stopped += 1;
            }
        }
        stopped
    }
}

/// The tool call a permission asks for, whole (its input): never only what the ask lists (it
/// leaves out a command's `cd`), so what's judged is what would run.
async fn call_input(engine: &BotEngine, permission: &Value) -> Option<Value> {
    let call = permission["source"]["id"].as_str()?;
    let message = permission["source"]["messageID"].as_str();
    let asker = permission["asker"].as_str()?;
    for each in last_messages(engine, asker, 30).await {
        if message.is_some_and(|message| each["id"] != message) {
            continue;
        }
        let parts = each["content"].as_array().into_iter().flatten();
        if let Some(part) = parts
            .into_iter()
            .find(|part| part["type"] == "tool" && part["id"] == call)
        {
            let input = &part["state"]["input"];
            return Some(if input.is_object() {
                input.clone()
            } else {
                json!({})
            });
        }
    }
    None
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
