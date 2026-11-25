//! What's news in a conversation's events (NOTIFY_RULE), said as your notifications say it: a
//! permission or a question asked, a run that failed, and a reply that answers you.

use std::{path::Path, sync::LazyLock, time::Duration};

use http::Method;
use kuvo_engine::BotEngine;
use kuvo_services::git::ProjectNames;
use regex::Regex;
use serde_json::{Map, Value, json};

use super::{
    Notice,
    mask::{masked, shows_all},
    words::{SENTENCE_LONGEST, one_sentence, permission_line, reason_words, short_words},
};
use crate::work::watch::turns::{page, summary_row_at_end, turn_in, written_by_you};

const WAIT: Duration = Duration::from_secs(10);

/// The events that can be news.
pub const NEWS: &[&str] = &[
    "permission.asked",
    "form.created",
    "session.execution.succeeded",
    "session.execution.failed",
];

fn strings(value: &Value) -> Vec<String> {
    let items = value.as_array().into_iter().flatten();
    items.filter_map(Value::as_str).map(str::to_owned).collect()
}

/// A folder outside the project as people know it ("~/notes", not "/Users/me/notes/*").
pub fn outside_shown(pattern: &str, home: Option<&str>) -> String {
    let mut path = pattern.trim();
    if path == "*" || path == "**" {
        return "Any folder".to_owned();
    }
    for glob in ["/**", "/*"] {
        if let Some(cut) = path.strip_suffix(glob) {
            path = if cut.is_empty() { "/" } else { cut };
            break;
        }
    }
    while path.len() > 1 && path.ends_with('/') {
        path = &path[..path.len() - 1];
    }
    let own = home.map(|home| {
        if home.len() > 1 {
            home.trim_end_matches('/')
        } else {
            home
        }
    });
    let rest = if path == "~" {
        Some("")
    } else if let Some(rest) = path.strip_prefix("~/") {
        Some(rest)
    } else {
        own.and_then(|own| {
            if path == own {
                Some("")
            } else {
                path.strip_prefix(own)
                    .and_then(|rest| rest.strip_prefix('/'))
            }
        })
    };
    match rest {
        None => path.to_owned(),
        Some("") => "~".to_owned(),
        Some(rest) => format!("~/{rest}"),
    }
}

// Whether Always saves only what a permission's notice shows.
fn offers_always(resources: &Value, save: &Value) -> bool {
    let shown = strings(resources);
    let saved = if save.is_array() {
        strings(save)
    } else {
        shown.clone()
    };
    !saved.is_empty()
        && !saved.iter().any(|each| each == "*")
        && saved.iter().all(|each| shown.contains(each))
}

const ALWAYS_LONGEST: usize = 24;

/// What a permission's Always saves, as its button says it: "Always Allow Edits", "Always Allow
/// git push", "Always Allow ~/notes".
pub fn always_title(action: &str, resources: &Value, save: &Value, home: Option<&str>) -> String {
    let asked = strings(resources);
    let saved = if save.is_array() {
        strings(save)
    } else {
        asked.clone()
    };
    // The line asked about, saved beside its kind: the kind says what it lets through.
    let kinds: Vec<String> = saved
        .iter()
        .filter(|each| !asked.contains(each))
        .cloned()
        .collect();
    let patterns = if kinds.is_empty() { saved } else { kinds };
    let kind = |pattern: &str| {
        pattern
            .strip_suffix(" *")
            .or(pattern.strip_suffix("/*"))
            .unwrap_or(pattern)
            .to_owned()
    };
    let short = |text: &str| {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() > ALWAYS_LONGEST {
            format!(
                "{}…",
                chars[..ALWAYS_LONGEST - 1].iter().collect::<String>()
            )
        } else {
            text.to_owned()
        }
    };
    let only = |named: String| format!("Always Allow {named}");
    match action {
        "edit" | "patch" | "write" => "Always Allow Edits".to_owned(),
        "shell" | "bash" => match patterns.as_slice() {
            [] => "Always Allow Commands".to_owned(),
            [one] if one == "*" => "Always Allow Commands".to_owned(),
            [one] => only(short(&kind(one))),
            _ => "Always Allow These Commands".to_owned(),
        },
        "external_directory" => match patterns.as_slice() {
            [one] => {
                let shown = outside_shown(one, home);
                let named = if shown.chars().count() > ALWAYS_LONGEST {
                    let base = Path::new(&shown)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned());
                    format!("…/{}", base.unwrap_or_default())
                } else {
                    shown
                };
                only(short(&named))
            }
            _ => "Always Allow These Folders".to_owned(),
        },
        _ => match patterns.as_slice() {
            [one] if one != "*" => only(short(&kind(one))),
            _ => "Always Allow".to_owned(),
        },
    }
}

