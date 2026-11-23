//! A conversation's replies as someone reads them (the user's polish pass, 11-22): a Work Queue item
//! named in a reply is a chip where it's written, and a Work Queue step's row carries its item's
//! chip, each opening the item in the Work Queue from the app's own window; one time, Copy and
//! Quote for a whole reply, shown at its end while the pointer is on any of it; and no "Worked
//! for" line under a finished turn. Against the stand-in box (nothing real is reached); the app
//! opens as it does on a Mac (its window's root is its opening, `Launch`). Clicks and moves go
//! through the window.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::conversation::ConversationView;
use kuvo::launch::{Launch, Opens};
use kuvo::queue::Shown;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::install::OnlyAttaches;
use kuvo_ui::selection::UnitId;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

/// 2026-11-14, 09:53 UTC.
const NOW: f64 = 1_794_650_000_000.0;
const SESSION: &str = "ses_recipes";
const NAMED: &str = "Next up is #234, then the rest.";

/// The app opened on `stand_in` as on a Mac (attaching to its box), showing `SESSION`.
fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, Entity<ConversationView>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let attach: Arc<dyn Fn() -> kuvo::launch::Attached + Send + Sync> = Arc::new(move || {
        let boxes = kuvo::home::stand_in_boxes(stand_in.clone());
        let first = boxes.nearest().expect("the stand-in is the nearest box");
        Ok((boxes, first))
    });
    let opens = Opens { installer: Err(OnlyAttaches("Kuvo is set up on this Mac.".into())), attach, waits_within: Duration::from_secs(20) };
    let window = cx.add_window(move |window, cx| Launch::new(opens, false, window, cx));
    let launch = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1400.)));
    cx.run_until_parked();
    let home = launch.read_with(cx, |launch, _| launch.home().cloned()).expect("the app opened");
    home.update_in(cx, |home, _, cx| {
        home.set_now(NOW, cx);
        home.read_at_once(cx);
    });
    cx.run_until_parked();
    let workspace = home.read_with(cx, |home, _| home.workspace().clone());
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once(SESSION, window, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().expect("the conversation").clone());
    (workspace, view, cx)
}

/// The stand-in with item #234 in `SESSION`'s queue, and a turn of its own at the end.
fn with_turn(content: serde_json::Value) -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.work_item_fixture("wqi_234", json!({ "owner": SESSION, "title": "Dark mode for the settings page", "status": "todo" }), None);
    stand_in.work_queue_mention_fixture(NAMED, json!([{ "start": 11, "end": 15, "number": 234 }]));
    stand_in.add_message(SESSION, json!({ "type": "user", "id": "msg_ask", "time": { "created": NOW - 60_000.0 }, "text": "What's next?" }));
    stand_in.add_message(SESSION, json!({ "type": "assistant", "id": "msg_turn", "time": { "created": NOW - 50_000.0, "completed": NOW - 20_000.0 }, "finish": "stop", "content": content }));
    stand_in.add_message(SESSION, json!({ "type": "idle", "id": "msg_done", "time": { "created": NOW - 20_000.0 }, "outcome": "succeeded" }));
    stand_in
}

fn text(words: &str) -> serde_json::Value {
    json!({ "type": "text", "text": words })
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn hover(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_mouse_move(at.center(), None, Modifiers::default());
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

/// Item #234 is open, picked, in the Work Queue.
fn shows_item_234(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Queue, "the Work Queue is shown");
    let page = workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Queue).and_then(|view| view.read(cx).queue_page().cloned()).expect("the Work Queue"));
    assert_eq!(page.read_with(cx, |page, _| page.shown().clone()), Shown::Item("wqi_234".into()));
    assert!(bounds(cx, "item-page").is_some(), "its page");
}

