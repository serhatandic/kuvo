//! The rail and its areas as someone uses them, against the stand-in box (nothing real is
//! reached): switching areas by the rail and by ⌃1 to ⌃5, folding the area's sidebar by its
//! button and in a narrow window, "Needs you" on top of Sessions, back and forward, and the rings.
//! Clicks and keys go through the window as AppKit hands them on.

use gpui::{AppContext as _, Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, TestAppContext, VisualTestContext, point, px, size};
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::stand_in::areas::UsageSample;
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

fn read_update_offers(workspace: &mut Workspace, cx: &mut gpui::Context<Workspace>) {
    use kuvo_box::boxes::{Pairing, Reach, Row};
    let client = workspace.client().clone();
    let snapshot = client.computers_snapshot().unwrap();
    let shown = snapshot.computers.iter().find(|computer| computer.here).map(|computer| computer.key.clone());
    let rows = snapshot.computers.into_iter().map(|computer| Row {
        reach: if computer.here { Reach::Nearest } else { Reach::Connecting },
        computer,
    }).collect();
    workspace.set_lister(Some(client));
    workspace.set_switching(Some(kuvo::home::Switching { rows, shown, pairing: Pairing::Paired, fleet: snapshot.fleet_update }), cx);
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

    // Bots: its sidebar lists the box's bots, then its group chats, each opening its conversation.
    click(cx, "rail-bots");
    assert_eq!(area(&workspace, cx), Area::Bots);
    assert!(bounds(cx, "bot-bot_mira").is_some());
    assert!(bounds(cx, "group-grp_launch").is_some(), "group chats under the bots");
    assert!(bounds(cx, "session-project-ses_signup").is_none(), "only the open area's sidebar shows");
    assert!(bounds(cx, "bots-empty").is_some(), "nothing picked yet");
    click(cx, "group-grp_launch");
    assert!(bounds(cx, "group-chat").is_some());
    assert_eq!(title(&workspace, cx).as_deref(), Some("Launch crew"));
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
    // The Work Queue's view with its counts, across your computers: whole, as the queue itself shows.
    let asked: Vec<String> = stand_in.seen_at("GET", "kuvo/work-queue").into_iter().map(|seen| seen.path).collect();
    assert_eq!(asked, vec!["kuvo/work-queue?owner=all&counts=1".to_owned()]);
    assert!(stand_in.seen_at("GET", "kuvo/bots").is_empty());
}

#[gpui::test]
fn control_one_to_five_switch_areas_from_anywhere_in_the_window(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert!(bounds(cx, "rail-bots").unwrap().top() < bounds(cx, "rail-sessions").unwrap().top());
    // From the composer of a conversation, as from anywhere.
    click(cx, "session-project-ses_limits");
    for (keys, expected, shown) in [
        ("ctrl-1", Area::Bots, "bot-bot_inbox"),
        ("ctrl-3", Area::Queue, "status-todo"),
        ("ctrl-4", Area::Computers, "computer-key_devbox"),
        ("ctrl-5", Area::Settings, "setting-general"),
        ("ctrl-2", Area::Sessions, "session-project-ses_limits"),
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
}

#[gpui::test]
fn a_folded_sidebar_stays_folded_as_you_switch_areas(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "rail-bots");
    assert!(bounds(cx, "area-sidebar").is_some());
    // Folded by the title bar's button, in Bots.
    click(cx, "toggle-sidebar");
    assert!(bounds(cx, "area-sidebar").is_none());
    // Over to Sessions and back by the rail, and on by the keyboard: still folded, in every area.
    click(cx, "rail-sessions");
    assert_eq!(area(&workspace, cx), Area::Sessions);
    assert!(bounds(cx, "area-sidebar").is_none(), "folded in Sessions");
    click(cx, "rail-bots");
    assert_eq!(area(&workspace, cx), Area::Bots);
    assert!(bounds(cx, "area-sidebar").is_none(), "folded back in Bots");
    cx.simulate_keystrokes("ctrl-3");
    cx.run_until_parked();
    assert_eq!(area(&workspace, cx), Area::Queue);
    assert!(bounds(cx, "area-sidebar").is_none(), "folded in the Work Queue");
    // Until you bring it back: then it's out, in every area.
    click(cx, "toggle-sidebar");
    assert!(bounds(cx, "area-sidebar").is_some());
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
    cx.simulate_keystrokes("ctrl-1");
    assert_eq!(area(&workspace, cx), Area::Bots);
    cx.simulate_keystrokes("ctrl-2 cmd-shift-f");
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

/// A bot's row is drawn from what the box says it's doing now (`now`): its face in the box's
/// state, and no spinner or count beside it (the bot says it; the rail keeps its count).
#[gpui::test]
fn a_bots_row_shows_what_the_box_says_its_doing_now(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.waiting().bots), 2, "the rail's count is the box's");
    click(cx, "rail-bots");
    let drawn = |cx: &mut VisualTestContext, bot: &str| cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().shown(bot));
    assert_eq!(drawn(cx, "bot_mira"), Some(kuvo_ui::bot::State::NeedsYou), "at work and waiting: needs you");
    assert_eq!(drawn(cx, "bot_docs"), Some(kuvo_ui::bot::State::Done));
    for bot in ["bot_mira", "bot_inbox", "bot_release", "bot_docs"] {
        assert!(bounds(cx, &format!("bot-asking-{bot}")).is_none() && bounds(cx, &format!("bot-working-{bot}")).is_none(), "{bot}");
    }
}