fn shown_fields(form: &Value) -> Vec<&Value> {
    let fields = form["fields"].as_array().into_iter().flatten();
    fields
        .filter(|field| {
            field["hidden"] != true && field["when"].as_array().is_none_or(Vec::is_empty)
        })
        .collect()
}

// A question's one field and its choices, as answered from a notification.
fn single_question(form: &Value) -> Option<&Value> {
    if form["metadata"]["kind"] != "question" {
        return None;
    }
    match shown_fields(form).as_slice() {
        [one] if one["type"] == "string" => Some(*one),
        _ => None,
    }
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty())
}

/// What a form asks, as a notification says it.
pub fn describe_form(form: &Value) -> String {
    let (kind, fields) = (&form["metadata"]["kind"], &form["fields"][0]);
    if kind == "mcp-elicitation" {
        let said = text(&form["metadata"]["message"])
            .or(text(&fields["description"]))
            .or(text(&fields["title"]));
        return said.unwrap_or("Needs your answer.").to_owned();
    }
    if kind != "question" {
        return "Needs your answer.".to_owned();
    }
    match single_question(form) {
        Some(field) => text(&field["description"])
            .or(text(&field["title"]))
            .or(text(&form["title"]))
            .unwrap_or("Has a question for you.")
            .to_owned(),
        None => "Has a question for you.".to_owned(),
    }
}

/// Whether a session is a bot's own conversation.
pub fn is_bot(session: &Value) -> bool {
    let directory = session["location"]["directory"]
        .as_str()
        .unwrap_or_default();
    super::super::work::watch::truthy(&session["metadata"]["kuvo"]["bot"])
        || format!("{directory}/")
            .split("/bots/bot_")
            .nth(1)
            .is_some_and(|rest| rest.contains('/'))
}

fn hidden_run(session: &Value) -> bool {
    let kuvo = &session["metadata"]["kuvo"];
    ["engineFor", "seat", "reviewPart"]
        .iter()
        .any(|key| super::super::work::watch::truthy(&kuvo[*key]))
}

/// What began a session's latest turn: you (your message, or your answer, approval or pick passed
/// on), another agent (its letter or work for it), the session's own accord (its wake-up or
/// watch), what it left running or started (`Follow`), or `Unknown`, its start out of reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Began {
    You,
    Agent,
    Own,
    Follow,
    Unknown,
}

static SESSION_NEWS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\[Session (?:finished|still working|failed|asks|stopped)\] ""#)
        .expect("a pattern")
});

fn began_with(message: &Value) -> Began {
    if message["type"] != "user" {
        return Began::Follow;
    }
    let text = message["text"].as_str().unwrap_or_default();
    let kuvo = &message["metadata"]["kuvo"];
    let wake = kuvo["wake"]["summary"]
        .as_str()
        .is_some_and(|summary| !summary.trim().is_empty());
    if wake || text.starts_with("[Wake-up]") {
        return Began::Own;
    }
    if text.starts_with("[Message from ") || text.starts_with("[Work from ") {
        return Began::Agent;
    }
    if text.starts_with("[The user ") || text.starts_with("[Decision changed]") {
        return Began::You;
    }
    // A note the box wrote: who it's from.
    let note = &kuvo["note"];
    let from = match (note["from"].as_str(), note["line"].is_string()) {
        (Some(from), true) => Some(from),
        _ if text.starts_with("[The user answered] ") => Some("Your answer"),
        _ if SESSION_NEWS.is_match(text) => Some("Its sessions"),
        _ if note.is_object() => Some("Kuvo Code"),
        _ => None,
    };
    match from {
        Some("Your answer" | "Autopilot") => Began::You,
        Some("Its sessions") => Began::Follow,
        Some(_) => Began::Own,
        None if written_by_you(message) => Began::You,
        None => Began::Own,
    }
}

/// What began the latest turn in `messages` (newest first): any word of yours in it makes it
/// yours; else what began it. `whole`: they reach back to its start.
pub fn turn_for(messages: &[Value], whole: bool) -> Began {
    let rows: Vec<Value> = messages
        .iter()
        .filter(|row| row["type"].is_string())
        .cloned()
        .collect();
    let (from, end) = turn_in(&rows);
    let turn = &rows[from.min(rows.len())..end.unwrap_or(rows.len())];
    let kinds: Vec<Began> = turn
        .iter()
        .filter(|row| row["type"] == "user" || row["type"] == "synthetic")
        .map(began_with)
        .collect();
    if kinds.contains(&Began::You) {
        return Began::You;
    }
    if end.is_none() && !whole {
        return Began::Unknown;
    }
    kinds.last().copied().unwrap_or(Began::You)
}

