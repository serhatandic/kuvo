//! Selecting and copying a conversation's text with the mouse and keys, as someone does it: events
//! go through the window as the platform sends them, and what's copied is read off the clipboard.
//! Against the stand-in box (nothing real is reached).

use gpui::{
    Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, TestAppContext, VisualTestContext,
    px, size,
};
use kuvo::conversation::ConversationView;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::messages::Message;
use kuvo_box::{BoxClient, Conversation, ConversationUpdate, Where};
use kuvo_ui::selection::UnitId;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

const NOW: f64 = 1_794_650_000_000.0;

fn open<'a>(cx: &'a mut TestAppContext, session: &str) -> (Entity<ConversationView>, &'a mut VisualTestContext) {
    open_at(cx, session, 1600.)
}

/// Opens `session` in a window `height` tall (the sign-up conversation shows whole in 1600).
fn open_at<'a>(cx: &'a mut TestAppContext, session: &str, height: f32) -> (Entity<ConversationView>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(StandIn::at(Replies::Never, NOW));
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(height)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        workspace.open_read_at_once(session, window, cx);
    });
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    (view, cx)
}

/// A conversation of its own, handed to the view as its follow would hand it.
fn show(view: &Entity<ConversationView>, messages: Value, cx: &mut VisualTestContext) {
    show_running(view, messages, false, cx);
}

fn show_running(view: &Entity<ConversationView>, messages: Value, running: bool, cx: &mut VisualTestContext) {
    let mut newest_first: Vec<Message> = serde_json::from_value(messages).unwrap();
    newest_first.reverse();
    let mut conversation = Conversation::new("ses_signup", Where::Engine);
    conversation.catch_up(newest_first, None, &[], Some(running));
    view.update(cx, |view, cx| view.take(ConversationUpdate::Conversation(conversation), cx));
    cx.run_until_parked();
}

/// Where `needle` (its start, or its end) shows in `row`'s unit at `path`.
fn at(view: &Entity<ConversationView>, row: &str, path: &[u32], needle: &str, end: bool, cx: &mut VisualTestContext) -> Point<Pixels> {
    view.read_with(cx, |view, _| {
        let units = view.units();
        let (_, units) = units.iter().find(|(key, _)| key.as_ref() == row).unwrap_or_else(|| panic!("no row {row}"));
        let unit = units.iter().find(|unit| unit.path.as_slice() == path).unwrap_or_else(|| panic!("no unit {path:?} in {row}: {units:?}"));
        let offset = unit.text.find(needle).unwrap_or_else(|| panic!("{needle:?} isn't in {:?}", unit.text)) + if end { needle.len() } else { 0 };
        view.selection().point_for(&UnitId::new(row.to_owned(), path), offset).unwrap_or_else(|| panic!("{row} {path:?} wasn't drawn"))
    })
}

fn drag(from: Point<Pixels>, to: Point<Pixels>, cx: &mut VisualTestContext) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
}

fn clicks(position: Point<Pixels>, count: usize, cx: &mut VisualTestContext) {
    for n in 1..=count {
        cx.simulate_event(MouseDownEvent { position, modifiers: Modifiers::default(), button: MouseButton::Left, click_count: n, first_mouse: false });
        cx.simulate_event(MouseUpEvent { position, modifiers: Modifiers::default(), button: MouseButton::Left, click_count: n });
    }
}

fn copied(cx: &mut VisualTestContext) -> String {
    cx.simulate_keystrokes("cmd-c");
    cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default()
}

#[gpui::test]
fn dragging_across_a_reply_and_pressing_cmd_c_copies_what_was_dragged_over(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    // From the reply's first word into its code block: the bold and inline code as plain text,
    // the code as it is.
    let from = at(&view, "msg_s2", &[3, 0], "Found", false, cx);
    let to = at(&view, "msg_s2", &[3, 1], "reset();", true, cx);
    drag(from, to, cx);
    assert_eq!(
        copied(cx),
        "Found it. When the server answers 409 (the email is taken), the error path calls reset() before it shows the message, so every field is cleared:\n\n} catch (error) {\n  reset();"
    );
}

#[gpui::test]
fn a_selection_runs_across_messages_and_copies_them_a_blank_line_apart(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let from = at(&view, "msg_s1", &[0], "completely", false, cx);
    let to = at(&view, "msg_s2", &[3, 0], "Found it", true, cx);
    drag(from, to, cx);
    // The steps between (thinking, reading, searching) are rows without text: they're skipped.
    assert_eq!(copied(cx), "completely wiped if the email is already taken. Can you look?\n\nFound it");
}

