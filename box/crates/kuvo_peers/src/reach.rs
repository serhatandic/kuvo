//! Reach facts for peer actions and the shared standing rule for rows drawn from copies.

use std::{collections::BTreeMap, io, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::kept;

pub type Millis = i64;
const CHECKED_EVERY: Millis = 300_000;
const HEARD_FOR: Millis = 30 * 60_000;
const LIVE_FOR: Millis = 60_000;
pub const SWITCH_GRACE: Millis = 10 * 60_000;
pub const COPY_QUIET_AFTER: Millis = 180_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct UpdatePresence {
    pub away_for: Option<Millis>,
    pub heard: Option<Millis>,
}

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
        // A predecessor can have recorded a newer observation on disk before
        // this resident took up its peer view. Keep that record when changing one peer.
        *self = Self::load(run);
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
        *self = Self::load(run);
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

    /// Activity uses the last heard fact, not the ordinary list copy's shorter quiet age.
    pub(crate) fn activity_away_at(&self, key: &str, heard: Millis, release: Option<ReleaseState>, now: Millis) -> Option<Millis> {
        if self.standing_of(key, self.peer_is_away(key, Some(heard), now), Some(heard), release, now).away() {
            return None;
        }
        let quiet = self.away_since(key).map_or(heard.saturating_add(HEARD_FOR + 1), |_| now);
        let since = heard.max(self.away_since(key).unwrap_or_default());
        let grace = if matches!(release, Some(ReleaseState::Ready | ReleaseState::Updating)) { SWITCH_GRACE } else { LIVE_FOR };
        let due = quiet.max(since.saturating_add(grace));
        (due > now).then_some(due)
    }

    pub(crate) fn update_presence(
        &self,
        key: &str,
        heard: Option<Millis>,
        now: Millis,
    ) -> UpdatePresence {
        let away_for = self.peer_is_away(key, heard, now).then(|| {
            heard
                .filter(|at| now.saturating_sub(*at) <= HEARD_FOR)
                .map_or(Millis::MAX, |at| {
                    now.saturating_sub(self.away_since(key).unwrap_or(at))
                })
        });
        UpdatePresence { away_for, heard }
    }

    /// The next instant at which kept reach facts can change a displayed
    /// standing without another relay event. Healthy copies renew before this.
    pub(crate) fn next_standing_change(
        &self, key: &str, seen: Option<Millis>, heard: Option<Millis>,
        release: Option<ReleaseState>, now: Millis,
    ) -> Option<Millis> {
        let away = self.away_since(key);
        let mut next = None;
        let mut add = |at: Millis| {
            if at > now { next = Some(next.map_or(at, |before: Millis| before.min(at))); }
        };
        if away.is_none() {
            if let Some(at) = seen { add(at.saturating_add(COPY_QUIET_AFTER).saturating_add(1)); }
            if let Some(at) = heard { add(at.saturating_add(HEARD_FOR).saturating_add(1)); }
        }
        if away.is_some() || seen.is_some_and(|at| now.saturating_sub(at) > COPY_QUIET_AFTER) {
            let since = seen.unwrap_or_default().max(away.unwrap_or_default());
            if since != 0 {
                let grace = if matches!(release, Some(ReleaseState::Ready | ReleaseState::Updating)) {
                    SWITCH_GRACE
                } else { LIVE_FOR };
                add(since.saturating_add(grace));
            }
        }
        next
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standing_deadline_exists_only_until_kept_facts_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut reach = Reach::default();
        let seen = 1_000_000;
        assert_eq!(reach.next_standing_change("peer", None, None, None, seen), None);
        let quiet = seen + COPY_QUIET_AFTER + 1;
        assert_eq!(reach.next_standing_change("peer", Some(seen), None, None, seen), Some(quiet));
        assert_eq!(reach.copy_standing("peer", Some(seen), None, quiet - 1), Standing::Live);
        assert!(reach.copy_standing("peer", Some(seen), None, quiet).away());
        assert_eq!(reach.next_standing_change("peer", Some(seen), None, None, quiet), None);
        let away = seen + 10_000;
        reach.unreachable(dir.path(), "peer", away).unwrap();
        assert_eq!(reach.next_standing_change("peer", Some(seen), None, Some(ReleaseState::Updating), away), Some(away + SWITCH_GRACE));
    }

    #[test]
    fn activity_standing_uses_the_actual_heard_or_reach_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let mut reach = Reach::default();
        let heard = 1_000_000;
        assert_eq!(reach.activity_away_at("peer", heard, None, heard), Some(heard + HEARD_FOR + 1));
        assert_eq!(reach.activity_away_at("peer", heard, None, heard + HEARD_FOR + 1), None);
        let dropped = heard + 10_000;
        reach.unreachable(dir.path(), "peer", dropped).unwrap();
        assert_eq!(reach.activity_away_at("peer", heard, None, dropped), Some(dropped + LIVE_FOR));
        assert_eq!(reach.activity_away_at("peer", heard, Some(ReleaseState::Updating), dropped), Some(dropped + SWITCH_GRACE));
        assert_eq!(reach.activity_away_at("peer", heard, None, dropped + LIVE_FOR), None);
    }

    #[test]
    fn brief_unreachable_drop_stays_live_and_clears_on_hearing_back() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path();
        let mut reach = Reach::default();
        let seen = 1_000_000;
        let dropped = seen + 10_000;

        reach.unreachable(run, "peerA", dropped).unwrap();
        assert_eq!(reach.away_since("peerA"), Some(dropped));
        assert_eq!(reach.last_found_away("peerA"), Some(dropped));

        // Under the live grace window, copied rows remain live rather than flapping to away.
        assert_eq!(
            reach.copy_standing("peerA", Some(seen), None, dropped + LIVE_FOR - 1),
            Standing::Live
        );
        // Updating peers keep their updating standing across the longer switch grace window.
        assert_eq!(
            reach.copy_standing(
                "peerA",
                Some(seen),
                Some(ReleaseState::Updating),
                dropped + SWITCH_GRACE - 1,
            ),
            Standing::Updating
        );
        // Once the grace period elapses without hearing back, standing transitions to away.
        assert_eq!(
            reach.copy_standing("peerA", Some(seen), None, dropped + LIVE_FOR),
            Standing::Away {
                last_seen: Some(seen)
            }
        );

        // Hearing from the peer clears both persisted reach files and restores live standing.
        assert!(reach.heard(run, "peerA").unwrap());
        assert_eq!(reach.away_since("peerA"), None);
        assert_eq!(reach.last_found_away("peerA"), None);
        assert_eq!(
            reach.copy_standing("peerA", Some(dropped), None, dropped + LIVE_FOR + 10_000),
            Standing::Live
        );
        let reloaded = Reach::load(run);
        assert_eq!(reloaded.away_since("peerA"), None);
        assert_eq!(reloaded.last_found_away("peerA"), None);
    }
}
