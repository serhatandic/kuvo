//! The Work Queue area's main view: the queue (its items under the box's statuses, with scope tabs,
//! a search the box answers, removable filters, and what's closed folded), an item's page (what it
//! says, how far it is, its notes, files, links and the sessions working on it), and what you do:
//! add an item, change what one says, where it stands, its priority, labels, assignee and due day,
//! close it, delete it and bring it back, and make, change and remove labels. Everything on it is
//! the box's (`kuvo/work-queue?owner=all&counts=1`, with `q` for a search;
//! `kuvo/work-queue/items/<id>`): its statuses, priorities and labels, its order, its counts, and
//! what may be done to each item (`can`). Every change goes to the box's route and what it answers
//! is what's drawn after, or why not, in its words. The app decides nothing but what it draws: the
//! filters only choose which of the box's items show.
//!
//! Drawn as Bots' and Settings' pages are: the page's rows worked out once each time what they're
//! drawn from changes, then a virtual list that lays out only what's on screen.

pub mod marks;
mod page;
mod picker;

use std::collections::HashMap;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, Pixels, ScrollHandle, Size, Subscription, Task, px};
use kuvo_box::BoxClient;
use kuvo_box::accounts::said;
use kuvo_box::work_queue::{File, Item, ItemChange, ItemSession, Label, Link, NewItem, QueueSaid, Who, Whole};
use kuvo_ui::{TextField, TextFieldEvent};

pub use picker::{Field, Picker, Target};

use crate::areas::{Reading, reading};
use crate::time;

/// The fold's key for what's deleted.
pub const DELETED: &str = "deleted";

/// The two statuses an item is closed in, as the protocol names them: what Mark Done and Mark
/// Canceled set, where the box's view lists them and says the item may be closed.
pub const DONE: &str = "done";
pub const CANCELED: &str = "canceled";

/// The priority no mark is drawn for on a row: the one an item has when none was ever set.
pub const NORMAL: &str = "normal";

/// The colours a label may be: the looks' names, as the protocol lists them.
pub const COLORS: [&str; 10] = ["coral", "orange", "amber", "lime", "mint", "sky", "indigo", "violet", "pink", "graphite"];

/// Which of the queue's items the list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    /// One of the box's statuses, by its id.
    Status(String),
    /// The items wearing one label, by its id.
    Label(String),
}

impl Scope {
    /// The sidebar's row for it.
    pub fn key(&self) -> Option<String> {
        match self {
            Scope::All => None,
            Scope::Status(id) => Some(format!("status:{id}")),
            Scope::Label(id) => Some(format!("label:{id}")),
        }
    }
}

/// What the main view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    List(Scope),
    /// An item's page, by its id.
    Item(String),
}

/// A filter over the list, shown as a chip to take off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Items on one computer (by its key; this box's own is empty).
    Computer { key: String, name: String },
    /// Items from an outside source, by its kind.
    Source(String),
    /// One owner's items.
    Owner { owner: String, name: String },
}

impl Filter {
    pub fn words(&self) -> String {
        match self {
            Filter::Computer { name, .. } => format!("On {name}"),
            Filter::Source(kind) => format!("From {}", source_name(kind)),
            Filter::Owner { name, .. } => format!("Owner: {name}"),
        }
    }

    /// Whether it's the same filter as `other`, whatever its computer or owner is called now.
    pub fn same(&self, other: &Filter) -> bool {
        match (self, other) {
            (Filter::Computer { key, .. }, Filter::Computer { key: other, .. }) => key == other,
            (Filter::Source(kind), Filter::Source(other)) => kind == other,
            (Filter::Owner { owner, .. }, Filter::Owner { owner: other, .. }) => owner == other,
            _ => false,
        }
    }

    fn kind(&self) -> u8 {
        match self {
            Filter::Computer { .. } => 0,
            Filter::Source(_) => 1,
            Filter::Owner { .. } => 2,
        }
    }

    fn takes(&self, item: &Item) -> bool {
        match self {
            Filter::Computer { key, .. } => item.computer.as_ref().map_or("", |computer| computer.key.as_str()) == key,
            // (The box counts its sources by kind, `github`: a pull request's `github-pr` is one of them.)
            Filter::Source(kind) => item.source.as_ref().is_some_and(|source| source.kind.starts_with(kind.as_str())),
            Filter::Owner { owner, .. } => &item.owner == owner,
        }
    }
}

/// What the view asks of the window around it.
#[derive(Clone, Debug, PartialEq)]
pub enum QueueEvent {
    /// Open one of the sessions working on an item.
    Session(String),
    /// Pick this row in the sidebar (none: the whole queue).
    Select(Option<String>),
    /// The queue changed: the box's view is to be read again.
    Changed,
}

/// What a control asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Open(String),
    Back,
    Session(String),
    Link(String),
    Scope(Scope),
    Fold(String),
    AddFilter(Filter),
    RemoveFilter(usize),
    ClearFilters,
    /// Change an item: where it stands, its priority, labels, assignee or due day.
    Set { item: String, change: Box<ItemChange> },
    Delete(String),
    Restore(String),
    New,
    Edit,
    /// Open the menu of a chip (or close it, open already).
    Pick { target: Target, field: Field },
    ClosePicker,
    /// Whether New Item stays open after an item is made.
    CreateMore(bool),
    NewLabel,
    EditLabel(String),
    /// Ask before removing a label.
    RemoveLabel(String),
    /// The colour picked in a label's sheet (none: the box's choice).
    LabelColor(Option<String>),
}

