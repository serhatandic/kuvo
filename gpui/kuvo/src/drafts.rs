//! Stand-in visual design drafts for Work Queue #900 (shots only; never active in production).
//!
//! - [`FaceDraft`]: three approaches to bot status without overlapping bot faces (`draft-faces-a/b/c`).
//! - [`OutputsDraft`]: three compact glance layouts for the conversation header's activity popover
//!   (`draft-outputs-a/b/c`).

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Hsla, Image, InteractiveElement, IntoElement, LineCap, ObjectFit,
    ParentElement, PathBuilder, PathStyle, Pixels, Render, SharedString, StatefulInteractiveElement,
    StrokeOptions, Styled, StyledImage, Window, canvas, div, img, point,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::areas::{Bot, BotState, Look};
use kuvo_box::artifacts::Artifact;
use kuvo_box::background::Command;
use kuvo_box::bots::WorkState;
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TextRole, TypeSize,
    bot::State as BotPoseState,
    icon::{Mark, provider_mark, provider_mark_color},
    small::status_words,
    theme::{Colors, Theme, radius, space},
};

use crate::areas::{Doing, doing_parts, two_lines};
use crate::bot_face::still_avatar;

// ---------------------------------------------------------------------------
// Problem 1: Status indicators without covering bot faces
// ---------------------------------------------------------------------------

/// Visual draft variant for bot face status indicators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceDraft {
    /// Trailing status column on sidebar rows; dedicated under-chin gutter in compact strip.
    TrailingColumn,
    /// Thin outer halo ring with generous breathing gap around a slightly smaller static face.
    BreathingHalo,
    /// Left-edge vertical pill bar for live/urgent states + inline cue on the detail line.
    EdgeBarInline,
}

impl FaceDraft {
    fn heading(self) -> (&'static str, &'static str) {
        match self {
            Self::TrailingColumn => ("Draft A · Trailing column", "Untouched face, status aligned right; under-chin gutter when compact."),
            Self::BreathingHalo => ("Draft B · Breathing halo", "Thin outer ring floating clear of the face in both full and compact views."),
            Self::EdgeBarInline => ("Draft C · Edge bar & inline cue", "Full-size face, left accent rail for urgent states, thin cue beside status text."),
        }
    }
}

/// Every distinct status a bot can present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaceStatus {
    NeedsYou,
    AtWork,
    Wake(String),
    Blocked,
    Offline,
    Idle,
}

impl FaceStatus {
    /// Classifies a local bot from the box's live facts.
    pub fn of_bot(bot: &Bot, reached: bool, now: f64) -> Self {
        if !reached {
            return Self::Offline;
        }
        if bot.now.asking > 0 || bot.now.state == BotState::NeedsYou {
            return Self::NeedsYou;
        }
        if bot.now.state == BotState::Blocked {
            return Self::Blocked;
        }
        if bot.now.working > 0 || matches!(bot.now.state, BotState::Working | BotState::Thinking) {
            return Self::AtWork;
        }
        if !bot.now.waking && !bot.paused
            && let Some(due) = bot.wakes
        {
            if let Some(short) = terse_wake(due * 1000., now) {
                return Self::Wake(short);
            }
        }
        Self::Idle
    }

    fn short_label(&self) -> String {
        match self {
            Self::NeedsYou => "Waiting on you".into(),
            Self::AtWork => "At work".into(),
            Self::Wake(when) => format!("Wakes in {when}"),
            Self::Blocked => "Blocked".into(),
            Self::Offline => "Offline".into(),
            Self::Idle => "Idle".into(),
        }
    }
}

fn terse_wake(due_ms: f64, now_ms: f64) -> Option<String> {
    let ahead = due_ms - now_ms;
    if !ahead.is_finite() || ahead <= 0. {
        return None;
    }
    let minutes = (ahead / 60_000.).ceil() as u64;
    if minutes < 60 {
        Some(format!("{}m", minutes.max(1)))
    } else if minutes < 24 * 60 {
        Some(format!("{}h", minutes / 60))
    } else {
        Some(format!("{}d", minutes / (24 * 60)))
    }
}

/// Static face that always preserves the bot's personality/expression (never animated, never morphed).
/// Offline gently lowers opacity so the bot stays recognizable without looking "randomly greyed out".
fn static_face(id: &str, look: &Look, offline: bool, size: Pixels) -> AnyElement {
    div()
        .flex_none()
        .size(size)
        .when(offline, |d| d.opacity(0.42))
        .child(still_avatar(id, look, BotPoseState::Idle, size))
        .into_any_element()
}

