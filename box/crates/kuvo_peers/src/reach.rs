//! Reach facts for peer actions and the shared standing rule for rows drawn from copies.

use std::{collections::BTreeMap, io, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::kept;

pub type Millis = i64;
const CHECKED_EVERY: Millis = 300_000;
const HEARD_FOR: Millis = 30 * 60_000;
const LIVE_FOR: Millis = 60_000;
const SWITCH_GRACE: Millis = 10 * 60_000;
pub const COPY_QUIET_AFTER: Millis = 180_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Standing {
    #[default]
    Live,
    Updating,
    Away {
        last_seen: Option<Millis>,
    },
}

impl Standing {
    pub fn away(self) -> bool {
        matches!(self, Self::Away { .. })
    }

    pub fn last_seen(self) -> Option<Millis> {
        match self {
            Self::Away { last_seen } => last_seen,
            _ => None,
        }
    }

    pub fn updating(self) -> Option<bool> {
        (self == Self::Updating).then_some(true)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReleaseState {
    Ready,
    Updating,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct Reach {
    away: BTreeMap<String, Number>,
    checked: BTreeMap<String, Number>,
}

impl Reach {
    pub fn load(run: &Path) -> Self {
        Self {
            away: read_times(&run.join("peer-reach.json")),
            checked: read_times(&run.join("peer-reach-checked.json")),
        }
    }

    pub fn unreachable(&mut self, run: &Path, key: &str, now: Millis) -> io::Result<()> {
        let checked = self
            .checked
            .get(key)
            .and_then(Number::as_f64)
            .unwrap_or_default();
        if now as f64 - checked > CHECKED_EVERY as f64 {
            let mut next = self.checked.clone();
            next.insert(key.into(), now.into());
            kept::write(&run.join("peer-reach-checked.json"), &next)?;
            self.checked = next;
        }
        if !self.away.contains_key(key) {
            let mut next = self.away.clone();
            next.insert(key.into(), now.into());
            kept::write(&run.join("peer-reach.json"), &next)?;
            self.away = next;
        }
        Ok(())
    }

    pub fn heard(&mut self, run: &Path, key: &str) -> io::Result<bool> {
        if self.checked.contains_key(key) {
            let mut next = self.checked.clone();
            next.remove(key);
            kept::write(&run.join("peer-reach-checked.json"), &next)?;
            self.checked = next;
        }
        let back = self.away.contains_key(key);
        if back {
            let mut next = self.away.clone();
            next.remove(key);
            kept::write(&run.join("peer-reach.json"), &next)?;
            self.away = next;
        }
        Ok(back)
    }

    pub fn away_since(&self, key: &str) -> Option<Millis> {
        observed_at(self.away.get(key)?)
    }

    pub fn last_found_away(&self, key: &str) -> Option<Millis> {
        observed_at(self.checked.get(key)?)
    }

    pub fn peer_is_away(&self, key: &str, heard: Option<Millis>, now: Millis) -> bool {
        self.away.contains_key(key) || heard.is_some_and(|at| now.saturating_sub(at) > HEARD_FOR)
    }

    pub fn copy_standing(
        &self,
        key: &str,
        seen: Option<Millis>,
        release: Option<ReleaseState>,
        now: Millis,
    ) -> Standing {
        let quiet = self.away.contains_key(key)
            || seen.is_some_and(|at| now.saturating_sub(at) > COPY_QUIET_AFTER);
        self.standing_of(key, quiet, seen, release, now)
    }

    pub fn standing_of(
        &self,
        key: &str,
        quiet: bool,
        seen: Option<Millis>,
        release: Option<ReleaseState>,
        now: Millis,
    ) -> Standing {
        if !quiet {
            return Standing::Live;
        }
        let since = seen
            .unwrap_or_default()
            .max(self.away_since(key).unwrap_or_default());
        let quiet_for = if since == 0 {
            0
        } else {
            now.saturating_sub(since)
        };
        if matches!(release, Some(ReleaseState::Ready | ReleaseState::Updating))
            && quiet_for < SWITCH_GRACE
        {
            Standing::Updating
        } else if quiet_for < LIVE_FOR {
            Standing::Live
        } else {
            Standing::Away { last_seen: seen }
        }
    }
}

fn observed_at(time: &Number) -> Option<Millis> {
    time.as_i64()
        .or_else(|| time.as_f64().map(|time| time as Millis))
}

fn read_times(path: &Path) -> BTreeMap<String, Number> {
    // Observations made here are integer milliseconds. Keep every numeric
    // entry a sibling wrote, even when this event changes a different peer.
    kept::read::<BTreeMap<String, Value>>(path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, at)| match at {
            Value::Number(at) => Some((key, at)),
            _ => None,
        })
        .collect()
}
