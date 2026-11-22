//! The rail's rings and their cards as the box words them (#952, PROTOCOL.md "Cards and the
//! rail"): each card shows only its own account, a dot marks one on pace to run out or used up,
//! "Use a reset" shows only where the box offers one and asks first in a popover that moves
//! nothing, and an agent's many accounts are one ring whose card has a strip of them. The box is the
//! stand-in, answering as the box does; the pointer and clicks go through the window.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::stand_in::areas::UsageSample;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, sample: UsageSample) -> (StandIn, Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_usage_sample(sample);
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(900.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (stand_in, workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

/// Rests the pointer on a ring until its card opens.
fn rest_on(cx: &mut VisualTestContext, ring: &str) {
    let at = bounds(cx, ring).unwrap_or_else(|| panic!("{ring} isn't on screen")).center();
    cx.simulate_mouse_move(at, None, Modifiers::default());
    cx.executor().advance_clock(kuvo::rail::CARD_DELAY);
    cx.run_until_parked();
    for _ in 0..12 {
        cx.executor().advance_clock(std::time::Duration::from_micros(16_667));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    assert!(shown(cx, "ring-card"), "{ring}'s card opens");
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn each_rings_card_shows_only_its_own_account_and_a_dot_marks_one_running_out(cx: &mut TestAppContext) {
    let (_stand_in, _workspace, cx) = open(cx, UsageSample::Cards);
    // Claude · Max is on pace to run out, Claude · $20 is used up, Codex lasts.
    assert!(shown(cx, "ring-0-dot") && shown(cx, "ring-1-dot"));
    assert!(!shown(cx, "ring-2-dot"), "Codex lasts until its resets: no dot");

    rest_on(cx, "ring-0");
    assert!(shown(cx, "card-claude-Max-line-five_hour") && shown(cx, "card-claude-Max-line-seven_day"));
    assert!(!shown(cx, "card-claude-$20-line-five_hour"), "nothing of Claude · $20 on Claude · Max's card");
    // 5-hour first, then Weekly.
    assert!(bounds(cx, "card-claude-Max-line-five_hour").unwrap().top() < bounds(cx, "card-claude-Max-line-seven_day").unwrap().top());
    assert!(!shown(cx, "ring-card-resets"), "Claude · Max has no reset to use");
}

#[gpui::test]
fn use_a_reset_asks_first_in_a_popover_that_moves_nothing_then_uses_it(cx: &mut TestAppContext) {
    let (stand_in, _workspace, cx) = open(cx, UsageSample::Cards);
    rest_on(cx, "ring-2");
    let (card, footer) = (bounds(cx, "ring-card").unwrap(), bounds(cx, "ring-card-resets").expect("Codex's resets, below its windows"));
    assert!(footer.top() >= bounds(cx, "card-codex-line-secondary").unwrap().bottom(), "below the windows");

    click(cx, "ring-card-use-reset");
    assert!(shown(cx, "ring-card-confirm"), "it asks first");
    assert_eq!(bounds(cx, "ring-card"), Some(card), "the card stays put");
    assert_eq!(bounds(cx, "ring-card-resets"), Some(footer), "the footer's row stays put");
    assert!(stand_in.seen_at("POST", "kuvo/agents/api/kuvo/limits/reset").is_empty(), "nothing used before it's confirmed");

    click(cx, "ring-card-confirm-use");
    let asked = stand_in.seen_at("POST", "kuvo/agents/api/kuvo/limits/reset");
    assert_eq!(asked.len(), 2, "offered, then confirmed once");
    assert_eq!(asked[1].body.as_ref().unwrap()["row"], "codex#pro");
    assert_eq!(asked[1].body.as_ref().unwrap()["confirm"], "rsc_stand_in");
    assert!(!shown(cx, "ring-card-confirm"));
}

#[gpui::test]
fn cancel_closes_the_confirm_and_uses_nothing(cx: &mut TestAppContext) {
    let (stand_in, _workspace, cx) = open(cx, UsageSample::Cards);
    rest_on(cx, "ring-2");
    click(cx, "ring-card-use-reset");
    click(cx, "ring-card-confirm-cancel");
    assert!(!shown(cx, "ring-card-confirm"));
    assert!(stand_in.seen_at("POST", "kuvo/agents/api/kuvo/limits/reset").is_empty());
}

#[gpui::test]
fn no_reset_offered_means_no_footer(cx: &mut TestAppContext) {
    let (_stand_in, _workspace, cx) = open(cx, UsageSample::CardsFew);
    // Claude, Antigravity's four, then Codex with no reset.
    rest_on(cx, "ring-2");
    assert!(shown(cx, "card-codex-line-primary"));
    assert!(!shown(cx, "ring-card-resets") && !shown(cx, "ring-card-use-reset"));
}

#[gpui::test]
fn an_agents_many_accounts_are_one_ring_and_its_card_shows_each_in_a_strip(cx: &mut TestAppContext) {
    let (_stand_in, _workspace, cx) = open(cx, UsageSample::Cards);
    // Two Claude, Codex, and Antigravity's twenty as one ring with their count.
    assert!(shown(cx, "ring-3") && shown(cx, "ring-3-count") && !shown(cx, "ring-4"));
    rest_on(cx, "ring-3");
    assert!(shown(cx, "ring-card-summary"));
    assert!((0..20).all(|ix| shown(cx, &format!("ring-strip-{ix}"))) && !shown(cx, "ring-strip-20"));
    // The one in use in full; resting on another shows that one instead.
    assert!(shown(cx, "card-antigravity-Lab 12-line-gemini-5h"));
    let first = bounds(cx, "ring-strip-0").unwrap().center();
    cx.simulate_mouse_move(first, None, Modifiers::default());
    cx.run_until_parked();
    assert!(shown(cx, "card-antigravity-Lab 07-line-gemini-5h"), "the emptiest, Lab 07, rested on");
    assert!(!shown(cx, "card-antigravity-Lab 12-line-gemini-5h"));
}
