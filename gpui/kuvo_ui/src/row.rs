//! Rows: the sidebar's navigation rows and the list rows of cards and pages.
//!
//! A row shows a wash under the pointer, a stronger one while pressed, and a filled
//! background when it's the one selected. Its trailing slot holds a count badge, a time, or
//! whatever the row needs; long labels end in an ellipsis rather than wrapping.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder as _, px, relative,
};

use crate::{
    icon::{Icon, IconName, IconSize},
    theme::{ActiveTheme, StyledType, TextRole, TypeSize, radius, space},
};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// How a row is shown without the pointer, for the gallery and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Hovered,
    Pressed,
}

/// A sidebar navigation row: a 14 px icon or leading element, a label, and a trailing slot.
#[derive(IntoElement)]
pub struct NavRow {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    /// Shown only while the pointer is over the row (a "more" button, say).
    hover_trailing: Option<AnyElement>,
    selected: bool,
    /// Where the keyboard is, walking the list: its ring and a wash.
    cursor: bool,
    disabled: bool,
    muted: bool,
    indent: usize,
    shown: Option<RowState>,
    on_click: Option<ClickHandler>,
}

impl NavRow {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            leading: None,
            trailing: None,
            hover_trailing: None,
            selected: false,
            cursor: false,
            disabled: false,
            muted: false,
            indent: 0,
            shown: None,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Anything in place of the icon: an avatar, a status dot.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    pub fn hover_trailing(mut self, element: impl IntoElement) -> Self {
        self.hover_trailing = Some(element.into_any_element());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Draws the label in the second text level (read, finished, or less important rows).
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// Nests the row under the one above it, one step per level.
    pub fn indent(mut self, level: usize) -> Self {
        self.indent = level;
        self
    }

    pub fn shown(mut self, state: RowState) -> Self {
        self.shown = Some(state);
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Marks it as where the keyboard is in its list (arrows walk it, Return opens it).
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }
}

/// The height of a navigation row: a 13 px label with room around it, and a target well over
/// 28 px.
pub const NAV_ROW_HEIGHT: Pixels = px(30.);

impl RenderOnce for NavRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = &cx.theme().colors;
        let (hover, pressed, selected_bg, accent) = (c.hover, c.pressed, c.selected, c.accent);
        let text = if self.disabled {
            c.text_faint
        } else if self.selected || !self.muted {
            c.text
        } else {
            c.text_muted
        };
        // Selected is told by the row's wash, not a heavier icon: its icon only steps up
        // (`icon_on`), never to the text's near-black.
        let icon_color = if self.disabled {
            c.text_faint
        } else if self.selected {
            c.icon_on
        } else {
            c.icon
        };
        let group: SharedString = format!("row-{:?}", self.id).into();

        let mut row = div()
            .id(self.id)
            .group(group.clone())
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(space::S2)
            .h(NAV_ROW_HEIGHT)
            .pl(space::S2 + space::S4 * self.indent as f32)
            .pr(space::S2)
            .rounded(radius::NAV)
            .border_1()
            .border_color(gpui::transparent_black())
            .text_role(TextRole::RowLabel)
            .text_color(text)
            .when(self.selected, |r| r.bg(selected_bg));
        if self.cursor {
            row = row.border_color(accent);
            if !self.selected {
                row = row.bg(hover);
            }
        }
        match self.shown {
            Some(RowState::Hovered) => row = row.bg(hover),
            Some(RowState::Pressed) => row = row.bg(pressed),
            None => {}
        }
        if !self.disabled {
            row = row
                .cursor_pointer()
                .tab_index(0)
                .when_selected_or(self.selected, selected_bg, hover)
                .when(!self.selected, |row| row.active(move |s| s.bg(pressed)))
                .focus_visible(move |s| s.border_color(accent).bg(hover));
            if let Some(handler) = self.on_click {
                row = row.on_click(move |event, window, cx| handler(event, window, cx));
            }
        }

        let leading = match (self.leading, self.icon) {
            (Some(element), _) => Some(element),
            (None, Some(icon)) => {
                Some(Icon::new(icon).size(IconSize::Nav).color(icon_color).into_any_element())
            }
            (None, None) => None,
        };

