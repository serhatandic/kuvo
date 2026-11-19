//! The window's root: your boxes ([`kuvo_box::boxes::Boxes`]) and a workspace for each computer
//! you've opened, one shown at a time. No box is the app's own: the one on this computer is only
//! the nearest, shown first; the title bar's switcher shows another's sessions, bots, queue and
//! settings instead, asked of that computer's own box. A workspace you leave stays as it was (its
//! open conversation, its drafts), so coming back is at once.
//!
//! It decides nothing: which computers are yours, how each stands and whether this app is paired
//! are `Boxes`' facts, drawn in the switcher.

use std::collections::HashMap;

use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task, Window, div};
use kuvo_ui::ActiveTheme as _;
use kuvo_box::BoxClient;
use kuvo_box::boxes::{Boxes, Pairing, Row};

use kuvo_box::install::{Installer, Progress, Ran};

use crate::notifications::{Effects, Notifier, Open};
use crate::workspace::{OwnUpdate, Switch, Workspace};

/// What a box says it has once it takes the release this app carries the moment its work ends,
/// and says how that stands itself (its own row's `update`): then this app only offers it.
pub const UPDATES_ITSELF: &str = "computers.update.here";

/// What the switcher draws: your computers with how this app stands with each, which one the
/// window shows, and how the app's pairing stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Switching {
    pub rows: Vec<Row>,
    /// The key of the computer shown (none: the nearest, before its box said its key).
    pub shown: Option<String>,
    pub pairing: Pairing,
}

pub struct Home {
    boxes: Boxes,
    live: bool,
    /// Each computer's workspace, by its key ("" for the nearest box's).
    workspaces: HashMap<String, (Entity<Workspace>, Subscription)>,
    /// Which client each computer's workspace was made with ([`Boxes::client_made`]).
    made: HashMap<String, u64>,
    shown: String,
    now: Option<f64>,
    /// This device's notifications and Dock count, from every computer it reaches.
    notifier: Entity<Notifier>,
    _watch: Option<Task<()>>,
    /// Whether this computer's own Kuvo shows in its menu bar (its box's `kuvo/box/at-work`
    /// `applies`), so quitting leaves it at work there: the first Quit's note says so. Not known
    /// (its box not reached yet): no note.
    in_menu_bar: Option<bool>,
    /// The newer Kuvo version on this Mac the app last looked for its own update for (`appCheck`).
    app_checked: Option<String>,
    _menu_bar: Option<Task<()>>,
    /// The first Quit's note, while it's shown (`quit_note`), and what had focus before it.
    quit_note: Option<FocusHandle>,
    focused_before: Option<FocusHandle>,
    _notifier: Subscription,
    /// The installer this app carries, which brings this Mac's own Kuvo up to its release where
    /// that box is from before it did so itself; how that stands; and its runs.
    installer: Option<Installer>,
    own_update: Option<OwnUpdate>,
    /// Asking this Mac's box whether this app brings it up; the run that waits for its work to end
    /// (one at a time, and whether it's still at work); and Update Now's.
    _own_asking: Option<Task<()>>,
    own_waiting: Option<Task<()>>,
    own_waits: bool,
    own_now: Option<Task<()>>,
    /// What the waiting run last said while it waits (its words, and Update Now's question).
    own_waited: Option<OwnUpdate>,
    /// The computer the window showed last time (`yours::Place`), shown again once your
    /// computers' list says whether it's reached; nothing's drawn meanwhile (not another
    /// computer's window first).
    restoring: Option<String>,
    window_handle: gpui::AnyWindowHandle,
    _restoring: Option<Task<()>>,
    /// A link to another computer (`kuvo-code://open`) that came before this app reaches it (as the
    /// app opens for it, or while it's being reached): shown once it is, waited for at most
    /// `RESTORE_WAITS_AT_MOST`, as the computer shown last time is.
    opening: Option<Open>,
    _opening: Option<Task<()>>,
}

/// How long the window waits for your computers' list before it opens on the nearest instead
/// of the computer it showed last time.
const RESTORE_WAITS_AT_MOST: Duration = Duration::from_secs(3);

