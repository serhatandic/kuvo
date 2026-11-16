//! Settings, in two parts with a sidebar each (the user's pick, 2026-11-15): **your settings**,
//! this app's own on this device, and **a computer's settings**, kept on that computer by its box.
//! A row at the sidebar's foot goes from one to the other; a computer's sidebar is titled with its
//! picker (your computers, as the window's box lists them), which changes only Settings, never the
//! window. A search finds on the side it's on, and lists what it finds on the other.
//!
//! A computer's sections are its box's (`kuvo/settings`): each page is drawn from the box's
//! description alone (`described.rs`), every change goes to the box from the row's revision, and
//! what the box answers is what's drawn after. The app decides nothing there but what it draws.
//! Three sections its box doesn't describe yet are still drawn from their own routes (`rows.rs`).
//!
//! A page is drawn as Zed's settings are: its rows worked out once each time what they're drawn from
//! changes (the box's facts, the search, a change on its way), then a virtual list that lays out
//! only the rows on screen. Each group of rows is one card.

mod described;
pub mod form;
pub(crate) mod page;
mod rows;
pub mod shortcuts;
pub mod sign_in;

use std::collections::HashMap;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, SharedString, Subscription, Task, Window, px};
use kuvo_box::BoxClient;
use kuvo_box::accounts::{Accounts, Side as AccountSide, said};
use kuvo_box::areas::{Account, Computer};
use kuvo_box::follow::{Follow, Followed, Timing};
use kuvo_box::models::Offered;
use kuvo_box::settings::{Config, UsageSettings};
use kuvo_box::settings_pages::{Changed, Page, Section};
use kuvo_ui::{ActiveTheme as _, IconName, TextField, TextFieldEvent};
use serde_json::Value;

use crate::areas::Reading;
use crate::updates::Updates;

pub use rows::{Act, At, Block, Control, Leading, Meter, Notice, Push, Row, Tone, UpdateAct};
use form::{FormEvent, FormSheet};

/// What's said when you didn't confirm it was you: nothing was sent.
pub(crate) fn not_confirmed() -> kuvo_box::Error {
    kuvo_box::Error::Refused(kuvo_box::Refusal { status: 403, message: "It wasn’t confirmed, so nothing was done.".into(), reason: None, older_box: false })
}
use sign_in::SignIn;

/// The widest a page's cards grow.
const PAGE_WIDTH: f32 = 720.;

/// The sections a computer's sidebar lists under headings, in this order; any other heading a
/// box names comes after, in its order.
const GROUPS: &[&str] = &["General", "Models", "Agents & Tools", "Projects", "System"];
/// The heading over what isn't in the desktop app yet.
pub(crate) const LATER: &str = "Not in the desktop app yet";

/// A place picked in Settings (`record`: by a click, so back and forward go to it), or by the
/// search finding nothing on the page open. `place`: its key ([`yours_key`], [`box_key`]).
pub struct Pick {
    pub place: String,
    pub record: bool,
}

/// Add a Machine… was picked in the computer picker: the window goes to the Computers area's.
pub struct AddMachine;

/// The key of one of your own settings' sections.
pub fn yours_key(id: &str) -> String {
    format!("yours/{id}")
}

/// The key of a section of a computer's settings (`computer`: its key in `kuvo/computers`; empty
/// for the one the window shows).
pub fn box_key(computer: &str, id: &str) -> String {
    format!("box/{computer}/{id}")
}

/// Whose settings a sidebar lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// This app's own, on this device.
    Yours,
    /// A computer's, kept by its box.
    Computer,
}

/// How a section is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawn {
    /// One of your own: this app's.
    Yours,
    /// From its box's description.
    Described,
    /// A computer's, by this app still, from routes it knows.
    Own,
    /// A page saying it isn't here yet.
    Later,
}

/// A section as a sidebar lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub id: String,
    pub title: String,
    pub icon: IconName,
    pub group: String,
    /// What search finds it by: its line and keywords, lowercase.
    words: String,
    pub drawn: Drawn,
}

impl Entry {
    fn of(listed: &rows::Listed, drawn: Drawn) -> Entry {
        Entry { id: listed.id.into(), title: listed.title.into(), icon: listed.icon, group: if drawn == Drawn::Later { LATER.into() } else { listed.group.into() }, words: listed.keywords.into(), drawn }
    }

    /// Whether `query` (lowercase) is in its name, line or keywords.
    fn named(&self, query: &str) -> bool {
        self.title.to_lowercase().contains(query) || self.words.contains(query)
    }
}

/// Something the search found on the other side: where it is, and how it's listed there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub place: String,
    pub title: String,
    pub icon: IconName,
}

/// How the computer whose settings are shown stands with this app.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Standing {
    Reached,
    /// It can't be reached now, or from this app yet: a title and a line saying so.
    Not { title: String, words: String },
}

/// What a confirm asks before it does it.
#[derive(Clone, Debug, PartialEq)]
struct Confirm {
    title: String,
    body: String,
    button: String,
    act: Act,
}