/// Renders a full-width Bots sidebar row under `draft`.
pub fn bot_sidebar_row(
    draft: FaceDraft,
    id: &str,
    name: &str,
    look: &Look,
    status: &FaceStatus,
    detail_words: String,
    waiting_range: Option<std::ops::Range<usize>>,
    selected: bool,
    cx: &App,
) -> AnyElement {
    let c = &cx.theme().colors;
    let offline = *status == FaceStatus::Offline;
    let (hover, selected_bg) = (c.hover, c.selected);
    let selector = format!("draft-bot-{id}");

    match draft {
        FaceDraft::TrailingColumn => {
            let face = static_face(id, look, offline, px(40.));
            let detail = div()
                .truncate()
                .text_role(TextRole::RowDetail)
                .text_color(detail_color(status, c))
                .child(status_words(detail_words, waiting_range, cx));
            let trailing = trailing_mark(status, c);
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(space::S2)
                .px(space::S2)
                .py(space::S2)
                .mb(space::S0_5)
                .rounded(radius::NAV)
                .cursor_pointer()
                .when(selected, |r| r.bg(selected_bg))
                .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
                .child(face)
                .child(two_lines(name.to_owned(), detail, c))
                .children(trailing)
                .into_any_element()
        }
        FaceDraft::BreathingHalo => {
            let avatar = halo_avatar(id, look, status, px(44.), px(32.), c);
            let detail = div()
                .truncate()
                .text_role(TextRole::RowDetail)
                .text_color(detail_color(status, c))
                .child(status_words(detail_words, waiting_range, cx));
            let wake_meta = match status {
                FaceStatus::Wake(when) => Some(
                    div()
                        .flex_none()
                        .text_role(TextRole::Meta)
                        .text_color(c.text_faint)
                        .child(when.clone()),
                ),
                _ => None,
            };
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(space::S2)
                .px(space::S2)
                .py(space::S1_5)
                .mb(space::S0_5)
                .rounded(radius::NAV)
                .cursor_pointer()
                .when(selected, |r| r.bg(selected_bg))
                .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
                .child(avatar)
                .child(two_lines(name.to_owned(), detail, c))
                .children(wake_meta)
                .into_any_element()
        }
        FaceDraft::EdgeBarInline => {
            let face = static_face(id, look, offline, px(40.));
            let bar_color = edge_bar_color(status, c);
            let inline = inline_cue(status, c);
            let words = match status {
                FaceStatus::Wake(when) => format!("Wakes in {when}"),
                _ => detail_words,
            };
            let detail = div()
                .flex()
                .items_center()
                .gap(px(5.))
                .min_w_0()
                .children(inline)
                .child(
                    div()
                        .truncate()
                        .text_role(TextRole::RowDetail)
                        .text_color(detail_color(status, c))
                        .child(status_words(words, waiting_range, cx)),
                );
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .relative()
                .flex()
                .items_center()
                .gap(space::S2)
                .pl(px(10.))
                .pr(space::S2)
                .py(space::S2)
                .mb(space::S0_5)
                .rounded(radius::NAV)
                .cursor_pointer()
                .when(selected, |r| r.bg(selected_bg))
                .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
                .children(bar_color.map(|color| {
                    div()
                        .absolute()
                        .left(px(2.))
                        .top(px(14.))
                        .w(px(3.))
                        .h(px(28.))
                        .rounded(radius::PILL)
                        .bg(color)
                }))
                .child(face)
                .child(two_lines(name.to_owned(), detail, c))
                .into_any_element()
        }
    }
}

/// Renders a compact avatar-rail tile under `draft` (never overlaps a pixel of the face).
pub fn compact_avatar_tile(
    draft: FaceDraft,
    id: &str,
    look: &Look,
    status: &FaceStatus,
    selected: bool,
    cx: &App,
) -> AnyElement {
    let c = &cx.theme().colors;
    let offline = *status == FaceStatus::Offline;
    let fill = if selected { c.selected } else { c.hover };

    match draft {
        FaceDraft::TrailingColumn => {
            // Dedicated under-chin gutter strictly below the untouched 36px face.
            let gutter_mark = compact_gutter_mark(status, c);
            div()
                .flex()
                .flex_none()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .w(px(46.))
                .h(px(54.))
                .rounded(radius::CARD)
                .when(selected, |d| d.bg(fill))
                .child(static_face(id, look, offline, px(36.)))
                .child(
                    div()
                        .h(px(10.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .children(gutter_mark),
                )
                .into_any_element()
        }
        FaceDraft::BreathingHalo => {
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(48.))
                .rounded(radius::CARD)
                .when(selected, |d| d.bg(fill))
                .child(halo_avatar(id, look, status, px(44.), px(32.), c))
                .into_any_element()
        }
        FaceDraft::EdgeBarInline => {
            let bar_color = edge_bar_color(status, c);
            let quiet_mark = match status {
                FaceStatus::Wake(_) => Some(
                    Icon::new(IconName::Clock).size(IconSize::Indicator).color(c.icon).into_any_element(),
                ),
                FaceStatus::Offline => Some(hollow_dot(px(6.), c.text_faint)),
                _ => None,
            };
            div()
                .relative()
                .flex()
                .flex_none()
                .flex_col()
                .items_center()
                .justify_center()
                .w(px(48.))
                .h(px(52.))
                .rounded(radius::CARD)
                .when(selected, |d| d.bg(fill))
                .children(bar_color.map(|color| {
                    div()
                        .absolute()
                        .left(px(1.))
                        .top(px(14.))
                        .w(px(3.))
                        .h(px(22.))
                        .rounded(radius::PILL)
                        .bg(color)
                }))
                .child(static_face(id, look, offline, px(38.)))
                .child(
                    div()
                        .h(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .children(quiet_mark),
                )
                .into_any_element()
        }
    }
}

fn detail_color(status: &FaceStatus, c: &Colors) -> Hsla {
    match status {
        FaceStatus::Blocked => c.warning,
        FaceStatus::Offline => c.text_faint,
        _ => c.text_muted,
    }
}

fn edge_bar_color(status: &FaceStatus, c: &Colors) -> Option<Hsla> {
    match status {
        FaceStatus::NeedsYou => Some(c.accent),
        FaceStatus::AtWork => Some(c.success),
        FaceStatus::Blocked => Some(c.warning),
        FaceStatus::Wake(_) | FaceStatus::Offline | FaceStatus::Idle => None,
    }
}

fn hollow_dot(size: Pixels, color: Hsla) -> AnyElement {
    div()
        .flex_none()
        .size(size)
        .rounded(radius::PILL)
        .border_1()
        .border_color(color)
        .into_any_element()
}

fn halo_dot(color: Hsla, inner: Pixels, outer: Pixels) -> AnyElement {
    div()
        .flex_none()
        .size(outer)
        .rounded(radius::PILL)
        .bg(color.opacity(0.18))
        .flex()
        .items_center()
        .justify_center()
        .child(div().size(inner).rounded(radius::PILL).bg(color))
        .into_any_element()
}

/// Right-aligned trailing status column for Draft A.
fn trailing_mark(status: &FaceStatus, c: &Colors) -> Option<AnyElement> {
    Some(match status {
        FaceStatus::NeedsYou => div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.))
            .child(div().size(px(8.)).rounded(radius::PILL).bg(c.accent))
            .into_any_element(),
        FaceStatus::AtWork => div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.))
            .child(halo_dot(c.success, px(7.), px(14.)))
            .into_any_element(),
        FaceStatus::Wake(when) => div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(3.))
            .text_role(TextRole::Meta)
            .text_color(c.text_faint)
            .child(Icon::new(IconName::Clock).size(IconSize::XSmall).color(c.icon))
            .child(when.clone())
            .into_any_element(),
        FaceStatus::Blocked => div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.))
            .child(Icon::new(IconName::Warning).size(IconSize::Nav).color(c.warning))
            .into_any_element(),
        FaceStatus::Offline => div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.))
            .child(hollow_dot(px(8.), c.text_faint))
            .into_any_element(),
        FaceStatus::Idle => return None,
    })
}

