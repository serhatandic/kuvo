//! The Work Queue area's main view: the queue (its items under the box's statuses, with scope tabs,
//! a search the box answers, removable filters, and what's closed folded), an item's page (what it
//! says, how far it is, its notes, files, links and the sessions working on it), and what you do:
//! add an item, change what one says, where it stands, its priority, labels, assignee and due day,
//! close it, delete it and bring it back, and make, change and remove labels. Everything on it is
//! the box's (`kuvo/work-queue?owner=all&counts=1`, with `q` for a search;
//! `kuvo/work-queue/items/<id>`): its statuses, priorities and labels, its order, its counts, and
//! what may be done to each item (`can`). Every change goes to the box's route and what it answers
//! is what's drawn after, or why not, in its words. The app decides nothing but what it draws: a
//! search, the filters and a label's list are sent to the box, which narrows its view by them.
//!
//! Drawn as Bots' and Settings' pages are: the page's rows worked out once each time what they're
//! drawn from changes, then a virtual list that lays out only what's on screen.

pub(crate) mod attachments;
mod editor;
pub mod marks;
mod references;
mod relations;
mod page;
mod picker;

use std::collections::HashMap;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, Pixels, ScrollHandle, Size, Subscription, Task, px};
use kuvo_box::BoxClient;
use kuvo_box::accounts::said;
use kuvo_box::work_queue::{File, Item, ItemChange, ItemSession, Label, Link, Narrow, NewItem, QueueSaid, Who, Whole};
use kuvo_ui::{TextField, TextFieldEvent};

pub use picker::{Field, Picker, Target};

use crate::areas::Reading;
use crate::time;

pub(crate) fn bind_keys(cx: &mut App) {
    editor::bind_keys(cx);
}

/// The fold's key for what's deleted.
pub const DELETED: &str = "deleted";

/// The priority no mark is drawn for on a row: the one an item has when none was ever set.
pub const NORMAL: &str = "normal";

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

