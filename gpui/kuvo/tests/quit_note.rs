//! The first Quit's note (#761), as someone meets it, against the stand-in box (nothing real is
//! reached): on a Mac whose Kuvo stays at work in the menu bar, the first ⌘Q (or closing the
//! window) shows the note instead of quitting; Cancel or Escape stays, and it's never shown again on
//! this device; where the box says it isn't in the menu bar, nothing is noted. Keys go through the
//! window as AppKit hands them on.

use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use kuvo::Home;
use kuvo::quit_note::QuitNoteFile;
use kuvo::stand_in::{Replies, StandIn};
use kuvo_ui::{Appearance, Theme};

fn open<'a>(cx: &'a mut TestAppContext, stand_in: StandIn, folder: &std::path::Path) -> (Entity<Home>, &'a mut VisualTestContext) {
    let file = QuitNoteFile::in_folder(folder);
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
        cx.set_global(file);
        // As the app routes ⌘Q (main.rs).
        cx.on_action(|_: &kuvo::Quit, cx| cx.defer(kuvo::quit_note::quit));
    });
    let boxes = kuvo::home::stand_in_boxes(stand_in);
    let first = boxes.nearest().expect("the nearest box");
    let window = cx.add_window(move |window, cx| Home::new(boxes, first, false, window, cx));
    let home = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    home.update_in(cx, |home, _, cx| home.read_at_once(cx));
    cx.run_until_parked();
    (home, cx)
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn folder() -> tempfile::TempDir {
    tempfile::tempdir().expect("a folder of the test's own")
}

#[gpui::test]
fn the_first_quit_says_kuvo_keeps_working_and_cancel_stays(cx: &mut TestAppContext) {
    let folder = folder();
    let (home, cx) = open(cx, StandIn::new(Replies::Never), folder.path());
    assert!(!shown(cx, "quit-note"));
    // Focus where you are, as the app opens (its landing's field).
    home.update_in(cx, |home, window, cx| {
        let landing = home.workspace().read(cx).landing().focus_handle(cx);
        window.focus(&landing, cx);
    });
    let before = cx.update(|window, cx| window.focused(cx));
    assert!(before.is_some());
    cx.simulate_keystrokes("cmd-q");
    assert!(shown(cx, "quit-note"), "the first ⌘Q shows the note instead of quitting");
    assert!(shown(cx, "quit-note-item"), "with where Kuvo is: its item in the menu bar");
    // A second ⌘Q while it's up doesn't quit past it.
    cx.simulate_keystrokes("cmd-q");
    assert!(home.read_with(cx, |home, _| home.quit_note_shown()));
    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "quit-note"), "Escape stays");
    assert_eq!(cx.update(|window, cx| window.focused(cx)), before, "back where you were");
    let kept = std::fs::read_to_string(folder.path().join("settings.json")).expect("remembered in the app's own folder");
    assert!(kept.contains("\"quitNoteSeen\": true"), "{kept}");
    // Seen: the next ⌘Q quits without it.
    cx.simulate_keystrokes("cmd-q");
    assert!(!shown(cx, "quit-note"), "shown once on this device");
}

#[gpui::test]
fn seen_on_this_device_before_it_isnt_shown_again(cx: &mut TestAppContext) {
    let folder = folder();
    std::fs::write(folder.path().join("settings.json"), "{\"quitNoteSeen\": true, \"other\": 1}\n").unwrap();
    let (_, cx) = open(cx, StandIn::new(Replies::Never), folder.path());
    cx.simulate_keystrokes("cmd-q");
    assert!(!shown(cx, "quit-note"));
}

#[gpui::test]
fn closing_the_window_the_first_time_shows_it_too(cx: &mut TestAppContext) {
    let folder = folder();
    let (_, cx) = open(cx, StandIn::new(Replies::Never), folder.path());
    assert!(!cx.simulate_close(), "the window stays, with the note");
    cx.run_until_parked();
    assert!(shown(cx, "quit-note"));
    cx.simulate_keystrokes("escape");
    assert!(cx.simulate_close(), "seen: it closes");
}

#[gpui::test]
fn where_kuvo_isnt_in_the_menu_bar_quit_quits(cx: &mut TestAppContext) {
    let folder = folder();
    let stand_in = StandIn::new(Replies::Never);
    stand_in.not_in_menu_bar();
    let (_, cx) = open(cx, stand_in, folder.path());
    cx.simulate_keystrokes("cmd-q");
    assert!(!shown(cx, "quit-note"));
    assert!(!folder.path().join("settings.json").exists(), "nothing noted");
}