/// The Settings area: its sidebars and its pages.
pub struct SettingsView {
    /// The window's box: your computers are its list, and other boxes are reached from it.
    home: BoxClient,
    live: bool,
    side: Side,
    /// The section open on each side.
    yours: String,
    section: String,
    /// The computer whose settings are shown, by its key; none: the window's.
    computer: Option<String>,
    computers: Reading<Vec<Computer>>,
    /// Its box, where this app reaches it.
    client: Option<BoxClient>,
    standing: Standing,
    /// The box's own sections, and each page read so far.
    sections: Reading<Vec<Section>>,
    /// They're being asked for now.
    reading_sections: bool,
    pages: HashMap<String, Reading<Page>>,
    /// What makes sure it's you (Touch ID, or the computer's password) before what lets something
    /// use what's yours from now on (`verified`); none where this app can't ask.
    confirm_you: Option<crate::confirm_you::ConfirmYou>,
    /// Drawn in dark: the page is built again when the app's light or dark changes (System follows the Mac).
    dark: bool,
    /// The page open, followed as it changes (where the app runs against a box that streams).
    follow: Option<(String, Follow<Followed<Page>>)>,
    search: Entity<TextField>,
    query: String,
    /// The sections that aren't in the desktop app yet are listed (their one row is open).
    later_open: bool,
    /// The computer picker's menu, opened from its title.
    picker: kuvo_ui::menu::PopoverMenuHandle,
    accounts: Reading<Accounts>,
    limits: Reading<Vec<Account>>,
    config: Reading<Config>,
    offered: Reading<Offered>,
    usage: Reading<UsageSettings>,
    /// Rows with a change on its way, and what they say meanwhile.
    busy: Vec<(String, String)>,
    /// Secrets whose field is shown to replace the one set.
    replacing: Vec<String>,
    /// The fields of rows that take typing, by row key, made once and kept; and the revision
    /// each last showed its row's value at.
    fields: HashMap<String, (Entity<TextField>, Option<String>, Subscription)>,
    /// Hearing each of those fields left (it's saved then), by row key.
    blurs: HashMap<String, Subscription>,
    /// The rows whose breakdown is open, by key.
    expanded: Vec<String>,
    /// What the box last refused, shown over the page until it's closed or the page changes.
    problem: Option<String>,
    confirm: Option<Confirm>,
    folder_picker: Option<FolderPicker>,
    sign_in: Option<Entity<SignIn>>,
    added_sign_in: Option<String>,
    sign_in_closed: Option<Subscription>,
    /// A form the box asked to be filled in, as a sheet, and what it says as it ends.
    form: Option<Entity<FormSheet>>,
    form_events: Option<Subscription>,
    /// The fields an address is pasted back in under a flow, by row key.
    codes: HashMap<String, (Entity<TextField>, Subscription)>,
    /// The confirm asks for the keyboard as it opens (Escape cancels it).
    confirm_focus: FocusHandle,
    focus_confirm: bool,
    /// The app's updater, and how it stood when last drawn (Updates).
    updates: Option<(Entity<Updates>, crate::updates::Facts)>,
    /// The editors on this Mac General offers (read as Settings opens).
    editors: Vec<crate::yours::editors::Editor>,
    shortcuts: Vec<shortcuts::Shortcut>,
    /// Where System Settings shows this app's notifications.
    notification_settings: String,
    /// Your settings as last kept, and why the last change couldn't be, where it couldn't.
    yours_now: (crate::yours::Yours, Option<String>),
    /// The window's computer's name.
    this_box: String,
    now: Option<f64>,
    /// The page as drawn: its blocks, flattened into the list's items.
    items: Vec<Item>,
    list: ListState,
    focus: FocusHandle,
    tasks: HashMap<String, Task<()>>,
    list_filters: HashMap<String, String>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct FolderPicker {
    at: At,
    /// A form's folder field it chooses for, rather than the row's value.
    for_form: Option<String>,
    words: kuvo_box::settings_pages::Folder,
    path: String,
    back: Option<String>,
    folder: Option<kuvo_box::folders::Folder>,
    problem: Option<String>,
}

/// One of the page list's items.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Title(String),
    /// Whose these settings are, said once under the title.
    Whose(String),
    Group { label: String, words: String },
    Row { row: Box<Row>, first: bool, last: bool },
    Note { key: String, words: String, busy: bool, warning: bool },
    /// A section that isn't here yet, or a computer that isn't reached: its name, and why.
    Aside { key: &'static str, icon: IconName, title: String, words: String },
    Nothing(String),
    End,
}

impl EventEmitter<Pick> for SettingsView {}
impl EventEmitter<AddMachine> for SettingsView {}
impl EventEmitter<Opened> for SettingsView {}

/// An address a row opened (System Settings, a web page), opened already.
pub struct Opened {
    pub address: String,
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

const YOURS_WORDS: &str = "The desktop app’s own, on this device. Your other devices keep theirs.";
const LATER_WORDS: &str = "Coming to the desktop app.";

impl SettingsView {
    pub fn new(client: BoxClient, live: bool, cx: &mut Context<Self>) -> SettingsView {
        let search = cx.new(|cx| TextField::new(cx).bare().placeholder("Search your settings…"));
        let subscription = cx.subscribe(&search, |this: &mut SettingsView, field, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Changed => {
                let query = field.read(cx).text().trim().to_lowercase();
                this.searched(query, cx);
            }
            TextFieldEvent::Cancel => field.update(cx, |field, cx| field.set_text("", cx)),
            TextFieldEvent::Submit(_) => {}
        });
        let mut view = SettingsView {
            client: Some(client.clone()),
            home: client,
            live,
            side: Side::Yours,
            yours: "general".into(),
            section: "clock".into(),
            computer: None,
            computers: Reading::NotYet,
            standing: Standing::Reached,
            sections: Reading::NotYet,
            reading_sections: false,
            pages: HashMap::new(),
            confirm_you: crate::confirm_you::ConfirmYou::of(cx),
            dark: cx.theme().appearance.is_dark(),
            follow: None,
            search,
            query: String::new(),
            picker: Default::default(),
            later_open: false,
            accounts: Reading::NotYet,
            limits: Reading::NotYet,
            config: Reading::NotYet,
            offered: Reading::NotYet,
            usage: Reading::NotYet,
            busy: Vec::new(),
            replacing: Vec::new(),
            fields: HashMap::new(),
            blurs: HashMap::new(),
            expanded: Vec::new(),
            problem: None,
            confirm: None,
            folder_picker: None,
            sign_in: None,
            added_sign_in: None,
            sign_in_closed: None,
            form: None,
            form_events: None,
            codes: HashMap::new(),
            confirm_focus: cx.focus_handle(),
            focus_confirm: false,
            updates: None,
            shortcuts: shortcuts::read(cx),
            editors: if live { crate::yours::editors::installed() } else { Vec::new() },
            notification_settings: crate::yours::desk::notification_settings(),
            yours_now: Default::default(),
            this_box: "This Mac".into(),
            now: None,
            items: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            focus: cx.focus_handle(),
            tasks: HashMap::new(),
            list_filters: HashMap::new(),
            // (Your settings changed, here or by a key: their pages show it.)
            _subscriptions: vec![subscription, cx.observe_global::<crate::yours::Store>(|this: &mut SettingsView, cx| this.rebuild(cx))],
        };
        // The app's updates, followed as they change (a check starting or ending, a choice made
        // in Sparkle's own window).
        if let Some(updates) = Updates::get(cx) {
            view._subscriptions.push(cx.observe(&updates, |this: &mut SettingsView, updates, cx| {
                let facts = updates.read(cx).facts().clone();
                if this.updates.as_ref().is_some_and(|(_, was)| *was != facts) {
                    this.updates = Some((updates, facts));
                    this.rebuild(cx);
                }
            }));
            view.updates = Some((updates.clone(), updates.read(cx).facts().clone()));
        }
        view.rebuild(cx);
        let sized = kuvo_ui::theme::remeasure_on_text_size(&view.list, cx);
        view._subscriptions.push(sized);
        view
    }

    // MARK: Where it is

    /// The key of the place shown: the side, the computer and the section.
    pub fn place(&self) -> String {
        match self.side {
            Side::Yours => yours_key(&self.yours),
            Side::Computer => box_key(self.computer.as_deref().unwrap_or_default(), &self.section),
        }
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// The id of the section open on the side shown.
    pub fn section(&self) -> &str {
        match self.side {
            Side::Yours => &self.yours,
            Side::Computer => &self.section,
        }
    }

    /// The open page's name, for the window's title.
    pub fn title(&self) -> SharedString {
        if self.side == Side::Computer
            && let Standing::Not { .. } = self.standing
        {
            return self.computer_name().into();
        }
        let id = self.section();
        self.entries(self.side).into_iter().find(|entry| entry.id == id).map_or_else(|| "Settings".into(), |entry| entry.title.into())
    }

    pub fn search_field(&self) -> &Entity<TextField> {
        &self.search
    }

    /// The sign-in under way, if its sheet is open.
    pub fn sign_in(&self) -> Option<&Entity<SignIn>> {
        self.sign_in.as_ref()
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

    /// Your computers, as the window's box lists them: the picker's, and how the one shown stands.
    pub fn set_computers(&mut self, computers: Reading<Vec<Computer>>, cx: &mut Context<Self>) {
        if self.computers == computers {
            return;
        }
        self.computers = computers;
        // One that's no longer among them (removed), or the window's own picked by its key: the
        // window's, read afresh (nothing of the one that left stays under the window's name).
        let gone = self.forget_missing_computer();
        let (client, standing) = self.reach();
        if gone || standing != self.standing || client.is_some() != self.client.is_some() {
            self.connect(cx);
            if self.side == Side::Computer {
                self.read_open(cx);
            }
        }
        self.rebuild(cx);
    }

    /// The computer shown is no longer among yours: the window's is, and it says whether it was.
    fn forget_missing_computer(&mut self) -> bool {
        if let (Some(key), Reading::Ready(computers)) = (&self.computer, &self.computers)
            && !computers.iter().any(|computer| computer.key == *key && !computer.here)
        {
            self.computer = None;
            return true;
        }
        false
    }

    /// Your computers the picker lists: your own (those of another setup aren't yours to set).
    pub(crate) fn listed_computers(&self) -> Vec<Computer> {
        match &self.computers {
            Reading::Ready(computers) => computers.iter().filter(|computer| !computer.apart).cloned().collect(),
            _ => Vec::new(),
        }
    }

    fn shown_computer(&self) -> Option<&Computer> {
        let Reading::Ready(computers) = &self.computers else { return None };
        match &self.computer {
            Some(key) => computers.iter().find(|computer| computer.key == *key),
            None => computers.iter().find(|computer| computer.here),
        }
    }

    /// The key of the computer shown, as the picker checks it (none: the window's).
    pub(crate) fn computer_key(&self) -> Option<&str> {
        self.computer.as_deref()
    }

    /// The name of the computer whose settings are shown.
    pub fn computer_name(&self) -> String {
        // (The window's own is named as the title bar names it, "This Mac": one name for it.)
        self.shown_computer().filter(|computer| !computer.here).map_or_else(|| self.this_box.clone(), |computer| computer.name.clone())
    }

    /// The window's computer's name, as the title bar says it.
    pub(crate) fn this_box(&self) -> &str {
        &self.this_box
    }

    /// What the computer shown runs, in a word, beside its name.
    pub(crate) fn computer_kind(&self) -> &'static str {
        match self.shown_computer() {
            Some(computer) if computer.is_mac() => "macOS",
            Some(computer) if computer.kind == "machine" => "Linux",
            _ => "",
        }
    }

    /// The box of the computer shown, where this app reaches it, and how it stands otherwise.
    fn reach(&self) -> (Option<BoxClient>, Standing) {
        if self.computer.is_none() {
            return (Some(self.home.clone()), Standing::Reached);
        }
        let Some(computer) = self.shown_computer() else { return (None, Standing::Not { title: "Reading your computers…".into(), words: String::new() }) };
        let name = &computer.name;
        let not = |title: String, words: String| (None, Standing::Not { title, words });
        if computer.is_older() {
            let (title, words) = crate::workspace::held_words(Some(name), false);
            return not(title, words);
        }
        match computer.reach.as_str() {
            // The box's words about its settings; an older box's notice for Computers otherwise.
            "away" | "connecting" | "unknown" => match computer.settings_notice.as_ref().or(computer.notice.as_ref()) {
                Some(notice) => not(notice.title.clone(), notice.detail.clone()),
                None => not(computer.title.clone(), computer.detail.clone().unwrap_or_default()),
            },
            _ => match self.home.computer(computer) {
                Some(client) => (Some(client), Standing::Reached),
                // The desktop app reaches the window's box alone for now (as the title bar's
                // switcher says): said plainly, never read through this Mac instead.
                None => not(format!("{name}’s settings aren’t in the desktop app yet"), format!("The desktop app shows only {}’s settings for now.", self.this_box)),
            },
        }
    }

    /// Turns to the box of the computer shown: everything read from another is let go.
    fn connect(&mut self, _cx: &mut Context<Self>) {
        let (client, standing) = self.reach();
        self.client = client;
        self.standing = standing;
        self.sections = Reading::NotYet;
        self.reading_sections = false;
        self.pages.clear();
        self.accounts = Reading::NotYet;
        self.limits = Reading::NotYet;
        self.config = Reading::NotYet;
        self.offered = Reading::NotYet;
        self.usage = Reading::NotYet;
        self.busy.clear();
        self.replacing.clear();
        self.fields.clear();
        self.tasks.clear();
        self.follow = None;
        self.problem = None;
        self.confirm = None;
        self.folder_picker = None;
    }

    /// Shows the place `key` names: one of your own sections, or a computer's. A section's name
    /// alone is the computer shown's.
    pub fn set_place(&mut self, key: &str, cx: &mut Context<Self>) {
        let before = self.place();
        if let Some(id) = key.strip_prefix("yours/") {
            self.side = Side::Yours;
            self.yours = id.to_owned();
        } else {
            let (computer, id) = match key.strip_prefix("box/").and_then(|rest| rest.rsplit_once('/')) {
                Some((computer, id)) => (Some(computer.to_owned()).filter(|key| !key.is_empty()), id.to_owned()),
                None => (self.computer.clone(), key.to_lowercase().replace(' ', "-")),
            };
            // (The window's own, named by its key, is the window's.)
            let computer = computer.filter(|key| !matches!(&self.computers, Reading::Ready(computers) if computers.iter().any(|computer| computer.here && computer.key == *key)));
            self.side = Side::Computer;
            self.section = id;
            if computer != self.computer {
                self.computer = computer;
                // (A place kept from before its computer was removed is the window's now.)
                self.forget_missing_computer();
                self.connect(cx);
            }
        }
        if self.place() != before {
            self.problem = None;
            self.confirm = None;
            // (A form open for the page left goes with it, and what was typed in it.)
            self.form = None;
            self.form_events = None;
            self.folder_picker = None;
            self.replacing.clear();
            self.list_filters.clear();
            self.list.scroll_to(gpui::ListOffset::default());
            if self.follow.as_ref().is_some_and(|(of, _)| self.side != Side::Computer || *of != self.section) {
                self.follow = None;
                self.tasks.remove("follow");
            }
        }
        self.read_open(cx);
        self.rebuild(cx);
    }

    /// The area came on screen: the page open is read again, and the editors on this Mac.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        self.shortcuts = shortcuts::read(cx);
        if self.live {
            self.editors = crate::yours::editors::installed();
            self.rebuild(cx);
        }
        self.read_open(cx);
    }

