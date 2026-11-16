//! This device's notifications: what each of your computers tells (`kuvo/notices`, worded and
//! picked by its box) shown as macOS notifications while Kuvo isn't in front, with its sound, and
//! what waits for you on every computer (`kuvo/waiting`) counted on the Dock icon. Your settings ›
//! Notifications says whether, on this device; the box decides what's news and how it's said.
//! Clicking one opens its computer, and its session where it's one's.

use std::collections::{HashMap, HashSet};

use gpui::{App, Context, EventEmitter, Subscription, SystemNotification, Task};

/// A notification to post: its tag (where a click opens: [`Open::tag`]), its words, and whether
/// it comes with the system's notification sound.
#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub tag: String,
    pub title: String,
    pub body: String,
    pub sound: bool,
}
use kuvo_box::BoxClient;
use kuvo_box::areas::Waiting;
use kuvo_box::follow::{Followed, Timing};
use kuvo_box::notices::{Told, ToldNotice};

use crate::yours;

/// What only the system does or knows: posting a notification, the Dock's count, and whether
/// Kuvo is in front (tests keep what they'd do, and say whether a window of theirs is).
pub struct Effects {
    pub post: Box<dyn Fn(Post, &mut App)>,
    pub badge: Box<dyn Fn(usize)>,
    pub in_front: Box<dyn Fn(&App) -> bool>,
}

impl Effects {
    /// This Mac's notifications and Dock; in front while Kuvo is the active app with its window.
    pub fn system() -> Effects {
        Effects {
            post: Box::new(|post, cx| yours::desk::post(post.tag, post.title, post.body, post.sound, cx)),
            badge: Box::new(yours::desk::set_dock_count),
            in_front: Box::new(|cx| yours::desk::app_active() && cx.active_window().is_some()),
        }
    }

    /// None of the system's: posted through GPUI (its test platform keeps them), in front while a
    /// window is (tests and shots).
    pub fn none() -> Effects {
        Effects {
            post: Box::new(|post, cx| cx.show_system_notification(SystemNotification { tag: post.tag.into(), title: post.title.into(), body: post.body.into(), actions: Vec::new() })),
            badge: Box::new(|_| {}),
            in_front: Box::new(|cx| cx.active_window().is_some()),
        }
    }
}

/// A notification clicked: its computer (its key; "" for the one on this Mac before its box said
/// its key), and the session it's about, where it's one's.
#[derive(Clone, Debug, PartialEq)]
pub struct Open {
    pub computer: String,
    pub session: Option<String>,
}

impl Open {
    /// A notification's tag: the notice's id, then where it opens, so a click finds it after a
    /// restart too (the box's ids have no `|`).
    pub fn tag(&self, id: &str) -> String {
        format!("{id}|{}|{}", self.computer, self.session.as_deref().unwrap_or_default())
    }

    /// Where a `kuvo-code://open` link opens (Kuvo Helper's menu bar item opens one in this very
    /// app): `computer`, a box's key (none or empty: this Mac's), and `session`, one of its
    /// sessions. None for any other link, or one whose key or session isn't one.
    pub fn of_url(url: &str) -> Option<Open> {
        let rest = url.strip_prefix("kuvo-code://open")?;
        let query = match rest {
            "" | "/" => "",
            _ => rest.strip_prefix("/").unwrap_or(rest).strip_prefix('?')?,
        };
        let (mut computer, mut session) = (String::new(), None);
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_decoded(value)?;
            match name {
                "computer" => computer = value,
                "session" if !value.is_empty() => session = Some(value),
                _ => {}
            }
        }
        let key = computer.is_empty() || (computer.len() == 43 && computer.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
        let id = session.as_deref().is_none_or(|id| id.len() <= 64 && id.strip_prefix("ses_").is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())));
        (key && id).then_some(Open { computer, session })
    }

    /// Where a tag opens; none for one that isn't this app's.
    pub fn of_tag(tag: &str) -> Option<Open> {
        let mut parts = tag.splitn(3, '|');
        let (_, computer, session) = (parts.next()?, parts.next()?, parts.next()?);
        Some(Open { computer: computer.to_owned(), session: (!session.is_empty()).then(|| session.to_owned()) })
    }
}

/// One computer followed.
#[derive(Default)]
struct Computer {
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
    /// The notices each computer has told already, by id (none until its first read, whose
    /// notices are from before this app was there). Kept while a computer's follow comes and
    /// goes (a drop, a new address), so what it told meanwhile still shows when it's back.
    seen: HashMap<String, HashSet<String>>,
    /// The Dock's count as last set.
    badge: Option<usize>,
    _yours: Subscription,
}