/// Computers lists your computers as the box does (`kuvo/computers`): the one this window shows
/// first, then the others. No Mac is asked for its machines.
#[gpui::test]
fn computers_lists_yours_as_the_box_does(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-computers");
    let this = bounds(cx, "computer-this").expect("this one");
    let devbox = bounds(cx, "computer-key_devbox").expect("devbox");
    let studio = bounds(cx, "computer-key_studio").expect("one that's away");
    // The box says how each is reached: a dot for each it lists.
    assert!(bounds(cx, "computer-dot-this").is_some() && bounds(cx, "computer-dot-key_studio").is_some());
    assert!(this.top() < devbox.top() && devbox.top() < studio.top() && studio.top() < bounds(cx, "computer-key_old").expect("an older one").top());
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
    // Nothing is claimed of its reach, which only the list says: no dot.
    assert!(bounds(cx, "computer-dot-this").is_none());
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
fn a_box_alone_lists_itself(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::empty(Replies::Never));
    click(cx, "rail-computers");
    assert!(bounds(cx, "computer-this").is_some());
    assert!(bounds(cx, "computer-key_devbox").is_none());
}

#[gpui::test]
fn back_and_forward_go_where_the_window_has_been(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let opened = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned));
    assert!(!workspace.read_with(cx, |workspace, _| workspace.can_go_back()));
    click(cx, "session-project-ses_signup");
    click(cx, "session-project-ses_dark");
    click(cx, "rail-queue");
    click(cx, "status-backlog");

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
    assert_eq!((area(&workspace, cx), title(&workspace, cx).as_deref()), (Area::Queue, Some("Backlog")));
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
    // "+2" opens Insights.
    click(cx, "more-rings");
    assert_eq!(area(&workspace, cx), Area::Insights);
    assert_eq!(title(&workspace, cx).as_deref(), Some("Insights"));
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

/// A plan that ended (the box's `ended`: Cursor said there's no active subscription) has no ring:
/// the next account takes its place.
#[gpui::test]
fn a_plan_that_ended_has_no_ring(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let second = bounds(cx, "ring-1").expect("a second ring");
    let personal = bounds(cx, "ring-claude-Personal-arc-0").expect("Claude · Personal's ring, next after Claude · Work");
    assert!(second.contains(&personal.center()), "Cursor's ended plan isn't drawn in its place");
}

/// The rail's rings are this computer's: an account signed in only on another computer (its row's
/// `boxes` never `here`) has no ring here, and the next account takes its place.
#[gpui::test]
fn an_account_signed_in_only_elsewhere_has_no_ring_here(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert!(bounds(cx, "ring-zai-arc-0").is_none(), "Z.ai is signed in on mini-local only");
    let second = bounds(cx, "ring-1").expect("a second ring");
    let personal = bounds(cx, "ring-claude-Personal-arc-0").expect("Claude · Personal's ring");
    assert!(second.contains(&personal.center()), "the next account of this computer's takes its place");
}