#[gpui::test]
fn an_item_named_in_a_reply_is_a_chip_where_its_written_and_opens_the_item(cx: &mut TestAppContext) {
    let (workspace, view, cx) = open(cx, with_turn(json!([text(NAMED)])));
    let chip = bounds(cx, "item-chip-234").expect("the item's chip");
    // In the words, where "#234" is written: its number drawn on the chip, its title after it.
    let row = "msg_turn";
    let (number_at, title_at, rest_at) = view.read_with(cx, |view, _| {
        let units = view.units();
        let (_, units) = units.iter().find(|(key, _)| key.as_ref() == row).expect("the reply's row");
        let unit = units.iter().find(|unit| unit.text.contains("#234")).unwrap_or_else(|| panic!("#234 isn't in {units:?}"));
        let id = UnitId::new(row.to_owned(), unit.path.as_slice());
        let point = |needle: &str| view.selection().point_for(&id, unit.text.find(needle).unwrap_or_else(|| panic!("{needle:?} isn't in {:?}", unit.text)) + 1).expect("drawn");
        (point("#234"), point("Dark"), point("then the rest"))
    });
    assert!(chip.contains(&number_at), "its number is on the chip: {chip:?} / {number_at:?}");
    assert!(chip.contains(&title_at), "and its title: {chip:?} / {title_at:?}");
    assert!(!chip.contains(&rest_at) && rest_at.x > chip.right(), "the words go on after it, on its line");
    // Copied, it reads as words.
    view.update_in(cx, |view, window, cx| assert!(view.select_text("Next", "rest.", window, cx)));
    assert_eq!(view.read_with(cx, |view, _| view.selected_text()), "Next up is #234 Dark mode for the settings page, then the rest.");
    view.update(cx, |view, cx| {
        view.selection().clear();
        cx.notify();
    });
    cx.run_until_parked();

    click(cx, "item-chip-234");
    shows_item_234(&workspace, cx);
}

/// The user's (11-23, beta.46): "(#975)" in a reply drew its chip between the parentheses. The chip
/// takes their place; words round it keep theirs.
#[gpui::test]
fn an_items_chip_in_parentheses_drops_them(cx: &mut TestAppContext) {
    const WRAPPED: &str = "Next up (#234), then the rest.";
    let stand_in = with_turn(json!([text(WRAPPED)]));
    stand_in.work_queue_mention_fixture(WRAPPED, json!([{ "start": 9, "end": 13, "number": 234 }]));
    let (_, view, cx) = open(cx, stand_in);
    assert!(bounds(cx, "item-chip-234").is_some(), "the item's chip");
    view.update_in(cx, |view, window, cx| assert!(view.select_text("Next", "rest.", window, cx)));
    assert_eq!(view.read_with(cx, |view, _| view.selected_text()), "Next up #234 Dark mode for the settings page, then the rest.");
}

/// The user's flicker (11-22, beta.42): an item's chips in a conversation were one element to the
/// window, sharing one hover, so pointing at one opened a card under every chip of that item, and
/// the cards blinked off and on while the pointer rested there. Chips now differ from row to row,
/// but two in one text (an item named twice in a sentence) still shared theirs. One card opens,
/// by the chip pointed at, and stays while the pointer is on it.
#[gpui::test]
fn pointing_at_one_of_an_items_chips_opens_one_card_that_stays(cx: &mut TestAppContext) {
    const TWICE: &str = "Next up is #234, then #234 again.";
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.work_item_fixture("wqi_234", json!({ "owner": SESSION, "title": "Dark mode for the settings page", "status": "todo" }), None);
    stand_in.work_queue_mention_fixture(TWICE, json!([{ "start": 11, "end": 15, "number": 234 }, { "start": 22, "end": 26, "number": 234 }]));
    stand_in.add_message(SESSION, json!({ "type": "user", "id": "msg_ask", "time": { "created": NOW - 60_000.0 }, "text": "What's next?" }));
    stand_in.add_message(SESSION, json!({ "type": "assistant", "id": "msg_turn", "time": { "created": NOW - 50_000.0, "completed": NOW - 40_000.0 }, "finish": "stop", "content": [text(TWICE)] }));
    // A long reply after it, so it sits mid-window, clear of the composer.
    let long = (1..=24).map(|n| format!("Line {n} of the plan.")).collect::<Vec<_>>().join("\n\n");
    stand_in.add_message(SESSION, json!({ "type": "assistant", "id": "msg_long", "time": { "created": NOW - 15_000.0, "completed": NOW - 12_000.0 }, "finish": "stop", "content": [text(&long)] }));
    stand_in.add_message(SESSION, json!({ "type": "idle", "id": "msg_done", "time": { "created": NOW - 10_000.0 }, "outcome": "succeeded" }));
    let (_workspace, view, cx) = open(cx, stand_in);
    // (The conversation settled where it shows: drawn again once it's laid out.)
    cx.simulate_mouse_move(gpui::point(px(640.), px(200.)), None, Modifiers::default());
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    // Where each "#234" is written: on its chip.
    let named: Vec<_> = view.read_with(cx, |view, _| {
        view.units().iter().filter(|(key, _)| key.starts_with("msg_turn")).flat_map(|(key, units)| {
            units.iter().flat_map(|unit| unit.text.match_indices("#234").map(|(at, _)| {
                view.selection().point_for(&UnitId::new(key.to_string(), unit.path.as_slice()), at + 1).expect("drawn")
            }).collect::<Vec<_>>())
        }).collect()
    });
    let [first, second] = named[..] else { panic!("named twice in the reply: {named:?}") };
    assert!(second.y > first.y + px(10.), "the second chip on the next line: {first:?} / {second:?}");

    cx.simulate_mouse_move(first, None, Modifiers::default());
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(320));
    cx.run_until_parked();
    // Beside the chip pointed at (above it, or below the line under it), and only there.
    let by_first = |card: Bounds<Pixels>| (card.bottom() - first.y).abs() < px(30.) || (card.top() - first.y) > px(0.) && (card.top() - first.y) < px(30.);
    let card = bounds(cx, "queue-item-hover-card").expect("its card opens");
    assert!(by_first(card), "by the chip pointed at, and only there: {card:?} / {first:?} (the other chip at {second:?})");

    // The pointer rests on the chip (a hair's move, as a resting hand makes): the card stays.
    for step in 1..=3 {
        cx.simulate_mouse_move(first + gpui::point(px(step as f32), px(0.)), None, Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        let card = bounds(cx, "queue-item-hover-card").unwrap_or_else(|| panic!("the card blinked off after move {step}"));
        assert!(by_first(card), "still by the chip after move {step}: {card:?}");
    }
}

