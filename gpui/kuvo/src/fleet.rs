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

pub fn short(version: &str) -> &str {
    version.split_once('-').map_or(version, |(_, pre)| pre)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    UpToDate { version: String },
    Ready { target: String },
    GettingReady { target: String },
    Off { current: Option<String> },
    Cannot { current: Option<String>, reason: String, fixable: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Standing {
    pub line: Line,
    pub target: String,
    pub target_sha256: Option<String>,
    pub can_update: bool,
}

pub fn standing(computer: &Computer, _now: f64) -> Option<Standing> {
    let update = computer.fleet_update.as_ref()?;
    let line = match update.state.as_str() {
        "up-to-date" => Line::UpToDate { version: update.current.clone().unwrap_or_else(|| update.target.clone()) },
        "ready" => Line::Ready { target: update.target.clone() },
        "getting-ready" | "updating" | "behind" => Line::GettingReady { target: update.target.clone() },
        "off" => Line::Off { current: update.current.clone() },
        "cannot" => Line::Cannot {
            current: update.current.clone(),
            reason: update.reason.clone().unwrap_or_default(),
            fixable: update.fixable == Some(true),
        },
        _ => return None,
    };
    Some(Standing {
        can_update: matches!(line, Line::Cannot { fixable: true, .. }) && update.can_retry,
        line,
        target: update.target.clone(),
        target_sha256: update.target_sha256.clone(),
    })
}

fn with_version(version: Option<&str>, words: &str) -> String {
    version.map_or_else(|| words.to_owned(), |version| format!("{} · {words}", short(version)))
}

pub fn words(line: &Line) -> String {
    match line {
        Line::UpToDate { version } => format!("{} · Up to date", short(version)),
        Line::Ready { target } => format!("Ready for {}", short(target)),
        Line::GettingReady { target } => format!("Getting {} ready…", short(target)),
        Line::Off { current } => with_version(current.as_deref(), "Off: updates when it’s back"),
        Line::Cannot { current, reason, fixable } => {
            let words = with_version(current.as_deref(), reason);
            if *fixable { words } else { format!("{words} · Updates with the next release") }
        }
    }
}

pub fn short_words(line: &Line) -> Option<&'static str> {
    matches!(line, Line::Cannot { fixable: true, .. }).then_some("Can’t update")
}

pub fn color(line: &Line, cx: &App) -> Hsla {
    let c = &cx.theme().colors;
    dot(line, cx).unwrap_or(c.text_muted)
}

pub fn dot(line: &Line, cx: &App) -> Option<Hsla> {
    matches!(line, Line::Cannot { fixable: true, .. }).then(|| cx.theme().colors.warning)
}

pub fn status_line(id: &str, line: &Line, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let leading: AnyElement = match line {
        Line::UpToDate { .. } | Line::Ready { .. } => Icon::new(IconName::Check).size(IconSize::XSmall).color(c.success).into_any_element(),
        Line::GettingReady { .. } => spinner(SharedString::from(format!("fleet-spin-{id}")), IconSize::XSmall, c.accent).into_any_element(),
        Line::Cannot { fixable: true, .. } => Icon::new(IconName::Warning).size(IconSize::XSmall).color(c.warning).into_any_element(),
        _ => div().pt(px(2.)).child(status_dot(c.border_strong)).into_any_element(),
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
