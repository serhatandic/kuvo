//! The conversation rows for background work and bot activity.

use std::collections::{BTreeSet, VecDeque};

use serde_json::Value;

use super::{Facts, Kind, LocalSessions, Snapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityState {
    Working,
    Asking,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Activity {
    pub session: String,
    pub title: String,
    pub state: ActivityState,
    pub subagent: bool,
    pub bot: Option<String>,
    pub ancestors: Vec<String>,
}

impl Activity {
    /// The root's own turn is excluded from its background activity.
    pub fn works_under(&self, root: &str) -> bool {
        self.session != root && self.ancestors.iter().any(|ancestor| ancestor == root)
    }
}

impl crate::Engine {
    /// A fresh, complete list for settling the status of migrated Work Queue items.
    pub async fn work_queue_active_sessions(&self) -> std::io::Result<BTreeSet<String>> {
        let lease = self
            .acquire(crate::lifetime::Demand::Passive)
            .await
            .ok_or_else(|| std::io::Error::other("The engine is not running."))?;
        let active = self
            .call::<Value>(&lease, http::Method::GET, "api/session/active", None)
            .await
            .map_err(std::io::Error::other)?;
        match active {
            Value::Object(active) if !active.contains_key("data") => {
                Ok(active.into_iter().map(|(session, _)| session).collect())
            }
            Value::Array(active) => Ok(active
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()),
            _ => Err(std::io::Error::other(
                "The engine did not supply a complete run list.",
            )),
        }
    }
}

impl LocalSessions {
    /// A local snapshot for the background work writer and peer activity publisher.
    pub fn activity(&self) -> Vec<Activity> {
        self.snapshot
            .borrow()
            .as_deref()
            .map_or_else(Vec::new, Snapshot::activity)
    }
}

impl Snapshot {
    fn activity(&self) -> Vec<Activity> {
        self.sessions
            .iter()
            .filter_map(|(session, row)| {
                let facts = &row.facts;
                let state = state(facts)?;
                let subagent = facts.kind == Kind::Subagent;
                let title = text(facts, "title").unwrap_or(if subagent {
                    "Subagent"
                } else {
                    "New Session"
                });
                let mut bot = text(facts, "bot");
                let mut pending: VecDeque<_> = parents(facts).collect();
                let mut seen = BTreeSet::new();
                let mut ancestors = Vec::new();
                while ancestors.len() < 24 {
                    let Some(parent) = pending.pop_front() else {
                        break;
                    };
                    if parent == session || !seen.insert(parent) {
                        continue;
                    }
                    ancestors.push(parent.to_owned());
                    if let Some(above) = self.sessions.get(parent).map(|row| &row.facts) {
                        bot = bot.or_else(|| text(above, "bot"));
                        pending.extend(parents(above));
                    }
                }
                Some(Activity {
                    session: session.clone(),
                    title: title.to_owned(),
                    state,
                    subagent,
                    bot: bot.map(str::to_owned),
                    ancestors,
                })
            })
            .collect()
    }
}

fn state(facts: &Facts) -> Option<ActivityState> {
    if facts.flag("away")
        || facts.flag("archived")
        || facts.flag("reviewPart")
        || facts.kind == Kind::Hidden
    {
        return None;
    }
    if facts.flag("ownAsk") || (facts.extra.contains_key("asked") && !facts.flag("childAsk")) {
        return Some(ActivityState::Asking);
    }
    (facts.flag("running") && !facts.flag("childOnly") && !facts.flag("childAsk"))
        .then_some(ActivityState::Working)
}

fn text<'a>(facts: &'a Facts, key: &str) -> Option<&'a str> {
    facts.extra.get(key).and_then(Value::as_str)
}

fn parents(facts: &Facts) -> impl Iterator<Item = &str> {
    text(facts, "parent").into_iter().chain(
        facts
            .extra
            .get("line")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    )
}
