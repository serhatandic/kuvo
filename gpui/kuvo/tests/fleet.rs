//! The setup's update to its accepted release (`fleetUpdate`), against a stand-in of your
//! computers (nothing real is reached): the title bar's notifier shows until every computer is
//! done or you dismiss it, a new failure brings it back, Update Now shows only where a computer is
//! behind and not progressing and goes to that computer's own box, and a computer whose Kuvo is
//! older than the update routes says so instead of a button that does nothing.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::Home;
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Home>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let boxes = kuvo::home::stand_in_boxes(stand_in);
    let first = boxes.nearest().expect("the nearest box");
    let window = cx.add_window(move |window, cx| Home::new(boxes, first, false, window, cx));
    let home = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(820.)));
    home.update_in(cx, |home, _, cx| {
        home.set_now(NOW, cx);
        home.read_at_once(cx);
    });
    cx.run_until_parked();
    (home, cx)
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

fn workspace(home: &Entity<Home>, cx: &mut VisualTestContext) -> Entity<Workspace> {
    home.read_with(cx, |home, _| home.workspace().clone())
}

/// The list read again, as the box's live stream would bring it.
fn read_again(home: &Entity<Home>, cx: &mut VisualTestContext) {
    home.update_in(cx, |home, _, cx| home.read_at_once(cx));
    cx.run_until_parked();
}

fn computers(home: &Entity<Home>, cx: &mut VisualTestContext) {
    workspace(home, cx).update_in(cx, |workspace, window, cx| workspace.show_area(Area::Computers, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn the_header_says_how_the_setup_updates_and_offers_update_now_only_where_a_computer_is_stuck(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, false, NOW);
    let (home, cx) = open(cx, stand_in);
    assert!(shown(cx, "fleet-chip"), "the setup's update is in the title bar");
    assert!(!shown(cx, "update-chip"), "it takes the old chip's place");
    assert!(!shown(cx, "fleet-chip-failed"), "nothing failed");

    click(cx, "fleet-chip");
    assert!(shown(cx, "fleet-card"));
    // devbox is waiting for its sessions: under way, nothing to press.
    assert!(shown(cx, "fleet-line-card-key_devbox") && !shown(cx, "fleet-card-update-key_devbox"));
    // studio-mini is behind but can't be retried (away), this Mac is done: no Update Now.
    assert!(!shown(cx, "fleet-card-update-key_studio") && !shown(cx, "fleet-card-update-key_stand_in"));
    // worker-01 is behind and the box lets it be retried, but this app doesn't reach it: its page,
    // rather than a button that can't work.
    assert!(!shown(cx, "fleet-card-update-key_more_0") && shown(cx, "fleet-card-show-key_more_0"));
    // old-mini's Kuvo can't be asked: its page, where what brings it up is.
    assert!(shown(cx, "fleet-card-show-key_old"));
    let words = workspace(&home, cx).read_with(cx, |workspace, _| workspace.fleet_rows());
    let line = |key: &str| words.iter().find(|row| row.key == key).map(|row| kuvo::fleet::words(&row.standing.line)).unwrap();
    assert_eq!(line("key_devbox"), "Updating to beta.38 · waiting for 2 sessions to finish");
    assert_eq!(line("key_stand_in"), "beta.38 · Up to date");
}

#[gpui::test]
fn dismissing_hides_the_header_until_a_new_failure_brings_it_back(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, false, NOW);
    let (home, cx) = open(cx, stand_in.clone());
    click(cx, "fleet-chip");
    click(cx, "fleet-card-dismiss");
    let asked = stand_in.seen_at("POST", "kuvo/update/dismiss");
    assert_eq!(asked.len(), 1, "the box that lists your computers is told");
    assert_eq!(asked[0].body.as_ref().unwrap()["target"], json!("0.8.0-beta.38"));
    assert!(!shown(cx, "fleet-chip") && !shown(cx, "fleet-card"), "hidden once dismissed");
    assert!(!shown(cx, "update-chip"), "and the old chip doesn't come back in its place");

    // devbox fails its update: the box says it isn't dismissed any more.
    stand_in.fleet_sample(true, false, NOW);
    read_again(&home, cx);
    assert!(shown(cx, "fleet-chip") && shown(cx, "fleet-chip-failed"), "a failure brings it back");
    click(cx, "fleet-chip");
    assert!(shown(cx, "fleet-card-failed"));
    assert!(shown(cx, "fleet-card-update-key_devbox"), "a failed computer can be updated now");
}

/// devbox's own box answers Update Now for itself, with `can`: whether it may retry now.
fn devbox_failed(stand_in: &StandIn, can: bool) -> StandIn {
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    let failed = json!({ "current": "0.8.0-beta.35", "target": "0.8.0-beta.38", "state": "failed", "reason": "Its health check failed.", "rolledBackTo": "0.8.0-beta.35", "nextRetryAt": (NOW as u64) + 840_000, "failedAt": NOW as u64, "canRetry": can });
    devbox.fleet_as("key_devbox", json!({ "target": "0.8.0-beta.38", "stage": "failed", "done": 1, "updating": 0, "behind": 3, "failed": 1, "dismissed": false, "failures": [] }), failed);
    devbox
}

#[gpui::test]
fn update_now_in_the_title_bar_asks_that_computers_own_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(true, false, NOW);
    let devbox = devbox_failed(&stand_in, true);
    let (_home, cx) = open(cx, stand_in.clone());
    click(cx, "fleet-chip");
    click(cx, "fleet-card-update-key_devbox");
    let asked = devbox.seen_at("POST", "kuvo/update/retry");
    assert_eq!(asked.len(), 1, "devbox's own box is asked");
    assert_eq!(asked[0].body.as_ref().unwrap()["target"], json!("0.8.0-beta.38"));
    assert!(stand_in.seen_at("POST", "kuvo/update/retry").is_empty());
    assert!(!shown(cx, "fleet-card-error"));
}

