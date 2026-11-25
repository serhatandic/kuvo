//! One accepted reading drives both notice transitions and the menu. Rejected observations
//! never affect either, including the pace trail.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MINUTE: u64 = 60_000;
const JITTER: u64 = 5 * MINUTE;
const DROP: f64 = 0.02;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Turn,
    Usage,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub id: String,
    pub used: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<super::plan_facts::Amount>,
    pub read: u64,
    pub source: Source,
}
impl Window {
    fn valid(&self) -> bool {
        !self.id.is_empty() && self.used.is_finite()
    }
    pub fn expired(&self, now: u64) -> bool {
        self.resets_at.is_some_and(|at| at <= now)
    }
    pub fn title(&self) -> String {
        if let Some(name) = self.name.as_ref().or(self.model.as_ref()) {
            return name.clone();
        }
        match self.minutes {
            Some(300) => "Session".into(),
            Some(10080) => "Weekly".into(),
            Some(40000..=45000) => "Monthly".into(),
            Some(n) if n >= 1440 => format!("{}-day", n / 1440),
            Some(n) if n >= 60 => format!("{}-hour", n / 60),
            _ => "Limit".into(),
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Milestone {
    UnderTenPercent,
    HealthyToClose,
    CloseToRunningOut,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Deserialize, Serialize)]
enum Pace {
    #[default]
    Unknown,
    Healthy,
    Close,
    RunsOut,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Sample {
    at: u64,
    used: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trail {
    pub current: Window,
    // An uncorroborated early refill is not a current reading.
    candidate: Option<Window>,
    samples: Vec<Sample>,
    pace: Pace,
    fired: BTreeSet<Milestone>,
}
impl Trail {
    pub fn new(mut first: Window) -> Option<Self> {
        if !first.valid() {
            return None;
        }
        first.used = first.used.clamp(0.0, 1.0);
        let mut trail = Self {
            current: first,
            candidate: None,
            samples: vec![],
            pace: Pace::Unknown,
            fired: BTreeSet::new(),
        };
        trail.note();
        Some(trail)
    }
    // A reset never goes backwards. Small forward jitter is treated as belonging to
    // the same window. A large jump requires a drop and either the expected boundary
    // passing or a second consistent reading of an early refill.
    pub fn observe(
        &mut self,
        mut next: Window,
        toggles: &BTreeSet<Milestone>,
    ) -> Option<Vec<Milestone>> {
        let old = &self.current;
        if !next.valid() || next.id != old.id || next.read <= old.read {
            return None;
        }
        next.used = next.used.clamp(0.0, 1.0);
        let mut restarted = false;
        let mut corrected = false;
        match (old.resets_at, next.resets_at) {
            (Some(before), Some(after)) if after < before => return None,
            (Some(before), Some(after)) if after > before.saturating_add(JITTER) => {
                let drop = next.used < old.used - DROP;
                let plausible_start = next.minutes.is_some_and(|minutes| {
                    let start = after.saturating_sub(u64::from(minutes) * MINUTE);
                    start <= next.read && start.saturating_add(JITTER) >= old.read
                });
                if !drop || !plausible_start {
                    self.candidate = None;
                    return None;
                }
                let corroborated = self.candidate.as_ref().is_some_and(|candidate| {
                    candidate.resets_at == Some(after)
                        && next.read >= candidate.read.saturating_add(MINUTE)
                        && next.used >= candidate.used
                        && next.used < old.used - DROP
                });
                if next.read < before && !corroborated {
                    if self
                        .candidate
                        .as_ref()
                        .is_none_or(|c| c.resets_at != Some(after) || next.used < c.used)
                    {
                        self.candidate = Some(next);
                    }
                    return None;
                }
                restarted = true;
            }
            (Some(before), _) => {
                // Small timing noise must not create a new window or change its pace.
                next.resets_at = Some(before);
                if next.used < old.used - DROP {
                    let corroborated = self.candidate.as_ref().is_some_and(|candidate| {
                        candidate.resets_at == Some(before)
                            && next.read >= candidate.read.saturating_add(MINUTE)
                            && (next.used - candidate.used).abs() <= DROP
                    });
                    if !corroborated {
                        if self.candidate.as_ref().is_none_or(|c| {
                            c.resets_at != Some(before) || (c.used - next.used).abs() > DROP
                        }) {
                            self.candidate = Some(next);
                        }
                        return None;
                    }
                    corrected = true;
                }
            }
            (None, Some(_)) => {
                self.samples.clear();
            }
            _ => {}
        }
        self.candidate = None;
        let was_under = old.used > 0.9;
        let before = self.pace;
        if restarted || corrected {
            self.samples.clear();
            self.pace = Pace::Unknown;
            if restarted {
                self.fired.clear();
            }
        }
        self.current = next;
        self.note();
        if restarted || corrected {
            return Some(vec![]);
        }
        let mut notices = vec![];
        let mut cross = |milestone, crossed| {
            if crossed && toggles.contains(&milestone) && self.fired.insert(milestone) {
                notices.push(milestone);
            }
        };
        cross(
            Milestone::UnderTenPercent,
            !was_under && self.current.used > 0.9,
        );
        cross(
            Milestone::HealthyToClose,
            before < Pace::Close && self.pace == Pace::Close,
        );
        cross(
            Milestone::CloseToRunningOut,
            before < Pace::RunsOut && self.pace == Pace::RunsOut,
        );
        Some(notices)
    }
    fn lookback(&self) -> u64 {
        self.current.minutes.map_or(60 * MINUTE, |m| {
            (u64::from(m) * MINUTE / 5).clamp(30 * MINUTE, 1440 * MINUTE)
        })
    }
    fn note(&mut self) {
        let stretch = self.lookback();
        let now = self.current.read;
        if self.current.expired(now) {
            self.pace = Pace::Unknown;
            return;
        }
        let bucket = |at| at / (stretch / 24);
        if self.samples.len() >= 2 && bucket(self.samples[self.samples.len() - 2].at) == bucket(now)
        {
            self.samples.pop();
        }
        self.samples.push(Sample {
            at: now,
            used: self.current.used,
        });
        self.samples
            .retain(|s| s.at >= now.saturating_sub(2 * stretch));
        self.pace = self.project();
    }
    fn project(&self) -> Pace {
        let window = &self.current;
        let Some(reset) = window.resets_at else {
            return Pace::Unknown;
        };
        let Some(period) = window
            .minutes
            .map(|m| u64::from(m) * MINUTE)
            .filter(|p| *p > 0)
        else {
            return Pace::Unknown;
        };
        if window.expired(window.read) {
            return Pace::Unknown;
        }
        if window.used >= 0.995 {
            return Pace::RunsOut;
        }
        // Estimate only from real samples of this accepted window, never previous windows
        // nor fictitious usage at the beginning of a time period.
        let stretch = self.lookback();
        let start = reset.saturating_sub(period);
        let mut recent = self
            .samples
            .iter()
            .filter(|s| s.at >= start && s.at >= window.read.saturating_sub(stretch));
        let Some(first) = recent.next() else {
            return Pace::Unknown;
        };
        if recent.count() < 2 || window.read - first.at < (15 * MINUTE).max(stretch / 4) {
            return Pace::Unknown;
        }
        let slope = (window.used - first.used) / (window.read - first.at) as f64;
        let projected = window.used + slope * reset.saturating_sub(window.read) as f64;
        if projected >= 1.0 {
            Pace::RunsOut
        } else if projected > 0.9 {
            Pace::Close
        } else {
            Pace::Healthy
        }
    }
    /// Wake only for a known reset or an estimate of the 90% crossing.
    /// A wake confirms with the maker; it is not itself a new reading.
    pub fn check_at(&self) -> Option<u64> {
        let reset = self.current.resets_at?;
        let mut due = reset.saturating_add(MINUTE);
        if let Some(candidate) = &self.candidate {
            due = due.min(candidate.read.saturating_add(MINUTE));
        }
        let window = &self.current;
        let stretch = self.lookback();
        if window.used < 0.9
            && self.projected()
            && !self.fired.contains(&Milestone::UnderTenPercent)
        {
            if let Some(first) = self
                .samples
                .iter()
                .find(|s| s.at >= window.read.saturating_sub(stretch) && s.at < window.read)
            {
                let slope = (window.used - first.used) / (window.read - first.at) as f64;
                if slope > 0.0 {
                    let crossing = window
                        .read
                        .saturating_add(((0.9 - window.used) / slope).ceil() as u64);
                    if crossing > window.read && crossing < reset {
                        due = due.min(crossing);
                    }
                }
            }
        }
        Some(due)
    }

    pub fn runs_out(&self) -> bool {
        self.pace == Pace::RunsOut
    }
    pub fn projected(&self) -> bool {
        matches!(self.pace, Pace::Healthy | Pace::Close | Pace::RunsOut)
    }
}
