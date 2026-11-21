//! Your setup's update to its accepted release, as the box says it (`kuvo/computers`'
//! `fleetUpdate`, PROTOCOL.md "Fleet versions"): how each computer stands with it, and the
//! setup-wide summary the title bar's notifier shows. Every fact is the box's (its states, its
//! counts, whether it may be retried, whether you dismissed it); this only puts them in words
//! and draws them.

use gpui::{AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement, SharedString, Styled, div, px};
use kuvo_box::areas::{Computer, FleetUpdateSummary};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    small::{spinner, status_dot},
    theme::space,
};

/// The box's feature for the update routes (`POST kuvo/update/retry`, `…/dismiss`).
pub const FEATURE: &str = "update.fleet";

/// A release as people say it: "beta.38" for "0.8.0-beta.38", a plain version as it is.
pub fn short(version: &str) -> &str {
    version.split_once('-').map_or(version, |(_, pre)| pre)
}

/// How one computer stands with the setup's release.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    UpToDate { version: String },
    /// `current`: what it runs, where it said.
    Behind { current: Option<String>, target: String },
    /// `doing`: its stage in words ("waiting for 2 sessions to finish").
    Updating { target: String, doing: String },
    /// `back_to`: the version it went back to; `retry`: when it tries again, in words.
    Failed { back_to: Option<String>, retry: Option<String> },
}

/// A computer's update, as its row and page draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Standing {
    pub line: Line,
    /// Why, in the box's words (a failure's reason, or why it's behind), where it said.
    pub reason: Option<String>,
    /// The release Update Now asks for, and its pack's hash where the box names it (a retry of
    /// another pack of the same version is refused).
    pub target: String,
    pub target_sha256: Option<String>,
    /// Update Now shows: it's behind or failed, not making progress, and the box says it may be
    /// retried now (`canRetry`).
    pub can_update: bool,
    /// It's stopped behind the setup's release, the box offers no retry, and the box reports its
    /// Kuvo older than this app draws: its page can still be opened in Computers.
    pub show_computer: bool,
}

/// Draws the progress and retry control supplied by the box.
pub fn standing(computer: &Computer, now: f64) -> Option<Standing> {
    let update = computer.fleet_update.as_ref()?;
    let line = match update.state.as_str() {
        "up-to-date" => Line::UpToDate { version: update.current.clone().unwrap_or_else(|| update.target.clone()) },
        "updating" => Line::Updating { target: update.target.clone(), doing: doing(update.stage.as_deref(), update.running_turns) },
        "failed" => Line::Failed { back_to: update.rolled_back_to.clone(), retry: update.next_retry_at.map(|at| retry_words(at as f64, now)) },
        // (`behind`, and a state a newer box names that this doesn't know: not up to date.)
        _ => Line::Behind { current: update.current.clone(), target: update.target.clone() },
    };
    let stuck = matches!(line, Line::Behind { .. } | Line::Failed { .. });
    Some(Standing {
        line,
        reason: update.reason.clone().filter(|reason| !reason.is_empty()),
        target: update.target.clone(),
        target_sha256: update.target_sha256.clone(),
        can_update: update.can_retry,
        show_computer: stuck && !update.can_retry && computer.compatibility.is_older(),
    })
}

/// An update's stage in words: "downloading", "waiting for 2 sessions to finish", "switching over".
fn doing(stage: Option<&str>, running: Option<u64>) -> String {
    match (stage, running) {
        (Some("downloading"), _) => "downloading".into(),
        (Some("waiting"), Some(1)) => "waiting for 1 session to finish".into(),
        (Some("waiting"), Some(count)) if count > 1 => format!("waiting for {count} sessions to finish"),
        (Some("waiting"), _) => "waiting for its sessions to finish".into(),
        (Some("switching"), _) => "switching over".into(),
        _ => "under way".into(),
    }
}

/// When a failed update tries again: "Next retry in 14 min", or soon once that's passed.
fn retry_words(at: f64, now: f64) -> String {
    if at <= now { "Retrying soon".into() } else { format!("Next retry in {}", crate::time::until(at, now)) }
}

