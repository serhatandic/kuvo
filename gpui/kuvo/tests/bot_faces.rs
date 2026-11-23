//! The bots drawn live, as someone sees them, against the stand-in box (nothing real is reached):
//! each in the state its box says, a change played while it's on screen, nothing scheduled
//! between an idle bot's one-shots or with Reduce Motion on, Done until its conversation is seen
//! (the box told, for every client), nothing replayed on a later copy or a reconnect, and the
//! box followed live. Clicks go through
//! the window as AppKit hands them on; time is the test clock's, frames are delivered by hand.

use std::time::Duration;

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::bot::State;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

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
    cx.simulate_resize(size(px(1280.), px(900.)));
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

/// The state `bot` is drawn in now.
fn drawn(cx: &mut VisualTestContext, bot: &str) -> Option<State> {
    cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().shown(bot))
}

/// What the box says of `bot` now, read again as Bots would on its next line.
fn box_says(stand_in: &StandIn, workspace: &Entity<Workspace>, cx: &mut VisualTestContext, bot: &str, now: serde_json::Value) {
    stand_in.set_bot_now(bot, now);
    let view = workspace.read_with(cx, |workspace, _| workspace.area_view(Area::Bots).cloned()).expect("Bots");
    view.update(cx, |view, cx| view.shown(cx));
    cx.run_until_parked();
}

/// Bot frames drawn so far.
fn frames(cx: &mut VisualTestContext) -> u64 {
    cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().frames)
}

/// `seconds` of the test clock at 60 Hz, each frame delivered: how many ticks drew bots, and the
/// longest run of ticks that drew none.
fn run_for(cx: &mut VisualTestContext, seconds: f32) -> (usize, usize) {
    let (mut drew, mut gap, mut longest) = (0, 0, 0);
    for _ in 0..(seconds * 60.).round() as usize {
        let before = frames(cx);
        cx.executor().advance_clock(Duration::from_micros(16_667));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if frames(cx) > before {
            drew += 1;
            longest = longest.max(gap);
            gap = 0;
        } else {
            gap += 1;
        }
    }
    (drew, longest.max(gap))
}

fn idle(since: f64) -> serde_json::Value {
    json!({ "working": 0, "asking": 0, "state": "idle", "since": since })
}

/// Each bot is drawn in the state its box says, with no spinner or badge beside it (the bot says
/// it); a change the box says while it's on screen plays at once.
#[gpui::test]
fn a_bot_is_drawn_as_its_box_says_and_a_change_plays_on_screen(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    assert_eq!(drawn(cx, "bot_mira"), Some(State::NeedsYou), "at work and waiting on you: needs you, the box's precedence");
    assert_eq!(drawn(cx, "bot_docs"), Some(State::Done));
    assert_eq!(drawn(cx, "bot_release"), Some(State::Idle));
    for bot in ["bot_mira", "bot_inbox", "bot_release", "bot_docs"] {
        assert!(bounds(cx, &format!("bot-asking-{bot}")).is_none() && bounds(cx, &format!("bot-working-{bot}")).is_none(), "{bot}: no spinner or badge");
    }
    // A bot on a computer that's away is offline; one whose computer isn't known, idle.
    assert_eq!(drawn(cx, "bot_scout"), Some(State::Offline));
    assert_eq!(drawn(cx, "bot_wren"), Some(State::Idle));
    run_for(cx, 3.);

    box_says(&stand_in, &workspace, cx, "bot_release", json!({ "working": 1, "asking": 0, "thinking": 1, "state": "thinking", "since": NOW }));
    assert_eq!(drawn(cx, "bot_release"), Some(State::Thinking));
    let (drew, _) = run_for(cx, 1.5);
    assert!(drew > 10, "the change plays: its blink, the symbol coming in ({drew} frames)");
    let symbol = cx.update(|_, cx| kuvo::bot_face::pose_of("bot_release", 40., cx)).unwrap();
    assert!(symbol.glyph_amt > 0.9, "the orbit's in");
}