/// How often this computer's box is asked again whether it's in the menu bar, while it hasn't said.
const MENU_BAR_ASKED_EVERY: Duration = Duration::from_secs(30);

/// The app bundle `executable` runs from (`<bundle>.app/Contents/MacOS/<executable>`); none for a
/// program in no bundle (`cargo run`).
fn own_bundle(executable: &std::path::Path) -> Option<std::path::PathBuf> {
    let contents = executable.parent().filter(|folder| folder.file_name().is_some_and(|name| name == "MacOS"))?.parent()?;
    let bundle = contents.parent()?;
    (contents.file_name()? == "Contents" && bundle.extension()? == "app" && bundle.is_absolute()).then(|| bundle.to_owned())
}

/// How often this Mac's box is asked again whether this app brings it up, while it can't be reached.
const OWN_UPDATE_ASKED_EVERY: Duration = Duration::from_secs(30);

/// The nearest box's workspace, whatever its key turns out to be.
const NEAREST: &str = "";

/// What's said while this Mac's own Kuvo waits for its work to end, where the box didn't say.
// COMPAT(0.8 beta update): an installer's run always says it.
const WAITS: &str = "Kuvo on this Mac updates once your sessions finish.";
/// What's said while it's being updated, where the box didn't say.
const UPDATING: &str = "Updating Kuvo on this Mac…";