/// Lets an entry motion (`animate_in`, 150 ms) run to its end, a frame at a time at 60 Hz, so a
/// card is measured where it settles and not where it starts.
fn settle_entry_motion(cx: &mut VisualTestContext) {
    for _ in 0..12 {
        cx.executor().advance_clock(std::time::Duration::from_micros(16_667));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

/// Resting on a ring opens its card to the ring's right, raised to fit in a short window. The rings
/// below remain in view. The card captures clicks, stays open as the pointer enters it, and closes
/// when the pointer leaves both the ring and card.
#[gpui::test]
fn a_rings_card_opens_to_its_right_and_takes_its_own_clicks(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    // Settings, in a window short enough that its first sections are beside the rings.
    click(cx, "rail-settings");
    cx.simulate_resize(size(px(1000.), px(560.)));
    cx.run_until_parked();
    let ring = bounds(cx, "ring-0").unwrap();
    let below = bounds(cx, "ring-1").unwrap();
    cx.simulate_mouse_move(ring.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "ring-card").is_none(), "not before the pointer rests");
    cx.executor().advance_clock(kuvo::rail::CARD_DELAY);
    cx.run_until_parked();
    settle_entry_motion(cx);
    let card = bounds(cx, "ring-card").expect("its card");
    assert!(card.left() >= ring.right() - px(0.5), "to the ring's right: {card:?} beside {ring:?}");
    assert!(ring.top() + card.size.height > px(560.), "top alignment would cut off the card");
    assert!((card.bottom() - ring.bottom()).abs() < px(1.), "raised to the ring's bottom edge: {card:?} beside {ring:?}");
    assert!(card.left() >= below.right() - px(0.5), "the ring below stays in view");

    // Onto the card: it stays open. A click on it reaches nothing under it (a section of Settings).
    let sections = ["general", "appearance", "notifications", "updates"];
    let (section, inside) = sections
        .iter()
        .find_map(|section| {
            let row = bounds(cx, &format!("setting-{section}"))?;
            let at = gpui::point(row.center().x.max(card.left() + px(24.)), row.center().y);
            (card.contains(&at) && row.contains(&at)).then_some((*section, at))
        })
        .expect("a section of Settings under the card");
    cx.simulate_mouse_move(inside, None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "ring-card").is_some(), "moving onto the card keeps it open");
    let before = title(&workspace, cx);
    cx.simulate_click(inside, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(title(&workspace, cx), before, "the click on the card didn't open {section} under it");

    // Off both: it closes.
    cx.simulate_mouse_move(gpui::point(px(900.), px(300.)), None, Modifiers::default());
    cx.executor().advance_clock(kuvo::rail::CARD_GRACE);
    cx.run_until_parked();
    assert!(bounds(cx, "ring-card").is_none());
}

/// Across the gap between two rings, an open card moves to the next ring at once.
#[gpui::test]
fn an_open_card_follows_the_pointer_down_the_rings(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    let (first, second) = (bounds(cx, "ring-0").unwrap(), bounds(cx, "ring-1").unwrap());
    cx.simulate_mouse_move(first.center(), None, Modifiers::default());
    cx.executor().advance_clock(kuvo::rail::CARD_DELAY);
    cx.run_until_parked();
    assert!(bounds(cx, "ring-card").is_some());
    // Into the gap, then onto the next ring: no wait, the card beside it.
    cx.simulate_mouse_move(gpui::point(first.center().x, (first.bottom() + second.top()) / 2.), None, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_mouse_move(second.center(), None, Modifiers::default());
    cx.run_until_parked();
    let card = bounds(cx, "ring-card").expect("the card, at once");
    assert!((card.top() - second.top()).abs() < px(1.), "beside the second ring, top-aligned: {card:?} {second:?}");
}

/// The coding agents restarting keep the rings' last readings, marked as updating, their card saying
/// why in the box's words; down, one ring says the box's sentence where they were, with its Try
/// Again, which brings them back.
#[gpui::test]
fn the_rings_keep_their_readings_while_the_agents_restart_and_say_why_while_down(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let rings = workspace.read_with(cx, |workspace, _| workspace.rings().clone());
    let read_again = |cx: &mut VisualTestContext| {
        rings.update(cx, |rings, cx| rings.read_at_once(cx));
        cx.run_until_parked();
    };
    let before = rings.read_with(cx, |rings, _| rings.accounts().map(<[_]>::to_vec)).expect("read");
    stand_in.agents_restarting();
    read_again(cx);
    assert_eq!(rings.read_with(cx, |rings, _| rings.accounts().map(<[_]>::to_vec)), Some(before.clone()), "the last readings stay");
    assert!(rings.read_with(cx, |rings, _| rings.updating()));
    assert!(bounds(cx, "ring-0").is_some() && bounds(cx, "rings-agents-down").is_none());
    let ring = bounds(cx, "ring-0").unwrap().center();
    cx.simulate_mouse_move(ring, None, Modifiers::default());
    cx.executor().advance_clock(kuvo::rail::CARD_DELAY);
    cx.run_until_parked();
    assert!(bounds(cx, "ring-card-updating").is_some(), "its card says why");
    cx.simulate_mouse_move(gpui::point(px(900.), px(300.)), None, Modifiers::default());
    cx.executor().advance_clock(kuvo::rail::CARD_GRACE);
    cx.run_until_parked();

    // Down: the box's sentence where the rings were, with Try Again.
    stand_in.agents_running(false);
    read_again(cx);
    assert!(bounds(cx, "ring-0").is_none());
    let down = bounds(cx, "rings-agents-down").expect("where the rings were");
    assert_eq!(rings.read_with(cx, |rings, _| rings.agents().map(|state| state.message.clone())).as_deref(), Some("The coding agents haven't started."));
    cx.simulate_mouse_move(down.center(), None, Modifiers::default());
    cx.executor().advance_clock(kuvo::rail::CARD_DELAY);
    cx.run_until_parked();
    assert!(bounds(cx, "rings-agents-words").is_some());
    click(cx, "rings-try-again");
    assert_eq!(stand_in.seen_at("POST", "kuvo/agents/start").len(), 1);
    // Running again: read again, the rings back.
    assert!(rings.read_with(cx, |rings, _| rings.agents().is_none()));
    assert_eq!(rings.read_with(cx, |rings, _| rings.accounts().map(<[_]>::to_vec)), Some(before));
    assert!(bounds(cx, "ring-0").is_some() && bounds(cx, "rings-agents-down").is_none());
}

#[gpui::test]
fn limits_events_show_claudes_first_reading_and_remove_its_ring_on_sign_out(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stand_in = StandIn::at(Replies::Never, NOW);
    // Initially, Claude is signed in but the box has no reading yet.
    stand_in.set_usage_sample(UsageSample::UnmeasuredClaude);
    let client = BoxClient::new(stand_in.clone());
    let rings = cx.new(|cx| kuvo::rail::Rings::new(client, true, cx));
    let wait_for = |cx: &mut TestAppContext, matches: &dyn Fn(&[kuvo_box::areas::Account]) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if rings.read_with(cx, |rings, _| rings.accounts().is_some_and(matches)) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the limits update should arrive without advancing the fallback clock");
            std::thread::yield_now();
        }
    };
    wait_for(cx, &|accounts| stand_in.seen_at("GET", "kuvo/agents/api/kuvo/limits/events").len() == 1
        && matches!(accounts, [claude] if claude.service == "claude" && claude.here() && claude.windows.is_empty()));

    stand_in.complete_usage_refresh(UsageSample::One);
    wait_for(cx, &|accounts| matches!(accounts, [claude] if claude.service == "claude" && claude.here()
        && claude.rings.short.as_ref().is_some_and(|ring| ring.used == 0.46)
        && claude.rings.long.as_ref().is_some_and(|ring| ring.used == 0.38)));

    stand_in.complete_usage_refresh(UsageSample::Stale);
    wait_for(cx, &|accounts| !accounts.iter().any(|account| account.service == "claude" && account.here()));
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

#[gpui::test]
fn a_held_older_box_shows_one_view_with_what_moves_it_on_in_every_area_but_settings(cx: &mut TestAppContext) {
    let stand_in = StandIn::held_older(Replies::Never);
    stand_in.own_update_waits("waiting", 1);
    let (title, detail) = kuvo::workspace::held_words(Some("This Mac"), true);
    stand_in.own_update_words(title, detail);
    let (workspace, cx) = open(cx, stand_in);
    workspace.update_in(cx, |workspace, _, cx| read_update_offers(workspace, cx));
    cx.run_until_parked();
    assert!(bounds(cx, "update-chip").is_some());
    // Sessions: why it can't be used, in the gate's words, never an empty list or a composer
    // that takes nothing.
    assert!(bounds(cx, "blocked-held").is_some());
    assert!(bounds(cx, "no-accounts").is_none());
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.held_notice(), Some(("This Mac’s Kuvo is older than this app".into(), "It can’t be used here until it’s updated.".into())));
        assert_eq!(workspace.lists(), &kuvo::workspace::Lists::Held);
        let blocked = workspace.blocked(cx).expect("the view");
        assert_eq!((blocked.title.as_str(), blocked.detail.as_str()), ("This Mac’s Kuvo is older than this app", "It can’t be used here until it’s updated."));
        // Never words without a way on: Update Now, here as everywhere.
        assert_eq!(blocked.action.as_ref().map(|(label, _)| label.as_str()), Some("Update Now"));
    });
    assert!(bounds(cx, "blocked-action").is_some());
    for (rail, empty) in [("rail-bots", "bots-empty"), ("rail-queue", "queue-empty"), ("rail-computers", "computers-page")] {
        click(cx, rail);
        assert!(bounds(cx, "update-chip").is_some());
        assert!(bounds(cx, "blocked-held").is_some(), "{rail}: the same view");
        assert!(bounds(cx, empty).is_none(), "{rail}: never opened empty");
        assert!(bounds(cx, "held-area-sidebar").is_none(), "{rail}: no empty sidebar beside it");
    }
    click(cx, "rail-settings");
    assert!(bounds(cx, "blocked-held").is_none());
    assert!(bounds(cx, "row-editor").is_some(), "your own settings remain available");
    // Its own settings are its box's: they wait with it.
    click(cx, "settings-foot");
    assert!(bounds(cx, "settings-held").is_some());
    assert!(bounds(cx, "update-chip").is_some());
}

