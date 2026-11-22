//! The one way a warning or an error is drawn: a plain row, as the Outputs popover's rows are.
//!
//! Severity shows only in the small icon's color (danger, warning, accent); a medium title says
//! what happened, one muted line says why or what next, and at most one quiet text action sits at
//! its end. No tinted box, no card in a card, no pill, no close button: a notice goes when what it
//! says is over, because whoever draws it stops drawing it.
//!
//! Two forms of the same row:
//! - [`Notice::inline`]: one line where it belongs (under a message, in an account row, under a
//!   turn, over the composer); its line wraps when it's long rather than hiding the box's words.
//! - [`Notice::listed`]: title over its line, a row in a popover (the title bar's notifications).
//!
//! The words are the box's: this draws them, and decides nothing.

use gpui::{
    AnyElement, App, ClickEvent, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    theme::{radius, space},
};

/// How serious it is: only the icon's color says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Something stopped or failed.
    Error,
    /// Something needs you, or holds things up for now.
    Warning,
    /// News you may act on (an update ready).
    Info,
}

impl Tone {
    pub fn color(self, cx: &App) -> Hsla {
        let c = &cx.theme().colors;
        match self {
            Tone::Error => c.danger,
            Tone::Warning => c.warning,
            Tone::Info => c.accent,
        }
    }

    /// The icon a notice of this tone shows unless it says another.
    pub fn icon(self) -> IconName {
        match self {
            Tone::Error | Tone::Warning => IconName::Warning,
            Tone::Info => IconName::Info,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Inline,
    Listed,
}

type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// One notice. `id` names it for the window: the row is found by `id`, its icon by `{id}-icon`, its
/// action by `{id}-action` (debug selectors, for tests and shots).
#[derive(IntoElement)]
pub struct Notice {
    id: SharedString,
    tone: Tone,
    icon: Option<IconName>,
    title: SharedString,
    detail: Option<SharedString>,
    action: Option<(SharedString, Handler, bool)>,
    trailing: Option<AnyElement>,
    form: Form,
}

impl Notice {
    /// One line where it belongs: title, then its muted line, then its action.
    pub fn inline(id: impl Into<SharedString>, tone: Tone, title: impl Into<SharedString>) -> Self {
        Self::new(id.into(), tone, title.into(), Form::Inline)
    }

    /// One line, from words said as one sentence: what happened before its first ": ", and why
    /// (the box's words) after it, as its muted line ("Its changes couldn’t be read: …").
    pub fn said(id: impl Into<SharedString>, tone: Tone, words: impl Into<SharedString>) -> Self {
        let words: SharedString = words.into();
        match words.split_once(": ") {
            Some((what, why)) if !what.trim().is_empty() && !why.trim().is_empty() => Self::inline(id, tone, what.to_owned()).detail(why.to_owned()),
            _ => Self::inline(id, tone, words),
        }
    }

    /// A row in a list of notices (a popover): title over its muted line, its action at the end.
    pub fn listed(id: impl Into<SharedString>, tone: Tone, title: impl Into<SharedString>) -> Self {
        Self::new(id.into(), tone, title.into(), Form::Listed)
    }

    fn new(id: SharedString, tone: Tone, title: SharedString, form: Form) -> Self {
        Self { id, tone, icon: None, title, detail: None, action: None, trailing: None, form }
    }

    /// Another icon than its tone's (a clock for a limit, a computer for one that's off).
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Its one muted line: why, or what next.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        let detail = detail.into();
        self.detail = (!detail.trim().is_empty()).then_some(detail);
        self
    }

    pub fn detail_opt(self, detail: Option<impl Into<SharedString>>) -> Self {
        match detail {
            Some(detail) => self.detail(detail),
            None => self,
        }
    }

    /// Its one quiet text action.
    pub fn action(mut self, label: impl Into<SharedString>, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.action = Some((label.into(), Box::new(on_click), false));
        self
    }

    /// Its action, greyed while what it does is under way.
    pub fn action_disabled(mut self, disabled: bool) -> Self {
        if let Some(action) = self.action.as_mut() {
            action.2 = disabled;
        }
        self
    }

    /// Its action as the caller draws it (a quiet button of the caller's own, found by the caller's
    /// selector), in the action's place.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl RenderOnce for Notice {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let tone = self.tone.color(cx);
        let id = self.id.clone();
        let icon_selector = SharedString::from(format!("{id}-icon"));
        let action_selector = SharedString::from(format!("{id}-action"));
        let icon = div()
            .debug_selector(move || icon_selector.to_string())
            .flex_none()
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(self.icon.unwrap_or_else(|| self.tone.icon())).size(IconSize::Nav).color(tone));
        let action: Option<AnyElement> = match (self.action, self.trailing) {
            (_, Some(trailing)) => Some(div().flex_none().child(trailing).into_any_element()),
            (Some((label, on_click, disabled)), None) => Some(
                div()
                    .debug_selector(move || action_selector.to_string())
                    .flex_none()
                    .child(Button::new(SharedString::from(format!("{id}-action")), label).ghost().size(ButtonSize::Small).disabled(disabled).on_click(on_click))
                    .into_any_element(),
            ),
            (None, None) => None,
        };
        let title = div().text_ui(TypeSize::Small).medium().text_color(c.text).child(self.title);
        let root = self.id.clone();
        match self.form {
            Form::Inline => div()
                .debug_selector(move || root.to_string())
                .w_full()
                .min_w_0()
                .py(px(4.))
                .flex()
                .items_start()
                .gap(space::S2)
                // The icon sits on the first line's middle, as the words wrap under it.
                .child(div().flex_none().h(px(20.)).flex().items_center().child(icon))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pt(px(1.))
                        .flex()
                        .flex_wrap()
                        .items_baseline()
                        .gap_x(px(6.))
                        .child(title.flex_none().max_w_full())
                        .children(self.detail.map(|detail| div().min_w_0().text_ui(TypeSize::Small).text_color(c.text_muted).child(detail))),
                )
                .when_some(action, |d, action| d.child(div().flex_none().mt(px(-2.)).child(action)))
                .into_any_element(),
            Form::Listed => div()
                .debug_selector(move || root.to_string())
                .w_full()
                .min_w_0()
                .px(space::S2)
                .py(px(7.))
                .rounded(radius::NAV)
                .hover(|d| d.bg(c.hover))
                .flex()
                .items_center()
                .gap(space::S2)
                .child(icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(title.truncate())
                        .children(self.detail.map(|detail| div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(detail))),
                )
                .children(action)
                .into_any_element(),
        }
    }
}

/// A computer's state on its name, in its own words ("Off"): muted text after a dot, never a pill.
pub fn on_name(words: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let words = words.into();
    div()
        .debug_selector(|| "computer-state-on-name".into())
        .flex_none()
        .text_ui(TypeSize::Caption)
        .text_color(cx.theme().colors.text_faint)
        .child(SharedString::from(format!("· {words}")))
}