impl Home {
    /// `first`: the box the window opens with (the nearest, or with none here, one of your
    /// computers the kept list names). `live`: whether it follows your boxes (the app), or shows
    /// what it's handed (tests and shots).
    pub fn new(boxes: Boxes, first: BoxClient, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Home {
        let shown = if boxes.nearest().is_some() { NEAREST.to_owned() } else { boxes.rows().iter().find(|row| row.usable()).map(|row| row.key().to_owned()).unwrap_or_default() };
        // (Only the app touches the Dock and makes sounds; tests and shots keep to the window.)
        let effects = if live { Effects::system() } else { Effects::none() };
        let notifier = cx.new(|cx| Notifier::new(effects, cx));
        let opens = cx.subscribe_in(&notifier, window, |this: &mut Home, _, open: &Open, window, cx| this.open_notice(open, window, cx));
        if live {
            let home = cx.entity().downgrade();
            cx.on_system_notification_response(move |response, cx| {
                let _ = home.update(cx, |home, cx| home.notifier.update(cx, |notifier, cx| notifier.clicked(&response.tag, cx)));
            });
        }
        let mut home = Home { boxes: boxes.clone(), live, workspaces: HashMap::new(), made: HashMap::new(), shown: shown.clone(), now: None, notifier, _watch: None, _notifier: opens, in_menu_bar: None, app_checked: None, _menu_bar: None, quit_note: None, focused_before: None, installer: None, own_update: None, _own_asking: None, own_waiting: None, own_waits: false, own_now: None, own_waited: None, restoring: None, window_handle: window.window_handle(), _restoring: None, opening: None, _opening: None };
        home.make(&shown, first, window, cx);
        // The computer shown last time, where it isn't this one: once the list says it's reached.
        home.restoring = crate::yours::place(cx).computer.filter(|key| key != &shown);
        if live && home.restoring.is_some() {
            home._restoring = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RESTORE_WAITS_AT_MOST).await;
                this.update(cx, |this, cx| {
                    this.restoring = None;
                    cx.notify();
                })
                .ok();
            }));
        }
        home.ask_menu_bar(cx);
        // Links opened in the app (`kuvo-code://open`: the menu bar's Kuvo Helper opens its
        // computers and their sessions so), shown as a notification clicked would show them,
        // those from before the window was there first.
        if live {
            let opened = crate::notifications::opened::receiver();
            cx.spawn_in(window, async move |this, cx| {
                while let Ok(open) = opened.recv().await {
                    if this.update_in(cx, |home, window, cx| home.open_notice(&open, window, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        // Closing the window quits the app (it has only the one): the first time, the note first.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| this.update(cx, |home, cx| !home.notes_quit(window, cx)).unwrap_or(true));
        if live {
            boxes.start();
            let changed = boxes.changed();
            home._watch = Some(cx.spawn(async move |this, cx| {
                while changed.recv().await.is_ok() {
                    if this.update(cx, |this, cx| this.rows_changed(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        home.rows_changed(cx);
        home
    }

    pub fn boxes(&self) -> &Boxes {
        &self.boxes
    }

    /// Asks this computer's own box whether it shows in the menu bar (off the window's thread for
    /// the app; at once against a stand-in), again every so often until it says.
    fn ask_menu_bar(&mut self, cx: &mut Context<Self>) {
        let Some(nearest) = self.boxes.nearest() else { return };
        if !self.live {
            self.in_menu_bar = nearest.at_work().ok().map(|said| said.applies);
            return;
        }
        self._menu_bar = Some(cx.spawn(async move |this, cx| {
            loop {
                let client = nearest.clone();
                if let Ok(said) = cx.background_executor().spawn(async move { client.at_work() }).await {
                    this.update(cx, |this, _| this.in_menu_bar = Some(said.applies)).ok();
                    // Kuvo on this Mac is told where this app is and its version, so its menu bar's
                    // Open Kuvo opens this very app (an older copy with the same ID never), and it
                    // says when it runs a newer Kuvo than this app (`appCheck`). What it answers is
                    // no matter: an older box doesn't take it.
                    if let Some(bundle) = std::env::current_exe().ok().and_then(|executable| own_bundle(&executable)) {
                        let client = nearest.clone();
                        cx.background_executor().spawn(async move { client.register_desktop(&bundle.to_string_lossy(), crate::updates::version()) }).await.ok();
                    }
                    return;
                }
                cx.background_executor().timer(MENU_BAR_ASKED_EVERY).await;
            }
        }));
    }

    /// Quit (⌘Q): the first time on a Mac whose Kuvo stays at work in the menu bar, the note
    /// first; else the app quits.
    pub fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.notes_quit(window, cx) {
            cx.quit();
        }
    }

    /// Shows the first Quit's note where it's owed (or is shown already): whether it did.
    fn notes_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.quit_note.is_some() {
            return true;
        }
        if self.in_menu_bar != Some(true) || crate::quit_note::seen(cx) {
            return false;
        }
        self.focused_before = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.quit_note = Some(focus);
        cx.notify();
        true
    }

    /// Whether the first Quit's note is shown.
    pub fn quit_note_shown(&self) -> bool {
        self.quit_note.is_some()
    }

    /// The note answered: seen either way; Quit goes on, Cancel stays.
    fn quit_noted(&mut self, quits: bool, window: &mut Window, cx: &mut Context<Self>) {
        crate::quit_note::note_seen(cx);
        self.quit_note = None;
        if quits {
            return cx.quit();
        }
        // Back where you were (the composer you were typing in, a list).
        match self.focused_before.take() {
            Some(before) => window.focus(&before, cx),
            None => {
                let workspace = self.workspace().clone();
                workspace.update(cx, |workspace, cx| {
                    if workspace.is_landing() {
                        window.focus(&workspace.landing().focus_handle(cx), cx);
                    }
                });
            }
        }
        cx.notify();
    }

    pub fn notifier(&self) -> &Entity<Notifier> {
        &self.notifier
    }

    /// A notification clicked: the window comes forward on its computer, at its session.
    fn open_notice(&mut self, open: &Open, window: &mut Window, cx: &mut Context<Self>) {
        // Another computer this app doesn't reach yet (it opened for this link, or that computer is
        // being reached): waited for, a little, rather than lost.
        if self.slot(&open.computer) != NEAREST && self.boxes.client(&open.computer).is_none() {
            cx.activate(true);
            window.activate_window();
            self.opening = Some(open.clone());
            self._opening = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RESTORE_WAITS_AT_MOST).await;
                this.update(cx, |this, cx| {
                    this.opening = None;
                    cx.notify();
                })
                .ok();
            }));
            return;
        }
        self.opening = None;
        self._opening = None;
        // (It wins over where the window was last time.)
        self.restoring = None;
        cx.activate(true);
        window.activate_window();
        let shown = if open.computer == NEAREST {
            self.leave_first_run(cx);
            self.shown = NEAREST.to_owned();
            self.sync_shown(cx);
            self.workspaces.contains_key(NEAREST)
        } else {
            self.switch_to(&open.computer, window, cx)
        };
        if shown {
            self.keep_shown(cx);
        }
        if let (true, Some(session)) = (shown, &open.session) {
            let workspace = self.workspace().clone();
            workspace.update(cx, |workspace, cx| workspace.open_session_now(session, window, cx));
        }
        cx.notify();
    }

    /// The computers this device's notifications come from: the nearest, and each other one it
    /// reaches.
    fn notified(&self) -> Vec<(String, Option<u64>, BoxClient)> {
        let nearest = self.boxes.nearest_key();
        let mut clients: Vec<(String, Option<u64>, BoxClient)> = self.boxes.nearest().map(|client| (NEAREST.to_owned(), None, client)).into_iter().collect();
        for row in self.boxes.rows() {
            if !row.usable() || Some(row.key()) == nearest.as_deref() {
                continue;
            }
            if let Some(client) = self.boxes.client(row.key()) {
                clients.push((row.key().to_owned(), self.boxes.client_made(row.key()), client));
            }
        }
        clients
    }

    /// The workspace shown.
    pub fn workspace(&self) -> &Entity<Workspace> {
        &self.workspaces[&self.shown].0
    }

    /// The key of the computer shown (none: the nearest, before its box said its key).
    pub fn shown_key(&self) -> Option<String> {
        if self.shown == NEAREST { self.boxes.nearest_key() } else { Some(self.shown.clone()) }
    }

    /// Now, as each workspace's times count from it (fixed for shots).
    pub fn set_now(&mut self, now: f64, cx: &mut Context<Self>) {
        self.now = Some(now);
        for (workspace, _) in self.workspaces.values() {
            workspace.update(cx, |workspace, cx| workspace.set_now(now, cx));
        }
    }

    fn slot(&self, key: &str) -> String {
        if self.boxes.nearest().is_some() && self.boxes.nearest_key().as_deref() == Some(key) { NEAREST.to_owned() } else { key.to_owned() }
    }

    fn make(&mut self, slot: &str, client: BoxClient, window: &mut Window, cx: &mut Context<Self>) {
        let live = self.live;
        let switching = !self.workspaces.is_empty() || slot != NEAREST;
        let is_shown = slot == self.shown;
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::new(client, live, window, cx);
            workspace.set_shown(is_shown, cx);
            workspace.moves().update(cx, |moves, _| moves.set_computers(self.boxes.clone()));
            if switching {
                workspace.skip_first_run(cx);
            }
            workspace
        });
        // Each computer's window opens where it was last time on this device.
        let key = if slot == NEAREST { crate::yours::NEAREST.to_owned() } else { slot.to_owned() };
        workspace.update(cx, |workspace, cx| workspace.restore(key, window, cx));
        let (own, carries) = (self.own_update.clone(), self.installer.is_some());
        workspace.update(cx, |workspace, cx| {
            workspace.set_own_update(own, cx);
            workspace.set_carries_release(carries, cx);
        });
        if let Some(now) = self.now {
            workspace.update(cx, |workspace, cx| workspace.set_now(now, cx));
        }
        let subscription = cx.subscribe_in(&workspace, window, |this: &mut Home, _, event: &Switch, window, cx| match event {
            Switch::To(key) => {
                this.restoring = None;
                if this.switch_to(key, window, cx) {
                    this.keep_shown(cx);
                }
            }
            Switch::OpenMoved { computer, target } => {
                this.restoring = None;
                if this.switch_to(computer, window, cx) {
                    this.keep_shown(cx);
                    this.workspace().clone().update(cx, |view, cx| view.open_moved(target, window, cx));
                }
            }
            Switch::Session { computer, session } => {
                if this.switch_to(computer, window, cx) {
                    this.keep_shown(cx);
                    this.workspace().clone().update(cx, |workspace, cx| workspace.open_session(session, window, cx));
                }
            }
            Switch::PairAgain => this.pair_again(cx),
            // (Followed live, the list says it by itself.)
            Switch::ReadList => {
                if !this.live {
                    let _ = this.boxes.read_list();
                    this.rows_changed(cx);
                }
            }
            Switch::UpdateOwnBox { now } => this.run_own_update(*now, cx),
            Switch::ShowComputer { key, add_at } => this.show_computer(key, add_at.as_deref(), window, cx),
        });
        self.workspaces.insert(slot.to_owned(), (workspace, subscription));
    }

    /// Tells every workspace whether it's the one currently shown: background workspaces first so
    /// they stop writing to shared `bot_face` states, then the shown one so it restores its bots.
    fn sync_shown(&self, cx: &mut Context<Self>) {
        for (slot, (workspace, _)) in &self.workspaces {
            if slot != &self.shown {
                workspace.update(cx, |workspace, cx| workspace.set_shown(false, cx));
            }
        }
        if let Some((workspace, _)) = self.workspaces.get(&self.shown) {
            workspace.update(cx, |workspace, cx| workspace.set_shown(true, cx));
        }
    }

    /// Selecting work ends this device's onboarding for all cached computers.
    fn leave_first_run(&mut self, cx: &mut Context<Self>) {
        crate::yours::change(crate::yours::Change::Welcomed, cx);
        for (workspace, _) in self.workspaces.values() {
            workspace.update(cx, |workspace, cx| workspace.skip_first_run(cx));
        }
    }

    /// Shows the computer with this key: its own box's sessions, bots, queue and settings. False
    /// for one this app doesn't reach (not yours, not paired, nowhere it can be found).
    pub fn switch_to(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let slot = self.slot(key);
        if !self.workspaces.contains_key(&slot) {
            let Some(client) = self.boxes.client(key) else { return false };
            self.make(&slot, client, window, cx);
            if let Some(made) = self.boxes.client_made(key) {
                self.made.insert(slot.clone(), made);
            }
            #[cfg(feature = "stand-in")]
            if !self.live {
                self.workspaces[&slot].0.update(cx, |workspace, cx| workspace.read_at_once(cx));
            }
        }
        self.leave_first_run(cx);
        self.shown = slot;
        self.sync_shown(cx);
        self.rows_changed(cx);
        let workspace = self.workspace().clone();
        workspace.update(cx, |workspace, cx| {
            if workspace.is_landing() {
                window.focus(&workspace.landing().focus_handle(cx), cx);
            }
        });
        cx.notify();
        true
    }

    /// The computer shown last time, where this app reaches it now (connected, or held for its
    /// update, which it says itself); else the window stays on the one it opened with, quietly.
    fn restore_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.restoring.take() else { return };
        self._restoring = None;
        if self.slot(&key) != self.shown && self.boxes.rows().iter().any(|row| row.key() == key && row.usable()) {
            self.switch_to(&key, window, cx);
        }
        cx.notify();
    }

    /// Keeps the computer shown now, for the next time the window opens.
    fn keep_shown(&self, cx: &App) {
        let computer = if self.shown == NEAREST { None } else { Some(self.shown.clone()) };
        crate::yours::keep(crate::yours::Moved::Computer(computer), cx);
    }

    /// Whether the window still waits for your computers' list to show the one it showed last time.
    pub fn is_restoring(&self) -> bool {
        self.restoring.is_some()
    }

    /// After this app was removed from your devices, on your word: paired anew, off the UI's thread.
    fn pair_again(&mut self, cx: &mut Context<Self>) {
        let boxes = self.boxes.clone();
        if !self.live {
            boxes.pair_again();
            boxes.settle();
            return self.rows_changed(cx);
        }
        cx.background_spawn(async move {
            boxes.pair_again();
        })
        .detach();
    }

    /// Shows a computer in Computers, on the box that lists your computers (this Mac's, where it
    /// has one): Add a Machine with `add_at` typed in, else its page.
    fn show_computer(&mut self, key: &str, add_at: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        self.restoring = None;
        if self.boxes.nearest().is_some() && self.shown != NEAREST {
            self.leave_first_run(cx);
            self.shown = NEAREST.to_owned();
            self.sync_shown(cx);
            self.rows_changed(cx);
            self.keep_shown(cx);
        }
        let workspace = self.workspace().clone();
        workspace.update(cx, |workspace, cx| workspace.show_computer(key, add_at, window, cx));
        cx.notify();
    }

    /// Brings this Mac's own Kuvo up to the release this app carries, where its box is from before
    /// it took that itself the moment its work ended (or waits for an update it can't read now): by
    /// the installer this app carries, which runs the release's own code for it. It waits for the
    /// work on this Mac to end, saying so in the update chip, which offers Update Now. Nothing is
    /// drawn of a box that's up to date, nor where the installer can't (a build that carries none).
    // COMPAT(0.8 beta update): goes once no Mac's box is from before `computers.update.here`.
    pub fn update_own_box(&mut self, installer: Installer, cx: &mut Context<Self>) {
        let Some(nearest) = self.boxes.nearest() else { return };
        self.installer = Some(installer);
        for (workspace, _) in self.workspaces.values() {
            workspace.update(cx, |workspace, cx| workspace.set_carries_release(true, cx));
        }
        // Whether this app brings it up (none: it can't be reached now, to be asked again).
        let needs = move || match nearest.connect() {
            Ok(info) => Some(!info.has(UPDATES_ITSELF) || nearest.gate().is_older()),
            Err(kuvo_box::Error::NeedsUpdating { .. }) => Some(true),
            Err(_) => None,
        };
        if !self.live {
            if needs() == Some(true) {
                self.run_own_update(false, cx);
            }
            return;
        }
        self._own_asking = Some(cx.spawn(async move |this, cx| {
            loop {
                let asked = needs.clone();
                match cx.background_executor().spawn(async move { asked() }).await {
                    Some(true) => {
                        this.update(cx, |this, cx| this.run_own_update(false, cx)).ok();
                        return;
                    }
                    Some(false) => return,
                    None => cx.background_executor().timer(OWN_UPDATE_ASKED_EVERY).await,
                }
            }
        }));
    }

    /// Runs the installer's `update`: `now` (Update Now, asked first), else once the work on this
    /// Mac ends, at once where none is (one such run at a time: another asked meanwhile is the same).
    fn run_own_update(&mut self, now: bool, cx: &mut Context<Self>) {
        let Some(installer) = self.installer.clone() else { return };
        if now {
            self.set_own_update(Some(OwnUpdate::Updating { title: UPDATING.into() }), cx);
        } else if self.own_waits {
            // (Still waiting for the work here to end: as it said.)
            let waiting = self.own_waited.clone().unwrap_or(OwnUpdate::Waiting { title: WAITS.into(), confirm: None });
            return self.set_own_update(Some(waiting), cx);
        }
        if !self.live {
            let mut said = Vec::new();
            let ran = installer.update(now, &mut |progress| said.push(progress));
            for progress in said {
                self.own_progress(progress, cx);
            }
            return self.own_ended(now, ran, cx);
        }
        let (said, drawn) = async_channel::unbounded();
        let work = cx.background_spawn(async move { installer.update(now, &mut |progress| drop(said.send_blocking(progress))) });
        let run = cx.spawn(async move |this, cx| {
            while let Ok(progress) = drawn.recv().await {
                if this.update(cx, |this, cx| this.own_progress(progress, cx)).is_err() {
                    return;
                }
            }
            let ran = work.await;
            this.update(cx, |this, cx| {
                if !now {
                    this.own_waits = false;
                }
                this.own_ended(now, ran, cx);
            })
            .ok();
        });
        // (The one before, over, goes in its place.)
        if now {
            self.own_now = Some(run);
        } else {
            self.own_waits = true;
            self.own_waiting = Some(run);
        }
    }

    /// A step of bringing this Mac's own Kuvo up to date, in the box's words. (A run that waits
    /// says so while Update Now's is under way: that one's word stands.)
    fn own_progress(&mut self, progress: Progress, cx: &mut Context<Self>) {
        let Progress::Step(step) = progress else { return };
        let own = if step.step == "waiting" {
            let waiting = OwnUpdate::Waiting { title: step.title.unwrap_or_else(|| WAITS.into()), confirm: step.confirm };
            self.own_waited = Some(waiting.clone());
            if matches!(self.own_update, Some(OwnUpdate::Updating { .. })) {
                return;
            }
            waiting
        } else {
            OwnUpdate::Updating { title: step.title.unwrap_or_else(|| UPDATING.into()) }
        };
        self.set_own_update(Some(own), cx);
    }

    /// How bringing this Mac's own Kuvo up to date ended. Done, or up to date already: nothing
    /// more to say. Still waiting for its work to end: Update Now stays (unless Update Now's run
    /// is under way). Updating already (Update Now's `busy`): under way. Failed: said, with Try
    /// Again. It can't be updated here now (blocked, off, refused), or the installer couldn't say:
    /// said, with nothing to press.
    fn own_ended(&mut self, now: bool, ran: Ran, cx: &mut Context<Self>) {
        let said = |message: Option<String>| message.unwrap_or_else(|| "It didn’t go through. Try again in a moment.".into());
        let own = match ran {
            Ran::Answered { answer, .. } => match answer.result.as_deref() {
                Some("updated" | "current") => {
                    self.own_waited = None;
                    None
                }
                Some("busy") if now => Some(OwnUpdate::Updating { title: UPDATING.into() }),
                Some("busy") => match self.own_update.clone() {
                    Some(going @ (OwnUpdate::Waiting { .. } | OwnUpdate::Updating { .. })) => Some(going),
                    _ => Some(self.own_waited.clone().unwrap_or(OwnUpdate::Waiting { title: WAITS.into(), confirm: None })),
                },
                Some("failed") => Some(OwnUpdate::Failed { message: said(answer.message) }),
                Some("blocked" | "off" | "refused") => Some(OwnUpdate::CantNow { message: said(answer.message) }),
                _ => {
                    // (Nothing to say of this run: what another says stands.)
                    if !now && matches!(self.own_update, Some(OwnUpdate::Failed { .. } | OwnUpdate::Updating { .. })) {
                        return;
                    }
                    None
                }
            },
            Ran::Broken(why) if now => Some(OwnUpdate::Failed { message: why }),
            Ran::Broken(why) => Some(OwnUpdate::CantNow { message: why }),
        };
        self.set_own_update(own, cx);
    }

    fn set_own_update(&mut self, own: Option<OwnUpdate>, cx: &mut Context<Self>) {
        self.own_update = own.clone();
        for (workspace, _) in self.workspaces.values() {
            let own = own.clone();
            workspace.update(cx, |workspace, cx| workspace.set_own_update(own, cx));
        }
        cx.notify();
    }

    /// How bringing this Mac's own Kuvo up to date stands, where this app does it.
    pub fn own_update(&self) -> Option<&OwnUpdate> {
        self.own_update.as_ref()
    }

    /// Your computers, or how one stands, changed: every workspace's switcher draws them.
    pub fn rows_changed(&mut self, cx: &mut Context<Self>) {
        // Kuvo on this Mac runs a newer version than this app: the app looks for its own update
        // now, in the background (Sparkle installs it as you set it), once for each version.
        if self.live {
            let said = self.boxes.rows().into_iter().find(|row| row.computer.here).and_then(|row| row.computer.app_check.clone());
            if let Some(version) = crate::updates::app_check_due(said.as_deref(), self.app_checked.as_deref()).map(str::to_owned)
                && let Some(updates) = crate::updates::Updates::get(cx)
                && updates.update(cx, |updates, cx| updates.check_quietly(cx))
            {
                self.app_checked = Some(version);
            }
        }
        // A computer this app no longer reaches (dropped from your list, filed apart), or reaches
        // with another client now (paired anew, found elsewhere): its workspace held the old one,
        // and goes. The window falls back to the first it still has.
        let gone: Vec<String> = self.workspaces.keys().filter(|slot| slot.as_str() != NEAREST && self.boxes.client_made(slot) != self.made.get(*slot).copied()).cloned().collect();
        for slot in gone {
            if self.workspaces.len() > 1 {
                self.workspaces.remove(&slot);
                self.made.remove(&slot);
            }
        }
        if !self.workspaces.contains_key(&self.shown) {
            self.shown = if self.workspaces.contains_key(NEAREST) { NEAREST.to_owned() } else { self.workspaces.keys().next().cloned().unwrap_or_default() };
            self.sync_shown(cx);
        }
        if self.live {
            let clients = self.notified();
            self.notifier.update(cx, |notifier, cx| notifier.follow(clients, cx));
        }
        // Your computers are known (the nearest box listed them, or there's none here and the
        // kept list is all there is): the computer shown last time, if it's reached.
        // One still being reached is waited for (at most `RESTORE_WAITS_AT_MOST`, then this
        // computer's window): one that isn't answering yet isn't opened on.
        if let Some(key) = self.restoring.clone()
            && (self.boxes.nearest().is_none() || self.boxes.nearest_key().is_some())
        {
            let row = self.boxes.rows().into_iter().find(|row| row.key() == key);
            if self.live && row.as_ref().is_some_and(|row| row.reach == kuvo_box::boxes::Reach::Connecting) {
                // (Asked again as it's reached, or isn't.)
            } else if row.is_some_and(|row| row.usable()) {
                let (this, handle) = (cx.entity().downgrade(), self.window_handle);
                App::defer(cx, move |cx| {
                    handle.update(cx, |_, window, cx| this.update(cx, |this, cx| this.restore_shown(window, cx))).ok();
                });
            } else {
                self.restoring = None;
                self._restoring = None;
            }
        }
        // A link waiting for its computer: shown once this app reaches it.
        if self.opening.as_ref().is_some_and(|open| self.boxes.client(&open.computer).is_some())
            && let Some(open) = self.opening.take()
        {
            self._opening = None;
            let (this, handle) = (cx.entity().downgrade(), self.window_handle);
            App::defer(cx, move |cx| {
                handle.update(cx, |_, window, cx| this.update(cx, |this, cx| this.open_notice(&open, window, cx))).ok();
            });
        }
        let (rows, pairing) = (self.boxes.rows(), self.boxes.pairing());
        let nearest = self.boxes.nearest_key();
        let lister = self.boxes.lister();
        for (slot, (workspace, _)) in &self.workspaces {
            let shown = if slot == NEAREST { nearest.clone() } else { Some(slot.clone()) };
            let switching = Switching { rows: rows.clone(), shown, pairing: pairing.clone() };
            workspace.update(cx, |workspace, cx| {
                workspace.set_lister(lister.clone());
                workspace.set_switching(Some(switching), cx);
            });
        }
        cx.notify();
    }

    /// Reads your computers and how each stands at once (tests and shots: nothing waits on a thread).
    #[cfg(feature = "stand-in")]
    pub fn read_at_once(&mut self, cx: &mut Context<Self>) {
        let _ = self.boxes.read_list();
        self.boxes.settle();
        self.workspace().clone().update(cx, |workspace, cx| workspace.read_at_once(cx));
        self.rows_changed(cx);
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let note = self.quit_note.as_ref().map(|focus| {
            let this = cx.entity().downgrade();
            let other = this.clone();
            crate::quit_note::card(
                focus,
                move |window: &mut Window, cx: &mut App| drop(this.update(cx, |home, cx| home.quit_noted(false, window, cx))),
                move |window: &mut Window, cx: &mut App| drop(other.update(cx, |home, cx| home.quit_noted(true, window, cx))),
                cx,
            )
        });
        if self.restoring.is_some() {
            // (Nothing yet, not another computer's window first: only the window's colour.)
            // (Quit meanwhile: its note, over it.)
            return div().debug_selector(|| "window-restoring".into()).relative().size_full().bg(cx.theme().colors.background).children(note).into_any_element();
        }
        div().relative().size_full().child(self.workspace().clone()).children(note).into_any_element()
    }
}

/// Your boxes over a stand-in: it's the nearest, and its other computers are reached as it says
/// itself (its made-up peers), with no relay and no key kept.
#[cfg(feature = "stand-in")]
pub fn stand_in_boxes(stand_in: crate::stand_in::StandIn) -> Boxes {
    use std::sync::Arc;
    Boxes::new(kuvo_box::boxes::Options {
        nearest: Some(Arc::new(stand_in)),
        keeper: Arc::new(kuvo_box::link::MemoryKeeper::default()),
        dial: kuvo_box::link::stand_in::Relay::new().dial(),
        name: "Stand-in".into(),
        app: env!("CARGO_PKG_VERSION").into(),
        kept: None,
        timing: Default::default(),
    })
}