/// Idle bots ask for frames only during their blinks and glances: seconds go by with nothing
/// drawn, and most of a minute draws nothing at all.
#[gpui::test]
fn idle_bots_draw_nothing_between_their_one_shots(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    for bot in ["bot_mira", "bot_inbox", "bot_release", "bot_docs"] {
        stand_in.set_bot_now(bot, idle(NOW - 3_600_000.));
    }
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    let (drew, longest) = run_for(cx, 60.);
    assert!(drew > 0, "they blink and glance");
    assert!(drew < 60 * 60 / 2, "frames only during one-shots: {drew} of 3600 ticks");
    assert!(longest >= 60, "a second or more with nothing drawn (longest: {longest} ticks)");
}

/// With Reduce Motion on, each bot shows its state's still pose: nothing is ever scheduled, and a
/// change swaps the pose at once.
#[gpui::test]
fn reduce_motion_shows_still_poses_and_schedules_nothing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    cx.update(|cx| cx.set_reduce_motion(true));
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    let (drew, _) = run_for(cx, 20.);
    assert_eq!(drew, 0);
    assert!(!cx.update(|_, cx| kuvo::bot_face::faces(cx).borrow().scheduled()));
    box_says(&stand_in, &workspace, cx, "bot_release", json!({ "working": 0, "asking": 0, "blocked": 1, "blockedBy": "limit", "state": "blocked", "since": NOW }));
    let (drew, _) = run_for(cx, 2.);
    assert_eq!(drew, 0);
    let pose = cx.update(|_, cx| kuvo::bot_face::pose_of("bot_release", 40., cx)).unwrap();
    assert_eq!(pose.glyph_amt, 1., "the bar at once");
    assert!(bounds(cx, "bot-bot_release").is_some());
}

/// Done lasts until you've seen it: opening the bot's conversation tells its box, which says it's
/// idle again (for every client). Seen at once, it still shows its celebration first.
#[gpui::test]
fn done_lasts_until_its_conversation_is_seen(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    // Opening a bot's conversation tells its box, once; what that clears is the box's to say (one
    // that waits on you still does).
    click(cx, "bot-bot_mira");
    click(cx, "bot-chat");
    assert_eq!(stand_in.seen_at("POST", "kuvo/bots/bot_mira/seen").len(), 1);
    assert_eq!(drawn(cx, "bot_mira"), Some(State::NeedsYou));

    assert_eq!(drawn(cx, "bot_docs"), Some(State::Done));
    click(cx, "bot-bot_docs");
    assert_eq!(drawn(cx, "bot_docs"), Some(State::Done), "its page isn't its conversation");
    click(cx, "bot-chat");
    assert_eq!(stand_in.seen_at("POST", "kuvo/bots/bot_docs/seen").len(), 1, "its conversation shown: seen");
    let view = workspace.read_with(cx, |workspace, _| workspace.area_view(Area::Bots).cloned()).unwrap();
    view.update(cx, |view, cx| view.shown(cx));
    cx.run_until_parked();
    assert_eq!(drawn(cx, "bot_docs"), Some(State::Idle));

    // One that finishes while you watch: the celebration plays even though it's seen at once.
    run_for(cx, 2.);
    box_says(&stand_in, &workspace, cx, "bot_release", json!({ "working": 0, "asking": 0, "finished": NOW, "state": "done", "since": NOW }));
    box_says(&stand_in, &workspace, cx, "bot_release", idle(NOW));
    assert_eq!(drawn(cx, "bot_release"), Some(State::Done), "held through its celebration");
    run_for(cx, 1.6);
    assert_eq!(drawn(cx, "bot_release"), Some(State::Idle), "then idle, as the box says");
}

