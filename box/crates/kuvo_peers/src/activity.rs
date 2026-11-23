//! Active and last-seen sessions for a conversation or bot, kept by origin.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    sync::Arc,
};

use kuvo_wire::keys::encode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    directory::{Announcement, ComputerKey},
    kept,
    lists::{Computer, Kind, Local},
    reach::Millis,
    setup_lists::View,
};

const FEATURE: &str = "line.pending-computers";
const ROWS_PER_PART: usize = 20;
const MAX_PARTS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Working,
    Asking,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Fact {
    session: String,
    line: Vec<String>,
    title: String,
    state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subagent: Option<bool>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Saved {
    pub at: Millis,
    pub heard: Millis,
    rows: Vec<Fact>,
}

#[derive(Deserialize)]
struct Part {
    at: Millis,
    part: usize,
    total: usize,
    rows: Vec<Value>,
}

pub(crate) struct Receiving {
    at: Millis,
    parts: Vec<Option<Vec<Fact>>>,
}

pub(crate) fn load(run: &Path, key: &ComputerKey) -> Option<Saved> {
    kept::read(&run.join("activity").join(format!("{}.json", encode(key))))
}

pub(crate) fn take(
    run: &Path,
    key: ComputerKey,
    now: Millis,
    value: Value,
    copies: &mut BTreeMap<ComputerKey, Arc<Saved>>,
    receiving: &mut BTreeMap<ComputerKey, Receiving>,
) -> io::Result<()> {
    let Ok(part) = serde_json::from_value::<Part>(value) else {
        return Ok(());
    };
    if part.total > MAX_PARTS
        || part.part >= part.total
        || part.rows.len() > ROWS_PER_PART
        || part.at <= copies.get(&key).map_or(0, |copy| copy.at)
    {
        return Ok(());
    }
    let assembly = receiving.entry(key).or_insert_with(|| Receiving {
        at: part.at,
        parts: vec![None; part.total],
    });
    if part.at < assembly.at {
        return Ok(());
    }
    if part.at > assembly.at {
        *assembly = Receiving {
            at: part.at,
            parts: vec![None; part.total],
        };
    }
    if part.total != assembly.parts.len() {
        return Ok(());
    }
    assembly.parts[part.part] = Some(part.rows.into_iter().filter_map(read_fact).collect());
    if assembly.parts.iter().any(Option::is_none) {
        return Ok(());
    }
    let rows = receiving
        .remove(&key)
        .expect("completed activity")
        .parts
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    let copy = Saved {
        at: part.at,
        heard: now,
        rows,
    };
    kept::write(
        &run.join("activity").join(format!("{}.json", encode(&key))),
        &copy,
    )?;
    copies.insert(key, Arc::new(copy));
    Ok(())
}

fn read_fact(row: Value) -> Option<Fact> {
    let id = |value: &Value, prefix: &str| {
        let text = value.as_str()?;
        let suffix = text.strip_prefix(prefix)?;
        ((1..=60).contains(&suffix.len()) && suffix.bytes().all(|c| c.is_ascii_alphanumeric()))
            .then(|| text.to_owned())
    };
    let session = id(row.get("session")?, "ses_")?;
    let state = serde_json::from_value(row.get("state")?.clone()).ok()?;
    let title: String = row
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect();
    Some(Fact {
        session,
        state,
        title: if title.is_empty() {
            "New Session".into()
        } else {
            title
        },
        bot: row.get("bot").and_then(|bot| id(bot, "bot_")),
        line: row
            .get("line")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| id(part, "ses_"))
            .take(24)
            .collect(),
        subagent: (row.get("subagent").and_then(Value::as_bool) == Some(true)).then_some(true),
    })
}

