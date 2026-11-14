//! The rail and its areas as someone uses them, against the stand-in box (nothing real is
//! reached): switching areas by the rail and by ⌃1 to ⌃5, folding the area's sidebar by its
//! button and in a narrow window, "Needs you" on top of Sessions, back and forward, and the rings.
//! Clicks and keys go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn area(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Area {
    workspace.read_with(cx, |workspace, _| workspace.area())
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()))
}

#[gpui::test]
fn the_rail_switches_areas_and_sessions_comes_back_to_what_was_open(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "session-project-ses_signup");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Fix the sign-up form wiping on a taken email"));

    // Bots: its sidebar lists the box's bots, and its page isn't in the app yet.
    click(cx, "rail-bots");
    assert_eq!(area(&workspace, cx), Area::Bots);
    assert!(bounds(cx, "bot-bot_mira").is_some());
    assert!(bounds(cx, "group-grp_launch").is_some(), "group chats under the bots");
    assert!(bounds(cx, "session-project-ses_signup").is_none(), "only the open area's sidebar shows");
    assert!(bounds(cx, "placeholder").is_some());
    click(cx, "bot-bot_mira");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Mira"));

    // Back to Sessions: the conversation that was open.

    click(cx, "rail-sessions");
    assert_eq!(area(&workspace, cx), Area::Sessions);
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_signup".into()));
    assert!(bounds(cx, "placeholder").is_none());
    assert!(bounds(cx, "session-project-ses_signup").is_some());
}

#[gpui::test]
fn only_the_area_on_screen_asks_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, cx) = open(cx, stand_in.clone());
    assert!(stand_in.seen_at("GET", "kuvo/bots").is_empty());
    assert!(stand_in.seen_at("GET", "kuvo/work-queue").is_empty());
    click(cx, "rail-queue");
    // The Work Queue's view with its counts, across your computers, at a glance.
    let asked: Vec<String> = stand_in.seen_at("GET", "kuvo/work-queue").into_iter().map(|seen| seen.path).collect();
    assert_eq!(asked, vec!["kuvo/work-queue?owner=all&counts=1&glance=1".to_owned()]);
    assert!(stand_in.seen_at("GET", "kuvo/bots").is_empty());
}

#[gpui::test]
fn control_one_to_five_switch_areas_from_anywhere_in_the_window(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    // From the composer of a conversation, as from anywhere.
    click(cx, "session-project-ses_limits");
    for (keys, expected, shown) in [
        ("ctrl-2", Area::Bots, "bot-bot_inbox"),
        ("ctrl-3", Area::Queue, "category-next"),
        ("ctrl-4", Area::Computers, "computer-key_devbox"),
        ("ctrl-5", Area::Settings, "setting-General"),
        ("ctrl-1", Area::Sessions, "session-project-ses_limits"),
    ] {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        assert_eq!(area(&workspace, cx), expected, "{keys}");
        assert!(bounds(cx, shown).is_some(), "{keys} shows {shown}");
    }
    // Back in Sessions, the keyboard is back in its conversation: typing goes to its composer.
    cx.simulate_input("ok");
    let composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    assert_eq!(composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned()), "ok");
}

#[gpui::test]
fn the_open_areas_button_folds_its_sidebar_away_and_brings_it_back(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert!(bounds(cx, "area-sidebar").is_some());
    click(cx, "rail-sessions");
    assert!(bounds(cx, "area-sidebar").is_none());
    assert_eq!(area(&workspace, cx), Area::Sessions);
    click(cx, "rail-sessions");
    assert!(bounds(cx, "session-project-ses_signup").is_some());
    // Another area's button opens that area with its sidebar, even from folded.
    click(cx, "rail-sessions");
    click(cx, "rail-computers");
    assert!(bounds(cx, "computer-this").is_some());
}

#[gpui::test]
fn a_narrow_window_folds_the_areas_sidebar_away_and_the_rail_brings_it_out(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    cx.simulate_resize(size(px(760.), px(800.)));
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_none(), "folded in a narrow window");
    assert!(bounds(cx, "rail-sessions").is_some(), "the rail stays");

    // Its area's button brings it out, beside the conversation; again, it folds.
    click(cx, "rail-sessions");
    assert!(bounds(cx, "session-project-ses_signup").is_some());
    click(cx, "session-project-ses_signup");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_signup".into()));
    click(cx, "rail-sessions");
    assert!(bounds(cx, "area-sidebar").is_none());

    // ⌘⇧F needs the sidebar: it comes out with the search.
    cx.simulate_keystrokes("cmd-shift-f");
    cx.run_until_parked();
    assert!(bounds(cx, "sidebar-search").is_some());

    // A wide window shows it again, and narrowing again folds it.
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_some());
    cx.simulate_resize(size(px(760.), px(800.)));
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_none());
}

