//! The window's root: your boxes ([`kuvo_box::boxes::Boxes`]) and a workspace for each computer
//! you've opened, one shown at a time. No box is the app's own: the one on this computer is only
//! the nearest, shown first; the title bar's switcher shows another's sessions, bots, queue and
//! settings instead, asked of that computer's own box. A workspace you leave stays as it was (its
//! open conversation, its drafts), so coming back is at once.
//!
//! It decides nothing: which computers are yours, how each stands and whether this app is paired
//! are `Boxes`' facts, drawn in the switcher.

use std::collections::HashMap;

use gpui::{AppContext as _, Context, Entity, Focusable as _, IntoElement, Render, Subscription, Task, Window};
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
    shown: String,
    now: Option<f64>,
    _watch: Option<Task<()>>,
}

/// The nearest box's workspace, whatever its key turns out to be.
const NEAREST: &str = "";

impl Home {
    /// `first`: the box the window opens with (the nearest, or with none here, one of your
    /// computers the kept list names). `live`: whether it follows your boxes (the app), or shows
    /// what it's handed (tests and shots).
    pub fn new(boxes: Boxes, first: BoxClient, live: bool, window: &mut Window, cx: &mut Context<Self>) -> Home {
        let shown = if boxes.nearest().is_some() { NEAREST.to_owned() } else { boxes.rows().iter().find(|row| row.usable()).map(|row| row.key().to_owned()).unwrap_or_default() };
        let mut home = Home { boxes: boxes.clone(), live, workspaces: HashMap::new(), shown: shown.clone(), now: None, _watch: None };
        home.make(&shown, first, window, cx);
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
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.workspace().clone()
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
