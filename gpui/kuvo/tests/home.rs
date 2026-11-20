//! The window over your boxes (`kuvo::Home`), against a stand-in of two computers (nothing real
//! is reached): the title bar's switcher lists your computers as this app reaches them, picking
//! one shows that computer's own sessions, and what's written there goes to its box alone.

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
    // One that's away: listed, saying why.
    for (item, key) in [("menu-item-studio-mini", "key_studio")] {
        assert!(shown(cx, item), "{item} is listed");
        click(cx, item);
        assert_ne!(home.read_with(cx, |home, _| home.shown_key()).as_deref(), Some(key), "{item} isn't picked");
        if !shown(cx, item) {
            click(cx, "computer-switcher");
        }
    }
    assert_eq!(name(&home, cx), "This Mac");
    // Asked directly, it's refused too: no window is made for a computer this app doesn't reach.
    home.update_in(cx, |home, window, cx| assert!(!home.switch_to("key_studio", window, cx)));
}

#[gpui::test]
fn an_open_computer_menu_clears_its_checking_status_when_the_pairing_is_confirmed(cx: &mut TestAppContext) {
    use kuvo_box::boxes::Pairing;
    let (home, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let workspace = workspace(&home, cx);
    let set_pairing = |pairing, cx: &mut VisualTestContext| {
        workspace.update_in(cx, |workspace, _, cx| {
            let mut switching = workspace.switching().cloned().unwrap();
            switching.pairing = pairing;
            workspace.set_switching(Some(switching), cx);
        });
        cx.run_until_parked();
    };
    set_pairing(Pairing::Checking("Checking this app with your other computers…".into()), cx);
    click(cx, "computer-switcher");
    let checking = "menu-item-Checking this app with your other computers…";
    assert!(shown(cx, checking));
    set_pairing(Pairing::Paired, cx);
    assert!(shown(cx, "menu-item-devbox"), "the menu stays open");
    assert!(!shown(cx, checking));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "menu-item-devbox"), "the open menu still has keyboard focus");
}