    /// The editors General offers, as a test or a shot says they're on this Mac.
    pub fn set_editors(&mut self, editors: Vec<crate::yours::editors::Editor>, cx: &mut Context<Self>) {
        self.editors = editors;
        self.rebuild(cx);
    }

    /// The foot row: to the other side, at the section last open there.
    pub(crate) fn switch_side(&mut self, cx: &mut Context<Self>) {
        self.search.update(cx, |field, cx| field.set_text("", cx));
        let place = match self.side {
            Side::Yours => box_key(self.computer.as_deref().unwrap_or_default(), &self.section),
            Side::Computer => yours_key(&self.yours),
        };
        cx.emit(Pick { place, record: true });
    }

    /// The picker: another computer's settings, at the section open (its Clock where it has
    /// no such section, once its box says what it has). Only Settings changes, never the window.
    pub(crate) fn pick_computer(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        cx.emit(Pick { place: box_key(key.as_deref().unwrap_or_default(), &self.section), record: true });
    }

    // MARK: What it lists

    /// A side's sections, in its sidebar's order.
    pub(crate) fn entries(&self, side: Side) -> Vec<Entry> {
        if side == Side::Yours {
            return rows::YOURS.iter().map(|listed| Entry::of(listed, Drawn::Yours)).chain(rows::YOURS_LATER.iter().map(|listed| Entry::of(listed, Drawn::Later))).collect();
        }
        if self.standing != Standing::Reached {
            return Vec::new();
        }
        let mut entries: Vec<Entry> = Vec::new();
        if let Reading::Ready(sections) = &self.sections {
            // (One this app still draws itself stays its own until its page moves to the box.)
            entries.extend(sections.iter().filter(|section| !rows::OWN.iter().any(|own| own.id == section.id)).map(|section| Entry {
                id: section.id.clone(),
                title: section.title.clone(),
                icon: rows::icon_named(&section.icon),
                group: section.group.clone(),
                words: format!("{} {}", section.line, section.keywords.join(" ")).to_lowercase(),
                drawn: Drawn::Described,
            }));
        }
        entries.extend(rows::OWN.iter().map(|listed| Entry::of(listed, Drawn::Own)));
        let later: Vec<Entry> = rows::LATER.iter().filter(|listed| !entries.iter().any(|entry| entry.id == listed.id || entry.title.eq_ignore_ascii_case(listed.title))).map(|listed| Entry::of(listed, Drawn::Later)).collect();
        entries.extend(later);
        let order = |entry: &Entry| if entry.drawn == Drawn::Later { usize::MAX } else { GROUPS.iter().position(|group| *group == entry.group).unwrap_or(GROUPS.len()) };
        entries.sort_by_key(order);
        entries
    }

    fn facts(&self) -> rows::Facts<'_> {
        rows::Facts {
            accounts: &self.accounts,
            limits: &self.limits,
            config: &self.config,
            offered: &self.offered,
            usage: &self.usage,
            busy: &self.busy,
            this_box: self.shown_computer().filter(|computer| !computer.here).map_or(self.this_box.as_str(), |computer| computer.name.as_str()),
            here: self.client.as_ref().is_some_and(BoxClient::here),
        }
    }

    /// A section's page, as blocks.
    fn blocks(&self, entry: &Entry) -> Vec<Block> {
        match entry.drawn {
            Drawn::Yours => {
                let (yours, problem) = &self.yours_now;
                // (Kuvo on this computer, where the window's box is this computer's.)
                let here = self.home.here().then_some(self.this_box.as_str());
                rows::yours(&entry.id, &rows::YoursFacts { yours, editors: &self.editors, shortcuts: &self.shortcuts, shortcut_filter: self.list_filters.get("row-shortcut-search").map(String::as_str).unwrap_or(""), problem: problem.as_deref(), notification_settings: &self.notification_settings, updates: self.updates.as_ref().map(|(_, facts)| facts), here, now: self.now.unwrap_or_else(crate::time::now_ms) })
            }
            Drawn::Own => rows::page(&entry.id, &self.facts()),
            Drawn::Described => match self.pages.get(&entry.id) {
                Some(Reading::Ready(page)) => described::blocks(page, &described::Drawing { section: &entry.id, busy: &self.busy, replacing: &self.replacing, here: self.client.as_ref().is_some_and(BoxClient::here), filters: &self.list_filters, expanded: &self.expanded, confirms: self.confirm_you.is_some() }),
                Some(Reading::Failed(why)) => vec![Block::Note { key: "note-page".into(), words: format!("{} couldn’t be read: {why}", entry.title), busy: false, warning: true }],
                _ => vec![rows::note("note-page", format!("Reading {}…", entry.title), true)],
            },
            Drawn::Later => Vec::new(),
        }
    }

    /// The sections of `side` the search finds something on (all of them without one).
    pub(crate) fn sections_found(&self, side: Side) -> Vec<Entry> {
        self.entries(side).into_iter().filter(|entry| self.query.is_empty() || entry.named(&self.query) || !rows::found(false, self.blocks(entry), &self.query).is_empty()).collect()
    }

