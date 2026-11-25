//! One accepted reading drives both notice transitions and the menu. Rejected observations
//! never affect either, including the pace trail.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
/// The TypeScript peer state carries decisions even after their notice facts
/// are no longer within the ten-minute catch-up window.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeBucket {
    Untracked,
    Healthy,
    Close,
    RunningOut,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
    pub fired: BTreeSet<Milestone>,
    pub previous: NoticeBucket,
    pub under: bool,
    pub primed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<Milestone, u32>,
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
    #[serde(default)]
    counts: BTreeMap<Milestone, u32>,
    // A real recovery can re-arm a milestone; tiny jitter cannot.
    #[serde(default)]
    rearmed: BTreeMap<Milestone, u64>,
    #[serde(default)]
    recovering: BTreeMap<Milestone, u64>,
    #[serde(default)]
    notice_under: Option<bool>,
    #[serde(default)]
    notice_pace: Option<Pace>,
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
            counts: BTreeMap::new(),
            rearmed: BTreeMap::new(),
            recovering: BTreeMap::new(),
            notice_under: None,
            notice_pace: None,
        };
        trail.note();
        trail.notice_under = Some(trail.current.used > 0.9);
        trail.notice_pace = Some(trail.pace);
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
        let was_under = self.notice_under.unwrap_or(old.used > 0.9);
        let before = self.notice_pace.unwrap_or(self.pace);
        let now = next.read;
        let under = next.used > 0.9;
        if restarted || corrected {
            self.samples.clear();
            self.pace = Pace::Unknown;
            self.recovering.clear();
            if restarted {
                self.clear_fired();
            }
        }
        self.current = next;
        self.note();
        if restarted {
            self.notice_under = Some(under);
            self.notice_pace = Some(self.pace);
            return Some(vec![]);
        }
        if corrected {
            // Only the corroborated correction (two matching, lower reads at
            // least a minute apart) can rearm Almost Out within a window.
            if self.current.used <= 0.9 - DROP {
                self.rearm(Milestone::UnderTenPercent, now);
                self.notice_under = Some(false);
            } else if under {
                self.notice_under = Some(true);
            }
            return Some(vec![]);
        }
        for (milestone, recovered) in [
            (Milestone::HealthyToClose, self.pace == Pace::Healthy),
            (
                Milestone::CloseToRunningOut,
                matches!(self.pace, Pace::Healthy | Pace::Close),
            ),
        ] {
            if recovered && self.fired.contains(&milestone) {
                let since = *self.recovering.entry(milestone).or_insert(now);
                if now.saturating_sub(since) >= 5 * MINUTE {
                    self.rearm(milestone, now);
                }
            } else {
                self.recovering.remove(&milestone);
            }
        }
        let mut notices = vec![];
        let mut cross = |milestone, crossed| {
            if crossed && toggles.contains(&milestone) && self.fired.insert(milestone) {
                let count = self.counts.entry(milestone).or_default();
                *count = count.saturating_add(1);
                notices.push(milestone);
                true
            } else {
                false
            }
        };
        let under_fired = cross(Milestone::UnderTenPercent, !was_under && under);
        let close = cross(
            Milestone::HealthyToClose,
            before < Pace::Close && self.pace == Pace::Close,
        );
        let running = cross(
            Milestone::CloseToRunningOut,
            before < Pace::RunsOut && self.pace == Pace::RunsOut,
        );
        if under_fired
            || !under && !self.fired.contains(&Milestone::UnderTenPercent)
            || under && was_under
        {
            self.notice_under = Some(under);
        }
        if self.pace != Pace::Unknown && (self.pace <= before || close || running) {
            self.notice_pace = Some(self.pace);
        }
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
    pub fn notice_state(&self) -> NoticeState {
        let previous = match self.notice_pace.unwrap_or(self.pace) {
            Pace::Unknown => NoticeBucket::Untracked,
            Pace::Healthy => NoticeBucket::Healthy,
            Pace::Close => NoticeBucket::Close,
            Pace::RunsOut => NoticeBucket::RunningOut,
        };
        NoticeState {
            resets_at: self.current.resets_at,
            fired: self.fired.clone(),
            previous,
            under: self.notice_under.unwrap_or(self.current.used > 0.9),
            primed: true,
            read: Some(self.current.read),
            anchor: self.current.resets_at.map(|at| (at + 500) / 1000),
            counts: [
                Milestone::UnderTenPercent,
                Milestone::HealthyToClose,
                Milestone::CloseToRunningOut,
            ]
            .into_iter()
            .filter_map(|m| {
                let count = self.occurrence(m);
                (count > 0).then_some((m, count))
            })
            .collect(),
        }
    }
    pub fn occurrence(&self, m: Milestone) -> u32 {
        self.counts
            .get(&m)
            .copied()
            .unwrap_or(u32::from(self.fired.contains(&m)))
    }
    fn rearm(&mut self, m: Milestone, at: u64) {
        let count = self.occurrence(m);
        if count > 0 {
            self.counts.insert(m, count);
            self.rearmed.insert(m, at);
        }
        self.fired.remove(&m);
        self.recovering.remove(&m);
    }
    pub fn take_notice_state(&mut self, state: &NoticeState) {
        if state.primed && state.resets_at == self.current.resets_at {
            for m in [
                Milestone::UnderTenPercent,
                Milestone::HealthyToClose,
                Milestone::CloseToRunningOut,
            ] {
                let theirs = state
                    .counts
                    .get(&m)
                    .copied()
                    .unwrap_or(u32::from(state.fired.contains(&m)));
                let ours = self.occurrence(m);
                if theirs > ours {
                    self.counts.insert(m, theirs);
                    self.rearmed.remove(&m);
                    if state.fired.contains(&m) {
                        self.fired.insert(m);
                    } else {
                        self.fired.remove(&m);
                    }
                } else if theirs == ours
                    && state.fired.contains(&m)
                    && !self.rearmed.contains_key(&m)
                {
                    self.fired.insert(m);
                }
            }
        }
    }
    pub fn clear_fired(&mut self) {
        self.fired.clear();
        self.counts.clear();
        self.rearmed.clear();
        self.recovering.clear();
    }
    pub fn mark_fired_at(&mut self, milestone: Milestone, at: u64) {
        if self.rearmed.get(&milestone).is_some_and(|r| *r >= at) {
            return;
        }
        let count = self.occurrence(milestone).max(1);
        self.counts.insert(milestone, count);
        self.fired.insert(milestone);
    }
    pub fn merge_fired(&mut self, other: &Self) {
        if self.current.resets_at != other.current.resets_at {
            return;
        }
        for m in [
            Milestone::UnderTenPercent,
            Milestone::HealthyToClose,
            Milestone::CloseToRunningOut,
        ] {
            let ours = self.occurrence(m);
            let theirs = other.occurrence(m);
            if theirs > ours {
                self.counts.insert(m, theirs);
                self.rearmed.remove(&m);
                if other.fired.contains(&m) {
                    self.fired.insert(m);
                } else {
                    self.fired.remove(&m);
                }
            }
            if theirs >= ours {
                if let Some(at) = other.rearmed.get(&m) {
                    if self.rearmed.get(&m).is_none_or(|ours| at > ours) {
                        self.rearmed.insert(m, *at);
                        if !other.fired.contains(&m) {
                            self.fired.remove(&m);
                        }
                    }
                }
            }
            if theirs == ours && other.fired.contains(&m) && !self.rearmed.contains_key(&m) {
                self.fired.insert(m);
            }
        }
    }
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