#[gpui::test]
fn the_computer_menu_offers_pair_again_from_a_computers_reply_for_a_saved_pairing(cx: &mut TestAppContext) {
    use kuvo_box::{Refusal, boxes::{Pairing, Reach}};
    let (home, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let workspace = workspace(&home, cx);
    workspace.update_in(cx, |workspace, _, cx| {
        let mut switching = workspace.switching().cloned().unwrap();
        switching.pairing = Pairing::Paired;
        let devbox = switching.rows.iter_mut().find(|row| row.key() == "key_devbox").unwrap();
        devbox.reach = Reach::Readd(Refusal::from_answer(409, br#"{"reason":"pairing-readd","message":"Pair this app again","action":{"label":"Pair Again","route":"kuvo/devices/pair"}}"#));
        workspace.set_switching(Some(switching), cx);
    });
    click(cx, "computer-switcher");
    assert!(shown(cx, "menu-item-devbox"));
    click(cx, "menu-item-Pair Again");
    assert!(!shown(cx, "menu-item-Pair Again"));
    assert_ne!(workspace.read_with(cx, |workspace, _| workspace.switching().unwrap().pairing.clone()), Pairing::Paired);
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

/// This Mac's own Kuvo held for its update (the user's beta.30): your other computers stay listed
/// and usable, each its own box whatever this one's version.
#[gpui::test]
fn a_held_this_mac_still_lists_and_opens_your_other_computers(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_older(true);
    let (home, cx) = open(cx, stand_in.clone());
    assert!(shown(cx, "blocked-held"), "this Mac's own areas wait for its update");
    click(cx, "computer-switcher");
    assert!(shown(cx, "menu-item-devbox"), "devbox, as this Mac's list has it");
    click(cx, "menu-item-devbox");
    assert_eq!(name(&home, cx), "devbox");
    assert!(!shown(cx, "blocked-held"));
    assert!(shown(cx, "session-project-ses_releases"), "devbox's own sessions");
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

#[gpui::test]
fn an_older_computer_being_updated_says_so_in_the_title_bar_in_the_boxs_words(cx: &mut TestAppContext) {
    let (_, home, cx) = on_an_older_devbox(cx, "updating", 0);
    assert!(shown(cx, "update-chip"));
    assert!(!shown(cx, "held-box-notice"), "no strip across the window");
    assert_eq!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.held_notice()).map(|(title, _)| title).as_deref(), Some("Updating Kuvo on devbox…"));
    click(cx, "update-chip");
    assert!(shown(cx, "update-card") && shown(cx, "update-card-title"));
    // While it runs there's nothing to press.
    assert!(!shown(cx, "update-now"));
}

#[gpui::test]
fn update_now_with_work_running_there_asks_first_then_asks_the_box_that_lists_it(cx: &mut TestAppContext) {
    let (stand_in, home, cx) = on_an_older_devbox(cx, "waiting", 2);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    click(cx, "update-chip");
    click(cx, "update-now");
    // Two sessions are at work there: it asks first, and nothing is asked of a box yet.
    let ws = workspace(&home, cx);
    assert!(ws.read_with(cx, |workspace, _| workspace.update_card_confirming()));
    assert!(shown(cx, "update-confirm"));
    assert!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/update").is_empty());
    click(cx, "update-confirm");
    // Asked of the box that lists your computers, never of devbox's own (held) box.
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/update").len(), 1);
    assert!(devbox.seen().iter().all(|seen| !seen.path.contains("/update")));
    // The list says it's under way, and the chip with it.
    assert!(!shown(cx, "update-card"));
    assert_eq!(ws.read_with(cx, |workspace, _| workspace.held_update()).map(|held| held.updating), Some(true));
}

#[gpui::test]
fn cancel_leaves_an_older_computer_as_it_was(cx: &mut TestAppContext) {
    let (stand_in, home, cx) = on_an_older_devbox(cx, "waiting", 1);
    click(cx, "update-chip");
    click(cx, "update-now");
    click(cx, "update-cancel");
    assert!(!shown(cx, "update-card"));
    assert!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/update").is_empty());
    assert_eq!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.held_update()).map(|held| held.updating), Some(false));
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
fn a_setup_action_is_not_promoted_to_an_update_action(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.sets_up_again("key_devbox");
    let (home, cx) = on_devbox_held(cx, &stand_in);
    let chip = workspace(&home, cx).read_with(cx, |ws, _| ws.update_chip()).expect("held notice");
    assert!(chip.action.is_none());
    assert!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/setup").is_empty());
}

#[gpui::test]
fn an_older_machine_to_set_up_over_ssh_says_so_in_the_boxs_words_and_opens_add_a_machine(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.needs_set_up_over_ssh("key_devbox");
    let (home, cx) = on_devbox_held(cx, &stand_in);
    click(cx, "update-chip");
    let chip = workspace(&home, cx).read_with(cx, |workspace, _| workspace.update_chip()).expect("the chip");
    assert_eq!(chip.title, "Kuvo on devbox needs an update");
    assert_eq!(chip.detail, "Stand-in Mac hasn’t set it up over SSH, so it can’t update it yet. Set it up over SSH once: what runs there stays.");
    assert_eq!(chip.action.as_ref().map(|action| action.label.as_str()), Some("Set Up Over SSH…"));
    click(cx, "update-now");
    assert_eq!(home.read_with(cx, |home, _| home.shown_key()).as_deref(), Some("key_stand_in"));
    let page = computers_page(&home, cx);
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), kuvo::computers::Shown::Add);
    assert_eq!(page.read_with(cx, |page, cx| page.typed_host(cx)), "devbox");
    assert!(shown(cx, "add-machine"), "Add a Machine is on screen");
    assert!(stand_in.seen_at("POST", "kuvo/computers/add").is_empty(), "nothing added until you say");
}

#[gpui::test]
fn another_older_mac_says_its_own_app_updates_it_with_nothing_to_press(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.lists_as_mac("key_devbox");
    let (home, cx) = on_devbox_held(cx, &stand_in);
    click(cx, "update-chip");
    let chip = workspace(&home, cx).read_with(cx, |workspace, _| workspace.update_chip()).expect("the chip");
    assert_eq!(chip.detail, "Kuvo on devbox needs an update before it can be used here.");
    assert!(shown(cx, "update-card") && !shown(cx, "update-now"));
}