/// The words of a computer's update line: "beta.38 · Up to date", "beta.35 · beta.38 available",
/// "Updating to beta.38 · waiting for 2 sessions to finish", "Update failed · went back to beta.35 ·
/// Next retry in 14 min".
pub fn words(line: &Line) -> String {
    match line {
        Line::UpToDate { version } => format!("{} · Up to date", short(version)),
        Line::Behind { current: Some(current), target } => format!("{} · {} available", short(current), short(target)),
        Line::Behind { current: None, target } => format!("{} available", short(target)),
        Line::Updating { target, doing } => format!("Updating to {} · {doing}", short(target)),
        Line::Failed { back_to, retry } => {
            let mut words = "Update failed".to_owned();
            if let Some(back_to) = back_to {
                words.push_str(&format!(" · went back to {}", short(back_to)));
            }
            if let Some(retry) = retry {
                words.push_str(&format!(" · {retry}"));
            }
            words
        }
    }
}

/// A computer's update in a word or two, for the sidebar: none where it's up to date.
pub fn short_words(line: &Line) -> Option<&'static str> {
    match line {
        Line::UpToDate { .. } => None,
        Line::Behind { .. } => Some("Update available"),
        Line::Updating { .. } => Some("Updating…"),
        Line::Failed { .. } => Some("Update failed"),
    }
}

/// The colour a computer's update is drawn in: amber behind, red failed, quiet otherwise.
pub fn color(line: &Line, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    match line {
        Line::Behind { .. } => c.warning,
        Line::Failed { .. } => c.danger,
        _ => c.text_muted,
    }
}

/// The colour of a computer's tile dot where its update says more than its reach: amber behind,
/// red failed.
pub fn dot(line: &Line, cx: &App) -> Option<Hsla> {
    let c = &cx.theme().colors;
    match line {
        Line::Behind { .. } => Some(c.warning),
        Line::Failed { .. } => Some(c.danger),
        _ => None,
    }
}

/// A computer's update line, its mark before it: a check, an amber dot, a spinner, a warning.
pub fn status_line(id: &str, line: &Line, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let leading: AnyElement = match line {
        Line::UpToDate { .. } => Icon::new(IconName::Check).size(IconSize::XSmall).color(c.success).into_any_element(),
        Line::Behind { .. } => div().pt(px(2.)).child(status_dot(c.warning)).into_any_element(),
        Line::Updating { .. } => spinner(SharedString::from(format!("fleet-spin-{id}")), IconSize::XSmall, c.accent).into_any_element(),
        Line::Failed { .. } => Icon::new(IconName::Warning).size(IconSize::XSmall).color(c.danger).into_any_element(),
    };
    let selector = format!("fleet-line-{id}");
    div()
        .debug_selector(move || selector)
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap(space::S1_5)
        .text_ui(TypeSize::Small)
        .text_color(color(line, cx))
        .child(div().pt(px(3.)).flex_none().child(leading))
        .child(div().flex_1().min_w_0().child(words(line)))
        .into_any_element()
}

/// Whether the title bar shows the setup's update: while it isn't finished and you haven't
/// dismissed it (the box says both; a new release or a new failure brings it back).
pub fn header_shown(summary: Option<&FleetUpdateSummary>) -> bool {
    summary.is_some_and(|summary| !summary.dismissed && summary.stage != "finished")
}

/// How many computers the summary counts.
pub fn total(summary: &FleetUpdateSummary) -> usize {
    summary.done + summary.updating + summary.behind + summary.failed
}

/// The summary in a line: "Updating to beta.38 · 2 of 4 computers done".
pub fn summary_words(summary: &FleetUpdateSummary) -> String {
    let total = total(summary);
    format!("Updating to {} · {} of {total} {} done", short(&summary.target), summary.done, if total == 1 { "computer" } else { "computers" })
}

