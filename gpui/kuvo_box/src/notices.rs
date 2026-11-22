//! What a box told lately (`kuvo/notices`, PROTOCOL.md): its notices as it made them, for a client
//! to show as its own notifications. What's news and how it's said is the box's alone; a client
//! only decides whether this device shows it (Your settings › Notifications).

use std::collections::HashMap;

use serde::Deserialize;

use crate::client::BoxClient;
use crate::follow::{Follow, Followed, follow_lines};

/// What a box told in the last ten minutes, oldest first, and whether it tells its news now (a
/// machine whose bridge isn't running doesn't: it has no news to show until it runs again).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Told {
    #[serde(default)]
    pub telling: bool,
    #[serde(default)]
    pub notices: Vec<ToldNotice>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToldNotice {
    pub id: String,
    #[serde(default)]
    pub at: f64,
    pub notice: Notice,
    /// `"agent"`: an agent sent it on purpose (`notify_user`).
    #[serde(default)]
    pub by: Option<String>,
}

/// A notice as the box words it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Notice {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// `permission`, `question`, `ask`, `finished`, `failed`, `report`, `blocked`, `message` or
    /// `usage`; a client shows one it doesn't know plainly.
    pub category: String,
    #[serde(default)]
    pub thread: Option<String>,
    /// Where it opens (`session`, `group`), and how it's shown (`quiet: "1"`: without a sound).
    /// Only the string values are kept.
    #[serde(default, deserialize_with = "strings")]
    pub data: HashMap<String, String>,
}

impl Notice {
    /// Shown without a sound (what a goal needs of you, its digest).
    pub fn quiet(&self) -> bool {
        self.data.get("quiet").is_some_and(|quiet| quiet == "1")
    }

    /// About your accounts' limits (sent by the box's Usage notices), not a session.
    pub fn is_usage(&self) -> bool {
        self.category == "usage"
    }

    /// The session it opens, where it's one's.
    pub fn session(&self) -> Option<&str> {
        self.data.get("session").map(String::as_str)
    }
}

fn strings<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<HashMap<String, String>, D::Error> {
    let values: Option<HashMap<String, serde_json::Value>> = Option::deserialize(deserializer)?;
    Ok(values.unwrap_or_default().into_iter().filter_map(|(key, value)| value.as_str().map(|value| (key, value.to_owned()))).collect())
}

impl BoxClient {
    /// What the box told lately, as it is now.
    pub fn notices(&self) -> crate::Result<Told> {
        self.get("kuvo/notices")
    }

    /// What the box told lately: as it is now, then again each time it changes (over SSH, read
    /// again every few seconds).
    pub fn follow_notices(&self) -> Follow<Followed<Told>> {
        follow_lines(self, "kuvo/notices/events".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_reads_as_the_box_words_it_and_keeps_only_strings_in_its_data() {
        let told: Told = serde_json::from_str(r#"{"telling":true,"notices":[{"id":"ntc_1","at":5,"notice":{"title":"Fixer","subtitle":"kuvo-ai","body":"Done.","category":"finished","thread":"ses_1","data":{"session":"ses_1","quiet":"1","n":3}}},{"id":"ntc_2","at":6,"notice":{"category":"usage"},"by":"agent"}]}"#).unwrap();
        assert!(told.telling);
        let first = &told.notices[0].notice;
        assert_eq!((first.session(), first.quiet(), first.data.get("n")), (Some("ses_1"), true, None));
        assert!(told.notices[1].notice.is_usage() && told.notices[1].by.as_deref() == Some("agent"));
        // A box from before `telling` says nothing of it: it isn't telling.
        let older: Told = serde_json::from_str(r#"{"notices":[]}"#).unwrap();
        assert!(!older.telling);
    }
}
