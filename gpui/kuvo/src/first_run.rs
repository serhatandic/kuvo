//! The first run: what the window shows until the box says you've had it (`welcomed` in
//! `kuvo/setup`). One step at a time in a card on the dotted backdrop: a welcome, the coding agents
//! and plans to sign in to, what macOS must allow the program agents run under (only on a Mac
//! whose Kuvo says one isn't allowed yet: [`crate::permissions`]), and where to work (a computer,
//! and a folder on it); then the window opens on its landing view, set to start where you picked.
//!
//! Every fact is the box's: the accounts, how each stands and what can be done with it, in its
//! words (`kuvo/sign-in/accounts`: `plan` keeps the plans in the list and the keys apart, `added`
//! leaves out an account you added beside an agent's own); the models that need no account and
//! whether there's anything to ask about them at all (`kuvo/models/offered`'s `free`; `kuvo/setup/ready`); your computers
//! (`kuvo/computers`); and the folders worth starting in (`kuvo/folders/suggested`, browsed with
//! `kuvo/folders`). Signing in happens in place, under the account's row, with Settings' own
//! sign-in ([`SignIn`]); nothing here opens Settings or leaves the flow.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder as _, px, svg,
};
use kuvo_box::BoxClient;
use kuvo_box::accounts::{AccountEntry, Accounts, Side, State, said};
use kuvo_box::areas::Computer;
use kuvo_box::folders::{Folder, SuggestedFolders, Unreadable};
use kuvo_box::models::{Offered, OfferedModel, StartIn};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    backdrop::backdrop,
    small::spinner,
    theme::{radius, space},
};

use crate::areas::Reading;
use crate::settings::sign_in::{self, SignIn};
use crate::workspace::{BoxState, held_words};
use crate::{ToggleAppearance, computers, time, title_bar};

/// How many times, and how long apart, the box is asked again whether there's anything to ask
/// about your accounts, while it says a side can't tell yet.
const READY_AGAIN: u32 = 5;
const READY_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);
/// How wide its card is, where the window has the room.
const CARD: f32 = 560.;

/// Which card shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    /// The coding agents and plans, to sign in to.
    Agents,
    /// The accounts a key is entered for, behind "Use an API Key Instead…" (part of Agents).
    Keys,
    /// What macOS must allow the program this Mac's agents run under, each with its own Open
    /// System Settings; only where this Mac's Kuvo says one isn't allowed yet.
    Allow,
    /// A computer, and a folder on it.
    Where,
    /// A folder chosen by looking through the computer's folders (part of Where).
    Browse,
}

impl Step {
    /// Which of the dots it's at (`allow`: whether the permissions' step is among them).
    fn dot(self, allow: bool) -> usize {
        match self {
            Step::Welcome => 0,
            Step::Agents | Step::Keys => 1,
            Step::Allow => 2,
            Step::Where | Step::Browse => 2 + usize::from(allow),
        }
    }
}

/// The first run is done: the window opens on its landing view, to start in `place`.
pub struct Finished {
    pub place: StartIn,
}

/// A folder being looked through, as the box lists it.
#[derive(Clone, Debug, PartialEq)]
enum Listed {
    Reading,
    Ready(Folder),
    /// The computer wouldn't let it be read, in the box's words.
    Unreadable(Unreadable),
    Failed(String),
}

pub struct FirstRun {
    client: BoxClient,
    live: bool,
    step: Step,
    box_state: BoxState,
    box_name: SharedString,
    accounts: Reading<Accounts>,
    offered: Reading<Offered>,
    /// Whether the box says there's nothing to ask about your accounts (`kuvo/setup/ready`).
    ready: Reading<bool>,
    /// Which ask of that is the latest (an older one's answer is dropped), and how many times
    /// it's been asked again because the box couldn't say yet (`pending`).
    ready_asked: u64,
    ready_again: u32,
    /// The box said a side can't tell yet, and it's being asked again.
    waits_for_ready: bool,
    computers: Reading<Vec<Computer>>,
    folders: Reading<SuggestedFolders>,
    /// The computer picked, by its key (none: the box's own).
    computer: Option<String>,
    /// Where the first session starts (none: not picked yet).
    place: Option<StartIn>,
    /// A folder chosen by looking through the computer's, listed with the suggested ones.
    chosen: Option<String>,
    /// The folder being looked through, and what the box says is in it; and the folder it was
    /// opened from, to go back up to where the box couldn't list this one.
    browsing: Option<(String, Listed)>,
    browsed_from: Option<String>,
    /// Which read of the accounts is the latest asked for: an older one's answer is dropped.
    accounts_asked: u64,
    /// The accounts just signed in to, until the box has said how they stand now.
    checking: HashSet<String>,
    /// A sign-in under way in place: its account's key, and the sign-in.
    sign_in: Option<(String, Entity<SignIn>)>,
    sign_in_watch: Vec<Subscription>,
    /// The accounts whose agent is being installed, and what last went wrong with one, by key.
    installing: HashSet<String>,
    problems: HashMap<String, String>,
    /// Now, as the folders' times count from it (fixed for shots).
    now: Option<f64>,
    /// What macOS allows this Mac's agents, as the window's own reads it (none: not asked).
    permissions: Option<Entity<crate::permissions::HeadsUp>>,
    /// Whether the permissions' step has been shown: it stays one of the steps, back and forth,
    /// once what it asked is allowed.
    allow_shown: bool,
    focus: FocusHandle,
    wants_focus: bool,
    scroll: ScrollHandle,
    tasks: Vec<Task<()>>,
    _permissions: Option<Subscription>,
}

impl EventEmitter<Finished> for FirstRun {}

/// An account's key in this view: which side it's on, and its id.
fn key_of(entry: &AccountEntry) -> String {
    format!("{}-{}", if entry.side == Side::Agents { "agent" } else { "account" }, entry.integration.id)
}

