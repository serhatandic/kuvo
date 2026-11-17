//! The menu under one of an item's chips (its status, priority, assignee, due day, labels), for a
//! new item and for one that's there: what the box's view offers, a line to narrow it by (or, for
//! labels, to make one; for a due day, to type one), a number beside each of the first nine rows
//! that picks it, Up and Down, Return and Escape. Labels stay open while several are picked.
//!
//! A pick for a new item is kept until it's made; a pick for an item goes to the box at once
//! (`PATCH kuvo/work-queue/items/<id>`), and what it answers is what's drawn.

use gpui::{AnyElement, Context, Focusable as _, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use kuvo_box::accounts::said;
use kuvo_box::areas::Look;
use kuvo_box::work_queue::ItemChange;
use kuvo_ui::{
    ActiveTheme, Icon, IconName, StyledType, TextFieldEvent, TypeSize,
    small::spinner,
    theme::{radius, space},
};

use super::marks::{label_dot, priority_mark, status_mark, who_mark};
use super::{Act, QueueView, Sheet};
use crate::time;

/// What a chip says and its menu changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Status,
    Priority,
    Assignee,
    Due,
    Labels,
}

impl Field {
    pub fn slug(self) -> &'static str {
        match self {
            Field::Status => "status",
            Field::Priority => "priority",
            Field::Assignee => "assignee",
            Field::Due => "due",
            Field::Labels => "labels",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Field::Status => "Change status…",
            Field::Priority => "Change priority…",
            Field::Assignee => "Assign to…",
            Field::Due => "Or type a day",
            Field::Labels => "Find or make a label…",
        }
    }
}

/// What a menu changes: the item being written, or one that's there (by its id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    New,
    Item(String),
}

/// The menu open under a chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub target: Target,
    pub field: Field,
    /// The row the keyboard is on.
    pub at: usize,
    /// Why the box wouldn't make a label, in its words.
    pub error: Option<String>,
    /// A label is being made.
    pub making: bool,
    /// What its line said when the rows were last narrowed by it.
    pub typed: String,
}

/// What a row picks.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Picked {
    Status(String),
    Priority(String),
    /// A kind and an id; none for no one.
    Assignee(Option<(String, String)>),
    /// A day; none for no due day.
    Due(Option<String>),
    Label(String),
    /// A label to make, by the name typed.
    MakeLabel(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mark {
    Status(String),
    Priority(String),
    /// A look, an id and a name.
    Who(Option<Look>, String, String),
    Icon(IconName),
    Dot(String),
}

/// One row of a menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Pick {
    /// What a test finds it by.
    pub key: String,
    pub label: String,
    pub detail: Option<String>,
    pub mark: Mark,
    pub checked: bool,
    pub value: Picked,
    /// A header over it ("Open", "Closed").
    pub header: Option<&'static str>,
}

/// What an item's chips say, and whether the box says they may be changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Facts {
    pub status: Option<String>,
    /// What's shown in place of its status's name, where the box gave words for it.
    pub status_words: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    /// A kind, an id and a name.
    pub assignee: Option<(String, String, String)>,
    /// Its face, where it's a bot the box gave the look of.
    pub look: Option<Look>,
    /// Why something of it can't be changed, in the box's words.
    pub why: Option<String>,
    /// Its day is gone by, as the box says.
    pub late: bool,
    pub due: Option<String>,
    /// Its status among the open ones, its priority, labels and due day may be changed.
    pub set: bool,
    /// Who it's for may be said (among those the box's view lists).
    pub assign: bool,
    /// It may be closed.
    pub close: bool,
}