#[gpui::test]
fn held_machine_needs_a_box_offer_before_the_client_opens_ssh_setup(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (home, cx) = on_devbox_held(cx, &stand_in);
    click(cx, "update-chip");
    let chip = workspace(&home, cx).read_with(cx, |ws, _| ws.update_chip()).expect("held notice");
    assert!(chip.action.is_none());
    assert!(!shown(cx, "update-now"));
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

/// An older computer's page in a window showing another computer, whose own box can't bring it up
/// (a Linux box): the card has the way on of the box that lists your computers, which brings it up,
/// never a card without a button (#772: hetzner and lokum-tailscale on mini-local's page).
#[gpui::test]
fn an_older_computers_page_on_another_computers_window_has_the_listers_update_now(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    devbox.cant_bring_up("key_old", "Add computers from a Mac for now: on Linux, Kuvo Code can’t yet keep the list of your computers.");
    stand_in.bring_up_to_date("key_old", "waiting", false, 0);
    let (home, cx) = open(cx, stand_in.clone());
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    assert_eq!(name(&home, cx), "devbox");
    click(cx, "rail-computers");
    click(cx, "computer-key_old");
    let page = computers_page(&home, cx);
    assert_eq!(page.read_with(cx, |page, _| page.said("notice-update")).map(|(title, _)| title).as_deref(), Some("Kuvo on old-mini updates in a moment"));
    assert!(!shown(cx, "notice-older"), "one card");
    // What it can't say isn't drawn as "Not said yet"; it's "Kuvo", not "Kuvo Code".
    assert_eq!(page.read_with(cx, |page, _| page.said("fact-version")).map(|(title, _)| title).as_deref(), Some("Kuvo"));
    click(cx, "notice-update-action");
    // Asked of the box that lists your computers, never of devbox's, which can't.
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_old/update").len(), 1);
    assert!(devbox.seen_at("POST", "kuvo/computers/key_old/update").is_empty());
}

/// Where neither box brings it up, the window's box's words stand, as they are.
#[gpui::test]
fn where_nothing_brings_it_up_the_window_boxs_words_stand(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    devbox.cant_bring_up("key_old", "Add computers from a Mac for now: on Linux, Kuvo Code can’t yet keep the list of your computers.");
    let (home, cx) = open(cx, stand_in.clone());
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    click(cx, "rail-computers");
    click(cx, "computer-key_old");
    let page = computers_page(&home, cx);
    assert_eq!(page.read_with(cx, |page, _| page.said("notice-update")), Some(("Kuvo on old-mini needs an update".to_owned(), "Add computers from a Mac for now: on Linux, Kuvo Code can’t yet keep the list of your computers.".to_owned())));
}

/// A computer that can't be used shows one view, in every area, with what moves it on and another
/// of yours to switch to: never areas that open empty.
#[gpui::test]
fn a_computer_that_cant_be_used_shows_one_view_with_update_now_and_switch_to(cx: &mut TestAppContext) {
    let (stand_in, home, cx) = on_an_older_devbox(cx, "waiting", 0);
    assert!(shown(cx, "blocked-held"));
    assert!(!shown(cx, "area-sidebar"), "no empty sidebar beside it");
    let blocked = workspace(&home, cx).read_with(cx, |workspace, cx| workspace.blocked(cx)).expect("the view");
    assert_eq!((blocked.title.as_str(), blocked.detail.as_str()), ("devbox’s Kuvo is older than this app", "Kuvo on devbox updates in a moment."));
    assert_eq!(blocked.switch_to.as_ref().map(|(_, name)| name.as_str()), Some("Stand-in Mac"));
    click(cx, "rail-bots");
    assert!(shown(cx, "blocked-held"), "the same view in Bots");
    // Update Now, from the view, asks the box that lists your computers.
    click(cx, "blocked-action");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/update").len(), 1);
    // Switch to another of yours that works.
    click(cx, "blocked-switch");
    assert_eq!(home.read_with(cx, |home, _| home.shown_key()).as_deref(), Some("key_stand_in"));
    assert!(!shown(cx, "blocked-held"));
}

/// A computer that stops answering while the window shows it: why, in plain words (never the
/// connection's own error), Try Again, and another of yours to switch to.
#[gpui::test]
fn a_computer_out_of_reach_says_so_with_try_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    let (home, cx) = open(cx, stand_in.clone());
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    devbox.set_down(true);
    workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.reach_again(window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "blocked-unreached"));
    let blocked = workspace(&home, cx).read_with(cx, |workspace, cx| workspace.blocked(cx)).expect("the view");
    assert_eq!((blocked.title.as_str(), blocked.detail.as_str()), ("Can’t reach devbox", "Make sure it’s on and connected, then Try Again."));
    assert_eq!(blocked.action.as_ref().map(|(label, _)| label.as_str()), Some("Try Again"));
    assert_eq!(blocked.switch_to.as_ref().map(|(_, name)| name.as_str()), Some("Stand-in Mac"));
    // Back: Try Again reaches it, and its sessions show.
    devbox.set_down(false);
    click(cx, "blocked-action");
    assert!(!shown(cx, "blocked-unreached"));
    assert!(shown(cx, "session-project-ses_releases"));
}