/// This Mac's box reports an update offer that asks first. Update Now invokes the box's offer.
#[gpui::test]
fn update_now_on_a_held_mac_asks_its_own_box_to_update(cx: &mut TestAppContext) {
    let stand_in = StandIn::held_older(Replies::Never);
    stand_in.own_update_waits("waiting", 1);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, _, cx| read_update_offers(workspace, cx));
    cx.run_until_parked();
    click(cx, "blocked-action");
    assert!(stand_in.seen_at("POST", "kuvo/update").is_empty(), "asked first");
    click(cx, "update-confirm");
    assert_eq!(stand_in.seen_at("POST", "kuvo/update").len(), 1);
}

/// A box that lacks only features this app draws elsewhere (beta.30's bot states, on an earlier
/// Kuvo) is used as it is: no area is held, each draws what the box has, and its update is offered.
#[gpui::test]
fn a_box_behind_on_a_feature_is_used_as_it_is_with_its_update_offered(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_lacks(&["bots.state", "settings.vault", "work.background.items"]);
    stand_in.own_update_waits("waiting", 0);
    stand_in.own_update_words("This Mac’s Kuvo is older than this app".into(), kuvo::workspace::behind_words().into());
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, _, cx| read_update_offers(workspace, cx));
    cx.run_until_parked();
    assert!(bounds(cx, "blocked-held").is_none());
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.lists().clone()), kuvo::workspace::Lists::Ready);
    click(cx, "rail-bots");
    assert!(bounds(cx, "blocked-held").is_none());
    assert!(bounds(cx, "held-area").is_none());
    assert!(bounds(cx, "bot-bot_mira").is_some(), "its bots, as it says them");
    let chip = workspace.read_with(cx, |workspace, _| workspace.update_chip()).expect("its update, offered");
    assert_eq!(chip.detail, kuvo::workspace::behind_words(), "never said to wait while it's in use");
    assert_eq!(chip.action.map(|action| action.label), Some("Update Now".into()));
    // It updates under the open window (its server new): read again, the chip gone.
    stand_in.set_lacks(&[]);
    stand_in.finish_own_update();
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.read_at_once(cx);
        read_update_offers(workspace, cx);
    });
    cx.run_until_parked();
    assert!(workspace.read_with(cx, |workspace, _| workspace.update_chip()).is_none());
    assert!(bounds(cx, "update-chip").is_none());
}

#[gpui::test]
fn a_held_machine_notice_names_the_machine_and_its_update_path(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, StandIn::held_machine(Replies::Never));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.held_notice()), Some((
        "build-linux’s Kuvo is older than this app".into(),
        "Kuvo on build-linux needs an update before it can be used here.".into(),
    )));
    assert!(bounds(cx, "update-chip").is_some());
}

