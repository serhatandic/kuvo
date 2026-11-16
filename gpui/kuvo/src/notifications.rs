//! This device's notifications: what each of your computers tells (`kuvo/notices`, worded and
//! picked by its box) shown as macOS notifications while Kuvo isn't in front, with a sound, and
//! what waits for you on every computer (`kuvo/waiting`) counted on the Dock icon. Your settings ›
//! Notifications says whether, on this device; the box decides what's news and how it's said.
//! Clicking one opens its computer, and its session where it's one's.

use std::collections::{HashMap, HashSet};

use gpui::{App, Context, EventEmitter, Subscription, SystemNotification, Task};
use kuvo_box::BoxClient;
use kuvo_box::areas::Waiting;
use kuvo_box::follow::{Followed, Timing};
use kuvo_box::notices::{Told, ToldNotice};

use crate::yours;

/// What only the system does or knows: the Dock's count, a sound, and whether Kuvo is in front
/// (tests keep what they'd do, and say whether a window of theirs is).
pub struct Effects {
    pub badge: Box<dyn Fn(usize)>,
    pub sound: Box<dyn Fn()>,
    pub in_front: Box<dyn Fn(&App) -> bool>,
}

impl Effects {
    /// This Mac's Dock and alert sound; in front while Kuvo is the active app with its window.
    pub fn system() -> Effects {
        Effects { badge: Box::new(yours::desk::set_dock_count), sound: Box::new(yours::desk::play_sound), in_front: Box::new(|cx| yours::desk::app_active() && cx.active_window().is_some()) }
    }

    /// None of the system's: in front while a window is (tests and shots).
    pub fn none() -> Effects {
        Effects { badge: Box::new(|_| {}), sound: Box::new(|| {}), in_front: Box::new(|cx| cx.active_window().is_some()) }
    }
}

/// A notification clicked: its computer (its key; "" for the one on this Mac before its box said
/// its key), and the session it's about, where it's one's.
#[derive(Clone, Debug, PartialEq)]
pub struct Open {
    pub computer: String,
    pub session: Option<String>,
}

/// One computer followed.
#[derive(Default)]
struct Computer {
    /// The notices already there, by id (none until its first read, whose notices are from
    /// before this app was there, and aren't shown).
    seen: Option<HashSet<String>>,
    waiting: usize,
    /// Which client it's followed with ([`kuvo_box::boxes::Boxes::client_made`]): another one
    /// (paired anew, found elsewhere) is followed afresh.
    made: Option<u64>,
    _follows: Vec<Box<dyn std::any::Any>>,
    _tasks: Vec<Task<()>>,
}

pub struct Notifier {
    effects: Effects,
    computers: HashMap<String, Computer>,
    /// Where each notification shown opens, by its tag (the notice's id).
    opens: HashMap<String, Open>,
    /// The Dock's count as last set.
    badge: Option<usize>,
    _yours: Subscription,
}

impl EventEmitter<Open> for Notifier {}

impl Notifier {
    pub fn new(effects: Effects, cx: &mut Context<Self>) -> Notifier {
        // The Dock's switch turned: its count goes, or comes back.
        let yours = cx.observe_global::<yours::Store>(|this: &mut Notifier, cx| this.update_badge(cx));
        Notifier { effects, computers: HashMap::new(), opens: HashMap::new(), badge: None, _yours: yours }
    }

