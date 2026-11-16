//! The Computers area's main view: your computers side by side (how each stands, what it runs,
//! the Kuvo Code it has), a computer's page, and Add a Machine. Every fact is its box's
//! (`kuvo/computers`, PROTOCOL.md "Computers"), and so is what can be done with each (`actions`:
//! connecting again, removing, each with the box's words), done by the box, never here instead.
//! Renaming waits on its design. Adding a machine is the box's too (`kuvo/computers/add`, "Adding
//! one"): this takes the address, and draws the box's steps, its question and its words.
//!
//! Picked by the area's sidebar: nothing (all of them), `this` (the computer this window shows),
//! `computer:<key>` (another), or `add` (Add a Machine).

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Hsla, InteractiveElement, IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Task, Window, div, list, prelude::FluentBuilder as _, px,
};
use kuvo_box::BoxClient;
use kuvo_box::accounts::{Confirm, said};
use kuvo_box::areas::{AddTry, Adding, Computer};
use kuvo_box::follow::{Follow, Followed, Timing};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TypeSize,
    button::{Button, ButtonSize},
    small::spinner,
    text_field::{TextField, TextFieldEvent},
    theme::{radius, space},
};

use crate::areas::{Reading, reading};
use crate::settings::page::{ConfirmIds, confirm_card, sheet};
use crate::workspace::held_words;

/// The widest the page's cards grow (Settings' width).
const PAGE_WIDTH: f32 = 720.;

/// Why an action no box offers yet can't be done here, as the title bar's switcher says it.
const NOT_YET: &str = "Not in this app yet";

/// What the page shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// All your computers.
    All,
    /// The computer this window shows.
    This,
    /// Another, by its box's key.
    Computer(String),
    Add,
}

impl Shown {
    pub fn from_key(key: Option<&str>) -> Shown {
        match key {
            None => Shown::All,
            Some("this") => Shown::This,
            Some("add") => Shown::Add,
            Some(key) => key.strip_prefix("computer:").map(|key| Shown::Computer(key.to_owned())).unwrap_or(Shown::All),
        }
    }
}