#[gpui::test]
fn a_work_queue_steps_row_carries_its_items_chip_which_opens_the_item(cx: &mut TestAppContext) {
    let step = json!({ "type": "tool", "id": "call_q1", "name": "kuvo-browser_queue_update", "state": { "status": "completed", "input": { "item": "#234", "status": "in-progress" } } });
    let (workspace, _view, cx) = open(cx, with_turn(json!([step, text("Started on it.")])));
    let row = bounds(cx, "tool-msg_turn:0").expect("the step's row");
    let chip = bounds(cx, "row-item-chip-234").expect("its item's chip, in its row");
    assert!(row.contains(&chip.center()), "in the row: {row:?} / {chip:?}");
    assert!(bounds(cx, "item-chip-234").is_none(), "and nowhere else");
    click(cx, "row-item-chip-234");
    shows_item_234(&workspace, cx);
}

#[gpui::test]
fn a_reply_has_one_time_copy_and_quote_at_its_end_for_all_of_it(cx: &mut TestAppContext) {
    let step = json!({ "type": "tool", "id": "call_r", "name": "read", "state": { "status": "completed", "input": { "filePath": "/src/main.rs" } } });
    let (_workspace, view, cx) = open(cx, with_turn(json!([text("First I looked."), step, text("Then I **fixed** it.")])));
    let (first, last) = ("msg_turn", "msg_turn#2");
    assert!(bounds(cx, &format!("entry-{first}")).is_some() && bounds(cx, &format!("entry-{last}")).is_some(), "both parts drawn");
    // The pointer on the reply's first words: the reply's one row shows, at its end, and none
    // under the part it's on.
    hover(cx, &format!("entry-{first}"));
    assert!(bounds(cx, &format!("message-actions-{first}")).is_none(), "no row under each part");
    let actions = bounds(cx, &format!("message-actions-{last}")).expect("the reply's row, at its end");
    assert!(actions.top() >= bounds(cx, &format!("entry-{last}")).unwrap().bottom() - px(24.), "under its last words");
    // On what it did between them, too.
    hover(cx, "entry-msg_turn#1");
    assert!(bounds(cx, &format!("message-actions-{last}")).is_some());
    // Copy takes the whole reply, as written.
    hover(cx, &format!("entry-{last}"));
    click(cx, &format!("message-copy-{last}"));
    let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
    assert_eq!(copied, "First I looked.\n\nThen I **fixed** it.");
    // Quote, the whole reply's words.
    click(cx, &format!("message-quote-{last}"));
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    let quoted = composer.read_with(cx, |composer, cx| composer.quotes(cx).iter().map(|quote| quote.text.clone()).collect::<Vec<_>>());
    assert_eq!(quoted, ["First I looked.\n\nThen I fixed it."]);
}

#[gpui::test]
fn a_finished_turn_says_nothing_after_the_reply(cx: &mut TestAppContext) {
    let (_workspace, _view, cx) = open(cx, with_turn(json!([text("Done.")])));
    assert!(bounds(cx, "entry-msg_turn").is_some(), "the reply is drawn");
    assert!(bounds(cx, "entry-msg_done").is_none(), "no \"Worked for\" line under it");
}
