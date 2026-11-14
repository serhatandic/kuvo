//! Drawing the Work Queue's main view: the list's head (scope tabs, the search, the filters as
//! chips to take off), its items as rows in raised cards under each category, an item's page, and
//! the sheets over them (a new item, an item rewritten, a category named).

use gpui::{
    AnyElement, App, AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, list,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::work_queue::{File, Item, ItemSession, Link};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    button::{Button, ButtonSize},
    markdown::Markdown,
    menu::{Menu, MenuEntry, PopoverMenu},
    small::{Chip, spinner},
    theme::{radius, space},
};

use super::{Act, DELETED, DONE, QueueView, Row, Scope, Sheet, Shown, Tone, size_words, source_name};
use crate::areas::category_color;
use crate::bots::dim;

/// The widest the page's cards grow.
const PAGE_WIDTH: f32 = 880.;

/// A column as wide as the page's cards, in the middle.
fn column(body: impl IntoElement) -> gpui::Div {
    div().w_full().flex().justify_center().px(space::S8).child(div().w_full().max_w(px(PAGE_WIDTH)).child(body))
}

/// A category's chip: its dot and its name (or what the box shows in its place).
fn category_chip(id: &str, name: String, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1_5)
        .h(px(22.))
        .max_w(px(180.))
        .px(space::S2)
        .rounded(radius::PILL)
        .border_1()
        .border_color(c.border)
        .text_ui(TypeSize::Caption)
        .text_color(c.text_muted)
        .child(div().size(px(7.)).flex_none().rounded(radius::PILL).bg(category_color(id, cx)))
        .child(div().truncate().child(name))
        .into_any_element()
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

