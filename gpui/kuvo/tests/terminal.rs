//! A terminal in the side panel as someone uses it, against the stand-in box (nothing real is
//! reached, nothing waits on a clock): its shell made in the session's folder, what the shell prints
//! drawn in the theme's colours, keys and pastes going to the shell as a shell reads them, the size
//! it's laid out at told to the box, scrolling back and copying, and its ends said plainly: a shell
//! that exited, a connection that went, a computer that can't be reached for one yet. Events go
//! through the window, as the platform sends them.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    ClipboardItem, Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext,
    TouchPhase, VisualTestContext, point, px, size,
};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::terminal::{TerminalEvent, TerminalState, TerminalView};
use kuvo_box::BoxClient;
use kuvo_ui::{ActiveTheme as _, Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;
const FOLDER: &str = "/Users/sam/Projects/website";

/// A terminal on its own in a window, the keys going to it, as the panel shows one.
fn open<'a>(cx: &'a mut TestAppContext, stand_in: &StandIn) -> (Entity<TerminalView>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |_, cx| TerminalView::new(client, false, Some(FOLDER.into()), cx));
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(640.), px(400.)));
    view.update_in(cx, |view, window, cx| window.focus(&view.focus_handle(cx), cx));
    cx.run_until_parked();
    (view, cx)
}

fn screen(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |view, _| view.screen_text())
}

fn state(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> TerminalState {
    view.read_with(cx, |view, _| view.state())
}

/// What it tells the panel, as it tells it.
fn events(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> Rc<RefCell<Vec<TerminalEvent>>> {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let kept = heard.clone();
    cx.update(|_, cx| cx.subscribe(view, move |_, event: &TerminalEvent, _| kept.borrow_mut().push(event.clone())).detach());
    heard
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn a_new_terminal_starts_its_shell_in_the_folder_and_draws_what_it_prints_in_the_themes_colours(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(state(&view, cx), TerminalState::Running);
    assert_eq!(view.read_with(cx, |view, _| view.title()), "zsh", "named for its shell, as the engine says it");
    assert_eq!(stand_in.seen_at("POST", "api/pty")[0].body, Some(json!({ "cwd": FOLDER })), "made in the session's folder");
    assert!(cx.debug_bounds("terminal-grid").is_some());

    stand_in.pty_print("pty_1", b"\x1b[31mfailed\x1b[0m 2 tests\r\n\x1b[1;32m~/website\x1b[0m $ ");
    cx.run_until_parked();
    let shown = screen(&view, cx);
    assert_eq!((shown[0].as_str(), shown[1].as_str()), ("failed 2 tests", "~/website $"));
    let colors = cx.update(|_, cx| cx.theme().colors.clone());
    let at = |row, column, cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.color_at(row, column, cx));
    assert_eq!(at(0, 0, cx), Some(colors.diff_removed), "red is the theme's own red");
    assert_eq!(at(0, 7, cx), Some(colors.text), "plain text is the theme's text");
    assert_eq!(at(1, 0, cx), Some(colors.diff_added), "green, the theme's");
}

#[gpui::test]
fn what_it_printed_before_it_was_opened_is_drawn_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    stand_in.pty_print("pty_1", b"one\r\ntwo\r\n");
    cx.run_until_parked();
    assert_eq!(&screen(&view, cx)[..2], ["one", "two"]);
}

#[gpui::test]
fn typing_sends_the_keys_as_a_shell_reads_them(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_view, cx) = open(cx, &stand_in);
    cx.simulate_input("ls");
    cx.simulate_keystrokes("enter ctrl-c up alt-b backspace tab shift-tab escape f5");
    assert_eq!(stand_in.pty_typed("pty_1"), ["l", "s", "\r", "\x03", "\x1b[A", "\x1bb", "\x7f", "\t", "\x1b[Z", "\x1b", "\x1b[15~"]);

    // A full-screen program that asks for the cursor keys' application mode gets them so.
    stand_in.pty_print("pty_1", b"\x1b[?1h");
    cx.run_until_parked();
    cx.simulate_keystrokes("up");
    assert_eq!(stand_in.pty_typed("pty_1").last().map(String::as_str), Some("\x1bOA"));
}

