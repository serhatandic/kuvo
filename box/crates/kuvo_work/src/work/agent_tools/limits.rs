//! How many sessions an agent may start unattended: a few at a time (a bot's work with only a net
//! against a runaway), counted in `run/tool-limits.json` (the letters' count's file too), and the
//! limits you set for a bot (Edit Bot › Work), its sessions here and on your other computers.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_peers::work_rows::{Row, WorkState, for_lead};
use kuvo_store::locks::{OnBusy, StaleLock};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Caller, sessions::counted};
use crate::{
    bots::{here::BotHere, list::limits_from},
    work::{
        Work,
        watch::{is_archived, tools::all_work, works_for},
    },
};

const WITHIN: i64 = 600_000;
/// How long a bot's start is counted on its own, before its session shows where it works.
const STARTING_FOR: i64 = 120_000;

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Start {
    at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    prompted: bool,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Counts {
    #[serde(default)]
    sessions: BTreeMap<String, Vec<i64>>,
    #[serde(default)]
    bot_sessions: BTreeMap<String, Vec<i64>>,
    #[serde(default)]
    starting: BTreeMap<String, BTreeMap<String, Start>>,
    #[serde(default)]
    messages: BTreeMap<String, Vec<i64>>,
    /// (The letters' count, and what a newer box keeps.)
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// The counts as `change` leaves them, under their lock, the old left out.
fn change_counts<T>(run: &Path, change: impl FnOnce(&mut Counts) -> T) -> T {
    let _lock = StaleLock::acquire(
        &run.join(".tool-limits.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Proceed,
    );
    let file = run.join("tool-limits.json");
    let mut counts: Counts = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let at = now();
    for times in [
        &mut counts.sessions,
        &mut counts.bot_sessions,
        &mut counts.messages,
    ] {
        times
            .values_mut()
            .for_each(|times| times.retain(|when| at - when < WITHIN));
        times.retain(|_, times| !times.is_empty());
    }
    counts
        .starting
        .values_mut()
        .for_each(|starts| starts.retain(|_, start| at - start.at < STARTING_FOR));
    counts.starting.retain(|_, starts| !starts.is_empty());
    let value = change(&mut counts);
    let _ = serde_json::to_vec(&counts).map(|bytes| kuvo_store::files::write_secret(&file, &bytes));
    value
}

/// A start counted for its caller (and its bot's limits), until it's known how it went.
pub struct Counted {
    run: PathBuf,
    bot: Option<String>,
    key: String,
    at: i64,
    token: Option<String>,
}

impl Counted {
    /// It started (`session`, given its task or not): its bot's later starts count it until the
    /// engine does.
    pub fn went(&self, session: &str, prompted: bool) {
        self.note(Some(session), prompted);
    }

    /// It didn't start: taken back, so one tried again isn't counted twice.
    pub fn back(&self) {
        change_counts(&self.run, |counts| {
            let times = if self.bot.is_some() {
                &mut counts.bot_sessions
            } else {
                &mut counts.sessions
            };
            if let Some(lately) = times.get_mut(&self.key)
                && let Some(index) = lately.iter().position(|when| *when == self.at)
            {
                lately.remove(index);
            }
        });
        self.note(None, false);
    }

    /// Whether it started isn't known: no longer counted as starting.
    pub fn unknown(&self) {
        self.note(None, false);
    }

    fn note(&self, session: Option<&str>, prompted: bool) {
        let (Some(bot), Some(token)) = (&self.bot, &self.token) else {
            return;
        };
        change_counts(&self.run, |counts| {
            let Some(starts) = counts.starting.get_mut(bot) else {
                return;
            };
            match session {
                Some(session) => {
                    if let Some(start) = starts.get_mut(token) {
                        start.session = Some(session.into());
                        start.prompted |= prompted;
                    }
                }
                None => {
                    starts.remove(token);
                }
            }
        });
    }
}

/// Which limit you set a new session at `depth` would be past, in the words its bot reads; a
/// day's (`daily`) resets at midnight.
fn past_limit(
    limits: &Map<String, Value>,
    depth: u64,
    working: usize,
    started: usize,
    tokens: u64,
) -> Option<(String, bool)> {
    let limit = |name: &str| limits.get(name).and_then(Value::as_u64);
    let s = |most: u64| if most == 1 { "" } else { "s" };
    let ask = " Wait for one to finish, or ask the user to raise it (Edit Bot › Work).";
    if let Some(most) = limit("depth").filter(|most| depth > *most) {
        let deep = if most == 1 {
            "one level deep".into()
        } else {
            format!("{most} levels deep")
        };
        return Some((
            format!(
                "The user set your sessions to go {deep} at most, so this one can't start another. Do it in this session, or ask the user."
            ),
            false,
        ));
    }
    if let Some(most) = limit("atOnce").filter(|most| working as u64 >= *most) {
        return Some((
            format!(
                "The user set you to {most} session{} at once at most, and that many are working.{ask}",
                s(most)
            ),
            false,
        ));
    }
    if let Some(most) = limit("perDay").filter(|most| started as u64 >= *most) {
        return Some((
            format!(
                "The user set you to {most} new session{} a day at most, and you've started that many today.",
                s(most)
            ),
            true,
        ));
    }
    let most = limit("tokensPerDay").filter(|most| tokens >= *most)?;
    Some((
        format!(
            "Your sessions have used the {} tokens a day the user set.",
            counted(most)
        ),
        true,
    ))
}

impl Work {
    /// One more message from `key` (a conversation), twenty in ten minutes at most: when it's
    /// counted (to take back if it doesn't go), or none past that.
    pub(super) fn count_message(&self, key: &str) -> Option<i64> {
        let at = now();
        change_counts(&self.home.join("run"), |counts| {
            let lately = counts.messages.entry(key.to_owned()).or_default();
            (lately.len() < 20).then(|| lately.push(at)).map(|()| at)
        })
    }

    /// One counted at `at` that didn't go.
    pub(super) fn message_back(&self, key: &str, at: i64) {
        change_counts(&self.home.join("run"), |counts| {
            if let Some(lately) = counts.messages.get_mut(key) {
                lately.retain(|when| *when != at);
            }
        });
    }

    /// A start by `caller` at `depth` counted: refused past the limits you set for its bot (its
    /// sessions here and, as they last told, on your other computers), or past the net against a
    /// runaway. Noted before the limits are looked at, so starts asked for at once count each
    /// other.
    pub(super) async fn count_start(&self, caller: &Caller, depth: u64) -> Result<Counted, String> {
        let run = self.home.join("run");
        let bot = caller.works_for().map(|bot| bot.bot);
        let limits = bot.as_ref().and_then(|bot| {
            let here = BotHere::read(&self.bots, bot)?;
            limits_from(here.described.get("work")?.get("limits"))
        });
        let limits = limits.or_else(|| {
            bot.as_ref()
                .and(limits_from(caller.data["metadata"]["kuvo"].get("limits")))
        });
        let mut counted = Counted {
            run: run.clone(),
            bot: bot.clone(),
            key: caller.session.clone(),
            at: 0,
            token: None,
        };
        if let (Some(bot), Some(limits)) = (&bot, &limits) {
            let token = format!("{}-{:08x}", now(), rand_word());
            let before = change_counts(&run, |counts| {
                let starts = counts.starting.entry(bot.clone()).or_default();
                let before: Vec<Start> = starts.values().cloned().collect();
                starts.insert(
                    token.clone(),
                    Start {
                        at: now(),
                        ..Start::default()
                    },
                );
                before
            });
            counted.token = Some(token);
            if let Some((words, daily)) = self.past_bot_limit(bot, limits, depth, &before).await {
                counted.note(None, false);
                return Err(if daily {
                    format!(
                        "{words} Ask the user to raise it (Edit Bot › Work), or go on tomorrow."
                    )
                } else {
                    words
                });
            }
        }
        let (most, key) = match &bot {
            Some(bot) => (40, format!("bot:{bot}")),
            None => (6, caller.session.clone()),
        };
        let at = now();
        let allowed = change_counts(&run, |counts| {
            let times = if bot.is_some() {
                &mut counts.bot_sessions
            } else {
                &mut counts.sessions
            };
            let lately = times.entry(key.clone()).or_default();
            (lately.len() < most).then(|| lately.push(at)).is_some()
        });
        counted.key = key;
        if !allowed {
            counted.note(None, false);
            return Err(if bot.is_some() {
                "You've started a great many sessions in the last few minutes, which looks like a loop. Stop, check on the ones you have (list_sessions), and tell the user."
            } else {
                "You've started several sessions in the last few minutes. Write to those (send_message), or ask the user before starting more."
            }
            .into());
        }
        counted.at = at;
        Ok(counted)
    }

    /// The limit of `bot`'s a new session at `depth` would be past, counting what it has at work
    /// and started today here and elsewhere, and the starts noted `before` this one.
    async fn past_bot_limit(
        &self,
        bot: &str,
        limits: &Map<String, Value>,
        depth: u64,
        before: &[Start],
    ) -> Option<(String, bool)> {
        let engine = self.list.engine().running_bot_engine().await;
        let all = match &engine {
            Some(engine) => all_work(engine).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let (working, _) = match &engine {
            Some(engine) => self.at_work_now(engine).await,
            None => Default::default(),
        };
        let work: Vec<&Value> = all
            .iter()
            .filter(|data| works_for(data) == Some(bot))
            .collect();
        let midnight = jiff::Zoned::now()
            .start_of_day()
            .map_or(0, |day| day.timestamp().as_millisecond());
        let today: Vec<&Value> = work
            .iter()
            .copied()
            .filter(|data| data["time"]["created"].as_f64().unwrap_or(0.0) as i64 >= midnight)
            .collect();
        let ids: HashSet<&str> = work.iter().filter_map(|data| data["id"].as_str()).collect();
        let today_ids: HashSet<&str> = today
            .iter()
            .filter_map(|data| data["id"].as_str())
            .collect();
        // (One carried on with the coding agents is one start.)
        let started_today = today
            .iter()
            .filter(|data| {
                data["metadata"]["kuvo.carriedTo"]
                    .as_str()
                    .is_none_or(|to| !ids.contains(to))
            })
            .count();
        let own = BotHere::read(&self.bots, bot).map(|here| here.session);
        let mut far: Vec<Row> = Vec::new();
        for lead in own.iter().map(String::as_str).chain(ids.iter().copied()) {
            for there in for_lead(&self.peers, lead).await.unwrap_or_default() {
                far.extend(
                    there
                        .rows
                        .into_iter()
                        .filter(|row| !ids.contains(row.session.as_str())),
                );
            }
        }
        let far_today: Vec<&Row> = far
            .iter()
            .filter(|row| row.started as i64 >= midnight)
            .collect();
        let tokens: u64 = today
            .iter()
            .map(|data| {
                ["input", "output", "reasoning"]
                    .iter()
                    .map(|field| data["tokens"][field].as_u64().unwrap_or(0))
                    .sum::<u64>()
            })
            .sum::<u64>()
            + far_today.iter().map(|row| row.tokens as u64).sum::<u64>();
        let known = |start: &Start, ids: &HashSet<&str>, rows: &[&Row]| {
            start.session.as_deref().is_some_and(|session| {
                ids.contains(session) || rows.iter().any(|row| row.session == session)
            })
        };
        let far_all: Vec<&Row> = far.iter().collect();
        let working = work
            .iter()
            .filter(|data| {
                data["id"].as_str().is_some_and(|id| working.contains(id)) && !is_archived(data)
            })
            .count()
            + far
                .iter()
                .filter(|row| matches!(row.state, WorkState::Working | WorkState::Asking))
                .count()
            + before
                .iter()
                .filter(|start| !start.prompted || !known(start, &ids, &far_all))
                .count();
        let started = started_today
            + far_today.len()
            + before
                .iter()
                .filter(|start| !known(start, &today_ids, &far_today))
                .count();
        past_limit(limits, depth, working, started, tokens)
    }
}

fn rand_word() -> u32 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    u32::from_le_bytes(bytes)
}
