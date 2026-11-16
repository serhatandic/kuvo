//! Drawing the Work Queue's main view: the list's head (scope tabs, the search, the filters as
//! chips to take off), its items as rows in raised cards under each status, an item's page with its
//! chips, and the sheets over them (a new item, an item rewritten, a label made, changed, removed).

use kuvo_ui::motion::{MotionExt as _, Direction};
use gpui::{
    AnyElement, App, AppContext as _, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, anchored, deferred,
    div, img, list, point, prelude::FluentBuilder as _, px,
};
use kuvo_box::work_queue::{File, Item, ItemChange, ItemSession, Link};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    button::{Button, ButtonSize},
    markdown::Markdown,
    menu::{Menu, MenuEntry, PopoverMenu, Tooltip},
    small::{Chip, Switch, spinner},
    theme::{radius, space},
};

use super::marks::{label_chip, label_dot, priority_mark, status_mark, who_mark};
use super::{Act, DELETED, Field, NORMAL, QueueView, Row, Scope, Sheet, Shown, Target, Tone, size_words, source_name};
use crate::areas::look_tint;
use crate::bots::dim;
use crate::time;

/// The widest the page's cards grow.
const PAGE_WIDTH: f32 = 880.;

/// A window narrower than this shows less on a row (one label, no computer).
const NARROW: f32 = 900.;

/// A column as wide as the page's cards, in the middle.
fn column(body: impl IntoElement) -> gpui::Div {
    div().w_full().flex().justify_center().px(space::S8).child(div().w_full().max_w(px(PAGE_WIDTH)).child(body))
}

/// A quiet icon to press: a menu's trigger.
fn icon_trigger(id: &str, icon: IconName, cx: &App) -> gpui::Stateful<gpui::Div> {
    let c = &cx.theme().colors;
    let (selector, hover) = (id.to_owned(), c.hover);
    div()
        .id(SharedString::from(id.to_owned()))
        .debug_selector(move || selector)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(radius::NAV)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(Icon::new(icon).color(c.icon))
}

/// What names an item where its number would be: `#243`, or an issue's own name beside a link mark.
fn label(item: &Item, cx: &App) -> Option<AnyElement> {
    let c = &cx.theme().colors;
    let words = item.label()?;
    let gone = item.seen.as_ref().is_some_and(|seen| seen.gone);
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S1)
            .text_ui(TypeSize::Caption)
            .text_color(c.text_faint)
            .when(item.source.is_some(), |d| d.child(Icon::new(IconName::Link).color(c.text_faint)).when(gone, |d| d.opacity(0.5)))
            .child(words)
            .into_any_element(),
    )
}

/// A thin bar: how far, 0 to 1.
fn bar(value: f64, width: Option<f32>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let share = value.clamp(0., 1.) as f32;
    div()
        .flex_none()
        .when_some(width, |d, width| d.w(px(width)))
        .when(width.is_none(), |d| d.w_full())
        .h(px(4.))
        .rounded(radius::PILL)
        .bg(c.sunken)
        .child(div().h_full().w(gpui::relative(share)).rounded(radius::PILL).bg(c.accent))
        .into_any_element()
}

/// A card's row: rounded where it's the first or the last, a line under it otherwise.
fn card_row(first: bool, last: bool, cx: &App) -> gpui::Div {
    let c = &cx.theme().colors;
    div().w_full().bg(c.card).when(first, |d| d.rounded_t(radius::POPOVER)).when(last, |d| d.rounded_b(radius::POPOVER)).when(!last, |d| d.border_b_1().border_color(c.border))
}