/// How an item stands, as a small mark: at work, done, or open.
fn state_mark(item: &Item, busy: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let id = item.id.clone();
    let mark: AnyElement = if busy {
        spinner(SharedString::from(format!("item-busy-{id}")), IconSize::Nav, c.text_muted).into_any_element()
    } else if item.in_progress {
        let selector = format!("item-working-{id}");
        div().debug_selector(move || selector).child(spinner(SharedString::from(format!("item-spin-{id}")), IconSize::Nav, c.accent)).into_any_element()
    } else if item.category == DONE {
        let selector = format!("item-done-{id}");
        div().debug_selector(move || selector).child(Icon::new(IconName::Check).color(c.success)).into_any_element()
    } else {
        div().size(px(11.)).rounded(radius::PILL).border_1().border_color(c.text_faint).into_any_element()
    };
    div().flex().flex_none().items_center().justify_center().size(px(20.)).child(mark).into_any_element()
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
                            .h(px(if folded { 40. } else { 28. }))
                            .px(space::S3)
                            .rounded(radius::POPOVER)
                            .cursor_pointer()
                            // Folded, it's a card of its own, as a list put away.
                            .when(folded, |d| d.bg(c.card).shadow(cx.theme().card_shadow()))
                            .hover(|s| s.bg(c.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.act(act.clone(), cx)))
                            .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).color(c.icon))
                            .child(if key == DELETED { Icon::new(IconName::Trash).color(c.icon).into_any_element() } else { div().size(px(8.)).flex_none().rounded(radius::PILL).bg(category_color(&key, cx)).into_any_element() })
                            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(name))
                            .children(count.map(|count| {
                                let selector = format!("group-{key}-count-{count}");
                                div().debug_selector(move || selector).text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_string())
                            })),
                    )
                    .into_any_element()
            }
            Row::Item { item, category, first, last, deleted, busy } => self.item_row(*item, category, first, last, deleted, busy, cx),
            Row::Empty(words) => div().debug_selector(|| "queue-empty".into()).h(px(160.)).flex().items_center().justify_center().text_center().text_ui(TypeSize::Body).text_color(c.text_muted).child(words).into_any_element(),
            Row::Note { key, words, busy } => div()
                .debug_selector(move || key)
                .pt(space::S3)
                .flex()
                .items_start()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .children(busy.then(|| div().pt(px(3.)).child(spinner("queue-reading", IconSize::Nav, c.text_muted))))
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
            Row::Head { item, category, busy } => self.head(*item, category, busy, cx),
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
                                    .child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                                    .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)),
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

    /// One item in the list: how it stands, what names it, its title (and what's being done on it
    /// now), its category's chip, where it is, when it last changed; it opens its page.
    #[allow(clippy::too_many_arguments)]
    fn item_row(&mut self, item: Item, category: String, first: bool, last: bool, deleted: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let id = item.id.clone();
        // What's being done on it now, as the box says: its progress note, else the session at work.
        let doing = item.progress.as_ref().and_then(|progress| progress.note.clone()).filter(|note| !note.is_empty()).or_else(|| item.sessions.iter().find(|session| session.working).map(|session| session.title.clone())).filter(|_| item.in_progress);
        let value = item.progress.as_ref().and_then(|progress| progress.value);
        let ago = (item.updated.at > 0.).then(|| crate::time::ago(item.removed.as_ref().map_or(item.updated.at, |removed| removed.at), self.now()));
        let (selector, title_selector, chip_selector) = (format!("item-{id}"), format!("item-{id}-title"), format!("item-{id}-chip"));
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
            .child(state_mark(&item, busy, cx))
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
                    .children(doing.map(|doing| div().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(doing))),
            )
            .children(value.map(|value| bar(value, Some(48.), cx)))
            .children(item.computer.as_ref().map(|computer| Chip::new(SharedString::from(format!("item-{id}-computer")), crate::bots::computer_name(&computer.name).to_owned()).icon(IconName::Computer)))
            .when(!category.is_empty(), |d| d.child(div().debug_selector(move || chip_selector).flex_none().child(category_chip(&item.category, category, cx))))
            .children(ago.map(|ago| div().flex_none().w(px(30.)).text_right().text_ui(TypeSize::Caption).text_color(c.text_faint).child(ago)));
        let trailing: AnyElement = if deleted {
            let restore = Act::Restore(id.clone());
            let selector = format!("item-{id}-restore");
            div()
                .debug_selector(move || selector)
                .child(Button::new(SharedString::from(format!("restore-{id}")), "Restore").size(ButtonSize::Small).loading(busy).on_click(cx.listener(move |this, _, _, cx| this.act(restore.clone(), cx))))
                .into_any_element()
        } else {
            self.item_menu(&item, cx)
        };
        card_row(first, last, cx)
            .when(first && last, |d| d.shadow(cx.theme().card_shadow()))
            .flex()
            .items_center()
            .hover(|s| s.bg(c.hover))
            .child(main)
            .child(div().flex_none().pr(space::S3).child(trailing))
            .into_any_element()
    }

    // BOX-NEEDED: what may be done to each item, said by the box (`can: {finish, move, edit,
    // delete}` on an item, `kuvo/work-queue`). Until then two things are drawn from its facts: one
    // whose Done is someone else's (`closesOn`) is offered no Done, as the protocol says; a deleted
    // one is offered only Restore. Everything else is offered, and the box's refusal is drawn.
    /// The categories an item may be moved to, as the box lists them.
    fn move_targets(&self, item: &Item) -> Vec<MenuItemSpec> {
        if item.removed.is_some() {
            return Vec::new();
        }
        self.categories()
            .iter()
            .filter(|category| !(item.closes_on.is_some() && category.id == DONE))
            .map(|category| MenuItemSpec::Entry { label: category.name.clone(), icon: None, checked: category.id == item.category, danger: false, act: Act::Move { item: item.id.clone(), category: category.id.clone() } })
            .collect()
    }

    /// Whether Mark Done is offered: where the box keeps a Done, it isn't there, and Done isn't
    /// someone else's.
    fn can_finish(&self, item: &Item) -> bool {
        item.removed.is_none() && item.category != DONE && item.closes_on.is_none() && self.categories().iter().any(|category| category.id == DONE)
    }

    /// What you do to an item, as a menu's entries: mark it done, move it to another of the box's
    /// categories, delete it.
    fn item_entries(&self, item: &Item) -> Vec<MenuItemSpec> {
        let mut entries = Vec::new();
        if self.can_finish(item) {
            entries.push(MenuItemSpec::Entry { label: "Mark Done".into(), icon: Some(IconName::Check), checked: false, danger: false, act: Act::Move { item: item.id.clone(), category: DONE.into() } });
            entries.push(MenuItemSpec::Separator);
        }
        let targets = self.move_targets(item);
        if !targets.is_empty() {
            entries.push(MenuItemSpec::Header("Move to".into()));
            entries.extend(targets);
            entries.push(MenuItemSpec::Separator);
        }
        if item.removed.is_none() {
            entries.push(MenuItemSpec::Entry { label: "Delete".into(), icon: Some(IconName::Trash), checked: false, danger: true, act: Act::Delete(item.id.clone()) });
        }
        entries
    }

    fn item_menu(&mut self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let entries = self.item_entries(item);
        let id = format!("item-{}-more", item.id);
        let trigger = icon_trigger(&id, IconName::More, cx);
        menu_of(SharedString::from(format!("{id}-popover")), trigger, entries, this, 220.).into_any_element()
    }

    /// An item's page's head: what names it and how it stands, what you do to it, its title, and
    /// its category as a menu to move it by.
    fn head(&mut self, item: Item, category: String, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let this = cx.entity().downgrade();
        let done = item.category == DONE;
        let state: Option<(AnyElement, &str)> = if busy {
            Some((spinner("head-busy", IconSize::Nav, c.text_muted).into_any_element(), "Saving…"))
        } else if item.in_progress {
            Some((spinner("head-spin", IconSize::Nav, c.accent).into_any_element(), "In progress"))
        } else if done {
            Some((Icon::new(IconName::Check).color(c.success).into_any_element(), "Done"))
        } else {
            None
        };
        let can_finish = self.can_finish(&item);
        let finish = Act::Move { item: item.id.clone(), category: DONE.into() };
        // Its category, as a menu of the box's categories.
        let moves = self.move_targets(&item);
        let chip = category_chip(&item.category, if category.is_empty() { "No category".into() } else { category }, cx);
        let category_control: AnyElement = if moves.is_empty() {
            div().debug_selector(|| "item-category".into()).child(chip).into_any_element()
        } else {
            let trigger = div()
                .id("item-category")
                .debug_selector(|| "item-category".into())
                .flex()
                .items_center()
                .gap(space::S1)
                .cursor_pointer()
                .child(chip)
                .child(Icon::new(IconName::ChevronDown).color(c.icon));
            let mut entries = vec![MenuItemSpec::Header("Move to".into())];
            entries.extend(moves);
            menu_of("item-category-popover".into(), trigger, entries, this.clone(), 220.).into_any_element()
        };
        let more = (item.removed.is_none()).then(|| {
            let trigger = icon_trigger("item-more", IconName::More, cx);
            menu_of("item-more-popover".into(), trigger, vec![MenuItemSpec::Entry { label: "Delete".into(), icon: Some(IconName::Trash), checked: false, danger: true, act: Act::Delete(item.id.clone()) }], this, 180.)
        });
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
                    .when(can_finish, |d| {
                        d.child(div().debug_selector(|| "item-finish".into()).child(Button::new("item-finish", "Mark Done").icon(IconName::Check).size(ButtonSize::Small).disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.act(finish.clone(), cx)))))
                    })
                    // (What an item says is a client's to change only where the box takes it.)
                    .when(self.writes && item.removed.is_none() && self.whole().is_some_and(|whole| !whole.told), |d| {
                        d.child(div().debug_selector(|| "item-edit".into()).child(Button::new("item-edit", "Edit…").icon(IconName::Pencil).size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.act(Act::Edit, cx)))))
                    })
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
            .child(div().flex().items_center().gap(space::S2).pt(space::S1).child(category_control))
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
        let icon = if file.kind.starts_with("image/") { IconName::Image } else { IconName::File };
        let selector = format!("item-file-{}", file.name);
        card_row(first, last, cx)
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S5)
            .py(space::S3)
            .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).child(Icon::new(icon).color(c.icon)))
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
            .children(count.map(|count| div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(count.to_string())))
            .into_any_element()
    }

    /// The scope tabs (the whole queue, what's in progress, each of the box's categories with the
    /// box's count), the search, the filters and what's offered beside them.
    fn list_head(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let this = cx.entity().downgrade();
        let mut tabs = vec![self.tab("all", "All".into(), None, Scope::All, cx)];
        if let Some(said) = self.queue() {
            let counts = said.counts.clone();
            let categories = said.categories.clone();
            tabs.push(self.tab("progress", "In Progress".into(), Some(counts.in_progress), Scope::Progress, cx));
            for category in categories {
                tabs.push(self.tab(&category.id, category.name.clone(), counts.categories.get(&category.id).copied(), Scope::Category(category.id.clone()), cx));
            }
        }
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
        // Categories are the box's to keep: made here, and one you made renamed or removed.
        let mut more = vec![MenuItemSpec::Entry { label: "New Category…".into(), icon: Some(IconName::Plus), checked: false, danger: false, act: Act::NewCategory }];
        if let Shown::List(Scope::Category(id)) = &self.shown
            && let Some(category) = self.categories().iter().find(|category| &category.id == id)
            // BOX-NEEDED: no built-in categories (every one renamed and removed like any other):
            // today the box keeps Next, Later, Parked and Done (`builtIn`) and refuses both.
            && !category.built_in
        {
            more.push(MenuItemSpec::Separator);
            more.push(MenuItemSpec::Entry { label: format!("Rename {}…", category.name), icon: Some(IconName::Pencil), checked: false, danger: false, act: Act::RenameCategory(id.clone()) });
            more.push(MenuItemSpec::Entry { label: format!("Remove {}", category.name), icon: Some(IconName::Trash), checked: false, danger: true, act: Act::RemoveCategory(id.clone()) });
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
                        .child(div().id("queue-tabs").flex_1().min_w_0().flex().items_center().gap(space::S1).py(space::S1).overflow_x_scroll().children(tabs))
                        .child(search)
                        .children(filter)
                        .child(more)
                        .when(self.writes, |d| {
                            d.child(div().debug_selector(|| "queue-new".into()).child(Button::icon_only("queue-new", IconName::Plus, "New Item").primary().size(ButtonSize::Small).tooltip("New Item").on_click(cx.listener(|this, _, _, cx| this.act(Act::New, cx)))))
                        })
                        // A box that takes no item from a client: the + is there, off, and says why
                        // where it's looked for.
                        .when(!self.writes && self.queue().is_some(), |d| {
                            d.child(div().debug_selector(|| "queue-new-off".into()).child(Button::icon_only("queue-new-off", IconName::Plus, "New Item").size(ButtonSize::Small).disabled(true).tooltip(format!("Agents add items on {}. Adding one from here isn’t in Kuvo Code there yet.", self.this_box))))
                        }),
                )
                .children(chips),
        )
        .into_any_element()
    }

    // MARK: Sheets

    fn sheet_content(&mut self, sheet: Sheet, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let label = |words: &'static str| div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words);
        let item_fields = |this: &QueueView| div().flex().flex_col().gap(space::S2).child(label("Title")).child(div().debug_selector(|| "sheet-title-field".into()).child(this.title.clone())).child(div().pt(space::S2).child(label("Description"))).child(div().debug_selector(|| "sheet-body-field".into()).child(this.body.clone()));
        let (title, width, body, save): (String, f32, AnyElement, &'static str) = match &sheet {
            Sheet::New => {
                // Where it goes: one of the box's categories, or the box's own choice.
                let picked = self.form_category.clone();
                let shown = picked.as_ref().and_then(|id| self.category_name(id)).unwrap_or_else(|| "Chosen for you".into());
                let entries: Vec<MenuItemSpec> = self.categories().iter().map(|category| MenuItemSpec::Entry { label: category.name.clone(), icon: None, checked: picked.as_ref() == Some(&category.id), danger: false, act: Act::FormCategory(category.id.clone()) }).collect();
                let trigger = div()
                    .id("sheet-category")
                    .debug_selector(|| "sheet-category".into())
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .w(px(220.))
                    .h(px(32.))
                    .px(space::S3)
                    .rounded(radius::NAV)
                    .border_1()
                    .border_color(c.border_strong)
                    .bg(c.card)
                    .cursor_pointer()
                    .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(shown))
                    .child(Icon::new(IconName::ChevronDown).color(c.icon));
                let category = menu_of("sheet-category-popover".into(), trigger, entries, cx.entity().downgrade(), 220.);
                ("New Item".into(), 560., item_fields(self).child(div().pt(space::S2).child(label("Category"))).child(category).into_any_element(), "Add Item")
            }
            Sheet::Edit(_) => ("Edit Item".into(), 560., item_fields(self).into_any_element(), "Save"),
            Sheet::NewCategory => ("New Category".into(), 400., div().debug_selector(|| "sheet-name-field".into()).child(self.name.clone()).into_any_element(), "Add Category"),
            Sheet::RenameCategory(id) => (format!("Rename {}", self.category_name(id).unwrap_or_default()), 400., div().debug_selector(|| "sheet-name-field".into()).child(self.name.clone()).into_any_element(), "Save"),
        };
        let saving = self.saving;
        div()
            .debug_selector(|| "queue-sheet".into())
            .track_focus(&self.sheet_focus)
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_sheet(cx);
                }
            }))
            .w(px(width))
            .max_w_full()
            .flex()
            .flex_col()
            .gap(space::S3)
            .p(space::S6)
            .rounded(radius::CARD)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(cx.theme().elevated_shadow())
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
                    .child(div().debug_selector(|| "sheet-save".into()).child(Button::new("sheet-save", save).primary().loading(saving).on_click(cx.listener(|this, _, _, cx| this.save_sheet(cx))))),
            )
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
        if let Some(focus) = self.wants_focus.take() {
            window.focus(&focus, cx);
        }
        let listing = matches!(self.shown, Shown::List(_));
        let head = listing.then(|| self.list_head(cx));
        let page = list(self.list.clone(), cx.processor(|this, ix, _, cx| this.row(ix, cx))).size_full();
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
            let content = self.sheet_content(sheet, cx);
            div().id("queue-sheet-layer").absolute().inset_0().flex().items_center().justify_center().p(space::S6).bg(dim(cx)).occlude().child(content)
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
    }
}