pub(crate) fn local(local: &Local) -> Vec<Fact> {
    let Some(list) = &local.list else {
        return vec![];
    };
    list.sessions
        .iter()
        .filter_map(|(session, row)| {
            let facts = &row.facts;
            let asking =
                facts.own_ask.is_some() || (facts.asked.is_some() && facts.child_ask.is_none());
            let working = facts.running == Some(true)
                && facts.child_only.is_none()
                && facts.child_ask.is_none();
            if facts.away == Some(true)
                || facts.archived == Some(true)
                || facts.kind == Kind::Hidden
                || facts.review_part.is_some()
                || !(asking || working)
            {
                return None;
            }
            let mut parents: Vec<_> = facts
                .parent
                .iter()
                .chain(facts.line.iter().flatten())
                .collect();
            let mut line = Vec::new();
            let mut seen = BTreeSet::new();
            let mut bot = facts.bot.clone();
            let mut index = 0;
            while let Some(&parent) = parents.get(index).filter(|_| line.len() < 24) {
                index += 1;
                if parent == session || !seen.insert(parent) {
                    continue;
                }
                line.push(parent.clone());
                if let Some(above) = list.sessions.get(parent) {
                    bot = bot.or_else(|| above.facts.bot.clone());
                    parents.extend(
                        above
                            .facts
                            .parent
                            .iter()
                            .chain(above.facts.line.iter().flatten()),
                    );
                }
            }
            let subagent = (facts.kind == Kind::Subagent).then_some(true);
            Some(Fact {
                session: session.clone(),
                line,
                bot,
                subagent,
                title: facts.title.clone().unwrap_or_else(|| {
                    if subagent.is_some() {
                        "Subagent"
                    } else {
                        "New Session"
                    }
                    .into()
                }),
                state: if asking {
                    State::Asking
                } else {
                    State::Working
                },
            })
        })
        .collect()
}

pub(crate) fn split(rows: &[Fact], at: Millis) -> impl Iterator<Item = Value> {
    let total = rows.len().div_ceil(ROWS_PER_PART).max(1);
    (0..total.min(if total <= MAX_PARTS { total } else { 0 })).map(move |part| {
        let start = part * ROWS_PER_PART;
        serde_json::json!({"at":at,"part":part,"total":total,"rows":&rows[start..rows.len().min(start + ROWS_PER_PART)]})
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub working: usize,
    pub asking: usize,
    pub sessions: Vec<ActivitySession>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub last_seen_sessions: Vec<ActivitySession>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pending_computers: Vec<Computer>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySession {
    pub session: String,
    pub title: String,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<Millis>,
}

impl View {
    pub fn activity(
        &self,
        root: Option<&str>,
        bot: Option<&str>,
        besides: bool,
        now: Millis,
    ) -> Activity {
        let local = local(&self.local);
        let mut rows: BTreeMap<&str, (&Fact, Option<ComputerKey>, Option<Millis>)> = local
            .iter()
            .map(|fact| (fact.session.as_str(), (fact, None, None)))
            .collect();
        let mut peers: Vec<_> = self
            .roster
            .members
            .iter()
            .filter_map(|peer| {
                let copy = self.activities.get(&peer.key)?;
                let key = encode(&peer.key);
                let quiet = self.reach.peer_is_away(&key, Some(copy.heard), now);
                let release = self.announced.get(&key).and_then(Announcement::release);
                let standing = self
                    .reach
                    .standing_of(&key, quiet, Some(copy.heard), release, now);
                Some((peer.key, standing.away(), copy))
            })
            .collect();
        peers.sort_by_key(|(_, away, copy)| (*away, std::cmp::Reverse(copy.heard)));
        for (key, away, copy) in peers {
            let computer = encode(&key);
            for fact in &copy.rows {
                let owns = self.moved(&fact.session, now).map_or_else(
                    || !rows.contains_key(fact.session.as_str()),
                    |owner| owner == computer,
                );
                if owns {
                    rows.insert(&fact.session, (fact, Some(key), away.then_some(copy.heard)));
                }
            }
        }
        let mut result = Activity {
            working: 0,
            asking: 0,
            sessions: vec![],
            last_seen_sessions: vec![],
            pending_computers: self.pending(FEATURE, |key| self.activities.contains_key(key)),
        };
        for (fact, computer, last_seen) in rows.into_values() {
            if besides && root == Some(&fact.session) {
                continue;
            }
            if !(root
                .is_some_and(|root| fact.session == root || fact.line.iter().any(|id| id == root))
                || bot.is_some_and(|bot| fact.bot.as_deref() == Some(bot)))
            {
                continue;
            }
            let row = ActivitySession {
                session: fact.session.clone(),
                title: fact.title.clone(),
                state: fact.state,
                computer: computer.map(|computer| encode(&computer)),
                subagent: fact.subagent,
                last_seen,
            };
            if last_seen.is_some() {
                result.last_seen_sessions.push(row);
            } else {
                match row.state {
                    State::Working => result.working += 1,
                    State::Asking => result.asking += 1,
                }
                result.sessions.push(row);
            }
        }
        result
    }
}