/// A sheet over the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sheet {
    New,
    /// The item being rewritten, by its id.
    Edit(String),
    NewLabel,
    EditLabel(String),
    /// Removing a label, asked first.
    RemoveLabel(String),
}

/// What a new item is to be, beside what it says: each as picked from what the box offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    /// A kind (`bot`, `agent`) and an id.
    pub assignee: Option<(String, String)>,
    pub due: Option<String>,
}

/// How a notice reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Waiting,
    Quiet,
}

/// One of the page list's rows.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    /// A status's header (or Deleted's): its name, the box's count where it gave one for what's
    /// listed, and whether it's folded.
    Group { key: String, name: String, count: Option<usize>, folded: bool, first: bool },
    Item { item: Box<Item>, first: bool, last: bool, deleted: bool, busy: bool },
    /// Nothing to list, in a line.
    Empty(String),
    Note { key: String, words: String, busy: bool },
    /// The way back from an item to the list it was opened from.
    Back(String),
    Head { item: Box<Item>, busy: bool },
    Notice { key: String, tone: Tone, title: String, words: String, act: Option<(String, Act)> },
    Progress { value: Option<f64>, note: String },
    Section(String),
    Body(String),
    Session { session: ItemSession, first: bool, last: bool },
    Link { link: Link, first: bool, last: bool },
    File { file: File, first: bool, last: bool },
    Said { ix: usize, who: String, ago: String, text: String, first: bool, last: bool },
    Facts(String),
    End,
}