/// A filter over the list, shown as a chip to take off. The box applies it (`computers`, `sources`,
/// `owners` on the view's route): the app only says which are on.
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
            Filter::Source(kind) => source_name(kind),
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
    ToggleEditorSize,
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
    ItemLinks { relations: Vec<kuvo_box::work_queue::ItemRelation>, can_link: bool },
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
    /// The box's view narrowed by the search, as it last answered one, and the words it answered
    /// for: drawn only while they're what's searched for.
    found: Reading<QueueSaid>,
    found_for: String,
    /// The narrowed view being asked for now (its route): one at a time.
    narrow_flying: Option<String>,
    /// Asked for again while one was on its way: asked when it answers.
    narrow_again: bool,
    /// Changes to an item sent and not answered yet, and the latest wanted while one is on its
    /// way (sent when it answers), by the item's id.
    flying: HashMap<String, ItemChange>,
    waiting: HashMap<String, ItemChange>,
    /// What Edit Item opened on (a title and a description): only what's changed is sent.
    editing: Option<(String, String)>,
    /// The field of the item being written that last had the keyboard.
    text_focus: Option<FocusHandle>,
    tabs_scroll: ScrollHandle,
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
    body_composer: Entity<crate::composer::Composer>,
    picture_reads: HashMap<String, crate::pictures::Read>,
    viewing: Option<kuvo_box::pictures::Picture>,
    viewer_focus: FocusHandle,
    viewer_return: Option<FocusHandle>,
    enlarged: bool,
    draft_mentions: (String, Vec<kuvo_box::work_queue::Mention>),
    suggestion_at: usize,
    suggestions_dismissed: bool,
    return_to_draft: Option<(Shown, Reading<Whole>)>,
    relation_picker: Option<relations::Picker>,
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
        let title = cx.new(|cx| TextField::new(cx).bare().title().placeholder("Item title"));
        let body_composer = cx.new(|cx| crate::composer::Composer::item(client.clone(), cx));
        let body = body_composer.read(cx).field().clone();
        let name = cx.new(|cx| TextField::new(cx).placeholder("Name"));
        let picker_field = cx.new(|cx| TextField::new(cx).bare());
        let subscriptions = vec![
            cx.observe(&body_composer, |_, _, cx| cx.notify()),
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
                match event {
                    TextFieldEvent::Cancel => this.close_sheet(cx),
                    TextFieldEvent::Changed => this.refresh_draft_mentions(cx),
                    _ => {}
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
            found_for: String::new(),
            narrow_flying: None,
            narrow_again: false,
            flying: HashMap::new(),
            waiting: HashMap::new(),
            editing: None,
            text_focus: None,
            tabs_scroll: ScrollHandle::new(),
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
            body_composer,
            picture_reads: HashMap::new(),
            viewing: None,
            viewer_focus: cx.focus_handle(),
            viewer_return: None,
            enlarged: false,
            draft_mentions: Default::default(),
            suggestion_at: 0,
            suggestions_dismissed: false,
            return_to_draft: None,
            relation_picker: None,
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
        let sized = kuvo_ui::theme::remeasure_on_text_size(&view.list, cx);
        view._subscriptions.push(sized);
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

    /// The description composer, including its shared attachment draft.
    pub fn item_composer(&self) -> &Entity<crate::composer::Composer> {
        &self.body_composer
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

    /// The day `offset` days from today, as the box writes a day. Today is the box's (`today` on
    /// its view), never this computer's clock; nothing where the box hasn't said it.
    fn today(&self, offset: i64) -> String {
        self.queue().and_then(|said| time::day_after(&said.today, offset)).unwrap_or_default()
    }

    /// The box's view, as the sidebar read it.
    pub fn set_said(&mut self, said: Reading<QueueSaid>, cx: &mut Context<Self>) {
        if self.said != said {
            self.said = said;
            // (The queue changed: a narrowed list, which is the box's answer to another ask, is
            // asked for again, or it would show what was until you changed something.)
            if !self.narrowing().is_empty() {
                self.search(cx);
            }
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

    /// What narrows the list now, for the box to apply: what's searched for, the filters that are
    /// on, and the label whose list shows.
    pub fn narrowing(&self) -> Narrow {
        let mut by = Narrow { q: self.query.clone(), ..Narrow::default() };
        for filter in &self.filters {
            match filter {
                Filter::Computer { key, .. } => by.computers.push(key.clone()),
                Filter::Source(kind) => by.sources.push(kind.clone()),
                Filter::Owner { owner, .. } => by.owners.push(owner.clone()),
            }
        }
        if let Shown::List(Scope::Label(id)) = &self.shown {
            by.labels.push(id.clone());
        }
        by
    }

    /// What the list is drawn from: the box's view, or, while anything narrows it, the view the
    /// box narrowed by exactly that.
    fn view(&self) -> Option<&QueueSaid> {
        let by = self.narrowing();
        if by.is_empty() {
            return self.queue();
        }
        match &self.found {
            Reading::Ready(found) if self.found_for == by.route() => Some(found),
            _ => None,
        }
    }

    /// Whether the box is yet to answer for what narrows the list now.
    pub fn searching(&self) -> bool {
        let by = self.narrowing();
        !by.is_empty() && self.found_for != by.route()
    }

    /// The line at an item's page's foot (who added and changed it, whose queue it's in), as drawn.
    pub fn facts_line(&self) -> Option<String> {
        self.rows.iter().find_map(|row| match row {
            Row::Facts(words) => Some(words.clone()),
            _ => None,
        })
    }

    /// A notice on the page, by its key: its words, as they're drawn.
    pub fn notice(&self, key: &str) -> Option<String> {
        self.rows.iter().find_map(|row| match row {
            Row::Notice { key: each, words, .. } if each == key => Some(words.clone()),
            _ => None,
        })
    }

    /// Asks the box for the view narrowed by what's typed and the filters on. One at a time: what
    /// narrows it when an answer comes is asked next (so words typed quickly are a request for the
    /// first and one for the last, not one a letter), and an answer is only ever taken for what
    /// narrows the list now.
    fn search(&mut self, cx: &mut Context<Self>) {
        let by = self.narrowing();
        if by.is_empty() {
            self.found = Reading::NotYet;
            self.found_for.clear();
            return;
        }
        let route = by.route();
        // (One's on its way: this is asked when it answers.)
        if self.narrow_flying.is_some() {
            self.narrow_again = true;
            return;
        }
        self.narrow_flying = Some(route.clone());
        self.narrow_again = false;
        self.run(
            "search",
            move |client| client.work_queue_narrowed(&by),
            move |this, read, cx| {
                this.narrow_flying = None;
                // Still what narrows the list: taken. Narrowed otherwise meanwhile: dropped.
                if this.narrowing().route() == route {
                    this.found_for = route;
                    this.found = match read {
                        Ok(found) => Reading::Ready(found),
                        Err(kuvo_box::Error::NeedsUpdating { .. }) => Reading::Ready(Default::default()),
                        Err(error) => Reading::Failed(error.to_string()),
                    };
                }
                // Asked for again meanwhile (other words, a filter, or a change to an item): now.
                if this.narrow_again {
                    this.search(cx);
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
            self.return_to_draft = None;
            self.relation_picker = None;
            self.viewing = None;
            self.picture_reads.clear();
            self.picker = None;
            self.sheet_error = None;
            // (A change still on its way answers for its own item, not for what's shown now.)
            self.saving = false;
            self.whole = Reading::NotYet;
            self.list.scroll_to(gpui::ListOffset::default());
            self.read(cx);
            // (A label's list is the box's too: its items, and its counts of them.)
            self.search(cx);
        }
        self.rebuild(cx);
    }

    /// Reads the item shown, whole (again, each time it's shown).
    pub fn read(&mut self, cx: &mut Context<Self>) {
        let Shown::Item(item) = self.shown.clone() else { return };
        let asked = item.clone();
        self.run("read-item", move |client| client.work_item(&asked), move |this, read, _| {
            if this.shown == Shown::Item(item) {
                this.whole = match (read, &this.whole) {
                    (Ok(value), _) => Reading::Ready(value),
                    (Err(_), Reading::Ready(value)) => Reading::Ready(value.clone()),
                    (Err(kuvo_box::Error::NeedsUpdating { .. }), _) => {
                        this.scope = Scope::All;
                        this.shown = Shown::List(Scope::All);
                        return;
                    },
                    (Err(error), _) => Reading::Failed(error.to_string()),
                };
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

    /// The statuses the box says an item is closed in (Done, Canceled), in its order, but `now`'s:
    /// what an item's Mark menu offers, the first as its page's finish button.
    fn closing(&self, now: &str) -> Vec<(String, String)> {
        self.queue().map(|said| said.statuses.iter().filter(|status| status.closed && status.id != now).map(|status| (status.id.clone(), status.name.clone())).collect()).unwrap_or_default()
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

    /// Keep the unresolved state visible in offscreen loading-state captures.
    #[cfg(feature = "shots")]
    pub fn show_loading_for_shot(&mut self, cx: &mut Context<Self>) {
        self.said = Reading::NotYet;
        self.rebuild(cx);
    }

    /// Folded unless opened: what's closed (as the box says a status is) and what's deleted.
    fn is_folded(&self, key: &str) -> bool {
        self.folds.get(key).copied().unwrap_or(key == DELETED || self.is_closed(key))
    }

    /// Whether the list is narrowed by the search or a filter: nothing stays folded then.
    fn narrowed(&self) -> bool {
        !self.query.is_empty() || !self.filters.is_empty()
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
        if self.narrowing().is_empty() {
            return None;
        }
        // (What the box answered for other words or filters is never drawn under these.)
        if self.searching() {
            let words = if self.query.is_empty() { "Loading…".to_owned() } else { format!("Searching for “{}”…", self.query) };
            return Some(vec![Row::Note { key: "search-loading".into(), words, busy: true }, Row::End]);
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
        // The counts are the box's, of the view it listed (whole, or as it narrowed it by a search,
        // a filter or a label): each status's, wherever every status shows.
        let counted = !single;
        let mut rows = Vec::new();
        for status in &said.statuses {
            if let Scope::Status(id) = scope
                && id != &status.id
            {
                continue;
            }
            let items: Vec<&Item> = said.items.iter().filter(|item| item.status == status.id).collect();
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
        // An item in a status the view doesn't list (kept as it came from a newer box) isn't hidden:
        // each such status is a group of its own, after the rest, under the name it came by.
        if !single {
            let mut unlisted: Vec<&str> = Vec::new();
            for item in said.items.iter().filter(|item| !said.statuses.iter().any(|status| status.id == item.status)) {
                if !unlisted.contains(&item.status.as_str()) {
                    unlisted.push(&item.status);
                }
            }
            for status in unlisted {
                let items: Vec<&Item> = said.items.iter().filter(|item| item.status == status).collect();
                let folded = !narrowed && self.is_folded(status);
                rows.push(Row::Group { key: status.to_owned(), name: status.to_owned(), count: None, folded, first: rows.is_empty() });
                if !folded {
                    let last = items.len() - 1;
                    rows.extend(items.into_iter().enumerate().map(|(ix, item)| Row::Item { item: Box::new(item.clone()), first: ix == 0, last: ix == last, deleted: false, busy: self.busy.contains(&item.id) }));
                }
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
            let removed: Vec<&Item> = said.removed.iter().collect();
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
        let mut rows = vec![Row::Back(if self.return_to_draft.is_some() { "Back to draft".into() } else { self.scope_name(&self.scope) })];
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
            let read = if whole.is_some_and(|whole| whole.told) { format!("As {name} last reported it. Its description, notes, files and links are on {name}.") } else { format!("Read from {name}.") };
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
        if !whole.links.is_empty() || !whole.relations.is_empty() || whole.can_link_items {
            rows.push(Row::ItemLinks { relations: whole.relations.clone(), can_link: whole.can_link_items });
        }
        if !whole.links.is_empty() {
            let last = whole.links.len() - 1;
            rows.extend(whole.links.iter().enumerate().map(|(ix, link)| Row::Link { link: link.clone(), first: ix == 0, last: ix == last }));
        }
        if !whole.files.is_empty() {
            let kept = attachments::kept(whole);
            let placed: std::collections::HashSet<_> = attachments::anchors(&whole.body, &kept).iter()
                .filter_map(|(_, url)| attachments::resolve(url, &kept).map(|file| file.name.as_str())).collect();
            let extra: Vec<_> = whole.files.iter().filter(|file| !placed.contains(file.name.as_str())).collect();
            if !extra.is_empty() {
                rows.push(Row::Section("Files".into()));
                let last = extra.len() - 1;
                rows.extend(extra.into_iter().enumerate().map(|(ix, file)| Row::File { file: file.clone(), first: ix == 0, last: ix == last }));
            }
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
            // (Your own: the items you added that are no conversation's.)
            Some(owner) if owner.kind == "you" => facts.push("In your own list".into()),
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
        // (Each by the name the box gives it: your own list is "Your own".)
        let owners: Vec<Filter> = said.owners.iter().filter_map(|owner| Some(Filter::Owner { owner: owner.owner.clone(), name: owner.name()? })).collect();
        if !owners.is_empty() {
            groups.push(("Owner".to_owned(), owners));
        }
        groups
    }

    // MARK: Changes

    /// What a control asked for.
    pub fn act(&mut self, act: Act, cx: &mut Context<Self>) {
        if self.client.gate().is_held() && matches!(act, Act::New | Act::NewLabel) {
            return;
        }
        match act {
            Act::Open(item) if self.return_to_draft.is_some() => self.open_reference(item, cx),
            Act::Open(item) => cx.emit(QueueEvent::Select(Some(format!("item:{item}")))),
            Act::Back if self.return_to_draft.is_some() => {
                if let Some((shown, whole)) = self.return_to_draft.take() {
                    self.shown = shown;
                    self.whole = whole;
                    self.list.scroll_to(gpui::ListOffset::default());
                    self.wants_focus = Some(self.body.focus_handle(cx));
                }
            }
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
                    self.search(cx);
                }
            }
            Act::RemoveFilter(ix) => {
                if ix < self.filters.len() {
                    self.filters.remove(ix);
                    self.search(cx);
                }
            }
            Act::ClearFilters => {
                self.filters.clear();
                self.search(cx);
            }
            Act::Set { item, change } => {
                // One at a time for an item: while one is on its way, the latest wanted waits
                // behind it (what's said later wins, field by field) and goes when it answers.
                if self.busy.contains(&item) {
                    let waiting = self.waiting.entry(item).or_default();
                    merge(waiting, &change);
                } else {
                    self.flying.insert(item.clone(), (*change).clone());
                    let asked = item.clone();
                    self.change(&item, move |client| client.change_work_item(&asked, &change), cx);
                }
            }
            // (Neither while a change to it is on its way: `change` takes one at a time.)
            Act::Delete(item) => {
                let asked = item.clone();
                self.change(&item, move |client| client.delete_work_item(&asked), cx);
            }
            Act::Restore(item) => {
                let asked = item.clone();
                self.change(&item, move |client| client.restore_work_item(&asked), cx);
            }
            Act::New if self.return_to_draft.is_none() => {
                self.title.update(cx, |field, cx| field.set_text("", cx));
                // (Files go with a new one where this computer keeps them.)
                let attaching = self.client.gate().has("work-queue.attachments");
                self.body_composer.update(cx, |composer, cx| {
                    composer.clear_attachments(cx);
                    composer.set_attaching(attaching, cx);
                });
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
            Act::Edit if self.return_to_draft.is_none() => {
                // (Its title and description, where the box says they may be changed.)
                let Some(whole) = self.whole().filter(|whole| whole.item.can.edit).cloned() else { return };
                self.title.update(cx, |field, cx| field.set_text(whole.item.title.clone(), cx));
                self.body_composer.update(cx, |composer, cx| {
                    composer.clear_attachments(cx);
                    // (As its box says: where its computer keeps them.)
                    composer.set_attaching(whole.item.can.attach, cx);
                });
                self.body.update(cx, |field, cx| {
                    field.set_text(whole.body.clone(), cx);
                    // (From its start, so a long one opens on its first lines.)
                    field.select_range(0..0, cx);
                });
                self.draft_mentions = (whole.body.clone(), whole.mentions.clone());
                self.editing = Some((whole.item.title.trim().to_owned(), whole.body.trim().to_owned()));
                self.open_sheet(Sheet::Edit(whole.item.id), Some(self.title.focus_handle(cx)));
            }
            Act::New | Act::Edit => {}
            Act::ToggleEditorSize => self.enlarged = !self.enlarged,
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
                this.flying.remove(&id);
                match answer {
                    Ok(now) => {
                        this.took(now, cx);
                        // What was wanted of it meanwhile goes now.
                        if let Some(change) = this.waiting.remove(&id) {
                            this.act(Act::Set { item: id.clone(), change: Box::new(change) }, cx);
                        }
                    }
                    Err(error) => {
                        // (What waited behind a change the box refused isn't sent after it.)
                        this.waiting.remove(&id);
                        // Why not, over the list or its own page; over another item's page, nothing
                        // is said (nothing changed, and it wouldn't say of what).
                        if matches!(&this.shown, Shown::List(_)) || this.shown == Shown::Item(id.clone()) {
                            this.problem = Some(said(&error));
                        }
                    }
                }
            },
            cx,
        );
    }

    /// An item as the box answered it, drawn at once; its place, the counts and whatever else
    /// changed with it are the box's to say, read again (and searched again, where something is).
    fn took(&mut self, mut now: Item, cx: &mut Context<Self>) {
        let id = now.id.clone();
        // A change's answer says nothing of its sessions (the box names each "New Session", none
        // at work): what's known of them here is kept until the view is read again.
        if let Some(known) = self.item(&id).map(|item| item.sessions.clone()) {
            for session in &mut now.sessions {
                if let Some(known) = known.iter().find(|known| known.session == session.session) {
                    *session = known.clone();
                }
            }
        }
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
        self.enlarged = false;
        self.suggestions_dismissed = false;
        self.suggestion_at = 0;
        self.text_focus = None;
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
        self.body_composer.update(cx, |composer, cx| composer.clear_attachments(cx));
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
        let (title, name) = (self.title.read(cx).text().trim().to_owned(), self.name.read(cx).text().trim().to_owned());
        let files = self.body_composer.read(cx).item_attachments().map(|(key, path)| (key, path.clone())).collect();
        let (body, paths) = if matches!(sheet, Sheet::New | Sheet::Edit(_)) {
            attachments::for_save(self.body.read(cx).text().trim(), files)
        } else {
            (String::new(), Vec::new())
        };
        // An item rewritten: only what was changed is sent; nothing changed, nothing is.
        let mut rewritten = ItemChange::default();
        if let (Sheet::Edit(_), Some((was_title, was_body))) = (&sheet, &self.editing) {
            rewritten.title = (&title != was_title).then(|| title.clone());
            rewritten.body = (&body != was_body).then(|| body.clone());
            if rewritten == ItemChange::default() && paths.is_empty() {
                self.close_sheet(cx);
                return;
            }
        }
        self.saving = true;
        self.sheet_error = None;
        self.picker = None;
        let (draft, color) = (self.draft.clone(), self.label_color.clone());
        let (sent, opened) = (sheet.clone(), self.sheet_opened);
        enum Answer {
            Item(Box<kuvo_box::Result<Item>>),
            Labels(kuvo_box::Result<Vec<Label>>),
        }
        let pasted = crate::pasted::folder(cx);
        let remote_item = matches!(sheet, Sheet::Edit(_)) && self.whole().is_some_and(|whole| whole.item.computer.is_some() || whole.told);
        // (Under its own name: a later sheet's save never drops this one's answer.)
        self.run(
            format!("sheet-{opened}"),
            move |client| {
                let files = match kuvo_box::parts::carried(&paths, client.here() && !remote_item, &|path| path.starts_with(&pasted)) {
                    Ok(files) => files,
                    Err(error) => return Answer::Item(Box::new(Err(kuvo_box::Error::BadAnswer(error)))),
                };
                rewritten.files = files.clone();
                match &sheet {
                    Sheet::New => Answer::Item(Box::new(client.add_work_item(&NewItem { title, body, files, status: draft.status, priority: draft.priority, labels: draft.labels, assignee: draft.assignee, due: draft.due }))),
                    Sheet::Edit(item) => Answer::Item(Box::new(client.change_work_item(item, &rewritten))),
                    Sheet::NewLabel => Answer::Labels(client.add_label(&name, color.as_deref()).map(|(labels, _)| labels)),
                    Sheet::EditLabel(id) => Answer::Labels(client.change_label(id, Some(&name), color.as_deref()).map(|(labels, _)| labels)),
                    Sheet::RemoveLabel(id) => Answer::Labels(client.remove_label(id).map(|(labels, _)| labels)),
                }
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
                        this.body_composer.update(cx, |composer, cx| composer.clear_attachments(cx));
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

/// Puts what `later` says over what `wanted` says, field by field.
fn merge(wanted: &mut ItemChange, later: &ItemChange) {
    macro_rules! over {
        ($($field:ident),*) => { $(if later.$field.is_some() { wanted.$field = later.$field.clone(); })* };
    }
    over!(title, body, status, priority, labels, assignee, due);
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
        "github" => "GitHub issues".to_owned(),
        "github-pr" => "GitHub pull requests".to_owned(),
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
