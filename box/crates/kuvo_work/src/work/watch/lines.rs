//! What a conversation is told of the sessions it started, line by line (`[Session finished] …`,
//! `failed`, `asks`, `stopped`), and what it shows for them.

use std::path::Path;

use serde_json::Value;

use super::turns::cut;
use crate::work::Work;

/// What a stopped session's line starts with.
pub const STOPPED_OPENING: &str = "[Session stopped]";

/// How long a run took, in words.
fn took(ms: u64) -> String {
    let minutes = (ms as f64 / 60_000.0).round() as u64;
    match minutes {
        0 => "under a minute".into(),
        1..90 => format!("{minutes} min"),
        _ => format!("{} h", (minutes as f64 / 60.0).round() as u64),
    }
}

/// A session's folder.
pub fn folder_of(session: &Value) -> &str {
    let folder = session["location"]["directory"].as_str();
    folder.or(session["directory"].as_str()).unwrap_or_default()
}

impl Work {
    /// The project a session's folder is filed under, by its name: as the session list files it
    /// (its worktrees and other folders too), else as the box's projects name it.
    fn project_name(&self, session: &Value) -> Option<String> {
        let folder = folder_of(session);
        if folder.is_empty() {
            return None;
        }
        let chats = self.home.join("chats");
        if Path::new(folder).starts_with(&chats) {
            return Some("Chats".into());
        }
        let record = self.sessions.latest_record().unwrap_or_default();
        let id = session["id"].as_str().unwrap_or_default();
        let filed = record["sessions"][id]["facts"]["project"]
            .as_str()
            .unwrap_or(folder)
            .to_owned();
        if let Some(name) = record["projects"][&filed]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
        {
            return Some(name.to_owned());
        }
        let projects = kuvo_services::git::read_projects(&self.home).unwrap_or_default();
        let named = projects
            .iter()
            .find(|each| each["directory"] == filed.as_str());
        let named = named
            .and_then(|each| each["name"].as_str())
            .filter(|name| !name.is_empty());
        let base = Path::new(&filed)
            .file_name()
            .map(|base| base.to_string_lossy().into_owned());
        named
            .map(str::to_owned)
            .or(base)
            .filter(|name| !name.is_empty())
    }

    /// A session as a line to its lead names it: `"Title" in project (address ses_…)`.
    pub(super) fn named_of(&self, session: &Value) -> String {
        let title = session["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .unwrap_or("New Session");
        let folder = folder_of(session);
        let place = match self.project_name(session) {
            Some(project) => project,
            None => Path::new(folder)
                .file_name()
                .map_or(folder.to_owned(), |base| {
                    base.to_string_lossy().into_owned()
                }),
        };
        let id = session["id"].as_str().unwrap_or_default();
        format!("\"{title}\" in {place} (address {id})")
    }

    /// What a conversation is told of a summary it asked of a session that failed.
    pub(super) fn not_summarized_line(&self, session: &Value, why: Option<&str>) -> String {
        let why = why
            .filter(|why| !why.is_empty())
            .map(|why| cut(why.trim_end_matches('.'), 400));
        let why = why.unwrap_or_else(|| "It couldn't".into());
        let named = self.named_of(session);
        format!(
            "[Session not summarized] {named}: {why}. Its conversation and work are as they were."
        )
    }

    /// What a lead is told of one of its sessions that the user stopped: only that, so it doesn't
    /// wait on a session that won't finish now.
    pub fn stopped_line(&self, session: &Value) -> String {
        let named = self.named_of(session);
        let rest = "Nothing more comes from it unless it's written to again: don't start it or its work again unless the user asks.";
        format!("{STOPPED_OPENING} {named} was stopped by the user. {rest}")
    }

    /// A finished session's line: what it said, and what it still serves.
    pub(super) fn finished_line(
        &self,
        session: &Value,
        reply: Option<String>,
        serving: &[String],
    ) -> String {
        let named = self.named_of(session);
        let created = session["time"]["created"].as_f64().map(|at| at as u64);
        let since =
            created.map(|at| format!(", after {}", took(super::told::now().saturating_sub(at))));
        let reply = reply.map_or("(It ended without a reply.)".into(), |reply| {
            cut(&reply, 1200)
        });
        // (A service it left up, a dev server, which its finish didn't wait on.)
        let services = match serving {
            [] => String::new(),
            serving => format!("\n(Still serving: {}.)", serving.join(", ")),
        };
        format!(
            "[Session finished] {named}{}:\n{reply}{services}",
            since.unwrap_or_default()
        )
    }

    /// A failed session's line: why, and what to do; one at its account or its limit fails the same
    /// way again there, so it says which model can't answer now.
    pub(super) fn failed_line(
        &self,
        session: &Value,
        why: Option<&str>,
        held: Option<String>,
    ) -> String {
        let named = self.named_of(session);
        if let Some(held) = held {
            let next = "Starting it again there fails the same way: start it again on a model that can, from list_models, or tell the user.";
            return format!("[Session failed] {named} stopped: {held}. {next}");
        }
        let why = why
            .filter(|why| !why.is_empty())
            .map(|why| format!(": {}", cut(why.trim_end_matches('.'), 400)));
        let next = "Read it with read_conversation, then start it again, try another way, or tell the user.";
        format!(
            "[Session failed] {named} stopped with an error{}. {next}",
            why.unwrap_or_default()
        )
    }

    /// A permission a session asks, as its lead is told it.
    pub(super) fn permission_line(&self, session: &Value, asked: &Value) -> String {
        let what = describe_permission(asked);
        let what = what
            .strip_prefix("Wants to ")
            .map_or(what.clone(), |rest| format!("asks to {rest}"));
        let (named, id) = (
            self.named_of(session),
            asked["id"].as_str().unwrap_or_default(),
        );
        let what = what.strip_suffix('.').unwrap_or(&what);
        format!(
            "[Session asks] {named} {what} (ask {id}). Answer with answer_ask if it's yours to allow, or tell the user in a line and carry on."
        )
    }

    /// A question a session asks, as its lead is told it.
    pub(super) fn form_line(&self, session: &Value, form: &Value) -> String {
        let (named, id) = (
            self.named_of(session),
            form["id"].as_str().unwrap_or_default(),
        );
        let asks = describe_form(form);
        format!(
            "[Session asks] {named} asks: {asks} (ask {id}). Answer with answer_ask (answer), or tell the user in a line and carry on."
        )
    }
}

/// A permission, as a notice says it: "Wants to edit files: a.txt · b.txt".
pub fn describe_permission(asked: &Value) -> String {
    let action = asked["action"].as_str().unwrap_or_default();
    let what = match action {
        "edit" | "write" | "patch" => "edit files",
        "shell" | "bash" => "run a command",
        "read" => "read a file",
        "external_directory" => "work outside the project",
        "webfetch" => "open a web page",
        other => other,
    };
    let resources = asked["resources"].as_array().into_iter().flatten();
    let shown: Vec<String> = resources
        .filter(|each| each.as_str() != Some("*"))
        .map(|each| {
            each.as_str()
                .map_or_else(|| each.to_string(), str::to_owned)
        })
        .collect();
    if shown.is_empty() {
        return format!("Wants to {what}.");
    }
    let more = match shown.len() {
        count if count > 4 => format!(" · and {} more", count - 4),
        _ => String::new(),
    };
    let first: Vec<&str> = shown.iter().take(4).map(String::as_str).collect();
    format!("Wants to {what}: {}{more}", first.join(" · "))
}

/// A question, as a notice says it.
pub fn describe_form(form: &Value) -> String {
    let field = &form["fields"][0];
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    match form["metadata"]["kind"].as_str() {
        Some("mcp-elicitation") => text(&form["metadata"]["message"])
            .or_else(|| text(&field["description"]))
            .or_else(|| text(&field["title"]))
            .unwrap_or_else(|| "Needs your answer.".into()),
        Some("question") => {
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
                [field] if field["type"] == "string" => text(&field["description"])
                    .or_else(|| text(&field["title"]))
                    .or_else(|| text(&form["title"]))
                    .unwrap_or_else(|| "Has a question for you.".into()),
                _ => "Has a question for you.".into(),
            }
        }
        _ => "Needs your answer.".into(),
    }
}