#[gpui::test]
fn the_shells_questions_are_answered_so_its_prompt_doesnt_wait(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_view, cx) = open(cx, &stand_in);
    // Where's the cursor? (fish and many prompts ask as they start.)
    stand_in.pty_print("pty_1", b"$ \x1b[6n");
    cx.run_until_parked();
    assert_eq!(stand_in.pty_typed("pty_1"), ["\x1b[1;3R"]);
}

#[gpui::test]
fn pasting_goes_to_the_shell_marked_as_a_paste_where_it_asked(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_view, cx) = open(cx, &stand_in);
    cx.write_to_clipboard(ClipboardItem::new_string("echo one\necho two".into()));
    cx.simulate_keystrokes("cmd-v");
    assert_eq!(stand_in.pty_typed("pty_1"), ["echo one\recho two"]);
    // A shell that asks for bracketed paste gets the paste marked, so its lines don't run as they land.
    stand_in.pty_print("pty_1", b"\x1b[?2004h$ ");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-v");
    assert_eq!(stand_in.pty_typed("pty_1").last().map(String::as_str), Some("\x1b[200~echo one\recho two\x1b[201~"));
}

#[gpui::test]
fn the_box_is_told_the_size_its_drawn_at_and_again_when_it_changes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    let told = stand_in.pty_sizes("pty_1");
    let &(rows, cols) = told.last().expect("told as it's laid out");
    assert_eq!(screen(&view, cx).len(), rows as usize, "the rows told are the rows drawn");
    assert!(rows > 10 && cols > 40, "{told:?}");
    // Drawn again and again at that size: told once.
    for _ in 0..3 {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    assert_eq!(stand_in.pty_sizes("pty_1").len(), told.len(), "not each frame");

    cx.simulate_resize(size(px(400.), px(240.)));
    cx.run_until_parked();
    let resized = stand_in.pty_sizes("pty_1");
    assert_eq!(resized.len(), told.len() + 1, "{resized:?}");
    let &(fewer, narrower) = resized.last().unwrap();
    assert!(fewer < rows && narrower < cols, "smaller: {resized:?}");
    assert_eq!(screen(&view, cx).len(), fewer as usize);
    assert_eq!(stand_in.seen_at("PUT", "api/pty/pty_1").last().and_then(|seen| seen.body.clone()), Some(json!({ "size": { "rows": fewer, "cols": narrower } })));
}

#[gpui::test]
fn the_wheel_scrolls_back_through_what_it_printed_and_typing_comes_back(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    let printed: String = (1..=100).map(|n| format!("line {n}\r\n")).collect();
    stand_in.pty_print("pty_1", printed.as_bytes());
    cx.run_until_parked();
    assert!(!screen(&view, cx).contains(&"line 1".to_owned()), "scrolled off the top");

    let grid = cx.debug_bounds("terminal-grid").unwrap();
    cx.simulate_event(ScrollEvent { position: grid.center(), delta: ScrollDelta::Lines(point(0., 200.)), modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved, source: ScrollSource::Pointer });
    assert_eq!(screen(&view, cx)[0], "line 1", "back at the start of what it kept");

    cx.simulate_input("x");
    assert!(!screen(&view, cx).contains(&"line 1".to_owned()), "back where it's typed");
}

#[gpui::test]
fn dragging_across_a_line_and_pressing_cmd_c_copies_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_view, cx) = open(cx, &stand_in);
    stand_in.pty_print("pty_1", b"hello world\r\n$ ");
    cx.run_until_parked();
    let grid = cx.debug_bounds("terminal-grid").unwrap();
    let y = grid.top() + px(10.);
    cx.simulate_mouse_down(point(grid.left() + px(1.), y), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(grid.right() - px(1.), y), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(point(grid.right() - px(1.), y), MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("cmd-c");
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("hello world"));
    assert!(stand_in.pty_typed("pty_1").is_empty(), "⌘C is the app's, never a key to the shell");

    // A double click takes the word.
    let first = point(grid.left() + px(1.), y);
    cx.simulate_event(MouseDownEvent { position: first, modifiers: Modifiers::default(), button: MouseButton::Left, click_count: 2, first_mouse: false });
    cx.simulate_event(MouseUpEvent { position: first, modifiers: Modifiers::default(), button: MouseButton::Left, click_count: 2 });
    cx.simulate_keystrokes("cmd-c");
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("hello"));
}

