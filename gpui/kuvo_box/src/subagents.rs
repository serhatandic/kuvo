//! A conversation's subagents, at work and done (PROTOCOL.md, "Work › Background work",
//! `work.subagents`), as the side panel's Subagents tab lists them: the box says which run and
//! which finished, newest first, and the names of their models.

use serde::Deserialize;

use crate::error::Result;
use crate::follow::{Followed, follow_lines};
use crate::paths;
use crate::{BoxClient, Follow};

pub const SUBAGENTS: &str = "work.subagents";

/// A subagent at work.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Active {
    pub session: String,
    #[serde(default)]
    pub title: String,
    /// Since when (ms).
    pub since: f64,
    /// It waits on you.
    #[serde(default)]
    pub asking: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

/// How a subagent ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    Finished,
    Failed,
    Stopped,
    #[serde(other)]
    Unknown,
}

/// A subagent that's done.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Done {
    pub session: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub started: f64,
    pub ended: f64,
    pub state: Ended,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

/// A conversation's subagents, each list newest first.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Subagents {
    #[serde(default)]
    pub active: Vec<Active>,
    #[serde(default)]
    pub done: Vec<Done>,
}

impl Subagents {
    /// The one with `session`'s id, at work or done: its title, model and effort.
    pub fn find(&self, session: &str) -> Option<(&str, Option<&str>, Option<&str>)> {
        self.active
            .iter()
            .find(|active| active.session == session)
            .map(|active| (active.title.as_str(), active.model.as_deref(), active.effort.as_deref()))
            .or_else(|| self.done.iter().find(|done| done.session == session).map(|done| (done.title.as_str(), done.model.as_deref(), done.effort.as_deref())))
    }
}

fn route(session: &str) -> String {
    format!("kuvo/work/subagents/{}", paths::segment(session))
}

impl BoxClient {
    /// `session`'s subagents now.
    pub fn subagents(&self, session: &str) -> Result<Subagents> {
        self.get(&route(session))
    }

    /// `session`'s subagents, now and as they change.
    pub fn follow_subagents(&self, session: &str) -> Follow<Followed<Subagents>> {
        let once = route(session);
        follow_lines(self, format!("{once}/events"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversations_subagents_read_at_work_and_done() {
        let read: Subagents = serde_json::from_str(r#"{"active":[{"session":"ses_a","title":"Check the retry path","since":5,"asking":true,"model":"Claude Sonnet 4.6"}],"done":[{"session":"ses_b","title":"Read the error codes","started":1,"ended":3,"state":"finished","model":"Claude Sonnet 4.6","effort":"High"},{"session":"ses_c","title":"Gone","ended":4,"state":"crashed"}]}"#).unwrap();
        assert!(read.active[0].asking);
        assert_eq!(read.done[1].state, Ended::Unknown, "a word a newer box says");
        assert_eq!(read.find("ses_b"), Some(("Read the error codes", Some("Claude Sonnet 4.6"), Some("High"))));
    }
}