impl QueueView {
    /// What `target`'s chips say: a new item's as picked so far, an item's as the box says them.
    pub(super) fn facts(&self, target: &Target) -> Option<Facts> {
        let listed = |kind: &str, id: &str| self.queue().and_then(|said| said.assignees.iter().find(|each| each.kind == kind && each.id == id));
        let named = |kind: &str, id: &str| listed(kind, id).map(|each| each.name.clone()).unwrap_or_default();
        match target {
            Target::New => {
                let draft = &self.draft;
                Some(Facts {
                    status: draft.status.clone(),
                    status_words: None,
                    priority: draft.priority.clone(),
                    labels: draft.labels.clone(),
                    assignee: draft.assignee.as_ref().map(|(kind, id)| (kind.clone(), id.clone(), named(kind, id))),
                    look: draft.assignee.as_ref().and_then(|(kind, id)| listed(kind, id)).and_then(|each| each.look.clone()),
                    why: None,
                    late: false,
                    due: draft.due.clone(),
                    set: true,
                    assign: true,
                    close: true,
                })
            }
            Target::Item(id) => {
                let item = self.item(id)?;
                let mut facts = Facts {
                    status: Some(item.status.clone()),
                    status_words: item.done_as.clone(),
                    priority: Some(item.priority.clone()).filter(|priority| !priority.is_empty()),
                    labels: item.labels.clone(),
                    assignee: item.assignee.as_ref().map(|assignee| (assignee.kind.clone(), assignee.id.clone(), assignee.name.clone())),
                    // Its look as the item says it (a bot of the box asked), else as the view lists it.
                    look: item.assignee.as_ref().and_then(|assignee| assignee.look.clone().or_else(|| listed(&assignee.kind, &assignee.id).and_then(|each| each.look.clone()))),
                    why: item.can.why.clone(),
                    late: item.overdue,
                    due: item.due.clone(),
                    set: item.can.set,
                    assign: item.can.assign,
                    close: item.can.close,
                };
                // What's been asked of it and not answered yet (a change on its way, and one
                // waiting behind it) is what the next pick starts from: a second label picked
                // quickly is put on beside the first.
                for wanted in [self.flying.get(id), self.waiting.get(id)].into_iter().flatten() {
                    if let Some(status) = &wanted.status {
                        facts.status = Some(status.clone());
                        facts.status_words = None;
                    }
                    if let Some(priority) = &wanted.priority {
                        facts.priority = Some(priority.clone());
                    }
                    if let Some(labels) = &wanted.labels {
                        facts.labels = labels.clone();
                    }
                    if let Some(assignee) = &wanted.assignee {
                        facts.assignee = assignee.as_ref().map(|(kind, id)| (kind.clone(), id.clone(), named(kind, id)));
                        facts.look = assignee.as_ref().and_then(|(kind, id)| listed(kind, id)).and_then(|each| each.look.clone());
                    }
                    if let Some(due) = &wanted.due {
                        facts.due = due.clone();
                        facts.late = false;
                    }
                }
                Some(facts)
            }
        }
    }