/// A path as it reads from the computer's home folder (`~/Projects/api`), where it's in it.
fn from_home(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some("") if !home.is_empty() => "~".into(),
        Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// A folder's name: the last part of its path.
fn name_of(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().filter(|last| !last.is_empty()).unwrap_or(path).to_owned()
}

impl FirstRun {
    pub fn new(client: BoxClient, live: bool, box_state: BoxState, box_name: SharedString, cx: &mut Context<Self>) -> FirstRun {
        let mut run = FirstRun {
            client,
            live,
            step: Step::Welcome,
            box_state,
            box_name,
            accounts: Reading::NotYet,
            offered: Reading::NotYet,
            ready: Reading::NotYet,
            ready_asked: 0,
            ready_again: 0,
            waits_for_ready: false,
            computers: Reading::NotYet,
            folders: Reading::NotYet,
            computer: None,
            place: None,
            chosen: None,
            browsing: None,
            browsed_from: None,
            accounts_asked: 0,
            checking: HashSet::new(),
            sign_in: None,
            sign_in_watch: Vec::new(),
            installing: HashSet::new(),
            problems: HashMap::new(),
            now: None,
            permissions: None,
            allow_shown: false,
            focus: cx.focus_handle(),
            wants_focus: true,
            scroll: ScrollHandle::new(),
            tasks: Vec::new(),
            _permissions: None,
        };
        run.read(cx);
        run
    }

    /// What macOS allows this Mac's agents: where it says one isn't allowed yet, a step of its
    /// own before where to work.
    pub fn set_permissions(&mut self, permissions: Entity<crate::permissions::HeadsUp>, cx: &mut Context<Self>) {
        self._permissions = Some(cx.observe(&permissions, |_, _, cx| cx.notify()));
        self.permissions = Some(permissions);
    }

    /// Whether the permissions' step is one of the steps: shown already, or this Mac's Kuvo says
    /// something isn't allowed yet.
    fn allows(&self, cx: &App) -> bool {
        self.allow_shown || self.permissions.as_ref().is_some_and(|permissions| permissions.read(cx).asks())
    }

    /// The step after the agents: the permissions', where it's one of them, else where to work.
    fn after_agents(&self, cx: &App) -> Step {
        if self.allows(cx) { Step::Allow } else { Step::Where }
    }

    /// What macOS allows this Mac's agents, as the step reads it.
    pub fn permissions(&self) -> Option<&Entity<crate::permissions::HeadsUp>> {
        self.permissions.as_ref()
    }

    // MARK: What it shows, for the window and for tests

    pub fn step(&self) -> Step {
        self.step
    }

    /// Where the first session starts, as picked so far.
    pub fn place(&self) -> Option<&StartIn> {
        self.place.as_ref()
    }

    /// The sign-in under way in place, and whose it is (its account's key).
    pub fn sign_in(&self) -> Option<(&str, &Entity<SignIn>)> {
        self.sign_in.as_ref().map(|(key, sheet)| (key.as_str(), sheet))
    }

    pub fn set_now(&mut self, now: f64) {
        self.now = Some(now);
    }

    /// How the box stands with this window, and its name, as the window says them.
    pub fn set_box(&mut self, state: BoxState, name: SharedString, cx: &mut Context<Self>) {
        let reached = self.box_state != BoxState::Connected && state == BoxState::Connected;
        if self.box_state != state || self.box_name != name {
            self.box_state = state;
            self.box_name = name;
            cx.notify();
        }
        // Reached (again, or let go by its update): what it couldn't say before is asked now.
        if reached && (!matches!(self.accounts, Reading::Ready(_)) || !matches!(self.offered, Reading::Ready(_)) || !matches!(self.ready, Reading::Ready(_)) || !matches!(self.folders, Reading::Ready(_))) {
            self.read(cx);
        }
    }

    /// Your computers, as the box lists them (the window reads and follows them).
    pub fn set_computers(&mut self, computers: Reading<Vec<Computer>>, cx: &mut Context<Self>) {
        if self.computers != computers {
            self.computers = computers;
            cx.notify();
        }
    }

    /// Runs `work` against the box (in the app off this thread; in tests and shots at once).
    fn run<T: Send + 'static>(&mut self, work: impl FnOnce(&BoxClient) -> T + Send + 'static, then: impl FnOnce(&mut FirstRun, T, &mut Context<FirstRun>) + 'static, cx: &mut Context<Self>) {
        let client = self.client.clone();
        if !self.live {
            let done = work(&client);
            then(self, done, cx);
            cx.notify();
            return;
        }
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move { work(&client) }).await;
            this.update(cx, |this, cx| {
                then(this, done, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Reads everything the steps draw from the box.
    fn read(&mut self, cx: &mut Context<Self>) {
        self.read_accounts(cx);
        self.read_folders(cx);
    }

    /// The accounts, and with them the models offered (a sign-in changes both).
    fn read_accounts(&mut self, cx: &mut Context<Self>) {
        self.accounts_asked += 1;
        let asked = self.accounts_asked;
        self.run(
            |client| client.sign_in_accounts(),
            move |this, read, _| {
                // (Unless they've been asked for again since: the later answer is the one shown.)
                if asked != this.accounts_asked {
                    return;
                }
                // An account just signed in to that couldn't be read again says so, rather than
                // offering to sign in as if nothing had happened.
                if let Err(error) = &read {
                    for key in this.checking.clone() {
                        this.problems.insert(key, format!("Its account couldn’t be read again. {}", said(error)));
                    }
                }
                this.checking.clear();
                this.accounts = keeping(read, &this.accounts);
            },
            cx,
        );
        self.run(|client| client.offered_models(None, None), |this, read, _| this.offered = keeping(read, &this.offered), cx);
        self.ready_again = 0;
        self.read_ready(cx);
    }

    /// Asks the box whether there's anything to ask about your accounts. While it says a side
    /// can't tell yet (`pending`: starting, or still listing an agent's models), it's asked again
    /// in a moment, a few times: the welcome waits that long before it goes on.
    fn read_ready(&mut self, cx: &mut Context<Self>) {
        self.ready_asked += 1;
        let asked = self.ready_asked;
        self.run(
            |client| client.first_run_ready(),
            move |this, read, cx| {
                // (The latest asked for is the one gone by. One it couldn't say is not ready,
                // never what it said before a change.)
                if asked != this.ready_asked {
                    return;
                }
                let pending = read.as_ref().is_ok_and(|said| !said.ready && !said.pending.is_empty());
                this.ready = match read {
                    Ok(said) => Reading::Ready(said.ready),
                    Err(error) => Reading::Failed(said(&error)),
                };
                if pending && this.live && this.ready_again < READY_AGAIN {
                    this.ready_again += 1;
                    this.waits_for_ready = true;
                    let again = cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(READY_PAUSE).await;
                        this.update(cx, |this, cx| {
                            if asked == this.ready_asked {
                                this.read_ready(cx);
                            }
                        })
                        .ok();
                    });
                    this.tasks.push(again);
                } else {
                    this.waits_for_ready = false;
                }
            },
            cx,
        );
    }

    fn read_folders(&mut self, cx: &mut Context<Self>) {
        self.run(
            |client| client.suggested_folders(),
            |this, read, _| {
                this.folders = keeping(read, &this.folders);
                // The folder worked in latest is picked to begin with; with none suggested, the
                // choice is left to make.
                if this.place.is_none()
                    && let Reading::Ready(suggested) = &this.folders
                    && let Some(first) = suggested.folders.first()
                {
                    this.place = Some(StartIn::Project(first.path.clone()));
                }
            },
            cx,
        );
    }

    // MARK: The box's facts, as the steps list them

    /// The plans the first run lists: the coding agents' accounts, then the engine's plans, each
    /// as the box says it (`plan`), in its order; an account you added beside an agent's own is
    /// left to Settings (`added`), so each agent has one row.
    pub fn plans(&self) -> Vec<&AccountEntry> {
        let Reading::Ready(accounts) = &self.accounts else { return Vec::new() };
        accounts.agents.iter().chain(&accounts.engine).filter(|entry| entry.plan && !entry.added).collect()
    }

    /// The accounts a key (or the computer's credentials) is entered for, kept apart.
    pub fn keys(&self) -> Vec<&AccountEntry> {
        let Reading::Ready(accounts) = &self.accounts else { return Vec::new() };
        accounts.agents.iter().chain(&accounts.engine).filter(|entry| !entry.plan && !entry.added).collect()
    }

    /// The models that need no account, as the box offers them.
    fn free_models(&self) -> Vec<&OfferedModel> {
        match &self.offered {
            Reading::Ready(offered) => offered.models.iter().filter(|model| model.free && model.is_offered()).collect(),
            _ => Vec::new(),
        }
    }

    /// Whether the box says a first run has nothing to ask about your accounts (`kuvo/setup/ready`:
    /// a model on one of them runs already and no plan needs you). Then the welcome goes straight
    /// on to where to work. The box decides; one that couldn't say isn't taken as ready.
    pub fn ready(&self) -> bool {
        matches!(self.ready, Reading::Ready(true))
    }

    /// Your own computers, the box's first (one that isn't yours is left out).
    fn own_computers(&self) -> Vec<&Computer> {
        match &self.computers {
            Reading::Ready(computers) => computers.iter().filter(|computer| !computer.apart).collect(),
            _ => Vec::new(),
        }
    }

    /// The computer picked, where it isn't the box's own.
    fn elsewhere(&self) -> Option<&Computer> {
        let key = self.computer.as_ref()?;
        self.own_computers().into_iter().find(|computer| &computer.key == key && !computer.here)
    }

    /// The computer's name as the steps say it: "This Mac" for this Mac's own.
    fn computer_name(&self) -> SharedString {
        match self.elsewhere() {
            Some(computer) => computer.name.clone().into(),
            None => self.box_name.clone(),
        }
    }

    /// Whether the step's way on can be taken now.
    pub fn can_go_on(&self, cx: &App) -> bool {
        if self.client.gate().is_held() {
            return false;
        }
        match self.step {
            // (Once the box has said its accounts and models, or that it can't: whether the next
            // step is passed over goes by them.)
            Step::Welcome => self.box_state == BoxState::Connected && !self.settling(),
            Step::Agents | Step::Keys => true,
            // (Once nothing is asked about: else Not Now is the way on.)
            Step::Allow => !self.permissions.as_ref().is_some_and(|permissions| permissions.read(cx).asks()),
            Step::Where => self.place.is_some() && self.elsewhere().is_none(),
            Step::Browse => matches!(self.browsing, Some((_, Listed::Ready(_)))),
        }
    }

    // MARK: Moving through it

    fn show(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        self.allow_shown |= step == Step::Allow;
        self.wants_focus = true;
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        cx.notify();
    }

    /// The step's way on (its button, or Return).
    pub fn go_on(&mut self, cx: &mut Context<Self>) {
        if !self.can_go_on(cx) {
            return;
        }
        match self.step {
            // Straight on past the agents, where the box says a model runs and nothing needs you.
            Step::Welcome => {
                let next = if self.ready() { self.after_agents(cx) } else { Step::Agents };
                self.show(next, cx)
            }
            Step::Agents => {
                self.close_sign_in(cx);
                let next = self.after_agents(cx);
                self.show(next, cx);
            }
            Step::Allow => self.show(Step::Where, cx),
            Step::Keys => {
                self.close_sign_in(cx);
                self.show(Step::Agents, cx);
            }
            Step::Where => {
                if let Some(place) = self.place.clone() {
                    self.close_sign_in(cx);
                    cx.emit(Finished { place });
                }
            }
            Step::Browse => self.use_browsed(cx),
        }
    }

    /// Back a step (its button, or Escape).
    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.client.gate().is_held() {
            return;
        }
        match self.step {
            Step::Welcome => {}
            Step::Agents => {
                self.close_sign_in(cx);
                self.show(Step::Welcome, cx);
            }
            Step::Keys => {
                self.close_sign_in(cx);
                self.show(Step::Agents, cx);
            }
            Step::Allow => self.show(Step::Agents, cx),
            Step::Where => {
                let back = if self.allow_shown { Step::Allow } else { Step::Agents };
                self.show(back, cx)
            }
            Step::Browse => {
                self.browsing = None;
                self.show(Step::Where, cx);
            }
        }
    }

    /// The accounts a key is entered for, in the plans' place.
    /// Not Now on the permissions' step: what it asked about is put off (the box keeps that), and
    /// on to where to work.
    pub fn not_now(&mut self, cx: &mut Context<Self>) {
        if self.step != Step::Allow {
            return;
        }
        if let Some(permissions) = self.permissions.clone() {
            permissions.update(cx, |permissions, cx| permissions.not_now(cx));
        }
        self.show(Step::Where, cx);
    }

    pub fn show_keys(&mut self, cx: &mut Context<Self>) {
        self.close_sign_in(cx);
        self.show(Step::Keys, cx);
    }

    // MARK: Accounts

    fn entry(&self, key: &str) -> Option<&AccountEntry> {
        let Reading::Ready(accounts) = &self.accounts else { return None };
        accounts.agents.iter().chain(&accounts.engine).find(|entry| key_of(entry) == key)
    }

    /// What an account's row offers: the box's first action that signs in or installs, where this
    /// app can do it from where it runs.
    fn offer(&self, entry: &AccountEntry) -> Option<kuvo_box::accounts::Action> {
        entry.actions.iter().find(|action| (action.signs_in() || action.id == "install") && action.offered(self.client.here())).cloned()
    }

    /// An account's button pressed: its agent installed, or its sign-in opened in place.
    pub fn act(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(key).cloned() else { return };
        let Some(action) = self.offer(&entry) else { return };
        self.problems.remove(key);
        if action.id == "install" {
            self.install(&entry, cx);
        } else {
            self.open_sign_in(&entry, cx);
        }
        cx.notify();
    }

    fn install(&mut self, entry: &AccountEntry, cx: &mut Context<Self>) {
        let key = key_of(entry);
        if !self.installing.insert(key.clone()) {
            return;
        }
        let (account, name) = (entry.integration.id.clone(), entry.integration.name.clone());
        self.run(
            // Installed, its accounts are read again in the same piece of work: what the box says
            // of it then is what the next step goes by (before it, it offered only Install).
            move |client| client.install_agent(&account).map(|installed| {
                let read = installed.installed.then(|| client.sign_in_accounts());
                (installed, read)
            }),
            move |this, done, cx| {
                this.installing.remove(&key);
                match done {
                    Ok((installed, read)) if installed.installed => {
                        // (A read asked for before the install ended is older than this one.)
                        this.accounts_asked += 1;
                        this.checking.clear();
                        // (What's installed may be what was wanted: asked of the box again.)
                        this.read_ready(cx);
                        match read {
                            Some(Ok(accounts)) => this.accounts = Reading::Ready(accounts),
                            Some(Err(error)) => drop(this.problems.insert(key.clone(), format!("{name} is installed, but its account couldn’t be read again. {}", said(&error)))),
                            None => {}
                        }
                        // Installed: on to signing in, where its box now offers it, no other
                        // sign-in is under way, and its row is still on screen (a page opened in
                        // the browser from a step that doesn't show why would only confuse).
                        if this.sign_in.is_none()
                            && matches!(this.step, Step::Agents | Step::Keys)
                            && let Some(entry) = this.entry(&key).cloned()
                            && this.offer(&entry).is_some_and(|action| action.signs_in())
                        {
                            this.open_sign_in(&entry, cx);
                        }
                    }
                    Ok((installed, _)) => drop(this.problems.insert(key.clone(), format!("{name} couldn’t be installed. {}", installed.message.unwrap_or_else(|| "It isn’t installed.".into())))),
                    Err(error) => drop(this.problems.insert(key.clone(), format!("{name} couldn’t be installed. {}", said(&error)))),
                }
            },
            cx,
        );
    }

    /// Opens an account's sign-in under its row (one at a time: another under way is given up).
    fn open_sign_in(&mut self, entry: &AccountEntry, cx: &mut Context<Self>) {
        self.close_sign_in(cx);
        let key = key_of(entry);
        let sheet = cx.new(|cx| SignIn::new(self.client.clone(), self.live, entry.side, &entry.integration.id, &entry.integration.name, cx).in_place());
        self.sign_in_watch = vec![
            // Signed in: its row says so (the accounts are read again as it closes).
            cx.observe(&sheet, |_: &mut FirstRun, sheet, cx| {
                if sheet.read(cx).step() == &sign_in::Step::Done {
                    sheet.update(cx, |sheet, cx| sheet.close(cx));
                }
            }),
            cx.subscribe(&sheet, |this: &mut FirstRun, sheet, closed: &sign_in::Closed, cx| {
                if let Some((key, _)) = this.sign_in.as_ref().filter(|(_, open)| *open == sheet) {
                    // Signed in: its row waits for the box's word on it, offering nothing.
                    if closed.signed_in {
                        this.checking.insert(key.clone());
                    }
                    this.sign_in = None;
                    this.sign_in_watch.clear();
                    this.wants_focus = true;
                }
                // However it ended (a key may have gone in as it closed): read again.
                this.read_accounts(cx);
                cx.notify();
            }),
        ];
        sheet.update(cx, |sheet, cx| sheet.start(cx));
        self.sign_in = Some((key, sheet));
        cx.notify();
    }

    /// Draws an account as its agent's installer runs (shots, which run no clock).
    #[cfg(feature = "stand-in")]
    pub fn show_installing(&mut self, key: &str, cx: &mut Context<Self>) {
        self.installing.insert(key.to_owned());
        cx.notify();
    }

    /// Gives up the sign-in under way, on its engine too.
    pub fn close_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some((_, sheet)) = self.sign_in.clone() {
            sheet.update(cx, |sheet, cx| sheet.close(cx));
        }
    }

    // MARK: What macOS allows

    fn allow_step(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(permissions) = self.permissions.clone() else { return Vec::new() };
        let (title, line) = permissions.read(cx).heading();
        let asks = permissions.read(cx).asks();
        let foot = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(space::S2)
            .pt(space::S1)
            .child(Button::new("first-run-back", "Back").icon(IconName::ArrowLeft).ghost().on_click(cx.listener(|this, _, _, cx| this.go_back(cx))))
            .child(div().flex_1())
            .when(asks, |d| d.child(Button::new("first-run-not-now", "Not Now").ghost().on_click(cx.listener(|this, _, _, cx| this.not_now(cx)))))
            .child(Button::new("first-run-next", "Continue").primary().shortcut("↵").size(ButtonSize::Medium).disabled(!self.can_go_on(cx)).on_click(cx.listener(|this, _, _, cx| this.go_on(cx))))
            .into_any_element();
        vec![heading(&title, Some(&line), cx), self.body(vec![permissions.into_any_element()]), foot]
    }

    // MARK: Where to work

    pub fn pick_computer(&mut self, key: &str, cx: &mut Context<Self>) {
        let here = self.own_computers().into_iter().any(|computer| computer.key == key && computer.here);
        self.computer = (!here).then(|| key.to_owned());
        cx.notify();
    }

    pub fn pick_place(&mut self, place: StartIn, cx: &mut Context<Self>) {
        self.place = Some(place);
        cx.notify();
    }

    /// Looks through the computer's folders, from `path` (none: its home folder).
    pub fn browse(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let path = path.unwrap_or_else(|| match &self.folders {
            Reading::Ready(suggested) if !suggested.home.is_empty() => suggested.home.clone(),
            _ => "~/".into(),
        });
        // (The folder this one is opened from, by the box's own name for it.)
        self.browsed_from = match &self.browsing {
            Some((_, Listed::Ready(folder))) if folder.path != path => Some(folder.path.clone()),
            _ => None,
        };
        self.browsing = Some((path.clone(), Listed::Reading));
        if self.step != Step::Browse {
            self.show(Step::Browse, cx);
        }
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        let asked = path.clone();
        self.run(
            move |client| client.folders_in(&asked),
            move |this, read, _| {
                // (Unless another folder's been opened meanwhile.)
                let Some((at, listed)) = &mut this.browsing else { return };
                if *at != path {
                    return;
                }
                *listed = match read {
                    Ok(folder) => {
                        // As the box names it (`~/` read as the home folder's own path).
                        if !folder.path.is_empty() {
                            *at = folder.path.clone();
                        }
                        Listed::Ready(folder)
                    }
                    Err(Ok(unreadable)) => Listed::Unreadable(unreadable),
                    Err(Err(error)) => Listed::Failed(said(&error)),
                };
            },
            cx,
        );
        cx.notify();
    }

    /// The folder looked at is the one to start in.
    fn use_browsed(&mut self, cx: &mut Context<Self>) {
        let Some((path, Listed::Ready(_))) = self.browsing.clone() else { return };
        self.browsing = None;
        let suggested = matches!(&self.folders, Reading::Ready(suggested) if suggested.folders.iter().any(|folder| folder.path == path));
        self.chosen = (!suggested).then(|| path.clone());
        self.place = Some(StartIn::Project(path));
        self.show(Step::Where, cx);
    }
}