/// Tiny mark in the dedicated under-chin gutter for Draft A compact mode.
fn compact_gutter_mark(status: &FaceStatus, c: &Colors) -> Option<AnyElement> {
    Some(match status {
        FaceStatus::NeedsYou => div().size(px(6.)).rounded(radius::PILL).bg(c.accent).into_any_element(),
        FaceStatus::AtWork => halo_dot(c.success, px(5.), px(10.)),
        FaceStatus::Wake(_) => Icon::new(IconName::Clock).size(IconSize::Indicator).color(c.icon).into_any_element(),
        FaceStatus::Blocked => Icon::new(IconName::Warning).size(IconSize::Indicator).color(c.warning).into_any_element(),
        FaceStatus::Offline => hollow_dot(px(6.), c.text_faint),
        FaceStatus::Idle => return None,
    })
}

/// Tiny inline cue at the start of the detail text line for Draft C.
fn inline_cue(status: &FaceStatus, c: &Colors) -> Option<AnyElement> {
    Some(match status {
        FaceStatus::NeedsYou => div().flex_none().size(px(6.)).rounded(radius::PILL).bg(c.accent).into_any_element(),
        FaceStatus::AtWork => div().flex_none().size(px(6.)).rounded(radius::PILL).bg(c.success).into_any_element(),
        FaceStatus::Wake(_) => Icon::new(IconName::Clock).size(IconSize::Indicator).color(c.icon).into_any_element(),
        FaceStatus::Blocked => Icon::new(IconName::Warning).size(IconSize::Indicator).color(c.warning).into_any_element(),
        FaceStatus::Offline => hollow_dot(px(6.), c.text_faint),
        FaceStatus::Idle => return None,
    })
}