#[gpui::test]
fn a_shell_that_exits_says_so_keeps_its_output_and_offers_a_new_terminal(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    let heard = events(&view, cx);
    stand_in.pty_print("pty_1", b"bye\r\n");
    stand_in.pty_exit("pty_1", 0);
    cx.run_until_parked();
    assert_eq!(state(&view, cx), TerminalState::Ended("The shell exited.".into()));
    assert!(cx.debug_bounds("terminal-ended").is_some(), "said under its output");
    assert_eq!(screen(&view, cx)[0], "bye", "what it printed stays to read");
    assert!(heard.borrow().contains(&TerminalEvent::Changed));

    click(cx, "terminal-new");
    assert_eq!(heard.borrow().iter().filter(|event| **event == TerminalEvent::Restart).count(), 1);
    // Return in it, as in 0.7's.
    cx.simulate_keystrokes("enter");
    assert_eq!(heard.borrow().iter().filter(|event| **event == TerminalEvent::Restart).count(), 2);
    assert!(stand_in.pty_typed("pty_1").is_empty(), "nothing goes to a shell that's gone");
}

#[gpui::test]
fn a_shell_that_fails_says_its_exit_code(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    stand_in.pty_exit("pty_1", 127);
    cx.run_until_parked();
    assert_eq!(state(&view, cx), TerminalState::Ended("The shell exited with code 127.".into()));
}

#[gpui::test]
fn a_connection_that_goes_ends_its_shell_on_the_box_and_says_so(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    stand_in.pty_drop("pty_1");
    cx.run_until_parked();
    assert_eq!(state(&view, cx), TerminalState::Ended("The connection closed.".into()));
    assert!(stand_in.ptys().is_empty(), "nothing runs on there unseen");
}

#[gpui::test]
fn a_computer_that_cant_be_reached_for_a_terminal_says_so_plainly_and_can_be_tried_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.terminals_not_here(Some("Terminals on devbox aren’t available over SSH yet."));
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(state(&view, cx), TerminalState::Unavailable("Terminals on devbox aren’t available over SSH yet.".into()));
    assert!(cx.debug_bounds("terminal-grid").is_none());
    assert!(stand_in.ptys().is_empty(), "the shell it made for nothing is ended again");

    stand_in.terminals_not_here(None);
    click(cx, "terminal-try-again");
    assert_eq!(state(&view, cx), TerminalState::Running);
}

#[gpui::test]
fn one_that_couldnt_start_says_why_and_tries_again(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.terminals_refused(Some("The engine isn’t running."));
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(state(&view, cx), TerminalState::Ended("The engine isn’t running.".into()));
    assert!(cx.debug_bounds("terminal-ended").is_none(), "it never ran: not \"ended\"");
    stand_in.terminals_refused(None);
    click(cx, "terminal-start-again");
    assert_eq!(state(&view, cx), TerminalState::Running);
}

#[gpui::test]
fn hidden_it_keeps_its_shell_and_reads_what_it_prints(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    view.update(cx, |view, cx| view.set_shown(false, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("terminal-grid").is_none(), "not drawn");
    stand_in.pty_print("pty_1", b"built in 3.2s\r\n");
    cx.run_until_parked();
    view.update(cx, |view, cx| view.set_shown(true, cx));
    cx.run_until_parked();
    assert_eq!(screen(&view, cx)[0], "built in 3.2s");
    assert_eq!(state(&view, cx), TerminalState::Running);
    assert_eq!(stand_in.ptys(), ["pty_1"]);
}

#[gpui::test]
fn closing_it_ends_its_shell_on_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (view, cx) = open(cx, &stand_in);
    assert_eq!(stand_in.ptys(), ["pty_1"]);
    drop(view);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
    assert!(stand_in.ptys().is_empty());
}
