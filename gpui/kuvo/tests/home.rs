//! The window over your boxes (`kuvo::Home`), against a stand-in of two computers (nothing real
//! is reached): one window shows every computer's sessions, with no computer to pick; another
//! computer's opens in it, and what's written there goes to its box alone.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
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
fn one_window_shows_every_computers_sessions_and_a_message_to_another_goes_to_its_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    let (home, cx) = open(cx, stand_in.clone());
    // No computer to pick: this computer's sessions and devbox's, side by side.
    assert!(!shown(cx, "computer-switcher"), "the title bar has no computer picker");
    assert!(shown(cx, "session-project-ses_limits") && shown(cx, "session-project-ses_releases"));
    assert_eq!(name(&home, cx), "This Mac");
    let window = workspace(&home, cx);

    // A link to devbox's session (a notification, Activity) opens it here, in the same window.
    window.update(cx, |_, cx| cx.emit(kuvo::workspace::Switch::Session { computer: "key_devbox".into(), session: "ses_releases".into() }));
    cx.run_until_parked();
    assert_eq!(workspace(&home, cx), window, "the same window");
    window.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_releases", window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "entry-msg_d2"), "its conversation, from its own box");
    // What's written there goes to devbox's box, never this computer's.
    let field = window.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Ship the rounding fix first.");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = devbox.seen_at("POST", "kuvo/sessions/ses_releases/send");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body.clone().unwrap()["words"], json!("Ship the rounding fix first."));
    assert!(stand_in.seen().iter().all(|seen| !seen.path.contains("ses_releases/")), "nothing of it was asked of this computer's box");
}

#[gpui::test]
fn computers_shows_a_quiet_pairing_check_then_the_boxs_readd_action(cx: &mut TestAppContext) {
    use kuvo_box::{Refusal, boxes::Pairing};
    let (home, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let workspace = workspace(&home, cx);
    let show = |state, cx: &mut gpui::VisualTestContext| {
        workspace.update_in(cx, |workspace, window, cx| {
            let mut switching = workspace.switching().cloned().expect("the switcher's computers");
            switching.pairing = state;
            workspace.set_switching(Some(switching), cx);
            workspace.show_area(kuvo::area::Area::Computers, window, cx);
        });
        cx.run_until_parked();
    };
    show(Pairing::Checking("Checking this app with your other computers…".into()), cx);
    assert!(shown(cx, "pairing-check"));
    assert!(!shown(cx, "notice-pairing"));
    show(Pairing::Paired, cx);
    assert!(!shown(cx, "pairing-check"));
    let body = br#"{"reason":"pairing-readd","title":"Title from the box","message":"Message from the box","action":{"label":"Pair Again","route":"kuvo/devices/pair"}}"#;
    show(Pairing::Readd(Refusal::from_answer(409, body)), cx);
    let page = workspace.read_with(cx, |workspace, cx| workspace.area_view(kuvo::area::Area::Computers).and_then(|view| view.read(cx).computers_page().cloned()).expect("Computers"));
    assert_eq!(page.read_with(cx, |page, _| page.said("notice-pairing")), Some(("Title from the box".into(), "Message from the box".into())));
    assert!(shown(cx, "notice-pairing"));
    assert!(shown(cx, "notice-pairing-action"));
    click(cx, "notice-pairing-action");
    cx.run_until_parked();
    assert!(!shown(cx, "notice-pairing-action"), "the button ran the existing pairing flow");
}

#[gpui::test]
fn when_this_apps_computer_was_blocked_the_boxs_words_show_whole_on_computers(cx: &mut TestAppContext) {
    // The nearest box refuses to pair this app, in the main computer's words (its app was removed,
    // and its box may add none until it's allowed again).
    let words = "Kuvo on MacBook was removed from your devices, so MacBook can’t add Kuvo Code’s app by itself now. Allow it again from Kuvo Code on Mac mini or another computer’s app: Computers, MacBook.";
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

/// A held local box without a Box-supplied action has no update control.
#[gpui::test]
fn a_held_local_box_without_an_offer_has_no_update_action(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_older(true);
    let (home, cx) = open(cx, stand_in.clone());
    let chip = workspace(&home, cx).read_with(cx, |ws, _| ws.update_chip()).expect("held notice");
    assert!(chip.action.is_none());
    assert!(!shown(cx, "blocked-action"));
    assert!(stand_in.seen_at("POST", "kuvo/update").is_empty());
}

/// devbox's box older than this app, which the box that lists your computers brings up to date:
/// the window opened on devbox.
fn on_an_older_devbox<'a>(cx: &'a mut TestAppContext, state: &'static str, working: usize) -> (StandIn, Entity<Home>, &'a mut VisualTestContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.peer_box("key_devbox").expect("devbox").set_older(true);
    stand_in.bring_up_to_date("key_devbox", state, false, working);
    let (home, cx) = open(cx, stand_in.clone());
    let rail_here = bounds(cx, "rail-sessions").expect("the rail");
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    assert_eq!(name(&home, cx), "devbox");
    // The rail stays where it was; what it says is in the title bar, and in the view in its areas.
    assert_eq!(bounds(cx, "rail-sessions").map(|at| at.origin), Some(rail_here.origin));
    (stand_in, home, cx)
}

/// devbox's box older than this app, which the box that lists your computers doesn't bring up to
/// date itself (from before it did, or one it can't): the window opened on devbox.
fn on_devbox_held<'a>(cx: &'a mut TestAppContext, stand_in: &StandIn) -> (Entity<Home>, &'a mut VisualTestContext) {
    stand_in.peer_box("key_devbox").expect("devbox").set_older(true);
    let (home, cx) = open(cx, stand_in.clone());
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    assert_eq!(name(&home, cx), "devbox");
    (home, cx)
}

fn computers_page(home: &Entity<Home>, cx: &mut VisualTestContext) -> Entity<kuvo::computers::ComputersView> {
    let ws = workspace(home, cx);
    ws.read_with(cx, |workspace, cx| workspace.area_view(kuvo::area::Area::Computers).and_then(|view| view.read(cx).computers_page().cloned())).expect("Computers' page")
}

#[gpui::test]
fn this_macs_kuvo_waiting_for_its_release_says_so_in_the_chip_and_updates_now_on_your_word(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.own_update_waits("waiting", 2);
    let (home, cx) = open(cx, stand_in.clone());
    assert!(shown(cx, "update-chip"));
    let chip = workspace(&home, cx).read_with(cx, |workspace, _| workspace.update_chip()).expect("the chip");
    assert!(chip.title.ends_with("updates once your sessions there finish."), "{}", chip.title);
    assert_eq!(kuvo::title_bar::chip_label(&chip), "Update Ready");
    click(cx, "update-chip");
    click(cx, "update-now");
    // Sessions are at work here: it asks first.
    assert!(shown(cx, "update-confirm"));
    assert!(stand_in.seen_at("POST", "kuvo/update").is_empty());
    click(cx, "update-confirm");
    let asked = stand_in.seen_at("POST", "kuvo/update");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].body.as_ref().map(|body| body["now"].clone()), Some(json!(true)));
    // Under way: a spinner, nothing to press.
    let chip = workspace(&home, cx).read_with(cx, |workspace, _| workspace.update_chip()).expect("the chip");
    assert!(chip.updating && chip.action.is_none());
    assert_eq!(kuvo::title_bar::chip_label(&chip), "Updating…");
    click(cx, "update-chip");
    assert!(shown(cx, "update-card-title") && !shown(cx, "update-now"));
}
