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
    let from = at(&view, "msg_s2#3", &[3, 0], "Found", false, cx);
    let to = at(&view, "msg_s2#3", &[3, 1], "reset();", true, cx);
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
    let to = at(&view, "msg_s2#3", &[3, 0], "Found it", true, cx);
    drag(from, to, cx);
    // The steps between (thinking, reading, searching) are rows without text: they're skipped.
    assert_eq!(copied(cx), "completely wiped if the email is already taken. Can you look?\n\nFound it");
}

#[gpui::test]
fn a_double_click_takes_a_word_a_triple_click_a_line_and_shift_click_extends(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let word = at(&view, "msg_s2#3", &[3, 2], "success", false, cx);
    clicks(word, 2, cx);
    assert_eq!(copied(cx), "success");
    // In code, a triple click is the line of code.
    let line = at(&view, "msg_s2#3", &[3, 1], "setError", false, cx);
    clicks(line, 3, cx);
    assert_eq!(copied(cx), "  setError(error.message);");
    // A shift-click keeps where it began and goes to where it's clicked.
    let start = at(&view, "msg_s2#3", &[3, 2], "Moving", false, cx);
    clicks(start, 1, cx);
    let end = at(&view, "msg_s2#3", &[3, 2], "keeps", false, cx);
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

/// The menu bar's Edit › Copy and Select All reach the conversation's text as ⌘C and ⌘A do: chosen
/// as the Mac sends a menu's choice, to the window from what has the keyboard.
#[gpui::test]
fn the_edit_menus_copy_and_select_all_reach_the_conversations_text(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    show(
        &view,
        json!([
            { "type": "user", "id": "m1", "time": { "created": 1000 }, "text": "Fix the form" },
            { "type": "assistant", "id": "m2", "time": { "created": 2000 }, "content": [{ "type": "text", "text": "Done, and the error shows under the field" }] }
        ]),
        cx,
    );
    let edit = |name: &str| -> Box<dyn gpui::Action> {
        let menus = kuvo::app_menu::menus();
        let menu = menus.iter().find(|menu| menu.name.as_ref() == "Edit").expect("an Edit menu");
        menu.items
            .iter()
            .find_map(|item| match item {
                gpui::MenuItem::Action { name: each, action, .. } if each.as_ref() == name => Some(action.boxed_clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no Edit › {name}"))
    };
    let choose = |name: &str, cx: &mut VisualTestContext| {
        let action = edit(name);
        assert!(cx.update(|window, cx| window.is_action_available(action.as_ref(), cx)), "Edit › {name} is enabled in the conversation");
        cx.update(|window, cx| window.dispatch_action(action, cx));
        cx.run_until_parked();
    };
    drag(at(&view, "m2", &[0, 0], "the error", false, cx), at(&view, "m2", &[0, 0], "shows", true, cx), cx);
    choose("Copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("the error shows"));
    // As ⌘A: the message you're in, then the whole conversation.
    choose("Select All", cx);
    choose("Copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("Done, and the error shows under the field"));
    choose("Select All", cx);
    choose("Copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("Fix the form\n\nDone, and the error shows under the field"));
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
    view.update(cx, |view, cx| view.open_step("msg_s5", 4, cx));
    cx.run_until_parked();
    let units = view.read_with(cx, |view, _| view.units());
    let (_, step) = units.iter().find(|(key, _)| key.as_ref() == "msg_s5").expect("the step");
    let body = step.iter().find(|unit| unit.path.as_slice() == [4, 1]).expect("what it printed");
    let first_line = body.text.lines().next().unwrap().to_owned();
    let from = at(&view, "msg_s5", &[4, 0], "$ npm", false, cx);
    let to = at(&view, "msg_s5", &[4, 1], &first_line, true, cx);
    drag(from, to, cx);
    assert_eq!(copied(cx), format!("$ npm test -- signup\n{first_line}"));
}

#[gpui::test]
fn a_click_in_the_transcript_that_selects_nothing_leaves_nothing_to_copy(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let word = at(&view, "msg_s2#3", &[3, 2], "success", false, cx);
    clicks(word, 2, cx);
    clicks(word, 1, cx);
    assert!(view.read_with(cx, |view, _| view.selection().is_empty()));
}

#[gpui::test]
fn dragging_past_the_top_scrolls_back_and_keeps_selecting(cx: &mut TestAppContext) {
    let (view, cx) = open_at(cx, "ses_signup", 700.);
    let list = view.read_with(cx, |view, _| view.list().list_state().clone());
    let bounds = list.viewport_bounds();
    let start = at(&view, "msg_s5#5", &[5, 0], "Moved", false, cx);
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
    let row = cx.debug_bounds("tool-msg_s5:3").expect("the failed check's row");
    clicks(row.center(), 2, cx);
    assert!(view.read_with(cx, |view, _| view.selection().is_empty()), "nothing selected");
    let typing = cx.update(|window, cx| gpui::Focusable::focus_handle(composer.read(cx), cx).is_focused(window));
    assert!(typing, "the composer still has the keyboard");
    // A drag that starts there still selects.
    let to = at(&view, "msg_s5#5", &[5, 0], "Moved the reset", true, cx);
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

fn right_click_at(at: Point<Pixels>, cx: &mut VisualTestContext) {
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
}

fn choose(item: &str, cx: &mut VisualTestContext) {
    let selector: &'static str = Box::leak(format!("menu-item-{item}").into_boxed_str());
    let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{item} isn't in the menu")).center();
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn what_you_select_in_a_reply_is_quoted_in_the_composer_and_sent_ahead_of_your_answer(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    // Part of the reply selected, then its menu’s Add to Chat: the composer has it, the keyboard there.
    let from = at(&view, "msg_s2#3", &[3, 0], "When the server", false, cx);
    let to = at(&view, "msg_s2#3", &[3, 0], "(the email is taken)", true, cx);
    drag(from, to, cx);
    right_click_at(to, cx);
    choose("Add to Chat", cx);
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    assert_eq!(composer.read_with(cx, |composer, cx| composer.quotes(cx).iter().map(|quote| quote.text.clone()).collect::<Vec<_>>()), ["When the server answers 409 (the email is taken)"]);
    assert!(cx.debug_bounds("quote-0").is_some(), "shown above the field");

    // Answered and sent: the quote ahead of the answer, as Markdown, as every app writes it; the
    // message shows it as a quote above your words.
    cx.simulate_input("Should it keep the email?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = view.read_with(cx, |view, _| view.entries().iter().rev().find_map(|entry| match entry {
        kuvo::entries::Entry::Outgoing { words, .. } => Some(words.clone()),
        _ => None,
    }));
    assert_eq!(sent.as_deref(), Some("> When the server answers 409 (the email is taken)\n\nShould it keep the email?"));
    assert!(composer.read_with(cx, |composer, cx| composer.quotes(cx).is_empty()));

    // Nothing selected: the message under the pointer is quoted whole.
    let mine = at(&view, "msg_s1", &[0], "completely", false, cx);
    clicks(mine, 1, cx);
    right_click_at(mine, cx);
    choose("Reply", cx);
    let quoted = composer.read_with(cx, |composer, cx| composer.quotes(cx).first().map(|quote| quote.text.clone()));
    assert!(quoted.is_some_and(|text| text.contains("completely wiped if the email is already taken")));
}

/// Where each of a message's quotes and answers shows in row `ix`, top to bottom, by what it is
/// (`quoted-<ix>-<n>`, `said-<ix>-<n>`, `n` its place in the message).
fn drawn_order(ix: usize, cx: &mut VisualTestContext) -> Vec<String> {
    let mut found: Vec<(Pixels, String)> = (0..8)
        .flat_map(|n| [format!("quoted-{ix}-{n}"), format!("said-{ix}-{n}")])
        .filter_map(|name| {
            let selector: &'static str = Box::leak(name.clone().into_boxed_str());
            cx.debug_bounds(selector).map(|bounds| (bounds.top(), name))
        })
        .collect();
    found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    found.into_iter().map(|(_, name)| name).collect()
}

#[gpui::test]
fn a_reply_to_two_quotes_shows_each_answer_under_its_quote_in_the_order_written(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    // Two quotes, each answered under it, and words after them, sent as the user writes them.
    for (at, (quote, answer)) in [("Keep what was typed", "Yes, keep it"), ("Show the error under the field", "ok")].into_iter().enumerate() {
        view.update_in(cx, |view, window, cx| view.quote(quote, window, cx));
        let field = composer.read_with(cx, |composer, _| composer.answer_field(at)).expect("its answer's field");
        field.update(cx, |field, cx| field.set_text(answer, cx));
    }
    // (The keyboard is in the last answer, as quoting leaves it; Return there sends.)
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    field.update(cx, |field, cx| field.set_text("Then run the tests", cx));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let written = "> Keep what was typed\n\nYes, keep it\n\n> Show the error under the field\n\nok\n\n---\n\nThen run the tests";
    let (ix, sent) = view.read_with(cx, |view, _| {
        view.entries().iter().enumerate().rev().find_map(|(ix, entry)| match entry {
            kuvo::entries::Entry::Outgoing { words, .. } => Some((ix, words.clone())),
            _ => None,
        })
    }).expect("on its way");
    assert_eq!(sent, written, "sent in the order written");
    let in_order = |ix: usize| vec![format!("quoted-{ix}-0"), format!("said-{ix}-0"), format!("quoted-{ix}-1"), format!("said-{ix}-1"), format!("said-{ix}-2")];
    assert_eq!(drawn_order(ix, cx), in_order(ix), "on its way: each answer under its quote");

    // The box's copy, as the engine keeps it: the same, and copied in that order.
    show(&view, json!([{ "type": "user", "id": "m1", "time": { "created": 1000 }, "text": written }]), cx);
    let ix = view.read_with(cx, |view, _| view.entries().iter().position(|entry| entry.key() == "m1")).expect("its row");
    assert_eq!(drawn_order(ix, cx), in_order(ix), "sent: each answer under its quote");
    // Each answer selects as drawn: a drag over one copies it, and ⌘A all of them, in order.
    drag(at(&view, "m1", &[1], "ok", false, cx), at(&view, "m1", &[1], "ok", true, cx), cx);
    assert_eq!(copied(cx), "ok");
    drag(at(&view, "m1", &[2], "run", false, cx), at(&view, "m1", &[2], "tests", true, cx), cx);
    assert_eq!(copied(cx), "run the tests");
    clicks(at(&view, "m1", &[0], "keep", false, cx), 1, cx);
    cx.simulate_keystrokes("cmd-a");
    assert_eq!(copied(cx), "Yes, keep it\n\nok\n\nThen run the tests");
}

#[gpui::test]
fn finishing_a_selection_shows_add_to_chat_near_its_end_until_it_is_cleared_or_used(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    assert!(cx.debug_bounds("add-to-chat-card").is_none(), "nothing selected, no card");
    let from = at(&view, "msg_s2#3", &[3, 0], "When the server", false, cx);
    let to = at(&view, "msg_s2#3", &[3, 0], "(the email is taken)", true, cx);
    // Mid-drag nothing shows; once the button comes up it does, over the selection's end.
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    assert!(cx.debug_bounds("add-to-chat-card").is_none(), "still dragging");
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let card = cx.debug_bounds("add-to-chat-card").expect("shown once the selection is finished");
    assert!(card.bottom() <= to.y, "above the end of the selection: {card:?} / {to:?}");
    assert!(card.left() <= to.x + px(1.) && to.x <= card.right() + px(1.) || card.left() >= to.x - px(1.), "level with its end: {card:?} / {to:?}");
    // It doesn't take the keyboard: ⌘C still copies the selection.
    assert_eq!(copied(cx), "When the server answers 409 (the email is taken)");
    assert!(cx.debug_bounds("add-to-chat-card").is_some());

    // A click elsewhere in the text clears it, and the card with it.
    let elsewhere = at(&view, "msg_s1", &[0], "completely", false, cx);
    clicks(elsewhere, 1, cx);
    cx.run_until_parked();
    assert!(cx.debug_bounds("add-to-chat-card").is_none(), "a click elsewhere takes it away");

    // A double click finishes a selection too, and Escape lets go of it.
    clicks(at(&view, "msg_s2#3", &[3, 0], "server", false, cx), 2, cx);
    cx.run_until_parked();
    assert!(cx.debug_bounds("add-to-chat-card").is_some(), "a double click selects a word and finishes it");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("add-to-chat-card").is_none() && view.read_with(cx, |view, _| view.selected_text().is_empty()), "Escape clears it");

    // Used: it quotes the selection as the menu's Add to Chat does, and goes.
    drag(from, to, cx);
    cx.run_until_parked();
    let at_card = cx.debug_bounds("add-to-chat-card").unwrap().center();
    cx.simulate_mouse_down(at_card, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(at_card, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    assert_eq!(composer.read_with(cx, |composer, cx| composer.quotes(cx).iter().map(|quote| quote.text.clone()).collect::<Vec<_>>()), ["When the server answers 409 (the email is taken)"]);
    assert!(cx.debug_bounds("add-to-chat-card").is_none() && view.read_with(cx, |view, _| view.selected_text().is_empty()));
}


fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<gpui::Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

#[gpui::test]
fn a_message_shows_its_time_copy_and_quote_under_the_pointer_without_moving_anything(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "ses_signup");
    let key = "msg_s2#3";
    assert!(bounds(cx, &format!("message-actions-{key}")).is_none(), "hidden until the pointer is over it");
    let row = bounds(cx, &format!("entry-{key}")).expect("the reply is drawn");
    let below = cx.debug_bounds("entry-msg_s3").map(|next| next.top());
    cx.simulate_mouse_move(row.center(), None, Modifiers::default());
    cx.run_until_parked();
    let actions = bounds(cx, &format!("message-actions-{key}")).expect("shown on hover");
    assert_eq!(bounds(cx, &format!("entry-{key}")), Some(row), "nothing moved");
    assert_eq!(cx.debug_bounds("entry-msg_s3").map(|next| next.top()), below, "nor the next row");
    let time = bounds(cx, &format!("message-time-{key}")).expect("its time");
    assert!(actions.top() >= row.bottom() - px(24.) && time.left() < actions.left() + px(8.), "under the agent's words, at their start: {actions:?} / {time:?}");
    assert!(time.right() <= bounds(cx, &format!("message-copy-{key}")).unwrap_or(time).left() + px(1.));

    // Copy puts its Markdown on the clipboard, and shows a check.
    let copy = bounds(cx, &format!("message-copy-{key}")).expect("Copy");
    cx.simulate_click(copy.center(), Modifiers::default());
    cx.run_until_parked();
    let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
    assert!(text.contains("When the server answers **409** (the email is taken)"), "Markdown, as written: {text}");
    assert!(view.read_with(cx, |view, _| view.copied_row() == Some(key.to_owned())), "the check shows");

    // Quote does what the menu's Reply does.
    let quote = bounds(cx, &format!("message-quote-{key}")).expect("Quote");
    cx.simulate_click(quote.center(), Modifiers::default());
    cx.run_until_parked();
    let composer = view.read_with(cx, |view, _| view.composer().clone());
    let quoted = composer.read_with(cx, |composer, cx| composer.quotes(cx).first().map(|quote| quote.text.clone()));
    assert!(quoted.is_some_and(|text| text.contains("When the server answers 409")));
}
