//! What the box makes of a composer's draft (PROTOCOL.md, "What a typed message means"): sent to
//! the agent as a message, or kept where the conversation's model can't run, saying why. Whatever
//! it starts with ("/", "!" or anything else) is the agent's to read.

use serde::{Deserialize, Serialize};

/// What the box would answer for a draft where it isn't sent as a message: with a model that
/// can't run, all of it kept, saying why in the box's words (`refusal`, from
/// `kuvo/sessions/models`). None: it's sent.
pub fn plain_answer(text: &str, refusal: Option<&str>) -> Option<Answer> {
    refusal.map(|refusal| Answer { restore: Some(Restore { text: text.to_owned(), beside: None }), error: Some(refusal.to_owned()), ..Answer::default() })
}

/// A model, by its provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// What goes back to the composer: `text` into an empty one; beside words written since,
/// `beside` in front of them where it's given, else `text`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Restore {
    pub text: String,
    #[serde(default)]
    pub beside: Option<String>,
}

impl Restore {
    /// What goes in front of the composer's `draft`.
    pub fn text_beside(&self, draft: &str) -> &str {
        if draft.is_empty() { &self.text } else { self.beside.as_deref().unwrap_or(&self.text) }
    }
}

/// A message to send: its words, and whether the composer's quotes go with it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Send {
    pub text: String,
    #[serde(default)]
    pub quoted: Option<bool>,
}

/// The box's answer: the message it sent, or what goes back to the composer and why. Every field
/// is left out where it doesn't apply.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub send: Option<Send>,
    // What goes back.
    pub restore: Option<Restore>,
    pub error: Option<String>,
    /// Where it's refused for the conversation's model: what to offer doing about it (none for
    /// one this client can't read: the rest of the answer stands).
    #[serde(default, deserialize_with = "crate::models::lenient")]
    pub action: Option<crate::models::Action>,
    #[serde(default)]
    pub ambiguous: Option<bool>,
}

impl Answer {
    /// The composer's files go with what's sent; they stay otherwise.
    pub fn takes_files(&self) -> bool {
        self.send.is_some()
    }

    /// What a box that couldn't answer for a text says (any status but 200): what goes back to the
    /// composer (`written` as it was, where the box didn't say), and why.
    pub fn refused(status: u16, body: &[u8], written: &str) -> Answer {
        #[derive(Deserialize)]
        struct Refused {
            message: Option<String>,
            restore: Option<Restore>,
        }
        let refused: Option<Refused> = serde_json::from_slice(body).ok();
        let (message, restore) = refused.map(|r| (r.message, r.restore)).unwrap_or((None, None));
        Answer {
            restore: Some(restore.unwrap_or(Restore { text: written.to_owned(), beside: None })),
            error: Some(message.unwrap_or_else(|| format!("Its computer couldn’t say what that means ({status}). Try again."))),
            ..Answer::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An action a newer box words otherwise is none: the rest of its answer stands.
    #[test]
    fn an_action_this_client_cant_read_leaves_the_answer() {
        let answer: Answer = serde_json::from_value(serde_json::json!({ "restore": { "text": "fix it" }, "error": "Choose a model to send it.", "action": { "do": "choose" } })).unwrap();
        assert_eq!((answer.error.as_deref(), answer.action), (Some("Choose a model to send it."), None));
    }
}