#[gpui::test]
fn opening_the_gate_refreshes_the_area_already_on_screen(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_older(true);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    assert!(bounds(cx, "held-area").is_some());
    assert!(bounds(cx, "bots-empty").is_none());
    stand_in.set_older(false);
    workspace.update_in(cx, |workspace, _, cx| {
        // A live session follower opens the gate and delivers Listing without connected().
        workspace.client().connect().unwrap();
        let mut listing = kuvo_box::Listing::new();
        listing.take(workspace.client().session_list(None).unwrap());
        workspace.take_list(kuvo_box::follow::ListUpdate::Listing(listing), cx);
    });
    cx.run_until_parked();
    assert!(bounds(cx, "update-chip").is_none());
    assert!(bounds(cx, "bot-bot_mira").is_some(), "already shown area is read again");
    click(cx, "rail-sessions");
    workspace.update_in(cx, |workspace, window, cx| {
        window.focus(&gpui::Focusable::focus_handle(workspace.landing().read(cx), cx), cx);
    });
    cx.simulate_input("ready");
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.landing().read(cx).field().read(cx).text(), "ready");
    });
}

#[gpui::test]
fn title_bar_button_and_command_b_toggle_the_same_sidebar_as_the_rail(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert!(bounds(cx, "area-sidebar").is_some());
    let toggle = bounds(cx, "toggle-sidebar").unwrap();
    let picker = bounds(cx, "computer-switcher").unwrap();
    assert!(toggle.right() <= picker.left(), "the toggle precedes the computer picker");
    click(cx, "toggle-sidebar");
    assert!(bounds(cx, "area-sidebar").is_none());
    cx.simulate_keystrokes("cmd-b");
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_some());
    cx.simulate_keystrokes("cmd-b");
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_none());
    click(cx, "rail-sessions");
    assert!(bounds(cx, "area-sidebar").is_some());
    click(cx, "toggle-sidebar");
    click(cx, "toggle-sidebar");
    assert!(bounds(cx, "area-sidebar").is_some());
}

/// The sidebar toggle, the computer picker, back and forward are one group over the sidebar side;
/// a thin divider ends it where the sidebar column ends, and the title starts after it. There is no
/// + in the bar (New Session is ⌘N and File › New Session), and with the sidebar hidden the group
/// and the divider stay together, then the title.
#[gpui::test]
fn title_bar_groups_the_sidebar_side_buttons_before_a_divider_and_the_title(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "rail-queue");
    assert!(bounds(cx, "new-session").is_none(), "the title bar has no + of its own");
    let order = |cx: &mut gpui::VisualTestContext| {
        ["toggle-sidebar", "computer-switcher", "back", "forward", "title-bar-divider", "window-title"].map(|name| bounds(cx, name).unwrap_or_else(|| panic!("{name} is drawn")))
    };
    let shown = order(cx);
    for pair in shown.windows(2) {
        assert!(pair[0].right() <= pair[1].left(), "left to right: {pair:?}");
    }
    let divider = shown[4];
    assert!(divider.size.width <= gpui::px(2.) && divider.size.height < gpui::px(kuvo::title_bar::HEIGHT) / 2., "thin and short: {divider:?}");
    let sidebar = bounds(cx, "area-sidebar").unwrap();
    assert!((divider.right() - sidebar.right()).abs() <= gpui::px(2.), "the divider sits where the sidebar column ends: {divider:?} / {sidebar:?}");
    click(cx, "toggle-sidebar");
    let hidden = order(cx);
    for pair in hidden.windows(2) {
        assert!(pair[0].right() <= pair[1].left(), "left to right, sidebar hidden: {pair:?}");
    }
    assert!(hidden[4].left() - hidden[3].right() <= gpui::px(16.), "the divider stays with the group");
}

fn drag_sidebar(cx: &mut VisualTestContext, delta: f32) {
    let from = bounds(cx, "sidebar-edge").unwrap().center();
    let to = point(from.x + px(delta), from.y);
    let modifiers = Modifiers::default();
    cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
    cx.run_until_parked();
}

fn resizable_sidebar_width(cx: &mut VisualTestContext) -> Pixels {
    let sidebar = bounds(cx, "area-sidebar").unwrap();
    let divider = bounds(cx, "title-bar-divider").unwrap();
    assert_eq!(divider.right(), sidebar.right(), "the border and divider share the same edge");
    assert!(bounds(cx, "toggle-sidebar").unwrap().right() <= divider.left());
    for name in ["computer-switcher", "back", "forward"] {
        let button = bounds(cx, name).unwrap();
        if sidebar.size.width == px(64.) {
            assert!(button.left() >= divider.right(), "{name} sits beside the title in compact mode");
        } else {
            assert!(button.right() <= divider.left(), "{name} fits");
        }
    }
    sidebar.size.width
}

#[gpui::test]
fn resizable_sidebar_keeps_a_long_computer_name_inside_the_header(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let name = "WWWWW Engineering Workstation East Coast";
    assert_eq!(name.chars().count(), 40);
    stand_in.set_elsewhere(true);
    stand_in.set_computer_name(name);
    let (workspace, cx) = open(cx, stand_in);
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.box_name().to_string()), name);
    click(cx, "rail-queue");
    assert_eq!(resizable_sidebar_width(cx), px(272.));
    let picker = bounds(cx, "computer-switcher").unwrap().size.width;
    drag_sidebar(cx, 160.);
    assert_eq!(resizable_sidebar_width(cx), px(432.));
    assert!(bounds(cx, "computer-switcher").unwrap().size.width > picker);
}