/// Breathing Halo Ring avatar for Draft B: static face centered with generous air inside a thin ring.
fn halo_avatar(
    id: &str,
    look: &Look,
    status: &FaceStatus,
    ring_size: Pixels,
    face_size: Pixels,
    c: &Colors,
) -> AnyElement {
    let offline = *status == FaceStatus::Offline;
    let ring_status = status.clone();
    let (accent, success, warning, muted, faint) = (c.accent, c.success, c.warning, c.text_muted, c.text_faint);
    let ring = (*status != FaceStatus::Idle).then(|| {
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let stroke = px(1.6);
                let r = (ring_size - stroke) / 2.;
                let center = point(bounds.origin.x + ring_size / 2., bounds.origin.y + ring_size / 2.);
                let style = |w: f32| {
                    PathStyle::Stroke(StrokeOptions {
                        line_width: w,
                        line_cap: LineCap::Round,
                        ..Default::default()
                    })
                };
                let pt = |angle: f32| point(center.x + r * angle.cos(), center.y + r * angle.sin());
                let full_circle = |w: f32, color: Hsla, window: &mut Window| {
                    let top = point(center.x, center.y - r);
                    let bottom = point(center.x, center.y + r);
                    let mut path = PathBuilder::stroke(px(w)).with_style(style(w));
                    path.move_to(top);
                    path.arc_to(point(r, r), px(0.), false, true, bottom);
                    path.arc_to(point(r, r), px(0.), false, true, top);
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                };
                match &ring_status {
                    FaceStatus::NeedsYou => full_circle(1.75, accent, window),
                    FaceStatus::Blocked => full_circle(1.75, warning, window),
                    FaceStatus::AtWork => {
                        // Faint full track + crisp 270-degree open arc starting from top.
                        full_circle(1.5, success.opacity(0.20), window);
                        let start = -FRAC_PI_2;
                        let sweep = TAU * 0.72;
                        let mut arc = PathBuilder::stroke(px(1.75)).with_style(style(1.75));
                        arc.move_to(pt(start));
                        arc.arc_to(point(r, r), px(0.), true, true, pt(start + sweep));
                        if let Ok(path) = arc.build() {
                            window.paint_path(path, success);
                        }
                    }
                    FaceStatus::Wake(_) => {
                        // 12 crisp clock-dial dashes around the outer halo.
                        let dashes = 12;
                        let step = TAU / dashes as f32;
                        let span = step * 0.42;
                        for i in 0..dashes {
                            let a0 = -FRAC_PI_2 + i as f32 * step;
                            let mut seg = PathBuilder::stroke(px(1.5)).with_style(style(1.5));
                            seg.move_to(pt(a0));
                            seg.arc_to(point(r, r), px(0.), false, true, pt(a0 + span));
                            if let Ok(path) = seg.build() {
                                window.paint_path(path, muted.opacity(0.70));
                            }
                        }
                    }
                    FaceStatus::Offline => {
                        // 6 sparse faint dashes for unreachable/offline.
                        let dashes = 6;
                        let step = TAU / dashes as f32;
                        let span = step * 0.36;
                        for i in 0..dashes {
                            let a0 = -FRAC_PI_2 + i as f32 * step;
                            let mut seg = PathBuilder::stroke(px(1.3)).with_style(style(1.3));
                            seg.move_to(pt(a0));
                            seg.arc_to(point(r, r), px(0.), false, true, pt(a0 + span));
                            if let Ok(path) = seg.build() {
                                window.paint_path(path, faint.opacity(0.55));
                            }
                        }
                    }
                    FaceStatus::Idle => {}
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size(ring_size)
    });

    div()
        .relative()
        .flex_none()
        .size(ring_size)
        .flex()
        .items_center()
        .justify_center()
        .child(static_face(id, look, offline, face_size))
        .children(ring)
        .into_any_element()
}

/// Companion showcase shown in the Bots main pane during `draft-faces-a/b/c` shots, so each shot
/// presents the real Bots sidebar on the left alongside all 6 states and the compact avatar strip.
pub struct FaceDraftShowcase {
    draft: FaceDraft,
}

impl FaceDraftShowcase {
    pub fn new(draft: FaceDraft) -> Self {
        Self { draft }
    }
}

struct SampleBot {
    id: &'static str,
    name: &'static str,
    look: Look,
    status: FaceStatus,
    detail: &'static str,
    waiting: bool,
}

fn showcase_samples() -> Vec<SampleBot> {
    let look = |color: &str, shape: &str, face: &str| Look {
        color: color.into(),
        shape: shape.into(),
        face: face.into(),
    };
    vec![
        SampleBot { id: "bot_mira", name: "Mira", look: look("sky", "round", "happy"), status: FaceStatus::NeedsYou, detail: "Waiting on you", waiting: true },
        SampleBot { id: "bot_inbox", name: "Inbox Helper", look: look("coral", "squircle", "curious"), status: FaceStatus::AtWork, detail: "1 at work", waiting: false },
        SampleBot { id: "bot_release", name: "Release Captain", look: look("lime", "pebble", "wink"), status: FaceStatus::Wake("3h".into()), detail: "Ships a beta every Friday", waiting: false },
        SampleBot { id: "bot_ops", name: "Ops Watcher", look: look("orange", "capsule", "bright"), status: FaceStatus::Blocked, detail: "Hit a usage limit", waiting: false },
        SampleBot { id: "bot_scout", name: "Scout", look: look("amber", "bloom", "sleepy"), status: FaceStatus::Offline, detail: "Studio Mini · Offline", waiting: false },
        SampleBot { id: "bot_docs", name: "Docs Keeper", look: look("violet", "gem", "calm"), status: FaceStatus::Idle, detail: "Finished 3h ago", waiting: false },
    ]
}

impl Render for FaceDraftShowcase {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let (title, subtitle) = self.draft.heading();
        let samples = showcase_samples();

        let states_grid = div()
            .grid()
            .grid_cols(2)
            .gap(space::S2)
            .children(samples.iter().map(|sample| {
                let waiting = sample.waiting.then_some(0..sample.detail.len());
                let row = bot_sidebar_row(
                    self.draft,
                    &format!("grid_{}", sample.id),
                    sample.name,
                    &sample.look,
                    &sample.status,
                    sample.detail.to_owned(),
                    waiting,
                    false,
                    cx,
                );
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .p(space::S2)
                    .rounded(radius::NAV)
                    .bg(c.sidebar)
                    .border_1()
                    .border_color(c.border)
                    .child(
                        div()
                            .px(space::S2)
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child(sample.status.short_label()),
                    )
                    .child(row)
            }));

        let compact_strip = div()
            .flex()
            .items_stretch()
            .gap(space::S2)
            .children(samples.iter().enumerate().map(|(ix, sample)| {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(space::S1_5)
                    .py(px(10.))
                    .px(space::S1)
                    .rounded(radius::NAV)
                    .bg(c.sidebar)
                    .border_1()
                    .border_color(c.border)
                    .child(compact_avatar_tile(
                        self.draft,
                        &format!("compact_{}", sample.id),
                        &sample.look,
                        &sample.status,
                        ix == 0,
                        cx,
                    ))
                    .child(
                        div()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_muted)
                            .truncate()
                            .child(sample.status.short_label()),
                    )
            }));

        div()
            .id("face-draft-showcase")
            .size_full()
            .overflow_y_scroll()
            .bg(c.background)
            .p(space::S6)
            .child(
                div()
                    .w_full()
                    .max_w(px(660.))
                    .mx_auto()
                    .flex()
                    .flex_col()
                    .gap(space::S4)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::S1)
                            .child(div().text_ui(TypeSize::Title).medium().text_color(c.text).child(title))
                            .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(subtitle)),
                    )
                    .child(
                        card_shell(&theme)
                            .child(section_title("All states", c))
                            .child(states_grid),
                    )
                    .child(
                        card_shell(&theme)
                            .child(section_title("Compact avatar strip", c))
                            .child(compact_strip),
                    ),
            )
    }
}