impl QueueView {
    fn row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(row) = self.rows.get(ix).cloned() else { return div().into_any_element() };
        let body: AnyElement = match row {
            Row::Group { key, name, count, folded, first } => {
                let selector = format!("group-{key}");
                let act = Act::Fold(key.clone());
                div()
                    .pt(if first { space::S2 } else { space::S5 })
                    .pb(space::S1_5)
                    .child(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector)
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .h(px(40.))
                            .px(space::S3)
                            .rounded(radius::POPOVER)
                            .cursor_pointer()
                            // Folded, it's a card of its own, as a list put away.
                            .when(folded, |d| d.bg(c.card).shadow(cx.theme().card_shadow()))
                            .hover(|s| s.bg(c.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx)))
                            .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).color(c.icon))
                            .child(if key == DELETED { Icon::new(IconName::Trash).color(c.icon).into_any_element() } else { status_mark(&key, cx) })
                            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(name))
                            .children(count.map(|count| {
                                let selector = format!("group-{key}-count-{count}");
                                div().debug_selector(move || selector).text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_string())
                            })),
                    )
                    .into_any_element()
            }
            Row::Item { item, first, last, deleted, busy } => self.item_row(*item, first, last, deleted, busy, cx),
            Row::Empty(words) => div().debug_selector(|| "queue-empty".into()).h(px(160.)).flex().items_center().justify_center().text_center().text_ui(TypeSize::Body).text_color(c.text_muted).child(div().max_w(px(420.)).child(words)).into_any_element(),
            Row::Note { key, words, busy: true } => kuvo_ui::small::LoadingLabel::new(SharedString::from(key), words).into_any_element(),
            Row::Note { key, words, busy: false } => div()
                .debug_selector(move || key)
                .pt(space::S3)
                .flex()
                .items_start()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(div().flex_1().min_w_0().child(words))
                .into_any_element(),
            Row::Back(to) => div()
                .pt(space::S5)
                .flex()
                .child(
                    div()
                        .id("item-back")
                        .debug_selector(|| "item-back".into())
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .h(px(28.))
                        .pl(space::S1)
                        .pr(space::S2)
                        .ml(px(-6.))
                        .rounded(radius::NAV)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover))
                        .text_ui(TypeSize::Small)
                        .text_color(c.text_muted)
                        .on_click(cx.listener(|this, _, _, cx| this.act(Act::Back, cx)))
                        .child(Icon::new(IconName::ChevronLeft).color(c.icon))
                        .child(to),
                )
                .into_any_element(),
            Row::Head { item, busy } => self.head(*item, busy, cx),
            Row::Notice { key, tone, title, words, act } => {
                let icon = match tone {
                    Tone::Waiting => IconName::Clock,
                    Tone::Quiet => IconName::Info,
                };
                div()
                    .pt(space::S3)
                    .child(
                        div()
                            .debug_selector(move || key)
                            .flex()
                            .items_center()
                            .gap(space::S3)
                            .px(space::S5)
                            .py(space::S3)
                            .rounded(radius::POPOVER)
                            .bg(c.card)
                            .shadow(cx.theme().card_shadow())
                            .child(Icon::new(icon).color(c.icon))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(space::S0_5)
                                    .when(!title.is_empty(), |d| d.child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title)))
                                    .when(!words.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words))),
                            )
                            .children(act.map(|(label, act)| Button::new("notice-act", label).size(ButtonSize::Small).on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx))))),
                    )
                    .into_any_element()
            }
            Row::Progress { value, note } => div()
                .pt(space::S3)
                .child(
                    div()
                        .debug_selector(|| "item-progress".into())
                        .flex()
                        .flex_col()
                        .gap(space::S2)
                        .px(space::S5)
                        .py(space::S4)
                        .rounded(radius::POPOVER)
                        .bg(c.card)
                        .shadow(cx.theme().card_shadow())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S3)
                                .child(div().flex_1().min_w_0().text_ui(TypeSize::Body).text_color(c.text).child(if note.is_empty() { "Progress".to_owned() } else { note }))
                                .children(value.map(|value| div().debug_selector(|| "item-progress-value".into()).flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child(format!("{}%", (value.clamp(0., 1.) * 100.).round() as u32)))),
                        )
                        .children(value.map(|value| bar(value, None, cx))),
                )
                .into_any_element(),
            Row::Section(label) => div().pt(space::S6).pb(space::S2).text_ui(TypeSize::Body).text_color(c.text_muted).child(label).into_any_element(),
            Row::Body(text) => div().debug_selector(|| "item-body".into()).px(space::S5).py(space::S4).rounded(radius::POPOVER).bg(c.card).shadow(cx.theme().card_shadow()).child(Markdown::new("item-body", &text)).into_any_element(),
            Row::Session { session, first, last } => self.session_row(session, first, last, cx),
            Row::Link { link, first, last } => self.link_row(link, first, last, cx),
            Row::File { file, first, last } => self.file_row(file, first, last, cx),
            Row::Said { ix, who, ago, text, first, last } => card_row(first, last, cx)
                .debug_selector(move || format!("item-note-{ix}"))
                .px(space::S5)
                .py(space::S3)
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .when(!who.is_empty(), |d| d.child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(who)))
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(ago)),
                )
                .child(Markdown::new(SharedString::from(format!("item-note-{ix}")), &text))
                .into_any_element(),
            Row::Facts(words) => div().debug_selector(|| "item-facts".into()).pt(space::S5).text_ui(TypeSize::Caption).text_color(c.text_faint).child(words).into_any_element(),
            Row::End => div().h(space::S10).into_any_element(),
        };
        column(body).into_any_element()
    }

    /// One item in the list: its status, what names it, its title (and what's being done on it
    /// now), its priority, labels, who it's for, when it's due, where it is, when it last changed;
    /// it opens its page.
    fn item_row(&mut self, item: Item, first: bool, last: bool, deleted: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let id = item.id.clone();
        let narrow = self.viewport.width < px(NARROW);
        // What's being done on it now, as the box says: its progress note, else the session at work.
        let doing = item.progress.as_ref().and_then(|progress| progress.note.clone()).filter(|note| !note.is_empty()).or_else(|| item.sessions.iter().find(|session| session.working).map(|session| session.title.clone())).filter(|_| item.at_work());
        let value = item.progress.as_ref().and_then(|progress| progress.value);
        let ago = (item.updated.at > 0.).then(|| crate::time::ago(item.removed.as_ref().map_or(item.updated.at, |removed| removed.at), self.now()));
        let (selector, title_selector) = (format!("item-{id}"), format!("item-{id}-title"));
        let mark: AnyElement = if busy {
            div().flex().flex_none().items_center().justify_center().size(px(16.)).child(spinner(SharedString::from(format!("item-busy-{id}")), IconSize::Nav, c.text_muted)).into_any_element()
        } else {
            let selector = format!("item-{id}-status-{}", item.status);
            div().debug_selector(move || selector).flex_none().child(status_mark(&item.status, cx)).into_any_element()
        };
        // Its priority, where one was set (Normal is no mark).
        let priority = (!item.priority.is_empty() && item.priority != NORMAL).then(|| {
            let selector = format!("item-{id}-priority-{}", item.priority);
            div().debug_selector(move || selector).flex_none().child(priority_mark(&item.priority, cx))
        });
        // Its labels, as the box's view names them: the first ones, then how many more.
        let labels: Vec<_> = item.labels.iter().filter_map(|label| self.label(label).cloned()).collect();
        let fits = if narrow { 1 } else { 2 };
        let mut chips: Vec<AnyElement> = labels
            .iter()
            .take(fits)
            .map(|label| {
                let selector = format!("item-{id}-label-{}", label.id);
                div().debug_selector(move || selector).flex_none().child(label_chip(&label.name, &label.color, cx)).into_any_element()
            })
            .collect();
        if labels.len() > fits {
            let selector = format!("item-{id}-more-labels");
            chips.push(
                div()
                    .debug_selector(move || selector)
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(22.))
                    .px(space::S1_5)
                    .rounded(radius::PILL)
                    .border_1()
                    .border_color(c.border)
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_muted)
                    .child(format!("+{}", labels.len() - fits))
                    .into_any_element(),
            );
        }
        // Who it's for: a bot's face where the box's view gives its look.
        let who = item.assignee.as_ref().map(|assignee| {
            // (Its look as the item says it; else as the view lists who an item may be for.)
            let look = assignee.look.clone().or_else(|| self.queue().and_then(|said| said.assignees.iter().find(|each| each.kind == assignee.kind && each.id == assignee.id)).and_then(|each| each.look.clone()));
            let selector = format!("item-{id}-assignee");
            div().debug_selector(move || selector).flex_none().child(who_mark(look.as_ref(), &assignee.id, &assignee.name, px(18.), cx))
        });
        let due = item.due.as_ref().map(|due| {
            let selector = format!("item-{id}-due");
            // Late where the box says so (`overdue`): nothing of it is worked out here.
            let late = item.overdue;
            let late_selector = format!("item-{id}-due-late");
            // (Counted from the box's today.)
            let (words, id_) = (time::due_words(due, &self.today(0)), id.clone());
            div()
                .debug_selector(move || selector)
                .flex_none()
                .text_ui(TypeSize::Caption)
                .text_color(if late { c.danger } else { c.text_muted })
                .child(words.clone())
                .child(div().debug_selector(move || format!("item-{id_}-due:{words}")))
                .when(late, |d| d.child(div().debug_selector(move || late_selector)))
        });
        let open = Act::Open(id.clone());
        let main = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(space::S3)
            .pl(space::S4)
            .pr(space::S2)
            .py(space::S2)
            .min_h(px(44.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.act(open.clone(), cx)))
            .child(mark)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .children(label(&item, cx))
                            .child(
                                div()
                                    .debug_selector(move || title_selector)
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_ui(TypeSize::Body)
                                    .text_color(if deleted || item.struck { c.text_muted } else { c.text })
                                    .when(item.struck, |d| d.line_through())
                                    .child(item.title.clone()),
                            ),
                    )
                    .children(doing.map(|doing| {
                        let selector = format!("item-working-{id}");
                        div()
                            .debug_selector(move || selector)
                            .flex()
                            .items_center()
                            .gap(space::S1_5)
                            .child(spinner(SharedString::from(format!("item-spin-{id}")), IconSize::Nav, c.text_muted))
                            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(doing))
                    })),
            )
            .children(value.map(|value| bar(value, Some(48.), cx)))
            // (What the box shows in place of its status while GitHub makes it Done.)
            .children(item.done_as.clone().filter(|_| !narrow).map(|words| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(words)))
            .children(priority)
            .children(chips)
            .children(who)
            .children(due)
            .children(item.computer.as_ref().filter(|_| !narrow).map(|computer| {
                let selector = format!("item-{id}-computer");
                div().debug_selector(move || selector).flex_none().child(Chip::new(SharedString::from(format!("item-{id}-computer")), crate::bots::computer_name(&computer.name).to_owned()).icon(IconName::Computer))
            }))
            .children(ago.map(|ago| div().flex_none().w(px(30.)).text_right().text_ui(TypeSize::Caption).text_color(c.text_faint).child(ago)));
        let trailing: Option<AnyElement> = if deleted {
            // (Brought back where the box says it may be.)
            item.can.restore.then(|| {
                let restore = Act::Restore(id.clone());
                let selector = format!("item-{id}-restore");
                div()
                    .debug_selector(move || selector)
                    .child(Button::new(SharedString::from(format!("restore-{id}")), "Restore").size(ButtonSize::Small).loading(busy).disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.act(restore.clone(), cx))))
                    .into_any_element()
            })
        } else if busy {
            // A change to it is on its way: nothing more is offered (a delete, least of all) until
            // the box answers.
            let selector = format!("item-{id}-more-off");
            Some(div().debug_selector(move || selector).flex().items_center().justify_center().size(px(28.)).opacity(0.4).child(Icon::new(IconName::More).color(c.icon)).into_any_element())
        } else {
            // Nothing may be done to it: why, in the box's words, where its ••• would be.
            self.item_menu(&item, cx).or_else(|| {
                let why = item.can.why.clone()?;
                let selector = format!("item-{id}-why");
                Some(
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(28.))
                        .tooltip(move |_, cx| Tooltip::new(why.clone()).build(cx))
                        .child(Icon::new(IconName::Info).color(c.text_faint))
                        .into_any_element(),
                )
            })
        };
        card_row(first, last, cx)
            .when(first && last, |d| d.shadow(cx.theme().card_shadow()))
            .flex()
            .items_center()
            .hover(|s| s.bg(c.hover))
            .child(main)
            // (Its place is kept where nothing may be done, so the rows' ends line up.)
            .child(div().flex_none().w(px(40.)).pr(space::S3).flex().justify_end().children(trailing).when(deleted, |d| d.w_auto()))
            .into_any_element()
    }

    /// What you do to an item from its row, as a menu's entries: only what the box says may be done
    /// (`can`). Close it (each status the box says is closed: Done, Canceled), set where it stands
    /// among the open statuses, delete it.
    fn item_entries(&self, item: &Item) -> Vec<MenuItemSpec> {
        let mut entries = Vec::new();
        let status = |id: &str| Act::Set { item: item.id.clone(), change: Box::new(ItemChange { status: Some(id.to_owned()), ..ItemChange::default() }) };
        if item.can.close {
            // (The box's first closed status, Done, is the one that finishes it: a check.)
            let first = self.closing("").into_iter().next().map(|(id, _)| id);
            for (id, name) in self.closing(&item.status) {
                let icon = if Some(&id) == first.as_ref() { IconName::Check } else { IconName::Close };
                entries.push(MenuItemSpec::Entry { label: format!("Mark {name}"), icon: Some(icon), checked: false, danger: false, act: status(&id) });
            }
        }
        if item.can.set {
            let open: Vec<MenuItemSpec> = self
                .queue()
                .map(|said| said.statuses.iter().filter(|each| !each.closed && each.id != item.status).map(|each| MenuItemSpec::Entry { label: each.name.clone(), icon: None, checked: false, danger: false, act: status(&each.id) }).collect())
                .unwrap_or_default();
            if !open.is_empty() {
                if !entries.is_empty() {
                    entries.push(MenuItemSpec::Separator);
                }
                entries.push(MenuItemSpec::Header("Move to".into()));
                entries.extend(open);
            }
        }
        if item.can.delete {
            if !entries.is_empty() {
                entries.push(MenuItemSpec::Separator);
            }
            entries.push(MenuItemSpec::Entry { label: "Delete".into(), icon: Some(IconName::Trash), checked: false, danger: true, act: Act::Delete(item.id.clone()) });
        }
        entries
    }

    /// An item's ••• on its row; none where the box says nothing may be done to it.
    fn item_menu(&mut self, item: &Item, cx: &mut Context<Self>) -> Option<AnyElement> {
        let this = cx.entity().downgrade();
        let entries = self.item_entries(item);
        if entries.is_empty() {
            return None;
        }
        let id = format!("item-{}-more", item.id);
        let trigger = icon_trigger(&id, IconName::More, cx);
        Some(menu_of(SharedString::from(format!("{id}-popover")), trigger, entries, this, 220.).into_any_element())
    }

    /// One chip: its mark and what it says, opening its menu where the box says it may be changed.
    #[allow(clippy::too_many_arguments)]
    fn chip(&mut self, target: &Target, field: Field, mark: AnyElement, words: String, set: bool, enabled: bool, danger: bool, why: Option<&String>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let selector = format!("chip-{}", field.slug());
        let open = self.picker.as_ref().is_some_and(|picker| &picker.target == target && picker.field == field);
        let act = Act::Pick { target: target.clone(), field };
        let chip = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S1_5)
            .h(px(28.))
            .max_w(px(240.))
            .px(px(10.))
            .rounded(radius::NAV)
            .text_ui(TypeSize::Small)
            // One that opens a menu is a control: a card with an edge (a stronger one on a dark
            // sheet, where the quiet one is lost). One that can't be changed is a fact: no edge,
            // no fill, quieter words.
            .when(enabled, |d| d.border_1().border_color(if open || cx.theme().appearance.is_dark() { c.border_strong } else { c.border }).bg(c.card).text_color(if danger { c.danger } else if set { c.text } else { c.text_muted }))
            .when(!enabled, |d| {
                let selector = format!("chip-{}-fact", field.slug());
                d.px(space::S1).text_color(if danger { c.danger } else { c.text_muted }).child(div().debug_selector(move || selector))
            })
            .when(open, |d| d.bg(c.hover))
            .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c.hover)).on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx))))
            // One that doesn't open says why, in the box's words, when rested on.
            .when_some(why.filter(|_| !enabled).cloned(), |d, why| d.tooltip(move |_, cx| Tooltip::new(why.clone()).build(cx)))
            .child(mark)
            .child(div().truncate().child(words));
        // Its menu, under it (over it where there's no room below).
        let menu = open.then(|| {
            let menu = self.picker_view(cx);
            div().absolute().top_full().left_0().child(deferred(anchored().anchor(gpui::Anchor::TopLeft).offset(point(px(0.), space::S1)).snap_to_window_with_margin(px(8.)).child(menu)).with_priority(2))
        });
        div().relative().flex_none().child(chip).children(menu).into_any_element()
    }

    /// An item's chips (a new one's, or one that's there): its status, priority, who it's for, when
    /// it's due and its labels. One that says nothing and can't be changed isn't drawn.
    pub(super) fn chips(&mut self, target: Target, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(facts) = self.facts(&target) else { return div().into_any_element() };
        let quiet = |icon: IconName| div().flex().flex_none().items_center().justify_center().size(px(16.)).child(Icon::new(icon).color(c.icon)).into_any_element();
        let mut chips = Vec::new();
        {
            let words = facts.status_words.clone().or_else(|| facts.status.as_ref().and_then(|id| self.status_name(id))).unwrap_or_else(|| "Status".into());
            let mark = facts.status.as_ref().map_or_else(|| status_mark("", cx), |id| status_mark(id, cx));
            chips.push(self.chip(&target, Field::Status, mark, words, facts.status.is_some(), facts.set || facts.close, false, facts.why.as_ref(), cx));
        }
        if facts.priority.is_some() || facts.set {
            let words = facts.priority.as_ref().and_then(|id| self.priority_name(id)).unwrap_or_else(|| "Priority".into());
            let mark = priority_mark(facts.priority.as_deref().unwrap_or_default(), cx);
            chips.push(self.chip(&target, Field::Priority, mark, words, facts.priority.is_some(), facts.set, false, facts.why.as_ref(), cx));
        }
        if facts.assignee.is_some() || facts.assign {
            let (mark, words) = match &facts.assignee {
                Some((_, id, name)) => (who_mark(facts.look.as_ref(), id, name, px(16.), cx), name.clone()),
                None => (quiet(IconName::User), "Assignee".to_owned()),
            };
            chips.push(self.chip(&target, Field::Assignee, mark, words, facts.assignee.is_some(), facts.assign, false, facts.why.as_ref(), cx));
        }
        if facts.due.is_some() || facts.set {
            let late = facts.late;
            let words = facts.due.as_ref().map_or_else(|| "Due date".to_owned(), |due| time::due_words(due, &self.today(0)));
            let selector = format!("chip-due:{words}");
            let chip = self.chip(&target, Field::Due, quiet(IconName::Clock), words, facts.due.is_some(), facts.set, late, facts.why.as_ref(), cx);
            chips.push(div().debug_selector(move || selector).flex_none().child(chip).into_any_element());
        }
        // Its labels, as the box's view names them: one by name, more by their dots and how many.
        let labels: Vec<_> = facts.labels.iter().filter_map(|label| self.label(label).cloned()).collect();
        if !labels.is_empty() || facts.set {
            let dots = div().flex().flex_none().items_center().gap(px(3.)).children(labels.iter().take(4).map(|label| label_dot(&label.color)));
            let (mark, words) = match labels.as_slice() {
                [] => (div().flex().flex_none().items_center().justify_center().size(px(16.)).child(div().size(px(8.)).rounded(radius::PILL).border_1().border_color(c.text_faint)).into_any_element(), "Labels".to_owned()),
                [one] => (dots.into_any_element(), one.name.clone()),
                many => (dots.into_any_element(), format!("{} labels", many.len())),
            };
            chips.push(self.chip(&target, Field::Labels, mark, words, !labels.is_empty(), facts.set, false, facts.why.as_ref(), cx));
        }
        div().debug_selector(|| "item-chips".into()).flex().flex_wrap().items_center().gap(space::S2).children(chips).into_any_element()
    }

    /// An item's page's head: what names it and whether it's at work, what you do to it (only what
    /// the box says may be done), its title, and its chips.
    fn head(&mut self, item: Item, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let this = cx.entity().downgrade();
        let state: Option<(AnyElement, &str)> = if busy {
            Some((spinner("head-busy", IconSize::Nav, c.text_muted).into_any_element(), "Saving…"))
        } else if item.at_work() {
            Some((spinner("head-spin", IconSize::Nav, c.text_muted).into_any_element(), "At work"))
        } else {
            None
        };
        let status = |id: &str| Act::Set { item: item.id.clone(), change: Box::new(ItemChange { status: Some(id.to_owned()), ..ItemChange::default() }) };
        // (The box's first closed status, Done, is the page's button; the others are in its menu.)
        let mut closing = if item.can.close { self.closing("") } else { Vec::new() };
        let first = (!closing.is_empty()).then(|| closing.remove(0));
        let finish = first.filter(|(id, _)| *id != item.status);
        let mut more = Vec::new();
        for (id, name) in closing.into_iter().filter(|(id, _)| *id != item.status) {
            more.push(MenuItemSpec::Entry { label: format!("Mark {name}"), icon: Some(IconName::Close), checked: false, danger: false, act: status(&id) });
        }
        if item.can.delete {
            if !more.is_empty() {
                more.push(MenuItemSpec::Separator);
            }
            more.push(MenuItemSpec::Entry { label: "Delete".into(), icon: Some(IconName::Trash), checked: false, danger: true, act: Act::Delete(item.id.clone()) });
        }
        // (While a change to it is on its way, its menu waits: no delete over a change.)
        let more = (!more.is_empty()).then(|| {
            if busy {
                div().debug_selector(|| "item-more-off".into()).flex().flex_none().items_center().justify_center().size(px(28.)).opacity(0.4).child(Icon::new(IconName::More).color(c.icon))
            } else {
                menu_of("item-more-popover".into(), icon_trigger("item-more", IconName::More, cx), more, this, 200.)
            }
        });
        // (What it says is changed where the box says it may be, once it's read whole.)
        let can_edit = item.can.edit && self.whole().is_some_and(|whole| !whole.told);
        let chips = self.chips(Target::Item(item.id.clone()), cx);
        div()
            .debug_selector(|| "item-head".into())
            .pt(space::S3)
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(30.))
                    .children(label(&item, cx))
                    .children(state.map(|(mark, words)| div().debug_selector(|| "item-state".into()).flex().items_center().gap(space::S1_5).child(mark).child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words.to_owned()))))
                    .child(div().flex_1())
                    .when_some(finish, |d, (id, name)| {
                        let finish = status(&id);
                        d.child(div().debug_selector(|| "item-finish".into()).child(Button::new("item-finish", format!("Mark {name}")).icon(IconName::Check).size(ButtonSize::Small).disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.act(finish.clone(), cx)))))
                    })
                    .when(can_edit, |d| d.child(div().debug_selector(|| "item-edit".into()).child(Button::new("item-edit", "Edit…").icon(IconName::Pencil).size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.act(Act::Edit, cx))))))
                    .children(more),
            )
            .child(
                div()
                    .debug_selector(|| "item-title".into())
                    .text_ui(TypeSize::Title)
                    .medium()
                    .text_color(if item.struck { c.text_muted } else { c.text })
                    .when(item.struck, |d| d.line_through())
                    .child(item.title.clone()),
            )
            .child(div().pt(space::S1).child(chips))
            .into_any_element()
    }

    /// A session working on it: at work or not, its title; it opens where sessions are.
    fn session_row(&mut self, session: ItemSession, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let id = session.session.clone();
        let mark: AnyElement = if session.working { spinner(SharedString::from(format!("session-spin-{id}")), IconSize::Nav, c.text_muted).into_any_element() } else { Icon::new(IconName::Message).color(c.icon).into_any_element() };
        let title = Some(session.title.clone()).filter(|title| !title.trim().is_empty()).unwrap_or_else(|| "New Session".into());
        let selector = format!("item-session-{id}");
        card_row(first, last, cx)
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S5)
            .py(space::S3)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.act(Act::Session(id.clone()), cx)))
            .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).child(mark))
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(title))
            .when(session.working, |d| d.child(div().flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child("At work")))
            .child(Icon::new(IconName::ChevronRight).color(c.icon))
            .into_any_element()
    }

    fn link_row(&mut self, link: Link, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let url = link.url.clone();
        let icon = if link.kind.as_deref() == Some("github-pr") { IconName::Branch } else { IconName::Link };
        let opens = kuvo_ui::markdown::opens(&url);
        let title = link.title.clone().filter(|title| !title.trim().is_empty());
        let selector = format!("item-link-{}", link.url);
        card_row(first, last, cx)
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S5)
            .py(space::S3)
            .when(opens, |d| d.cursor_pointer().hover(|s| s.bg(c.hover)).on_click(cx.listener(move |this, _, _, cx| this.act(Act::Link(url.clone()), cx))))
            .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).child(Icon::new(icon).color(c.icon)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_ui(TypeSize::Body).text_color(c.text).child(title.clone().unwrap_or_else(|| link.url.clone())))
                    .when(title.is_some(), |d| d.child(div().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(link.url.clone()))),
            )
            .when(opens, |d| d.child(Icon::new(IconName::External).color(c.icon)))
            .into_any_element()
    }

    fn file_row(&mut self, file: File, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let image = file.kind.starts_with("image/");
        let key = self.whole().map(|whole| format!("{}/{}", whole.item.id, file.name)).unwrap_or_default();
        if image && !self.picture_reads.contains_key(&key) {
            if let Some(whole) = self.whole() {
                let (item, name, read_key) = (whole.item.id.clone(), file.name.clone(), key.clone());
                self.picture_reads.insert(key.clone(), crate::pictures::Read::Reading);
                self.run(format!("picture-{key}"), move |client| {
                    match client.work_item_file(&item, &name) {
                        Ok(bytes) => crate::pictures::shown(bytes),
                        Err(error) => crate::pictures::Read::Failed { why: error.to_string(), again: false },
                    }
                }, move |this, read, cx| { this.picture_reads.insert(read_key, read); cx.notify(); }, cx);
            }
        }
        let thumb = match self.picture_reads.get(&key) {
            Some(crate::pictures::Read::Shown { thumb, .. }) => img(thumb.clone()).size(px(52.)).into_any_element(),
            _ => Icon::new(if image { IconName::Image } else { IconName::File }).color(c.icon).into_any_element(),
        };
        let picture = kuvo_box::pictures::Picture { id: key, source: kuvo_box::pictures::Source::File(String::new()), name: Some(file.name.clone()), caption: None };
        let selector = format!("item-file-{}", file.name);
        card_row(first, last, cx)
            .debug_selector(move || selector)
            .id(SharedString::from(format!("queue-file-{}", file.name)))
            .when(image, |row| row.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| { this.viewing = Some(picture.clone()); window.focus(&this.focus, cx); cx.notify(); })))
            .flex().items_center().gap(space::S3).px(space::S5).py(space::S3)
            .child(thumb)
            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(file.name.clone()))
            .child(div().flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child(size_words(file.size)))
            .into_any_element()
    }

    // MARK: The list's head

    fn tab(&self, key: &str, name: String, count: Option<usize>, scope: Scope, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let on = self.shown == Shown::List(scope.clone());
        let selector = format!("tab-{key}");
        let on_selector = format!("tab-{key}-on");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S1_5)
            .h(px(30.))
            .px(px(10.))
            .rounded(radius::NAV)
            .cursor_pointer()
            .text_ui(TypeSize::Small)
            .when(on, |d| d.bg(c.card).shadow(cx.theme().card_shadow()).text_color(c.text).medium().child(div().debug_selector(move || on_selector)))
            .when(!on, |d| d.text_color(c.text_muted).hover(|s| s.bg(c.hover)))
            .on_click(cx.listener(move |this, _, _, cx| this.act(Act::Scope(scope.clone()), cx)))
            .child(name)
            .children(count.map(|count| {
                let selector = format!("tab-{key}-count-{count}");
                div().debug_selector(move || selector).text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_string())
            }))
            .into_any_element()
    }

    /// The whole queue's tab: "All", and how many of it are open.
    fn tab_all(&self, open: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let on = self.shown == Shown::List(Scope::All);
        let selector = format!("tab-all-open-{open}");
        div()
            .id("tab-all")
            .debug_selector(|| "tab-all".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S1_5)
            .h(px(30.))
            .px(px(10.))
            .rounded(radius::NAV)
            .cursor_pointer()
            .text_ui(TypeSize::Small)
            .when(on, |d| d.bg(c.card).shadow(cx.theme().card_shadow()).text_color(c.text).medium().child(div().debug_selector(|| "tab-all-on".into())))
            .when(!on, |d| d.text_color(c.text_muted).hover(|s| s.bg(c.hover)))
            .on_click(cx.listener(|this, _, _, cx| this.act(Act::Scope(Scope::All), cx)))
            .child("All")
            .child(div().debug_selector(move || selector).text_ui(TypeSize::Caption).text_color(c.text_faint).child(format!("{open} open")))
            .into_any_element()
    }

    /// The scope tabs (the whole queue, then each open status with the box's count, and what's
    /// shown where it's neither), the search, the filters and what's offered beside them.
    fn list_head(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let this = cx.entity().downgrade();
        // All, with how many of them are open (the box's count, as the sidebar's row says it).
        let open = self.view().map(|view| view.counts.open);
        let mut tabs = vec![self.tab("all", "All".into(), None, Scope::All, cx)];
        if let Some(open) = open {
            tabs[0] = self.tab_all(open, cx);
        }
        if let Some(said) = self.queue() {
            // The counts are the box's, of the view it listed (as it narrowed it: a search, a
            // filter, a label).
            let counts = self.view().map(|view| view.counts.statuses.clone());
            let statuses = said.statuses.clone();
            for status in statuses.iter().filter(|status| !status.closed) {
                tabs.push(self.tab(&status.id, status.name.clone(), counts.as_ref().and_then(|counts| counts.get(&status.id).copied()), Scope::Status(status.id.clone()), cx));
            }
            // A closed status or a label shown: its own tab, beside All (where a narrow window still
            // shows it), so where you are is said here too.
            match self.shown.clone() {
                Shown::List(Scope::Status(id)) if self.is_closed(&id) => {
                    let name = self.status_name(&id).unwrap_or_default();
                    let tab = self.tab(&id, name, counts.as_ref().and_then(|counts| counts.get(&id).copied()), Scope::Status(id.clone()), cx);
                    tabs.insert(1, tab);
                }
                Shown::List(Scope::Label(id)) => {
                    if let Some(label) = self.label(&id).cloned() {
                        let tab = self.tab(&format!("label-{id}"), label.name, None, Scope::Label(id), cx);
                        tabs.insert(1, tab);
                    }
                }
                _ => {}
            }
        }
        // (As they were last drawn: how far they scroll, and how far they have.)
        let tabs_cut = self.tabs_scroll.max_offset().x + self.tabs_scroll.offset().x > px(1.);
        let search = div()
            .debug_selector(|| "queue-search".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S1_5)
            .w(px(168.))
            .h(px(30.))
            .px(space::S2)
            .rounded(radius::NAV)
            .bg(c.card)
            .border_1()
            .border_color(c.border)
            .child(Icon::new(IconName::Search).color(c.icon))
            .child(div().flex_1().min_w_0().child(self.search.clone()));
        // The filters the box's view offers; one that's on is checked, and picking it takes it off.
        let offered = self.offered_filters();
        let filter = (!offered.is_empty()).then(|| {
            let mut entries = Vec::new();
            for (header, filters) in offered {
                entries.push(MenuItemSpec::Header(header));
                for filter in filters {
                    let on = self.filters.iter().position(|each| each.same(&filter));
                    let label = match &filter {
                        super::Filter::Computer { name, .. } | super::Filter::Owner { name, .. } => name.clone(),
                        super::Filter::Source(kind) => source_name(kind),
                    };
                    entries.push(MenuItemSpec::Entry { label, icon: None, checked: on.is_some(), danger: false, act: on.map_or(Act::AddFilter(filter), Act::RemoveFilter) });
                }
            }
            menu_of("queue-filter-popover".into(), icon_trigger("queue-filter", IconName::Sliders, cx), entries, this.clone(), 240.)
        });
        // Labels are the box's to keep: made here, and the one shown changed or removed.
        let mut more = vec![MenuItemSpec::Entry { label: "New Label…".into(), icon: Some(IconName::Plus), checked: false, danger: false, act: Act::NewLabel }];
        if let Shown::List(Scope::Label(id)) = &self.shown
            && let Some(label) = self.label(id)
        {
            more.push(MenuItemSpec::Separator);
            more.push(MenuItemSpec::Entry { label: format!("Edit {}…", label.name), icon: Some(IconName::Pencil), checked: false, danger: false, act: Act::EditLabel(id.clone()) });
            more.push(MenuItemSpec::Entry { label: format!("Remove {}…", label.name), icon: Some(IconName::Trash), checked: false, danger: true, act: Act::RemoveLabel(id.clone()) });
        }
        let more = menu_of("queue-more-popover".into(), icon_trigger("queue-more", IconName::More, cx), more, this, 240.);
        let chips = (!self.filters.is_empty()).then(|| {
            let chips: Vec<AnyElement> = self
                .filters
                .iter()
                .enumerate()
                .map(|(ix, filter)| {
                    let this = cx.entity().downgrade();
                    let selector = format!("filter-{ix}");
                    div()
                        .debug_selector(move || selector)
                        .child(Chip::new(SharedString::from(format!("filter-chip-{ix}")), filter.words()).on_remove(move |_, cx| {
                            this.update(cx, |this, cx| this.act(Act::RemoveFilter(ix), cx)).ok();
                        }))
                        .into_any_element()
                })
                .collect();
            div()
                .debug_selector(|| "queue-filters".into())
                .flex()
                .flex_wrap()
                .items_center()
                .gap(space::S2)
                .pt(space::S3)
                .children(chips)
                .child(div().flex_1())
                .child(Button::new("filters-clear", "Clear").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.act(Act::ClearFilters, cx))))
        });
        column(
            div()
                .debug_selector(|| "queue-head".into())
                .pt(space::S5)
                .pb(space::S2)
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        // (The tabs scroll sideways where the box has more categories than fit.)
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_w_0()
                                .child(div().id("queue-tabs").track_scroll(&self.tabs_scroll).w_full().flex().items_center().gap(space::S1).py(space::S1).overflow_x_scroll().children(tabs))
                                // More tabs than fit: the last fades, so it's seen that they scroll.
                                .when(tabs_cut, |d| {
                                    d.child(div().debug_selector(|| "queue-tabs-more".into()).absolute().top_0().bottom_0().right_0().w(px(36.)).bg(gpui::linear_gradient(90., gpui::linear_color_stop(c.sidebar.opacity(0.), 0.), gpui::linear_color_stop(c.sidebar, 1.))))
                                }),
                        )
                        .child(search)
                        .children(filter)
                        .child(more)
                        .when(self.queue().is_some(), |d| {
                            d.child(div().debug_selector(|| "queue-new".into()).child(Button::icon_only("queue-new", IconName::Plus, "New Item").primary().size(ButtonSize::Small).tooltip("New Item").disabled(self.client.gate().is_held()).on_click(cx.listener(|this, _, _, cx| this.act(Act::New, cx)))))
                        }),
                )
                .children(chips),
        )
        .into_any_element()
    }

    // MARK: Sheets

    /// New Item and Edit Item, as a composer: a title line, a description under it, a new item's
    /// chips, and what's done with it at the foot (⌘↩).
    fn item_sheet(&mut self, new: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let saving = self.saving;
        let chips = new.then(|| self.chips(Target::New, cx));
        let create_more = self.create_more;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .pl(space::S5)
                    .pr(space::S3)
                    .pt(space::S3)
                    .child(div().debug_selector(|| "sheet-title".into()).flex_1().text_ui(TypeSize::Small).text_color(c.text_muted).child(if new { "New Item" } else { "Edit Item" }))
                    .child(Button::icon_only("sheet-close", IconName::Close, "Close").ghost().size(ButtonSize::Small).disabled(saving).on_click(cx.listener(|this, _, _, cx| this.close_sheet(cx)))),
            )
            .child(div().debug_selector(|| "sheet-title-field".into()).px(space::S5).pt(space::S1).child(self.title.clone()))
            .child(div().debug_selector(|| "sheet-body-field".into()).px(space::S5).pt(space::S1).pb(space::S3).child(self.body_composer.clone()))
            .children(chips.map(|chips| div().px(space::S5).pb(space::S4).child(chips)))
            // Why the box turned it down, in its words.
            .children(self.sheet_error.clone().map(|error| div().debug_selector(|| "sheet-error".into()).px(space::S5).pb(space::S3).text_ui(TypeSize::Small).text_color(c.danger).child(error)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .px(space::S5)
                    .py(space::S3)
                    .border_t_1()
                    // (On a dark sheet the quiet edge is lost: the stronger one.)
                    .border_color(if cx.theme().appearance.is_dark() { c.border_strong } else { c.border })
                    // What it last made, while it stays open for the next.
                    .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).children(self.created.clone().filter(|_| new).map(|made| div().debug_selector(|| "sheet-created".into()).child(made))))
                    .when(new, |d| {
                        d.child(Switch::new("create-more", create_more).label("Create more").on_change({
                            let this = cx.entity().downgrade();
                            move |on, _, cx| {
                                this.update(cx, |this, cx| this.act(Act::CreateMore(on), cx)).ok();
                            }
                        }))
                    })
                    .child(div().debug_selector(|| "sheet-save".into()).child(Button::new("sheet-save", if new { "Create" } else { "Save" }).primary().shortcut("⌘↩").loading(saving).on_click(cx.listener(|this, _, _, cx| this.save_sheet(cx))))),
            )
            .into_any_element()
    }

    /// A label's sheet: its name and its colour (one of the view's `colors`; a new one's is the box's choice
    /// until one's picked), or, to remove it, what that does, asked first.
    fn label_sheet(&mut self, sheet: &Sheet, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let saving = self.saving;
        let name_of = |id: &String| self.label(id).map(|label| label.name.clone()).unwrap_or_default();
        let (title, save, danger) = match sheet {
            Sheet::NewLabel => ("New Label".to_owned(), "Add Label", false),
            Sheet::EditLabel(_) => ("Edit Label".to_owned(), "Save", false),
            Sheet::RemoveLabel(id) => (format!("Remove {}?", name_of(id)), "Remove", true),
            _ => (String::new(), "Save", false),
        };
        let body: AnyElement = if danger {
            div().debug_selector(|| "sheet-removes".into()).text_ui(TypeSize::Small).text_color(c.text_muted).child("It comes off every item that has it. The items themselves stay as they are.").into_any_element()
        } else {
            let picked = self.label_color.clone();
            let colors = self.queue().map(|said| said.colors.clone()).unwrap_or_default();
            let swatches = colors.into_iter().map(|color| {
                let on = picked.as_deref() == Some(color.as_str());
                let selector = format!("color-{color}");
                let act = Act::LabelColor(Some(color.clone()));
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(26.))
                    .rounded(radius::PILL)
                    .border(px(1.5))
                    .border_color(if on { c.text } else { gpui::transparent_black() })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx)))
                    .child(div().size(px(16.)).rounded(radius::PILL).bg(look_tint(&color)))
                    .when(on, |d| {
                        let selector = format!("color-{color}-on");
                        d.child(div().debug_selector(move || selector))
                    })
            });
            div()
                .flex()
                .flex_col()
                .gap(space::S3)
                .child(div().debug_selector(|| "sheet-name-field".into()).child(self.name.clone()))
                .child(div().flex().items_center().gap(space::S1).children(swatches))
                .when(picked.is_none(), |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child("A colour is chosen for it unless you pick one.")))
                .into_any_element()
        };
        let go = Button::new("sheet-save", save).loading(saving).on_click(cx.listener(|this, _, _, cx| this.save_sheet(cx)));
        div()
            .flex()
            .flex_col()
            .gap(space::S3)
            .p(space::S6)
            .child(div().debug_selector(|| "sheet-title".into()).text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
            .child(body)
            // Why the box turned it down, in its words.
            .children(self.sheet_error.clone().map(|error| div().debug_selector(|| "sheet-error".into()).text_ui(TypeSize::Small).text_color(c.danger).child(error)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(space::S2)
                    .pt(space::S2)
                    .child(Button::new("sheet-cancel", "Cancel").disabled(saving).on_click(cx.listener(|this, _, _, cx| this.close_sheet(cx))))
                    .child(div().debug_selector(|| "sheet-save".into()).child(if danger { go.destructive() } else { go.primary() })),
            )
            .into_any_element()
    }

    fn sheet_content(&mut self, sheet: Sheet, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let (width, body) = match &sheet {
            Sheet::New => (640., self.item_sheet(true, cx)),
            Sheet::Edit(_) => (640., self.item_sheet(false, cx)),
            Sheet::NewLabel | Sheet::EditLabel(_) | Sheet::RemoveLabel(_) => (400., self.label_sheet(&sheet, cx)),
        };
        // Return confirms a label's sheet from anywhere in it (a colour just picked, a removal asked).
        let enter_saves = matches!(sheet, Sheet::NewLabel | Sheet::EditLabel(_) | Sheet::RemoveLabel(_));
        div()
            .id(gpui::SharedString::from(format!("queue-sheet-{sheet:?}")))
            .debug_selector(|| "queue-sheet".into())
            .track_focus(&self.sheet_focus)
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| match event.keystroke.key.as_str() {
                "escape" => this.close_sheet(cx),
                // ⌘↩ (Ctrl and Return elsewhere) saves from any of its fields.
                "enter" if event.keystroke.modifiers.secondary(cx) || enter_saves => this.save_sheet(cx),
                _ => {}
            }))
            .w(px(width))
            .max_w_full()
            .rounded(radius::CARD)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(cx.theme().elevated_shadow())
            .child(body)
            .animate_in(Direction::FromTop, true)
            .into_any_element()
    }
}