#[gpui::test]
fn resizable_sidebar_switches_to_avatars_below_the_threshold(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_bot_now("bot_mira", serde_json::json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }));
    let (workspace, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    drag_sidebar(cx, -56.);
    assert_eq!(resizable_sidebar_width(cx), px(216.));
    assert!(bounds(cx, "bot-bot_mira").is_some());
    drag_sidebar(cx, -1.);
    assert_eq!(resizable_sidebar_width(cx), px(64.));
    assert!(bounds(cx, "bot-bot_mira").is_none());
    assert!(bounds(cx, "bot-avatar-at-work").is_some());
    assert!(bounds(cx, "bot-avatar-needs-you").is_some());
    assert!(bounds(cx, "group-grp_launch").is_none());
    let mira = bounds(cx, "bot-avatar-bot:bot_mira").unwrap();
    let inbox = bounds(cx, "bot-avatar-bot:bot_inbox").unwrap();
    assert_eq!(mira.size.width, px(48.));
    assert_eq!(inbox.top() - mira.bottom(), px(4.));
    drag_sidebar(cx, -45.);
    assert_eq!(resizable_sidebar_width(cx), px(64.));
    click(cx, "bot-avatar-bot:bot_mira");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Mira"));
    cx.simulate_keystrokes("cmd-b");
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_none());
    click(cx, "rail-bots");
    assert_eq!(resizable_sidebar_width(cx), px(64.));
    drag_sidebar(cx, 152.);
    assert_eq!(resizable_sidebar_width(cx), px(216.));
    assert!(bounds(cx, "bot-bot_mira").is_some());
}

#[gpui::test]
fn resizable_sessions_sidebar_fits_rows_and_times_inside_sidebar_width(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_facts("ses_signup", |facts| {
        facts["title"] = serde_json::json!("Fix the sign-up form wiping on a taken email when the previous invitation has expired");
    });
    let (_, cx) = open(cx, stand_in);
    for (delta, expected) in [(-56., px(216.)), (144., px(360.))] {
        drag_sidebar(cx, delta);
        assert_eq!(resizable_sidebar_width(cx), expected);
        let sidebar = bounds(cx, "area-sidebar").unwrap();
        let search = bounds(cx, "sidebar-search").unwrap();
        let row = bounds(cx, "session-project-ses_signup").unwrap();
        let when = bounds(cx, "session-when-project-ses_signup").unwrap();
        assert!(search.right() <= sidebar.right(), "search fits in sidebar: {search:?} / {sidebar:?}");
        assert!(row.right() <= sidebar.right(), "nested project row fits in sidebar: {row:?} / {sidebar:?}");
        assert!(when.right() <= sidebar.right(), "time label stays inside sidebar: {when:?} / {sidebar:?}");
        assert!(when.right() >= sidebar.right() - px(24.), "time label stays right-aligned: {when:?} / {sidebar:?}");
    }
}

#[gpui::test]
fn resizable_sidebar_search_opens_bots_and_sessions_without_expanding(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_facts("ses_signup", |facts| {
        facts["bot"] = serde_json::json!("bot_mira");
        facts["title"] = serde_json::json!("Fix the sign-up form wiping on a taken email when the previous invitation has expired");
    });
    let (workspace, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    drag_sidebar(cx, -200.);
    click(cx, "bot-avatar-search");
    assert!(bounds(cx, "sidebar-quick-search").is_some());
    assert_eq!(resizable_sidebar_width(cx), px(64.));

    cx.simulate_input("in");
    assert!(bounds(cx, "sidebar-search-result-bot:bot_inbox").is_some());
    assert!(bounds(cx, "sidebar-search-result-ses_signup").is_some());
    assert!(bounds(cx, "sidebar-search-result-bot:bot_inbox-avatar").is_some());
    assert!(bounds(cx, "sidebar-search-result-ses_signup-avatar").is_none());
    assert!(bounds(cx, "sidebar-search-result-ses_signup-provider-claude").is_some());
    let leading = bounds(cx, "sidebar-search-leading").unwrap();
    let input = bounds(cx, "sidebar-search-input").unwrap();
    assert_eq!(leading.size.width, px(20.));
    assert_eq!(input.left(), leading.right() + px(8.));
    for selector in ["sidebar-search-result-bot:bot_inbox-leading", "sidebar-search-result-ses_signup-leading"] {
        let row = bounds(cx, selector).unwrap();
        assert_eq!((row.left(), row.size.width), (leading.left(), leading.size.width));
    }
    cx.simulate_keystrokes("escape");
    assert!(bounds(cx, "sidebar-quick-search").is_none());
    assert_eq!(area(&workspace, cx), Area::Bots);
    assert_eq!(resizable_sidebar_width(cx), px(64.));

    click(cx, "bot-avatar-search");
    cx.simulate_keystrokes("up down down enter");
    assert_eq!(title(&workspace, cx).as_deref(), Some("Inbox Helper"));
    assert!(bounds(cx, "sidebar-quick-search").is_none());
    assert_eq!(resizable_sidebar_width(cx), px(64.));

    click(cx, "bot-avatar-search");
    cx.simulate_input("redis");
    assert!(bounds(cx, "sidebar-search-result-ses_limits-avatar").is_none());
    let chat = bounds(cx, "sidebar-search-result-ses_limits-leading").unwrap();
    assert_eq!((chat.left(), chat.size.width), (leading.left(), leading.size.width));
    click(cx, "sidebar-search-result-ses_limits");
    assert_eq!(area(&workspace, cx), Area::Sessions);
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).as_deref(), Some("ses_limits"));
    assert!(bounds(cx, "sidebar-quick-search").is_none());
    assert_eq!(resizable_sidebar_width(cx), px(64.));
    assert!(bounds(cx, "bot-avatar-search").is_some());

    click(cx, "bot-avatar-search");
    cx.simulate_input("zzzz");
    assert!(bounds(cx, "sidebar-search-No matches.").is_some());
    cx.simulate_keystrokes("enter");
    assert!(bounds(cx, "sidebar-quick-search").is_some());
}

