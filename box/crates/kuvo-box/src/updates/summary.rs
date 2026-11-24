//! Summarize the signed release seen by the computers list and keep its settlement time.

use std::{collections::BTreeSet, path::PathBuf};

use kuvo_keeper::fleet::Candidate;
use kuvo_protocol::{
    json::Nullable,
    routes::types::{
        GetKuvoComputersStatus200FleetUpdate as Summary,
        GetKuvoComputersStatus200FleetUpdatePhase as Phase,
        GetKuvoComputersStatus200FleetUpdateStage2 as Stage, PostKuvoUpdateRetryStatus202 as Row,
        PostKuvoUpdateRetryStatus202ObjectState as State,
    },
};
use kuvo_store::{files, home::Home};
use serde::{Deserialize, Serialize};

use super::progress::short;

const SETTLED_SHOWN: i64 = 600_000;

#[derive(PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Observation {
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<i64>,
}

pub(super) struct History {
    path: PathBuf,
    observed: Option<Observation>,
}

impl History {
    pub(super) fn load(home: &Home) -> Self {
        let path = home.run().join("fleet-finished.json");
        let observed = files::read(&path)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Self { path, observed }
    }

    pub(super) fn summarize<'a>(
        &mut self,
        release: &Candidate,
        target_sha256: Option<String>,
        rows: impl IntoIterator<Item = (&'a str, &'a Row)>,
        now: i64,
    ) -> Summary {
        let mut counts = Counts::default();
        for (name, row) in rows {
            counts.include(name, row);
        }
        let phase = counts.phase();
        let warning = !counts.fixable.is_empty();
        let shown = self.shown(release.version(), phase, warning, now);
        let (title, detail) = counts.words(short(release.version()), phase);
        Summary {
            target: release.version().into(),
            target_sha256,
            done: (counts.done + counts.ready + counts.switching).into(),
            updating: counts.preparing.into(),
            behind: counts.off.len().into(),
            failed: counts.failed.into(),
            shown: Some(shown),
            phase,
            stage: counts.stage(),
            title: Some(title),
            detail: detail.filter(|_| shown),
            warning: Some(warning),
            dismissed: false,
            failures: vec![],
        }
    }

    fn shown(&mut self, target: &str, phase: Option<Phase>, warning: bool, now: i64) -> bool {
        if warning
            || matches!(
                phase,
                Some(Phase::GettingReady | Phase::Ready | Phase::Switching)
            )
        {
            self.remember(Observation {
                target: target.into(),
                finished_at: None,
            });
            return true;
        }
        let Some(observed) = self.observed.as_ref().filter(|seen| seen.target == target) else {
            return false;
        };
        if !matches!(phase, Some(Phase::Waiting | Phase::Done)) {
            return false;
        }
        if let Some(at) = observed.finished_at {
            return now.saturating_sub(at) < SETTLED_SHOWN;
        }
        self.remember(Observation {
            target: target.into(),
            finished_at: Some(now),
        });
        true
    }

    fn remember(&mut self, observed: Observation) {
        if self.observed.as_ref() == Some(&observed) {
            return;
        }
        if serde_json::to_vec(&observed)
            .is_ok_and(|bytes| files::write_secret(&self.path, &bytes).is_ok())
        {
            self.observed = Some(observed);
        }
    }
}

#[derive(Default)]
struct Counts<'a> {
    done: usize,
    ready: usize,
    switching: usize,
    preparing: usize,
    failed: usize,
    off: Vec<&'a str>,
    fixable: Vec<(&'a str, &'a str)>,
    running: BTreeSet<Option<&'a str>>,
    still: BTreeSet<Option<&'a str>>,
}