pub enum ComputersEvent {
    /// A row picked: `this`, `computer:<key>`, `add`, or none (all of them).
    Select(Option<String>),
    /// The Sessions area, where this window's own sessions at work are.
    Sessions,
    /// The box did something to your computers: the list is read again where it isn't followed.
    ReadAgain,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tone {
    /// Waiting on something that comes by itself.
    Waiting,
    /// Something to do about it (an update).
    Attention,
    /// Just so.
    Info,
}

#[derive(Clone, Debug, PartialEq)]
enum Item {
    Title { title: String, line: String, add: bool },
    Group(String),
    Row { computer: Box<Computer>, first: bool, last: bool },
    Head(Box<Computer>),
    Notice { key: String, tone: Tone, title: String, words: String },
    Fact { key: String, title: String, value: String, first: bool, last: bool },
    /// Sessions at work there, as its box counts them; `open`: they're this window's, under Sessions.
    Working { title: String, open: bool },
    /// One of Manage's rows: what it does (none: not in this app yet), and its words while it's under way.
    Action { key: String, title: String, button: String, act: Option<Manage>, busy: Option<String>, why: Option<String>, first: bool, last: bool },
    Note { key: String, words: String, busy: bool },
    /// The address field; `can`: the box adds computers.
    AddForm { can: bool },
    /// The hosts the box suggests, those that go with what's typed.
    Hosts(Vec<String>),
    /// Cancel and Add Machine; `ready`: there's an address to add.
    AddFoot { ready: bool, busy: bool },
    /// A try at adding, setting up again or uninstalling, as its box says it.
    Try(Box<AddTry>),
    End,
}

/// One of Manage's rows as the box offers it: its key, title, button, what it does, and why it waits.
type Offered = (&'static str, String, String, Option<Manage>, Option<String>);

/// What the box is asked to do, off this thread.
type Work = Box<dyn FnOnce(&BoxClient) -> kuvo_box::Result<()> + Send>;

/// What one of Manage's buttons does, as the box offered it for a computer (by its key), asked
/// first where the box gives a confirm.
#[derive(Clone, Debug, PartialEq)]
struct Manage {
    what: What,
    key: String,
    confirm: Option<Confirm>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum What {
    Connect,
    Remove,
    AllowDevices,
    SetUp,
    Uninstall,
}

impl What {
    /// Its row on the page.
    fn row(self) -> &'static str {
        match self {
            What::Connect => "action-reconnect",
            What::Remove => "action-remove",
            What::AllowDevices => "action-allow-devices",
            What::SetUp => "action-setup",
            What::Uninstall => "action-uninstall",
        }
    }
}

pub struct ComputersView {
    client: BoxClient,
    /// Run against a live box (off this thread), else at once (tests and shots).
    live: bool,
    computers: Reading<Vec<Computer>>,
    /// Why this app doesn't reach your other computers (its pairing, as the box on this computer
    /// said it): a title and the box's words, whole. None while it does.
    pairing: Option<(String, String)>,
    /// The actions under way, by computer and row: their words meanwhile, and the work.
    busy: HashMap<(String, &'static str), (String, Task<()>)>,
    /// What the box last refused, in its words, over the page it was asked on.
    problem: Option<(Shown, String)>,
    /// The confirm open: its words, and what going ahead does.
    confirm: Option<(Confirm, Manage)>,
    confirm_focus: FocusHandle,
    focus_confirm: bool,
    this_box: String,
    this_kind: String,
    /// The window shows this Mac's own box (as `held_words` takes it).
    this_mac: bool,
    shown: Shown,
    host: Entity<TextField>,
    /// Adding computers, as the box says it: whether it does, its hosts, its tries.
    adding: Reading<Adding>,
    /// Something asked of the box about a try, under way.
    add_busy: Option<Task<()>>,
    /// What the box last refused about adding, in its words.
    add_problem: Option<String>,
    _adding_follow: Option<Follow<Followed<Adding>>>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
    items: Vec<Item>,
    list: ListState,
}

impl EventEmitter<ComputersEvent> for ComputersView {}

impl ComputersView {
    pub fn new(client: BoxClient, live: bool, this_mac: bool, cx: &mut Context<Self>) -> ComputersView {
        let host = cx.new(|cx| TextField::new(cx).placeholder("my-server or user@host"));
        let typed = cx.subscribe(&host, |this: &mut ComputersView, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submit(_) => this.add(cx),
            TextFieldEvent::Cancel => cx.emit(ComputersEvent::Select(None)),
            _ => {
                this.add_problem = None;
                this.rebuild(cx);
            }
        });
        let mut view = ComputersView {
            client,
            live,
            computers: Reading::NotYet,
            pairing: None,
            busy: HashMap::new(),
            problem: None,
            confirm: None,
            confirm_focus: cx.focus_handle(),
            focus_confirm: false,
            this_box: "This Mac".into(),
            this_kind: String::new(),
            this_mac,
            shown: Shown::All,
            host,
            adding: Reading::NotYet,
            add_busy: None,
            add_problem: None,
            _adding_follow: None,
            _tasks: Vec::new(),
            _subscriptions: vec![typed],
            items: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
        };
        let sized = kuvo_ui::theme::remeasure_on_text_size(&view.list, cx);
        view._subscriptions.push(sized);
        // The box's tries, followed as they go; in tests and shots, read as each thing is asked.
        if live {
            let follow = view.client.follow_adding(Timing::default());
            let updates = follow.updates().clone();
            view._adding_follow = Some(follow);
            view._tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    let read = match update {
                        Followed::Now(said) => Ok(said),
                        Followed::Failed(error) => Err(error),
                    };
                    if this.update(cx, |this, cx| this.take_adding(read, cx)).is_err() {
                        break;
                    }
                }
            }));
        } else {
            view.adding = reading(view.client.adding(), &Reading::NotYet);
            view.fit_field(cx);
        }
        view.rebuild(cx);
        view
    }

    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    /// What the page says under `key`, as it draws it: a notice's title and words, a fact's value, a
    /// note's words (the title alone).
    pub fn said(&self, key: &str) -> Option<(String, String)> {
        self.items.iter().find_map(|item| match item {
            Item::Notice { key: at, title, words, .. } if at == key => Some((title.clone(), words.clone())),
            Item::Fact { key: at, title, value, .. } if at == key => Some((title.clone(), value.clone())),
            Item::Working { title, .. } if key == "fact-at-work" => Some((title.clone(), String::new())),
            Item::Note { key: at, words, .. } if at == key => Some((words.clone(), String::new())),
            _ => None,
        })
    }

    /// The confirm open, as drawn: its question, what it comes to, and its button.
    pub fn confirm_words(&self) -> Option<(String, String, String)> {
        self.confirm.as_ref().map(|(confirm, _)| (confirm.title.clone(), confirm.message.clone(), confirm.label.clone()))
    }

    /// The tries the page draws now, in its order, as the box says them.
    pub fn tries_drawn(&self) -> Vec<AddTry> {
        self.items.iter().filter_map(|item| if let Item::Try(tried) = item { Some((**tried).clone()) } else { None }).collect()
    }

    /// What's typed for a machine's address.
    pub fn typed_host(&self, cx: &App) -> String {
        self.host.read(cx).text().to_owned()
    }

    /// Reads the box's tries again (where they aren't followed).
    pub fn read_adding(&mut self, cx: &mut Context<Self>) {
        let read = self.client.adding();
        self.take_adding(read, cx);
    }

    fn take_adding(&mut self, read: kuvo_box::Result<Adding>, cx: &mut Context<Self>) {
        let adding = reading(read, &self.adding);
        if self.adding != adding {
            // A try that came to a computer: the list has it now.
            let came = |adding: &Reading<Adding>| match adding {
                Reading::Ready(adding) => adding.tries.iter().filter(|tried| tried.state == "done").count(),
                _ => 0,
            };
            let more = came(&adding) != came(&self.adding);
            self.adding = adding;
            self.fit_field(cx);
            if more {
                cx.emit(ComputersEvent::ReadAgain);
            }
            self.rebuild(cx);
        }
    }

    /// The address field as the box's word leaves it: taking typing only where the box adds
    /// computers, and clear once what was typed is being added (refused, it stays to be mended).
    fn fit_field(&mut self, cx: &mut Context<Self>) {
        let Reading::Ready(adding) = &self.adding else { return };
        let (can, typed) = (adding.can, self.host.read(cx).text().trim().to_owned());
        let begun = !typed.is_empty() && adding.tries.iter().any(|tried| tried.kind == "add" && tried.under_way() && tried.destination == typed);
        self.host.update(cx, |host, cx| {
            host.set_disabled(!can, cx);
            if begun {
                host.set_text("", cx);
            }
        });
    }

    /// Asks the box something about adding (off this thread in the app); a refusal is shown in
    /// its words, and its tries are read again where they aren't followed.
    fn ask_adding(&mut self, work: Work, cx: &mut Context<Self>) {
        if self.add_busy.is_some() {
            return;
        }
        self.add_problem = None;
        let client = self.client.clone();
        let then = |this: &mut ComputersView, done: kuvo_box::Result<()>, cx: &mut Context<ComputersView>| {
            this.add_busy = None;
            if let Err(error) = done {
                this.add_problem = Some(said(&error));
            }
            if this._adding_follow.is_none() {
                this.read_adding(cx);
            }
            this.rebuild(cx);
        };
        if !self.live {
            let done = work(&client);
            return then(self, done, cx);
        }
        self.add_busy = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work(&client) }).await;
            this.update(cx, |this, cx| then(this, done, cx)).ok();
        }));
        self.rebuild(cx);
    }

    /// Asks the box to add the machine at the address typed. Whether it's an address is the box's to say.
    fn add(&mut self, cx: &mut Context<Self>) {
        let destination = self.host.read(cx).text().trim().to_owned();
        if destination.is_empty() {
            return;
        }
        self.add_to(destination, cx);
    }

    fn add_to(&mut self, destination: String, cx: &mut Context<Self>) {
        self.ask_adding(Box::new(move |client| client.add_computer(&destination).map(|_| ())), cx);
    }

    fn answer(&mut self, id: String, at: u64, answer: String, cx: &mut Context<Self>) {
        self.ask_adding(Box::new(move |client| client.answer_add(&id, at, &answer).map(|_| ())), cx);
    }

    /// Stops a try under way, or forgets one that's over.
    fn end(&mut self, id: String, cx: &mut Context<Self>) {
        self.ask_adding(Box::new(move |client| client.end_add(&id)), cx);
    }

    /// Tries again what failed: the same asked of the box anew.
    fn again(&mut self, tried: &AddTry, cx: &mut Context<Self>) {
        let (kind, destination, key) = (tried.kind.clone(), tried.destination.clone(), self.key_at(&tried.destination));
        self.ask_adding(
            Box::new(move |client| match (kind.as_str(), key) {
                ("setup", Some(key)) => client.setup_computer(&key).map(|_| ()),
                ("uninstall", Some(key)) => client.uninstall_computer(&key).map(|_| ()),
                ("add", _) => client.add_computer(&destination).map(|_| ()),
                // (A computer that's gone from the list since: nothing to ask again.)
                _ => Ok(()),
            }),
            cx,
        );
    }

    /// The machine added under `destination` (its name on this box), by its key.
    fn key_at(&self, destination: &str) -> Option<String> {
        match &self.computers {
            Reading::Ready(computers) => computers.iter().find(|computer| computer.id.as_deref() == Some(destination)).map(|computer| computer.key.clone()),
            _ => None,
        }
    }

    /// The box's tries about `computer`: it's being set up again or uninstalled, or that failed.
    fn tries_of(&self, computer: &Computer) -> Vec<AddTry> {
        match &self.adding {
            Reading::Ready(adding) if !computer.here => adding.tries.iter().filter(|tried| computer.id.as_deref() == Some(tried.destination.as_str()) && (tried.kind != "add" || tried.under_way())).cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// Add a Machine: what a machine is; the box's tries at adding one, those under way first; and
    /// the address to give, with the hosts the box suggests, unless one is being added now.
    fn add_items(&self, cx: &App) -> Vec<Item> {
        let this = &self.this_box;
        let mut items = vec![Item::Title { title: "Add a Machine".into(), line: format!("A Mac or Linux server you reach with SSH, like a Mac mini or a VPS. Its sessions, bots and agents run there, and show here beside {this}’s."), add: false }];
        let adding = match &self.adding {
            Reading::NotYet => {
                items.push(Item::Note { key: "add-loading".into(), words: format!("Asking {this}…"), busy: true });
                items.push(Item::End);
                return items;
            }
            Reading::Failed(why) => {
                items.push(Item::Notice { key: "add-failed".into(), tone: Tone::Attention, title: format!("{this} couldn’t be asked about adding machines"), words: why.clone() });
                items.push(Item::End);
                return items;
            }
            Reading::Ready(adding) => adding,
        };
        let tries: Vec<&AddTry> = adding.tries.iter().filter(|tried| tried.kind == "add").collect();
        items.extend(tries.iter().filter(|tried| tried.under_way()).map(|tried| Item::Try(Box::new((*tried).clone()))));
        // One at a time on this page: while one is being added, its steps are the page.
        if !tries.iter().any(|tried| tried.under_way()) {
            // How the last ones ended, first: what you were waiting for, then the form for another.
            items.extend(tries.iter().filter(|tried| !tried.under_way()).map(|tried| Item::Try(Box::new((*tried).clone()))));
            items.push(Item::AddForm { can: adding.can });
            if let Some(why) = adding.why.as_ref().filter(|_| !adding.can) {
                items.push(Item::Notice { key: "add-cannot".into(), tone: Tone::Info, title: format!("Machines aren’t added from {this}"), words: why.clone() });
            } else {
                let typed = self.host.read(cx).text().trim().to_lowercase();
                let hosts: Vec<String> = adding.hosts.iter().filter(|host| host.to_lowercase() != typed && host.to_lowercase().contains(&typed)).take(6).cloned().collect();
                if !hosts.is_empty() {
                    items.push(Item::Hosts(hosts));
                }
                items.push(Item::Note { key: "add-how".into(), words: format!("{this} connects with its SSH keys and agent, never a password, and installs Kuvo Code in ~/.kuvo-code there."), busy: false });
            }
            if let Some(problem) = &self.add_problem {
                items.push(Item::Notice { key: "add-problem".into(), tone: Tone::Attention, title: "That can’t be added".into(), words: problem.clone() });
            }
            items.push(Item::AddFoot { ready: adding.can && !self.host.read(cx).text().trim().is_empty(), busy: self.add_busy.is_some() });
        } else if let Some(problem) = &self.add_problem {
            items.push(Item::Notice { key: "add-problem".into(), tone: Tone::Attention, title: "That couldn’t be done".into(), words: problem.clone() });
        }
        items.push(Item::End);
        items
    }

    pub fn show(&mut self, key: Option<&str>, cx: &mut Context<Self>) {
        let shown = Shown::from_key(key);
        if self.shown != shown {
            self.add_problem = None;
            if shown == Shown::Add && self._adding_follow.is_none() {
                self.adding = reading(self.client.adding(), &self.adding);
                self.fit_field(cx);
            }
            self.shown = shown;
            self.confirm = None;
            // Each page opens at its top.
            self.list.scroll_to(gpui::ListOffset::default());
            self.rebuild(cx);
        }
    }

    /// How this app's pairing stands, where it isn't paired: said over All and This Computer.
    pub fn set_pairing(&mut self, pairing: Option<(String, String)>, cx: &mut Context<Self>) {
        if self.pairing != pairing {
            self.pairing = pairing;
            self.rebuild(cx);
        }
    }

    pub fn set_computers(&mut self, computers: Reading<Vec<Computer>>, cx: &mut Context<Self>) {
        if self.computers != computers {
            self.computers = computers;
            self.rebuild(cx);
        }
    }

    pub fn set_this_box(&mut self, name: &str, kind: &str, cx: &mut Context<Self>) {
        if self.this_box != name || self.this_kind != kind {
            self.this_box = name.to_owned();
            self.this_kind = kind.to_owned();
            self.rebuild(cx);
        }
    }

    /// The computer this window shows, as the box lists it; while the list isn't read, what the
    /// window knows of it (its name and kind), with no claim of how it stands.
    fn this(&self) -> Computer {
        let listed = match &self.computers {
            Reading::Ready(computers) => computers.iter().find(|computer| computer.here).cloned(),
            _ => None,
        };
        let mut this = listed.unwrap_or_else(|| Computer { kind: self.this_kind.clone(), here: true, via: "here".into(), ..Default::default() });
        // Named as the window names it ("This Mac" for this Mac's own).
        this.name = self.this_box.clone();
        this
    }

    fn other(&self, key: &str) -> Option<Computer> {
        match &self.computers {
            Reading::Ready(computers) => computers.iter().find(|computer| !computer.here && computer.key == key).cloned(),
            _ => None,
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let mut items = match self.shown.clone() {
            Shown::All => self.all_items(),
            Shown::This => self.page_items(self.this()),
            Shown::Computer(key) => match self.other(&key) {
                Some(computer) => self.page_items(computer),
                None => self.missing_items(),
            },
            Shown::Add => self.add_items(cx),
        };
        if let (Some((title, words)), Shown::All | Shown::This) = (&self.pairing, &self.shown) {
            items.insert(1.min(items.len()), Item::Notice { key: "notice-pairing".into(), tone: Tone::Attention, title: title.clone(), words: words.clone() });
        }
        // Only the items that changed are laid out again; the rest keep their places and the page
        // keeps its scroll.
        if items != self.items {
            let start = self.items.iter().zip(&items).take_while(|(old, new)| old == new).count();
            let tail = self.items[start..].iter().rev().zip(items[start..].iter().rev()).take_while(|(old, new)| old == new).count();
            self.list.splice(start..self.items.len() - tail, items.len() - tail - start);
            self.items = items;
        }
        cx.notify();
    }

    /// All your computers: yours, then those filed apart, each a row with how it stands.
    fn all_items(&self) -> Vec<Item> {
        let mut items = vec![Item::Title { title: "Your Computers".into(), line: "Each runs its own sessions, bots and agents.".into(), add: true }];
        let computers = match &self.computers {
            Reading::NotYet => {
                items.push(Item::Note { key: "computers-loading".into(), words: "Loading your computers…".into(), busy: true });
                items.push(Item::End);
                return items;
            }
            Reading::Failed(why) => {
                items.push(Item::Note { key: "computers-failed".into(), words: format!("Your computers couldn’t be read: {why}"), busy: false });
                items.push(Item::End);
                return items;
            }
            Reading::Ready(computers) => computers,
        };
        let named = |computer: &Computer| {
            let mut computer = computer.clone();
            if computer.here {
                computer.name = self.this_box.clone();
            }
            computer
        };
        let (apart, yours): (Vec<&Computer>, Vec<&Computer>) = computers.iter().partition(|computer| computer.shared || computer.apart);
        // (The computer this window shows is known without the list.)
        let yours: Vec<Computer> = if yours.iter().any(|computer| computer.here) { yours.into_iter().map(named).collect() } else { std::iter::once(self.this()).chain(yours.into_iter().map(named)).collect() };
        // What this box did with a machine that's no longer listed (uninstalled, or it failed and
        // the machine is gone since), where its own page can't say it.
        if let Reading::Ready(adding) = &self.adding {
            let gone = adding.tries.iter().filter(|tried| tried.kind != "add" && !tried.under_way() && !computers.iter().any(|computer| computer.id.as_deref() == Some(tried.destination.as_str())));
            items.extend(gone.map(|tried| Item::Try(Box::new(tried.clone()))));
        }
        rows(&mut items, yours);
        if yours_alone(computers) {
            items.push(Item::Note { key: "computers-alone".into(), words: "Add a machine (a Mac or Linux server you reach with SSH) to run sessions and bots there too.".into(), busy: false });
        }
        if !apart.is_empty() {
            items.push(Item::Group("Shared Machines".into()));
            rows(&mut items, apart.into_iter().map(named).collect());
        }
        items.push(Item::End);
        items
    }

    /// One computer's page: who it is, how it stands and why, what's at work there, what it runs,
    /// and what can be done with it.
    fn page_items(&self, computer: Computer) -> Vec<Item> {
        let name = computer.name.clone();
        let mut items = vec![Item::Head(Box::new(computer.clone()))];
        if let Some(notice) = self.notice(&computer) {
            items.push(notice);
        }
        if let Some((on, problem)) = &self.problem
            && *on == self.shown
        {
            items.push(Item::Notice { key: "manage-problem".into(), tone: Tone::Attention, title: "That couldn’t be done".into(), words: problem.clone() });
        }
        // What this box is doing with it over SSH now (set up again, uninstalled), or that failed.
        items.extend(self.tries_of(&computer).into_iter().map(|tried| Item::Try(Box::new(tried))));
        // What's at work there, as its box counts it, where it said (`working`); none is claimed of
        // one that never said. One that's away has no At Work (a drawing choice: its notice says
        // why, and the box's 0 there means "nothing seen", not "nothing at work"). This window's
        // own are under Sessions.
        // (Which ones, `atWork`, waits on its design: the count is drawn as before.)
        let working = computer.working.filter(|_| computer.reach != "away");
        if let Some(working) = working {
            items.push(Item::Group("At Work".into()));
            if working == 0 {
                items.push(Item::Note { key: "at-work-none".into(), words: format!("Nothing is at work on {name} now."), busy: false });
            } else {
                items.push(Item::Working { title: computer.working_title.clone().unwrap_or_default(), open: computer.here });
                if !computer.here {
                    // (The window shows another computer from the title bar's computer menu.)
                    items.push(Item::Note { key: "at-work-elsewhere".into(), words: format!("To open them, pick {name} in the computer menu at the top of the window."), busy: false });
                }
            }
        }
        items.push(Item::Group("About".into()));
        let said = |value: &Option<String>| value.clone().unwrap_or_else(|| "Not said yet".into());
        let mut version = said(&computer.version);
        if computer.is_older() {
            version = format!("{version}, older than this app");
        }
        let limits = if computer.limits.is_empty() {
            "None".to_owned()
        } else {
            computer.limit_services.iter().map(|service| service.title.clone()).collect::<Vec<_>>().join(", ")
        };
        let facts = [
            ("fact-kind", "Kind", if computer.is_mac() { "Mac".to_owned() } else if computer.kind.is_empty() { "Not said yet".to_owned() } else { "Machine".to_owned() }),
            ("fact-system", "System", said(&computer.system)),
            ("fact-version", "Kuvo Code", version),
            ("fact-engine", "Engine", said(&computer.engine)),
            ("fact-reached", "Reached", reached(&computer)),
            ("fact-limits", "Plan limits it reads", limits),
        ];
        let count = facts.len();
        items.extend(facts.into_iter().enumerate().map(|(ix, (key, title, value))| Item::Fact { key: key.into(), title: title.into(), value, first: ix == 0, last: ix + 1 == count }));
        // Its app was removed from your devices, and its box may add none until you allow it: said
        // in the main computer's words, above what can be done about it.
        if let Some(vouching) = computer.vouching.as_ref().filter(|vouching| vouching.blocked) {
            items.push(Item::Notice { key: "notice-vouching".into(), tone: Tone::Attention, title: format!("Kuvo Code’s app on {name} isn’t one of your devices"), words: vouching.words.clone() });
        }
        // What can be done with it, as its box offers it here (`actions`), in its order and words.
        let actions: Vec<Offered> = computer
            .actions
            .iter()
            .filter_map(|action| {
                // (Marked as the platform marks a button that asks first.)
                let label = if action.confirm.is_some() { format!("{}…", action.label) } else { action.label.clone() };
                // (One the box says can't be done now isn't done here: its words say why.)
                let act = |what| action.why.is_none().then(|| Manage { what, key: computer.key.clone(), confirm: action.confirm.clone() });
                let why = action.why.clone();
                Some(match action.id.as_str() {
                    // (Renaming waits on its design: drawn, not done; it opens an editor, hence "…".)
                    "rename" => ("action-rename", "Name".to_owned(), format!("{}…", action.label), None, why),
                    "connect" => (What::Connect.row(), "Connection".to_owned(), label, act(What::Connect), why),
                    "remove" => (What::Remove.row(), "Remove".to_owned(), label, act(What::Remove), why),
                    "allow-devices" => (What::AllowDevices.row(), "Kuvo Code’s app there".to_owned(), label, act(What::AllowDevices), why),
                    "setup" => (What::SetUp.row(), "Kuvo Code there".to_owned(), label, act(What::SetUp), why),
                    "uninstall" => (What::Uninstall.row(), "Uninstall".to_owned(), label, act(What::Uninstall), why),
                    _ => return None,
                })
            })
            .collect();
        if !actions.is_empty() {
            items.push(Item::Group("Manage".into()));
            let count = actions.len();
            let whys: Vec<String> = actions.iter().filter_map(|(_, _, _, _, why)| why.clone()).collect();
            items.extend(actions.into_iter().enumerate().map(|(ix, (key, title, button, act, why))| {
                let busy = self.busy.get(&(computer.key.clone(), key)).map(|(words, _)| words.clone());
                Item::Action { key: key.into(), title, button, act, busy, why, first: ix == 0, last: ix + 1 == count }
            }));
            // What waits, and why, in the box's words.
            for (ix, why) in whys.into_iter().enumerate() {
                items.push(Item::Note { key: if ix == 0 { "manage-why".into() } else { format!("manage-why-{ix}") }, words: why, busy: false });
            }
        }
        items.push(Item::End);
        items
    }

    /// Does what a Manage button offers: asked first where the box says to, then done by the box;
    /// the list is read again (followed, it comes by itself). A refusal is shown in the box's words.
    fn manage(&mut self, act: Manage, cx: &mut Context<Self>) {
        if let Some(confirm) = act.confirm.clone() {
            self.focus_confirm = true;
            self.confirm = Some((confirm, act));
            return self.rebuild(cx);
        }
        let row = act.what.row();
        let busy = (act.key.clone(), row);
        if self.busy.contains_key(&busy) {
            return;
        }
        let (words, work): (&str, Work) = match act.what {
            What::Remove => ("Removing…", Box::new(move |client| client.remove_computer(&act.key))),
            What::Connect => ("Connecting…", Box::new(move |client| client.connect_computer(&act.key))),
            What::AllowDevices => ("Allowing…", Box::new(move |client| client.allow_devices(&act.key))),
            What::SetUp => ("Starting…", Box::new(move |client| client.setup_computer(&act.key).map(|_| ()))),
            What::Uninstall => ("Starting…", Box::new(move |client| client.uninstall_computer(&act.key).map(|_| ()))),
        };
        let tries = matches!(act.what, What::SetUp | What::Uninstall);
        self.problem = None;
        let on = self.shown.clone();
        let done_with = busy.clone();
        let then = move |this: &mut ComputersView, done: kuvo_box::Result<()>, cx: &mut Context<ComputersView>| {
            this.busy.remove(&done_with);
            match done {
                Ok(()) => cx.emit(ComputersEvent::ReadAgain),
                Err(error) => this.problem = Some((on, said(&error))),
            }
            // (What it began is one of the box's tries: read where they aren't followed.)
            if tries && this._adding_follow.is_none() {
                this.read_adding(cx);
            }
        };
        let client = self.client.clone();
        if !self.live {
            let done = work(&client);
            then(self, done, cx);
            return self.rebuild(cx);
        }
        let task = cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work(&client) }).await;
            this.update(cx, |this, cx| {
                then(this, done, cx);
                this.rebuild(cx);
            })
            .ok();
        });
        self.busy.insert(busy, (words.to_owned(), task));
        self.rebuild(cx);
    }

    /// Why a computer stands as it does, where that's more than "connected".
    fn notice(&self, computer: &Computer) -> Option<Item> {
        let name = &computer.name;
        if computer.is_older() {
            let (title, words) = held_words(Some(name), computer.here && self.this_mac);
            return Some(Item::Notice { key: "notice-older".into(), tone: Tone::Attention, title, words });
        }
        if computer.apart {
            return Some(Item::Notice {
                key: "notice-apart".into(),
                tone: Tone::Info,
                title: format!("{name} isn’t one of your own computers"),
                words: "Someone else’s setup shares this machine, so what runs there isn’t shown.".into(),
            });
        }
        computer.notice.as_ref().map(|notice| Item::Notice {
            key: format!("notice-{}", computer.reach),
            tone: if computer.reach == "unknown" { Tone::Info } else { Tone::Waiting },
            title: notice.title.clone(),
            words: notice.detail.clone(),
        })
    }

    /// A computer picked that the list doesn't have (gone since, or not read yet).
    fn missing_items(&self) -> Vec<Item> {
        let words = match &self.computers {
            Reading::NotYet => return vec![Item::Note { key: "computers-loading".into(), words: "Loading your computers…".into(), busy: true }, Item::End],
            Reading::Failed(why) => format!("Your computers couldn’t be read: {why}"),
            Reading::Ready(_) => "This computer isn’t among yours any more.".into(),
        };
        vec![Item::Note { key: "computer-missing".into(), words, busy: false }, Item::End]
    }

    fn item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let Some(item) = self.items.get(ix).cloned() else { return div().into_any_element() };
        let body: AnyElement = match item {
            Item::Title { title, line, add } => div()
                .debug_selector(|| "computers-title".into())
                .pt(space::S8)
                .pb(space::S2)
                .flex()
                .items_center()
                .gap(space::S4)
                .child(div().flex_1().min_w_0().flex().flex_col().gap(space::S1).child(div().text_ui(TypeSize::Title).medium().text_color(c.text).child(title)).child(div().text_ui(TypeSize::Body).text_color(c.text_muted).child(line)))
                .when(add, |d| d.child(div().flex_none().child(Button::new("computers-add", "Add a Machine…").icon(IconName::Plus).on_click(cx.listener(|_, _, _, cx| cx.emit(ComputersEvent::Select(Some("add".into()))))))))
                .into_any_element(),
            Item::Group(label) => div().pt(space::S6).pb(space::S2).text_ui(TypeSize::Body).text_color(c.text_muted).child(label).into_any_element(),
            Item::Row { computer, first, last } => self.row(&computer, first, last, cx),
            Item::Head(computer) => head(&computer, cx),
            Item::Notice { key, tone, title, words } => {
                let (icon, color) = match tone {
                    Tone::Waiting => (IconName::Clock, c.icon),
                    Tone::Attention => (IconName::Warning, c.warning),
                    Tone::Info => (IconName::Info, c.icon),
                };
                let title_selector = format!("{key}-title");
                div()
                    .pt(space::S4)
                    .child(
                        div()
                            .debug_selector(move || key)
                            .flex()
                            .items_start()
                            .gap(space::S3)
                            .px(space::S5)
                            .py(space::S4)
                            .rounded(radius::POPOVER)
                            .bg(c.card)
                            .shadow(cx.theme().card_shadow())
                            .child(div().pt(px(2.)).child(Icon::new(icon).color(color)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(space::S0_5)
                                    .child(div().debug_selector(move || title_selector).text_ui(TypeSize::Body).medium().text_color(c.text).child(title))
                                    .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words)),
                            ),
                    )
                    .into_any_element()
            }
            Item::Fact { key, title, value, first, last } => {
                let value_selector = format!("{key}-value");
                card_row(key, first, last, cx)
                    .child(div().flex_1().min_w_0().text_ui(TypeSize::Body).text_color(c.text).child(title))
                    .when(!value.is_empty(), |d| d.child(div().debug_selector(move || value_selector).flex_shrink_0().max_w(px(360.)).text_right().text_ui(TypeSize::Body).text_color(c.text_muted).child(value)))
                    .into_any_element()
            }
            Item::Working { title, open } => {
                card_row("fact-at-work".into(), true, true, cx)
                    .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).child(spinner("at-work-spin", IconSize::Nav, c.text_muted)))
                    .child(div().flex_1().min_w_0().text_ui(TypeSize::Body).text_color(c.text).child(title))
                    .when(open, |d| d.child(Button::new("at-work-show", "Show in Sessions").size(ButtonSize::Small).on_click(cx.listener(|_, _, _, cx| cx.emit(ComputersEvent::Sessions)))))
                    .into_any_element()
            }
            Item::Action { key, title, button, act, busy, why, first, last } => {
                let button = Button::new(SharedString::from(format!("{key}-button")), busy.clone().unwrap_or(button)).size(ButtonSize::Small);
                let button = match act {
                    Some(_) if busy.is_some() => button.disabled(true),
                    Some(act) => button.on_click(cx.listener(move |this, _, _, cx| this.manage(act.clone(), cx))),
                    None => button.disabled(true).tooltip(why.map(SharedString::from).unwrap_or(NOT_YET.into())),
                };
                card_row(key.clone(), first, last, cx).child(div().flex_1().min_w_0().text_ui(TypeSize::Body).text_color(c.text).child(title)).child(button).into_any_element()
            }
            Item::Note { key, words, busy } => div()
                .debug_selector(move || key)
                .pt(space::S2)
                .flex()
                .items_start()
                .gap(space::S2)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .children(busy.then(|| div().pt(px(3.)).child(spinner("computers-reading", IconSize::Nav, c.text_muted))))
                .child(div().flex_1().min_w_0().child(words))
                .into_any_element(),
            Item::AddForm { .. } => self.add_form(cx),
            Item::Hosts(hosts) => div()
                .debug_selector(|| "add-hosts".into())
                .pt(space::S3)
                .flex()
                .flex_wrap()
                .items_center()
                .gap(space::S2)
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child("From your SSH config"))
                .children(hosts.into_iter().map(|host| {
                    let picked = host.clone();
                    let selector = format!("add-host-{host}");
                    div().debug_selector(move || selector).child(Button::new(SharedString::from(format!("add-host-{host}")), host).size(ButtonSize::Small).pill().on_click(cx.listener(move |this, _, _, cx| {
                        this.host.update(cx, |field, cx| field.set_text(picked.clone(), cx));
                        this.add_problem = None;
                        this.rebuild(cx);
                    })))
                }))
                .into_any_element(),
            Item::AddFoot { ready, busy } => div()
                .pt(space::S5)
                .flex()
                .items_center()
                .justify_end()
                .gap(space::S3)
                .child(Button::new("add-cancel", "Cancel").on_click(cx.listener(|_, _, _, cx| cx.emit(ComputersEvent::Select(None)))))
                .child(div().debug_selector(|| "add-machine".into()).child(Button::new("add-machine", "Add Machine").primary().disabled(!ready || busy).loading(busy).on_click(cx.listener(|this, _, _, cx| this.add(cx)))))
                .into_any_element(),
            Item::Try(tried) => self.try_card(&tried, cx),
            Item::End => div().h(space::S10).into_any_element(),
        };
        // Each item in a column as wide as a page's cards, in the middle.
        div().w_full().flex().justify_center().px(space::S8).child(div().w_full().max_w(px(PAGE_WIDTH)).child(body)).into_any_element()
    }

    /// A computer among all of them: how it stands, its system, the Kuvo Code it has, what's at
    /// work there; it opens its page.
    fn row(&mut self, computer: &Computer, first: bool, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let key = if computer.here { "this".to_owned() } else { format!("computer:{}", computer.key) };
        let id = if computer.here { "this".to_owned() } else { computer.key.clone() };
        let selector = format!("computers-row-{id}");
        let version = computer.version.as_ref().map(|version| format!("Kuvo Code {version}"));
        let working = computer.working.filter(|working| *working > 0).map(|working| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1)
                .child(spinner(SharedString::from(format!("computers-row-spin-{id}")), IconSize::Nav, c.text_muted))
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(format!("{working} at work")))
        });
        card_row(selector.clone(), first, last, cx)
            .id(SharedString::from(selector))
            .cursor_pointer()
            .hover(|s| s.bg(c.hover))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ComputersEvent::Select(Some(key.clone())))))
            .child(tile(computer, px(32.), cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_ui(TypeSize::Body).medium().text_color(c.text).child(computer.name.clone()))
                    .child(div().truncate().text_ui(TypeSize::Small).text_color(if computer.is_older() { c.warning } else { c.text_muted }).child(standing(computer))),
            )
            .children(working)
            .children(version.map(|version| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(version)))
            .child(Icon::new(IconName::ChevronRight).color(c.icon))
            .into_any_element()
    }

    /// A try as its box says it: its title and how it stands, its steps, what it asks or how it
    /// ended, and what can be done about it. Every word is the box's but the buttons that stop,
    /// retry and dismiss.
    fn try_card(&mut self, tried: &AddTry, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let id = tried.id.clone();
        let busy = self.add_busy.is_some();
        let (icon, color): (Option<IconName>, Hsla) = match tried.state.as_str() {
            "done" => (Some(IconName::Check), c.success),
            "failed" => (Some(IconName::Warning), c.warning),
            _ => (None, c.text_muted),
        };
        let head = div()
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S5)
            .py(space::S4)
            .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).map(|d| match icon {
                Some(icon) => d.child(Icon::new(icon).color(color)),
                None => d.child(spinner(SharedString::from(format!("add-try-spin-{id}")), IconSize::Nav, color)),
            }))
            .child(div().debug_selector(|| "add-try-title".into()).flex_1().min_w_0().text_ui(TypeSize::Body).medium().text_color(c.text).child(tried.title.clone()))
            .when(tried.state == "working", |d| {
                let id = id.clone();
                d.child(div().debug_selector(|| "add-try-cancel".into()).child(Button::new(SharedString::from(format!("add-try-cancel-{id}")), "Cancel").size(ButtonSize::Small).disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.end(id.clone(), cx)))))
            });
        let steps = tried.steps.iter().map(|step| {
            let selector = format!("add-step-{}", step.id);
            let mark: AnyElement = match step.state.as_str() {
                "done" => Icon::new(IconName::Check).color(c.text_muted).into_any_element(),
                "now" => spinner(SharedString::from(format!("add-step-spin-{}-{}", tried.id, step.id)), IconSize::Nav, c.text_muted).into_any_element(),
                "failed" => Icon::new(IconName::Warning).color(c.warning).into_any_element(),
                _ => div().size(px(6.)).rounded_full().bg(c.border).into_any_element(),
            };
            // What the step says beside it (how far a copy has got, in the box's words).
            let detail = step.detail.clone();
            let waiting = step.state == "waiting";
            div()
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(space::S3)
                .px(space::S5)
                .py(space::S2)
                .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).child(mark))
                .child(div().flex_1().min_w_0().text_ui(TypeSize::Body).text_color(if waiting { c.text_muted } else { c.text }).child(step.title.clone()))
                .children(detail.map(|detail| div().flex_shrink_0().max_w(px(380.)).truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(detail)))
        });
        // What it asks: the box's question, its facts (a fingerprint in the code font), its answers.
        let question = tried.question.as_ref().map(|question| {
            let answers = question.answers.iter().map(|answer| {
                let (try_id, at, answer_id) = (tried.id.clone(), question.at, answer.id.clone());
                let selector = format!("add-answer-{}", answer.id);
                let button = Button::new(SharedString::from(format!("add-answer-{}-{}", tried.id, answer.id)), answer.label.clone()).disabled(busy);
                let button = match answer.kind.as_str() {
                    "primary" => button.primary(),
                    "danger" => button.destructive(),
                    _ => button,
                };
                div().debug_selector(move || selector).child(button.on_click(cx.listener(move |this, _, _, cx| this.answer(try_id.clone(), at, answer_id.clone(), cx))))
            });
            div()
                .debug_selector(|| "add-question".into())
                .border_t_1()
                .border_color(c.border)
                .px(space::S5)
                .py(space::S4)
                .flex()
                .flex_col()
                .gap(space::S3)
                .child(div().flex().flex_col().gap(space::S0_5).child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(question.title.clone())).child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(question.message.clone())))
                .when(!question.facts.is_empty(), |d| {
                    d.child(div().rounded(radius::NAV).bg(c.surface).px(space::S4).py(space::S3).flex().flex_col().gap(space::S2).children(question.facts.iter().map(|fact| {
                        let value = div().flex_1().min_w_0().text_color(c.text);
                        let value = if fact.code { value.text_mono(TypeSize::Body) } else { value.text_ui(TypeSize::Body) };
                        div().flex().items_start().gap(space::S4).child(div().w(px(170.)).flex_none().text_ui(TypeSize::Small).text_color(c.text_muted).child(fact.label.clone())).child(value.child(fact.value.clone()))
                    })))
                })
                .child(div().flex().items_center().justify_end().gap(space::S3).children(answers))
        });
        // How it ended: why it failed, with trying again; or the computer it came to.
        let failure = tried.failure.as_ref().map(|failure| {
            let (again, dismissed) = (tried.clone(), tried.id.clone());
            div()
                .debug_selector(|| "add-failure".into())
                .border_t_1()
                .border_color(c.border)
                .px(space::S5)
                .py(space::S4)
                .flex()
                .flex_col()
                .gap(space::S3)
                .child(div().flex().flex_col().gap(space::S0_5).when(failure.title != tried.title, |d| d.child(div().text_ui(TypeSize::Body).medium().text_color(c.text).child(failure.title.clone()))).child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(failure.message.clone())))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(space::S3)
                        .child(div().debug_selector(|| "add-try-dismiss".into()).child(Button::new(SharedString::from(format!("add-try-dismiss-{}", tried.id)), "Dismiss").size(ButtonSize::Small).disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.end(dismissed.clone(), cx)))))
                        .when(failure.again, |d| d.child(div().debug_selector(|| "add-try-again".into()).child(Button::new(SharedString::from(format!("add-try-again-{}", tried.id)), "Try Again").size(ButtonSize::Small).disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.again(&again, cx)))))),
                )
        });
        let done = (tried.state == "done").then(|| {
            let dismissed = tried.id.clone();
            let show = tried.computer.clone().filter(|_| tried.kind != "uninstall");
            div()
                .border_t_1()
                .border_color(c.border)
                .px(space::S5)
                .py(space::S3)
                .flex()
                .items_center()
                .justify_end()
                .gap(space::S3)
                .child(div().debug_selector(|| "add-try-dismiss".into()).child(Button::new(SharedString::from(format!("add-try-done-{}", tried.id)), "Dismiss").size(ButtonSize::Small).disabled(busy).on_click(cx.listener(move |this, _, _, cx| this.end(dismissed.clone(), cx)))))
                .children(show.map(|computer| {
                    let key = format!("computer:{}", computer.key);
                    div().debug_selector(|| "add-try-show".into()).child(Button::new(SharedString::from(format!("add-try-show-{}", tried.id)), format!("Show {}", computer.name)).size(ButtonSize::Small).primary().on_click(cx.listener(move |_, _, _, cx| cx.emit(ComputersEvent::Select(Some(key.clone()))))))
                }))
        });
        let selector = format!("add-try-{}", tried.id);
        div()
            .pt(space::S4)
            .child(
                div()
                    .debug_selector(move || selector)
                    .rounded(radius::POPOVER)
                    .bg(c.card)
                    .shadow(cx.theme().card_shadow())
                    .flex()
                    .flex_col()
                    .child(head)
                    // (One that's done says so in a line: its steps are all behind it.)
                    .when(tried.state != "done", |d| d.child(div().pb(space::S3).flex().flex_col().children(steps)))
                    .children(question)
                    .children(failure)
                    .children(done),
            )
            .into_any_element()
    }

    fn add_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .debug_selector(|| "add-form".into())
            .pt(space::S4)
            .flex()
            .flex_col()
            .gap(space::S2)
            .child(
                div()
                    .px(space::S5)
                    .py(space::S4)
                    .rounded(radius::POPOVER)
                    .bg(c.card)
                    .shadow(cx.theme().card_shadow())
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .child(Icon::new(IconName::Terminal).color(c.icon))
                    .child(div().flex_none().text_ui(TypeSize::Body).text_color(c.text).child("SSH host"))
                    .child(div().flex_1().min_w_0().child(self.host.clone())),
            )
            .into_any_element()
    }
}

