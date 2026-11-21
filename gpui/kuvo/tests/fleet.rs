//! The notifier and retry actions supplied by the box.

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
fn the_header_draws_preparing_ready_off_and_current_rows(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, false, NOW);
    let (home, cx) = open(cx, stand_in);
    assert!(shown(cx, "fleet-chip"), "the setup's update is in the title bar");
    assert!(!shown(cx, "update-chip"), "it takes the old chip's place");
    assert!(!shown(cx, "fleet-chip-failed"), "nothing failed");

    click(cx, "fleet-chip");
    assert!(shown(cx, "fleet-card"));
    assert!(shown(cx, "fleet-line-card-key_devbox") && !shown(cx, "fleet-card-update-key_devbox"));
    assert!(!shown(cx, "fleet-card-update-key_studio"));
    assert!(!shown(cx, "fleet-card-update-key_stand_in"));
    assert!(!shown(cx, "fleet-card-update-key_more_0"));
    assert!(!shown(cx, "fleet-card-dismiss"));
    let words = workspace(&home, cx).read_with(cx, |workspace, _| workspace.fleet_rows());
    let line = |key: &str| words.iter().find(|row| row.key == key).map(|row| kuvo::fleet::words(&row.standing.line)).unwrap();
    assert_eq!(line("key_devbox"), "Getting beta.38 ready…");
    assert_eq!(line("key_stand_in"), "beta.38 · Up to date");
}

#[gpui::test]
fn the_box_can_hide_the_notifier_and_show_a_fixable_failure(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, true, NOW);
    let (home, cx) = open(cx, stand_in.clone());
    assert!(!shown(cx, "fleet-chip"));
    stand_in.fleet_sample(true, false, NOW);
    read_again(&home, cx);
    assert!(shown(cx, "fleet-chip"));
    click(cx, "fleet-chip");
    assert!(shown(cx, "fleet-card-update-key_devbox"));
}

/// devbox's own box answers Update Now for itself, with `can`: whether it may retry now.
fn devbox_failed(stand_in: &StandIn, can: bool) -> StandIn {
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    let failed = json!({ "current": "0.8.0-beta.35", "target": "0.8.0-beta.38", "state": "cannot", "reason": "There isn’t room.", "fixable": true, "canRetry": can });
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
        vec![("key_stand_in", json!({ "current": "0.8.0-beta.35", "target": target, "state": "cannot", "reason": "There isn’t room.", "fixable": true, "canRetry": true }))],
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
fn an_off_computer_has_no_retry_action(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(false, false, NOW);
    let (home, cx) = open(cx, stand_in);
    computers(&home, cx);
    assert!(!shown(cx, "fleet-update-key_studio"));
    let rows = workspace(&home, cx).read_with(cx, |workspace, _| workspace.fleet_rows());
    let off = rows.iter().find(|row| row.key == "key_studio").expect("studio-mini is listed");
    assert_eq!(kuvo::fleet::words(&off.standing.line), "beta.35 · Off: updates when it’s back");
}

/// Each row's words, by key, as the notifier and the Computers page draw them.
fn lines(home: &Entity<Home>, cx: &mut VisualTestContext) -> Vec<(String, String, Option<&'static str>)> {
    let rows = workspace(home, cx).read_with(cx, |workspace, _| workspace.fleet_rows());
    rows.iter().map(|row| (row.key.clone(), kuvo::fleet::words(&row.standing.line), kuvo::fleet::short_words(&row.standing.line))).collect()
}

fn line_of(lines: &[(String, String, Option<&'static str>)], key: &str) -> (String, Option<&'static str>) {
    lines.iter().find(|(each, _, _)| each == key).map(|(_, words, short)| (words.clone(), *short)).unwrap_or_else(|| panic!("{key} is listed"))
}

#[gpui::test]
fn states_that_need_nothing_offer_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (target, old) = ("0.8.0-beta.38", "0.8.0-beta.35");
    stand_in.more_computers(1);
    stand_in.fleet_update(
        json!({ "target": target, "stage": "downloading", "done": 1, "updating": 1, "behind": 1, "failed": 1, "dismissed": false, "failures": [],
            "shown": true, "warning": false, "title": "Updating to beta.38 · 1 of 3 ready", "detail": "Everything works on beta.35 while your computers get ready." }),
        vec![
            // (Ahead of the release: up to date, on what it runs.)
            ("key_stand_in", json!({ "current": "0.8.0-beta.40", "target": target, "state": "up-to-date", "canRetry": false })),
            ("key_devbox", json!({ "current": old, "target": target, "state": "getting-ready", "canRetry": false })),
            ("key_studio", json!({ "current": old, "target": target, "state": "off", "canRetry": false })),
            ("key_more_0", json!({ "current": old, "target": target, "state": "cannot", "reason": "beta.38 doesn’t run here.", "fixable": false, "canRetry": false })),
        ],
    );
    let (home, cx) = open(cx, stand_in);
    let title = workspace(&home, cx).read_with(cx, |workspace, _| workspace.fleet_header().map(kuvo::fleet::summary_words));
    assert_eq!(title.as_deref(), Some("Updating to beta.38 · 1 of 3 ready"));
    click(cx, "fleet-chip");
    assert!(shown(cx, "fleet-card-detail") && !shown(cx, "fleet-card-detail-warning"), "the line under the title is quiet while computers get ready");
    let lines = lines(&home, cx);
    assert_eq!(line_of(&lines, "key_stand_in"), ("beta.40 · Up to date".into(), None), "never an older version as available");
    assert_eq!(line_of(&lines, "key_devbox"), ("Getting beta.38 ready…".into(), None));
    assert_eq!(line_of(&lines, "key_studio"), ("beta.35 · Off: updates when it’s back".into(), None));
    assert_eq!(line_of(&lines, "key_more_0"), ("beta.35 · beta.38 doesn’t run here. · Updates with the next release".into(), None));
    for key in ["key_stand_in", "key_devbox", "key_studio", "key_more_0"] {
        assert!(!shown(cx, &format!("fleet-card-update-{key}")), "{key}: nothing to press");
    }
    assert!(!shown(cx, "fleet-card-dismiss"), "the notifier hides itself");
    assert!(shown(cx, "fleet-card-open"), "Open Computers stays");
    // On the Computers page too: no Update now, no Try Again.
    click(cx, "fleet-card-open");
    for id in ["this", "key_devbox", "key_studio", "key_more_0"] {
        assert!(!shown(cx, &format!("fleet-update-{id}")), "{id}");
    }
}

#[gpui::test]
fn a_fixable_cannot_names_the_computer_and_offers_try_again_alone(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.fleet_sample(true, false, NOW);
    let (home, cx) = open(cx, stand_in);
    let title = workspace(&home, cx).read_with(cx, |workspace, _| workspace.fleet_header().map(kuvo::fleet::summary_words));
    assert_eq!(title.as_deref(), Some("devbox can’t update to beta.38"));
    click(cx, "fleet-chip");
    assert!(shown(cx, "fleet-card-detail-warning"), "the reason, in amber, where you can fix it");
    let lines = lines(&home, cx);
    assert_eq!(line_of(&lines, "key_devbox"), ("beta.35 · There isn’t room to prepare the release.".into(), Some("Can’t update")));
    assert_eq!(line_of(&lines, "key_more_0"), ("Ready for beta.38".into(), None));
    assert!(shown(cx, "fleet-card-update-key_devbox"));
    for key in ["key_stand_in", "key_studio", "key_more_0"] {
        assert!(!shown(cx, &format!("fleet-card-update-{key}")), "{key}");
    }
}