        // The end of the row: what it shows at rest, and what takes its place under the
        // pointer (the hover action sits over the resting content, so nothing shifts).
        let end = match (self.trailing, self.hover_trailing) {
            (None, None) => None,
            (Some(rest), None) => Some(div().flex().flex_none().items_center().child(rest)),
            (rest, Some(action)) => Some(
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_end()
                    .min_w(px(24.))
                    .h_full()
                    .children(rest.map(|rest| {
                        div().flex().items_center().group_hover(group.clone(), |s| s.opacity(0.)).child(rest)
                    }))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-4.))
                            .flex()
                            .items_center()
                            .opacity(0.)
                            .group_hover(group.clone(), |s| s.opacity(1.))
                            .child(action),
                    ),
            ),
        };
        // Leading things narrower than an icon (status dots) are centred in an icon's width, so
        // every label in a list starts at the same place.
        let leading = leading.map(|l| div().flex().flex_none().justify_center().min_w(IconSize::Nav.pixels()).child(l));
        row.children(leading)
            .child(div().flex_1().min_w_0().truncate().child(self.label))
            .children(end)
    }
}

/// Hover for rows: selected rows keep their fill rather than taking the lighter wash.
trait RowHover: Sized {
    fn when_selected_or(self, selected: bool, selected_bg: gpui::Hsla, hover: gpui::Hsla) -> Self;
}

impl<E: InteractiveElement + Styled> RowHover for E {
    fn when_selected_or(self, selected: bool, selected_bg: gpui::Hsla, hover: gpui::Hsla) -> Self {
        if selected { self.hover(move |s| s.bg(selected_bg)) } else { self.hover(move |s| s.bg(hover)) }
    }
}

/// A section's header in a sidebar: a quiet label, with actions that show under the pointer.
#[derive(IntoElement)]
pub struct SectionHeader {
    id: ElementId,
    label: SharedString,
    actions: Vec<AnyElement>,
}

impl SectionHeader {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self { id: id.into(), label: label.into(), actions: Vec::new() }
    }

    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = &cx.theme().colors;
        let group: SharedString = format!("section-{:?}", self.id).into();
        div()
            .id(self.id)
            .group(group.clone())
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_between()
            .gap(space::S2)
            .h(px(28.))
            .pl(space::S2)
            .pt(space::S2)
            .text_role(TextRole::Meta)
            .text_color(c.text_faint)
            .child(div().flex_1().min_w_0().truncate().child(self.label))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .children(self.actions),
            )
    }
}

/// A row in a card's list: a leading tile, a title over a line of detail, and something at the
/// end (a button, a chip, a value with a chevron). Rows in a list are divided by hairlines.
#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    title: SharedString,
    title_extra: Option<AnyElement>,
    detail: Option<SharedString>,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    shown: Option<RowState>,
    on_click: Option<ClickHandler>,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            title_extra: None,
            detail: None,
            leading: None,
            trailing: None,
            shown: None,
            on_click: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Something right after the title (a status chip).
    pub fn title_extra(mut self, element: impl IntoElement) -> Self {
        self.title_extra = Some(element.into_any_element());
        self
    }

    /// An icon in a 36 px tile, at the card size.
    pub fn icon(mut self, icon: IconName, cx: &App) -> Self {
        let c = &cx.theme().colors;
        self.leading = Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(36.))
                .rounded(radius::NAV)
                .bg(c.sunken)
                .border_1()
                .border_color(c.border)
                .child(Icon::new(icon).size(IconSize::Card).color(c.icon))
                .into_any_element(),
        );
        self
    }

    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    pub fn shown(mut self, state: RowState) -> Self {
        self.shown = Some(state);
        self
    }

    /// Makes the whole row a target (a row that opens a page or a setting).
    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ListRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = &cx.theme().colors;
        let (hover, pressed) = (c.hover, c.pressed);
        let mut row = div()
            .id(self.id)
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S4)
            .py(space::S3)
            .when(self.shown == Some(RowState::Hovered), |d| d.bg(hover))
            .when(self.shown == Some(RowState::Pressed), |d| d.bg(pressed))
            .children(self.leading.map(|l| div().flex().flex_none().items_center().child(l)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .child(div().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(self.title))
                            .children(self.title_extra.map(|e| div().flex().flex_none().items_center().child(e))),
                    )
                    .children(self.detail.map(|d| {
                        div().w_full().min_w_0().text_ui(TypeSize::Caption).text_color(c.text_muted).truncate().child(d)
                    })),
            )
            .children(self.trailing.map(|t| div().flex().flex_none().items_center().child(t)));
        if let Some(handler) = self.on_click {
            row = row
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(pressed))
                .on_click(move |e, w, cx| handler(e, w, cx));
        }
        row
    }
}

