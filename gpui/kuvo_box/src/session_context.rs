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

impl BoxClient {
    /// How full `session`'s context is, at `place`: none before a measurement, after a summary
    /// without a new one, or from a box that doesn't say.
    pub fn session_context(&self, session: &str, place: Where) -> Result<Option<ContextSize>> {
        let page: Value = self.get(&place.session_route(session, ""))?;
        Ok(page.get("data").and_then(|data| data.get("context")).and_then(|context| serde_json::from_value(context.clone()).ok()))
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
