//! What each bot here is doing now (PROTOCOL.md, "What a bot is doing now"): what its sessions
//! come to, the one state it's in, and since when, as this resident saw it change.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::Value;

/// How far off a bot's next wake-up is for it to be asleep (ms).
pub const ASLEEP_AFTER: i64 = 12 * 3_600_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Idle,
    Working,
    Thinking,
    NeedsYou,
    Blocked,
    Done,
    Asleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockedBy {
    Error,
    Limit,
}

/// What a bot's sessions come to: `now` but its state.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Counts {
    pub working: u64,
    pub asking: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<u64>,
    #[serde(rename = "blockedBy", skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<BlockedBy>,
    #[serde(skip)]
    pub unseen: Option<i64>,
}

/// What the box knows of a bot beside its sessions: whether its wake-ups are paused, and its
/// next one (seconds since 1970).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rest {
    pub paused: bool,
    pub wakes: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Now {
    #[serde(flatten)]
    pub counts: Counts,
    pub state: State,
    pub since: i64,
}

fn number(value: &Value) -> Option<i64> {
    value.as_f64().map(|value| value as i64)
}

/// What each bot's sessions come to, by bot, from the session list's facts: how many are at work
/// and wait on you, and with neither when the newest last changed; how many at work think and how
/// many are blocked, what blocked the newest, and when the newest turn you haven't read ended
/// (`seen`: when you last saw each bot). Archived sessions and copies not this box's own aren't
/// counted.
pub fn counts(
    sessions: &serde_json::Map<String, Value>,
    seen: &HashMap<String, i64>,
) -> BTreeMap<String, Counts> {
    let mut bots: BTreeMap<String, Counts> = BTreeMap::new();
    let mut newest_blocked: HashMap<String, i64> = HashMap::new();
    for row in sessions.values() {
        let facts = &row["facts"];
        let Some(bot) = facts["bot"].as_str() else {
            continue;
        };
        if facts["archived"] == Value::Bool(true) || facts["away"] == Value::Bool(true) {
            continue;
        }
        let now = bots.entry(bot.to_owned()).or_default();
        let updated = number(&facts["updated"]).unwrap_or_default();
        let asking = facts.get("asked").is_some();
        let running = facts["running"] == Value::Bool(true);
        let stands = if asking {
            Some(true)
        } else if running {
            Some(false)
        } else {
            None
        };
        match stands {
            Some(true) => now.asking += 1,
            Some(false) => now.working += 1,
            None if updated != 0 => now.finished = Some(now.finished.unwrap_or(0).max(updated)),
            None => {}
        }
        if facts["kind"] == "bot" && running && facts["wakeTurn"] == Value::Bool(true) {
            now.waking = Some(true);
        }
        if stands == Some(false) && facts["thinking"] == Value::Bool(true) {
            now.thinking = Some(now.thinking.unwrap_or(0) + 1);
        }
        let seen_at = seen.get(bot).copied();
        let unread = number(&facts["unread"]);
        let news = unread.is_some_and(|unread| seen_at.is_none_or(|seen| unread > seen));
        let blocked = match facts["blocked"].as_str() {
            Some("error") => Some(BlockedBy::Error),
            Some("limit") => Some(BlockedBy::Limit),
            _ => None,
        };
        if let Some(by) = blocked
            && (facts["retrying"] == Value::Bool(true) || news)
        {
            now.blocked = Some(now.blocked.unwrap_or(0) + 1);
            if newest_blocked
                .get(bot)
                .is_none_or(|newest| updated >= *newest)
            {
                newest_blocked.insert(bot.to_owned(), updated);
                now.blocked_by = Some(by);
            }
        } else if stands.is_none() && news {
            now.unseen = Some(now.unseen.unwrap_or(0).max(unread.unwrap_or_default()));
        }
    }
    for now in bots.values_mut() {
        if now.working > 0 || now.asking > 0 {
            now.finished = None;
        }
    }
    bots
}