#[gpui::test]
fn resizable_sidebar_saves_width_on_this_device_and_double_click_resets_it(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join(kuvo::yours::FILE);
    cx.update(|cx| kuvo::yours::init(Some(path.clone()), cx));
    let (_, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    drag_sidebar(cx, 400.);
    assert_eq!(resizable_sidebar_width(cx), px(440.));
    assert_eq!(kuvo::yours::Store::read(Some(path.clone())).now().sidebar_width, Some(440.));
    cx.simulate_resize(size(px(820.), px(800.)));
    cx.run_until_parked();
    assert_eq!(resizable_sidebar_width(cx), px(413.));
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.run_until_parked();
    assert_eq!(resizable_sidebar_width(cx), px(440.));

    let position = bounds(cx, "sidebar-edge").unwrap().center();
    let modifiers = Modifiers::default();
    let button = MouseButton::Left;
    cx.simulate_event(MouseDownEvent { button, position, modifiers, click_count: 2, first_mouse: false });
    cx.simulate_event(MouseUpEvent { button, position, modifiers, click_count: 2 });
    cx.run_until_parked();
    assert_eq!(resizable_sidebar_width(cx), px(272.));
    assert_eq!(kuvo::yours::Store::read(Some(path)).now().sidebar_width, Some(272.));
}

#[gpui::test]
fn resizable_sidebar_handles_one_bot_and_a_computer_going_away(cx: &mut TestAppContext) {
    let stand_in = StandIn::empty(Replies::Never);
    stand_in.set_elsewhere(true);
    stand_in.set_computer_name("Only Box");
    stand_in.add_bot("solo", "One Bot with a Very Long Name", serde_json::json!({
        "color": "mint", "shape": "round", "face": "calm",
    }));
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    drag_sidebar(cx, -200.);
    assert_eq!(resizable_sidebar_width(cx), px(64.));
    click(cx, "bot-avatar-bot:solo");
    assert_eq!(title(&workspace, cx).as_deref(), Some("One Bot with a Very Long Name"));

    stand_in.set_down(true);
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_none());
    assert!(bounds(cx, "held-area").is_some());
    click(cx, "computer-switcher");
    assert!(bounds(cx, "menu-item-Only Box").is_some());
    assert!(bounds(cx, "menu-item-Show Computers").is_none(), "there are no other computers");
    cx.simulate_keystrokes("escape ctrl-4");
    cx.run_until_parked();
    assert_eq!(area(&workspace, cx), Area::Computers);
}

#[gpui::test]
fn compact_sidebar_search_closes_when_folded(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "rail-bots");
    drag_sidebar(cx, -210.);
    click(cx, "bot-avatar-search");
    cx.simulate_input("Mira");
    cx.simulate_keystrokes("cmd-b");
    assert!(bounds(cx, "area-sidebar").is_none());
    assert!(bounds(cx, "sidebar-quick-search").is_none());
    // Keyboard focus is on the visible toggle, so Return brings the strip back.
    cx.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse("enter").unwrap() });
    assert!(bounds(cx, "bot-avatar-search").is_some());
    click(cx, "bot-avatar-search");
    assert!(bounds(cx, "sidebar-search-result-bot:bot_inbox").is_some(), "the old search is gone");
}

#[gpui::test]
fn compact_sidebar_search_closes_on_disconnect_and_cannot_reopen(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_elsewhere(true);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    drag_sidebar(cx, -210.);
    click(cx, "bot-avatar-search");
    cx.simulate_input("Mira");
    stand_in.set_down(true);
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert!(bounds(cx, "blocked-unreached").is_some());
    assert!(bounds(cx, "sidebar-quick-search").is_none());
    cx.simulate_keystrokes("cmd-shift-f");
    assert!(bounds(cx, "sidebar-quick-search").is_none());
    assert!(bounds(cx, "blocked-unreached").is_some());
    assert_eq!(area(&workspace, cx), Area::Bots);
}

#[gpui::test]
fn compact_sidebar_keyboard_search_reveals_strip_and_accepts_input(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "rail-bots");
    drag_sidebar(cx, -210.);
    click(cx, "bot-avatar-search");
    cx.simulate_input("redis");
    click(cx, "sidebar-search-result-ses_limits");
    cx.simulate_keystrokes("cmd-b");
    assert!(bounds(cx, "area-sidebar").is_none());
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("Mira");
    assert!(bounds(cx, "sidebar-quick-search").is_some());
    assert!(bounds(cx, "sidebar-search-result-bot:bot_mira").is_some());
    assert!(bounds(cx, "sidebar-search-result-bot:bot_inbox").is_none());
    assert_eq!(resizable_sidebar_width(cx), px(64.));
}