    /// The rows of the menu that's open: what the box's view offers for it, less what the line
    /// typed leaves out.
    pub(super) fn picks(&self, cx: &gpui::App) -> Vec<Pick> {
        let Some(picker) = &self.picker else { return Vec::new() };
        let (Some(view), Some(facts)) = (self.queue(), self.facts(&picker.target)) else { return Vec::new() };
        let typed = self.picker_field.read(cx).text().trim().to_owned();
        let lower = typed.to_lowercase();
        let takes = |name: &str| lower.is_empty() || name.to_lowercase().contains(&lower);
        match picker.field {
            Field::Status => {
                let mut picks: Vec<Pick> = Vec::new();
                // Under "Open" and "Closed", as the box says each status is.
                let mut under = None;
                for status in view.statuses.iter().filter(|status| if status.closed { facts.close } else { facts.set }).filter(|status| takes(&status.name)) {
                    let header = (under != Some(status.closed)).then_some(if status.closed { "Closed" } else { "Open" });
                    under = Some(status.closed);
                    picks.push(Pick { key: status.id.clone(), label: status.name.clone(), detail: None, mark: Mark::Status(status.id.clone()), checked: facts.status.as_ref() == Some(&status.id), value: Picked::Status(status.id.clone()), header });
                }
                picks
            }
            Field::Priority => view
                .priorities
                .iter()
                .filter(|priority| takes(&priority.name))
                .map(|priority| Pick { key: priority.id.clone(), label: priority.name.clone(), detail: None, mark: Mark::Priority(priority.id.clone()), checked: facts.priority.as_ref() == Some(&priority.id), value: Picked::Priority(priority.id.clone()), header: None })
                .collect(),
            Field::Assignee => {
                let mut picks = Vec::new();
                if takes("No one") {
                    picks.push(Pick { key: "none".into(), label: "No one".into(), detail: None, mark: Mark::Icon(IconName::User), checked: facts.assignee.is_none(), value: Picked::Assignee(None), header: None });
                }
                picks.extend(view.assignees.iter().filter(|each| takes(&each.name)).map(|each| Pick {
                    key: each.id.clone(),
                    label: each.name.clone(),
                    detail: Some(if each.kind == "bot" { "Bot" } else { "Coding agent" }.to_owned()),
                    mark: Mark::Who(each.look.clone(), each.id.clone(), each.name.clone()),
                    checked: facts.assignee.as_ref().is_some_and(|(kind, id, _)| kind == &each.kind && id == &each.id),
                    value: Picked::Assignee(Some((each.kind.clone(), each.id.clone()))),
                    header: None,
                }));
                picks
            }
            Field::Due => {
                // Counted from the box's today, never this computer's clock (a box that hasn't said
                // its day offers a day typed, and none).
                let today = self.today(0);
                let day = |key: &str, label: &str, offset: i64| {
                    let day = self.today(offset);
                    Pick { key: key.into(), label: label.into(), detail: Some(day_words(&day)), mark: Mark::Icon(IconName::Clock), checked: facts.due.as_ref() == Some(&day), value: Picked::Due(Some(day)), header: None }
                };
                let mut picks = if today.is_empty() { Vec::new() } else { vec![day("today", "Today", 0), day("tomorrow", "Tomorrow", 1), day("week", "In a week", 7)] };
                // A day set some other way is said as it is.
                if let Some(due) = facts.due.as_ref().filter(|due| !picks.iter().any(|pick| pick.checked) && !due.is_empty()) {
                    picks.push(Pick { key: "set".into(), label: time::due_words(due, &today), detail: Some(due.clone()), mark: Mark::Icon(IconName::Clock), checked: true, value: Picked::Due(Some(due.clone())), header: None });
                }
                picks.push(Pick { key: "none".into(), label: "No due date".into(), detail: None, mark: Mark::Icon(IconName::Close), checked: facts.due.is_none(), value: Picked::Due(None), header: None });
                picks
            }
            Field::Labels => {
                let mut picks: Vec<Pick> = view
                    .labels
                    .iter()
                    .filter(|label| takes(&label.name))
                    .map(|label| Pick { key: label.id.clone(), label: label.name.clone(), detail: None, mark: Mark::Dot(label.color.clone()), checked: facts.labels.contains(&label.id), value: Picked::Label(label.id.clone()), header: None })
                    .collect();
                // What's typed names no label yet: it can be made (what a label may be called is
                // the box's to say).
                if !typed.is_empty() && !view.labels.iter().any(|label| label.name.to_lowercase() == lower) {
                    picks.push(Pick { key: "make".into(), label: format!("Make label “{typed}”"), detail: None, mark: Mark::Icon(IconName::Plus), checked: false, value: Picked::MakeLabel(typed.clone()), header: None });
                }
                picks
            }
        }
    }

    /// Opens a chip's menu, or closes it where it's the one open.
    pub(super) fn open_picker(&mut self, target: Target, field: Field, cx: &mut Context<Self>) {
        if self.picker.as_ref().is_some_and(|open| open.target == target && open.field == field) {
            self.close_picker(cx);
            return;
        }
        let Some(facts) = self.facts(&target) else { return };
        // (Nothing of it may be changed: no menu.)
        let may = match field {
            Field::Status => facts.set || facts.close,
            Field::Assignee => facts.assign,
            _ => facts.set,
        };
        if !may {
            return;
        }
        // (A due day's line shows how a day is written, with one a fortnight from the box's today.)
        let placeholder = match self.today(14) {
            example if field == Field::Due && !example.is_empty() => format!("{}: {example}", field.placeholder()),
            _ => field.placeholder().to_owned(),
        };
        self.picker_field.update(cx, |line, cx| {
            line.set_placeholder(placeholder);
            line.set_text("", cx);
        });
        self.picker = Some(Picker { target, field, at: 0, error: None, making: false, typed: String::new() });
        // The keyboard starts on what it is now, so Return changes nothing unasked.
        let at = self.picks(cx).iter().position(|pick| pick.checked).filter(|_| field != Field::Labels).unwrap_or(0);
        if let Some(picker) = &mut self.picker {
            picker.at = at;
        }
        self.picker_scroll = gpui::ScrollHandle::new();
        // Typing narrows the menu at once; a due day's numbers pick its rows until its line is
        // clicked into.
        self.wants_focus = Some(if field == Field::Due { self.picker_focus.clone() } else { self.picker_field.focus_handle(cx) });
        cx.notify();
    }