/// The Work Queue area's main view.
pub struct QueueView {
    client: BoxClient,
    live: bool,
    said: Reading<QueueSaid>,
    shown: Shown,
    /// The list last shown: where Back from an item goes.
    scope: Scope,
    /// The item shown, whole.
    whole: Reading<Whole>,
    search: Entity<TextField>,
    /// What's searched for, as typed (the box searches; nothing is matched here).
    query: String,
    /// The box's view narrowed by the search, as it last answered one.
    found: Reading<QueueSaid>,
    filters: Vec<Filter>,
    /// Groups folded or unfolded by hand, by their key (a status's id, or Deleted).
    folds: HashMap<String, bool>,
    /// Items with a change on its way.
    busy: Vec<String>,
    /// What the box last refused, over the page until closed.
    problem: Option<String>,
    sheet: Option<Sheet>,
    saving: bool,
    sheet_error: Option<String>,
    /// Which opening of a sheet this is: an answer is its own opening's, never a later one's.
    sheet_opened: u64,
    /// The sheets' fields, made once: an item's title and description, a label's name.
    title: Entity<TextField>,
    body: Entity<TextField>,
    name: Entity<TextField>,
    /// What New Item's chips say.
    draft: Draft,
    /// New Item stays open after an item is made, its text cleared.
    create_more: bool,
    /// What New Item last made, said at its foot while it stays open.
    created: Option<String>,
    /// The colour picked in a label's sheet (none: the box's choice).
    label_color: Option<String>,
    /// The menu open under a chip.
    picker: Option<Picker>,
    /// Its line: what narrows it, a label to make, a day to type.
    picker_field: Entity<TextField>,
    picker_focus: FocusHandle,
    picker_scroll: ScrollHandle,
    /// The window's size, as last drawn: a narrow one shows less on a row.
    viewport: Size<Pixels>,
    wants_focus: Option<FocusHandle>,
    sheet_focus: FocusHandle,
    this_box: String,
    now: Option<f64>,
    rows: Vec<Row>,
    list: ListState,
    focus: FocusHandle,
    tasks: HashMap<String, Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<QueueEvent> for QueueView {}

impl Focusable for QueueView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QueueView {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> QueueView {
        let search = cx.new(|cx| TextField::new(cx).bare().placeholder("Search items"));
        let title = cx.new(|cx| TextField::new(cx).bare().placeholder("Item title"));
        let body = cx.new(|cx| TextField::new(cx).bare().multi_line(3, 12).placeholder("Add a description. Markdown works."));
        let name = cx.new(|cx| TextField::new(cx).placeholder("Name"));
        let picker_field = cx.new(|cx| TextField::new(cx).bare());
        let subscriptions = vec![
            cx.subscribe(&search, |this: &mut QueueView, field, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Changed => {
                    let query = field.read(cx).text().trim().to_owned();
                    if this.query != query {
                        this.query = query;
                        this.list.scroll_to(gpui::ListOffset::default());
                        this.search(cx);
                        this.rebuild(cx);
                    }
                }
                TextFieldEvent::Cancel => field.update(cx, |field, cx| field.set_text("", cx)),
                TextFieldEvent::Submit(_) => {}
            }),
            cx.subscribe(&title, |this: &mut QueueView, _, event: &TextFieldEvent, cx| match event {
                // Return in the title goes on to the description, as in a composer; ⌘↩ saves.
                TextFieldEvent::Submit(_) => {
                    this.wants_focus = Some(this.body.focus_handle(cx));
                    cx.notify();
                }
                TextFieldEvent::Cancel => this.close_sheet(cx),
                TextFieldEvent::Changed => {}
            }),
            cx.subscribe(&name, |this: &mut QueueView, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submit(_) => this.save_sheet(cx),
                TextFieldEvent::Cancel => this.close_sheet(cx),
                TextFieldEvent::Changed => {}
            }),
            cx.subscribe(&body, |this: &mut QueueView, _, event: &TextFieldEvent, cx| {
                if matches!(event, TextFieldEvent::Cancel) {
                    this.close_sheet(cx);
                }
            }),
            cx.subscribe(&picker_field, |this: &mut QueueView, _, event: &TextFieldEvent, cx| this.picker_typed(event, cx)),
        ];
        let mut view = QueueView {
            client,
            live,
            said: Reading::NotYet,
            shown: Shown::List(Scope::All),
            scope: Scope::All,
            whole: Reading::NotYet,
            search,
            query: String::new(),
            found: Reading::NotYet,
            filters: Vec::new(),
            folds: HashMap::new(),
            busy: Vec::new(),
            problem: None,
            sheet: None,
            saving: false,
            sheet_error: None,
            sheet_opened: 0,
            title,
            body,
            name,
            draft: Draft::default(),
            create_more: false,
            created: None,
            label_color: None,
            picker: None,
            picker_field,
            picker_focus: cx.focus_handle(),
            picker_scroll: ScrollHandle::new(),
            viewport: Size::default(),
            wants_focus: None,
            sheet_focus: cx.focus_handle(),
            this_box: "This Mac".into(),
            now: None,
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            focus: cx.focus_handle(),
            tasks: HashMap::new(),
            _subscriptions: subscriptions,
        };
        view.rebuild(cx);
        view
    }

    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    pub fn sheet(&self) -> Option<&Sheet> {
        self.sheet.as_ref()
    }

    pub fn filters(&self) -> &[Filter] {
        &self.filters
    }

    pub fn search_field(&self) -> &Entity<TextField> {
        &self.search
    }

    /// The fields an item is written in: its title and its description.
    pub fn item_fields(&self) -> (&Entity<TextField>, &Entity<TextField>) {
        (&self.title, &self.body)
    }

    /// The field a label is named in.
    pub fn name_field(&self) -> &Entity<TextField> {
        &self.name
    }

    /// What New Item's chips say.
    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// The menu open under a chip, and its line.
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    pub fn picker_field(&self) -> &Entity<TextField> {
        &self.picker_field
    }

    /// What's searched for, as it was typed.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The item shown, whole, once the box has said it.
    pub fn whole(&self) -> Option<&Whole> {
        match (&self.whole, &self.shown) {
            (Reading::Ready(whole), Shown::Item(id)) if &whole.item.id == id => Some(whole),
            _ => None,
        }
    }

    pub fn set_now(&mut self, now: f64) {
        self.now = Some(now);
    }

    pub fn set_this_box(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.this_box != name {
            self.this_box = name.to_owned();
            self.rebuild(cx);
        }
    }

    fn now(&self) -> f64 {
        self.now.unwrap_or_else(time::now_ms)
    }

    /// Today, as the box writes a day: this computer's own day (a moment set from outside, in
    /// tests and shots, is read as UTC's).
    fn today(&self, offset: i64) -> String {
        let now = self.now();
        let ahead = if self.now.is_some() { 0 } else { kuvo_box::work_queue::utc_offset(now) };
        time::day(now, ahead, offset)
    }

    /// The box's view, as the sidebar read it.
    pub fn set_said(&mut self, said: Reading<QueueSaid>, cx: &mut Context<Self>) {
        if self.said != said {
            self.said = said;
            self.rebuild(cx);
        }
    }

    /// The box's whole view.
    fn queue(&self) -> Option<&QueueSaid> {
        match &self.said {
            Reading::Ready(said) => Some(said),
            _ => None,
        }
    }

    /// What the list is drawn from: the box's view, or, while something's searched for, the view
    /// the box narrowed by it.
    fn view(&self) -> Option<&QueueSaid> {
        if self.query.is_empty() {
            return self.queue();
        }
        match &self.found {
            Reading::Ready(found) => Some(found),
            _ => None,
        }
    }

    /// Asks the box for the view narrowed by what's typed. A newer search replaces an older one
    /// still on its way, and an answer is only ever taken for what's searched for now.
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.query.clone();
        if query.is_empty() {
            self.found = Reading::NotYet;
            self.tasks.remove("search");
            return;
        }
        let asked = query.clone();
        self.run(
            "search",
            move |client| client.work_queue_search(&asked),
            move |this, read, _| {
                if this.query == query {
                    this.found = match read {
                        Ok(found) => Reading::Ready(found),
                        Err(error) => Reading::Failed(error.to_string()),
                    };
                }
            },
            cx,
        );
    }

    /// Shows what the sidebar's row `key` names (none: the whole queue).
    pub fn show(&mut self, key: Option<&str>, cx: &mut Context<Self>) {
        let shown = match key.map(|key| key.split_once(':').unwrap_or((key, ""))) {
            Some(("item", id)) => Shown::Item(id.to_owned()),
            Some(("status", id)) => Shown::List(Scope::Status(id.to_owned())),
            Some(("label", id)) => Shown::List(Scope::Label(id.to_owned())),
            _ => Shown::List(Scope::All),
        };
        if self.shown != shown {
            if let Shown::List(scope) = &shown {
                self.scope = scope.clone();
            }
            self.shown = shown;
            self.problem = None;
            self.sheet = None;
            self.picker = None;
            self.sheet_error = None;
            // (A change still on its way answers for its own item, not for what's shown now.)
            self.saving = false;
            self.whole = Reading::NotYet;
            self.list.scroll_to(gpui::ListOffset::default());
            self.read(cx);
        }
        self.rebuild(cx);
    }

    /// Reads the item shown, whole (again, each time it's shown).
    pub fn read(&mut self, cx: &mut Context<Self>) {
        let Shown::Item(item) = self.shown.clone() else { return };
        let asked = item.clone();
        self.run("read-item", move |client| client.work_item(&asked), move |this, read, _| {
            if this.shown == Shown::Item(item) {
                this.whole = reading(read, &this.whole);
            }
        }, cx);
    }

    /// Runs `work` against the box, in the app off this thread, in tests and shots at once, and
    /// hands what it came to to `then`. A newer one under the same name replaces an older.
    fn run<T: Send + 'static>(&mut self, name: impl Into<String>, work: impl FnOnce(&BoxClient) -> T + Send + 'static, then: impl FnOnce(&mut QueueView, T, &mut Context<QueueView>) + 'static, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            let done = work(&client);
            then(self, done, cx);
            self.rebuild(cx);
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work(&client) }).await;
            this.update(cx, |this, cx| {
                then(this, done, cx);
                this.rebuild(cx);
            })
            .ok();
        });
        self.tasks.insert(name.into(), task);
    }

    // MARK: What's shown

    fn status_name(&self, id: &str) -> Option<String> {
        self.queue()?.statuses.iter().find(|status| status.id == id).map(|status| status.name.clone())
    }

    /// Whether the box says an item in this status is finished.
    fn is_closed(&self, id: &str) -> bool {
        self.queue().is_some_and(|said| said.statuses.iter().any(|status| status.id == id && status.closed))
    }

    fn lists_status(&self, id: &str) -> bool {
        self.queue().is_some_and(|said| said.statuses.iter().any(|status| status.id == id))
    }

    fn priority_name(&self, id: &str) -> Option<String> {
        self.queue()?.priorities.iter().find(|priority| priority.id == id).map(|priority| priority.name.clone())
    }

    /// A label, as the box's view lists it.
    fn label(&self, id: &str) -> Option<&Label> {
        self.queue()?.labels.iter().find(|label| label.id == id)
    }

    /// The item shown: whole once read, else as the list has it.
    fn item(&self, id: &str) -> Option<&Item> {
        if let Some(whole) = self.whole() {
            return Some(&whole.item);
        }
        let said = self.queue()?;
        said.items.iter().chain(&said.removed).find(|item| item.id == id)
    }

    /// The box's view as it's drawn now (with what a change answered since it was read).
    pub fn said(&self) -> &Reading<QueueSaid> {
        &self.said
    }

    fn scope_name(&self, scope: &Scope) -> String {
        match scope {
            Scope::All => None,
            Scope::Status(id) => self.status_name(id),
            Scope::Label(id) => self.label(id).map(|label| label.name.clone()),
        }
        .unwrap_or_else(|| "Work Queue".into())
    }

    /// Folded unless opened: what's closed (as the box says a status is) and what's deleted.
    fn is_folded(&self, key: &str) -> bool {
        self.folds.get(key).copied().unwrap_or(key == DELETED || self.is_closed(key))
    }

    /// Whether the list is narrowed by the search or a filter: nothing stays folded then.
    fn narrowed(&self) -> bool {
        !self.query.is_empty() || !self.filters.is_empty()
    }

    /// Whether the filters take an item: every kind of filter picked, any one of a kind.
    fn takes(&self, item: &Item) -> bool {
        (0..3u8).all(|kind| {
            let mut of_kind = self.filters.iter().filter(|filter| filter.kind() == kind).peekable();
            of_kind.peek().is_none() || of_kind.any(|filter| filter.takes(item))
        })
    }

    /// Works the page's rows out again, from what it's drawn from now.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows = match self.shown.clone() {
            Shown::List(scope) => self.list_rows(&scope),
            Shown::Item(id) => self.item_rows(&id),
        };
        // Only the rows that changed are laid out again; the rest keep their places and the page
        // keeps its scroll.
        if rows != self.rows {
            let start = self.rows.iter().zip(&rows).take_while(|(old, new)| old == new).count();
            let tail = self.rows[start..].iter().rev().zip(rows[start..].iter().rev()).take_while(|(old, new)| old == new).count();
            self.list.splice(start..self.rows.len() - tail, rows.len() - tail - start);
            self.rows = rows;
        }
        cx.notify();
    }

    fn unread(&self) -> Option<Vec<Row>> {
        match &self.said {
            Reading::NotYet => return Some(vec![Row::Note { key: "queue-loading".into(), words: "Loading the Work Queue…".into(), busy: true }, Row::End]),
            Reading::Failed(why) => return Some(vec![Row::Note { key: "queue-failed".into(), words: format!("The Work Queue couldn’t be read: {why}"), busy: false }, Row::End]),
            Reading::Ready(_) => {}
        }
        if self.query.is_empty() {
            return None;
        }
        match &self.found {
            Reading::NotYet => Some(vec![Row::Note { key: "search-loading".into(), words: format!("Searching for “{}”…", self.query), busy: true }, Row::End]),
            Reading::Failed(why) => Some(vec![Row::Note { key: "search-failed".into(), words: format!("The search couldn’t be read: {why}"), busy: false }, Row::End]),
            Reading::Ready(_) => None,
        }
    }

    fn list_rows(&self, scope: &Scope) -> Vec<Row> {
        if let Some(rows) = self.unread() {
            return rows;
        }
        let Some(said) = self.view() else { return Vec::new() };
        let narrowed = self.narrowed();
        let single = matches!(scope, Scope::Status(_));
        // The counts are the box's, of the view it listed (whole, or as a search narrowed it):
        // shown only where that's what's listed, so not under a label or a filter.
        let counted = *scope == Scope::All && self.filters.is_empty();
        let mut rows = Vec::new();
        for status in &said.statuses {
            if let Scope::Status(id) = scope
                && id != &status.id
            {
                continue;
            }
            let items: Vec<&Item> = said.items.iter().filter(|item| item.status == status.id && self.takes(item)).filter(|item| !matches!(scope, Scope::Label(label) if !item.labels.contains(label))).collect();
            if items.is_empty() {
                continue;
            }
            let folded = !single && !narrowed && self.is_folded(&status.id);
            if !single {
                let count = counted.then(|| said.counts.statuses.get(&status.id).copied()).flatten();
                rows.push(Row::Group { key: status.id.clone(), name: status.name.clone(), count, folded, first: rows.is_empty() });
            }
            if !folded {
                let last = items.len() - 1;
                rows.extend(items.into_iter().enumerate().map(|(ix, item)| Row::Item { item: Box::new(item.clone()), first: ix == 0, last: ix == last, deleted: false, busy: self.busy.contains(&item.id) }));
            }
        }
        if rows.is_empty() {
            let words = match scope {
                Scope::Status(id) if !self.lists_status(id) => "This status isn’t in the Work Queue any more.".to_owned(),
                Scope::Label(id) if self.label(id).is_none() => "This label isn’t in the Work Queue any more.".to_owned(),
                _ if !self.query.is_empty() => format!("Nothing matches “{}”.", self.query),
                _ if narrowed => "Nothing matches.".to_owned(),
                Scope::All => "Nothing in the Work Queue yet. Add an item with +, or let your agents add theirs as they work.".to_owned(),
                Scope::Status(id) => format!("Nothing is {} now.", self.status_name(id).unwrap_or_default()),
                Scope::Label(id) => format!("Nothing has the label {} yet.", self.label(id).map(|label| label.name.clone()).unwrap_or_default()),
            };
            rows.push(Row::Empty(words));
        }
        // What's deleted, the box keeps a while: under the whole queue, folded.
        if *scope == Scope::All {
            let removed: Vec<&Item> = said.removed.iter().filter(|item| self.takes(item)).collect();
            if !removed.is_empty() {
                let folded = !narrowed && self.is_folded(DELETED);
                rows.push(Row::Group { key: DELETED.into(), name: "Deleted".into(), count: counted.then_some(said.counts.removed), folded, first: false });
                if !folded {
                    let last = removed.len() - 1;
                    rows.extend(removed.into_iter().enumerate().map(|(ix, item)| Row::Item { item: Box::new(item.clone()), first: ix == 0, last: ix == last, deleted: true, busy: self.busy.contains(&item.id) }));
                }
            }
        }
        rows.push(Row::End);
        rows
    }

    fn item_rows(&self, id: &str) -> Vec<Row> {
        let mut rows = vec![Row::Back(self.scope_name(&self.scope))];
        let Some(item) = self.item(id).cloned() else {
            rows.push(match (&self.whole, &self.said) {
                (Reading::Failed(why), _) => Row::Note { key: "item-failed".into(), words: format!("The item couldn’t be read: {why}"), busy: false },
                (Reading::NotYet, _) | (_, Reading::NotYet) => Row::Note { key: "item-loading".into(), words: "Loading the item…".into(), busy: true },
                _ => Row::Note { key: "item-gone".into(), words: "This item isn’t in the Work Queue any more.".into(), busy: false },
            });
            rows.push(Row::End);
            return rows;
        };
        let now = self.now();
        rows.push(Row::Head { item: Box::new(item.clone()), busy: self.busy.contains(&item.id) });
        // Why something can't be done to it, in the box's words (`can.why`): said once, in the
        // notice it belongs to, else in one of its own.
        let mut why = item.can.why.clone().filter(|why| !why.trim().is_empty());
        if let Some(removed) = &item.removed {
            // (Bringing it back is offered where the box says it may be.)
            let act = item.can.restore.then(|| ("Restore".to_owned(), Act::Restore(item.id.clone())));
            rows.push(Row::Notice { key: "item-deleted".into(), tone: Tone::Quiet, title: format!("Deleted {}", ago(removed.at, now)), words: why.take().unwrap_or_else(|| "It’s kept a while, to bring back.".into()), act });
        }
        if let Some(handing) = &item.handing {
            rows.push(Row::Notice { key: "item-handing".into(), tone: Tone::Waiting, title: format!("On its way to {}", handing.name), words: why.take().unwrap_or_default(), act: None });
        }
        let whole = self.whole();
        if let Some(computer) = &item.computer {
            let name = crate::bots::computer_name(&computer.name);
            // As that computer last told it (`told`): what only it keeps isn't here.
            let read = if whole.is_some_and(|whole| whole.told) { format!("Shown as {name} last told {}. Its description, notes, files and links are on {name}.", self.this_box) } else { format!("Read from {name}.") };
            let words = match why.take() {
                Some(why) => format!("{read} {why}"),
                None => format!("{read} What you change here is changed there."),
            };
            rows.push(Row::Notice { key: "item-elsewhere".into(), tone: Tone::Quiet, title: format!("On {name}"), words, act: None });
        }
        if let Some(why) = why {
            rows.push(Row::Notice { key: "item-why".into(), tone: Tone::Quiet, title: String::new(), words: why, act: None });
        }
        if let Some(progress) = item.progress.as_ref().filter(|progress| progress.value.is_some() || progress.note.as_ref().is_some_and(|note| !note.is_empty())) {
            rows.push(Row::Progress { value: progress.value, note: progress.note.clone().unwrap_or_default() });
        }
        let Some(whole) = whole else {
            rows.push(match &self.whole {
                Reading::Failed(why) => Row::Note { key: "item-failed".into(), words: format!("The rest of it couldn’t be read: {why}"), busy: false },
                _ => Row::Note { key: "item-loading".into(), words: "Loading what it says…".into(), busy: true },
            });
            rows.push(Row::End);
            return rows;
        };
        if !whole.told {
            rows.push(Row::Section("Description".into()));
            if whole.body.trim().is_empty() {
                rows.push(Row::Note { key: "item-no-body".into(), words: "No description yet.".into(), busy: false });
            } else {
                rows.push(Row::Body(whole.body.clone()));
            }
        }
        if !item.sessions.is_empty() {
            rows.push(Row::Section("Sessions".into()));
            let last = item.sessions.len() - 1;
            rows.extend(item.sessions.iter().enumerate().map(|(ix, session)| Row::Session { session: session.clone(), first: ix == 0, last: ix == last }));
        }
        if !whole.links.is_empty() {
            rows.push(Row::Section("Links".into()));
            let last = whole.links.len() - 1;
            rows.extend(whole.links.iter().enumerate().map(|(ix, link)| Row::Link { link: link.clone(), first: ix == 0, last: ix == last }));
        }
        if !whole.files.is_empty() {
            rows.push(Row::Section("Files".into()));
            let last = whole.files.len() - 1;
            rows.extend(whole.files.iter().enumerate().map(|(ix, file)| Row::File { file: file.clone(), first: ix == 0, last: ix == last }));
        }
        if !whole.notes.is_empty() {
            rows.push(Row::Section("Notes".into()));
            let last = whole.notes.len() - 1;
            // The newest first.
            rows.extend(whole.notes.iter().rev().enumerate().map(|(ix, note)| Row::Said { ix, who: who(&note.by), ago: ago(note.at, now), text: note.text.clone(), first: ix == 0, last: ix == last }));
        }
        // Who added it and who changed it last, and whose queue it's in, as the box names them.
        let mut facts = Vec::new();
        let by = |who_: &Who| Some(who(who_)).filter(|name| !name.is_empty()).map(|name| format!(" by {name}")).unwrap_or_default();
        if item.created.at > 0. {
            facts.push(format!("Added{} {}", by(&item.created.by), ago(item.created.at, now)));
        }
        if item.updated.at > item.created.at {
            facts.push(format!("Changed{} {}", by(&item.updated.by), ago(item.updated.at, now)));
        }
        match self.queue().and_then(|said| said.owners.iter().find(|owner| owner.owner == item.owner)) {
            // (Your own queue: the items you added that are no conversation's.)
            Some(owner) if owner.kind == "you" => facts.push("In your own queue".into()),
            Some(owner) => facts.extend(owner.name().map(|owner| format!("In {owner}’s queue"))),
            None => {}
        }
        if !facts.is_empty() {
            rows.push(Row::Facts(facts.join(" · ")));
        }
        rows.push(Row::End);
        rows
    }

    /// The filters the box's view offers: its computers (where another told of its queue), its
    /// outside sources, and its owners.
    fn offered_filters(&self) -> Vec<(String, Vec<Filter>)> {
        let Some(said) = self.queue() else { return Vec::new() };
        let mut groups = Vec::new();
        if !said.computers.is_empty() {
            let mut computers = vec![Filter::Computer { key: String::new(), name: self.this_box.clone() }];
            computers.extend(said.computers.iter().map(|computer| Filter::Computer { key: computer.key.clone(), name: crate::bots::computer_name(&computer.name).to_owned() }));
            groups.push(("Computer".to_owned(), computers));
        }
        let mut sources: Vec<&String> = said.counts.sources.keys().collect();
        sources.sort();
        if !sources.is_empty() {
            groups.push(("Source".to_owned(), sources.into_iter().map(|kind| Filter::Source(kind.clone())).collect()));
        }
        // (Your own queue is named here: the box gives it no title.)
        let owners: Vec<Filter> = said.owners.iter().filter_map(|owner| Some(Filter::Owner { owner: owner.owner.clone(), name: owner.name().or_else(|| (owner.kind == "you").then(|| "You".to_owned()))? })).collect();
        if !owners.is_empty() {
            groups.push(("Owner".to_owned(), owners));
        }
        groups
    }

    // MARK: Changes

    /// What a control asked for.
    pub fn act(&mut self, act: Act, cx: &mut Context<Self>) {
        match act {
            Act::Open(item) => cx.emit(QueueEvent::Select(Some(format!("item:{item}")))),
            Act::Back => cx.emit(QueueEvent::Select(self.scope.key())),
            Act::Session(session) => cx.emit(QueueEvent::Session(session)),
            Act::Link(url) => {
                if kuvo_ui::markdown::opens(&url) {
                    cx.open_url(&url);
                }
            }
            Act::Scope(scope) => cx.emit(QueueEvent::Select(scope.key())),
            Act::Fold(key) => {
                let folded = self.is_folded(&key);
                self.folds.insert(key, !folded);
            }
            Act::AddFilter(filter) => {
                if !self.filters.iter().any(|each| each.same(&filter)) {
                    self.filters.push(filter);
                    self.list.scroll_to(gpui::ListOffset::default());
                }
            }
            Act::RemoveFilter(ix) => {
                if ix < self.filters.len() {
                    self.filters.remove(ix);
                }
            }
            Act::ClearFilters => self.filters.clear(),
            Act::Set { item, change } => {
                let asked = item.clone();
                self.change(&item, move |client| client.change_work_item(&asked, &change), cx);
            }
            Act::Delete(item) => {
                let asked = item.clone();
                self.change(&item, move |client| client.delete_work_item(&asked), cx);
            }
            Act::Restore(item) => {
                let asked = item.clone();
                self.change(&item, move |client| client.restore_work_item(&asked), cx);
            }
            Act::New => {
                self.title.update(cx, |field, cx| field.set_text("", cx));
                self.body.update(cx, |field, cx| field.set_text("", cx));
                // What the box says a new item starts as (`new`), shown and sent, so what's shown
                // is what's made; then as the list shown: its status, or its label.
                let starts = self.queue().and_then(|said| said.new.clone());
                self.draft = Draft { status: starts.as_ref().map(|starts| starts.status.clone()), priority: starts.map(|starts| starts.priority), ..Draft::default() };
                match &self.scope {
                    Scope::Status(id) if self.lists_status(id) => self.draft.status = Some(id.clone()),
                    Scope::Label(id) if self.label(id).is_some() => self.draft.labels = vec![id.clone()],
                    _ => {}
                }
                self.created = None;
                self.open_sheet(Sheet::New, Some(self.title.focus_handle(cx)));
            }
            Act::Edit => {
                // (Its title and description, where the box says they may be changed.)
                let Some(whole) = self.whole().filter(|whole| whole.item.can.edit).cloned() else { return };
                self.title.update(cx, |field, cx| field.set_text(whole.item.title.clone(), cx));
                self.body.update(cx, |field, cx| {
                    field.set_text(whole.body.clone(), cx);
                    // (From its start, so a long one opens on its first lines.)
                    field.select_range(0..0, cx);
                });
                self.open_sheet(Sheet::Edit(whole.item.id), Some(self.title.focus_handle(cx)));
            }
            Act::Pick { target, field } => self.open_picker(target, field, cx),
            Act::ClosePicker => self.close_picker(cx),
            Act::CreateMore(on) => self.create_more = on,
            Act::NewLabel => {
                self.name.update(cx, |field, cx| field.set_text("", cx));
                self.label_color = None;
                self.open_sheet(Sheet::NewLabel, Some(self.name.focus_handle(cx)));
            }
            Act::EditLabel(id) => {
                let Some(label) = self.label(&id).cloned() else { return };
                self.name.update(cx, |field, cx| {
                    let end = label.name.len();
                    field.set_text(label.name.clone(), cx);
                    field.select_range(0..end, cx);
                });
                self.label_color = Some(label.color);
                self.open_sheet(Sheet::EditLabel(id), Some(self.name.focus_handle(cx)));
            }
            Act::RemoveLabel(id) => {
                if self.label(&id).is_some() {
                    self.open_sheet(Sheet::RemoveLabel(id), None);
                }
            }
            Act::LabelColor(color) => self.label_color = color,
        }
        self.rebuild(cx);
    }

    /// A change to one item: busy while it goes, then the box's answer drawn, or its refusal over
    /// the page. One at a time for an item.
    fn change(&mut self, item: &str, work: impl FnOnce(&BoxClient) -> kuvo_box::Result<Item> + Send + 'static, cx: &mut Context<Self>) {
        if self.busy.iter().any(|busy| busy == item) {
            return;
        }
        self.problem = None;
        self.busy.push(item.to_owned());
        let id = item.to_owned();
        self.run(
            format!("change-{item}"),
            work,
            move |this, answer, cx| {
                this.busy.retain(|busy| busy != &id);
                match answer {
                    Ok(now) => this.took(now, cx),
                    // Why not, over the list or its own page; over another item's page, nothing
                    // is said (nothing changed, and it wouldn't say of what).
                    Err(error) if matches!(&this.shown, Shown::List(_)) || this.shown == Shown::Item(id.clone()) => this.problem = Some(said(&error)),
                    Err(_) => {}
                }
            },
            cx,
        );
    }

    /// An item as the box answered it, drawn at once; its place, the counts and whatever else
    /// changed with it are the box's to say, read again (and searched again, where something is).
    fn took(&mut self, now: Item, cx: &mut Context<Self>) {
        let id = now.id.clone();
        let place = |said: &mut QueueSaid, add: bool| {
            let listed = said.items.iter().position(|item| item.id == id);
            let removed = said.removed.iter().position(|item| item.id == id);
            match (now.removed.is_some(), listed, removed) {
                (false, Some(at), _) => said.items[at] = now.clone(),
                (false, None, was) => {
                    if let Some(at) = was {
                        said.removed.remove(at);
                    }
                    if add || was.is_some() {
                        said.items.push(now.clone());
                    }
                }
                (true, was, None) => {
                    if let Some(at) = was {
                        said.items.remove(at);
                    }
                    if add || was.is_some() {
                        said.removed.insert(0, now.clone());
                    }
                }
                (true, _, Some(at)) => said.removed[at] = now.clone(),
            }
        };
        if let Reading::Ready(said) = &mut self.said {
            place(said, true);
        }
        // (What a search found: changed where it's listed; whether it still matches, or does now,
        // is the box's to say.)
        if let Reading::Ready(found) = &mut self.found {
            place(found, false);
        }
        if let Reading::Ready(whole) = &mut self.whole
            && whole.item.id == id
        {
            whole.item = now;
        }
        cx.emit(QueueEvent::Changed);
        self.search(cx);
        // The item's page: what it says now, whole.
        if self.shown == Shown::Item(id) {
            self.read(cx);
        }
    }

    /// The labels as the box answered them, drawn at once.
    fn took_labels(&mut self, labels: Vec<Label>, cx: &mut Context<Self>) {
        if let Reading::Ready(said) = &mut self.said {
            said.labels = labels.clone();
        }
        if let Reading::Ready(found) = &mut self.found {
            found.labels = labels;
        }
        cx.emit(QueueEvent::Changed);
        self.search(cx);
    }

    fn open_sheet(&mut self, sheet: Sheet, focus: Option<FocusHandle>) {
        self.sheet_opened += 1;
        self.sheet = Some(sheet);
        self.picker = None;
        self.sheet_error = None;
        self.saving = false;
        self.problem = None;
        self.wants_focus = Some(focus.unwrap_or_else(|| self.sheet_focus.clone()));
    }

    pub fn close_sheet(&mut self, cx: &mut Context<Self>) {
        // (One whose change is on its way stays until the box answers.)
        if self.saving {
            return;
        }
        self.sheet = None;
        self.picker = None;
        self.sheet_error = None;
        self.wants_focus = Some(self.focus.clone());
        cx.notify();
    }

    /// The sheet's Save: sent to the box as written, less the space around it (what an item or a
    /// label may be called is the box's to say), the sheet staying until it answers.
    pub fn save_sheet(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(sheet) = self.sheet.clone() else { return };
        self.saving = true;
        self.sheet_error = None;
        self.picker = None;
        let (title, body, name) = (self.title.read(cx).text().trim().to_owned(), self.body.read(cx).text().trim().to_owned(), self.name.read(cx).text().trim().to_owned());
        let (draft, color) = (self.draft.clone(), self.label_color.clone());
        let (sent, opened) = (sheet.clone(), self.sheet_opened);
        enum Answer {
            Item(Box<kuvo_box::Result<Item>>),
            Labels(kuvo_box::Result<Vec<Label>>),
        }
        // (Under its own name: a later sheet's save never drops this one's answer.)
        self.run(
            format!("sheet-{opened}"),
            move |client| match &sheet {
                Sheet::New => Answer::Item(Box::new(client.add_work_item(&NewItem { title, body, status: draft.status, priority: draft.priority, labels: draft.labels, assignee: draft.assignee, due: draft.due }))),
                Sheet::Edit(item) => Answer::Item(Box::new(client.change_work_item(item, &ItemChange { title: Some(title), body: Some(body), ..ItemChange::default() }))),
                Sheet::NewLabel => Answer::Labels(client.add_label(&name, color.as_deref()).map(|(labels, _)| labels)),
                Sheet::EditLabel(id) => Answer::Labels(client.change_label(id, Some(&name), color.as_deref()).map(|(labels, _)| labels)),
                Sheet::RemoveLabel(id) => Answer::Labels(client.remove_label(id).map(|(labels, _)| labels)),
            },
            move |this, answer, cx| {
                // The sheet it was sent from, still open where it was: it closes, or says why not.
                let its_sheet = this.sheet.is_some() && this.sheet_opened == opened;
                if its_sheet {
                    this.saving = false;
                }
                let mut made = None;
                let failed = match answer {
                    Answer::Item(item) => match *item {
                        Ok(item) => {
                            made = item.label();
                            this.took(item, cx);
                            None
                        }
                        Err(error) => Some(said(&error)),
                    },
                    Answer::Labels(Ok(labels)) => {
                        this.took_labels(labels, cx);
                        // A label that's gone takes its list with it: back to the whole queue.
                        if let Sheet::RemoveLabel(id) = &sent
                            && this.shown == Shown::List(Scope::Label(id.clone()))
                        {
                            cx.emit(QueueEvent::Select(None));
                        }
                        None
                    }
                    Answer::Labels(Err(error)) => Some(said(&error)),
                };
                match failed {
                    // Create more: it stays, its text cleared for the next one.
                    None if its_sheet && sent == Sheet::New && this.create_more => {
                        this.title.update(cx, |field, cx| field.set_text("", cx));
                        this.body.update(cx, |field, cx| field.set_text("", cx));
                        this.created = Some(made.map_or_else(|| "Added.".to_owned(), |label| format!("Added {label}.")));
                        this.wants_focus = Some(this.title.focus_handle(cx));
                    }
                    None if its_sheet => {
                        this.sheet = None;
                        this.wants_focus = Some(this.focus.clone());
                    }
                    // (Gone elsewhere meanwhile: nothing is said over another page.)
                    Some(why) if its_sheet => {
                        this.sheet_error = Some(why);
                        // The keyboard goes back to its first field, to put it right.
                        this.wants_focus = Some(match sent {
                            Sheet::New | Sheet::Edit(_) => this.title.focus_handle(cx),
                            Sheet::NewLabel | Sheet::EditLabel(_) => this.name.focus_handle(cx),
                            Sheet::RemoveLabel(_) => this.sheet_focus.clone(),
                        });
                    }
                    _ => {}
                }
            },
            cx,
        );
        self.rebuild(cx);
    }
}

/// Who did something, by name: you, or the name the box gave (none: unsaid).
fn who(who: &Who) -> String {
    if who.user { "You".to_owned() } else { who.name.clone().unwrap_or_default() }
}

/// "3h ago", or "just now".
fn ago(then: f64, now: f64) -> String {
    let ago = time::ago(then, now);
    if ago == "now" || ago.is_empty() { "just now".to_owned() } else { format!("{ago} ago") }
}

/// An outside source by name, from its kind (one this doesn't know, as the box wrote it).
pub fn source_name(kind: &str) -> String {
    match kind {
        "github" | "github-pr" => "GitHub".to_owned(),
        other => other.to_owned(),
    }
}

/// A file's size, in a few characters.
fn size_words(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_size_and_a_time_ago_read_short() {
        assert_eq!((size_words(512), size_words(184_320), size_words(1_258_291)), ("512 B".into(), "180 KB".into(), "1.2 MB".into()));
        let hour = 3_600_000.;
        assert_eq!(ago(hour, 4. * hour), "3h ago");
    }
}