#[gpui::test]
fn compact_sidebar_width_leaves_room_for_the_conversation_with_both_panes(cx: &mut TestAppContext) {
    let (_, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    click(cx, "session-project-ses_signup");
    drag_sidebar(cx, 400.);
    click(cx, "toggle-panel");
    cx.simulate_resize(size(px(820.), px(800.)));
    cx.run_until_parked();
    assert!(bounds(cx, "area-sidebar").is_some());
    assert!(bounds(cx, "panel-edge").is_some());
    assert!(bounds(cx, "workspace-content").unwrap().size.width >= px(360.));
    drag_sidebar(cx, 100.);
    assert!(bounds(cx, "workspace-content").unwrap().size.width >= px(360.));
    cx.simulate_resize(size(px(1400.), px(800.)));
    cx.run_until_parked();
    assert_eq!(resizable_sidebar_width(cx), px(440.));
}

#[gpui::test]
fn compact_sidebar_selection_follows_the_open_sessions_bot(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_facts("ses_signup", |facts| facts["bot"] = serde_json::json!("bot_inbox"));
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    drag_sidebar(cx, -210.);
    click(cx, "bot-avatar-bot:bot_mira");
    assert!(bounds(cx, "bot-avatar-bot:bot_mira-selected").is_some());
    click(cx, "bot-avatar-search");
    cx.simulate_input("sign-up");
    click(cx, "sidebar-search-result-ses_signup");
    assert!(bounds(cx, "bot-avatar-bot:bot_inbox-selected").is_some());
    assert!(bounds(cx, "bot-avatar-bot:bot_mira-selected").is_none());
    click(cx, "bot-avatar-search");
    cx.simulate_input("redis");
    click(cx, "sidebar-search-result-ses_limits");
    assert!(bounds(cx, "bot-avatar-bot:bot_mira-selected").is_none());
    assert!(bounds(cx, "bot-avatar-bot:bot_inbox-selected").is_none());
}

#[gpui::test]
fn compact_sidebar_shows_a_scheduled_wake_badge_and_tooltip(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_bot_now("bot_mira", serde_json::json!({"working":0,"asking":0,"state":"idle"}));
    stand_in.set_bot_wakes("bot_mira", vec![serde_json::json!({
        "id": "at-build", "due": NOW / 1000. + 180., "summary": "Check the build", "cancelKey": "build",
    })], vec![]);
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    drag_sidebar(cx, -210.);

    let avatar = bounds(cx, "bot-avatar-bot:bot_mira").unwrap();
    let face = bounds(cx, "bot-avatar-face-bot:bot_mira").unwrap();
    let badge = bounds(cx, "bot-avatar-wake").unwrap();
    assert!(avatar.contains(&badge.center()));
    assert!(badge.top() >= face.bottom(), "compact mark should sit in gutter under face");
    cx.simulate_mouse_move(avatar.center(), None, Modifiers::default());
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(bounds(cx, "tooltip-Mira · Wakes in 3 min").is_some());
}

#[gpui::test]
fn compact_sidebar_marks_remote_bot_changes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.refuse_bot_edit("bot_wren", "lab", "The change was declined.");
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");

    // Full rows: offline Scout (with a waiting change) shows Offline, online Wren (refused) shows ChangeRefused.
    let scout_row_face = bounds(cx, "bot-bot_scout-avatar").unwrap();
    let wren_row_face = bounds(cx, "bot-bot_wren-avatar").unwrap();
    let row_offline = bounds(cx, "bot-avatar-offline").unwrap();
    let row_refused = bounds(cx, "bot-avatar-change-refused").unwrap();
    assert!(row_offline.left() >= scout_row_face.right()
        && row_offline.center().y >= scout_row_face.top()
        && row_offline.center().y <= scout_row_face.bottom(),
        "full row should show offline mark for offline Scout");
    assert!(row_refused.left() >= wren_row_face.right()
        && row_refused.center().y >= wren_row_face.top()
        && row_refused.center().y <= wren_row_face.bottom(),
        "full row should show refused mark for online Wren");
    assert!(bounds(cx, "bot-avatar-change-waiting").is_none());

    // Compact strip must show the exact same marks in the gutter under each face (never NeedsYou for Scout).
    drag_sidebar(cx, -210.);
    let scout_avatar = bounds(cx, "bot-avatar-elsewhere:bot_scout").unwrap();
    let scout_face = bounds(cx, "bot-avatar-face-elsewhere:bot_scout").unwrap();
    let wren_avatar = bounds(cx, "bot-avatar-elsewhere:bot_wren").unwrap();
    let wren_face = bounds(cx, "bot-avatar-face-elsewhere:bot_wren").unwrap();
    let compact_offline = bounds(cx, "bot-avatar-offline").unwrap();
    let compact_refused = bounds(cx, "bot-avatar-change-refused").unwrap();
    assert!(scout_avatar.contains(&compact_offline.center()));
    assert!(compact_offline.top() >= scout_face.bottom(), "offline mark should sit under Scout's face");
    assert!(wren_avatar.contains(&compact_refused.center()));
    assert!(compact_refused.top() >= wren_face.bottom(), "refused mark should sit under Wren's face");
    assert!(bounds(cx, "bot-avatar-change-waiting").is_none());
    // NeedsYou mark must not appear on Scout's tile.
    let needs_you = bounds(cx, "bot-avatar-needs-you").unwrap();
    assert!(!scout_avatar.contains(&needs_you.center()), "offline Scout must not reuse the NeedsYou blue dot");

    // Hovering Scout still tells the user it is waiting for studio-mini.
    cx.simulate_mouse_move(scout_avatar.center(), None, Modifiers::default());
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(bounds(cx, "tooltip-Scout · Waiting for studio-mini").is_some());
}