    /// What the search finds on the other side: each section it names, else the rows it finds there.
    pub(crate) fn hits(&self) -> Vec<Hit> {
        if self.query.is_empty() {
            return Vec::new();
        }
        let other = if self.side == Side::Yours { Side::Computer } else { Side::Yours };
        let mut hits = Vec::new();
        for entry in self.entries(other) {
            let place = if other == Side::Yours { yours_key(&entry.id) } else { box_key(self.computer.as_deref().unwrap_or_default(), &entry.id) };
            if entry.named(&self.query) {
                hits.push(Hit { place, title: entry.title.clone(), icon: entry.icon });
                continue;
            }
            let rows = rows::found(false, self.blocks(&entry), &self.query).into_iter().flat_map(|block| match block {
                Block::Group(_, _, rows) => rows,
                Block::Note { .. } => Vec::new(),
            });
            hits.extend(rows.filter(|row| !row.title.is_empty()).take(2).map(|row| Hit { place: place.clone(), title: format!("{} › {}", entry.title, row.title), icon: entry.icon }));
        }
        hits.truncate(8);
        hits
    }

    pub(crate) fn later_open(&self) -> bool {
        self.later_open
    }

    /// The one row over what isn't in the desktop app yet: opens its short list, or folds it.
    pub(crate) fn toggle_later(&mut self, cx: &mut Context<Self>) {
        self.later_open = !self.later_open;
        cx.notify();
    }

    pub(crate) fn is_searching(&self) -> bool {
        !self.query.is_empty()
    }

    /// Why the computer's own sections aren't listed, where its box couldn't say them.
    pub(crate) fn sections_problem(&self) -> Option<&str> {
        match &self.sections {
            Reading::Failed(why) if self.standing == Standing::Reached => Some(why),
            _ => None,
        }
    }

    fn searched(&mut self, query: String, cx: &mut Context<Self>) {
        if query == self.query {
            return;
        }
        // The first search reads every page of the computer shown, so what it finds on any can
        // be listed, from either side.
        if self.query.is_empty() && !query.is_empty() {
            self.query = query;
            self.read_all(cx);
        } else {
            self.query = query;
        }
        self.rebuild(cx);
    }

    /// While searching, the page open has nothing found: the first section of its side that has
    /// (again as each page's reading comes in, so a search typed before they came finds them).
    fn follow_search(&mut self, cx: &mut Context<Self>) {
        if self.query.is_empty() {
            return;
        }
        let found = self.sections_found(self.side);
        let open = self.section().to_owned();
        if !found.iter().any(|entry| entry.id == open)
            && let Some(first) = found.first()
        {
            let place = if self.side == Side::Yours { yours_key(&first.id) } else { box_key(self.computer.as_deref().unwrap_or_default(), &first.id) };
            cx.emit(Pick { place, record: false });
        }
    }

    /// The computer's box said its sections: one open that it hasn't goes to its Clock (or its
    /// first), as after switching from a computer that had it.
    fn settle(&mut self, cx: &mut Context<Self>) {
        if self.side != Side::Computer || !matches!(self.sections, Reading::Ready(_)) {
            return;
        }
        let entries = self.entries(Side::Computer);
        if entries.iter().any(|entry| entry.id == self.section) {
            return;
        }
        if let Some(to) = entries.iter().find(|entry| entry.id == "clock").or(entries.first()) {
            cx.emit(Pick { place: box_key(self.computer.as_deref().unwrap_or_default(), &to.id), record: false });
        }
    }

    /// The line under a row on the page open, as drawn.
    pub fn row_detail(&self, key: &str) -> Option<String> {
        self.items.iter().find_map(|item| match item {
            Item::Row { row, .. } if row.key == key => Some(row.detail.clone()),
            _ => None,
        })
    }

    /// What a menu row on the page open shows as picked.
    pub fn row_shown(&self, key: &str) -> Option<String> {
        self.items.iter().find_map(|item| match item {
            Item::Row { row, .. } if row.key == key => match &row.control {
                rows::Control::Menu { shown, .. } => shown.clone(),
                _ => None,
            },
            _ => None,
        })
    }

    /// The rows on the page open, by key, in order (a list's items among them).
    pub fn page_rows(&self) -> Vec<String> {
        self.items.iter().filter_map(|item| if let Item::Row { row, .. } = item { Some(row.key.clone()) } else { None }).collect()
    }

    /// Every word on the page open: its rows' titles, lines and whys, and its notes.
    pub fn page_words(&self) -> String {
        let mut words = String::new();
        for item in &self.items {
            match item {
                Item::Row { row, .. } => words.extend([row.title.as_str(), "\n", row.detail.as_str(), "\n", row.why.as_deref().unwrap_or_default(), "\n"]),
                Item::Note { words: note, .. } => words.extend([note.as_str(), "\n"]),
                _ => {}
            }
        }
        words
    }

    /// Why a row on the page open can't be changed from here, as drawn under it.
    pub fn row_why(&self, key: &str) -> Option<String> {
        self.items.iter().find_map(|item| match item {
            Item::Row { row, .. } if row.key == key => row.why.clone(),
            _ => None,
        })
    }

    /// What shows in place of a computer's settings while it isn't reached: its title and words.
    pub fn away_words(&self) -> Option<(String, String)> {
        self.items.iter().find_map(|item| match item {
            Item::Aside { key, title, words, .. } if *key == "settings-away" => Some((title.clone(), words.clone())),
            _ => None,
        })
    }

    /// What the box last refused, as shown over the page.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// What the box's action `id` on `account` does here, as its row's control asks it: none
    /// where the box doesn't offer it now (for the shots, which press no button).
    pub fn account_act(&self, account: &str, id: &str) -> Option<Act> {
        let Reading::Ready(accounts) = &self.accounts else { return None };
        let entry = accounts.agents.iter().chain(&accounts.engine).find(|entry| entry.integration.id == account)?;
        rows::act_of(entry, entry.actions.iter().find(|action| action.id == id)?)
    }

    /// The confirm open, as drawn: its question, what it comes to, and its button.
    pub fn confirm_words(&self) -> Option<(String, String, String)> {
        self.confirm.as_ref().map(|confirm| (confirm.title.clone(), confirm.body.clone(), confirm.button.clone()))
    }