/// How a [`TextRow`]'s text behaves when it reaches the edge of its container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRowMode {
    /// Stays on one line and clips with an ellipsis inside its container.
    Truncate,
    /// Wraps onto multiple lines inside its container.
    Wrap,
}

/// A shared horizontal text row that keeps text strictly within its container's width:
/// - [`TextRow::truncated`] for single-line rows (step summaries, step rows, popover rows, status
///   lines) that clip with an ellipsis while keeping leading and trailing slots intact.
/// - [`TextRow::wrapping`] for multi-line rows (answer options, choices, descriptions) that wrap
///   cleanly inside the container and align leading elements to the first line.
#[derive(IntoElement)]
pub struct TextRow {
    id: ElementId,
    selector: Option<String>,
    mode: TextRowMode,
    text: SharedString,
    custom_body: Option<AnyElement>,
    secondary: Option<(SharedString, bool, Option<Hsla>)>,
    extra: Option<(SharedString, Option<Hsla>)>,
    leading: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
    fill: bool,
    max_lines: Option<usize>,
    gap: Pixels,
    text_size: TypeSize,
    text_color: Option<Hsla>,
    font_weight: Option<FontWeight>,
    mono: bool,
}

impl TextRow {
    /// A single-line row whose text clips with an ellipsis inside its container.
    pub fn truncated(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self::new(id, text, TextRowMode::Truncate)
    }

