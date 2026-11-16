//! Settings › Updates as someone uses it: the app's version, Check Now, checking and downloading
//! by itself, betas, and where Kuvo on this Mac's own version is. The app's updater (Sparkle in a
//! bundle) is outside the app, so it's stood in for (`updates::stand_in`); it says when it changed
//! by itself through the same channel Sparkle's delegate does. Clicks go through the window.

use std::rc::Rc;

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::settings::SettingsView;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::updates::Updates;
use kuvo::updates::stand_in::StandIn as Updater;
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;
const HOUR: f64 = 3_600_000.0;

struct Opened<'a> {
    workspace: Entity<Workspace>,
    cx: &'a mut VisualTestContext,
}

/// The window, with the app's updater set as `updater` says (none: a build that doesn't update
/// itself), at Settings › Updates.
fn open(cx: &mut TestAppContext, updater: Option<(Rc<Updater>, async_channel::Receiver<()>)>) -> Opened<'_> {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
        if let Some((updater, changes)) = updater {
            Updates::install(updater, Some(changes), cx);
        }
    });
    let client = BoxClient::new(StandIn::at(Replies::Never, NOW));
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1200.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    click(cx, "rail-settings");
    click(cx, "setting-updates");
    Opened { workspace, cx }
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

fn settings(opened: &mut Opened) -> Entity<SettingsView> {
    opened.workspace.read_with(opened.cx, |workspace, cx| workspace.area_view(Area::Settings).and_then(|view| view.read(cx).settings().cloned()).expect("Settings"))
}

fn detail(opened: &mut Opened, key: &str) -> Option<String> {
    let view = settings(opened);
    view.read_with(opened.cx, |view, _| view.row_detail(key))
}

fn updater(version: &str) -> (Rc<Updater>, async_channel::Sender<()>, async_channel::Receiver<()>) {
    let (tell, changes) = async_channel::unbounded();
    (Rc::new(Updater::new(version, Some(NOW - HOUR))), tell, changes)
}

#[gpui::test]
fn updates_says_the_version_and_checks_now(cx: &mut TestAppContext) {
    let (updater, tell, changes) = updater("0.8.0");
    let mut opened = open(cx, Some((updater.clone(), changes)));
    // Under your own settings: the app's, on this device.
    assert!(shown(opened.cx, "settings-yours-title"));
    assert_eq!(detail(&mut opened, "row-version").as_deref(), Some("Last checked 1 hour ago."));
    assert!(!shown(opened.cx, "row-version-badge"), "a release has no Beta mark");
    // Check Now: the updater looks (its own window shows what it finds), and the button waits.
    click(opened.cx, "row-version-check-now");
    assert_eq!(*updater.asked.borrow(), ["check"]);
    assert_eq!(detail(&mut opened, "row-version").as_deref(), Some("Checking for updates…"));
    click(opened.cx, "row-version-check-now");
    assert_eq!(updater.asked.borrow().len(), 1, "not again while it's checking");
    // It ends by itself, and says so as Sparkle does: the page follows.
    updater.checked(NOW);
    tell.try_send(()).unwrap();
    opened.cx.run_until_parked();
    assert_eq!(detail(&mut opened, "row-version").as_deref(), Some("Last checked just now."));
    click(opened.cx, "row-version-check-now");
    assert_eq!(updater.asked.borrow().len(), 2);
}

#[gpui::test]
fn checking_downloading_and_betas_are_switched_here(cx: &mut TestAppContext) {
    let (updater, _tell, changes) = updater("0.8.0");
    let mut opened = open(cx, Some((updater.clone(), changes)));
    // Downloading by itself, then off again.
    click(opened.cx, "row-update-downloads-switch");
    click(opened.cx, "row-update-downloads-switch");
    // Betas on: a release gets none until you say.
    click(opened.cx, "row-update-betas-switch");
    // Checking off: downloading by itself goes with it, and says why it can't be switched.
    click(opened.cx, "row-update-checks-switch");
    assert_eq!(*updater.asked.borrow(), ["downloads on", "downloads off", "betas on", "checks off"]);
    let view = settings(&mut opened);
    assert_eq!(view.read_with(opened.cx, |view, _| view.row_why("row-update-downloads")).as_deref(), Some("Turn on automatic checks first."));
    click(opened.cx, "row-update-downloads-switch");
    assert_eq!(updater.asked.borrow().len(), 4, "a switch that can't be switched sends nothing");
    click(opened.cx, "row-update-checks-switch");
    assert_eq!(updater.asked.borrow().last().map(String::as_str), Some("checks on"));
}

#[gpui::test]
fn a_beta_is_marked_and_gets_betas(cx: &mut TestAppContext) {
    let (updater, _tell, changes) = updater("0.8.0-beta.2");
    let opened = open(cx, Some((updater.clone(), changes)));
    assert!(shown(opened.cx, "row-version-badge"));
    // On already, unless you chose otherwise: turning it off is what's sent.
    click(opened.cx, "row-update-betas-switch");
    assert_eq!(*updater.asked.borrow(), ["betas off"]);
}

#[gpui::test]
fn a_build_that_doesnt_update_itself_says_so_and_offers_nothing(cx: &mut TestAppContext) {
    let mut opened = open(cx, None);
    assert_eq!(detail(&mut opened, "row-version").as_deref(), Some("This build doesn’t update itself."));
    for control in ["row-version-check-now", "row-update-checks-switch", "row-update-downloads-switch", "row-update-betas-switch"] {
        assert!(!shown(opened.cx, control), "{control}");
    }
}

#[gpui::test]
fn kuvo_on_this_mac_has_its_own_version_under_this_mac(cx: &mut TestAppContext) {
    let (updater, _tell, changes) = updater("0.8.0");
    let mut opened = open(cx, Some((updater, changes)));
    click(opened.cx, "row-update-computer-open");
    let view = settings(&mut opened);
    assert_eq!(view.read_with(opened.cx, |view, _| view.place()), "box//computer");
    assert!(shown(opened.cx, "settings-computer"), "the computer's settings, at This Mac");
}

#[gpui::test]
fn the_search_finds_updates_from_the_computers_side(cx: &mut TestAppContext) {
    let (updater, _tell, changes) = updater("0.8.0");
    let mut opened = open(cx, Some((updater, changes)));
    // On the computer's side, "beta" lists Updates' row on yours, and goes there.
    click(opened.cx, "settings-foot");
    click(opened.cx, "settings-search");
    opened.cx.simulate_input("beta");
    opened.cx.run_until_parked();
    assert!(shown(opened.cx, "settings-hit-0"));
    click(opened.cx, "settings-hit-0");
    let view = settings(&mut opened);
    assert_eq!(view.read_with(opened.cx, |view, _| view.place()), "yours/updates");
    assert!(shown(opened.cx, "row-update-betas"));
}