/// NOTIFY_RULE: whether a reply is news for you. A reply to you is; one to another agent, or of
/// the session's own accord, isn't. What it left running or started coming back is, in a
/// conversation of yours (not a bot's, nor one an agent started), and so is a turn whose start is
/// out of reach there.
pub fn answers_you(began: Began, session: &Value) -> bool {
    match began {
        Began::You => true,
        Began::Follow | Began::Unknown => {
            !is_bot(session)
                && session["metadata"]["kuvo"]["startedBy"]
                    .as_str()
                    .is_none_or(str::is_empty)
        }
        Began::Agent | Began::Own => false,
    }
}

/// The running sessions (of `folder`) with a tool of MCP server `server` under way.
async fn waiting_on_server(engine: &BotEngine, server: &str, folder: Option<&str>) -> Vec<String> {
    let active = engine
        .call(Method::GET, "api/session/active", None, WAIT)
        .await
        .unwrap_or_default();
    let ids: Vec<String> = match &active {
        Value::Array(ids) => ids
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Value::Object(ids) => ids.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let name: String = server
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut found = Vec::new();
    for id in ids {
        let Ok(candidate) = engine.session_data(&id).await else {
            continue;
        };
        if folder.is_some_and(|folder| candidate["location"]["directory"] != folder) {
            continue;
        }
        let rows = crate::work::watch::turns::newest(engine, &id, 8)
            .await
            .unwrap_or_default();
        let steps = rows.iter().filter(|row| row["type"] == "assistant");
        let mut blocks =
            steps.flat_map(|row| row["content"].as_array().cloned().unwrap_or_default());
        let runs = blocks.any(|block| {
            block["type"] == "tool"
                && block["state"]["status"] == "running"
                && (block["name"]
                    .as_str()
                    .is_some_and(|tool| tool.starts_with(&format!("{name}_")))
                    || (block["name"] == "execute"
                        && block["state"]["input"]["code"]
                            .as_str()
                            .is_some_and(|code| code.contains(server))))
        });
        if runs {
            found.push(id);
        }
    }
    found
}

/// Where news is looked up: the engine (and the coding agents' conversations through it), and
/// this computer's projects for their names.
pub struct News<'a> {
    pub engine: &'a BotEngine,
    pub home: &'a Path,
    pub names: &'a ProjectNames,
}