/// Nothing replays: a copy drawn after a change (its conversation's header, opened later) shows
/// the state settled, and bots coming back from offline (the box reached again after a sleep)
/// don't play what they did meanwhile, Done's celebration least of all.
#[gpui::test]
fn nothing_replays_on_a_later_copy_or_a_reconnect(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    click(cx, "bot-bot_release");
    box_says(&stand_in, &workspace, cx, "bot_release", json!({ "working": 0, "asking": 0, "finished": NOW, "state": "done", "since": NOW }));
    let celebrating = |cx: &mut VisualTestContext, size: f32| cx.update(|_, cx| kuvo::bot_face::pose_of("bot_release", size, cx).map(|pose| pose.burst > 0. || pose.spin.abs() > 0.01).unwrap_or(false));
    run_for(cx, 0.5);
    assert!(celebrating(cx, 40.), "seen finishing in the sidebar: it celebrates");
    run_for(cx, 2.);
    click(cx, "bot-chat");
    assert!(bounds(cx, "bot-chat-title").is_some());
    for _ in 0..30 {
        run_for(cx, 1. / 60.);
        assert!(!celebrating(cx, 22.), "the header's copy, drawn later, doesn't celebrate again");
    }

    // The box can't be reached, then is again: every bot is offline, then back as it is, still.
    let view = workspace.read_with(cx, |workspace, _| workspace.area_view(Area::Bots).cloned()).unwrap();
    assert!(bounds(cx, "bot-bot_docs").is_some(), "Docs Keeper's row on screen");
    stand_in.bots_readable(false);
    view.update(cx, |view, cx| view.shown(cx));
    cx.run_until_parked();
    assert_eq!(drawn(cx, "bot_docs"), Some(State::Offline));
    run_for(cx, 2.);
    stand_in.bots_readable(true);
    view.update(cx, |view, cx| view.shown(cx));
    cx.run_until_parked();
    assert_eq!(drawn(cx, "bot_docs"), Some(State::Done));
    for _ in 0..90 {
        run_for(cx, 1. / 60.);
        let docs = cx.update(|_, cx| kuvo::bot_face::pose_of("bot_docs", 40., cx).unwrap());
        assert!(docs.burst == 0. && docs.spin.abs() < 0.01 && docs.y.abs() < 0.05, "back from offline: no celebration replayed");
    }
}

/// The app follows the box: a bot's state, as its box says it on `kuvo/bots/events`, is drawn as
/// it comes, with nothing read again by hand.
#[gpui::test]
fn the_faces_follow_the_box_live(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, true, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    workspace.update_in(cx, |workspace, window, cx| workspace.show_area(Area::Bots, window, cx));
    let view = workspace.read_with(cx, |workspace, _| workspace.area_view(Area::Bots).cloned()).unwrap();
    until(cx, &view, |cx| drawn(cx, "bot_release") == Some(State::Idle));
    stand_in.set_bot_now("bot_release", json!({ "working": 1, "asking": 0, "thinking": 1, "state": "thinking", "since": NOW }));
    until(cx, &view, |cx| drawn(cx, "bot_release") == Some(State::Thinking));
    stand_in.set_bot_now("bot_release", json!({ "working": 0, "asking": 0, "blocked": 1, "blockedBy": "error", "state": "blocked", "since": NOW }));
    until(cx, &view, |cx| drawn(cx, "bot_release") == Some(State::Blocked));
    assert!(bounds(cx, "bot-bot_release").is_some());
}