fn card_shell(theme: &Theme) -> gpui::Div {
    let c = &theme.colors;
    div()
        .flex()
        .flex_col()
        .gap(space::S3)
        .p(space::S4)
        .rounded(radius::CARD)
        .bg(c.card)
        .border_1()
        .border_color(c.border)
        .shadow(theme.card_shadow())
}

fn section_title(label: &'static str, c: &Colors) -> gpui::Div {
    div()
        .text_ui(TypeSize::Caption)
        .medium()
        .text_color(c.text_muted)
        .child(label)
}

/// Helper for `AreaView::bots_rows` to format local bot detail words cleanly.
pub fn bot_detail_parts(bot: &Bot, reached: bool, now: f64) -> (String, Option<std::ops::Range<usize>>) {
    if !reached {
        return ("Offline".into(), None);
    }
    doing_parts(&bot.now, now)
        .map(Doing::into_parts)
        .unwrap_or_else(|| (bot.job.clone(), None))
}

// ---------------------------------------------------------------------------
// Problem 2: Compact, glanceable Outputs popover
// ---------------------------------------------------------------------------

/// Visual draft variant for the conversation header's Activity / Outputs popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputsDraft {
    /// Codex-compact stacked sections: Outputs capped to 3 + inline kind pills + Subagents + Processes.
    CodexCompact,
    /// Visual media thumbnail strip for images + compact file rows + Subagents + Processes.
    MediaStrip,
    /// Kuvo white-raised-cards-on-soft-grey: top summary tiles (Subagents & Processes) + grouped Outputs.
    GlanceCards,
}

/// Minimal snapshot of an active subagent row passed from `WorkBars`.
pub struct DraftSession {
    pub id: String,
    pub title: String,
    pub state: WorkState,
    pub provider: Option<String>,
}

/// Renders the chosen Outputs popover draft. Height is strictly bounded so `anchor::place` never
/// clamps upward over the header trigger icon.
pub fn render_outputs_popover(
    draft: OutputsDraft,
    outputs: &[Artifact],
    thumbnails: &HashMap<String, Arc<Image>>,
    sessions: &[DraftSession],
    commands: &[Command],
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let max_h = px(480.).min(window.viewport_size().height - px(120.));

    match draft {
        OutputsDraft::CodexCompact => render_outputs_a(outputs, sessions, commands, max_h, &theme, cx),
        OutputsDraft::MediaStrip => render_outputs_b(outputs, thumbnails, sessions, commands, max_h, &theme, cx),
        OutputsDraft::GlanceCards => render_outputs_c(outputs, thumbnails, sessions, commands, max_h, &theme, cx),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputBucket {
    Doc,
    Image,
    Data,
}

fn bucket_of(artifact: &Artifact) -> OutputBucket {
    match artifact.kind.as_str() {
        "image" => OutputBucket::Image,
        "code" | "download" => OutputBucket::Data,
        _ if artifact.name.ends_with(".csv") || artifact.name.ends_with(".json") => OutputBucket::Data,
        _ => OutputBucket::Doc,
    }
}

fn ext_tag(name: &str) -> String {
    name.rsplit('.')
        .next()
        .filter(|ext| ext.len() <= 4 && *ext != name)
        .map(|ext| ext.to_ascii_uppercase())
        .unwrap_or_else(|| "FILE".into())
}

fn output_icon(artifact: &Artifact) -> IconName {
    match artifact.kind.as_str() {
        "image" => IconName::Image,
        "page" => IconName::Globe,
        "code" => IconName::Terminal,
        "download" => IconName::ArrowDown,
        _ => IconName::File,
    }
}

/// Middle-truncates long filenames so distinguishing suffixes (`-light.png`, `-dark.png`) stay visible.
fn middle_trim(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars || max_chars < 10 {
        return name.to_owned();
    }
    let tail = (max_chars / 3).clamp(6, 11);
    let head = max_chars.saturating_sub(tail + 1);
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}…{end}")
}

fn session_counts_label(sessions: &[DraftSession]) -> (String, usize) {
    let asking = sessions.iter().filter(|s| s.state == WorkState::Asking).count();
    let working = sessions.len().saturating_sub(asking);
    let label = match (working, asking) {
        (0, n) => format!("{n} waiting"),
        (n, 0) => format!("{n} working"),
        (n, m) => format!("{n} working · {m} waiting"),
    };
    (label, asking)
}

fn subagent_avatar(session: &DraftSession, border: Hsla, c: &Colors) -> AnyElement {
    let inner = if let Some((provider, (service, path))) = session
        .provider
        .as_deref()
        .and_then(|p| Some((p, provider_mark(p)?)))
    {
        let color = provider_mark_color(provider).or_else(|| kuvo_ui::icon::mark_color(service));
        Mark::new(path).size(px(11.)).color(color.unwrap_or(c.text)).into_any_element()
    } else {
        Icon::new(IconName::User).size(IconSize::XSmall).color(c.icon).into_any_element()
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded(radius::PILL)
        .border_2()
        .border_color(border)
        .bg(c.sunken)
        .child(inner)
        .into_any_element()
}

fn avatar_stack(sessions: &[DraftSession], border: Hsla, c: &Colors) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .children(sessions.iter().take(3).enumerate().map(|(ix, session)| {
            div()
                .flex_none()
                .when(ix > 0, |d| d.ml(px(-6.)))
                .child(subagent_avatar(session, border, c))
        }))
        .into_any_element()
}

fn popover_row(id: impl Into<SharedString>, c: &Colors) -> gpui::Stateful<gpui::Div> {
    let id = id.into();
    let hover = c.hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S2)
        .h(px(28.))
        .px(space::S2)
        .rounded(radius::NAV)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
}

