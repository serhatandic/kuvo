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

use gpui::{App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task, Window, div};
use kuvo_box::BoxClient;
use kuvo_box::boxes::{Boxes, Pairing, Row};

use crate::workspace::{Switch, Workspace};

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
    _watch: Option<Task<()>>,
    /// Whether this computer's own Kuvo shows in its menu bar (its box's `kuvo/box/at-work`
    /// `applies`), so quitting leaves it at work there: the first Quit's note says so. Not known
    /// (its box not reached yet): no note.
    in_menu_bar: Option<bool>,
    _menu_bar: Option<Task<()>>,
    /// The first Quit's note, while it's shown (`quit_note`), and what had focus before it.
    quit_note: Option<FocusHandle>,
    focused_before: Option<FocusHandle>,
}

/// How often this computer's box is asked again whether it's in the menu bar, while it hasn't said.
const MENU_BAR_ASKED_EVERY: Duration = Duration::from_secs(30);

/// The nearest box's workspace, whatever its key turns out to be.
const NEAREST: &str = "";

impl Home {
    /// `first`: the box the window opens with (the nearest, or with none here, one of your
    /// computers the kept list names). `live`: whether it follows your boxes (the app), or shows
    /// what it's handed (tests and shots).
    pub fn new(boxes: Boxes, first: BoxClient, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Home {
        let shown = if boxes.nearest().is_some() { NEAREST.to_owned() } else { boxes.rows().iter().find(|row| row.usable()).map(|row| row.key().to_owned()).unwrap_or_default() };
        let mut home = Home { boxes: boxes.clone(), live, workspaces: HashMap::new(), made: HashMap::new(), shown: shown.clone(), now: None, _watch: None, in_menu_bar: None, _menu_bar: None, quit_note: None, focused_before: None };
        home.make(&shown, first, window, cx);
        home.ask_menu_bar(cx);
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
        let workspace = cx.new(|cx| Workspace::new(client, live, window, cx));
        if let Some(now) = self.now {
            workspace.update(cx, |workspace, cx| workspace.set_now(now, cx));
        }
        let subscription = cx.subscribe_in(&workspace, window, |this: &mut Home, _, event: &Switch, window, cx| match event {
            Switch::To(key) => {
                this.switch_to(key, window, cx);
            }
            Switch::PairAgain => this.pair_again(cx),
        });
        self.workspaces.insert(slot.to_owned(), (workspace, subscription));
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
        self.shown = slot;
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

    /// Your computers, or how one stands, changed: every workspace's switcher draws them.
    pub fn rows_changed(&mut self, cx: &mut Context<Self>) {
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
        }
        let (rows, pairing) = (self.boxes.rows(), self.boxes.pairing());
        let nearest = self.boxes.nearest_key();
        for (slot, (workspace, _)) in &self.workspaces {
            let shown = if slot == NEAREST { nearest.clone() } else { Some(slot.clone()) };
            let switching = Switching { rows: rows.clone(), shown, pairing: pairing.clone() };
            workspace.update(cx, |workspace, cx| workspace.set_switching(Some(switching), cx));
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
        div().relative().size_full().child(self.workspace().clone()).children(note)
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