/// What a read makes of what's shown: what it read, or (failing) what was there, else why.
fn keeping<T: Clone>(read: kuvo_box::Result<T>, before: &Reading<T>) -> Reading<T> {
    match (read, before) {
        (Ok(value), _) => Reading::Ready(value),
        (Err(_), Reading::Ready(value)) => Reading::Ready(value.clone()),
        (Err(error), _) => Reading::Failed(said(&error)),
    }
}

impl Focusable for FirstRun {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// MARK: Drawing

/// An account's mark: the box's `service`, else its name's first letter.
fn mark(service: Option<&str>, name: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let size = px(32.);
    let tile = div().flex().flex_none().items_center().justify_center().size(size).rounded(radius::PILL).bg(c.sunken).border_1().border_color(c.border);
    match service.and_then(kuvo_ui::icon::mark_path) {
        Some(path) => tile.child(svg().path(path).size(px(17.)).text_color(service.and_then(kuvo_ui::icon::mark_color).unwrap_or(c.text))).into_any_element(),
        None => {
            let letter = name.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default();
            tile.text_ui(TypeSize::Small).medium().text_color(c.text_muted).child(letter).into_any_element()
        }
    }
}

/// A round tile with an icon, where a row has no mark of its own.
pub(crate) fn icon_tile(icon: IconName, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(radius::PILL).bg(c.sunken).border_1().border_color(c.border).child(Icon::new(icon).color(c.icon)).into_any_element()
}

/// Rows in a list, divided by hairlines.
pub(crate) fn list(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let mut card = div().flex().flex_col().flex_none().rounded(radius::POPOVER).border_1().border_color(c.border).bg(c.card).overflow_hidden();
    for (ix, row) in rows.into_iter().enumerate() {
        card = card.child(div().when(ix > 0, |d| d.border_t_1().border_color(c.border)).child(row));
    }
    card.into_any_element()
}

/// A step's title, and the line under it.
pub(crate) fn heading(title: &str, line: Option<&str>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(space::S1)
        .child(div().debug_selector(|| "first-run-title".into()).text_ui(TypeSize::Title).medium().text_color(c.text).child(title.to_owned()))
        .children(line.map(|line| div().text_ui(TypeSize::Body).text_color(c.text_muted).child(line.to_owned())))
        .into_any_element()
}

/// A small label over a group.
fn label(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div().px(space::S1).text_ui(TypeSize::Caption).medium().text_color(c.text_faint).child(text.into()).into_any_element()
}

/// A line saying what's being waited for.
fn waiting(id: &'static str, words: &str, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div().debug_selector(move || id.into()).flex().items_center().gap(space::S2).py(space::S2).text_ui(TypeSize::Small).text_color(c.text_muted).child(spinner(id, IconSize::Nav, c.text_muted)).child(words.to_owned()).into_any_element()
}

/// A title over a detail, as every row here has them.
pub(crate) fn two_lines(title: impl Into<SharedString>, detail: impl Into<SharedString>, detail_color: gpui::Hsla, cx: &App) -> gpui::Div {
    let c = &cx.theme().colors;
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).truncate().child(title.into()))
        .child(div().text_ui(TypeSize::Caption).text_color(detail_color).truncate().child(detail.into()))
}