/// Rows in one card.
fn rows(items: &mut Vec<Item>, computers: Vec<Computer>) {
    let count = computers.len();
    items.extend(computers.into_iter().enumerate().map(|(ix, computer)| Item::Row { computer: Box::new(computer), first: ix == 0, last: ix + 1 == count }));
}

/// Only the computer this window shows is listed (none of your own besides).
fn yours_alone(computers: &[Computer]) -> bool {
    !computers.iter().any(|computer| !computer.here && !computer.shared && !computer.apart)
}

/// How the box this window shows knows a computer.
fn reached(computer: &Computer) -> String {
    if computer.here {
        return "Shown in this window".into();
    }
    match computer.via.as_str() {
        "relay" => "Through the relay, end to end encrypted".into(),
        "peer" => format!("Only by {}’s word", computer.through.as_deref().unwrap_or("another computer")),
        _ => "Not said yet".into(),
    }
}

/// A row of a page's card.
fn card_row(key: String, first: bool, last: bool, cx: &App) -> gpui::Div {
    let c = &cx.theme().colors;
    div()
        .debug_selector(move || key)
        .w_full()
        .flex()
        .items_center()
        .gap(space::S3)
        .px(space::S5)
        .py(space::S3)
        .bg(c.card)
        .when(first, |d| d.rounded_t(radius::POPOVER))
        .when(last, |d| d.rounded_b(radius::POPOVER))
        .when(!last, |d| d.border_b_1().border_color(c.border))
        .shadow(if first && last { cx.theme().card_shadow() } else { Vec::new() })
}

