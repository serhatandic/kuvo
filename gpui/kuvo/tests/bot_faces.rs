//! The bots drawn live, as someone sees them, against the stand-in box (nothing real is reached):
//! each in the state its box says, a change played while it's on screen, nothing scheduled
//! between an idle bot's one-shots or with Reduce Motion on, Done until its conversation is seen
//! (the box told, for every client), and the copies of one bot moving as one. Clicks go through
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
    for _ in 0..(seconds * 60.) as usize {
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
    let symbol = cx.update(|_, cx| kuvo::bot_face::pose_of("bot_release", 28., cx)).unwrap();
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
    let pose = cx.update(|_, cx| kuvo::bot_face::pose_of("bot_release", 28., cx)).unwrap();
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
    // Opening a bot that isn't done or blocked tells the box nothing.
    click(cx, "bot-bot_mira");
    click(cx, "bot-chat");
    assert!(stand_in.seen_at("POST", "kuvo/bots/bot_mira/seen").is_empty());

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

/// The copies of one bot are one creature: its conversation's header, opened later, moves with
/// its sidebar row.
#[gpui::test]
fn copies_of_one_bot_move_as_one(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    click(cx, "bot-bot_release");
    box_says(&stand_in, &workspace, cx, "bot_release", json!({ "working": 1, "asking": 0, "state": "working", "since": NOW }));
    run_for(cx, 0.1);
    click(cx, "bot-chat");
    assert!(bounds(cx, "bot-chat-strip").is_some() && bounds(cx, "bot-bot_release").is_some(), "the header's copy and the row's, both on screen");
    run_for(cx, 0.05);
    let (row, header) = cx.update(|_, cx| (kuvo::bot_face::pose_of("bot_release", 28., cx).unwrap(), kuvo::bot_face::pose_of("bot_release", 22., cx).unwrap()));
    assert_eq!((row.open, row.gx, row.gy, row.turn), (header.open, header.gx, header.gy, header.turn), "the same blink, the same glance");
    // Another bot moves on its own.
    let other = cx.update(|_, cx| kuvo::bot_face::pose_of("bot_inbox", 28., cx).unwrap());
    assert_ne!((other.gx, other.gy), (row.gx, row.gy));
}
