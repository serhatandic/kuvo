//! Runs on (docs/architecture/message-delivery.md, "Removing the computer picker"): a new session's
//! composer, and New Bot, say which of your computers it runs on, as the nearest box lists them
//! (`kuvo/setup/runs-on/events`). This computer first; one that's away says when it was last seen
//! and can't be picked; a start goes to the box of the computer picked, saying so. The boxes are the
//! stand-in and its peer (`devbox`); clicks and typing go through the window.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;
const MINUTE: f64 = 60_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1600.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| workspace.new_session(None, window, cx));
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn runs_on(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.landing().read(cx).runs_on().map(str::to_owned))
}

fn write(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, words: &str) {
    let field = workspace.read_with(cx, |workspace, cx| workspace.landing().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input(words);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

#[gpui::test]
fn a_new_session_runs_on_the_computer_picked_and_starts_on_its_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    let (workspace, cx) = open(cx, stand_in.clone());
    // One choice of computer, beside the model: this one first.
    assert!(shown(cx, "runs-on-chip") && shown(cx, "model-chip"));
    assert_eq!(runs_on(&workspace, cx), None);
    click(cx, "runs-on-chip");
    assert!(shown(cx, "menu-item-This Mac") && shown(cx, "menu-item-devbox"));
    click(cx, "menu-item-devbox");
    assert_eq!(runs_on(&workspace, cx).as_deref(), Some("key_devbox"));
    // What it can start on is devbox's to say: read there.
    assert!(!devbox.seen_at("GET", "kuvo/models/offered").is_empty(), "its models are read on devbox");

    let here_before = stand_in.seen_at("POST", "kuvo/sessions").len();
    write(&workspace, cx, "Look at the deploy logs");
    let started = devbox.seen_at("POST", "kuvo/sessions");
    assert_eq!(started.len(), 1, "started on devbox's box");
    assert_eq!(started[0].body.as_ref().unwrap()["computer"], json!("key_devbox"), "the start says where it's for");
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions").len(), here_before, "nothing started on this computer");
    // It opens, read where it runs.
    let opened = workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).expect("the new session opens");
    let client = workspace.read_with(cx, |workspace, _| workspace.client_for(&opened)).expect("its computer's box");
    assert!(!client.here(), "its conversation is devbox's");
}

#[gpui::test]
fn an_away_computer_says_when_it_was_seen_and_cant_be_picked(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    stand_in.setup_away("key_devbox", NOW - 3.0 * 60.0 * MINUTE, false);
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "runs-on-chip");
    assert!(shown(cx, "menu-item-devbox"), "still listed");
    let rows = workspace.read_with(cx, |workspace, _| workspace.runs_on().to_vec());
    assert!(rows.iter().any(|row| row.computer.id == "key_devbox" && row.away && row.last_seen == Some(NOW - 3.0 * 60.0 * MINUTE)));
    let seen = devbox.seen().len();
    click(cx, "menu-item-devbox");
    assert_eq!(runs_on(&workspace, cx), None, "this computer stays picked");
    assert_eq!(devbox.seen().len(), seen, "an away computer isn't asked");
}

#[gpui::test]
fn with_one_computer_there_is_no_runs_on(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_, cx) = open(cx, stand_in);
    assert!(shown(cx, "model-chip"));
    assert!(!shown(cx, "runs-on-chip"));
}

#[gpui::test]
fn a_new_bot_is_made_on_the_computer_picked_in_runs_on(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    let (_, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    click(cx, "new-bot");
    click(cx, "new-runs-on-menu");
    click(cx, "menu-item-devbox");
    assert!(!devbox.seen_at("GET", "kuvo/bots/templates").is_empty(), "what a bot can be there is devbox's to say");
    click(cx, "new-name-field");
    cx.simulate_input("Watcher");
    let here_before = stand_in.seen_at("POST", "kuvo/bots").len();
    click(cx, "new-create");
    assert_eq!(devbox.seen_at("POST", "kuvo/bots").len(), 1, "made on devbox's box");
    assert_eq!(stand_in.seen_at("POST", "kuvo/bots").len(), here_before, "not on this computer");
}