    pub(super) fn close_picker(&mut self, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            // The keyboard goes back to where it was: the title or the description of the item
            // being written, or the page.
            self.wants_focus = Some(match self.sheet {
                Some(Sheet::New | Sheet::Edit(_)) => self.text_focus.clone().unwrap_or_else(|| self.title.focus_handle(cx)),
                _ => self.focus.clone(),
            });
            cx.notify();
        }
    }

    /// The menu's line: typing narrows it, Return picks the row the keyboard is on (a day typed,
    /// for a due day), Escape closes the menu alone.
    pub(super) fn picker_typed(&mut self, event: &TextFieldEvent, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Changed => {
                let typed = self.picker_field.read(cx).text().to_owned();
                // (Its line cleared as it opens, or after a pick, leaves the keyboard's row be.)
                if let Some(picker) = self.picker.as_mut().filter(|picker| picker.typed != typed) {
                    picker.typed = typed;
                    picker.at = 0;
                    picker.error = None;
                    cx.notify();
                }
            }
            TextFieldEvent::Submit(text) => {
                let typed = text.trim().to_owned();
                match &self.picker {
                    // Sent as typed: what a day is, is the box's to say.
                    Some(picker) if picker.field == Field::Due && !typed.is_empty() => self.pick(Picked::Due(Some(typed)), cx),
                    Some(_) => self.pick_at(None, cx),
                    None => {}
                }
            }
            TextFieldEvent::Cancel => self.close_picker(cx),
        }
    }

    /// Moves the keyboard's row by `by`, around the ends.
    pub(super) fn picker_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.picks(cx).len() as isize;
        if let Some(picker) = &mut self.picker
            && count > 0
        {
            picker.at = (picker.at as isize + by).rem_euclid(count) as usize;
            self.picker_scroll.scroll_to_item(picker.at);
            cx.notify();
        }
    }

    /// Picks a row: the one the keyboard is on, or the `n`th.
    pub(super) fn pick_at(&mut self, n: Option<usize>, cx: &mut Context<Self>) {
        let picks = self.picks(cx);
        let at = n.or(self.picker.as_ref().map(|picker| picker.at));
        if let Some(pick) = at.and_then(|at| picks.get(at)) {
            self.pick(pick.value.clone(), cx);
        }
    }

    /// Whether a number key picks a row now: not while something's typed in the menu's line, nor in
    /// a due day's line (where numbers are the day).
    fn numbers_pick(&self, window: &gpui::Window, cx: &gpui::App) -> bool {
        let line = self.picker_field.read(cx);
        line.text().is_empty() && !(self.picker.as_ref().is_some_and(|picker| picker.field == Field::Due) && line.focus_handle(cx).is_focused(window))
    }

    /// What a row picked: kept for a new item; sent to the box for one that's there.
    pub(super) fn pick(&mut self, value: Picked, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.clone() else { return };
        if let Picked::MakeLabel(name) = value {
            self.make_label(name, cx);
            return;
        }
        let Some(facts) = self.facts(&picker.target) else { return };
        // Labels are all of an item's, with this one put on or taken off.
        let toggled = |id: &String| {
            let mut labels = facts.labels.clone();
            match labels.iter().position(|label| label == id) {
                Some(at) => {
                    labels.remove(at);
                }
                None => labels.push(id.clone()),
            }
            labels
        };
        match &picker.target {
            Target::New => match &value {
                Picked::Status(id) => self.draft.status = Some(id.clone()),
                Picked::Priority(id) => self.draft.priority = Some(id.clone()),
                Picked::Assignee(who) => self.draft.assignee = who.clone(),
                Picked::Due(day) => self.draft.due = day.clone(),
                Picked::Label(id) => self.draft.labels = toggled(id),
                Picked::MakeLabel(_) => {}
            },
            Target::Item(item) => {
                // (What it is already: nothing to ask.)
                let change = match &value {
                    Picked::Status(id) => (facts.status.as_ref() != Some(id)).then(|| ItemChange { status: Some(id.clone()), ..ItemChange::default() }),
                    Picked::Priority(id) => (facts.priority.as_ref() != Some(id)).then(|| ItemChange { priority: Some(id.clone()), ..ItemChange::default() }),
                    Picked::Assignee(who) => (facts.assignee.as_ref().map(|(kind, id, _)| (kind.clone(), id.clone())) != *who).then(|| ItemChange { assignee: Some(who.clone()), ..ItemChange::default() }),
                    Picked::Due(day) => (facts.due != *day).then(|| ItemChange { due: Some(day.clone()), ..ItemChange::default() }),
                    Picked::Label(id) => Some(ItemChange { labels: Some(toggled(id)), ..ItemChange::default() }),
                    Picked::MakeLabel(_) => None,
                };
                if let Some(change) = change {
                    self.act(Act::Set { item: item.clone(), change: Box::new(change) }, cx);
                }
            }
        }
        // Several labels are picked in one go; anything else is one choice.
        if picker.field == Field::Labels {
            self.picker_field.update(cx, |line, cx| line.set_text("", cx));
            self.rebuild(cx);
        } else {
            self.close_picker(cx);
            self.rebuild(cx);
        }
    }

    /// Makes the label typed (`POST kuvo/work-queue/labels {add}`), then puts it on.
    fn make_label(&mut self, name: String, cx: &mut Context<Self>) {
        if self.picker.as_ref().is_none_or(|picker| picker.making) {
            return;
        }
        if let Some(picker) = &mut self.picker {
            picker.making = true;
            picker.error = None;
        }
        let was = self.picker.clone();
        self.run(
            "label-make",
            move |client| client.add_label(&name, None),
            move |this, answer, cx| {
                // (Its own menu, still open: else the label is made, and nothing more is done.)
                let its_menu = this.picker.as_ref().is_some_and(|picker| was.as_ref().is_some_and(|was| was.target == picker.target && was.field == picker.field));
                if its_menu && let Some(picker) = &mut this.picker {
                    picker.making = false;
                }
                match answer {
                    Ok((labels, made)) => {
                        this.took_labels(labels, cx);
                        if let (true, Some(made)) = (its_menu, made) {
                            this.pick(Picked::Label(made.id), cx);
                        }
                    }
                    Err(error) => {
                        if its_menu && let Some(picker) = &mut this.picker {
                            picker.error = Some(said(&error));
                        }
                    }
                }
            },
            cx,
        );
        cx.notify();
    }

    /// The open menu, drawn: its line, its rows (a long list scrolls), and what's wrong.
    pub(super) fn picker_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(picker) = self.picker.clone() else { return div().into_any_element() };
        let picks = self.picks(cx);
        let typed = !self.picker_field.read(cx).text().trim().is_empty();
        let slug = picker.field.slug();
        // (As it was last drawn: how far it scrolls, and how far it has.)
        let more_below = self.picker_scroll.max_offset().y + self.picker_scroll.offset().y > px(1.);
        let rows: Vec<AnyElement> = picks
            .iter()
            .enumerate()
            .map(|(ix, pick)| {
                let selector = format!("pick-{}", pick.key);
                let on = picker.at == ix;
                let mark: AnyElement = match &pick.mark {
                    Mark::Status(id) => status_mark(id, cx),
                    Mark::Priority(id) => priority_mark(id, cx),
                    Mark::Who(look, id, name) => who_mark(look.as_ref(), id, name, px(16.), cx),
                    Mark::Icon(icon) => div().flex().flex_none().items_center().justify_center().size(px(16.)).child(Icon::new(*icon).color(c.icon)).into_any_element(),
                    Mark::Dot(color) => div().flex().flex_none().items_center().justify_center().size(px(16.)).child(label_dot(color)).into_any_element(),
                };
                let value = pick.value.clone();
                let row = div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(space::S2)
                    .h(px(30.))
                    .px(space::S2)
                    .rounded(radius::NAV)
                    .cursor_pointer()
                    .text_ui(TypeSize::Small)
                    .text_color(c.text)
                    .when(on, |d| d.bg(c.hover))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if let (true, Some(picker)) = (*hovered, &mut this.picker)
                            && picker.at != ix
                        {
                            picker.at = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(value.clone(), cx)))
                    .child(mark)
                    .child(div().flex_1().min_w_0().truncate().child(pick.label.clone()))
                    .children(pick.detail.clone().map(|detail| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(detail)))
                    .when(pick.checked, |d| {
                        let selector = format!("pick-{}-on", pick.key);
                        d.child(div().debug_selector(move || selector).flex_none().child(Icon::new(IconName::Check).color(c.text)))
                    })
                    // The number that picks it, beside each of the first nine.
                    .child(div().flex_none().w(px(10.)).text_right().text_ui(TypeSize::Caption).text_color(c.text_faint).children((ix < 9).then(|| (ix + 1).to_string())));
                match pick.header {
                    Some(header) => div().flex().flex_none().flex_col().child(div().px(space::S2).pt(space::S2).pb(space::S1).text_ui(TypeSize::Caption).text_color(c.text_faint).child(header)).child(row).into_any_element(),
                    None => row.into_any_element(),
                }
            })
            .collect();
        // Nothing to list: said plainly, with what to do about it.
        let empty = picks.is_empty().then(|| {
            let words = match picker.field {
                Field::Labels if !typed => "No labels yet. Type a name to make the first one.",
                _ => "Nothing matches.",
            };
            div().debug_selector(|| "picker-empty".into()).px(space::S2).py(space::S3).text_ui(TypeSize::Small).text_color(c.text_muted).child(words)
        });
        div()
            .id("queue-picker")
            .debug_selector(move || format!("picker-{slug}"))
            .track_focus(&self.picker_focus)
            // Up and Down walk the rows from the menu's line too.
            .capture_action(cx.listener(|this, _: &kuvo_ui::text_field::Up, _, cx| {
                this.picker_step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &kuvo_ui::text_field::Down, _, cx| {
                this.picker_step(1, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                let plain = !event.keystroke.modifiers.modified();
                match key {
                    "escape" => this.close_picker(cx),
                    "up" => this.picker_step(-1, cx),
                    "down" => this.picker_step(1, cx),
                    "enter" if plain => this.pick_at(None, cx),
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" if plain && this.numbers_pick(window, cx) => this.pick_at(key.parse::<usize>().ok().map(|n| n - 1), cx),
                    // (Anything else is the line's, or the sheet's: ⌘↩.)
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .occlude()
            .w(px(264.))
            .flex()
            .flex_col()
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(cx.theme().elevated_shadow())
            .child(
                div()
                    .debug_selector(|| "picker-line".into())
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .h(px(38.))
                    .px(space::S3)
                    .border_b_1()
                    .border_color(c.border)
                    .child(div().flex_1().min_w_0().child(self.picker_field.clone()))
                    .children(picker.making.then(|| spinner("picker-making", kuvo_ui::IconSize::Nav, c.text_muted))),
            )
            .child(
                div()
                    .relative()
                    .child(div().id("queue-picker-rows").track_scroll(&self.picker_scroll).max_h(px(308.)).overflow_y_scroll().flex().flex_col().p(space::S1).children(rows).children(empty))
                    // More rows than fit: the last ones fade, so it's seen that it scrolls.
                    .when(more_below, |d| {
                        d.child(div().debug_selector(|| "picker-more".into()).absolute().left_0().right_0().bottom_0().h(px(28.)).rounded_b(radius::POPOVER).bg(gpui::linear_gradient(180., gpui::linear_color_stop(c.elevated.opacity(0.), 0.), gpui::linear_color_stop(c.elevated, 1.))))
                    }),
            )
            // Why the box wouldn't make the label, in its words.
            .children(picker.error.clone().map(|error| div().debug_selector(|| "picker-error".into()).px(space::S3).pb(space::S2).text_ui(TypeSize::Small).text_color(c.danger).child(error)))
            .into_any_element()
    }
}

/// A day in a couple of words, for beside "Today" and "In a week": "Nov 21".
fn day_words(day: &str) -> String {
    // (Seen from a day that isn't near it, so it's never "Today" twice.)
    time::due_words(day, "0001-01-01").split(',').next().unwrap_or_default().to_owned()
}
