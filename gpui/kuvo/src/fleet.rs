//! Draws the fleet update state and notifier supplied by the box.

use gpui::{AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement, SharedString, Styled, div, px};
use kuvo_box::areas::{Computer, FleetUpdateSummary};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    small::{spinner, status_dot},
    theme::space,
};

#[cfg(feature = "stand-in")]
pub const FEATURE: &str = "update.fleet";

/// How a row looks, as the box says it (`tone`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Done,
    Working,
    /// It can't update for a reason you can fix.
    Warning,
    /// It can't until the next release.
    Blocked,
    Off,
}

/// How far along its way a computer is, as the box says it (`phase`). Colours only go forward:
/// grey while it gets ready, blue once ready (a spinner on it while it switches), green only when
/// it's up to date.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Getting the release ready, this far (0 to 1) where the box knows.
    GettingReady(Option<f32>),
    Ready,
    Switching,
    Done,
    Off,
}

/// A computer's row in the setup's update, in the box's words.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub words: String,
    pub tone: Tone,
    pub short: Option<String>,
    /// None where it can't update (its tone says so).
    pub phase: Option<Phase>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Standing {
    pub line: Line,
    pub target: String,
    pub target_sha256: Option<String>,
    /// Try Again is offered, as the box says (`canRetry`).
    pub can_update: bool,
}

/// How a computer's update stands, as the box says it; none where it says nothing to draw.
pub fn standing(computer: &Computer) -> Option<Standing> {
    let update = computer.fleet_update.as_ref()?;
    let line = update.line.as_ref()?;
    let tone = match line.tone.as_str() {
        "done" => Tone::Done,
        "working" => Tone::Working,
        "warning" => Tone::Warning,
        "blocked" => Tone::Blocked,
        "off" => Tone::Off,
        _ => return None,
    };
    let phase = match line.phase.as_deref() {
        Some("getting-ready") => Some(Phase::GettingReady(line.fill.map(|fill| fill as f32 / 1000.))),
        Some("ready") => Some(Phase::Ready),
        Some("switching") => Some(Phase::Switching),
        Some("done") => Some(Phase::Done),
        Some("off") => Some(Phase::Off),
        // (A box from before phases: its tone, as near as it says.)
        _ => match tone {
            Tone::Done => Some(Phase::Done),
            Tone::Working => Some(Phase::GettingReady(None)),
            Tone::Off => Some(Phase::Off),
            Tone::Warning | Tone::Blocked => None,
        },
    };
    Some(Standing {
        line: Line { words: line.words.clone(), tone, short: line.short.clone(), phase },
        can_update: update.can_retry,
        target: update.target.clone(),
        target_sha256: update.target_sha256.clone(),
    })
}

pub fn words(line: &Line) -> String {
    line.words.clone()
}

pub fn short_words(line: &Line) -> Option<&str> {
    line.short.as_deref()
}

pub fn color(line: &Line, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    dot(line, cx).unwrap_or(c.text_muted)
}

pub fn dot(line: &Line, cx: &App) -> Option<Hsla> {
    (line.tone == Tone::Warning).then(|| cx.theme().colors.warning)
}

pub fn status_line(id: &str, line: &Line, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let leading = mark(id, line, cx);
    let selector = format!("fleet-line-{id}");
    div()
        .debug_selector(move || selector)
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap(space::S1_5)
        .text_ui(TypeSize::Small)
        .text_color(if line.phase == Some(Phase::Off) { c.text_faint } else { color(line, cx) })
        .child(div().pt(px(3.)).w(px(12.)).flex_none().flex().justify_center().child(leading))
        .child(div().flex_1().min_w_0().child(words(line)))
        .into_any_element()
}