fn filter_chip(label: String, active: bool, c: &Colors) -> AnyElement {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(3.))
        .rounded(radius::PILL)
        .text_ui(TypeSize::Caption)
        .when(active, |d| d.bg(c.selected).text_color(c.text).medium())
        .when(!active, |d| d.bg(c.sunken).text_color(c.text_muted))
        .child(label)
        .into_any_element()
}

/// Draft A: Codex-compact stacked sections with capped Outputs (3 latest) + kind filter pills.
fn render_outputs_a(
    outputs: &[Artifact],
    sessions: &[DraftSession],
    commands: &[Command],
    max_h: Pixels,
    theme: &Theme,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let total = outputs.len();
    let docs = outputs.iter().filter(|a| bucket_of(a) == OutputBucket::Doc).count();
    let images = outputs.iter().filter(|a| bucket_of(a) == OutputBucket::Image).count();
    let data = outputs.iter().filter(|a| bucket_of(a) == OutputBucket::Data).count();
    let (sess_label, asking) = session_counts_label(sessions);

    let outputs_section = div()
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .py(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text_muted).child("Outputs"))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(total.to_string())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.accent)
                        .child(format!("All {total}"))
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.accent)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S1)
                .px(space::S2)
                .pb(space::S1)
                .child(filter_chip(format!("All {total}"), true, c))
                .child(filter_chip(format!("Docs {docs}"), false, c))
                .child(filter_chip(format!("Images {images}"), false, c))
                .child(filter_chip(format!("Data {data}"), false, c)),
        )
        .children(outputs.iter().take(3).map(|artifact| {
            popover_row(format!("draft-a-out-{}", artifact.id), c)
                .child(Icon::new(output_icon(artifact)).size(IconSize::Nav).color(c.icon))
                .child(div().flex_1().min_w_0().truncate().child(middle_trim(&artifact.name, 29)))
                .child(
                    div()
                        .flex_none()
                        .text_mono(TypeSize::Caption)
                        .text_color(c.text_faint)
                        .child(ext_tag(&artifact.name)),
                )
        }));

    let sessions_section = (!sessions.is_empty()).then(|| {
        div()
            .mt(space::S2)
            .pt(space::S2)
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .px(space::S2)
                    .py(px(2.))
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_muted)
                    .child("Subagents"),
            )
            .child(
                popover_row("draft-a-subagents", c)
                    .child(avatar_stack(sessions, c.elevated, c))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(sess_label),
                    )
                    .children((asking > 0).then(|| div().size(px(6.)).rounded(radius::PILL).bg(c.accent)))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::Nav).color(c.text_faint)),
            )
    });

    let processes_section = (!commands.is_empty()).then(|| {
        div()
            .mt(space::S2)
            .pt(space::S2)
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .px(space::S2)
                    .py(px(2.))
                    .text_ui(TypeSize::Small)
                    .text_color(c.text_muted)
                    .child("Background processes"),
            )
            .children(commands.iter().take(3).map(|cmd| {
                let line = cmd.command.lines().next().unwrap_or("").to_owned();
                popover_row(format!("draft-a-proc-{}", cmd.id), c)
                    .child(Icon::new(IconName::TerminalWindow).size(IconSize::Nav).color(c.icon))
                    .child(div().flex_1().min_w_0().truncate().text_mono(TypeSize::Small).child(line))
                    .child(div().size(px(6.)).flex_none().rounded(radius::PILL).bg(c.success))
            }))
    });

    let _ = cx;
    div()
        .id("conversation-activity-popover")
        .debug_selector(|| "draft-outputs-a-popover".into())
        .occlude()
        .w(px(320.).min(px(320.)))
        .max_h(max_h)
        .overflow_y_scroll()
        .p(space::S3)
        .rounded(radius::POPOVER)
        .bg(c.elevated)
        .border_1()
        .border_color(c.border)
        .shadow(theme.elevated_shadow())
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .flex()
        .flex_col()
        .child(outputs_section)
        .children(sessions_section)
        .children(processes_section)
        .into_any_element()
}

