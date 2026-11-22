//! What a conversation waits on you for (PROTOCOL.md, "A conversation's asks", `sessions.asks`):
//! permissions and the agent's questions, its subagents' too, and what the box asks you itself (a
//! command outside the sandbox, something from the vault), each worded by the box, and answering
//! them. A client draws an ask as the box says it and sends back the answer picked.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::follow::{Follow, Followed, follow_lines};
use crate::paths;
use crate::transport::Request;

/// The asks waiting, oldest first. Each is read on its own: one this client can't read (a newer
/// kind, or one missing a field) is [`Ask::Unknown`], and the rest still show.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Asks {
    pub asks: Vec<Ask>,
}

impl<'de> Deserialize<'de> for Asks {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            asks: Vec<Value>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Asks { asks: raw.asks.into_iter().map(|ask| serde_json::from_value(ask).unwrap_or(Ask::Unknown)).collect() })
    }
}

/// One ask, as the box words it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Ask {
    Permission(Permission),
    Question(Question),
    Sandbox(Sandbox),
    Vault(Vault),
    /// Anything else it waits on you for (a form a card can't take, an MCP server's), answered where
    /// it was asked; `dismiss`: whether it can be put away unanswered from here.
    Other { id: String, session: String, title: String, message: String, dismiss: bool },
    /// A kind this client doesn't know yet (a newer box's): left out, the rest still read.
    #[serde(other)]
    Unknown,
}

impl Ask {
    pub fn id(&self) -> &str {
        match self {
            Ask::Permission(permission) => &permission.id,
            Ask::Question(question) => &question.id,
            Ask::Sandbox(sandbox) => &sandbox.id,
            Ask::Vault(vault) => &vault.id,
            Ask::Other { id, .. } => id,
            Ask::Unknown => "",
        }
    }
}

/// A permission: what the card's title says, the engine's words, the whole command line it would
/// run, the changes it would make, what it covers, and its answers in their order.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Permission {
    pub id: String,
    pub session: String,
    pub title: String,
    pub message: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub files: Vec<Changes>,
    #[serde(default)]
    pub resources: Vec<String>,
    pub choices: Vec<Choice>,
}

/// A file a permission would change: its patch, as a step's changes are kept.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Changes {
    pub file: String,
    pub patch: String,
    pub additions: u64,
    pub deletions: u64,
    pub status: Option<String>,
}

/// One answer an ask offers: `once`, `always`, `reject`, or `instead` (Don't Allow, with your
/// words, which opens a line for them: its `placeholder`). `named`: what Always names, shown as
/// code after its title, and `after`, words after that; `help`: what it does whole (its hover
/// text); `spoken`: as a screen reader says it. `item`: the vault's item it gives, sent with the
/// answer. `confirm`: it makes a grant, which a client makes sure is you first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Choice {
    pub answer: String,
    pub title: String,
    pub named: Option<String>,
    pub placeholder: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub spoken: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub confirm: bool,
}

/// A command the sandbox stopped, which the agent asks to run outside it: what stopped it
/// (`message`), the command, what the sandbox refused it, the error it got where that can't be
/// said, who asks and why, and its answers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Sandbox {
    pub id: String,
    pub session: String,
    pub title: String,
    pub message: String,
    pub command: String,
    #[serde(default)]
    pub stopped: Vec<Stopped>,
    pub error: Option<String>,
    pub asker: String,
    pub why: String,
    pub choices: Vec<Choice>,
}

/// One thing the sandbox refused a command: its tag ("Write") and where, or in words.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Stopped {
    pub label: String,
    pub detail: String,
}

/// What the agent asks of the vault: to use an item (`need` "use") or an item it needs ("item"),
/// its words, the site it's for, why nothing can be given here (`note`), and its answers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Vault {
    pub id: String,
    pub session: String,
    pub need: String,
    pub title: String,
    pub message: Option<String>,
    pub site: Option<String>,
    pub note: Option<String>,
    pub choices: Vec<Choice>,
}

/// The agent's questions.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Question {
    pub id: String,
    pub session: String,
    pub title: String,
    pub questions: Vec<Asked>,
}

