//! The Work Queue area's main view: the queue (its items under the box's categories, with scope
//! tabs, a search, removable filters, and Done folded), an item's page (what it says, how far it
//! is, its notes, files, links and the sessions working on it), and what you do to one: move it
//! between categories, mark it done, delete it and bring it back; add one and change what it says
//! where the box takes that. Everything on it is the box's (`kuvo/work-queue?owner=all&counts=1`,
//! `kuvo/work-queue/items/<id>`): its categories, whatever they are, its order, its counts. Every
//! change goes to the box's route and what it answers is what's drawn after, or why not, in its
//! words. The app decides nothing but what it draws: the search and the filters only choose which
//! of the box's items show.
//!
//! Drawn as Bots' and Settings' pages are: the page's rows worked out once each time what they're
//! drawn from changes, then a virtual list that lays out only what's on screen.

mod page;

use std::collections::HashMap;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, Subscription, Task, px};
use kuvo_box::BoxClient;
use kuvo_box::accounts::said;
use kuvo_box::areas::Category;
use kuvo_box::work_queue::{File, Item, ItemSession, Link, NewItem, QueueSaid, Who, Whole};
use kuvo_ui::{TextField, TextFieldEvent};

use crate::areas::{Reading, reading};
use crate::time;

// BOX-NEEDED: whether an item is done as a state of its own (`state` on an item, and a route to
// finish one), so Done needn't be a category. Until then Done is the protocol's built-in `done`
// category, and marking one done is a move there (`POST kuvo/work-queue/items/<item>/move`).
/// The category the protocol keeps finished items in.
pub const DONE: &str = "done";

/// The fold's key for what's deleted.
pub const DELETED: &str = "deleted";

/// Which of the queue's items the list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    /// Those a session is at work on now.
    Progress,
    /// One of the box's categories, by its id.
    Category(String),
}