    /// Follows the computers in `clients` (by key, with which client each is: see `made`) and lets
    /// go of the others.
    pub fn follow(&mut self, clients: Vec<(String, Option<u64>, BoxClient)>, cx: &mut Context<Self>) {
        let now: HashMap<&String, Option<u64>> = clients.iter().map(|(key, made, _)| (key, *made)).collect();
        self.computers.retain(|key, computer| now.get(key).is_some_and(|made| *made == computer.made));
        for (key, made, client) in clients {
            if self.computers.contains_key(&key) {
                continue;
            }
            let notices = client.follow_notices(Timing::default());
            let waiting = client.follow_waiting(Timing::default());
            let (told, waits) = (notices.updates().clone(), waiting.updates().clone());
            let told_key = key.clone();
            let told_task = cx.spawn(async move |this, cx| {
                while let Ok(update) = told.recv().await {
                    if let Followed::Now(told) = update
                        && this.update(cx, |this, cx| this.take_notices(&told_key, told, cx)).is_err()
                    {
                        break;
                    }
                }
            });
            let waits_key = key.clone();
            let waits_task = cx.spawn(async move |this, cx| {
                while let Ok(update) = waits.recv().await {
                    if let Followed::Now(waiting) = update
                        && this.update(cx, |this, cx| this.take_waiting(&waits_key, waiting, cx)).is_err()
                    {
                        break;
                    }
                }
            });
            let follows: Vec<Box<dyn std::any::Any>> = vec![Box::new(notices), Box::new(waiting)];
            self.computers.insert(key, Computer { seen: None, waiting: 0, made, _follows: follows, _tasks: vec![told_task, waits_task] });
        }
        self.update_badge(cx);
    }

    /// What a computer told lately: each notice new since its last read is shown, by your
    /// settings. None of its first read is (it was told before this app was there).
    pub fn take_notices(&mut self, computer: &str, told: Told, cx: &mut Context<Self>) {
        let followed = self.computers.entry(computer.to_owned()).or_default();
        let fresh: Vec<ToldNotice> = match &followed.seen {
            None => Vec::new(),
            Some(seen) => told.notices.iter().filter(|notice| !seen.contains(&notice.id)).cloned().collect(),
        };
        followed.seen = Some(told.notices.iter().map(|notice| notice.id.clone()).collect());
        // (A box that isn't telling its news now has none to show.)
        if !told.telling {
            return;
        }
        for notice in fresh {
            self.show(computer, notice, cx);
        }
    }

    /// What waits for you on a computer, as its box counts it.
    pub fn take_waiting(&mut self, computer: &str, waiting: Waiting, cx: &mut Context<Self>) {
        self.computers.entry(computer.to_owned()).or_default().waiting = waiting.sessions + waiting.bots;
        self.update_badge(cx);
    }

    fn show(&mut self, computer: &str, told: ToldNotice, cx: &mut Context<Self>) {
        let settings = yours::now(cx).notifications;
        // Your accounts' limits are set on each computer's Usage page; the rest are sessions'.
        if !told.notice.is_usage() && !settings.sessions {
            return;
        }
        // In front, the window shows it already (its sidebar, its rail).
        if (self.effects.in_front)(cx) {
            return;
        }
        let notice = &told.notice;
        let title = notice.title.clone().unwrap_or_else(|| "Kuvo".to_owned());
        let title = match notice.subtitle.as_deref().filter(|subtitle| !subtitle.is_empty()) {
            Some(subtitle) => format!("{title} · {subtitle}"),
            None => title,
        };
        cx.show_system_notification(SystemNotification { tag: told.id.clone().into(), title: title.into(), body: notice.body.clone().unwrap_or_default().into(), actions: Vec::new() });
        if settings.sound && !notice.quiet() {
            (self.effects.sound)();
        }
        // (Those of long ago go: a click on one opens the app as it is.)
        if self.opens.len() > 500 {
            self.opens.clear();
        }
        self.opens.insert(told.id, Open { computer: computer.to_owned(), session: notice.session().map(str::to_owned) });
    }

    /// A notification clicked: where it opens, said to the window.
    pub fn clicked(&mut self, tag: &str, cx: &mut Context<Self>) {
        if let Some(open) = self.opens.get(tag).cloned() {
            cx.emit(open);
        }
    }

    /// What the Dock counts now.
    pub fn count(&self, cx: &App) -> usize {
        if yours::now(cx).notifications.dock { self.computers.values().map(|computer| computer.waiting).sum() } else { 0 }
    }

    fn update_badge(&mut self, cx: &mut Context<Self>) {
        let count = self.count(cx);
        if self.badge != Some(count) {
            self.badge = Some(count);
            (self.effects.badge)(count);
        }
    }
}