/// One of a question's: what it asks, its choices, whether it takes several, words of your own,
/// and whether it must be answered.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Asked {
    pub key: String,
    pub title: Option<String>,
    pub question: String,
    #[serde(default)]
    pub options: Vec<Choose>,
    pub multiple: bool,
    pub custom: bool,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Choose {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// An answer, as the box takes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Answer {
    /// A permission's: `once`, `always` or `reject`; `instead` with your words. The box's own asks'
    /// the same, with the vault's item a choice gives.
    Permission {
        answer: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    /// A question's answers, by each question's key: a choice's value, your words, or several.
    Questions { answers: serde_json::Map<String, Value> },
    /// A question put away unanswered.
    Dismiss { dismiss: bool },
}

impl Answer {
    pub fn permission(answer: &str, message: Option<String>) -> Answer {
        Answer::Permission { answer: answer.to_owned(), message, item: None }
    }

    /// One of the choices an ask offers, as it is (with the item it gives).
    pub fn choice(choice: &Choice) -> Answer {
        Answer::Permission { answer: choice.answer.clone(), message: None, item: choice.item.clone() }
    }

    pub fn dismiss() -> Answer {
        Answer::Dismiss { dismiss: true }
    }

    /// Each question's answer: one value, or several for one that takes several.
    pub fn questions(answers: impl IntoIterator<Item = (String, Vec<String>, bool)>) -> Answer {
        let answers = answers.into_iter().map(|(key, values, multiple)| (key, if multiple { json!(values) } else { json!(values.into_iter().next().unwrap_or_default()) })).collect();
        Answer::Questions { answers }
    }
}

/// Where a conversation's asks are.
pub fn route(session: &str) -> String {
    format!("kuvo/sessions/{}/asks", paths::segment(session))
}

impl BoxClient {
    /// What `session` waits on you for now.
    pub fn asks(&self, session: &str) -> Result<Asks> {
        self.get(&route(session))
    }

    /// Follows `session`'s asks: those now, and again as they change.
    pub fn follow_asks(&self, session: &str) -> Follow<Followed<Asks>> {
        follow_lines(self, format!("{}/events", route(session)))
    }

    /// Answers `ask` in `session`. A refusal says why (an answer that didn't fit, or one already
    /// answered: `404`).
    pub fn answer_ask(&self, session: &str, ask: &str, answer: &Answer) -> Result<()> {
        let body = serde_json::to_value(answer).map_err(|error| Error::BadAnswer(error.to_string()))?;
        let response = self.send(&Request::post(format!("{}/{}", route(session), paths::segment(ask)), body))?;
        if response.is_success() { Ok(()) } else { Err(Error::Refused(Refusal::from_answer(response.status, &response.body))) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_read_as_the_box_words_them() {
        let asks: Asks = serde_json::from_value(json!({ "asks": [
            { "id": "per_1", "kind": "permission", "session": "ses_1", "title": "Allow running this command?", "message": null, "command": "git push", "files": [], "resources": ["git push"], "choices": [
                { "answer": "once", "title": "Allow Once", "named": null, "placeholder": null },
                { "answer": "always", "title": "Always Allow", "named": "git push", "placeholder": null }
            ] },
            { "id": "frm_1", "kind": "question", "session": "ses_sub", "title": "Question", "questions": [
                { "key": "q0", "title": "Database", "question": "Which one?", "options": [{ "value": "pg", "label": "Postgres", "description": null }], "multiple": false, "custom": true, "required": true }
            ] },
            { "id": "frm_2", "kind": "other", "session": "ses_1", "title": "Linear", "message": "Needs your answer.", "dismiss": true },
            { "id": "zzz_1", "kind": "poll", "session": "ses_1", "anything": [1, 2] },
            { "id": "sq_1", "kind": "sandbox", "session": "ses_1", "title": "Scout wants to run a command outside the sandbox", "message": "The sandbox stopped it.", "command": "npm i", "stopped": [{ "label": "Write", "detail": "~/.npm" }], "error": null, "asker": "Scout", "why": "To add it.", "choices": [
                { "answer": "always", "title": "Always Allow", "named": "~/.npm", "placeholder": null, "help": "Runs it, and from now on…", "spoken": "Always Allow ~/.npm", "after": "in every chat" }
            ] },
            { "id": "q_1", "kind": "vault", "session": "ses_1", "need": "item", "title": "Scout needs a key", "message": null, "site": null, "note": null, "choices": [
                { "answer": "always", "title": "Always Give", "named": "OpenAI", "placeholder": null, "help": null, "spoken": null, "item": "itm_1", "confirm": true }
            ] }
        ] })).unwrap();
        assert_eq!(asks.asks[3], Ask::Unknown, "a newer kind doesn't spoil the rest");
        assert!(matches!(&asks.asks[4], Ask::Sandbox(sandbox) if sandbox.stopped[0].label == "Write" && sandbox.choices[0].after.as_deref() == Some("in every chat")));
        assert!(matches!(&asks.asks[5], Ask::Vault(vault) if vault.choices[0].confirm && vault.choices[0].item.as_deref() == Some("itm_1")));
        assert!(matches!(&asks.asks[0], Ask::Permission(permission) if permission.choices[1].named.as_deref() == Some("git push")));
        assert!(matches!(&asks.asks[1], Ask::Question(question) if question.questions[0].options[0].label == "Postgres"));
        assert_eq!(asks.asks[2].id(), "frm_2");
    }

    #[test]
    fn answers_go_as_the_box_takes_them() {
        assert_eq!(serde_json::to_value(Answer::permission("instead", Some("Use a branch".into()))).unwrap(), json!({ "answer": "instead", "message": "Use a branch" }));
        assert_eq!(serde_json::to_value(Answer::permission("once", None)).unwrap(), json!({ "answer": "once" }));
        let give = Choice { answer: "once".into(), title: "Give".into(), named: Some("OpenAI".into()), placeholder: None, help: None, spoken: None, after: None, item: Some("itm_1".into()), confirm: false };
        assert_eq!(serde_json::to_value(Answer::choice(&give)).unwrap(), json!({ "answer": "once", "item": "itm_1" }));
        let answers = Answer::questions([("q0".into(), vec!["pg".into()], false), ("q1".into(), vec!["a".into(), "b".into()], true)]);
        assert_eq!(serde_json::to_value(answers).unwrap(), json!({ "answers": { "q0": "pg", "q1": ["a", "b"] } }));
        assert_eq!(serde_json::to_value(Answer::dismiss()).unwrap(), json!({ "dismiss": true }));
    }
}