/// A row's mark: a grey spinner while it gets ready, a blue dot once ready, a blue spinner while it
/// switches, a green check only when it's up to date, an empty ring while it's off.
fn mark(id: &str, line: &Line, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let spin = |color| spinner(SharedString::from(format!("fleet-spin-{id}")), IconSize::XSmall, color).into_any_element();
    match (line.tone, line.phase) {
        (Tone::Warning, _) => Icon::new(IconName::Warning).size(IconSize::XSmall).color(c.warning).into_any_element(),
        (Tone::Blocked, _) => div().pt(px(2.)).child(status_dot(c.border_strong)).into_any_element(),
        (_, Some(Phase::Done)) => Icon::new(IconName::Check).size(IconSize::XSmall).color(c.success).into_any_element(),
        (_, Some(Phase::Ready)) => div().pt(px(2.)).child(status_dot(c.accent)).into_any_element(),
        (_, Some(Phase::Switching)) => spin(c.accent),
        (_, Some(Phase::Off) | None) => div().pt(px(2.)).child(ring(c.border_strong)).into_any_element(),
        (_, Some(Phase::GettingReady(_))) => spin(c.text_muted),
    }
}

/// An empty ring: off.
pub fn ring(color: Hsla) -> AnyElement {
    div().size(px(7.)).flex_none().rounded(kuvo_ui::theme::radius::PILL).border_1().border_color(color).into_any_element()
}

/// The colour of a computer's piece of the card's bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shade {
    /// Getting ready.
    Grey,
    /// Ready, or switching.
    Blue,
    /// Up to date.
    Green,
    /// It can't update.
    Amber,
    /// Off: the bare track.
    Track,
}

/// A computer's piece of the card's bar: how full, and in what colour (grey getting ready, blue
/// ready or switching, green up to date; amber where it can't update; the bare track while off).
pub fn segment(line: &Line) -> (f32, Shade) {
    match (line.tone, line.phase) {
        (Tone::Warning | Tone::Blocked, _) => (1., Shade::Amber),
        (_, Some(Phase::GettingReady(fill))) => (fill.unwrap_or(0.), Shade::Grey),
        (_, Some(Phase::Ready | Phase::Switching)) => (1., Shade::Blue),
        (_, Some(Phase::Done)) => (1., Shade::Green),
        (_, Some(Phase::Off) | None) => (0., Shade::Track),
    }
}

pub fn shade(shade: Shade, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    match shade {
        Shade::Grey => c.text_faint,
        Shade::Blue => c.accent,
        Shade::Green => c.success,
        Shade::Amber => c.warning,
        Shade::Track => c.border,
    }
}

pub fn header_shown(summary: Option<&FleetUpdateSummary>) -> bool {
    summary.and_then(|summary| summary.shown) == Some(true)
}

pub fn summary_words(summary: &FleetUpdateSummary) -> String {
    summary.title.clone().unwrap_or_default()
}

/// Where the setup's update is, as the box says it (`phase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupPhase {
    GettingReady,
    Ready,
    Switching,
    /// The rest up to date, some off.
    Waiting,
    Done,
}

pub fn setup_phase(summary: &FleetUpdateSummary) -> SetupPhase {
    match summary.phase.as_deref() {
        Some("ready") => SetupPhase::Ready,
        Some("switching") => SetupPhase::Switching,
        Some("waiting") => SetupPhase::Waiting,
        Some("done") => SetupPhase::Done,
        // (A box from before phases says only its stage.)
        _ => match summary.stage.as_str() {
            "switching" => SetupPhase::Switching,
            "waiting" => SetupPhase::Ready,
            _ => SetupPhase::GettingReady,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_box_controls_whether_the_fleet_notifier_shows() {
        let summary: FleetUpdateSummary = serde_json::from_value(serde_json::json!({
            "target": "0.8.0-beta.41", "stage": "finished", "done": 1, "updating": 0,
            "behind": 0, "failed": 0, "dismissed": true, "failures": [],
            "shown": true, "title": "Supplied by the box"
        })).unwrap();
        assert!(header_shown(Some(&summary)));
        assert_eq!(summary_words(&summary), "Supplied by the box");
    }
}