    /// A multi-line row whose text wraps cleanly inside its container.
    pub fn wrapping(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self::new(id, text, TextRowMode::Wrap)
    }

    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>, mode: TextRowMode) -> Self {
        Self {
            id: id.into(),
            selector: None,
            mode,
            text: text.into(),
            custom_body: None,
            secondary: None,
            extra: None,
            leading: Vec::new(),
            trailing: Vec::new(),
            fill: true,
            max_lines: None,
            gap: space::S2,
            text_size: TypeSize::Small,
            text_color: None,
            font_weight: None,
            mono: false,
        }
    }

    /// Limits a wrapping row to at most `lines` lines, ending the last line in an ellipsis when
    /// clipped (preventing GPUI's bare `line_clamp` from hard-clipping mid-word).
    pub fn max_lines(mut self, lines: usize) -> Self {
        self.max_lines = Some(lines);
        self
    }

    /// Attaches a debug selector to the row (and `<selector>-text` to its text slot) for tests.
    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    /// Keeps trailing items immediately after short text instead of pinning them to the right edge
    /// (for compact rows like step summaries and tool steps). Still truncates inside the container.
    pub fn hug(mut self) -> Self {
        self.fill = false;
        self
    }

    /// Pins trailing items to the far right edge while the text fills the space between.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = gap;
        self
    }

    pub fn text_size(mut self, size: TypeSize) -> Self {
        self.text_size = size;
        self
    }

    pub fn text_color(mut self, color: Hsla) -> Self {
        self.text_color = color.into();
        self
    }

    pub fn font_weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = Some(weight);
        self
    }

    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Adds a non-shrinking leading element (icon, keycap, spinner, status dot).
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading.push(element.into_any_element());
        self
    }

    /// Adds a non-shrinking leading element centred inside a fixed-width slot.
    pub fn leading_slot(mut self, width: Pixels, element: impl IntoElement) -> Self {
        self.leading.push(
            div().flex().flex_none().w(width).items_center().justify_center().child(element).into_any_element(),
        );
        self
    }

    /// Adds a secondary truncated text segment (such as a tool step's command or file target).
    pub fn secondary(mut self, text: impl Into<SharedString>, mono: bool, color: Option<Hsla>) -> Self {
        self.secondary = Some((text.into(), mono, color));
        self
    }

    /// Adds a third muted truncated text segment (such as a group's prior summary while running).
    pub fn extra(mut self, text: impl Into<SharedString>, color: Option<Hsla>) -> Self {
        self.extra = Some((text.into(), color));
        self
    }

    /// Replaces the text child with a custom element placed inside the row's bounded text slot.
    pub fn body(mut self, element: impl IntoElement) -> Self {
        self.custom_body = Some(element.into_any_element());
        self
    }

    /// Adds a non-shrinking trailing element (chevron, badge, count, status, action).
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    /// Builds the row as a `Stateful<Div>` so callers can chain padding, hover, click, or tooltips
    /// while keeping all width-containment invariants intact.
    pub fn into_div(self, cx: &App) -> Stateful<Div> {
        let c = &cx.theme().colors;
        let color = self.text_color.unwrap_or(c.text);
        let row_selector = self.selector.clone();
        let text_selector = self.selector.map(|s| format!("{s}-text"));
        let leading = self.leading.into_iter().map(|el| div().flex().flex_none().items_center().child(el));
        let trailing: Vec<AnyElement> = self
            .trailing
            .into_iter()
            .map(|el| div().flex().flex_none().items_center().child(el).into_any_element())
            .collect();

        let mut row = div()
            .id(self.id)
            .when_some(row_selector, |d, selector| d.debug_selector(move || selector))
            .w_full()
            .min_w_0()
            .flex()
            .gap(self.gap)
            .when(self.mono, |d| d.text_mono(self.text_size))
            .when(!self.mono, |d| d.text_ui(self.text_size))
            .text_color(color)
            .when_some(self.font_weight, |d, w| d.font_weight(w));

        match self.mode {
            TextRowMode::Truncate => {
                row = row.items_center().overflow_hidden().children(leading);
                let has_after_primary = self.secondary.is_some() || self.extra.is_some();
                let mut inner = div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(self.gap)
                    .when_some(text_selector, |d, selector| d.debug_selector(move || selector));

                if let Some(custom) = self.custom_body {
                    inner = inner.child(div().min_w_0().truncate().child(custom));
                } else if has_after_primary {
                    // Keep short leading verbs ("Ran", "Read") untruncated while still capping an
                    // unusually long primary label so it can never push the row past its container.
                    inner = inner
                        .child(
                            div()
                                .flex_none()
                                .max_w(relative(0.6))
                                .min_w_0()
                                .truncate()
                                .child(self.text),
                        )
                        .children(self.secondary.map(|(sec, sec_mono, sec_color)| {
                            div()
                                .min_w_0()
                                .truncate()
                                .when(sec_mono, |d| d.text_mono(self.text_size))
                                .when(!sec_mono, |d| d.text_ui(self.text_size))
                                .text_color(sec_color.unwrap_or(c.text_faint))
                                .child(sec)
                        }))
                        .children(self.extra.map(|(extra, extra_color)| {
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(extra_color.unwrap_or(c.text_faint))
                                .child(extra)
                        }));
                } else {
                    inner = inner.child(div().min_w_0().truncate().child(self.text));
                }

                if self.fill {
                    row.child(inner).children(trailing)
                } else {
                    row.child(inner.children(trailing))
                }
            }
            TextRowMode::Wrap => {
                let body = div()
                    .flex_1()
                    .min_w_0()
                    .when_some(self.max_lines, |d, lines| d.line_clamp(lines).text_ellipsis())
                    .when_some(text_selector, |d, selector| d.debug_selector(move || selector))
                    .map(|d| match self.custom_body {
                        Some(custom) => d.child(custom),
                        None => d.child(self.text),
                    });
                row.items_start().children(leading).child(body).children(trailing)
            }
        }
    }
}

impl RenderOnce for TextRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.into_div(cx)
    }
}

/// A card holding rows, divided by hairlines (settings, accounts, integrations).
pub fn row_card(rows: impl IntoIterator<Item = AnyElement>, cx: &App) -> impl IntoElement {
    let c = &cx.theme().colors;
    let mut card = div()
        .flex()
        .flex_col()
        .rounded(radius::CARD)
        .border_1()
        .border_color(c.border)
        .bg(c.surface)
        .overflow_hidden();
    for (ix, row) in rows.into_iter().enumerate() {
        if ix > 0 {
            card = card.child(div().h(px(1.)).mx(space::S4).bg(c.border));
        }
        card = card.child(row);
    }
    card
}