impl FirstRun {
    /// Something that couldn't be read, in the box's words, with a way to ask again.
    fn failed(&self, id: &'static str, why: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div()
            .debug_selector(move || id.into())
            .flex()
            .items_center()
            .gap(space::S3)
            .py(space::S2)
            .child(div().flex_1().min_w_0().text_ui(TypeSize::Small).text_color(c.danger).child(why.to_owned()))
            .child(Button::new(SharedString::from(format!("{id}-again")), "Try Again").size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.read(cx))))
            .into_any_element()
    }

    /// Back on the left, the step's way on at the right.
    fn foot(&self, back: bool, next: &'static str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .pt(space::S1)
            .when(back, |d| d.child(Button::new("first-run-back", "Back").icon(IconName::ArrowLeft).ghost().on_click(cx.listener(|this, _, _, cx| this.go_back(cx)))))
            .child(div().flex_1())
            .child(Button::new("first-run-next", next).primary().shortcut("↵").size(ButtonSize::Medium).loading(self.settling()).disabled(!self.can_go_on(cx)).on_click(cx.listener(|this, _, _, cx| this.go_on(cx))))
            .into_any_element()
    }

    /// Whether the welcome still waits for the box's accounts and models before it can go on.
    fn settling(&self) -> bool {
        self.step == Step::Welcome && self.box_state == BoxState::Connected && !self.client.gate().is_held() && (matches!(self.accounts, Reading::NotYet) || matches!(self.offered, Reading::NotYet) || matches!(self.ready, Reading::NotYet) || self.waits_for_ready)
    }

    /// The scrolling middle of a card: what a step lists, however long.
    fn body(&self, children: Vec<AnyElement>) -> AnyElement {
        div().id("first-run-body").debug_selector(|| "first-run-body".into()).min_h_0().flex().flex_col().gap(space::S4).overflow_y_scroll().track_scroll(&self.scroll).children(children).into_any_element()
    }

    // MARK: Welcome

    fn welcome(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        // The box beside the app: reached, being reached, or why not.
        let (line, color): (SharedString, _) = match &self.box_state {
            BoxState::Connected => ("Ready".into(), c.text_muted),
            BoxState::Connecting => ("Starting…".into(), c.text_muted),
            BoxState::Held => ("Needs an update".into(), c.warning),
            BoxState::Unreachable(why) => (why.clone().into(), c.danger),
        };
        let trailing: AnyElement = match &self.box_state {
            BoxState::Connected => Icon::new(IconName::Check).color(c.success).into_any_element(),
            BoxState::Connecting => spinner("first-run-starting", IconSize::Nav, c.text_muted).into_any_element(),
            _ => Icon::new(IconName::Warning).color(color).into_any_element(),
        };
        let mut rows = vec![
            div()
                .debug_selector(|| "first-run-computer".into())
                .flex()
                .items_center()
                .gap(space::S3)
                .px(space::S4)
                .py(space::S3)
                .child(icon_tile(IconName::Computer, cx))
                .child(two_lines(self.box_name.clone(), line, color, cx))
                .child(trailing)
                .into_any_element(),
        ];
        // What's signed in already, where that's all a session needs: the next step is skipped.
        let signed_in: Vec<&AccountEntry> = self.plans().into_iter().chain(self.keys()).filter(|entry| entry.state == State::SignedIn).collect();
        if self.ready() && !signed_in.is_empty() {
            let names = signed_in.iter().map(|entry| entry.integration.name.clone()).collect::<Vec<_>>().join(", ");
            let first = signed_in.first().map(|entry| mark(entry.service.as_deref(), &entry.integration.name, cx));
            rows.push(
                div()
                    .debug_selector(|| "first-run-signed-in".into())
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S4)
                    .py(space::S3)
                    .children(first)
                    .child(two_lines(names, "Ready to use", c.text_muted, cx))
                    .child(Icon::new(IconName::Check).color(c.success))
                    .into_any_element(),
            );
        }
        vec![heading("Welcome to Kuvo Code", None, cx), self.body(vec![list(rows, cx)]), self.foot(false, "Get Started", cx)]
    }

    // MARK: Agents and plans

    /// What an account's row ends in: a check once it's signed in; the box's first action that
    /// signs in or installs, in its words; "Installing…" while its installer runs; Cancel while
    /// its sign-in is open under it.
    fn row_end(&self, entry: &AccountEntry, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let key = key_of(entry);
        if self.installing.contains(&key) {
            return div()
                .debug_selector({
                    let key = key.clone();
                    move || format!("first-run-{key}-installing")
                })
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1_5)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(spinner(SharedString::from(format!("installing-{key}")), IconSize::Nav, c.text_muted))
                .child("Installing…")
                .into_any_element();
        }
        if self.sign_in.as_ref().is_some_and(|(open, _)| *open == key) {
            return Button::new(SharedString::from(format!("first-run-{key}-cancel")), "Cancel").size(ButtonSize::Small).ghost().on_click(cx.listener(|this, _, _, cx| this.close_sign_in(cx))).into_any_element();
        }
        if self.checking.contains(&key) && entry.state != State::SignedIn {
            return div()
                .debug_selector({
                    let key = key.clone();
                    move || format!("first-run-{key}-checking")
                })
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1_5)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(spinner(SharedString::from(format!("checking-{key}")), IconSize::Nav, c.text_muted))
                .child("Checking…")
                .into_any_element();
        }
        if entry.state == State::SignedIn {
            return div()
                .debug_selector({
                    let key = key.clone();
                    move || format!("first-run-{key}-signed-in")
                })
                .child(Icon::new(IconName::Check).color(c.success))
                .into_any_element();
        }
        match self.offer(entry) {
            Some(action) => Button::new(SharedString::from(format!("first-run-{key}-act")), action.label.clone())
                .size(ButtonSize::Small)
                .on_click(cx.listener(move |this, _, _, cx| this.act(&key, cx)))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// An account's row: its mark, its name, the box's line (a warning where it needs you), what
    /// it offers, and under it its sign-in while that's open, or what went wrong installing it.
    fn account_row(&self, entry: &AccountEntry, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let key = key_of(entry);
        let line_color = if entry.state.needs_you() { c.warning } else { c.text_muted };
        let open = self.sign_in.as_ref().filter(|(open, _)| *open == key).map(|(_, sheet)| sheet.clone());
        let problem = self.problems.get(&key).cloned();
        let selector = format!("first-run-{key}");
        div()
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S4)
                    .py(space::S3)
                    .child(mark(entry.service.as_deref(), &entry.integration.name, cx))
                    .child(two_lines(entry.integration.name.clone(), entry.line.clone(), line_color, cx))
                    .child(self.row_end(entry, cx)),
            )
            .children(open.map(|sheet| div().ml(px(60.)).mr(space::S4).mb(space::S3).p(space::S3).rounded(radius::NAV).bg(c.sunken).border_1().border_color(c.border).child(sheet)))
            .children(problem.map(|why| {
                let selector = format!("first-run-{key}-problem");
                div().debug_selector(move || selector).ml(px(60.)).mr(space::S4).mb(space::S3).text_ui(TypeSize::Caption).text_color(c.danger).child(why)
            }))
            .into_any_element()
    }

    /// The models that need no account: always there, so nothing has to be signed in to go on.
    fn free_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.theme().colors.clone();
        let free = self.free_models();
        if free.is_empty() {
            return None;
        }
        let names = free.iter().map(|model| model.name.clone()).collect::<Vec<_>>().join(", ");
        Some(
            div()
                .debug_selector(|| "first-run-free".into())
                .flex()
                .items_center()
                .gap(space::S3)
                .px(space::S4)
                .py(space::S3)
                .child(icon_tile(IconName::Sparkle, cx))
                .child(two_lines("Free models", format!("No account needed · {names}"), c.text_muted, cx))
                .child(Icon::new(IconName::Check).color(c.success))
                .into_any_element(),
        )
    }

    /// Why a side of the box couldn't list its accounts just now, in its words.
    fn down(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let Reading::Ready(accounts) = &self.accounts else { return Vec::new() };
        [&accounts.agents_down, &accounts.engine_down]
            .into_iter()
            .flatten()
            .map(|why| div().debug_selector(|| "first-run-down".into()).flex().items_center().gap(space::S2).text_ui(TypeSize::Small).text_color(c.warning).child(Icon::new(IconName::Warning).color(c.warning)).child(why.clone()).into_any_element())
            .collect()
    }

    fn agents(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut body = self.down(cx);
        match &self.accounts {
            Reading::NotYet => body.push(waiting("first-run-accounts-reading", "Looking for your coding agents…", cx)),
            Reading::Failed(why) => body.push(self.failed("first-run-accounts-failed", &why.clone(), cx)),
            Reading::Ready(_) => {
                let plans: Vec<AccountEntry> = self.plans().into_iter().cloned().collect();
                let mut rows: Vec<AnyElement> = plans.iter().map(|entry| self.account_row(entry, cx)).collect();
                rows.extend(self.free_row(cx));
                if !rows.is_empty() {
                    body.push(list(rows, cx));
                }
                if !self.keys().is_empty() {
                    // In a row of its own, so the column doesn't stretch it.
                    body.push(div().flex().flex_none().ml(-space::S2).child(Button::new("first-run-keys", "Use an API Key Instead…").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.show_keys(cx)))).into_any_element());
                }
            }
        }
        vec![
            heading("Agents and plans", Some("Sign in to the ones you use. You can change them later in Settings."), cx),
            self.body(body),
            self.foot(true, "Continue", cx),
        ]
    }

    fn keys_step(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let keys: Vec<AccountEntry> = self.keys().into_iter().cloned().collect();
        let rows: Vec<AnyElement> = keys.iter().map(|entry| self.account_row(entry, cx)).collect();
        let mut body = self.down(cx);
        body.push(list(rows, cx));
        vec![heading("API keys", Some(&format!("A key stays on {}.", self.box_name)), cx), self.body(body), self.foot(true, "Done", cx)]
    }

    // MARK: Where to work

    /// A computer to work on, to pick: selected is the theme's own wash.
    fn computer_tile(&self, computer: &Computer, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let selected = if computer.here { self.elsewhere().is_none() } else { self.computer.as_deref() == Some(computer.key.as_str()) };
        let name: SharedString = if computer.here { self.box_name.clone() } else { computer.name.clone().into() };
        let line = if computer.here { computer.system.clone().unwrap_or_else(|| computer.name.clone()) } else { crate::areas::computer_line(computer) };
        let key = computer.key.clone();
        let id = if computer.here { "this".to_owned() } else { computer.key.clone() };
        let selector = format!("first-run-computer-{id}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            // Two to a row, where there's more than one.
            .w(gpui::relative(0.5))
            .p(space::S1)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S3)
                    .py(space::S3)
                    .rounded(radius::POPOVER)
                    .border_1()
                    .border_color(c.border)
                    .bg(if selected { c.selected } else { c.card })
                    .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
                    .cursor_pointer()
                    .child(computers::tile(computer, px(32.), cx))
                    .child(two_lines(name, line, c.text_muted, cx))
                    .when(selected, |d| d.child(Icon::new(IconName::Check).color(c.text))),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.pick_computer(&key, cx)))
            .into_any_element()
    }

    /// A place to start in, to pick: a folder, or no project at all.
    fn place_row(&self, id: String, icon: IconName, title: String, detail: String, place: StartIn, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let selected = self.place.as_ref() == Some(&place);
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(space::S3)
            .px(space::S4)
            .py(px(10.))
            .when(selected, |d| d.bg(c.selected))
            .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
            .cursor_pointer()
            .child(Icon::new(icon).color(c.icon))
            .child(two_lines(title, detail, c.text_muted, cx))
            .when(selected, |d| d.child(Icon::new(IconName::Check).color(c.text)))
            .on_click(cx.listener(move |this, _, _, cx| this.pick_place(place.clone(), cx)))
            .into_any_element()
    }

    fn where_step(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let mut body = Vec::new();
        // The computers, where there's more than this one to pick from.
        let own: Vec<Computer> = self.own_computers().into_iter().cloned().collect();
        if own.len() > 1 {
            body.push(div().flex().flex_none().flex_wrap().m(-space::S1).children(own.iter().map(|computer| self.computer_tile(computer, cx))).into_any_element());
        }
        if let Some(computer) = self.elsewhere().cloned() {
            // Said plainly: this app shows one computer's box, and switching isn't built yet.
            body.push(
                div()
                    .debug_selector(|| "first-run-elsewhere".into())
                    .flex()
                    .flex_none()
                    .gap(space::S3)
                    .p(space::S4)
                    .rounded(radius::POPOVER)
                    .border_1()
                    .border_color(c.border)
                    .bg(c.card)
                    .child(Icon::new(IconName::Info).color(c.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(space::S1)
                            .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(format!("Working on {} isn’t in the desktop app yet", computer.name)))
                            .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(format!("It stays in your computers, and its sessions keep running there. Pick {} to go on.", self.box_name))),
                    )
                    .into_any_element(),
            );
        } else {
            body.push(label(format!("Folders on {}", self.computer_name()), cx));
            match &self.folders {
                Reading::NotYet => body.push(waiting("first-run-folders-reading", "Looking for your projects…", cx)),
                Reading::Failed(why) => body.push(self.failed("first-run-folders-failed", &why.clone(), cx)),
                Reading::Ready(_) => {}
            }
            let (home, suggested) = match &self.folders {
                Reading::Ready(suggested) => (suggested.home.clone(), suggested.folders.clone()),
                _ => (String::new(), Vec::new()),
            };
            let now = self.now.unwrap_or_else(time::now_ms);
            let mut rows = Vec::new();
            // A folder you chose yourself, first.
            if let Some(path) = self.chosen.clone() {
                rows.push(self.place_row("first-run-folder-chosen".into(), IconName::Folder, name_of(&path), from_home(&path, &home), StartIn::Project(path), cx));
            }
            for (ix, folder) in suggested.iter().enumerate() {
                let path = from_home(&folder.path, &home);
                let detail = match folder.seen {
                    Some(seen) => match time::ago(seen as f64, now).as_str() {
                        "now" => format!("{path} · just now"),
                        ago => format!("{path} · {ago} ago"),
                    },
                    None => path,
                };
                rows.push(self.place_row(format!("first-run-folder-{ix}"), if folder.repository { IconName::Branch } else { IconName::Folder }, folder.name.clone(), detail, StartIn::Project(folder.path.clone()), cx));
            }
            if matches!(self.folders, Reading::Ready(_)) && rows.is_empty() {
                body.push(div().debug_selector(|| "first-run-no-folders".into()).px(space::S1).text_ui(TypeSize::Small).text_color(c.text_muted).child("No projects found here yet. Choose a folder to work in, or start with no project.").into_any_element());
            }
            rows.push(self.place_row("first-run-no-project".into(), IconName::Message, "No project".into(), "Just chat, in no folder".into(), StartIn::Chat, cx));
            body.push(list(rows, cx));
            body.push(div().flex().flex_none().ml(-space::S2).child(Button::new("first-run-choose", "Choose a Folder…").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.browse(None, cx)))).into_any_element());
        }
        vec![heading("Where to work", None, cx), self.body(body), self.foot(true, "Continue", cx)]
    }

    fn browse_step(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let Some((path, listed)) = self.browsing.clone() else { return Vec::new() };
        let home = match &self.folders {
            Reading::Ready(suggested) => suggested.home.clone(),
            _ => String::new(),
        };
        // Up: the folder the box says this one is in; for one it couldn't list, back to where
        // it was opened from.
        let up = match &listed {
            Listed::Ready(folder) => folder.parent.clone().filter(|parent| *parent != folder.path),
            _ => self.browsed_from.clone(),
        };
        let mut body = vec![
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S2)
                .child(
                    Button::icon_only("first-run-up", IconName::ArrowUp, "The folder this is in")
                        .ghost()
                        .size(ButtonSize::Small)
                        .tooltip("The folder this is in")
                        .disabled(up.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| this.browse(up.clone(), cx))),
                )
                .child(Icon::new(IconName::Folder).color(c.icon))
                .child(div().debug_selector(|| "first-run-browse-path".into()).flex_1().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(from_home(&path, &home)))
                .into_any_element(),
        ];
        match listed {
            Listed::Reading => body.push(waiting("first-run-browse-reading", "Reading this folder…", cx)),
            Listed::Failed(why) => body.push(div().debug_selector(|| "first-run-browse-failed".into()).text_ui(TypeSize::Small).text_color(c.danger).child(why).into_any_element()),
            Listed::Unreadable(unreadable) => body.push(
                div()
                    .debug_selector(|| "first-run-browse-unreadable".into())
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .child(div().text_ui(TypeSize::Small).text_color(c.text).child(unreadable.message))
                    .children(unreadable.reason.map(|reason| div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(reason)))
                    .into_any_element(),
            ),
            Listed::Ready(folder) if folder.folders.is_empty() => body.push(div().debug_selector(|| "first-run-browse-empty".into()).px(space::S1).text_ui(TypeSize::Small).text_color(c.text_muted).child("No folders in here.").into_any_element()),
            Listed::Ready(folder) => {
                let rows = folder
                    .folders
                    .iter()
                    .enumerate()
                    .map(|(ix, named)| {
                        let inside = format!("{}/{}", folder.path.trim_end_matches('/'), named.name);
                        let selector = format!("first-run-browse-{}", named.name);
                        div()
                            .id(("first-run-browse", ix))
                            .debug_selector(move || selector)
                            .flex()
                            .items_center()
                            .gap(space::S3)
                            .px(space::S4)
                            .py(space::S2)
                            .hover(|s| s.bg(c.hover))
                            .cursor_pointer()
                            .child(Icon::new(if named.repository { IconName::Branch } else { IconName::Folder }).color(c.icon))
                            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(named.name.clone()))
                            .child(Icon::new(IconName::ChevronRight).color(c.text_faint))
                            .on_click(cx.listener(move |this, _, _, cx| this.browse(Some(inside.clone()), cx)))
                            .into_any_element()
                    })
                    .collect();
                body.push(list(rows, cx));
            }
        }
        vec![heading("Choose a folder", Some(&format!("On {}.", self.computer_name())), cx), self.body(body), self.foot(true, "Use This Folder", cx)]
    }

    /// The box is older than this app needs: said in the window's own words, with nothing to go
    /// on to until it has updated.
    fn held(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let (title, next) = held_words(Some(self.box_name.as_ref()), self.client.here());
        vec![
            heading("Welcome to Kuvo Code", None, cx),
            div()
                .debug_selector(|| "first-run-held".into())
                .flex()
                .flex_none()
                .gap(space::S3)
                .p(space::S4)
                .rounded(radius::POPOVER)
                .border_1()
                .border_color(c.border)
                .bg(c.card)
                .child(Icon::new(IconName::Warning).color(c.warning))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(space::S1).child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(title)).child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(next)))
                .into_any_element(),
        ]
    }
}

