//! What the window shows in place of its areas while the computer it shows can't be used: its Kuvo
//! is older than this app (held by the update gate) or it can't be reached. One view, in the main
//! area's middle: what's true now, in plain words, and what moves it on (Update Now, Try Again), with
//! Switch to another of your computers that works. Every fact is the box's or the gate's: this only
//! draws it (the words of an older computer's update are the box's that brings it up,
//! `Workspace::held_update`).

use gpui::{AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder as _, px};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    button::Button,
    small::spinner,
    theme::{radius, space},
};

use crate::workspace::Workspace;

/// What the view's main button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockedDo {
    /// Update Now, as the title bar's update chip offers it (asked first where the box says to).
    Update,
    /// Reach the computer's box again.
    TryAgain,
}

/// The computer the window shows can't be used now: why, and what can be done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    /// Older (`held`) or not reached (`unreached`): the view's selector.
    pub kind: &'static str,
    pub title: String,
    pub detail: String,
    /// Under way now (its update, or reaching it): a spinner, and no button.
    pub busy: bool,
    /// It failed, or has waited long: drawn with a warning.
    pub attention: bool,
    /// Its button's words and what it does (none: nothing can be done from here).
    pub action: Option<(String, BlockedDo)>,
    /// Another of your computers that works, by its key and name.
    pub switch_to: Option<(String, String)>,
    /// Why what was asked last didn't go, in the box's words.
    pub error: Option<String>,
}

/// The view, in the main area's middle.
pub fn render(blocked: &Blocked, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let kind = blocked.kind;
    let mark: AnyElement = if blocked.busy {
        spinner("blocked-spin", IconSize::Card, c.text_muted).into_any_element()
    } else {
        Icon::new(if blocked.attention || kind == "held" { IconName::Warning } else { IconName::Info }).size(IconSize::Card).color(if blocked.attention { c.warning } else { c.icon }).into_any_element()
    };
    let action = blocked.action.clone().filter(|_| !blocked.busy).map(|(label, does)| {
        div().debug_selector(|| "blocked-action".into()).child(Button::new("blocked-action", label).primary().on_click(cx.listener(move |this, _, window, cx| match does {
            BlockedDo::Update => this.blocked_update(cx),
            BlockedDo::TryAgain => this.reach_again(window, cx),
        })))
    });
    let switch = blocked.switch_to.clone().map(|(key, name)| {
        div().debug_selector(|| "blocked-switch".into()).child(Button::new("blocked-switch", format!("Switch to {name}")).on_click(cx.listener(move |this, _, _, cx| this.switch_to(&key, cx))))
    });
    div()
        .debug_selector(move || format!("blocked-{kind}"))
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(space::S3)
        .px(space::S8)
        .pb(px(64.))
        .child(div().flex().items_center().justify_center().size(px(48.)).rounded(radius::PILL).bg(c.sunken).child(mark))
        .child(div().debug_selector(|| "blocked-title".into()).max_w(px(480.)).text_center().text_ui(TypeSize::Title).medium().text_color(c.text).child(blocked.title.clone()))
        .when(!blocked.detail.is_empty(), |d| d.child(div().debug_selector(|| "blocked-detail".into()).max_w(px(440.)).text_center().text_ui(TypeSize::Body).text_color(c.text_muted).child(blocked.detail.clone())))
        .children(blocked.error.clone().map(|error| div().debug_selector(|| "blocked-error".into()).max_w(px(440.)).text_center().text_ui(TypeSize::Small).text_color(c.warning).child(error)))
        .when(action.is_some() || switch.is_some(), |d| d.child(div().pt(space::S2).flex().items_center().gap(space::S3).children(switch).children(action)))
        .into_any_element()
}

/// The words for a computer that can't be reached, where the box says nothing more: what to do.
pub fn unreached_words(name: &str) -> (String, String) {
    (format!("Can’t reach {name}"), "Make sure it’s on and connected, then Try Again.".to_owned())
}