#[gpui::test]
fn switching_to_unwelcomed_computers_opens_the_app_even_without_projects(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    devbox.set_first_run(|run| run.welcomed = false);
    devbox.clear_projects_and_sessions();
    let (home, cx) = open(cx, stand_in);
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    assert!(shown(cx, "rail-settings") && !shown(cx, "first-run"));
    assert!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.first_run().is_none()));
    assert!(!shown(cx, "project-Billing"));
    assert!(devbox.seen_at("GET", "kuvo/setup/welcomed").is_empty());
}

#[gpui::test]
fn first_run_computer_picker_switches_and_returning_keeps_the_app_open(cx: &mut TestAppContext) {
    let stand_in = StandIn::first_run(Replies::Never, NOW, kuvo::stand_in::first_run::Scenario::Running);
    let (home, cx) = open(cx, stand_in);
    assert!(shown(cx, "first-run"));
    click(cx, "first-run-next");
    click(cx, "first-run-next");
    click(cx, "first-run-computer-key_devbox");
    assert_eq!(name(&home, cx), "devbox");
    assert!(shown(cx, "rail-settings") && !shown(cx, "first-run"));
    let here = home.read_with(cx, |home, _| home.boxes().nearest_key().unwrap());
    home.update_in(cx, |home, window, cx| assert!(home.switch_to(&here, window, cx)));
    cx.run_until_parked();
    assert!(shown(cx, "rail-settings") && !shown(cx, "first-run"));
}

#[gpui::test]
fn a_background_computer_disconnecting_does_not_grey_out_the_shown_computers_bots(cx: &mut TestAppContext) {
    use kuvo_ui::bot::State;

    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    devbox.add_bot("bot_scout", "Scout", json!({ "color": "mint", "shape": "round", "face": "calm" }));
    devbox.set_bot_now("bot_scout", json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }));
    let (home, cx) = open(cx, stand_in.clone());

    // Open Bots on this computer so NEAREST has loaded bots (with bot_scout listed under elsewhere).
    let nearest = workspace(&home, cx);
    click(cx, "rail-bots");
    assert_eq!(cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().shown("bot_mira")), Some(State::NeedsYou));

    // Switch to devbox and open Bots there: bot_scout is working on devbox.
    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    click(cx, "rail-bots");
    assert_eq!(cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().shown("bot_scout")), Some(State::Working));

    // The background NEAREST workspace loses connection: it must not overwrite global bot_face
    // states with Offline and grey out the shown devbox workspace's bots.
    stand_in.bots_readable(false);
    let bots = nearest.read_with(cx, |ws, _| ws.area_view(Area::Bots).expect("Bots").clone());
    bots.update(cx, |view, cx| view.shown(cx));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().shown("bot_scout")),
        Some(State::Working),
        "disconnecting a background computer must not turn the shown computer's bots grey/offline"
    );

    // Switching back to the disconnected NEAREST workspace shows its bots offline, and switching
    // back to the connected devbox restores devbox's live bot faces immediately.
    let here = home.read_with(cx, |home, _| home.boxes().nearest_key().unwrap());
    home.update_in(cx, |home, window, cx| assert!(home.switch_to(&here, window, cx)));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().shown("bot_mira")), Some(State::Offline));

    home.update_in(cx, |home, window, cx| assert!(home.switch_to("key_devbox", window, cx)));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().shown("bot_scout")), Some(State::Working));
}