#[gpui::test]
fn switching_areas_in_a_wide_window_still_folds_the_sidebar_when_it_narrows(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "session-project-ses_signup");
    cx.simulate_keystrokes("ctrl-2");
    assert_eq!(area(&workspace, cx), Area::Bots);
    cx.simulate_keystrokes("ctrl-1 cmd-shift-f");
    cx.run_until_parked();
    assert_eq!(area(&workspace, cx), Area::Sessions);
    cx.simulate_resize(size(px(760.), px(800.)));
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_none());
}

#[gpui::test]
fn a_short_window_keeps_help_and_settings_in_reach(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    cx.simulate_resize(size(px(1000.), px(480.)));
    cx.run_until_parked();
    let settings = bounds(cx, "rail-settings").expect("Settings");
    assert!(settings.bottom() <= px(480.), "Settings at {:?}", settings);
    let more = bounds(cx, "more-rings").expect("the rest behind +N");
    assert!(more.bottom() <= bounds(cx, "rail-help").unwrap().top());
    assert!(bounds(cx, "ring-3").is_none(), "fewer rings where there's less room");
}

#[gpui::test]
fn needs_you_lists_whats_waiting_on_you_across_projects_with_what_each_asks(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    // On top, in the box's order (the newest ask first), each with its project and what it asks.
    let header = bounds(cx, "header-Needs you").expect("Needs you");
    let limits = bounds(cx, "needs-ses_limits").expect("the rate limits ask");
    let cms = bounds(cx, "needs-ses_cms").expect("the CMS question");
    let website = bounds(cx, "project-Website").unwrap();
    assert!(header.top() < limits.top() && limits.top() < cms.top() && cms.top() < website.top());
    assert!(bounds(cx, "needs-ses_signup").is_none(), "one at work isn't waiting on you");
    // The rail counts them on Sessions' icon.
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.waiting().sessions), 2);

    // A click opens it, from wherever the window was.
    click(cx, "rail-bots");
    click(cx, "rail-sessions");
    click(cx, "needs-ses_cms");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_cms".into()));
}

#[gpui::test]
fn an_answered_ask_leaves_needs_you_as_the_box_says(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    assert!(bounds(cx, "needs-ses_cms").is_some());
    stand_in.set_facts("ses_cms", |facts| {
        facts.as_object_mut().unwrap().remove("asked");
    });
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(bounds(cx, "needs-ses_cms").is_none());
    assert!(bounds(cx, "needs-ses_limits").is_some());
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.waiting().sessions), 1);
}

/// A project's badge and spinner are the box's sums (`projects[<dir>].{asking, working}`): how
/// many of its sessions wait on you, else (folded) whether any is at work.
#[gpui::test]
fn a_project_shows_the_boxs_sums_of_who_waits_and_who_works(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    assert!(bounds(cx, "project-asking-API").is_some() && bounds(cx, "project-asking-Website").is_some());
    assert!(bounds(cx, "project-asking-Mobile").is_none());
    // The CMS question is answered: Website has one at work and none waiting. Folded, it spins.
    stand_in.set_facts("ses_cms", |facts| {
        facts.as_object_mut().unwrap().remove("asked");
    });
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(bounds(cx, "project-asking-Website").is_none());
    assert!(bounds(cx, "project-working-Website").is_none(), "open, its own session's row spins");
    click(cx, "project-Website");
    assert!(bounds(cx, "project-working-Website").is_some());
    // The one at work stops to ask: it waits, and isn't counted at work.
    stand_in.set_facts("ses_signup", |facts| facts["asked"] = serde_json::json!(NOW));
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(bounds(cx, "project-asking-Website").is_some() && bounds(cx, "project-working-Website").is_none());
}

/// A bot's row is drawn from what the box says it's doing now (`now`): a count where it waits on
/// you, a spinner where it only works, neither where it does nothing.
#[gpui::test]
fn a_bots_row_shows_what_the_box_says_its_doing_now(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.waiting().bots), 2, "the rail's count is the box's");
    click(cx, "rail-bots");
    assert!(bounds(cx, "bot-asking-bot_mira").is_some(), "at work and waiting: the count");
    assert!(bounds(cx, "bot-asking-bot_inbox").is_some());
    for quiet in ["bot_release", "bot_docs"] {
        assert!(bounds(cx, &format!("bot-asking-{quiet}")).is_none() && bounds(cx, &format!("bot-working-{quiet}")).is_none(), "{quiet}");
    }
}

/// Computers lists your computers as the box does (`kuvo/computers`): the one this window shows
/// first, then the others; a machine shared with another setup under its own header. No Mac is
/// asked for its machines.
#[gpui::test]
fn computers_lists_yours_as_the_box_does_with_shared_machines_apart(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-computers");
    let this = bounds(cx, "computer-this").expect("this one");
    let devbox = bounds(cx, "computer-key_devbox").expect("devbox");
    let studio = bounds(cx, "computer-key_studio").expect("one that's away");
    let header = bounds(cx, "header-shared").expect("Shared Machines");
    let shared = bounds(cx, "computer-key_build").expect("a shared machine");
    assert!(this.top() < devbox.top() && devbox.top() < studio.top() && studio.top() < header.top() && header.top() < shared.top());
    assert!(stand_in.seen_at("GET", "kuvo/machines").is_empty());
    click(cx, "computer-key_devbox");
    assert_eq!(title(&workspace, cx).as_deref(), Some("devbox"));
}

