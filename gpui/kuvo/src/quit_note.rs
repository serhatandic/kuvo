//! The first Quit's note (#761): on a Mac whose Kuvo Kuvo Helper runs, quitting the app leaves Kuvo
//! at work (its sessions, bots and wake-ups), and nothing on screen would say so but the menu bar
//! item. So the first time you quit (⌘Q, or closing the window), a card says it, and shows where
//! Kuvo is: Quit goes on, Cancel stays. It shows once on this device, either way, remembered in the
//! app's own folder (`~/.kuvo-code/desktop/settings.json`, `quitNoteSeen`).
//!
//! Whether it applies is the box's to say (`kuvo/box/at-work`'s `applies`: a Mac's own box, which
//! Kuvo Helper draws in the menu bar); the app only remembers that you've seen it.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, Global, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Styled, Window, div, px};
use kuvo_ui::{ActiveTheme, Button, StyledType, TypeSize, theme::{radius, space}};

/// Quit (⌘Q, its menu): through the window's [`crate::Home`] where it's open (which notes the
/// first Quit first, where it's owed), else at once. Run after the key's dispatch (`cx.defer`):
/// the window it came through is busy until then.
pub fn quit(cx: &mut App) {
    for window in cx.windows() {
        if let Some(handle) = window.downcast::<crate::Home>() {
            if handle.update(cx, |home, window, cx| home.quit(window, cx)).is_ok() {
                return;
            }
        }
        if let Some(handle) = window.downcast::<crate::launch::Launch>() {
            if let Some(home) = handle.read(cx).ok().and_then(|launch| launch.home().cloned()) {
                if handle.update(cx, |_, window, cx| home.update(cx, |home, cx| home.quit(window, cx))).is_ok() {
                    return;
                }
            }
        }
    }
    cx.quit();
}

/// Where this app remembers the note was seen: its own settings file. None for one that keeps
/// nothing (tests and shots, unless they say).
#[derive(Clone, Debug, Default)]
pub struct QuitNoteFile(pub Option<PathBuf>);

impl Global for QuitNoteFile {}

impl QuitNoteFile {
    /// The app's own settings file in `folder` (its `desktop` folder).
    pub fn in_folder(folder: &Path) -> QuitNoteFile {
        QuitNoteFile(Some(folder.join("settings.json")))
    }
}

/// Whether the note was seen on this device.
pub fn seen(cx: &App) -> bool {
    let Some(file) = cx.try_global::<QuitNoteFile>().and_then(|file| file.0.clone()) else { return false };
    read(&file).get("quitNoteSeen").and_then(serde_json::Value::as_bool).unwrap_or(false)
}

/// Notes it was seen, keeping whatever else the file holds.
pub fn note_seen(cx: &App) {
    let Some(file) = cx.try_global::<QuitNoteFile>().and_then(|file| file.0.clone()) else { return };
    let mut kept = read(&file);
    kept.insert("quitNoteSeen".into(), serde_json::Value::Bool(true));
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(kept)).unwrap_or_default();
    // (Written whole, then moved in place: a quit halfway never leaves half a file.)
    let next = file.with_extension("json.new");
    if std::fs::write(&next, text + "\n").is_ok() {
        let _ = std::fs::rename(&next, &file);
    }
}

fn read(file: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read(file).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).and_then(|value| value.as_object().cloned()).unwrap_or_default()
}

/// The card, over the window: where Kuvo is (a strip of the menu bar with its item), what goes on,
/// Cancel (Escape) and Quit (Return).
pub fn card(focus: &gpui::FocusHandle, cancel: impl Fn(&mut Window, &mut App) + Clone + 'static, quit: impl Fn(&mut Window, &mut App) + Clone + 'static, cx: &App) -> AnyElement {
    let c = cx.theme().colors.clone();
    let dim = crate::settings::page::dim(cx);
    let (on_cancel, on_quit) = (cancel.clone(), quit.clone());
    let keys = move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| match event.keystroke.key.as_str() {
        "escape" => cancel(window, cx),
        "enter" => quit(window, cx),
        _ => {}
    };
    // A strip of the menu bar, as it looks with Kuvo's item in it.
    let strip = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(space::S3)
        .h(px(26.))
        .px(space::S3)
        .rounded(radius::SMALL)
        .bg(c.sunken)
        .border_1()
        .border_color(c.border)
        .child(
            div()
                .debug_selector(|| "quit-note-item".into())
                .flex()
                .items_center()
                .gap(px(3.))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(c.selected)
                .child(gpui::img(kuvo_ui::icon::kuvo_mark(cx.theme().appearance.is_dark())).size(px(16.)).flex_none()),
        )
        .child(div().w(px(46.)).h(px(6.)).rounded(radius::PILL).bg(c.border_strong))
        .child(div().w(px(30.)).h(px(6.)).rounded(radius::PILL).bg(c.border_strong));
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .p(space::S6)
        .bg(dim)
        .occlude()
        .child(
            div()
                .id("quit-note")
                .debug_selector(|| "quit-note".into())
                .track_focus(focus)
                .on_key_down(keys)
                .w(px(380.))
                .max_w_full()
                .flex()
                .flex_col()
                .gap(space::S3)
                .p(space::S6)
                .rounded(radius::CARD)
                .bg(c.elevated)
                .border_1()
                .border_color(c.border)
                .shadow(cx.theme().elevated_shadow())
                .child(strip)
                .child(div().pt(space::S1).text_ui(TypeSize::Body).medium().text_color(c.text).child("Kuvo keeps working"))
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child("Your sessions, bots and wake-ups go on after you quit. Kuvo is in the menu bar: open it or stop it there."))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .pt(space::S2)
                        .child(Button::new("quit-note-cancel", "Cancel").on_click(move |_, window, cx| on_cancel(window, cx)))
                        .child(Button::new("quit-note-quit", "Quit").primary().on_click(move |_, window, cx| on_quit(window, cx))),
                ),
        )
        .into_any_element()
}