#[gpui::test]
fn a_double_click_takes_a_word_a_triple_click_a_line_and_shift_click_extends(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let word = at(&view, "msg_s2", &[3, 2], "success", false, cx);
    clicks(word, 2, cx);
    assert_eq!(copied(cx), "success");
    // In code, a triple click is the line of code.
    let line = at(&view, "msg_s2", &[3, 1], "setError", false, cx);
    clicks(line, 3, cx);
    assert_eq!(copied(cx), "  setError(error.message);");
    // A shift-click keeps where it began and goes to where it's clicked.
    let start = at(&view, "msg_s2", &[3, 2], "Moving", false, cx);
    clicks(start, 1, cx);
    let end = at(&view, "msg_s2", &[3, 2], "keeps", false, cx);
    cx.simulate_event(MouseDownEvent { position: end, modifiers: Modifiers::shift(), button: MouseButton::Left, click_count: 1, first_mouse: false });
    cx.simulate_event(MouseUpEvent { position: end, modifiers: Modifiers::shift(), button: MouseButton::Left, click_count: 1 });
    assert_eq!(copied(cx), "Moving reset() into the success path ");
}

#[gpui::test]
fn cmd_a_selects_the_message_youre_in_then_the_whole_conversation(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    show(
        &view,
        json!([
            { "type": "user", "id": "m1", "time": { "created": 1000 }, "text": "Fix the form" },
            { "type": "assistant", "id": "m2", "time": { "created": 2000 }, "content": [{ "type": "text", "text": "Done:\n\n- kept what was typed\n- the error shows under the field" }] }
        ]),
        cx,
    );
    clicks(at(&view, "m2", &[0, 0], "Done", false, cx), 1, cx);
    cx.simulate_keystrokes("cmd-a");
    assert_eq!(copied(cx), "Done:\n\n- kept what was typed\n- the error shows under the field");
    cx.simulate_keystrokes("cmd-a");
    assert_eq!(copied(cx), "Fix the form\n\nDone:\n\n- kept what was typed\n- the error shows under the field");
}

#[gpui::test]
fn a_link_opens_on_a_click_but_not_at_the_end_of_a_drag(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    show(&view, json!([{ "type": "assistant", "id": "m1", "time": { "created": 1000 }, "content": [{ "type": "text", "text": "See [the docs](https://kuvo.run/code/docs/) for more." }] }]), cx);
    let link = at(&view, "m1", &[0, 0], "docs", false, cx);
    drag(at(&view, "m1", &[0, 0], "the", false, cx), link, cx);
    assert_eq!(cx.opened_url(), None, "a drag ending on a link selects; it doesn't open it");
    assert_eq!(copied(cx), "the ");
    cx.simulate_click(link, Modifiers::default());
    assert_eq!(cx.opened_url().as_deref(), Some("https://kuvo.run/code/docs/"));
}

#[gpui::test]
fn a_selection_stays_put_while_the_reply_streams_on(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let step = |text: &str| json!([
        { "type": "user", "id": "m1", "time": { "created": 1000 }, "text": "Fix the form" },
        { "type": "assistant", "id": "m2", "time": { "created": 2000 }, "content": [{ "type": "text", "text": text }] }
    ]);
    show(&view, step("Moved the reset into the success path"), cx);
    drag(at(&view, "m2", &[0, 0], "reset", false, cx), at(&view, "m2", &[0, 0], "success", true, cx), cx);
    // More words come, then a new paragraph and a list.
    show(&view, step("Moved the reset into the success path, and the error now shows"), cx);
    show(&view, step("Moved the reset into the success path, and the error now shows.\n\n- one\n- two"), cx);
    assert_eq!(copied(cx), "reset into the success");
}

#[gpui::test]
fn opened_tool_output_selects_and_copies_like_any_text(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    // The test run's output, opened.
    view.update(cx, |view, cx| view.toggle(kuvo::entries::Fold::Step("msg_s5".to_owned(), 1), cx));
    cx.run_until_parked();
    let units = view.read_with(cx, |view, _| view.units());
    let (_, step) = units.iter().find(|(key, _)| key.as_ref() == "msg_s5").expect("the step");
    let body = step.iter().find(|unit| unit.path.as_slice() == [1, 1]).expect("what it printed");
    let first_line = body.text.lines().next().unwrap().to_owned();
    let from = at(&view, "msg_s5", &[1, 0], "$ npm", false, cx);
    let to = at(&view, "msg_s5", &[1, 1], &first_line, true, cx);
    drag(from, to, cx);
    assert_eq!(copied(cx), format!("$ npm test -- signup\n{first_line}"));
}