/// While the box's list can't be read, the computer this window shows is still there: the window
/// knows which it is.
#[gpui::test]
fn computers_keeps_this_one_while_the_list_cant_be_read(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.computers_readable(false);
    let (_workspace, cx) = open(cx, stand_in);
    click(cx, "rail-computers");
    assert!(bounds(cx, "computer-this").is_some());
    assert!(bounds(cx, "computer-key_devbox").is_none());
}

/// The title bar's switcher names your other computers as the box lists them; this window's first.
#[gpui::test]
fn the_switcher_lists_your_other_computers_as_the_box_does(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "computer-switcher");
    let devbox = bounds(cx, "menu-item-devbox").expect("devbox");
    let studio = bounds(cx, "menu-item-studio-mini").expect("one that's away");
    assert!(devbox.top() < studio.top());
    assert!(bounds(cx, "menu-item-Show Computers").is_some());
}

#[gpui::test]
fn a_box_alone_lists_itself_and_no_shared_machines(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::empty(Replies::Never));
    click(cx, "rail-computers");
    assert!(bounds(cx, "computer-this").is_some());
    assert!(bounds(cx, "header-shared").is_none());
}

#[gpui::test]
fn back_and_forward_go_where_the_window_has_been(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let opened = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned));
    assert!(!workspace.read_with(cx, |workspace, _| workspace.can_go_back()));
    click(cx, "session-project-ses_signup");
    click(cx, "session-project-ses_dark");
    click(cx, "rail-queue");
    click(cx, "category-later");

    cx.simulate_keystrokes("cmd-[");
    assert_eq!((area(&workspace, cx), title(&workspace, cx).as_deref()), (Area::Queue, Some("Work Queue")));
    click(cx, "back");
    assert_eq!((area(&workspace, cx), opened(&workspace, cx).as_deref()), (Area::Sessions, Some("ses_dark")));
    // Back in the conversation still open, the keyboard is in it.
    cx.simulate_input("ok");
    let composer = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).composer().clone());
    assert_eq!(composer.read_with(cx, |composer, cx| composer.field().read(cx).text().to_owned()), "ok");
    click(cx, "back");
    assert_eq!(opened(&workspace, cx).as_deref(), Some("ses_signup"));
    cx.simulate_keystrokes("cmd-] cmd-] cmd-]");
    assert_eq!((area(&workspace, cx), title(&workspace, cx).as_deref()), (Area::Queue, Some("Later")));
    assert!(!workspace.read_with(cx, |workspace, _| workspace.can_go_forward()));

    // Going somewhere new drops what was ahead.
    click(cx, "back");
    click(cx, "rail-bots");
    assert!(!workspace.read_with(cx, |workspace, _| workspace.can_go_forward()));
}

#[gpui::test]
fn the_rings_show_each_account_and_the_rest_wait_behind_more(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    for ring in ["ring-0", "ring-1", "ring-2", "ring-3"] {
        assert!(bounds(cx, ring).is_some(), "{ring}");
    }
    assert!(bounds(cx, "ring-4").is_none(), "four at most");
    // "+2" opens Settings › Usage.
    click(cx, "more-rings");
    assert_eq!(area(&workspace, cx), Area::Settings);
    assert_eq!(title(&workspace, cx).as_deref(), Some("Usage"));
}

/// Two rings an account, as the box picks them (`rows[].rings`): its short window outside, its
/// long one inside; an account whose plan has one window, one ring.
#[gpui::test]
fn an_account_has_the_two_rings_the_box_says_or_the_one(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert!(bounds(cx, "ring-claude-Work-arc-0").is_some() && bounds(cx, "ring-claude-Work-arc-1").is_some());
    let (outer, inner) = (bounds(cx, "ring-claude-Work-arc-0").unwrap(), bounds(cx, "ring-claude-Work-arc-1").unwrap());
    assert!(inner.size.width < outer.size.width && outer.contains(&inner.center()), "the long window inside the short one");
    assert!(bounds(cx, "ring-claude-Work-arc-2").is_none(), "a model's own window is no ring");
    assert!(bounds(cx, "ring-copilot-arc-0").is_some() && bounds(cx, "ring-copilot-arc-1").is_none(), "one window, one ring");
}

#[gpui::test]
fn with_no_account_an_empty_ring_goes_to_sign_in(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::empty(Replies::Never));
    assert!(bounds(cx, "ring-0").is_none());
    click(cx, "no-accounts");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Accounts"));
}

#[gpui::test]
fn back_passes_over_a_session_gone_since(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "session-project-ses_signup");
    click(cx, "session-project-ses_webhook");
    click(cx, "rail-bots");
    // The webhook session is archived elsewhere, and the list says so.
    stand_in.set_facts("ses_webhook", |facts| facts["archived"] = serde_json::json!(true));
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    click(cx, "back");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_signup".into()));
    assert_eq!(area(&workspace, cx), Area::Sessions);
}