    /// Works the page's items out again, from what it's drawn from now.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.dark = cx.theme().appearance.is_dark();
        self.yours_now = cx.try_global::<crate::yours::Store>().map(|store| (store.now().clone(), store.problem().map(str::to_owned))).unwrap_or_default();
        let name = self.computer_name();
        let placeholder = if self.side == Side::Yours { "Search your settings…".to_owned() } else { format!("Search {name}’s settings…") };
        self.search.update(cx, |field, _| field.set_placeholder(placeholder));
        let mut items = Vec::new();
        let open = self.section().to_owned();
        let entry = self.entries(self.side).into_iter().find(|entry| entry.id == open);
        match (&self.standing, entry) {
            (Standing::Not { title, words }, _) if self.side == Side::Computer => items.push(Item::Aside { key: "settings-away", icon: IconName::Computer, title: title.clone(), words: words.clone() }),
            (_, Some(entry)) if entry.drawn == Drawn::Later => items.push(Item::Aside { key: "placeholder", icon: entry.icon, title: entry.title.clone(), words: LATER_WORDS.into() }),
            (_, Some(entry)) => {
                items.push(Item::Title(entry.title.clone()));
                items.push(Item::Whose(if self.side == Side::Yours { YOURS_WORDS.to_owned() } else { format!("Kept on {name}, the same in every app that connects to it.") }));
                if let Some(Reading::Ready(page)) = self.pages.get(&entry.id).filter(|_| entry.drawn == Drawn::Described)
                    && let Some(line) = page.line.clone().filter(|line| !line.is_empty())
                {
                    items.push(Item::Note { key: "page-line".into(), words: line, busy: false, warning: false });
                }
                // The rows found; the whole page where its name was what's found, or its words
                // (the box's keywords) with no row to show for them.
                let titled = entry.title.to_lowercase().contains(&self.query);
                let mut blocks = rows::found(titled, self.blocks(&entry), &self.query);
                if blocks.is_empty() && entry.named(&self.query) {
                    blocks = self.blocks(&entry);
                }
                if blocks.is_empty() && !self.query.is_empty() {
                    items.push(Item::Nothing(self.query.clone()));
                }
                for block in blocks {
                    match block {
                        Block::Group(label, words, rows) => {
                            items.push(Item::Group { label, words });
                            let count = rows.len();
                            items.extend(rows.into_iter().enumerate().map(|(ix, row)| Item::Row { row: Box::new(row), first: ix == 0, last: ix + 1 == count }));
                        }
                        Block::Note { key, words, busy, warning } => items.push(Item::Note { key, words, busy, warning }),
                    }
                }
            }
            // A computer's section before its box has said what it has.
            (_, None) => match &self.sections {
                Reading::Failed(why) => items.push(Item::Note { key: "note-sections".into(), words: format!("{name}’s settings couldn’t be read: {why}"), busy: false, warning: true }),
                _ => items.push(Item::Note { key: "note-sections".into(), words: format!("Reading {name}’s settings…"), busy: true, warning: false }),
            },
        }
        items.push(Item::End);
        self.sync_fields(&items, cx);
        // Only the items that changed are laid out again; the rest keep their places and the
        // page keeps its scroll (a reset would take it back to the top).
        if items != self.items {
            let start = self.items.iter().zip(&items).take_while(|(old, new)| old == new).count();
            let tail = self.items[start..].iter().rev().zip(items[start..].iter().rev()).take_while(|(old, new)| old == new).count();
            self.list.splice(start..self.items.len() - tail, items.len() - tail - start);
            self.items = items;
        }
        self.follow_search(cx);
        cx.notify();
    }

    /// The fields of the rows that take typing: each made once and kept (typing never rebuilds
    /// it), showing its row's value again only when the box says another.
    fn sync_fields(&mut self, items: &[Item], cx: &mut Context<Self>) {
        let mut kept = Vec::new();
        for item in items {
            let Item::Row { row, .. } = item else { continue };
            let Control::Field { lines, masked, value, placeholder, revision, .. } = &row.control else { continue };
            kept.push(row.key.clone());
            match self.fields.get_mut(&row.key) {
                Some((_, Some(shown), _)) if shown == revision => {}
                Some((field, shown, _)) => {
                    field.update(cx, |field, cx| field.set_text(value.clone(), cx));
                    *shown = Some(revision.clone());
                }
                None => {
                    let (masked, placeholder, value, lines) = (*masked, placeholder.clone(), value.clone(), *lines);
                    let field = cx.new(|cx| {
                        let mut field = TextField::new(cx).bare().placeholder(placeholder);
                        if let Some((min, max)) = lines { field = field.multi_line(min, max).submits_on_enter(false); }
                        if masked {
                            field = field.masked();
                        }
                        field.set_text(value, cx);
                        field
                    });
                    let key = row.key.clone();
                    let subscription = cx.subscribe(&field, move |this: &mut SettingsView, field, event: &TextFieldEvent, cx| {
                        if matches!(event, TextFieldEvent::Changed) && this.items.iter().any(|item| matches!(item, Item::Row { row, .. } if row.key == key && matches!(row.control, Control::Field { save: Act::Filter, .. }))) {
                            this.list_filters.insert(key.clone(), field.read(cx).text().to_owned());
                            this.rebuild(cx);
                        }
                        match event {
                            TextFieldEvent::Submit(_) => this.save_field(&key, false, cx),
                            // Escape puts back what the box has (a search's is left to it).
                            TextFieldEvent::Cancel => {
                                if let Some(value) = this.field_row_value(&key) {
                                    field.update(cx, |field, cx| field.set_text(value, cx));
                                } else if this.is_secret(&key) {
                                    field.update(cx, |field, cx| field.set_text("", cx));
                                }
                            }
                            TextFieldEvent::Changed => {}
                        }
                    });
                    // (A field made again is heard again: the last one's listener goes with it.)
                    self.blurs.remove(&row.key);
                    self.fields.insert(row.key.clone(), (field, Some(revision.clone()), subscription));
                }
            }
        }
        self.fields.retain(|key, _| kept.contains(key));
        self.blurs.retain(|key, _| kept.contains(key));
        // The fields an address is pasted back in, under a flow that has one.
        let mut codes = Vec::new();
        for item in items {
            let Item::Row { row, .. } = item else { continue };
            let Some((placeholder, _, act)) = row.flow.as_ref().and_then(|flow| flow.code.clone()) else { continue };
            codes.push(row.key.clone());
            if !self.codes.contains_key(&row.key) {
                let field = cx.new(|cx| TextField::new(cx).bare().placeholder(placeholder));
                let subscription = cx.subscribe(&field, move |this: &mut SettingsView, _, event: &TextFieldEvent, cx| {
                    if let TextFieldEvent::Submit(_) = event {
                        this.act(act.clone(), cx);
                    }
                });
                self.codes.insert(row.key.clone(), (field, subscription));
            }
        }
        self.codes.retain(|key, _| codes.contains(key));
    }

    /// Hears each field that takes typing being left, once (it needs the window, which only
    /// drawing has): it's saved then.
    pub(crate) fn hear_blurs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unheard: Vec<(String, gpui::FocusHandle)> = self.fields.iter().filter(|(key, _)| !self.blurs.contains_key(*key)).map(|(key, (field, _, _))| (key.clone(), field.focus_handle(cx))).collect();
        for (key, handle) in unheard {
            let left = key.clone();
            let subscription = cx.on_blur(&handle, window, move |this, _, cx| this.save_field(&left, true, cx));
            self.blurs.insert(key, subscription);
        }
    }

    /// A field's row's value as it's drawn (for tests).
    pub fn row_value(&self, key: &str) -> Option<String> {
        self.field_row_value(key)
    }

    fn is_secret(&self, key: &str) -> bool {
        self.items.iter().any(|item| matches!(item, Item::Row { row, .. } if row.key == key && matches!(row.control, Control::Field { masked: true, .. })))
    }

    /// The value a field's row has now, as its field shows it (none for a search, or a secret,
    /// which shows none).
    fn field_row_value(&self, key: &str) -> Option<String> {
        self.items.iter().find_map(|item| match item {
            Item::Row { row, .. } if row.key == key => match &row.control {
                Control::Field { save: Act::Filter, .. } | Control::Field { masked: true, .. } => None,
                Control::Field { value, .. } => Some(value.clone()),
                _ => None,
            },
            _ => None,
        })
    }

    /// A field's text saved as its row's value: on Return, or as it's left (`left`), where it
    /// says something other than the row does (left empty, a secret sends nothing). The row's
    /// revision as read goes with it, so a change made elsewhere meanwhile is still refused.
    fn save_field(&mut self, key: &str, left: bool, cx: &mut Context<Self>) {
        // (Left for its own folder picker: what's picked there is what's sent.)
        if left && self.folder_picker.is_some() {
            return;
        }
        let Some((save, value, masked)) = self.items.iter().find_map(|item| match item {
            Item::Row { row, .. } if row.key == key => match &row.control {
                Control::Field { save, value, masked, .. } => Some((save.clone(), value.clone(), *masked)),
                _ => None,
            },
            _ => None,
        }) else {
            return;
        };
        let Act::Save { at, number } = &save else { return };
        let Some(text) = self.fields.get(key).map(|(field, _, _)| field.read(cx).text().to_owned()) else { return };
        // Nothing to save where it says what the row does (a secret, where nothing's typed); and a
        // change already on its way isn't sent twice.
        // A secret is sent only with Return: one left half typed, or on the way to Cancel, isn't.
        let same = if masked {
            left || text.is_empty()
        } else if *number {
            at.field_value(&text, true).is_some_and(|typed| at.original.as_ref().is_some_and(|was| was.as_f64() == typed.as_f64()))
        } else {
            at.field_value(&text, false) == Some(Value::String(if at.multiline { value.clone() } else { value.trim().to_owned() }))
        };
        if same || self.busy.iter().any(|(busy, _)| busy == key) {
            return;
        }
        self.act(save, cx);
    }

    /// The field an address is pasted back in under a row's flow.
    pub fn code_field(&self, key: &str) -> Option<&Entity<TextField>> {
        self.codes.get(key).map(|(field, _)| field)
    }

    /// The form open over the page.
    pub fn form(&self) -> Option<&Entity<FormSheet>> {
        self.form.as_ref()
    }

    /// A row's field, as its control draws it.
    pub fn field(&self, key: &str) -> Option<&Entity<TextField>> {
        self.fields.get(key).map(|(field, _, _)| field)
    }

    // MARK: Reading

    /// Runs `work` against the box of the computer shown, in the app off this thread, in tests
    /// and shots at once, and hands what it came to to `then`. A newer one under the same name
    /// replaces an older; one for a computer no longer shown is dropped unanswered.
    fn run<T: Send + 'static>(
        &mut self,
        name: impl Into<String>,
        work: impl FnOnce(&BoxClient) -> T + Send + 'static,
        then: impl FnOnce(&mut SettingsView, T, &mut Context<SettingsView>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else { return };
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

    /// Asks the computer's box what its sections are, then reads the page open (and, while
    /// searching, every page).
    fn read_sections(&mut self, cx: &mut Context<Self>) {
        // Another computer's box is asked what it is first, as the window's was when it opened:
        // one older than this app needs is held, and says so.
        let other = self.computer.is_some();
        self.reading_sections = true;
        self.run(
            "sections",
            move |client| {
                if other {
                    client.connect()?;
                }
                client.settings_sections()
            },
            move |this, read, cx| {
                this.reading_sections = false;
                this.sections = match (read, &this.sections) {
                    (Ok(sections), _) => Reading::Ready(sections),
                    (Err(_), Reading::Ready(sections)) => Reading::Ready(sections.clone()),
                    (Err(error), _) => Reading::Failed(said(&error)),
                };
                this.settle(cx);
                // (What this app draws itself is read even where the box's sections couldn't be.)
                if this.side == Side::Computer {
                    let section = this.section.clone();
                    this.read_section(&section, cx);
                }
                if !this.query.is_empty() {
                    this.read_pages(cx);
                }
            },
            cx,
        );
    }

    fn held(&self) -> bool {
        self.client.as_ref().is_none_or(|client| client.gate().is_held())
    }

    /// Reads what the page open is drawn from (your own pages read nothing). Sections that
    /// aren't read yet, or couldn't be the last time, are asked for first.
    fn read_open(&mut self, cx: &mut Context<Self>) {
        if self.side != Side::Computer || self.held() {
            self.rebuild(cx);
            return;
        }
        if !matches!(self.sections, Reading::Ready(_)) {
            if !self.reading_sections {
                self.read_sections(cx);
            }
            return;
        }
        let section = self.section.clone();
        self.read_section(&section, cx);
    }

    /// Reads every page of the computer shown (for a search).
    fn read_all(&mut self, cx: &mut Context<Self>) {
        if self.held() {
            return;
        }
        if !matches!(self.sections, Reading::Ready(_)) {
            if !self.reading_sections {
                self.read_sections(cx);
            }
            return;
        }
        self.read_pages(cx);
    }

    fn read_pages(&mut self, cx: &mut Context<Self>) {
        for entry in self.entries(Side::Computer) {
            let unread = match entry.drawn {
                Drawn::Described => !matches!(self.pages.get(&entry.id), Some(Reading::Ready(_))),
                Drawn::Own => true,
                _ => false,
            };
            if unread && !(self.side == Side::Computer && entry.id == self.section) {
                self.read_section(&entry.id, cx);
            }
        }
    }

    fn read_section(&mut self, section: &str, cx: &mut Context<Self>) {
        let drawn = self.entries(Side::Computer).into_iter().find(|entry| entry.id == section).map(|entry| entry.drawn);
        match (drawn, section) {
            (Some(Drawn::Described), _) => self.read_page(section, cx),
            (Some(Drawn::Own), "accounts") => self.run("read-accounts", |client| client.sign_in_accounts(), |this, read, _| this.accounts = reading(read, &this.accounts), cx),
            (Some(Drawn::Own), "usage") => {
                self.run("read-limits", |client| client.accounts(), |this, read, _| this.limits = reading(read, &this.limits), cx);
                self.run("read-usage", |client| client.usage_settings(), |this, read, _| this.usage = reading(read, &this.usage), cx);
            }
            (Some(Drawn::Own), "models") => {
                self.run("read-config", |client| client.config(), |this, read, _| this.config = reading(read, &this.config), cx);
                self.run("read-offered", |client| client.offered_models(None, None), |this, read, _| this.offered = reading(read, &this.offered), cx);
            }
            _ => {}
        }
    }

    /// Reads a page its box describes. The one open is followed as it changes instead (another
    /// client's change, an agent's), where the app runs against a box that streams.
    fn read_page(&mut self, section: &str, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        let id = section.to_owned();
        if self.live && client.streams() && self.side == Side::Computer && self.section == section {
            if self.follow.as_ref().is_some_and(|(of, _)| of == section) {
                return;
            }
            let follow = client.follow_settings_page(section, Timing::default());
            let updates = follow.updates().clone();
            self.follow = Some((id.clone(), follow));
            let task = cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    let read = match update {
                        Followed::Now(page) => Ok(page),
                        Followed::Failed(error) => Err(error),
                    };
                    let took = this.update(cx, |this, cx| {
                        this.take_page(&id, read);
                        this.rebuild(cx);
                    });
                    if took.is_err() {
                        break;
                    }
                }
            });
            self.tasks.insert("follow".into(), task);
            return;
        }
        self.run(format!("page-{section}"), { let id = id.clone(); move |client| client.settings_page(&id) }, move |this, read, _| this.take_page(&id, read), cx);
    }

    fn take_page(&mut self, section: &str, read: kuvo_box::Result<Page>) {
        let before = self.pages.remove(section).unwrap_or(Reading::NotYet);
        self.pages.insert(section.to_owned(), reading(read, &before));
    }

    // MARK: Changes

    fn set_busy(&mut self, key: &str, words: &str) {
        self.busy.retain(|(busy, _)| busy != key);
        self.busy.push((key.to_owned(), words.to_owned()));
    }

    fn done(&mut self, key: &str) {
        self.busy.retain(|(busy, _)| busy != key);
    }

    /// What the box made of a change to a row it describes: the page as it is now; where the row
    /// had changed since it was read, that page with the box's words over it; else its refusal.
    fn take_changed(&mut self, section: &str, row: &str, done: kuvo_box::Result<Changed>, cx: &mut Context<Self>) {
        match done {
            Ok(Changed::Done(page)) => {
                self.replacing.retain(|id| id != row);
                // (A secret's field is let go: what was typed is the box's now, never kept here.)
                let key = described::key(row);
                let secret = self.items.iter().any(|item| matches!(item, Item::Row { row, .. } if row.key == key && matches!(row.control, Control::Field { masked: true, .. })));
                if secret {
                    self.fields.remove(&key);
                } else if let Some((_, shown, _)) = self.fields.get_mut(&key) {
                    *shown = None;
                }
                self.pages.insert(section.to_owned(), Reading::Ready(page));
            }
            Ok(Changed::Stale { message, page }) => {
                self.problem = Some(message);
                self.pages.insert(section.to_owned(), Reading::Ready(page));
            }
            Err(error) => {
                self.problem = Some(said(&error));
                self.read_page(section, cx);
            }
        }
    }

    /// What a row's control asked for.
    pub fn act(&mut self, act: Act, cx: &mut Context<Self>) {
        self.problem = None;
        match act {
            Act::Change { at, value, asked } => {
                if let (false, Some(asks)) = (asked, at.asks_for(&value)) {
                    let button = if at.below(&value) { match (&at.unit, value.as_f64()) {
                        (Some(unit), Some(value)) => format!("{} {}", asks.button, unit.display(value)),
                        (None, Some(value)) if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 => format!("{} {}", asks.button, value as i64),
                        _ => format!("{} {}", asks.button, value),
                    }} else { asks.button };
                    self.ask(asks.title, asks.body, button, Act::Change { at, value, asked: true });
                } else {
                    let key = described::key(&at.row);
                    self.set_busy(&key, "Saving…");
                    let to = at.clone();
                    let confirmed = asked && at.below(&value);
                    self.run(
                        key.clone(),
                        move |client| client.change_setting_confirmed_at(&to.section, &to.row, &to.revision, value, confirmed),
                        move |this, done, cx| {
                            this.done(&key);
                            this.take_changed(&at.section, &at.row, done, cx);
                        },
                        cx,
                    );
                }
            }
            Act::Do { at, action, confirm, asked } => {
                if let (false, Some(asks)) = (asked, confirm.clone()) {
                    self.ask(asks.title, asks.body, asks.button, Act::Do { at, action, confirm, asked: true });
                } else {
                    let key = described::key(&at.row);
                    // What lets something use what's yours from now on: Touch ID, or the
                    // computer's password, first; not it, nothing's sent.
                    let verify = self.verified_action(&at.section, &at.row, &action).map(|title| (title, crate::confirm_you::ConfirmYou::of(cx)));
                    self.set_busy(&key, "Working…");
                    let to = at.clone();
                    let flow_before = self.flow_of(&at.section, &at.row);
                    self.run(
                        key.clone(),
                        move |client| {
                            if let Some((title, confirm)) = &verify
                                && !confirm.as_ref().is_some_and(|confirm| (confirm.0)(&format!("confirm {title} in Settings")))
                            {
                                return Err(not_confirmed());
                            }
                            client.setting_action_at(&to.section, &to.row, &to.revision, &action)
                        },
                        move |this, done, cx| {
                            this.done(&key);
                            // A flow this started: its page opened here, once (any other client
                            // shows its button).
                            if let Ok(Changed::Done(page)) = &done
                                && let Some(flow) = page.row(&at.row).and_then(|row| row.flow.clone())
                                && flow.state == kuvo_box::settings_pages::FlowState::Waiting
                                && flow_before.as_deref() != Some(flow.id.as_str())
                                && let Some(address) = flow.opens.clone()
                                && (address.starts_with("https://") || address.starts_with("http://"))
                            {
                                this.act(Act::Open(address), cx);
                            }
                            this.take_changed(&at.section, &at.row, done, cx);
                        },
                        cx,
                    );
                }
            }
            Act::Form { at, action, form } => self.open_form(at, action, *form, cx),
            Act::Code { at, action } => {
                let key = described::key(&at.row);
                let Some(code) = self.codes.get(&key).map(|(field, _)| field.read(cx).text().trim().to_owned()) else { return };
                if code.is_empty() {
                    return;
                }
                self.set_busy(&key, "Working…");
                let to = at.clone();
                self.run(
                    key.clone(),
                    move |client| {
                        let mut values = serde_json::Map::new();
                        values.insert("code".into(), Value::String(code));
                        client.setting_form_at(&to.section, &to.row, &to.revision, &action, &values)
                    },
                    move |this, done, cx| {
                        this.done(&key);
                        let done = match done {
                            Ok(kuvo_box::settings_pages::Sent::Changed(changed)) => Ok(changed),
                            Ok(kuvo_box::settings_pages::Sent::Refused { message, .. }) => {
                                this.problem = Some(message);
                                return;
                            }
                            Err(error) => Err(error),
                        };
                        if done.is_ok()
                            && let Some((field, _)) = this.codes.get(&key)
                        {
                            field.update(cx, |field, cx| field.set_text("", cx));
                        }
                        this.take_changed(&at.section, &at.row, done, cx);
                    },
                    cx,
                );
            }
            Act::Save { at, number } => {
                let Some(text) = self.fields.get(&described::key(&at.row)).map(|(field, _, _)| field.read(cx).text().to_owned()) else { return };
                let Some(value) = at.field_value(&text, number) else {
                    self.problem = Some(format!("{} takes a number.", at.title));
                    self.rebuild(cx);
                    return;
                };
                self.act(Act::Change { at, value, asked: false }, cx);
                return;
            }
            Act::Filter => {},
            Act::Folder { at, words } => {
                let path = at.original.as_ref().and_then(Value::as_str).filter(|path| !path.is_empty()).unwrap_or("~").to_owned();
                self.folder_picker = Some(FolderPicker { at, for_form: None, words, path: path.clone(), back: None, folder: None, problem: None });
                self.focus_confirm = true;
                self.browse_folder(path, cx);
            }
            Act::Replacing { row, on } => {
                // (What was typed to replace it goes with Cancel: it's never kept or sent.)
                if !on && let Some((field, _, _)) = self.fields.get(&described::key(&row)) {
                    field.update(cx, |field, cx| field.set_text("", cx));
                }
                self.replacing.retain(|id| *id != row);
                if on {
                    self.replacing.push(row);
                }
            }
            Act::Expand(key) => {
                if self.expanded.contains(&key) {
                    self.expanded.retain(|open| *open != key);
                } else {
                    self.expanded.push(key);
                }
            }
            Act::Section(id) => cx.emit(Pick { place: box_key(self.computer.as_deref().unwrap_or_default(), &id), record: true }),
            Act::Go(place) => {
                self.search.update(cx, |field, cx| field.set_text("", cx));
                cx.emit(Pick { place, record: true });
            }
            Act::SignIn { side, account, name, sheet_title } => self.open_sign_in_titled(side, &account, &name, sheet_title, cx),
            Act::AddAgent { account, name, sheet_title } => {
                let key = format!("account-{account}");
                self.set_busy(&key, "Adding…");
                self.run(key.clone(), move |client| client.add_agent_account(&account), move |this, result, cx| {
                    this.done(&key);
                    match result {
                        Ok(account) => {
                            this.open_sign_in_titled(AccountSide::Agents, &account, &name, sheet_title, cx);
                            this.added_sign_in = Some(account);
                        },
                        Err(error) => this.problem = Some(said(&error)),
                    }
                }, cx);
            }
            Act::Install { account, name } => {
                let key = format!("account-{account}");
                self.set_busy(&key, "Installing…");
                let installing = account.clone();
                self.run(
                    key.clone(),
                    // Installed, its accounts are read again in the same piece of work: what the
                    // box says of it after the install is what the next step goes by (before it,
                    // it offered only Install).
                    move |client| client.install_agent(&installing).map(|installed| {
                        let read = installed.installed.then(|| client.sign_in_accounts());
                        (installed, read)
                    }),
                    move |this, done, cx| {
                        this.done(&key);
                        match done {
                            Ok((installed, read)) if installed.installed => {
                                // (Installed, but its accounts couldn't be read again: said, rather
                                // than a row that still offers Install with no word why.)
                                if let Some(Err(error)) = &read {
                                    this.problem = Some(format!("{name} is installed, but its accounts couldn’t be read again. {}", said(error)));
                                }
                                if let Some(read) = read {
                                    this.accounts = reading(read, &this.accounts);
                                }
                                // Installed: on to signing in, where its box now offers it.
                                if this.offers_sign_in(AccountSide::Agents, &account) {
                                    this.open_sign_in(AccountSide::Agents, &account, &name, cx);
                                }
                            }
                            Ok((installed, _)) => this.problem = Some(format!("{name} couldn’t be installed. {}", installed.message.unwrap_or_else(|| "It isn’t installed.".into()))),
                            Err(error) => this.problem = Some(format!("{name} couldn’t be installed. {}", said(&error))),
                        }
                    },
                    cx,
                );
            }
            // Asked first where the box says to, in its words; else done at once.
            Act::Disconnect { account, confirm: Some(asks) } => self.ask(asks.title, asks.message, asks.label, Act::Disconnect { account, confirm: None }),
            Act::RemoveAgent { account, confirm: Some(asks) } => self.ask(asks.title, asks.message, asks.label, Act::RemoveAgent { account, confirm: None }),
            Act::Disconnect { account, confirm: None } => {
                let forgetting = account.clone();
                self.account_change(account, move |client| client.forget_account(&forgetting), "Disconnecting…", cx)
            }
            Act::RemoveAgent { account, confirm: None } => {
                let removing = account.clone();
                self.account_change(account, move |client| client.remove_agent_account(&removing), "Removing…", cx)
            }
            Act::SignOut { account } => self.account_change(account.clone(), move |client| client.sign_out_agent(&account), "Signing out…", cx),
            Act::SetModel { path, model, was } => {
                let key = rows_model_key(path);
                self.set_busy(&key, "Saving…");
                self.run(
                    key.clone(),
                    move |client| client.set_config(path, model.as_deref(), was.as_deref()),
                    move |this, done, cx| {
                        this.done(&key);
                        match done {
                            Ok(config) => this.config = Reading::Ready(config),
                            Err(error) => {
                                // Refused (changed meanwhile, or unreadable): what's there now.
                                this.problem = Some(said(&error));
                                this.read_section("models", cx);
                            }
                        }
                    },
                    cx,
                );
            }
            Act::Open(url) => {
                cx.open_url(&url);
                // (Said to the window, which helps a permission along where that's what it is.)
                cx.emit(Opened { address: url });
            }
            Act::SetNotice(notice, on) => {
                let Reading::Ready(usage) = &self.usage else { return };
                let mut notices = usage.notices;
                let key = match notice {
                    Notice::AlmostOut => {
                        notices.under_ten_percent = on;
                        "notice-almost-out"
                    }
                    Notice::CuttingItClose => {
                        notices.healthy_to_close = on;
                        "notice-close"
                    }
                    Notice::WillRunOut => {
                        notices.close_to_running_out = on;
                        "notice-run-out"
                    }
                };
                let based = usage.version.clone();
                // A part changed on another computer meanwhile is turned away: the box's is shown.
                self.change(key, move |client| client.set_usage_notices(notices, based.as_deref()), |this, usage| this.usage = Reading::Ready(usage), cx)
            }
            Act::Update(asked) => {
                let Some((updates, _)) = self.updates.clone() else { return };
                updates.update(cx, |updates, cx| match asked {
                    UpdateAct::Check => updates.check(cx),
                    UpdateAct::Checks(on) => updates.set_checks(on, cx),
                    UpdateAct::Downloads(on) => updates.set_downloads(on, cx),
                    UpdateAct::Betas(on) => updates.set_betas(on, cx),
                });
                // (What it says now, at once: the observer above goes after this.)
                self.updates = Some((updates.clone(), updates.read(cx).facts().clone()));
            }
            Act::Yours(change) => crate::yours::change(change, cx),
        }
        self.rebuild(cx);
    }

    /// A change to a row: busy while it goes, then the box's answer, or its refusal over the page.
    fn change<T: Send + 'static>(
        &mut self,
        key: &str,
        work: impl FnOnce(&BoxClient) -> kuvo_box::Result<T> + Send + 'static,
        take: impl FnOnce(&mut SettingsView, T) + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = key.to_owned();
        self.set_busy(&key, "Saving…");
        let done_key = key.clone();
        self.run(
            key,
            work,
            move |this, done, _| {
                this.done(&done_key);
                match done {
                    Ok(value) => take(this, value),
                    Err(error) => this.problem = Some(said(&error)),
                }
            },
            cx,
        );
    }

    /// A change to an account, then the accounts read again.
    fn account_change(&mut self, account: String, work: impl FnOnce(&BoxClient) -> kuvo_box::Result<()> + Send + 'static, words: &str, cx: &mut Context<Self>) {
        let key = format!("account-{account}");
        self.set_busy(&key, words);
        let done_key = key.clone();
        self.run(
            key,
            move |client| work(client).and_then(|_| client.sign_in_accounts()),
            move |this, done, _| {
                this.done(&done_key);
                match done {
                    Ok(accounts) => this.accounts = Reading::Ready(accounts),
                    Err(error) => this.problem = Some(said(&error)),
                }
            },
            cx,
        );
    }

    pub fn browse_folder(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(picker) = self.folder_picker.as_mut() else { return };
        if let Some(prev) = picker.folder.take() {
            picker.back = Some(prev.path);
        }
        picker.path = path.clone();
        picker.problem = None;
        cx.notify();
        let asked = path.clone();
        self.run("settings-folder", move |client| client.folders_in(&asked), move |this, read, _| {
            let Some(picker) = this.folder_picker.as_mut().filter(|picker| picker.path == path) else { return };
            match read {
                Ok(folder) => {
                    picker.back = folder.parent.clone().filter(|parent| *parent != folder.path);
                    picker.path = folder.path.clone();
                    picker.folder = Some(folder);
                }
                Err(Ok(why)) => picker.problem = Some(why.message),
                Err(Err(error)) => picker.problem = Some(said(&error)),
            }
        }, cx);
    }

    /// Opens the confirm the box asked for, with its words; `act` is what going ahead does.
    fn ask(&mut self, title: String, body: String, button: String, act: Act) {
        self.focus_confirm = true;
        self.confirm = Some(Confirm { title, body, button, act });
    }

    fn confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else { return };
        self.act(confirm.act, cx);
        self.rebuild(cx);
    }

    /// A row's flow now, by its id.
    fn flow_of(&self, section: &str, row: &str) -> Option<String> {
        match self.pages.get(section) {
            Some(Reading::Ready(page)) => page.row(row).and_then(|row| row.flow.as_ref()).map(|flow| flow.id.clone()),
            _ => None,
        }
    }

    /// The title of what `row` offers as `action` on the page `section` as read, where it makes
    /// sure it's you first (`verified`).
    fn verified_action(&self, section: &str, row: &str, action: &str) -> Option<String> {
        let Some(Reading::Ready(page)) = self.pages.get(section) else { return None };
        let row = page.row(row)?;
        row.actions.iter().chain(row.add.iter()).chain(row.flow.iter().flat_map(|flow| flow.actions.iter())).find(|each| each.id == action && each.verified).map(|each| each.title.trim_end_matches('…').to_owned())
    }

    // MARK: Forms

    /// Opens a form the box asked to be filled in: done, the page is the box's answer; a folder
    /// field's Choose… opens the page's picker for it.
    pub fn open_form(&mut self, at: At, action: String, form: kuvo_box::settings_pages::Form, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        let was = form.revision.clone().unwrap_or_else(|| at.revision.clone());
        let target = form::Target { section: at.section.clone(), row: at.row.clone(), was, action };
        let sheet = cx.new(|cx| FormSheet::new(client, self.live, target, form, cx));
        self.form_events = Some(cx.subscribe(&sheet, move |this: &mut SettingsView, _, event: &FormEvent, cx| {
            match event {
                FormEvent::Done(changed) => {
                    this.form = None;
                    this.form_events = None;
                    this.take_changed(&at.section, &at.row, Ok(changed.clone()), cx);
                }
                // (What it shows changed meanwhile: the page under it is the box's now; the sheet
                // stays with what was typed, and says so.)
                FormEvent::Stale(page) => {
                    this.pages.insert(at.section.clone(), Reading::Ready(page.clone()));
                }
                FormEvent::Cancelled => {
                    this.form = None;
                    this.form_events = None;
                }
                FormEvent::ChooseFolder { field, words, start } => {
                    let start = if start.is_empty() { "~".to_owned() } else { start.clone() };
                    this.folder_picker = Some(FolderPicker { at: at.clone(), for_form: Some(field.clone()), words: words.clone(), path: start.clone(), back: None, folder: None, problem: None });
                    this.focus_confirm = true;
                    this.browse_folder(start, cx);
                }
            }
            this.rebuild(cx);
        }));
        sheet.update(cx, |sheet, cx| sheet.start(cx));
        self.form = Some(sheet);
        cx.notify();
    }

    // MARK: Signing in

    fn offers_sign_in(&self, side: AccountSide, account: &str) -> bool {
        let (Reading::Ready(accounts), Some(client)) = (&self.accounts, &self.client) else { return false };
        let list = if side == AccountSide::Agents { &accounts.agents } else { &accounts.engine };
        // As the box says: a sign-in among its actions, one this app can start from where it runs.
        list.iter().any(|entry| entry.integration.id == account && entry.actions.iter().any(|action| action.signs_in() && action.offered(client.here())))
    }

    pub fn open_sign_in(&mut self, side: AccountSide, account: &str, name: &str, cx: &mut Context<Self>) {
        let Reading::Ready(accounts) = &self.accounts else { return };
        let list = if side == AccountSide::Agents { &accounts.agents } else { &accounts.engine };
        let Some(title) = list.iter().find(|entry| entry.integration.id == account)
            .and_then(|entry| entry.actions.iter().find(|action| action.signs_in()))
            .and_then(|action| action.sheet_title.clone()) else { return };
        self.open_sign_in_titled(side, account, name, title, cx);
    }

    fn open_sign_in_titled(&mut self, side: AccountSide, account: &str, name: &str, title: String, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        let sheet = cx.new(|cx| SignIn::new(client, self.live, side, account, name, cx).sheet_title(title));
        // Closed, however it ended (a key may have gone in as it closed): Accounts read again.
        self.sign_in_closed = Some(cx.subscribe(&sheet, |this: &mut SettingsView, _, closed: &sign_in::Closed, cx| {
            this.sign_in = None;
            this.sign_in_closed = None;
            let remove = this.added_sign_in.take().filter(|_| !closed.signed_in);
            this.run("read-accounts", move |client| {
                if let Some(account) = remove { client.remove_unfinished_agent_account(&account)?; }
                client.sign_in_accounts()
            }, |this, read, _| {
                if let Err(error) = &read { this.problem = Some(said(error)); }
                this.accounts = reading(read, &this.accounts);
            }, cx);
            cx.notify();
        }));
        sheet.update(cx, |sheet, cx| sheet.start(cx));
        self.sign_in = Some(sheet);
        cx.notify();
    }
}

/// The key of a model setting's row.
fn rows_model_key(path: &[&str]) -> String {
    let ix = ["model", "agents", "subagent_model"].iter().position(|first| path.first() == Some(first)).unwrap_or(0);
    format!("model-{ix}")
}

/// What a read makes of what's shown: what it read, or (failing) what was there, else why.
fn reading<T: Clone>(read: kuvo_box::Result<T>, before: &Reading<T>) -> Reading<T> {
    match (read, before) {
        (Ok(value), _) => Reading::Ready(value),
        (Err(_), Reading::Ready(value)) => Reading::Ready(value.clone()),
        (Err(error), _) => Reading::Failed(said(&error)),
    }
}
