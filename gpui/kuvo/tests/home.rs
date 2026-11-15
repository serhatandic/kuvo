//! The window over your boxes (`kuvo::Home`), against a stand-in of two computers (nothing real
//! is reached): the title bar's switcher lists your computers as this app reaches them, picking
//! one shows that computer's own sessions, and what's written there goes to its box alone.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::Home;
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
    cx.simulate_resize(size(px(1200.), px(800.)));
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

fn name(home: &Entity<Home>, cx: &mut VisualTestContext) -> String {
    workspace(home, cx).read_with(cx, |workspace, _| workspace.box_name().to_string())
}

#[gpui::test]
fn the_switcher_shows_another_computers_sessions_and_a_message_written_there_goes_to_its_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    let (home, cx) = open(cx, stand_in.clone());
    // It opens with the box on this computer: its sessions, none of the other computer's.
    assert!(shown(cx, "session-project-ses_limits") && !shown(cx, "session-project-ses_releases"));

    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    assert_eq!(name(&home, cx), "devbox");
    assert_eq!(home.read_with(cx, |home, _| home.shown_key()).as_deref(), Some("key_devbox"));
    // The sidebar is devbox's own list, read from devbox's box.
    assert!(shown(cx, "project-Billing") && shown(cx, "session-project-ses_releases"));
    assert!(!shown(cx, "session-project-ses_limits") && !shown(cx, "project-Website"));

    // A session there opens, and what's written goes to devbox's box, never this computer's.
    let shown_there = workspace(&home, cx);
    shown_there.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_releases", window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "entry-msg_d2"), "its conversation, from its own box");
    let field = shown_there.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Ship the rounding fix first.");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = devbox.seen_at("POST", "kuvo/sessions/ses_releases/send");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body.clone().unwrap()["words"], json!("Ship the rounding fix first."));
    assert!(stand_in.seen().iter().all(|seen| !seen.path.contains("ses_releases")), "nothing of it was asked of this computer's box");
}

#[gpui::test]
fn going_back_finds_this_computers_window_as_it_was_left(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (home, cx) = open(cx, stand_in);
    // A conversation open here, with words half written.
    let here = workspace(&home, cx);
    here.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
    cx.run_until_parked();
    let field = here.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("half a thought");

    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    assert_eq!(name(&home, cx), "devbox");
    assert!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.is_landing()), "the other computer opens on its own landing view");

    // Back: this computer by its own name in the list, its conversation and draft as they were.
    click(cx, "computer-switcher");
    assert!(shown(cx, "menu-item-devbox"), "the computer shown is the one checked");
    let back = home.read_with(cx, |home, _| home.boxes().nearest_key()).expect("the nearest box's key");
    home.update_in(cx, |home, window, cx| assert!(home.switch_to(&back, window, cx)));
    cx.run_until_parked();
    assert_eq!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).as_deref(), Some("ses_limits"));
    assert_eq!(field.read_with(cx, |field, _| field.text().to_owned()), "half a thought");
    assert_eq!(workspace(&home, cx), here, "the same workspace, not a new one");
}

#[gpui::test]
fn a_computer_this_app_doesnt_reach_says_why_and_cant_be_picked(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (home, cx) = open(cx, stand_in);
    click(cx, "computer-switcher");
    // One that's away, one that's older, and one that isn't yours: listed, each saying why.
    for (item, key) in [("menu-item-studio-mini", "key_studio"), ("menu-item-build-box", "key_build")] {
        assert!(shown(cx, item), "{item} is listed");
        click(cx, item);
        assert_ne!(home.read_with(cx, |home, _| home.shown_key()).as_deref(), Some(key), "{item} isn't picked");
        if !shown(cx, item) {
            click(cx, "computer-switcher");
        }
    }
    assert_eq!(name(&home, cx), "This Mac");
    // Asked directly, it's refused too: no window is ever made for a computer that isn't yours.
    home.update_in(cx, |home, window, cx| assert!(!home.switch_to("key_build", window, cx)));
}

#[gpui::test]
fn when_this_apps_computer_was_blocked_the_boxs_words_show_whole_on_computers(cx: &mut TestAppContext) {
    // The nearest box refuses to pair this app, in the main computer's words (its app was removed,
    // and its box may add none until it's allowed again).
    let words = "Kuvo on MacBook was removed from your devices, so MacBook can’t add Kuvo Code’s app by itself now. Allow it again from Mac mini or one of your devices: Computers, MacBook.";
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (home, cx) = open(cx, stand_in);
    let workspace = workspace(&home, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let mut switching = workspace.switching().cloned().expect("the switcher's computers");
        switching.pairing = kuvo_box::boxes::Pairing::Refused(words.into());
        workspace.set_switching(Some(switching), cx);
        workspace.show_area(kuvo::area::Area::Computers, window, cx);
    });
    cx.run_until_parked();
    let page = workspace.read_with(cx, |workspace, cx| workspace.area_view(kuvo::area::Area::Computers).and_then(|view| view.read(cx).computers_page().cloned()).expect("Computers"));
    assert_eq!(page.read_with(cx, |page, _| page.said("notice-pairing")), Some(("This app doesn’t reach your other computers yet".to_owned(), words.to_owned())));
    assert!(shown(cx, "notice-pairing"));
}