#[gpui::test]
fn a_click_in_the_transcript_that_selects_nothing_leaves_nothing_to_copy(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let word = at(&view, "msg_s2", &[3, 2], "success", false, cx);
    clicks(word, 2, cx);
    clicks(word, 1, cx);
    assert!(view.read_with(cx, |view, _| view.selection().is_empty()));
}

#[gpui::test]
fn dragging_past_the_top_scrolls_back_and_keeps_selecting(cx: &mut TestAppContext) {
    let (view, cx) = open_at(cx, "ses_signup", 700.);
    let list = view.read_with(cx, |view, _| view.list().list_state().clone());
    let bounds = list.viewport_bounds();
    let start = at(&view, "msg_s5", &[2, 0], "Moved", false, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let before = list.logical_scroll_top();
    // Held above the list: each frame scrolls back a little and the selection follows.
    let above = gpui::point(start.x, bounds.top() - px(80.));
    for _ in 0..40 {
        cx.simulate_mouse_move(above, MouseButton::Left, Modifiers::default());
    }
    cx.simulate_mouse_up(above, MouseButton::Left, Modifiers::default());
    let after = list.logical_scroll_top();
    assert!((after.item_ix, after.offset_in_item) < (before.item_ix, before.offset_in_item) || after.item_ix < before.item_ix, "it scrolled back: {before:?} → {after:?}");
    // From what came into view back above, to where the drag began.
    let text = view.read_with(cx, |view, _| view.selected_text());
    assert!(text.ends_with("Keep what they typed, and say which field is wrong"), "{text}");
    assert!(text.contains("Moving reset() into the success path"), "{text}");
}

#[gpui::test]
fn a_link_opens_though_the_pointer_wobbles_a_little_while_its_clicked(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    show(&view, json!([{ "type": "assistant", "id": "m1", "time": { "created": 1000 }, "content": [{ "type": "text", "text": "See [the docs](https://kuvo.run/code/docs/) for more." }] }]), cx);
    let link = at(&view, "m1", &[0, 0], "docs", false, cx);
    let wobbled = gpui::point(link.x + px(2.), link.y + px(1.));
    cx.simulate_mouse_down(link, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(wobbled, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(wobbled, MouseButton::Left, Modifiers::default());
    assert_eq!(cx.opened_url().as_deref(), Some("https://kuvo.run/code/docs/"));
}

#[gpui::test]
fn double_clicking_a_steps_row_leaves_the_selection_and_the_composers_keyboard_alone(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let composer = view.read_with(cx, |view, cx| view.composer().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&gpui::Focusable::focus_handle(composer.read(cx), cx), cx));
    let row = cx.debug_bounds("tool-msg_s5:1").expect("the test run's row");
    clicks(row.center(), 2, cx);
    assert!(view.read_with(cx, |view, _| view.selection().is_empty()), "nothing selected");
    let typing = cx.update(|window, cx| gpui::Focusable::focus_handle(composer.read(cx), cx).is_focused(window));
    assert!(typing, "the composer still has the keyboard");
    // A drag that starts there still selects.
    let to = at(&view, "msg_s5", &[2, 0], "Moved the reset", true, cx);
    drag(row.center(), to, cx);
    assert_eq!(copied(cx), "Moved the reset");
}

#[gpui::test]
fn cmd_a_twice_while_the_agent_works_still_copies_once_the_turn_ends(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let messages = json!([
        { "type": "user", "id": "m1", "time": { "created": 1000 }, "text": "Fix the form" },
        { "type": "assistant", "id": "m2", "time": { "created": 2000 }, "content": [{ "type": "text", "text": "On it." }] }
    ]);
    show_running(&view, messages.clone(), true, cx);
    clicks(at(&view, "m2", &[0, 0], "On", false, cx), 1, cx);
    cx.simulate_keystrokes("cmd-a cmd-a");
    // The turn ends: "Working…" goes.
    show_running(&view, messages, false, cx);
    assert_eq!(copied(cx), "Fix the form\n\nOn it.");
}
