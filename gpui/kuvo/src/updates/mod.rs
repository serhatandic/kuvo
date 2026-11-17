//! The desktop app updating itself: one of the few things only an installed app does (CLAUDE.md,
//! "Only this stays in a client"). Kuvo on this Mac isn't updated here: the release each app
//! carries is offered to it as the app opens (`launch.rs`), and its version and updates are its
//! own, under Settings › This Mac.
//!
//! An [`Updater`] says how this build updates itself; [`Updates`] holds it for the app, with the
//! facts Settings › Updates and the app's menu are drawn from. On macOS that's Sparkle, in the
//! bundle's `Contents/Frameworks` (`sparkle.rs`); a build without a feed (a `cargo run`, a copy
//! built without one) doesn't update itself and says so. Another system plugs in its own
//! `Updater` and keeps the same page (`.claude/notes/759-desktop-app-updates.md`).

#[cfg(target_os = "macos")]
pub mod sparkle;

use std::rc::Rc;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};

/// Where 0.7 kept "Get beta updates", in the app's own defaults: the desktop app has the same
/// bundle ID, so the choice made there carries over.
pub const BETAS_KEY: &str = "updates.betas";

/// How this build stands with updating itself, as the Updates page draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    /// The app's version ("0.8.0", "0.8.0-beta.2").
    pub version: String,
    /// Whether it updates itself; where it doesn't, why, in a sentence.
    pub can: Can,
    /// A check may start now (not while one is under way, or an update's window is open).
    pub can_check: bool,
    /// A check, a download or an install is under way.
    pub busy: bool,
    /// It looks for updates once a day by itself.
    pub checks: bool,
    /// What it finds downloads by itself, and installs as the app quits; none where this build
    /// doesn't offer that.
    pub downloads: Option<bool>,
    /// Betas too.
    pub betas: bool,
    /// When it last looked (ms since 1970).
    pub checked: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Can {
    Update,
    Not(String),
}

impl Facts {
    /// A build that doesn't update itself, and why.
    pub fn without(version: impl Into<String>, why: impl Into<String>) -> Facts {
        Facts { version: version.into(), can: Can::Not(why.into()), can_check: false, busy: false, checks: false, downloads: None, betas: false, checked: None }
    }

    pub fn updates(&self) -> bool {
        self.can == Can::Update
    }
}

/// This app's version, as the bundle's build stamped it (`KUVO_APP_VERSION`).
pub fn version() -> &'static str {
    option_env!("KUVO_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// A beta: a version tried out before everyone gets it ("0.8.0-beta.2").
pub fn is_beta(version: &str) -> bool {
    version.contains("-beta")
}

/// Whether betas are offered: as you chose; unless you did, a beta gets the next ones and a
/// release doesn't (as 0.7 did).
pub fn wants_betas(chosen: Option<bool>, version: &str) -> bool {
    chosen.unwrap_or_else(|| is_beta(version))
}

/// The channels of the feed this copy reads: releases are in none, for everyone; betas in "beta".
pub fn channels(betas: bool) -> &'static [&'static str] {
    if betas { &["beta"] } else { &[] }
}

/// How this build updates itself.
pub trait Updater {
    /// How it stands now.
    fn facts(&self) -> Facts;
    /// Looks for an update now, showing what it finds (or that there's none) in its own window.
    fn check(&self);
    /// Looks for an update now in the background, as its daily check does: what it finds is
    /// downloaded and installed as you set it (by itself as the app quits, or its own window).
    fn check_quietly(&self);
    fn set_checks(&self, on: bool);
    fn set_downloads(&self, on: bool);
    fn set_betas(&self, on: bool);
}

/// The one a build without a way to update itself has.
pub struct Without {
    pub why: String,
}

impl Updater for Without {
    fn facts(&self) -> Facts {
        Facts::without(version(), self.why.clone())
    }
    fn check(&self) {}
    fn check_quietly(&self) {}
    fn set_checks(&self, _: bool) {}
    fn set_downloads(&self, _: bool) {}
    fn set_betas(&self, _: bool) {}
}

/// The words for a build that doesn't update itself.
pub const NOT_THIS_BUILD: &str = "This build doesn’t update itself.";

/// The app's updater, and how it stands, for Settings and the menu.
pub struct Updates {
    updater: Rc<dyn Updater>,
    facts: Facts,
    _changes: Option<Task<()>>,
}

struct Held(Entity<Updates>);
impl Global for Held {}