#[gpui::test]
fn a_failed_sessions_list_leaves_each_bots_source_independent(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.sessions_readable(false);
    stand_in.bots_readable(false);
    let peer = stand_in.peer_box("key_devbox").expect("devbox");
    stand_in.move_bot("bot_release", "devbox");
    peer.add_bot("bot_release", "Release Captain", json!({ "color": "lime", "shape": "pebble", "face": "calm" }));
    peer.set_bot_now("bot_release", json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }));

    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, true, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    workspace.update_in(cx, |workspace, window, cx| workspace.show_area(Area::Bots, window, cx));
    until(cx, &workspace, |cx| {
        workspace.read_with(cx, |workspace, _| matches!(workspace.lists(), kuvo::workspace::Lists::Failed(_)))
    });
    stand_in.bots_readable(true);
    until(cx, &workspace, |cx| bounds(cx, "bot-bot_docs").is_some());
    assert_eq!(drawn(cx, "bot_docs"), Some(State::Done));
    until(cx, &workspace, |cx| drawn(cx, "bot_release") == Some(State::Working));
    assert_eq!(drawn(cx, "bot_scout"), Some(State::Offline), "Computers lists Studio as away");
    assert!(bounds(cx, "bot-bot_docs").is_some());
    assert!(bounds(cx, "elsewhere-bot_release").is_some());

    // Only devbox's Bots feed fails. The local bot keeps its state.
    peer.bots_readable(false);
    until(cx, &workspace, |cx| drawn(cx, "bot_release") == Some(State::Offline));
    assert_eq!(drawn(cx, "bot_docs"), Some(State::Done));

    // The same peer snapshot restores its face while Sessions still fails.
    peer.bots_readable(true);
    until(cx, &workspace, |cx| drawn(cx, "bot_release") == Some(State::Working));
    assert_eq!(drawn(cx, "bot_scout"), Some(State::Offline));
    assert!(workspace.read_with(cx, |workspace, _| matches!(workspace.lists(), kuvo::workspace::Lists::Failed(_))));
}

/// A bot on a computer that's away says when it was last seen, never "Can’t be reached"; once
/// its box says the computer answers again, the row is live with no step of yours.
#[gpui::test]
fn a_bot_on_a_computer_that_comes_back_goes_live_as_its_box_says(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    // Release Captain lives on studio-mini, which hasn't been heard from for three hours.
    stand_in.move_bot("bot_release", "studio-mini");
    stand_in.bot_computer_away("bot_release", Some(NOW - 3.0 * 3_600_000.0));
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, true, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.show_area(Area::Bots, window, cx);
    });
    until(cx, &workspace, |cx| bounds(cx, "elsewhere-bot_release").is_some() && drawn(cx, "bot_release").is_some());
    assert_eq!(drawn(cx, "bot_release"), Some(State::Offline));
    assert!(bounds(cx, "bot-said-bot_release-seen 3h").is_some(), "when studio-mini was last seen");

    // It answers again: live, with no step of yours.
    stand_in.bot_computer_away("bot_release", None);
    until(cx, &workspace, |cx| drawn(cx, "bot_release") != Some(State::Offline));
    assert!(bounds(cx, "bot-said-bot_release-seen 3h").is_none(), "live again: no last seen");
}

/// A bot on a computer only switching to a new release isn't away: its row stays as it is, saying
/// "Updating…", never greyed or "seen now" (the user, 11-23, beta.45 to 46).
#[gpui::test]
fn a_bot_on_a_computer_switching_to_a_new_release_stays_live_saying_updating(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.move_bot("bot_release", "studio-mini");
    stand_in.bot_computer_updating("bot_release", true);
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, true, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.show_area(Area::Bots, window, cx);
    });
    until(cx, &workspace, |cx| bounds(cx, "elsewhere-bot_release").is_some() && drawn(cx, "bot_release").is_some());
    assert_ne!(drawn(cx, "bot_release"), Some(State::Offline), "not greyed as away");
    assert!(bounds(cx, "bot-said-bot_release-Updating…").is_some(), "says it's updating");
}

/// Waits, as the live follows hand on what they read, until `done`.
fn until<T: 'static>(cx: &mut VisualTestContext, view: &Entity<T>, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    use futures::StreamExt as _;
    cx.executor().allow_parking();
    let mut told = cx.notifications(view);
    // A guard, not a clock the test leans on: a follow that never hands anything on would leave it
    // waiting for good, so the run ends, saying so, rather than hanging unseen.
    let (finished, guard) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if guard.recv_timeout(std::time::Duration::from_secs(60)) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            eprintln!("the live follow handed nothing on for a minute: the test stops here");
            std::process::exit(101);
        }
    });
    let _finished = finished;
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        cx.foreground_executor().block_test(told.next());
    }
}