/// A menu's entry, as it's built when the menu opens.
#[derive(Clone)]
pub(super) enum MenuItemSpec {
    Header(String),
    Separator,
    Entry { label: String, icon: Option<IconName>, checked: bool, danger: bool, act: Act },
}

/// A menu under `trigger`, made only as it opens; each entry does its `act`.
fn menu_of(id: SharedString, trigger: impl IntoElement, entries: Vec<MenuItemSpec>, this: WeakEntity<QueueView>, width: f32) -> gpui::Div {
    let menu = PopoverMenu::new(id, trigger, move |_, cx| {
        let (entries, this) = (entries.clone(), this.clone());
        cx.new(|cx| {
            let mut menu = Menu::new(cx).width(px(width));
            for entry in entries {
                menu = match entry {
                    MenuItemSpec::Header(label) => menu.header(label),
                    MenuItemSpec::Separator => menu.separator(),
                    MenuItemSpec::Entry { label, icon, checked, danger, act } => {
                        let this = this.clone();
                        let mut entry = MenuEntry::new(label).checked(checked).on_select(move |_, cx| {
                            this.update(cx, |this, cx| this.act(act.clone(), cx)).ok();
                        });
                        if let Some(icon) = icon {
                            entry = entry.icon(icon);
                        }
                        if danger {
                            entry = entry.destructive();
                        }
                        menu.entry(entry)
                    }
                };
            }
            menu
        })
    })
    .align_right();
    // (In a block of its own, so a row centres it as it does its other controls.)
    div().flex_none().child(menu)
}