/// The one state a bot is in at `at` (ms), the first that holds.
pub fn state(counts: &Counts, rest: Rest, at: i64) -> State {
    if counts.asking > 0 {
        State::NeedsYou
    } else if counts.blocked.unwrap_or(0) > 0 {
        State::Blocked
    } else if counts.working > 0 {
        if counts.thinking.unwrap_or(0) >= counts.working {
            State::Thinking
        } else {
            State::Working
        }
    } else if counts.unseen.is_some() {
        State::Done
    } else if rest.paused
        || rest
            .wakes
            .is_some_and(|wakes| (wakes * 1000.0) as i64 - at > ASLEEP_AFTER)
    {
        State::Asleep
    } else {
        State::Idle
    }
}

/// When a bot's state may change with nothing else changing, after `at`: as its next wake-up comes
/// within 12 hours.
pub fn turns(rest: Rest, at: i64) -> Option<i64> {
    if rest.paused {
        return None;
    }
    let turns = (rest.wakes? * 1000.0) as i64 - ASLEEP_AFTER;
    (turns > at).then_some(turns)
}

/// Each bot's state as last said here, and since when.
#[derive(Default)]
pub struct Told {
    states: HashMap<String, (State, i64)>,
}

impl Told {
    /// `id`'s `now` at `at`, noted.
    pub fn now_of(&mut self, id: &str, counts: Counts, rest: Rest, at: i64) -> Now {
        let state = state(&counts, rest, at);
        let before = self.states.get(id);
        let since = match (state, before) {
            (State::Done, _) => counts.unseen.unwrap_or(at),
            (_, Some((was, since))) if *was == state => *since,
            _ => at,
        };
        self.states.insert(id.to_owned(), (state, since));
        Now {
            counts,
            state,
            since,
        }
    }

    /// Only these kept (a bot deleted is let go).
    pub fn keep_only(&mut self, ids: &[String]) {
        self.states.retain(|id, _| ids.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_bots_state_is_the_first_that_holds() {
        let sessions = json!({
            "ses_a": {"facts": {"kind": "bot", "bot": "bot_ada", "updated": 5, "running": true, "thinking": true}},
            "ses_b": {"facts": {"kind": "project", "bot": "bot_ada", "updated": 9, "unread": 9}},
            "ses_c": {"facts": {"kind": "project", "bot": "bot_ben", "updated": 7, "unread": 7}},
            "ses_d": {"facts": {"kind": "project", "bot": "bot_ben", "updated": 8, "unread": 8, "blocked": "limit"}},
            "ses_e": {"facts": {"kind": "project", "bot": "bot_cy", "updated": 3, "archived": true}},
        });
        let seen = HashMap::from([("bot_ben".to_owned(), 8)]);
        let counts = counts(sessions.as_object().unwrap(), &seen);
        let ada = &counts["bot_ada"];
        assert_eq!(
            (ada.working, ada.thinking, ada.unseen),
            (1, Some(1), Some(9))
        );
        assert_eq!(state(ada, Rest::default(), 0), State::Thinking);
        let ben = &counts["bot_ben"];
        assert_eq!(
            (ben.blocked, ben.unseen, ben.finished),
            (None, None, Some(8))
        );
        assert_eq!(state(ben, Rest::default(), 0), State::Idle);
        assert!(!counts.contains_key("bot_cy"));
        let far = Rest {
            paused: false,
            wakes: Some(100_000.0),
        };
        assert_eq!(state(&Counts::default(), far, 0), State::Asleep);
        assert_eq!(turns(far, 0), Some(100_000_000 - ASLEEP_AFTER));
        let mut told = Told::default();
        assert_eq!(
            told.now_of("bot_x", Counts::default(), Rest::default(), 10)
                .since,
            10
        );
        assert_eq!(
            told.now_of("bot_x", Counts::default(), Rest::default(), 20)
                .since,
            10
        );
    }
}