/// A computer's page head: its tile, its name and how it stands.
fn head(computer: &Computer, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let line = standing(computer);
    div()
        .debug_selector(|| "computer-head".into())
        .pt(space::S8)
        .pb(space::S2)
        .flex()
        .items_center()
        .gap(space::S4)
        .child(tile(computer, px(56.), cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(space::S1)
                .child(div().debug_selector(|| "computer-head-name".into()).truncate().text_ui(TypeSize::Title).medium().text_color(c.text).child(computer.name.clone()))
                .child(div().debug_selector(|| "computer-head-line".into()).text_ui(TypeSize::Body).text_color(c.text_muted).child(line)),
        )
        .into_any_element()
}

/// How it stands, then what it is: "Connected · Ubuntu 24.04.1 LTS", "Can’t be reached · Mac".
fn standing(computer: &Computer) -> String {
    let what = computer.system.clone().unwrap_or_else(|| if computer.is_mac() { "Mac".into() } else { "Machine".into() });
    if computer.here {
        format!("Shown in this window · {what}")
    } else if computer.reach.is_empty() {
        what
    } else {
        format!("{} · {what}", state_words(computer))
    }
}

/// How a computer stands, in a word or two.
fn state_words(computer: &Computer) -> &str {
    if computer.is_older() { return computer.reach_titles.get("held").map(String::as_str).unwrap_or_default(); }
    &computer.title
}

/// How a computer's reach is drawn: green connected, amber connecting or older, red away, faint
/// where this box can't say. None while the list isn't read (no claim).
pub fn reach_color(computer: &Computer, cx: &App) -> Option<Hsla> {
    let c = &cx.theme().colors;
    if computer.reach.is_empty() {
        return None;
    }
    if computer.is_older() {
        return Some(c.warning);
    }
    Some(match computer.reach.as_str() {
        "connected" => c.success,
        "connecting" => c.warning,
        "away" => c.danger,
        _ => c.text_faint,
    })
}

/// A computer's tile: a Mac or a machine, with a dot for how it stands.
pub fn tile(computer: &Computer, size: gpui::Pixels, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let id = if computer.here { "this".to_owned() } else { computer.key.clone() };
    let dot_selector = format!("computer-dot-{id}");
    let dot = (size * 0.32).min(px(12.));
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(size)
        .rounded(radius::NAV)
        .bg(c.sunken)
        .border_1()
        .border_color(c.border)
        .child(Icon::new(if computer.is_mac() { IconName::Computer } else { IconName::Terminal }).size(if size > px(40.) { IconSize::Card } else { IconSize::Nav }).color(c.icon))
        .children(reach_color(computer, cx).map(|color| div().debug_selector(move || dot_selector).absolute().bottom(px(-2.)).right(px(-2.)).size(dot).rounded(radius::PILL).border_2().border_color(c.surface).bg(color)))
        .into_any_element()
}

impl Render for ComputersView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        if std::mem::take(&mut self.focus_confirm) && self.confirm.is_some() {
            window.focus(&self.confirm_focus, cx);
        }
        // The box's confirm, as Settings draws one: its question, what it comes to, its button.
        let confirm = self.confirm.clone().map(|(confirm, act)| {
            let card = confirm_card(
                ConfirmIds { card: "computers-confirm", cancel: "computers-confirm-cancel", go: "computers-confirm-go" },
                &self.confirm_focus,
                (confirm.title, confirm.message, confirm.label),
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                    this.confirm = None;
                    cx.notify();
                }),
                cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.confirm = None;
                        cx.notify();
                    }
                }),
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.confirm = None;
                    this.manage(Manage { confirm: None, ..act.clone() }, cx);
                }),
                cx,
            );
            sheet(card, cx)
        });
        let selector = match self.shown {
            Shown::All => "computers-page",
            Shown::Add => "add-machine-page",
            _ => "computer-page",
        };
        div()
            .id("computers-page")
            .debug_selector(move || selector.into())
            .relative()
            .size_full()
            .bg(c.sidebar)
            .child(list(self.list.clone(), cx.processor(|this, ix, _, cx| this.item(ix, cx))).size_full())
            .children(confirm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computer(json: &str) -> Computer {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_key_picks_what_the_page_shows() {
        assert_eq!(Shown::from_key(None), Shown::All);
        assert_eq!(Shown::from_key(Some("this")), Shown::This);
        assert_eq!(Shown::from_key(Some("add")), Shown::Add);
        assert_eq!(Shown::from_key(Some("computer:key_devbox")), Shown::Computer("key_devbox".into()));
    }

    /// How a computer is reached is said as the box says it: by the relay, or by another's word.
    #[test]
    fn how_a_computer_is_reached_is_the_boxs_word() {
        assert_eq!(reached(&computer(r#"{"name":"devbox","via":"relay"}"#)), "Through the relay, end to end encrypted");
        assert_eq!(reached(&computer(r#"{"name":"mini","via":"peer","through":"devbox"}"#)), "Only by devbox’s word");
        assert_eq!(reached(&computer(r#"{"name":"Studio","here":true,"via":"here"}"#)), "Shown in this window");
    }
}