impl Updates {
    /// Holds `updater` for the app. `changes` says when it changed by itself (a check started or
    /// ended, a choice made in its own window), so what's drawn is read again.
    pub fn install(updater: Rc<dyn Updater>, changes: Option<async_channel::Receiver<()>>, cx: &mut App) -> Entity<Updates> {
        let entity = cx.new(|cx: &mut Context<Updates>| {
            let task = changes.map(|changes| {
                cx.spawn(async move |this, cx| {
                    while changes.recv().await.is_ok() {
                        if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                            break;
                        }
                    }
                })
            });
            Updates { facts: updater.facts(), updater, _changes: task }
        });
        cx.set_global(Held(entity.clone()));
        entity
    }

    /// The app's, where it has one (tests and shots that set none have none).
    pub fn get(cx: &App) -> Option<Entity<Updates>> {
        cx.try_global::<Held>().map(|held| held.0.clone())
    }

    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    /// Reads how it stands again, and says so where that changed.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let facts = self.updater.facts();
        if facts != self.facts {
            self.facts = facts;
            cx.notify();
        }
    }

    /// Looks now; while an update is under way, Sparkle brings its window forward instead.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.facts.updates() && (self.facts.can_check || self.facts.busy) {
            self.updater.check();
        }
        self.refresh(cx);
    }

    /// Looks in the background (Kuvo on this Mac runs a newer version than this app): not while a
    /// check or an update is under way, or its window is open.
    pub fn check_quietly(&mut self, cx: &mut Context<Self>) {
        if self.facts.updates() && self.facts.can_check && !self.facts.busy {
            self.updater.check_quietly();
        }
        self.refresh(cx);
    }

    pub fn set_checks(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.facts.updates() {
            self.updater.set_checks(on);
        }
        self.refresh(cx);
    }

    pub fn set_downloads(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.facts.updates() && self.facts.checks {
            self.updater.set_downloads(on);
        }
        self.refresh(cx);
    }

    pub fn set_betas(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.facts.updates() {
            self.updater.set_betas(on);
        }
        self.refresh(cx);
    }
}

/// Whether the app looks for its update now, as Kuvo on this Mac says it runs version `said`, newer
/// than this app (`appCheck`): once for each version it says, `checked` noting the last.
pub fn app_check_due(said: Option<&str>, checked: &mut Option<String>) -> bool {
    match said {
        Some(version) if checked.as_deref() != Some(version) => {
            *checked = Some(version.to_owned());
            true
        }
        _ => false,
    }
}

/// Starts the app's updater: Sparkle where this build carries it and names a feed; otherwise
/// one that says this build doesn't update itself.
pub fn start(cx: &mut App) -> Entity<Updates> {
    #[cfg(target_os = "macos")]
    {
        let (tell, changes) = async_channel::unbounded();
        match sparkle::Sparkle::start(tell) {
            Ok(sparkle) => return Updates::install(Rc::new(sparkle), Some(changes), cx),
            Err(why) => return Updates::install(Rc::new(Without { why }), None, cx),
        }
    }
    #[allow(unreachable_code)]
    Updates::install(Rc::new(Without { why: NOT_THIS_BUILD.into() }), None, cx)
}

/// An updater that does what it's told and remembers it, for the stand-in box's app and the tests.
#[cfg(feature = "stand-in")]
pub mod stand_in {
    use std::cell::RefCell;

    use super::{Can, Facts, Updater};

    pub struct StandIn {
        pub facts: RefCell<Facts>,
        /// What it was asked, in order: "check", "checks on", "downloads off", "betas on"…
        pub asked: RefCell<Vec<String>>,
    }

    impl StandIn {
        /// One that updates itself, at `version`, last checked at `checked`.
        pub fn new(version: &str, checked: Option<f64>) -> StandIn {
            let facts = Facts { version: version.into(), can: Can::Update, can_check: true, busy: false, checks: true, downloads: Some(false), betas: super::is_beta(version), checked };
            StandIn { facts: RefCell::new(facts), asked: RefCell::new(Vec::new()) }
        }

        fn said(&self, what: &str, on: bool) {
            self.asked.borrow_mut().push(format!("{what} {}", if on { "on" } else { "off" }));
        }
    }

    impl Updater for StandIn {
        fn facts(&self) -> Facts {
            self.facts.borrow().clone()
        }
        /// A check starts: busy, and none other until it ends ([`StandIn::checked`]).
        fn check(&self) {
            self.asked.borrow_mut().push("check".into());
            let mut facts = self.facts.borrow_mut();
            facts.busy = true;
            facts.can_check = false;
        }
        fn check_quietly(&self) {
            self.asked.borrow_mut().push("check quietly".into());
        }
        fn set_checks(&self, on: bool) {
            self.said("checks", on);
            self.facts.borrow_mut().checks = on;
        }
        fn set_downloads(&self, on: bool) {
            self.said("downloads", on);
            self.facts.borrow_mut().downloads = Some(on);
        }
        fn set_betas(&self, on: bool) {
            self.said("betas", on);
            self.facts.borrow_mut().betas = on;
        }
    }

    impl StandIn {
        /// The check under way ended, at `at`.
        pub fn checked(&self, at: f64) {
            let mut facts = self.facts.borrow_mut();
            facts.busy = false;
            facts.can_check = true;
            facts.checked = Some(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_beta_gets_betas_and_a_release_doesnt_until_you_choose() {
        assert!(wants_betas(None, "0.8.0-beta.2"));
        assert!(!wants_betas(None, "0.8.0"));
        assert!(wants_betas(Some(true), "0.8.0"), "chosen in 0.7, kept");
        assert!(!wants_betas(Some(false), "0.8.0-beta.2"));
        assert_eq!(channels(true), &["beta"]);
        assert!(channels(false).is_empty(), "releases are in no channel");
    }

    #[test]
    fn the_app_looks_for_its_update_once_for_each_newer_kuvo_on_this_mac() {
        let mut checked = None;
        assert!(!app_check_due(None, &mut checked), "Kuvo here isn't ahead: nothing to look for");
        assert!(app_check_due(Some("1.0.5"), &mut checked), "Kuvo here runs 1.0.5, newer than this app");
        assert!(!app_check_due(Some("1.0.5"), &mut checked), "looked once for 1.0.5 already");
        assert!(!app_check_due(None, &mut checked));
        assert!(app_check_due(Some("1.0.6"), &mut checked), "a newer one again");
    }
}