impl News<'_> {
    // A subagent's request shows in the session that started it.
    async fn shown(&self, id: &str) -> Option<Value> {
        let mut current = self.engine.session_data(id).await.ok()?;
        for _ in 0..5 {
            let Some(parent) = current["parentID"]
                .as_str()
                .filter(|parent| !parent.is_empty())
            else {
                break;
            };
            current = self.engine.session_data(parent).await.ok()?;
        }
        Some(current)
    }

    fn project_of(&self, session: &Value) -> Option<String> {
        let folder = session["location"]["directory"]
            .as_str()
            .unwrap_or_default();
        let chats = self.home.join("chats");
        let in_chats = Path::new(folder) == chats
            || std::fs::canonicalize(folder)
                .ok()
                .zip(std::fs::canonicalize(&chats).ok())
                .is_some_and(|(a, b)| a == b);
        if in_chats && !folder.is_empty() {
            return Some("Chats".to_owned());
        }
        self.names
            .name_of(self.home, folder, &session["metadata"]["kuvo"]["filed"])
    }

    /// A notice about `shown`: its title (`untitled` for none), its project, where it opens.
    /// `answerable` false: its buttons would answer what it can't show, so it has none.
    pub fn about(
        &self,
        shown: &Value,
        category: &str,
        body: String,
        extra: Map<String, Value>,
        answerable: bool,
        untitled: &str,
    ) -> Notice {
        let bot = is_bot(shown);
        let mut data = Map::from_iter([("session".to_owned(), shown["id"].clone())]);
        data.extend(extra);
        if !answerable {
            data.insert("answerable".to_owned(), json!("0"));
        }
        Notice {
            title: text(&shown["title"]).unwrap_or(untitled).to_owned(),
            subtitle: if bot { None } else { self.project_of(shown) },
            body: Some(body),
            category: category.to_owned(),
            thread: shown["id"].as_str().map(str::to_owned),
            data,
        }
    }

    /// A session's latest messages, newest first, as far back as its latest turn begins, and
    /// whether they reach back to its start (or there's nothing older).
    async fn latest_turn(&self, id: &str) -> (Vec<Value>, bool) {
        if let Some(rows) = self.engine.agent_recent(id, 400).await {
            let whole = rows.len() < 400 || turn_in(&rows).1.is_some();
            return (rows, whole);
        }
        let mut data: Vec<Value> = Vec::new();
        let mut query = "limit=50&order=desc".to_owned();
        for _ in 0..20 {
            let Some(read) = page(self.engine, id, &query).await else {
                return (data, true);
            };
            let rows: Vec<Value> = read["data"]
                .as_array()
                .or(read.as_array())
                .cloned()
                .unwrap_or_default();
            let rows: Vec<Value> = rows
                .into_iter()
                .filter(|row| row["type"].is_string())
                .collect();
            let empty = rows.is_empty();
            data.extend(rows);
            let next = read["cursor"]["next"]
                .as_str()
                .or(read["cursor"]["previous"].as_str());
            let (Some(next), false) = (next, empty) else {
                return (data, true);
            };
            if turn_in(&data).1.is_some() {
                return (data, true);
            }
            let next =
                percent_encoding::utf8_percent_encode(next, percent_encoding::NON_ALPHANUMERIC);
            query = format!("limit=100&cursor={next}");
        }
        (data, false)
    }

    /// Whether the engine still lists `form` as asked: when it can't say, it's taken as asked.
    async fn still_asked(&self, form: &Value, folder: Option<&str>) -> bool {
        let session = form["sessionID"].as_str().unwrap_or_default();
        let path = format!(
            "api/session/{}/form",
            percent_encoding::utf8_percent_encode(session, percent_encoding::NON_ALPHANUMERIC)
        );
        let asked = match folder {
            Some(folder) => self.engine.call_in(Method::GET, &path, folder, WAIT).await,
            None => self.engine.call(Method::GET, &path, None, WAIT).await,
        };
        match asked {
            Ok(Value::Array(forms)) => forms.iter().any(|each| each["id"] == form["id"]),
            _ => true,
        }
    }

    /// The notice for an event, or none for one that isn't news.
    pub async fn notice_for(&self, kind: &str, data: &Value, location: &Value) -> Option<Notice> {
        let request_session = |shown: &Value, asker: &str| {
            let mut extra = Map::new();
            if shown["id"] != asker {
                extra.insert("requestSession".to_owned(), json!(asker));
            }
            extra
        };
        match kind {
            "permission.asked" => {
                let asker = data["sessionID"].as_str()?;
                let shown = self.shown(asker).await?;
                if hidden_run(&shown) {
                    return None;
                }
                // In a few plain words, never the command or the paths (the card shows them).
                let action = data["action"].as_str().unwrap_or_default();
                let (body, whole) = permission_line(action, &strings(&data["resources"]));
                let mut extra = request_session(&shown, asker);
                extra.insert("request".to_owned(), data["id"].clone());
                if !offers_always(&data["resources"], &data["save"]) {
                    extra.insert("always".to_owned(), json!("0"));
                }
                let home = std::env::var("HOME").ok();
                let title =
                    always_title(action, &data["resources"], &data["save"], home.as_deref());
                extra.insert("alwaysTitle".to_owned(), json!(masked(&title)));
                let answerable = whole && shows_all(&body);
                Some(self.about(&shown, "permission", body, extra, answerable, "Kuvo Code"))
            }
            "form.created" => {
                let form = if data["form"].is_object() {
                    &data["form"]
                } else {
                    data
                };
                let folder = location["directory"].as_str();
                let asker = match form["sessionID"].as_str()? {
                    "global" => {
                        let server = form["metadata"]["server"].as_str()?;
                        waiting_on_server(self.engine, server, folder)
                            .await
                            .into_iter()
                            .next()?
                    }
                    asker => asker.to_owned(),
                };
                let shown = self.shown(&asker).await?;
                if hidden_run(&shown) {
                    return None;
                }
                // On Autopilot nothing waits for you, unless it's still asked once the box has looked.
                if shown["metadata"]["kuvo"]["autopilot"] == true
                    && !self.still_asked(form, folder).await
                {
                    return None;
                }
                let body = short_words(&describe_form(form), SENTENCE_LONGEST);
                let choices = single_question(form).and_then(|field| field["options"].as_array());
                let choices_shown = choices.into_iter().flatten().all(|option| {
                    let label = match &option["label"] {
                        Value::String(label) => label.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    shows_all(&label)
                });
                let mut extra =
                    request_session(&shown, form["sessionID"].as_str().unwrap_or_default());
                extra.insert("request".to_owned(), form["id"].clone());
                let answerable = !body.ends_with('…') && shows_all(&body) && choices_shown;
                Some(self.about(&shown, "question", body, extra, answerable, "Kuvo Code"))
            }
            "session.execution.succeeded" | "session.execution.failed" => {
                let id = data["sessionID"].as_str()?;
                let ended = self.engine.session_data(id).await.ok()?;
                // A subagent's run is part of its parent's.
                if ended["parentID"]
                    .as_str()
                    .is_some_and(|parent| !parent.is_empty())
                    || hidden_run(&ended)
                {
                    return None;
                }
                let (rows, whole) = self.latest_turn(id).await;
                let plain = |body: &str| {
                    self.about(
                        &ended,
                        "failed",
                        body.to_owned(),
                        Map::new(),
                        true,
                        "New Session",
                    )
                };
                // Only a summary it was asked for: that it failed, whichever way its run ended.
                let summary = summary_row_at_end(&rows).and_then(|row| row["status"].as_str());
                if summary == Some("failed") {
                    return Some(plain("Couldn't summarize the conversation."));
                }
                if kind == "session.execution.failed" {
                    let error = &data["error"];
                    let reason = reason_words(
                        error["message"]
                            .as_str()
                            .or(error.as_str())
                            .unwrap_or_default(),
                    );
                    return Some(plain(&if reason.is_empty() {
                        "Stopped with an error.".to_owned()
                    } else {
                        format!("Stopped with an error: {reason}")
                    }));
                }
                if summary == Some("completed") || !answers_you(turn_for(&rows, whole), &ended) {
                    return None;
                }
                // What it said first, in a sentence.
                let replies = rows.iter().filter(|row| row["type"] == "assistant");
                let reply = replies
                    .map(|row| {
                        let parts = row["content"].as_array().into_iter().flatten();
                        let texts = parts.filter(|part| part["type"] == "text");
                        let said: Vec<&str> =
                            texts.filter_map(|part| part["text"].as_str()).collect();
                        one_sentence(&said.join("\n\n"), SENTENCE_LONGEST)
                    })
                    .find(|reply| !reply.is_empty());
                let category = if is_bot(&ended) { "report" } else { "finished" };
                let body = reply.unwrap_or_else(|| "Finished.".to_owned());
                Some(self.about(&ended, category, body, Map::new(), true, "New Session"))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Value {
        json!({ "type": "user", "text": text })
    }

    #[test]
    fn a_turn_is_yours_where_a_word_of_yours_began_it() {
        let idle = json!({ "type": "idle" });
        let reply = json!({ "type": "assistant" });
        assert_eq!(turn_for(&[reply.clone(), user("Fix it")], true), Began::You);
        assert_eq!(
            turn_for(
                &[
                    reply.clone(),
                    user("[Wake-up] look"),
                    idle.clone(),
                    user("old")
                ],
                true
            ),
            Began::Own
        );
        assert_eq!(
            turn_for(
                &[reply.clone(), user("[Message from Mira · ses_1] hi")],
                true
            ),
            Began::Agent
        );
        assert_eq!(
            turn_for(&[reply.clone(), json!({ "type": "synthetic" })], false),
            Began::Unknown
        );
        let mine = json!({ "metadata": {} });
        let started = json!({ "metadata": { "kuvo": { "startedBy": "ses_a" } } });
        assert!(answers_you(Began::Unknown, &mine));
        assert!(!answers_you(Began::Follow, &started));
        assert!(!answers_you(Began::Own, &mine));
    }

    #[test]
    fn always_says_what_it_saves() {
        let home = Some("/Users/me");
        assert_eq!(
            always_title(
                "bash",
                &json!(["git push origin main"]),
                &json!(["git push *"]),
                home
            ),
            "Always Allow git push"
        );
        assert_eq!(
            always_title("edit", &json!(["a.md"]), &Value::Null, home),
            "Always Allow Edits"
        );
        assert_eq!(
            always_title(
                "external_directory",
                &json!(["/Users/me/notes/*"]),
                &Value::Null,
                home
            ),
            "Always Allow ~/notes"
        );
        assert!(!offers_always(
            &json!(["git push origin main"]),
            &json!(["git push *"])
        ));
        assert_eq!(
            describe_form(
                &json!({ "metadata": { "kind": "question" }, "fields": [{ "type": "string", "title": "Which one?" }] })
            ),
            "Which one?"
        );
    }
}