impl Render for QueueView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        if self.client.gate().is_held() {
            return div().id("queue-page").debug_selector(|| "held-area".into()).size_full().bg(c.sidebar);
        }
        if let Some(focus) = self.wants_focus.take() {
            window.focus(&focus, cx);
        }
        self.viewport = window.viewport_size();
        // Which of an item's two fields has the keyboard: where it goes back to after a menu.
        for field in [&self.title, &self.body] {
            let focus = gpui::Focusable::focus_handle(field.read(cx), cx);
            if focus.is_focused(window) {
                self.text_focus = Some(focus);
            }
        }
        // A chip's menu is open: a press anywhere else closes it, and does nothing more.
        let viewport = self.viewport;
        let scrim = self.picker.is_some().then(|| {
            deferred(anchored().position(point(px(0.), px(0.))).child(div().id("picker-scrim").debug_selector(|| "picker-scrim".into()).w(viewport.width).h(viewport.height).occlude().on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.act(Act::ClosePicker, cx)))))
                .with_priority(1)
        });
        let listing = matches!(self.shown, Shown::List(_));
        let head = listing.then(|| self.list_head(cx));
        let loading = match self.rows.as_slice() {
            [Row::Note { key, words, busy: true }, Row::End] => Some((key.clone(), words.clone())),
            _ => None,
        };
        let page = if let Some((key, words)) = loading {
            div().debug_selector({ let key = key.clone(); move || key }).size_full().flex().items_center().justify_center()
                .child(kuvo_ui::small::LoadingLabel::new(SharedString::from(key), words)).into_any_element()
        } else {
            list(self.list.clone(), cx.processor(|this, ix, _, cx| this.row(ix, cx))).size_full().into_any_element()
        };
        let problem = self.problem.clone().map(|problem| {
            // (In the page's flow, above the tabs: it covers nothing.)
            column(
                div()
                    .debug_selector(|| "queue-problem".into())
                    .mt(space::S4)
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S4)
                    .py(space::S3)
                    .rounded(radius::NAV)
                    .bg(c.card)
                    .border_1()
                    .border_color(c.danger)
                    .shadow(cx.theme().card_shadow())
                    .child(Icon::new(IconName::Warning).color(c.danger))
                    .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.text).child(problem))
                    .child(Button::icon_only("queue-problem-close", IconName::Close, "Close").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| {
                        this.problem = None;
                        cx.notify();
                    }))),
            )
        });
        let sheet = self.sheet.clone().map(|sheet| {
            // An item is written near the top, so its chips' menus have room under them.
            let high = matches!(sheet, Sheet::New | Sheet::Edit(_));
            let content = self.sheet_content(sheet, cx);
            div().id("queue-sheet-layer").absolute().inset_0().flex().justify_center().p(space::S6).when(high, |d| d.items_start().pt(px(88.))).when(!high, |d| d.items_center()).bg(dim(cx)).occlude().child(content)
        });
        let viewer = self.viewing.as_ref().map(|picture| {
            div().id("queue-picture-viewer").absolute().inset_0()
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| { if event.keystroke.key == "escape" { this.viewing = None; cx.notify(); } }))
                .on_click(cx.listener(|this, _, _, cx| { this.viewing = None; cx.notify(); }))
                .child(crate::pictures::viewer(picture, 0, 1, self.picture_reads.get(&picture.id), window))
        });
        div()
            .id("queue-page")
            .debug_selector(|| if listing { "queue-page".into() } else { "item-page".into() })
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.sidebar)
            .children(problem)
            .children(head)
            .child(div().flex_1().min_h_0().child(page))
            .children(sheet)
            .children(scrim)
            .children(viewer)
    }
}