#[gpui::test]
fn a_refused_update_now_says_why_in_the_boxs_words(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(true, false, NOW);
    // (The list still says it may retry; its own box has since moved on.)
    let devbox = devbox_failed(&stand_in, false);
    let (_home, cx) = open(cx, stand_in);
    click(cx, "fleet-chip");
    click(cx, "fleet-card-update-key_devbox");
    assert_eq!(devbox.seen_at("POST", "kuvo/update/retry").len(), 1);
    assert!(shown(cx, "fleet-card-error"), "the 409's message, never a silent no-op");
}

#[gpui::test]
fn update_now_for_the_windows_own_computer_asks_its_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let target = "0.8.0-beta.38";
    stand_in.fleet_update(
        json!({ "target": target, "stage": "behind", "done": 0, "updating": 0, "behind": 1, "failed": 0, "dismissed": false, "failures": [] }),
        vec![("key_stand_in", json!({ "current": "0.8.0-beta.35", "target": target, "state": "behind", "canRetry": true }))],
    );
    let (home, cx) = open(cx, stand_in.clone());
    computers(&home, cx);
    click(cx, "fleet-update-this");
    assert_eq!(stand_in.seen_at("POST", "kuvo/update/retry").len(), 1);
    assert!(!shown(cx, "manage-problem"));
}

#[gpui::test]
fn the_header_goes_once_every_computer_is_done(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, true, NOW);
    let (_home, cx) = open(cx, stand_in);
    assert!(!shown(cx, "fleet-chip") && !shown(cx, "update-chip"));
}

#[gpui::test]
fn update_now_on_the_computers_page_asks_that_computers_own_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(true, false, NOW);
    let devbox = devbox_failed(&stand_in, true);
    let (home, cx) = open(cx, stand_in.clone());
    computers(&home, cx);
    // Updating, up to date and behind-but-not-stuck have no button; failed does.
    assert!(shown(cx, "fleet-update-key_devbox"));
    assert!(!shown(cx, "fleet-update-this") && !shown(cx, "fleet-update-key_studio"));

    click(cx, "fleet-update-key_devbox");
    let asked = devbox.seen_at("POST", "kuvo/update/retry");
    assert_eq!(asked.len(), 1, "devbox's own box is asked");
    assert_eq!(asked[0].body.as_ref().unwrap()["target"], json!("0.8.0-beta.38"));
    assert!(stand_in.seen_at("POST", "kuvo/update/retry").is_empty(), "never this Mac's box instead");
}

#[gpui::test]
fn a_computer_this_app_doesnt_reach_says_so_in_place_of_update_now(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, false, NOW);
    let (home, cx) = open(cx, stand_in);
    computers(&home, cx);
    // (worker-01 has no box this app reaches.)
    assert!(!shown(cx, "fleet-update-key_more_0") && shown(cx, "fleet-unreached-key_more_0"), "never a button that does nothing");
}

#[gpui::test]
fn a_computer_on_an_older_kuvo_shows_its_version_behind_and_needs_an_update_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, false, NOW);
    let (home, cx) = open(cx, stand_in);
    computers(&home, cx);
    assert!(shown(cx, "fleet-line-key_old"), "behind, from what it does say");
    assert!(shown(cx, "fleet-first-key_old") && !shown(cx, "fleet-update-key_old"), "why, in place of a button that can't work");
    click(cx, "fleet-chip");
    let rows = workspace(&home, cx).read_with(cx, |workspace, _| workspace.fleet_rows());
    let old = rows.iter().find(|row| row.key == "key_old").expect("old-mini is listed");
    assert!(old.standing.retry_notice.as_deref() == Some("Kuvo on old-mini needs an update first") && !old.standing.can_update);
    assert_eq!(kuvo::fleet::words(&old.standing.line), "beta.38 available");
}