impl<'a> Counts<'a> {
    fn include(&mut self, name: &'a str, row: &'a Row) {
        let (current, in_progress) = match row {
            Row::UpToDate(_) => {
                self.done += 1;
                return;
            }
            Row::Updating(row) => {
                self.switching += 1;
                (&row.current, true)
            }
            Row::Cannot(row) => {
                self.failed += 1;
                if row.fixable {
                    self.fixable.push((name, &row.reason));
                }
                (&row.current, false)
            }
            Row::Object(row) => {
                match row.state {
                    State::Ready => self.ready += 1,
                    State::GettingReady => self.preparing += 1,
                    State::Off => self.off.push(name),
                }
                (&row.current, row.state != State::Off)
            }
            Row::Behind(row) => (&row.current, false),
            Row::Failed(row) => (&row.current, false),
        };
        let current = match current {
            Nullable::Null(_) => None,
            Nullable::Value(version) => Some(version.as_str()),
        };
        self.still.insert(current);
        if in_progress {
            self.running.insert(current);
        }
    }

    fn total(&self) -> usize {
        self.done + self.ready + self.switching + self.preparing + self.failed + self.off.len()
    }

    fn phase(&self) -> Option<Phase> {
        let phase = match (self.done, self.switching, self.ready, self.preparing) {
            (0, 0, _, 1..) => Phase::GettingReady,
            (0, 0, 1.., _) => Phase::Ready,
            (_, 1.., _, _) | (1.., _, 1.., _) | (1.., _, _, 1..) => Phase::Switching,
            (0, _, _, _) => return None,
            (done, _, _, _) if done == self.total() => Phase::Done,
            _ => Phase::Waiting,
        };
        Some(phase)
    }

    fn stage(&self) -> Stage {
        match (
            self.fixable.len(),
            self.switching,
            self.preparing,
            self.ready,
        ) {
            (1.., _, _, _) => Stage::Failed,
            (0, 1.., _, _) => Stage::Switching,
            (0, 0, 1.., _) => Stage::Downloading,
            (0, 0, 0, 1..) => Stage::Waiting,
            _ => Stage::Finished,
        }
    }

    fn words(&self, version: &str, phase: Option<Phase>) -> (String, Option<String>) {
        if let Some((name, reason)) = self.fixable.first() {
            let title = match self.fixable.len() {
                1 => format!("{name} can’t update to {version}"),
                count => format!("{count} computers can’t update to {version}"),
            };
            return (title, Some(format!("{name}: {reason}")));
        }
        let total = self.total();
        match phase {
            Some(Phase::GettingReady) => {
                let title = if self.ready > 0 {
                    format!("Getting {version} ready · {} of {total}", self.ready)
                } else {
                    format!("Getting {version} ready")
                };
                let detail = one_version(&self.running).map_or_else(
                    || "Your computers keep what they run meanwhile.".into(),
                    |current| format!("Your computers stay on {current} meanwhile."),
                );
                (title, Some(detail))
            }
            Some(Phase::Ready) => (
                format!("{version} is ready · {} of {total}", self.ready),
                Some("They switch next.".into()),
            ),
            Some(Phase::Switching) => {
                let remaining = total - self.done;
                let detail = one_version(&self.still).map_or_else(
                    || format!("{remaining} still to switch."),
                    |current| format!("{remaining} still on {current}."),
                );
                (
                    format!("Switching to {version} · {} of {total} done", self.done),
                    Some(detail),
                )
            }
            Some(Phase::Done) => {
                let title = if total == 1 {
                    format!("Up to date on {version}")
                } else {
                    format!("All {total} on {version}")
                };
                (title, None)
            }
            _ => {
                let detail = match self.off.as_slice() {
                    [] => None,
                    [name] => Some(format!("{name} updates when it’s back.")),
                    off => Some(format!("{} computers update when they’re back.", off.len())),
                };
                (
                    format!("{version} · {} of {total} up to date", self.done),
                    detail,
                )
            }
        }
    }
}

fn one_version<'a>(versions: &BTreeSet<Option<&'a str>>) -> Option<&'a str> {
    if versions.len() != 1 {
        return None;
    }
    versions.first().copied().flatten().map(short)
}