/// Where you are in the steps: the current one longer.
pub(crate) fn step_dots(at: usize, of: usize, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .debug_selector(|| "first-run-dots".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(space::S1_5)
        .children((0..of).map(|ix| div().h(px(6.)).w(if ix == at { px(16.) } else { px(6.) }).rounded(radius::PILL).bg(if ix == at { c.text } else { c.border_strong })))
        .into_any_element()
}

impl Render for FirstRun {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.wants_focus) {
            window.focus(&self.focus, cx);
        }
        let held = self.client.gate().is_held();
        let card = if held {
            self.held(cx)
        } else {
            match self.step {
                Step::Welcome => self.welcome(cx),
                Step::Agents => self.agents(cx),
                Step::Keys => self.keys_step(cx),
                Step::Allow => self.allow_step(cx),
                Step::Where => self.where_step(cx),
                Step::Browse => self.browse_step(cx),
            }
        };
        let allows = self.allows(cx);
        let dot = if held { 0 } else { self.step.dot(allows) };
        framed(card, Some((dot, if allows { 4 } else { 3 })), cx)
            .id("first-run")
            .debug_selector(|| "first-run".into())
            .key_context("FirstRun")
            .track_focus(&self.focus)
            // Return goes on and Escape goes back, unless a field in the card has the keyboard.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !this.focus.is_focused(window) {
                    return;
                }
                match event.keystroke.key.as_str() {
                    // (With a sign-in open under a row, Return isn't the way on: what's typed
                    // there would be lost.)
                    "enter" if !event.keystroke.modifiers.modified() && this.sign_in.is_none() => this.go_on(cx),
                    "escape" => this.go_back(cx),
                    _ => {}
                }
            }))
            .on_action(cx.listener(|_, _: &ToggleAppearance, _, cx| {
                let next = cx.theme().appearance.toggled();
                kuvo_ui::Theme::set_appearance(next, cx);
            }))
    }
}