/// Draft B: Visual media strip for image outputs + compact file rows + Subagents + Processes.
fn render_outputs_b(
    outputs: &[Artifact],
    thumbnails: &HashMap<String, Arc<Image>>,
    sessions: &[DraftSession],
    commands: &[Command],
    max_h: Pixels,
    theme: &Theme,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let images: Vec<&Artifact> = outputs.iter().filter(|a| bucket_of(a) == OutputBucket::Image).collect();
    let files: Vec<&Artifact> = outputs.iter().filter(|a| bucket_of(a) != OutputBucket::Image).collect();
    let (sess_label, asking) = session_counts_label(sessions);
    let shown_thumbs = 3usize;
    let more_images = images.len().saturating_sub(shown_thumbs);

    let media_strip = (!images.is_empty()).then(|| {
        div()
            .flex()
            .items_stretch()
            .gap(space::S1_5)
            .px(space::S2)
            .pt(space::S1)
            .pb(space::S2)
            .children(images.iter().take(shown_thumbs).map(|artifact| {
                let caption = short_caption(&artifact.name);
                let preview = match thumbnails.get(&artifact.id) {
                    Some(image) => img(image.clone())
                        .w(px(78.))
                        .h(px(50.))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                    None => div()
                        .w(px(78.))
                        .h(px(50.))
                        .bg(c.sunken)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Image).size(IconSize::Medium).color(c.icon))
                        .into_any_element(),
                };
                div()
                    .flex_none()
                    .w(px(78.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .w(px(78.))
                            .h(px(50.))
                            .rounded(radius::NAV)
                            .overflow_hidden()
                            .border_1()
                            .border_color(c.border)
                            .child(preview),
                    )
                    .child(
                        div()
                            .w(px(78.))
                            .truncate()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_muted)
                            .child(caption),
                    )
            }))
            .children((more_images > 0).then(|| {
                div()
                    .flex_none()
                    .w(px(58.))
                    .h(px(50.))
                    .rounded(radius::NAV)
                    .bg(c.sunken)
                    .border_1()
                    .border_color(c.border)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(div().text_ui(TypeSize::Small).medium().text_color(c.text_muted).child(format!("+{more_images}")))
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("images"))
            }))
    });

    let more_files = files.len().saturating_sub(3);
    let files_block = div()
        .flex()
        .flex_col()
        .children(files.iter().take(3).map(|artifact| {
            popover_row(format!("draft-b-file-{}", artifact.id), c)
                .child(Icon::new(output_icon(artifact)).size(IconSize::Nav).color(c.icon))
                .child(div().flex_1().min_w_0().truncate().child(middle_trim(&artifact.name, 34)))
                .child(div().flex_none().text_mono(TypeSize::Caption).text_color(c.text_faint).child(ext_tag(&artifact.name)))
        }))
        .children((more_files > 0).then(|| {
            div()
                .px(space::S2)
                .pt(px(2.))
                .text_ui(TypeSize::Caption)
                .text_color(c.text_faint)
                .child(format!("+{more_files} more files"))
        }));

    let outputs_section = div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .pb(space::S1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text_muted).child("Outputs"))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(outputs.len().to_string())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.accent)
                        .child("See all")
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.accent)),
                ),
        )
        .children(media_strip)
        .child(files_block);

    let sessions_section = (!sessions.is_empty()).then(|| {
        div()
            .mt(space::S2)
            .pt(space::S2)
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().px(space::S2).py(px(2.)).text_ui(TypeSize::Small).text_color(c.text_muted).child("Subagents"))
            .child(
                popover_row("draft-b-subagents", c)
                    .child(avatar_stack(sessions, c.elevated, c))
                    .child(div().flex_1().min_w_0().truncate().child(sess_label))
                    .children((asking > 0).then(|| div().size(px(6.)).rounded(radius::PILL).bg(c.accent)))
                    .child(Icon::new(IconName::ChevronRight).size(IconSize::Nav).color(c.text_faint)),
            )
    });

    let processes_section = (!commands.is_empty()).then(|| {
        div()
            .mt(space::S2)
            .pt(space::S2)
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().px(space::S2).py(px(2.)).text_ui(TypeSize::Small).text_color(c.text_muted).child("Background processes"))
            .children(commands.iter().take(3).map(|cmd| {
                let line = cmd.command.lines().next().unwrap_or("").to_owned();
                popover_row(format!("draft-b-proc-{}", cmd.id), c)
                    .child(Icon::new(IconName::TerminalWindow).size(IconSize::Nav).color(c.icon))
                    .child(div().flex_1().min_w_0().truncate().text_mono(TypeSize::Small).child(line))
                    .child(div().size(px(6.)).flex_none().rounded(radius::PILL).bg(c.success))
            }))
    });

    let _ = cx;
    div()
        .id("conversation-activity-popover")
        .debug_selector(|| "draft-outputs-b-popover".into())
        .occlude()
        .w(px(356.))
        .max_h(max_h)
        .overflow_y_scroll()
        .p(space::S3)
        .rounded(radius::POPOVER)
        .bg(c.elevated)
        .border_1()
        .border_color(c.border)
        .shadow(theme.elevated_shadow())
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .flex()
        .flex_col()
        .child(outputs_section)
        .children(sessions_section)
        .children(processes_section)
        .into_any_element()
}