impl EventEmitter<Open> for Notifier {}

impl Notifier {
    pub fn new(effects: Effects, cx: &mut Context<Self>) -> Notifier {
        // The Dock's switch turned: its count goes, or comes back.
        let yours = cx.observe_global::<yours::Store>(|this: &mut Notifier, cx| this.update_badge(cx));
        Notifier { effects, computers: HashMap::new(), seen: HashMap::new(), badge: None, _yours: yours }
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
            self.computers.insert(key, Computer { waiting: 0, made, _follows: follows, _tasks: vec![told_task, waits_task] });
        }
        self.update_badge(cx);
    }

    /// What a computer told lately: each notice new since its last read is shown, by your
    /// settings. None of its first read is (it was told before this app was there).
    pub fn take_notices(&mut self, computer: &str, told: Told, cx: &mut Context<Self>) {
        let fresh: Vec<ToldNotice> = match self.seen.get(computer) {
            None => Vec::new(),
            Some(seen) => told.notices.iter().filter(|notice| !seen.contains(&notice.id)).cloned().collect(),
        };
        self.seen.insert(computer.to_owned(), told.notices.iter().map(|notice| notice.id.clone()).collect());
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
        let tag = Open { computer: computer.to_owned(), session: notice.session().map(str::to_owned) }.tag(&told.id);
        let post = Post { tag, title, body: notice.body.clone().unwrap_or_default(), sound: settings.sound && !notice.quiet() };
        (self.effects.post)(post, cx);
    }

    /// A notification clicked: where it opens, said to the window.
    pub fn clicked(&mut self, tag: &str, cx: &mut Context<Self>) {
        if let Some(open) = Open::of_tag(tag) {
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

/// `%XX` as the byte it stands for (and `+` as itself: neither a key nor a session has a space);
/// none where what it comes to isn't text.
fn percent_decoded(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Links opened in this app (`kuvo-code://open`, as macOS hands them over, `on_open_urls`), kept
/// until the window takes them: one can come as the app opens, before there's a window to show it.
pub mod opened {
    use std::sync::OnceLock;

    use super::Open;

    fn channel() -> &'static (async_channel::Sender<Open>, async_channel::Receiver<Open>) {
        static CHANNEL: OnceLock<(async_channel::Sender<Open>, async_channel::Receiver<Open>)> = OnceLock::new();
        CHANNEL.get_or_init(async_channel::unbounded)
    }

    /// Links macOS opened in this app: each of Kuvo's kept for the window; any other, let be.
    pub fn take(urls: Vec<String>) {
        for open in urls.iter().filter_map(|url| Open::of_url(url)) {
            channel().0.try_send(open).ok();
        }
    }

    /// Where the window hears of them, those from before it was there first.
    pub fn receiver() -> async_channel::Receiver<Open> {
        channel().1.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::Open;

    const KEY: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE";

    #[test]
    fn a_link_opens_its_computer_and_session() {
        assert_eq!(Open::of_url(&format!("kuvo-code://open?computer={KEY}&session=ses_abc123")), Some(Open { computer: KEY.into(), session: Some("ses_abc123".into()) }));
        // (Percent-encoded as a link may be.)
        assert_eq!(Open::of_url(&format!("kuvo-code://open?computer={}&session=ses_abc123", KEY.replace('-', "%2D"))), Some(Open { computer: KEY.into(), session: Some("ses_abc123".into()) }));
    }

    #[test]
    fn a_link_without_a_computer_opens_this_macs() {
        assert_eq!(Open::of_url("kuvo-code://open"), Some(Open { computer: String::new(), session: None }));
        assert_eq!(Open::of_url("kuvo-code://open?session=ses_x1"), Some(Open { computer: String::new(), session: Some("ses_x1".into()) }));
        assert_eq!(Open::of_url("kuvo-code://open/?computer="), Some(Open { computer: String::new(), session: None }));
    }

    #[test]
    fn other_links_are_let_be() {
        for url in ["https://kuvo.run", "kuvo-code://delete?computer=x", "kuvo-code://open?computer=short", "kuvo-code://open?session=../../etc", "kuvo-code://open?session=ses_", "kuvo-code://openx", "kuvo-code://open?computer=%zz"] {
            assert_eq!(Open::of_url(url), None, "{url}");
        }
    }
}