/// The first run's frame: the dotted backdrop, the window's own title bar (its buttons, and the
/// window moved by it), one card in the middle, and the steps' dots under it. Setting up this
/// Mac draws in it too ([`crate::launch`]), so the welcome that follows is the same card.
pub(crate) fn card_frame(card: Vec<AnyElement>, dot: usize, cx: &App) -> gpui::Div {
    framed(card, Some((dot, 3)), cx)
}

/// The same frame for a card that isn't one of the first run's steps (a move from an earlier
/// Kuvo Code, ahead of the Welcome): no dots under it.
pub(crate) fn card_alone(card: Vec<AnyElement>, cx: &App) -> gpui::Div {
    framed(card, None, cx)
}

/// `dot`: which of how many dots it's at, under the card (none: no dots).
fn framed(card: Vec<AnyElement>, dot: Option<(usize, usize)>, cx: &App) -> gpui::Div {
    let c = cx.theme().colors.clone();
    let width: Pixels = px(CARD);
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .bg(c.background)
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .child(backdrop())
        .child(div().id("first-run-title-bar").h(px(title_bar::HEIGHT)).flex_none().on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 1 {
                window.start_window_move();
            }
        }))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::S4)
                .px(space::S6)
                .pb(space::S10)
                .child(
                    div()
                        .debug_selector(|| "first-run-card".into())
                        .w_full()
                        .max_w(width)
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap(space::S5)
                        .p(space::S8)
                        .rounded(radius::CARD + px(4.))
                        .bg(c.surface)
                        .border_1()
                        .border_color(c.border)
                        .shadow(cx.theme().elevated_shadow())
                        .children(card),
                )
                .children(dot.map(|(at, of)| step_dots(at, of, cx))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_reads_from_the_home_folder() {
        assert_eq!(from_home("/Users/sam/Projects/api", "/Users/sam"), "~/Projects/api");
        assert_eq!(from_home("/Users/sam", "/Users/sam/"), "~");
        // (Another folder that only starts the same, and one outside it, read whole.)
        assert_eq!(from_home("/Users/samantha/api", "/Users/sam"), "/Users/samantha/api");
        assert_eq!(from_home("/srv/api", "/home/sam"), "/srv/api");
        assert_eq!(from_home("/srv/api", ""), "/srv/api");
    }

    #[test]
    fn a_folders_name_is_the_last_part_of_its_path() {
        assert_eq!(name_of("/Users/sam/Projects/"), "Projects");
    }
}