#[gpui::test]
fn a_remote_workspace_can_show_other_areas_when_its_first_sessions_list_fails(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    devbox.sessions_readable(false);
    devbox.add_bot("bot_build", "Build Runner", json!({ "color": "mint", "shape": "round", "face": "calm" }));
    let (home, cx) = open(cx, stand_in);

    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    let workspace = workspace(&home, cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(matches!(workspace.lists(), kuvo::workspace::Lists::Failed(_)));
        assert_eq!(workspace.box_state(), &kuvo::workspace::BoxState::Connected);
    });

    click(cx, "rail-queue");
    assert!(shown(cx, "queue-head"));
    click(cx, "rail-computers");
    assert!(shown(cx, "computers-title"));
    workspace.update_in(cx, |workspace, window, cx| workspace.insights_sample("insights-overview", window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "insights-chart"));

    click(cx, "rail-bots");
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.connected(Err(kuvo_box::Error::BadAnswer("The connection probe failed.".into())), cx);
    });
    cx.run_until_parked();
    assert!(shown(cx, "bot-bot_build"));
    assert!(!shown(cx, "blocked-unreached"));
}

#[gpui::test]
fn insights_on_a_computer_without_usage_insights_shows_the_update_state(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let devbox = stand_in.peer_box("key_devbox").expect("devbox");
    devbox.set_lacks(&["usage.insights"]);
    stand_in.bring_up_to_date("key_devbox", "waiting", false, 0);
    let (home, cx) = open(cx, stand_in.clone());

    click(cx, "computer-switcher");
    click(cx, "menu-item-devbox");
    // The computer is behind (not held), so Sessions works normally.
    assert!(!shown(cx, "blocked-held"));
    assert!(shown(cx, "session-project-ses_releases"));

    // Opening Insights (e.g. from the rail or clicking a usage ring) shows the update state instead
    // of calling /api/kuvo/insights and showing "Not available for this agent.".
    workspace(&home, cx).update_in(cx, |ws, window, cx| {
        ws.open_insights(
            Some(kuvo::area::UsageFilter { title: "Claude".into(), provider: "claude".into(), account: None }),
            window,
            cx,
        );
    });
    cx.run_until_parked();
    assert!(shown(cx, "blocked-held"), "Insights on an older computer must show the update state");
    let blocked = workspace(&home, cx).read_with(cx, |ws, cx| ws.blocked(cx)).expect("blocked state");
    assert_eq!(blocked.title, "Kuvo on devbox needs to update to show Insights");
    assert_eq!(blocked.action.as_ref().map(|(label, _)| label.as_str()), Some("Update Now"));
    assert_eq!(blocked.switch_to.as_ref().map(|(_, name)| name.as_str()), Some("Stand-in Mac"));
    assert!(
        devbox.seen_at("GET", "kuvo/agents/api/kuvo/insights").is_empty(),
        "must not request insights from a computer lacking usage.insights"
    );

    // Update Now triggers the computer's update, and switching goes to a computer that supports Insights.
    click(cx, "blocked-action");
    assert_eq!(stand_in.seen_at("POST", "kuvo/computers/key_devbox/update").len(), 1);
    click(cx, "blocked-switch");
    assert_eq!(home.read_with(cx, |home, _| home.shown_key()).as_deref(), Some("key_stand_in"));
    assert!(!shown(cx, "blocked-held"));
}