/// Which computers failed, in a sentence: "Build Node failed its update.", "Build Node and
/// devbox failed their updates."
pub fn failed_words(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [one] => Some(format!("{one} failed its update.")),
        [rest @ .., last] => Some(format!("{} and {last} failed their updates.", rest.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computer(json: serde_json::Value) -> Computer {
        serde_json::from_value(json).unwrap()
    }

    const NOW: f64 = 1_794_650_000_000.0;

    fn with(update: serde_json::Value) -> Computer {
        computer(serde_json::json!({ "key": "k", "name": "Build Node", "kind": "machine", "features": [FEATURE], "fleetUpdate": update }))
    }

    #[test]
    fn update_now_shows_only_for_a_computer_behind_and_not_progressing() {
        let up = with(serde_json::json!({ "current": "0.8.0-beta.38", "target": "0.8.0-beta.38", "state": "up-to-date", "canRetry": false }));
        let updating = with(serde_json::json!({ "current": "0.8.0-beta.35", "target": "0.8.0-beta.38", "state": "updating", "stage": "waiting", "runningTurns": 2, "canRetry": false }));
        let fresh = with(serde_json::json!({ "current": "0.8.0-beta.35", "target": "0.8.0-beta.38", "state": "behind", "canRetry": false }));
        let stuck = with(serde_json::json!({ "current": "0.8.0-beta.35", "target": "0.8.0-beta.38", "state": "behind", "canRetry": true }));
        let failed = with(serde_json::json!({ "current": "0.8.0-beta.35", "target": "0.8.0-beta.38", "state": "failed", "reason": "Its health check failed.", "rolledBackTo": "0.8.0-beta.35", "nextRetryAt": (NOW as u64) + 14 * 60_000, "failedAt": NOW as u64, "canRetry": true }));
        let can = |computer: &Computer| standing(computer, NOW).unwrap().can_update;
        assert!(!can(&up), "up to date");
        assert!(!can(&updating), "the box is making progress and offers no retry");
        assert!(!can(&fresh), "behind, but the box doesn't let it be retried yet");
        assert!(can(&stuck) && can(&failed));
        assert_eq!(words(&standing(&failed, NOW).unwrap().line), "Update failed · went back to beta.35 · Next retry in 14 min");
        assert_eq!(words(&standing(&updating, NOW).unwrap().line), "Updating to beta.38 · waiting for 2 sessions to finish");
        assert_eq!(words(&standing(&stuck, NOW).unwrap().line), "beta.35 · beta.38 available");
        assert_eq!(words(&standing(&up, NOW).unwrap().line), "beta.38 · Up to date");
    }

    #[test]
    fn the_status_and_retry_control_come_from_the_box() {
        let no_progress = computer(serde_json::json!({ "key": "k", "name": "old-mini", "kind": "machine", "version": "0.8.0-beta.31" }));
        assert_eq!(standing(&no_progress, NOW), None);
        let listed = with(serde_json::json!({ "current": "0.8.0-beta.31", "target": "0.8.0-beta.38",
            "state": "behind", "canRetry": false }));
        let standing_now = standing(&listed, NOW).unwrap();
        assert_eq!(words(&standing_now.line), "beta.31 · beta.38 available");
        assert!(!standing_now.can_update);
        assert!(!standing_now.show_computer, "no compatibility report: nothing more offered");
        let mut older = listed.clone();
        older.compatibility = kuvo_box::info::Compatibility::Held;
        assert!(standing(&older, NOW).unwrap().show_computer, "an older box's page can still be opened");
    }

    #[test]
    fn the_header_shows_until_finished_or_dismissed() {
        let summary = |stage: &str, dismissed: bool| FleetUpdateSummary {
            target_sha256: None, target: "0.8.0-beta.38".into(), stage: stage.into(), done: 2, updating: 1, behind: 0, failed: 1, dismissed, failures: vec![],
        };
        assert!(!header_shown(None));
        assert!(header_shown(Some(&summary("waiting", false))));
        assert!(header_shown(Some(&summary("failed", false))));
        assert!(!header_shown(Some(&summary("failed", true))), "dismissed");
        assert!(!header_shown(Some(&summary("finished", false))), "every computer is done");
        assert_eq!(summary_words(&summary("waiting", false)), "Updating to beta.38 · 2 of 4 computers done");
        assert_eq!(failed_words(&["Build Node".into()]).as_deref(), Some("Build Node failed its update."));
        assert_eq!(failed_words(&["a".into(), "b".into()]).as_deref(), Some("a and b failed their updates."));
    }
}