impl Scope {
    /// The sidebar's row for it.
    pub fn key(&self) -> Option<String> {
        match self {
            Scope::All => None,
            Scope::Progress => Some("progress".into()),
            Scope::Category(id) => Some(format!("category:{id}")),
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
    /// Move an item to a category (by its id): Done marks it done.
    Move { item: String, category: String },
    Delete(String),
    Restore(String),
    New,
    Edit,
    FormCategory(String),
    NewCategory,
    RenameCategory(String),
    RemoveCategory(String),
}

/// A sheet over the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sheet {
    New,
    /// The item being rewritten, by its id.
    Edit(String),
    NewCategory,
    RenameCategory(String),
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
    /// A category's header (or Deleted's): its name, the box's count where it gave one for what's
    /// listed, and whether it's folded.
    Group { key: String, name: String, count: Option<usize>, folded: bool, first: bool },
    Item { item: Box<Item>, category: String, first: bool, last: bool, deleted: bool, busy: bool },
    /// Nothing to list, in a line.
    Empty(String),
    Note { key: String, words: String, busy: bool },
    /// The way back from an item to the list it was opened from.
    Back(String),
    Head { item: Box<Item>, category: String, busy: bool },
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
    /// The box takes an item added or rewritten by a client (`work-queue.edit`).
    writes: bool,
    search: Entity<TextField>,
    query: String,
    filters: Vec<Filter>,
    /// Groups folded or unfolded by hand, by their key (a category's id, or Deleted).
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
    /// The sheets' fields, made once: an item's title and description, a category's name.
    title: Entity<TextField>,
    body: Entity<TextField>,
    name: Entity<TextField>,
    /// The category a new item goes in (none: the box's choice).
    form_category: Option<String>,
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
        let title = cx.new(|cx| TextField::new(cx).placeholder("What needs doing"));
        let body = cx.new(|cx| TextField::new(cx).multi_line(6, 14).placeholder("What it is, and how to tell it’s done. Markdown works."));
        let name = cx.new(|cx| TextField::new(cx).placeholder("Name"));
        let subscriptions = vec![
            cx.subscribe(&search, |this: &mut QueueView, field, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Changed => {
                    let query = field.read(cx).text().trim().to_lowercase();
                    if this.query != query {
                        this.query = query;
                        this.list.scroll_to(gpui::ListOffset::default());
                        this.rebuild(cx);
                    }
                }
                TextFieldEvent::Cancel => field.update(cx, |field, cx| field.set_text("", cx)),
                TextFieldEvent::Submit(_) => {}
            }),
            cx.subscribe(&title, |this: &mut QueueView, _, event: &TextFieldEvent, cx| this.sheet_key(event, cx)),
            cx.subscribe(&name, |this: &mut QueueView, _, event: &TextFieldEvent, cx| this.sheet_key(event, cx)),
            cx.subscribe(&body, |this: &mut QueueView, _, event: &TextFieldEvent, cx| {
                if matches!(event, TextFieldEvent::Cancel) {
                    this.close_sheet(cx);
                }
            }),
        ];
        let mut view = QueueView {
            client,
            live,
            said: Reading::NotYet,
            shown: Shown::List(Scope::All),
            scope: Scope::All,
            whole: Reading::NotYet,
            writes: false,
            search,
            query: String::new(),
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
            form_category: None,
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

    /// The field a category is named in.
    pub fn name_field(&self) -> &Entity<TextField> {
        &self.name
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

    /// Whether the box takes an item added or rewritten by a client, as its `features` say.
    pub fn set_writes(&mut self, writes: bool, cx: &mut Context<Self>) {
        if self.writes != writes {
            self.writes = writes;
            self.rebuild(cx);
        }
    }

    pub fn writes(&self) -> bool {
        self.writes
    }

    fn now(&self) -> f64 {
        self.now.unwrap_or_else(time::now_ms)
    }

    /// The box's view, as the sidebar read it.
    pub fn set_said(&mut self, said: Reading<QueueSaid>, cx: &mut Context<Self>) {
        if self.said != said {
            self.said = said;
            self.rebuild(cx);
        }
    }

    fn queue(&self) -> Option<&QueueSaid> {
        match &self.said {
            Reading::Ready(said) => Some(said),
            _ => None,
        }
    }

    /// Shows what the sidebar's row `key` names (none: the whole queue).
    pub fn show(&mut self, key: Option<&str>, cx: &mut Context<Self>) {
        let shown = match key.map(|key| key.split_once(':').unwrap_or((key, ""))) {
            Some(("item", id)) => Shown::Item(id.to_owned()),
            Some(("category", id)) => Shown::List(Scope::Category(id.to_owned())),
            Some(("progress", _)) => Shown::List(Scope::Progress),
            _ => Shown::List(Scope::All),
        };
        if self.shown != shown {
            if let Shown::List(scope) = &shown {
                self.scope = scope.clone();
            }
            self.shown = shown;
            self.problem = None;
            self.sheet = None;
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

    fn categories(&self) -> &[Category] {
        self.queue().map(|said| said.categories.as_slice()).unwrap_or_default()
    }

    /// A category's name, as the box names it.
    fn category_name(&self, id: &str) -> Option<String> {
        self.categories().iter().find(|category| category.id == id).map(|category| category.name.clone())
    }

    /// What an item's chip says: what the box shows in place of its category, else the category's
    /// name (none where the box lists no such category).
    fn chip_of(&self, item: &Item) -> String {
        item.done_as.clone().or_else(|| self.category_name(&item.category)).unwrap_or_default()
    }

    /// The item shown: whole once read, else as the list has it.
    fn item(&self, id: &str) -> Option<&Item> {
        if let Some(whole) = self.whole() {
            return Some(&whole.item);
        }
        let said = self.queue()?;
        said.items.iter().chain(&said.removed).find(|item| item.id == id)
    }

    /// An item as the box's view lists it.
    fn item_listed(&self, id: &str) -> Option<&Item> {
        self.queue()?.items.iter().find(|item| item.id == id)
    }

    /// The box's view as it's drawn now (with what a change answered since it was read).
    pub fn said(&self) -> &Reading<QueueSaid> {
        &self.said
    }

    fn scope_name(&self, scope: &Scope) -> String {
        match scope {
            Scope::All => "Work Queue".into(),
            Scope::Progress => "In Progress".into(),
            Scope::Category(id) => self.category_name(id).unwrap_or_else(|| "Work Queue".into()),
        }
    }

    fn is_folded(&self, key: &str) -> bool {
        self.folds.get(key).copied().unwrap_or(key == DONE || key == DELETED)
    }

    /// Whether the list is narrowed by the search or a filter: the box's counts are of the whole
    /// view, so none shows then, and nothing stays folded.
    fn narrowed(&self) -> bool {
        !self.query.is_empty() || !self.filters.is_empty()
    }

    /// Whether the search and the filters take an item: every kind of filter picked, any one of a
    /// kind.
    fn takes(&self, item: &Item) -> bool {
        let filtered = (0..3u8).all(|kind| {
            let mut of_kind = self.filters.iter().filter(|filter| filter.kind() == kind).peekable();
            of_kind.peek().is_none() || of_kind.any(|filter| filter.takes(item))
        });
        // BOX-NEEDED: a search of what items say and their notes (`kuvo/work-queue?q=`): the list
        // carries no descriptions, so this only finds titles and numbers.
        let found = self.query.is_empty() || item.title.to_lowercase().contains(&self.query) || item.label().is_some_and(|label| label.to_lowercase().contains(&self.query));
        filtered && found
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
            Reading::NotYet => Some(vec![Row::Note { key: "queue-loading".into(), words: "Loading the Work Queue…".into(), busy: true }, Row::End]),
            Reading::Failed(why) => Some(vec![Row::Note { key: "queue-failed".into(), words: format!("The Work Queue couldn’t be read: {why}"), busy: false }, Row::End]),
            Reading::Ready(_) => None,
        }
    }

    fn list_rows(&self, scope: &Scope) -> Vec<Row> {
        if let Some(rows) = self.unread() {
            return rows;
        }
        let Some(said) = self.queue() else { return Vec::new() };
        let narrowed = self.narrowed();
        let single = matches!(scope, Scope::Category(_));
        let mut rows = Vec::new();
        for category in &said.categories {
            if let Scope::Category(id) = scope
                && id != &category.id
            {
                continue;
            }
            let items: Vec<&Item> = said.items.iter().filter(|item| item.category == category.id && (*scope != Scope::Progress || item.in_progress) && self.takes(item)).collect();
            if items.is_empty() {
                continue;
            }
            let folded = !single && !narrowed && self.is_folded(&category.id);
            if !single {
                // The count is the box's (`counts`), of the whole view: shown only where that's
                // what's listed.
                let count = (!narrowed && *scope == Scope::All).then(|| said.counts.categories.get(&category.id).copied()).flatten();
                rows.push(Row::Group { key: category.id.clone(), name: category.name.clone(), count, folded, first: rows.is_empty() });
            }
            if !folded {
                let last = items.len() - 1;
                rows.extend(items.into_iter().enumerate().map(|(ix, item)| Row::Item { item: Box::new(item.clone()), category: self.chip_of(item), first: ix == 0, last: ix == last, deleted: false, busy: self.busy.contains(&item.id) }));
            }
        }
        if rows.is_empty() {
            let words = match scope {
                _ if single && self.category_name(match scope { Scope::Category(id) => id, _ => "" }).is_none() => "This category isn’t in the Work Queue any more.".to_owned(),
                _ if narrowed => "Nothing matches.".to_owned(),
                Scope::All if self.writes => "Nothing in the Work Queue yet.".to_owned(),
                Scope::All => "Nothing in the Work Queue yet. Agents add items here as they work.".to_owned(),
                Scope::Progress => "Nothing is in progress.".to_owned(),
                Scope::Category(id) => format!("Nothing in {} yet.", self.category_name(id).unwrap_or_default()),
            };
            rows.push(Row::Empty(words));
        }
        // What's deleted, the box keeps a while: under the whole queue, folded.
        if *scope == Scope::All {
            let removed: Vec<&Item> = said.removed.iter().filter(|item| self.takes(item)).collect();
            if !removed.is_empty() {
                let folded = !narrowed && self.is_folded(DELETED);
                rows.push(Row::Group { key: DELETED.into(), name: "Deleted".into(), count: (!narrowed).then_some(said.counts.removed), folded, first: false });
                if !folded {
                    let last = removed.len() - 1;
                    rows.extend(removed.into_iter().enumerate().map(|(ix, item)| Row::Item { item: Box::new(item.clone()), category: self.chip_of(item), first: ix == 0, last: ix == last, deleted: true, busy: self.busy.contains(&item.id) }));
                }
            }
        }
        // A box that takes no item added or rewritten by a client (every box today): said plainly,
        // where New Item would be looked for.
        if *scope == Scope::All && !self.writes {
            rows.push(Row::Note { key: "queue-read-only".into(), words: format!("Agents add items on {} and write what they say. From here you move them, mark them done, delete them and bring them back.", self.this_box), busy: false });
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
        rows.push(Row::Head { item: Box::new(item.clone()), category: self.chip_of(&item), busy: self.busy.contains(&item.id) });
        if let Some(removed) = &item.removed {
            rows.push(Row::Notice { key: "item-deleted".into(), tone: Tone::Quiet, title: format!("Deleted {}", ago(removed.at, now)), words: "It’s kept a while, to bring back.".into(), act: Some(("Restore".into(), Act::Restore(item.id.clone()))) });
        }
        if let Some(handing) = &item.handing {
            rows.push(Row::Notice { key: "item-handing".into(), tone: Tone::Waiting, title: format!("On its way to {}", handing.name), words: "It can’t be changed until it’s there.".into(), act: None });
        }
        let whole = self.whole();
        if let Some(computer) = &item.computer {
            let name = crate::bots::computer_name(&computer.name);
            // As that computer last told it (`told`): what only it keeps isn't here.
            let words = if whole.is_some_and(|whole| whole.told) { format!("Shown as {name} last told {}. Its description, notes, files and links are on {name}.", self.this_box) } else { format!("Read from {name}. What you change here is changed there.") };
            rows.push(Row::Notice { key: "item-elsewhere".into(), tone: Tone::Quiet, title: format!("On {name}"), words, act: None });
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
        if let Some(owner) = self.queue().and_then(|said| said.owners.iter().find(|owner| owner.owner == item.owner)).and_then(|owner| owner.name()) {
            facts.push(format!("In {owner}’s queue"));
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
        let owners: Vec<Filter> = said.owners.iter().filter_map(|owner| Some(Filter::Owner { owner: owner.owner.clone(), name: owner.name()? })).collect();
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
            Act::Move { item, category } => {
                // (To where it is already: nothing to ask.)
                if self.item_listed(&item).is_some_and(|listed| listed.category == category) {
                    return;
                }
                let asked = item.clone();
                self.change(&item, move |client| client.move_work_item(&asked, &category), cx);
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
                if !self.writes {
                    return;
                }
                self.title.update(cx, |field, cx| field.set_text("", cx));
                self.body.update(cx, |field, cx| field.set_text("", cx));
                // Into the category shown, where one is; else the box's choice.
                self.form_category = match &self.scope {
                    Scope::Category(id) => Some(id.clone()),
                    _ => None,
                };
                self.open_sheet(Sheet::New, Some(self.title.focus_handle(cx)));
            }
            Act::Edit => {
                let Some(whole) = self.whole().filter(|_| self.writes).cloned() else { return };
                self.title.update(cx, |field, cx| field.set_text(whole.item.title.clone(), cx));
                self.body.update(cx, |field, cx| {
                    field.set_text(whole.body.clone(), cx);
                    // (From its start, so a long one opens on its first lines.)
                    field.select_range(0..0, cx);
                });
                self.open_sheet(Sheet::Edit(whole.item.id), Some(self.title.focus_handle(cx)));
            }
            Act::FormCategory(category) => self.form_category = Some(category),
            Act::NewCategory => {
                self.name.update(cx, |field, cx| field.set_text("", cx));
                self.open_sheet(Sheet::NewCategory, Some(self.name.focus_handle(cx)));
            }
            Act::RenameCategory(id) => {
                let Some(name) = self.category_name(&id) else { return };
                self.name.update(cx, |field, cx| {
                    let end = name.len();
                    field.set_text(name, cx);
                    field.select_range(0..end, cx);
                });
                self.open_sheet(Sheet::RenameCategory(id), Some(self.name.focus_handle(cx)));
            }
            Act::RemoveCategory(id) => {
                // (One at a time: a second press waits for the box's answer.)
                let key = format!("category:{id}");
                if self.busy.contains(&key) {
                    return;
                }
                self.busy.push(key.clone());
                self.problem = None;
                let asked = id.clone();
                self.run(
                    format!("category-{id}"),
                    move |client| client.remove_category(&asked),
                    move |this, removed, cx| {
                        this.busy.retain(|busy| busy != &key);
                        match removed {
                        Ok(categories) => {
                            this.took_categories(categories, cx);
                            // Its list is gone with it: back to the whole queue.
                            if this.shown == Shown::List(Scope::Category(id)) {
                                cx.emit(QueueEvent::Select(None));
                            }
                        }
                        // Why not (something's still in it), in the box's words.
                        Err(error) => this.problem = Some(said(&error)),
                        }
                    },
                    cx,
                );
            }
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
    /// changed with it are the box's to say, read again.
    fn took(&mut self, now: Item, cx: &mut Context<Self>) {
        let id = now.id.clone();
        if let Reading::Ready(said) = &mut self.said {
            let listed = said.items.iter().position(|item| item.id == id);
            let removed = said.removed.iter().position(|item| item.id == id);
            match (now.removed.is_some(), listed, removed) {
                (false, Some(at), _) => said.items[at] = now.clone(),
                (false, None, was) => {
                    if let Some(at) = was {
                        said.removed.remove(at);
                    }
                    said.items.push(now.clone());
                }
                (true, was, None) => {
                    if let Some(at) = was {
                        said.items.remove(at);
                    }
                    said.removed.insert(0, now.clone());
                }
                (true, _, Some(at)) => said.removed[at] = now.clone(),
            }
        }
        if let Reading::Ready(whole) = &mut self.whole
            && whole.item.id == id
        {
            whole.item = now;
        }
        cx.emit(QueueEvent::Changed);
        // The item's page: what it says now, whole.
        if self.shown == Shown::Item(id) {
            self.read(cx);
        }
    }

    fn took_categories(&mut self, categories: Vec<Category>, cx: &mut Context<Self>) {
        if let Reading::Ready(said) = &mut self.said {
            said.categories = categories;
        }
        cx.emit(QueueEvent::Changed);
    }

    fn open_sheet(&mut self, sheet: Sheet, focus: Option<FocusHandle>) {
        self.sheet_opened += 1;
        self.sheet = Some(sheet);
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
        self.sheet_error = None;
        self.wants_focus = Some(self.focus.clone());
        cx.notify();
    }

    /// Return or Escape in one of a sheet's one-line fields.
    fn sheet_key(&mut self, event: &TextFieldEvent, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Submit(_) => self.save_sheet(cx),
            TextFieldEvent::Cancel => self.close_sheet(cx),
            TextFieldEvent::Changed => {}
        }
    }

    /// The sheet's Save: sent to the box as written, less the space around it (what an item or a
    /// category may be called is the box's to say), the sheet staying until it answers.
    pub fn save_sheet(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(sheet) = self.sheet.clone() else { return };
        self.saving = true;
        self.sheet_error = None;
        let (title, body, name) = (self.title.read(cx).text().trim().to_owned(), self.body.read(cx).text().trim().to_owned(), self.name.read(cx).text().trim().to_owned());
        let category = self.form_category.clone();
        let (sent, opened) = (sheet.clone(), self.sheet_opened);
        enum Answer {
            Item(Box<kuvo_box::Result<Item>>),
            Categories(kuvo_box::Result<Vec<Category>>),
        }
        // (Under its own name: a later sheet's save never drops this one's answer.)
        self.run(
            format!("sheet-{opened}"),
            move |client| match &sheet {
                Sheet::New => Answer::Item(Box::new(client.add_work_item(&NewItem { title, body, category }))),
                Sheet::Edit(item) => Answer::Item(Box::new(client.edit_work_item(item, &title, &body))),
                Sheet::NewCategory => Answer::Categories(client.add_category(&name)),
                Sheet::RenameCategory(id) => Answer::Categories(client.rename_category(id, &name)),
            },
            move |this, answer, cx| {
                // The sheet it was sent from, still open where it was: it closes, or says why not.
                let its_sheet = this.sheet.is_some() && this.sheet_opened == opened;
                if its_sheet {
                    this.saving = false;
                }
                let failed = match answer {
                    Answer::Item(item) => match *item {
                        Ok(item) => {
                            this.took(item, cx);
                            None
                        }
                        Err(error) => Some(said(&error)),
                    },
                    Answer::Categories(Ok(categories)) => {
                        this.took_categories(categories, cx);
                        None
                    }
                    Answer::Categories(Err(error)) => Some(said(&error)),
                };
                match failed {
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
                            Sheet::NewCategory | Sheet::RenameCategory(_) => this.name.focus_handle(cx),
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
