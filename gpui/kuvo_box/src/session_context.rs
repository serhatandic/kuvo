//! How full a conversation's context is, as its box measures it (`data.context` of
//! `GET api/session/<id>`, PROTOCOL.md "Session context"), and where it's summarized by itself.

use serde::Deserialize;
use serde_json::Value;

use crate::messages::Where;
use crate::{BoxClient, Result};

/// In tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSize {
    /// What the conversation holds now.
    pub used: u64,
    /// What its model holds, where known.
    #[serde(default)]
    pub window: Option<u64>,
    /// Where it's summarized by itself, where the box's setting decides that.
    #[serde(default)]
    pub summarize_at: Option<u64>,
}

/// Where a conversation is summarized by itself, offered to pick on it (`data.summarize`): the
/// box's setting ("Summarize long conversations at", `kuvo/settings/conversations`), in its words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Summarize {
    /// The choice in use, by id.
    pub value: String,
    /// What the row says (`Summarize at`).
    pub label: String,
    pub choices: Vec<SummarizeChoice>,
    /// What it's for, under the choices (`All conversations`).
    #[serde(default)]
    pub line: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SummarizeChoice {
    pub id: String,
    pub title: String,
    /// Beside it (`Default`).
    #[serde(default)]
    pub line: Option<String>,
}

impl Summarize {
    /// The choice in use, in its words.
    pub fn current(&self) -> Option<&SummarizeChoice> {
        self.choices.iter().find(|choice| choice.id == self.value)
    }
}

/// What a conversation says of its context: how full (from 0 where its model's window is known
/// before a measurement), and where it's summarized, to pick there.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionContext {
    pub size: Option<ContextSize>,
    pub summarize: Option<Summarize>,
}

/// Turned down, in these words.
fn refused(message: &str) -> crate::Error {
    let body = serde_json::to_vec(&serde_json::json!({ "message": message })).unwrap_or_default();
    crate::Error::Refused(crate::error::Refusal::from_answer(409, &body))
}

/// The row of `kuvo/settings/conversations` the summarize point is.
const SECTION: &str = "conversations";
const ROW: &str = "compaction";

impl BoxClient {
    /// How full `session`'s context is, at `place`: none before a measurement, after a summary
    /// without a new one, or from a box that doesn't say.
    pub fn session_context(&self, session: &str, place: Where) -> Result<Option<ContextSize>> {
        Ok(self.session_context_and_point(session, place)?.size)
    }

    /// How full `session`'s context is, at `place`, and where it's summarized: before a
    /// measurement, 0 of its model's window where its box knows that (`data.contextWindow`).
    pub fn session_context_and_point(&self, session: &str, place: Where) -> Result<SessionContext> {
        let page: Value = self.get(&place.session_route(session, ""))?;
        let data = page.get("data");
        let measured = data.and_then(|data| data.get("context")).and_then(|context| serde_json::from_value::<ContextSize>(context.clone()).ok());
        let window = data.and_then(|data| data.get("contextWindow")).and_then(Value::as_u64).filter(|window| *window > 0);
        let size = measured.or_else(|| window.map(|window| ContextSize { used: 0, window: Some(window), summarize_at: None }));
        let summarize = data.and_then(|data| data.get("summarize")).and_then(|summarize| serde_json::from_value(summarize.clone()).ok());
        Ok(SessionContext { size, summarize })
    }

    /// Picks where conversations are summarized (a choice's id), as Settings › Conversations does:
    /// the setting's row as it is now, changed from its revision (once more, where it changed
    /// in between).
    pub fn pick_summarize_point(&self, choice: &str) -> Result<()> {
        let page = self.settings_page(SECTION)?;
        let mut revision = page.row(ROW).map(|row| row.revision.clone()).ok_or_else(|| refused("Kuvo on this computer has no summarize setting."))?;
        for _ in 0..2 {
            match self.change_setting_at(SECTION, ROW, &revision, Value::String(choice.to_owned()))? {
                crate::settings_pages::Changed::Done(_) => return Ok(()),
                crate::settings_pages::Changed::Stale { page, message } => match page.row(ROW) {
                    Some(row) => revision = row.revision.clone(),
                    None => return Err(refused(&message)),
                },
            }
        }
        Err(refused("It changed since you read it. Try again."))
    }
}

/// A count of tokens, short: `412k`, `1M`, `1.2M`.
pub fn short_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 999_500 {
        format!("{}k", (tokens as f64 / 1_000.).round() as u64)
    } else {
        let millions = tokens as f64 / 1_000_000.;
        let tenths = (millions * 10.).round() / 10.;
        if tenths.fract() == 0. { format!("{}M", tenths as u64) } else { format!("{tenths:.1}M") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_people_say_them() {
        assert_eq!(short_tokens(412_345), "412k");
        assert_eq!(short_tokens(1_000_000), "1M");
        assert_eq!(short_tokens(999_700), "1M");
        assert_eq!(short_tokens(1_250_000), "1.3M");
        assert_eq!(short_tokens(800), "800");
    }
}