/// Draft C: Kuvo's white-raised-cards-on-soft-grey language.
/// Live work (Subagents + Processes) pinned at the top as two side-by-side summary tiles,
/// over a white raised Outputs card grouped by kind (Documents / Images / Data).
fn render_outputs_c(
    outputs: &[Artifact],
    thumbnails: &HashMap<String, Arc<Image>>,
    sessions: &[DraftSession],
    commands: &[Command],
    max_h: Pixels,
    theme: &Theme,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let docs: Vec<&Artifact> = outputs.iter().filter(|a| bucket_of(a) == OutputBucket::Doc).collect();
    let images: Vec<&Artifact> = outputs.iter().filter(|a| bucket_of(a) == OutputBucket::Image).collect();
    let data: Vec<&Artifact> = outputs.iter().filter(|a| bucket_of(a) == OutputBucket::Data).collect();
    let (sess_label, asking) = session_counts_label(sessions);

    let tile = |theme: &Theme| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .p(px(10.))
            .rounded(px(10.))
            .bg(theme.colors.card)
            .border_1()
            .border_color(theme.colors.border)
            .shadow(theme.card_shadow())
    };

    let subagents_tile = tile(theme)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child("Subagents"))
                        .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text).child(sessions.len().to_string())),
                )
                .child(avatar_stack(sessions, c.card, c)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .children((asking > 0).then(|| div().flex_none().size(px(6.)).rounded(radius::PILL).bg(c.accent)))
                .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(c.text_muted).child(sess_label)),
        );

    let first_cmd = commands
        .first()
        .and_then(|cmd| cmd.command.lines().next())
        .unwrap_or("None running")
        .to_owned();
    let more_cmds = commands.len().saturating_sub(1);
    let processes_tile = tile(theme)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child("Processes"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .children((!commands.is_empty()).then(|| div().size(px(6.)).rounded(radius::PILL).bg(c.success)))
                        .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text).child(commands.len().to_string())),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S1_5)
                .child(Icon::new(IconName::TerminalWindow).size(IconSize::Nav).color(c.icon))
                .child(div().flex_1().min_w_0().truncate().text_mono(TypeSize::Caption).text_color(c.text).child(first_cmd))
                .children((more_cmds > 0).then(|| {
                    div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(format!("+{more_cmds}"))
                })),
        );

    let group_header = |label: &'static str, count: usize, shown: usize, c: &Colors| {
        let rest = count.saturating_sub(shown);
        div()
            .flex()
            .items_center()
            .justify_between()
            .px(space::S2)
            .pt(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S1)
                    .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child(label))
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_string())),
            )
            .children((rest > 0).then(|| {
                div().text_ui(TypeSize::Caption).text_color(c.accent).child(format!("+{rest} ›"))
            }))
    };

    let docs_group = (!docs.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(group_header("Documents", docs.len(), 2, c))
            .children(docs.iter().take(2).map(|artifact| {
                popover_row(format!("draft-c-doc-{}", artifact.id), c)
                    .child(Icon::new(IconName::File).size(IconSize::Nav).color(c.icon))
                    .child(div().flex_1().min_w_0().truncate().child(middle_trim(&artifact.name, 34)))
                    .child(div().flex_none().text_mono(TypeSize::Caption).text_color(c.text_faint).child(ext_tag(&artifact.name)))
            }))
    });

    let images_group = (!images.is_empty()).then(|| {
        let shown = 4usize;
        let rest = images.len().saturating_sub(shown);
        div()
            .pt(space::S2)
            .mt(space::S1_5)
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(space::S1_5)
            .child(group_header("Images", images.len(), shown, c))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .px(space::S2)
                    .children(images.iter().take(shown).map(|artifact| {
                        let preview = match thumbnails.get(&artifact.id) {
                            Some(image) => img(image.clone())
                                .w(px(56.))
                                .h(px(36.))
                                .object_fit(ObjectFit::Cover)
                                .into_any_element(),
                            None => div().w(px(56.)).h(px(36.)).bg(c.sunken).into_any_element(),
                        };
                        div()
                            .w(px(56.))
                            .h(px(36.))
                            .rounded(radius::SMALL)
                            .overflow_hidden()
                            .border_1()
                            .border_color(c.border)
                            .child(preview)
                    }))
                    .children((rest > 0).then(|| {
                        div()
                            .flex_1()
                            .h(px(36.))
                            .rounded(radius::SMALL)
                            .bg(c.sunken)
                            .border_1()
                            .border_color(c.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_muted)
                            .child(format!("+{rest}"))
                    })),
            )
    });

    let data_group = (!data.is_empty()).then(|| {
        div()
            .pt(space::S2)
            .mt(space::S1_5)
            .border_t_1()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(group_header("Data", data.len(), 1, c))
            .children(data.iter().take(1).map(|artifact| {
                popover_row(format!("draft-c-data-{}", artifact.id), c)
                    .child(Icon::new(output_icon(artifact)).size(IconSize::Nav).color(c.icon))
                    .child(div().flex_1().min_w_0().truncate().child(middle_trim(&artifact.name, 34)))
                    .child(div().flex_none().text_mono(TypeSize::Caption).text_color(c.text_faint).child(ext_tag(&artifact.name)))
            }))
    });

    let outputs_card = div()
        .flex()
        .flex_col()
        .p(px(10.))
        .rounded(px(10.))
        .bg(c.card)
        .border_1()
        .border_color(c.border)
        .shadow(theme.card_shadow())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .pb(space::S1_5)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child("Outputs"))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(outputs.len().to_string())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .text_ui(TypeSize::Caption)
                        .text_color(c.accent)
                        .child("Browse all")
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(c.accent)),
                ),
        )
        .children(docs_group)
        .children(images_group)
        .children(data_group);

    let shell_bg = if theme.appearance.is_dark() { c.background } else { c.sidebar };
    let _ = cx;
    div()
        .id("conversation-activity-popover")
        .debug_selector(|| "draft-outputs-c-popover".into())
        .occlude()
        .w(px(356.))
        .max_h(max_h)
        .overflow_y_scroll()
        .p(space::S2)
        .rounded(radius::POPOVER)
        .bg(shell_bg)
        .border_1()
        .border_color(c.border_strong)
        .shadow(theme.elevated_shadow())
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(
            div()
                .flex()
                .items_stretch()
                .gap(space::S2)
                .child(subagents_tile)
                .child(processes_tile),
        )
        .child(outputs_card)
        .into_any_element()
}

fn short_caption(name: &str) -> String {
    let stem = name
        .strip_prefix("screens-")
        .unwrap_or(name)
        .strip_suffix(".png")
        .or_else(|| name.strip_suffix(".svg"))
        .unwrap_or(name);
    let parts: Vec<&str> = stem.split('-').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 2 {
        return stem.to_owned();
    }
    let last = *parts.last().unwrap();
    let topic = parts
        .iter()
        .find(|&&p| !matches!(p, "computer" | "overview" | "waiting"))
        .copied()
        .unwrap_or(parts[0]);
    format!("{topic}-{last}")
}
