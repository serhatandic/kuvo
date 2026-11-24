//! The computers list measures each computer against its own pack in the signed release.

use kuvo_keeper::fleet::Candidate;
use kuvo_peers::{
    directory::{PreparedRelease, UpdateFacts},
    reach::{SWITCH_GRACE, UpdatePresence},
};
use kuvo_protocol::{
    json::{Null, Nullable},
    routes::types::{
        PostKuvoUpdateRetryStatus202 as FleetRow,
        PostKuvoUpdateRetryStatus202Object as ProgressRow,
        PostKuvoUpdateRetryStatus202ObjectState as ProgressState,
        PostKuvoUpdateRetryStatus202UpToDate as CurrentRow,
        PostKuvoUpdateRetryStatus202UpToDateLine as Line,
        PostKuvoUpdateRetryStatus202UpToDateLinePhase5 as Phase,
        PostKuvoUpdateRetryStatus202UpToDateLineTone2 as Tone,
        PostKuvoUpdateRetryStatus202UpToDateState as CurrentState,
        PostKuvoUpdateRetryStatus202Updating as SwitchingRow,
        PostKuvoUpdateRetryStatus202UpdatingStage2 as SwitchingStage,
        PostKuvoUpdateRetryStatus202UpdatingState as SwitchingState,
    },
};

use super::retry;

const QUIET_GRACE: i64 = 120_000;

pub(super) fn row(
    release: &Candidate,
    facts: &UpdateFacts,
    presence: UpdatePresence,
    can_retry: bool,
    now: i64,
) -> FleetRow {
    let version = release.version();
    let digest = facts
        .release_target
        .as_deref()
        .and_then(|target| release.pack_digest(target));
    let current = facts.kuvo.as_deref();
    let up_to_date = current.is_some() && !release.newer_than(current);
    let matches = |prepared: &PreparedRelease| {
        prepared.version == version && Some(prepared.sha256.as_str()) == digest
    };
    let ready = facts.staged.as_ref().is_some_and(matches);
    let switching = facts.switching.as_ref().is_some_and(matches);
    let phase = match presence.away_for {
        Some(age) if !up_to_date && (ready || switching) && age < SWITCH_GRACE => Phase::Switching,
        Some(age) if age >= QUIET_GRACE => Phase::Off,
        _ if up_to_date => Phase::Done,
        _ if switching => Phase::Switching,
        _ => {
            if let Some(failure) = retry::signed_failure(facts, release) {
                return failure.row(
                    facts.kuvo.clone(),
                    facts.installed_sha256.clone(),
                    can_retry && presence.away_for.is_none(),
                );
            }
            if ready {
                Phase::Ready
            } else {
                Phase::GettingReady
            }
        }
    };
    let seen = if phase == Phase::Off {
        presence.heard
    } else {
        None
    };
    let line = Some(line(phase, current, version, seen, now));

    // These protocol variants share the same release and line fields.
    macro_rules! wire_row {
        ($row:ident, $state:expr $(, $field:ident: $value:expr)* $(,)?) => {
            $row {
                can_retry: false,
                current: current.map_or(Nullable::Null(Null), |current| Nullable::Value(current.into())),
                current_sha256: facts.installed_sha256.clone(),
                fill: None,
                last_failure_at: None,
                line,
                requires_update: None,
                seen: seen.map(Into::into),
                state: $state,
                target: version.into(),
                target_sha256: digest.map(str::to_owned),
                $($field: $value,)*
            }
        };
    }
    match phase {
        Phase::Done => FleetRow::UpToDate(wire_row!(CurrentRow, CurrentState::UpToDate)),
        Phase::Switching => FleetRow::Updating(wire_row!(
            SwitchingRow,
            SwitchingState::Updating,
            stage: SwitchingStage::Switching,
            running_turns: None,
        )),
        Phase::Ready => FleetRow::Object(wire_row!(ProgressRow, ProgressState::Ready)),
        Phase::Off => FleetRow::Object(wire_row!(ProgressRow, ProgressState::Off)),
        Phase::GettingReady => {
            FleetRow::Object(wire_row!(ProgressRow, ProgressState::GettingReady))
        }
    }
}

fn line(phase: Phase, current: Option<&str>, target: &str, seen: Option<i64>, now: i64) -> Line {
    let (words, tone) = match phase {
        Phase::Done => (
            format!("{} · Up to date", short(current.unwrap_or(target))),
            Tone::Done,
        ),
        Phase::Ready => (format!("Ready for {}", short(target)), Tone::Done),
        Phase::Switching => (format!("Switching to {}…", short(target)), Tone::Working),
        Phase::GettingReady => ("Getting ready…".into(), Tone::Working),
        Phase::Off => (off_words(current, seen, now), Tone::Off),
    };
    Line {
        words,
        tone,
        phase: Some(phase),
        fill: None,
        short: None,
    }
}

pub(super) fn short(version: &str) -> &str {
    version.split_once('-').map_or(version, |(_, short)| short)
}

fn off_words(current: Option<&str>, seen: Option<i64>, now: i64) -> String {
    let age = seen.map(|at| now.saturating_sub(at).max(0) / 60_000);
    let words = match age {
        None => "Can’t be reached: updates when it’s back".into(),
        Some(0) => "Seen just now".into(),
        Some(minutes @ 1..=59) => format!("Seen {minutes} min ago"),
        Some(minutes @ 60..=1439) => format!("Seen {}h ago", minutes / 60),
        Some(1440..=2879) => "Seen yesterday".into(),
        Some(minutes) => format!("Seen {} days ago", minutes / 1440),
    };
    match (current, age) {
        (Some(version), Some(_)) => format!("{} · s{}", short(version), &words[1..]),
        (Some(version), None) => format!("{} · {words}", short(version)),
        (None, _) => words,
    }
}
