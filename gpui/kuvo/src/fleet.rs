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

/// A computer's row in the setup's update, in the box's words.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub words: String,
    pub tone: Tone,
    pub short: Option<String>,
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
    Some(Standing {
        line: Line { words: line.words.clone(), tone, short: line.short.clone() },
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
    let leading: AnyElement = match line.tone {
        Tone::Done => Icon::new(IconName::Check).size(IconSize::XSmall).color(c.success).into_any_element(),
        Tone::Working => spinner(SharedString::from(format!("fleet-spin-{id}")), IconSize::XSmall, c.accent).into_any_element(),
        Tone::Warning => Icon::new(IconName::Warning).size(IconSize::XSmall).color(c.warning).into_any_element(),
        Tone::Blocked | Tone::Off => div().pt(px(2.)).child(status_dot(c.border_strong)).into_any_element(),
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

pub fn header_shown(summary: Option<&FleetUpdateSummary>) -> bool {
    summary.and_then(|summary| summary.shown) == Some(true)
}

pub fn summary_words(summary: &FleetUpdateSummary) -> String {
    summary.title.clone().unwrap_or_default()
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