/// What goes to a conversation, from the lines gathered for it.
pub fn wake_text(lines: &[String]) -> String {
    lines.join("\n\n")
}

/// What a conversation shows for the lines told it, in place of them: "“Sign-in” finished", or how
/// many there are.
pub fn wake_line_shown(lines: &[String]) -> String {
    let [line] = lines else {
        return format!("{} updates from its sessions", lines.len());
    };
    let Some(rest) = line.strip_prefix("[Session ") else {
        return "News from a session".into();
    };
    let kinds = [
        ("finished", "finished"),
        ("still working", "still working"),
        ("failed", "stopped with an error"),
        ("stopped", "stopped by you"),
        ("asks", "asks something"),
    ];
    for (kind, said) in kinds {
        let Some(title) = rest
            .strip_prefix(kind)
            .and_then(|rest| rest.strip_prefix("] \""))
        else {
            continue;
        };
        let Some(end) = title
            .find(['"', '\n'])
            .filter(|end| title[*end..].starts_with('"'))
        else {
            return "News from a session".into();
        };
        let title = if end == 0 {
            "New Session"
        } else {
            &title[..end]
        };
        return format!("“{title}” {said}");
    }
    "News from a session".into()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_a_conversation_shows_for_its_sessions_news() {
        let line = |text: &str| vec![text.to_owned()];
        assert_eq!(
            wake_line_shown(&line(
                "[Session finished] \"Build\" in app (address ses_1):\nDone"
            )),
            "“Build” finished"
        );
        assert_eq!(
            wake_line_shown(&line("[Session failed] \"\" in app")),
            "“New Session” stopped with an error"
        );
        assert_eq!(
            wake_line_shown(&line("[Session stopped] \"Plan\" in app")),
            "“Plan” stopped by you"
        );
        assert_eq!(
            wake_line_shown(&line("Something else")),
            "News from a session"
        );
        assert_eq!(
            wake_line_shown(&["a".into(), "b".into()]),
            "2 updates from its sessions"
        );
    }

    #[test]
    fn a_permission_and_a_question_as_a_notice_says_them() {
        let files = json!({"action": "edit", "resources": ["a", "b", "c", "d", "e", "*"]});
        assert_eq!(
            describe_permission(&files),
            "Wants to edit files: a · b · c · d · and 1 more"
        );
        assert_eq!(
            describe_permission(&json!({"action": "bash", "resources": ["*"]})),
            "Wants to run a command."
        );
        let question = json!({"metadata": {"kind": "question"}, "fields": [{"type": "string", "title": "Which?"}]});
        assert_eq!(describe_form(&question), "Which?");
        assert_eq!(
            describe_form(&json!({"metadata": {"kind": "other"}})),
            "Needs your answer."
        );
    }
}
