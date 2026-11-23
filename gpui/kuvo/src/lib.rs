//! Kuvo, the desktop client of a Kuvo box, drawn with GPUI: a window with a rail of areas (Sessions,
//! Bots, Work Queue, Computers, Settings), each area's sidebar beside it, the box's sessions, a
//! conversation followed live, and the composer. It decides nothing the box decides: it
//! reads the box's facts through `kuvo_box`, changes them through its routes, and draws.
//!
//! `main.rs` opens it; `bin/shots.rs` renders its screens offscreen; `stand_in` (the `stand-in`
//! feature) is a made-up box it runs against without touching a real one.

pub mod app_menu;
pub mod area;
pub mod areas;
pub mod ask_card;
pub mod blocked;
pub mod bot_face;
pub mod bots;
pub mod browser_view;
pub mod composer;
pub mod confirm_you;
pub mod computers;
pub mod conversation;
pub mod entries;
pub mod first_run;
pub mod fleet;
pub mod groups;
pub mod home;
pub mod image_viewer;
pub mod insights;
pub mod keeper;
pub mod model_picker;
pub mod moves;
mod landing;
pub mod launch;
pub mod library;
pub mod marketplace;
pub mod notifications;
pub mod panel;
pub mod pasted;
pub mod open_shown;
pub mod pictures;
pub mod permissions;
pub mod queue;
pub mod quit_note;
pub mod reduce_motion;
pub mod rail;
pub mod session_menu;
pub mod settings;
pub mod shown_files;
pub mod sidebar;
#[cfg(feature = "stand-in")]
pub mod stand_in;
pub mod terminal;
pub mod time;
pub mod title_bar;
pub mod tool_step;
pub mod updates;
pub mod work_bars;
pub mod workspace;
pub mod yours;

use gpui::{App, KeyBinding, actions};

pub use home::Home;
pub use workspace::Workspace;

actions!(kuvo, [NewSession, OpenFolder, ToggleAppearance, Quit, TogglePanel, ToggleSidebar, ShowChanges, ShowFiles, CheckForUpdates]);

/// Binds the app's keys, after the blocks' (so "/"'s list takes its keys ahead of the field's).
mod wakes;

pub fn bind_keys(cx: &mut App) {
    wakes::bind_keys(cx);
    kuvo_ui::bind_keys(cx);
    composer::bind_keys(cx);
    queue::bind_keys(cx);
    model_picker::bind_keys(cx);
    sidebar::bind_keys(cx);
    session_menu::bind_keys(cx);
    area::bind_keys(cx);
    panel::changes::bind_keys(cx);
    terminal::bind_keys(cx);
    app_menu::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewSession, Some("Workspace")),
        KeyBinding::new("cmd-o", OpenFolder, Some("Workspace")),
        // The open area's sidebar folded away or brought back; keep ⌘0 as an alias.
        KeyBinding::new("cmd-b", ToggleSidebar, Some("Workspace")),
        KeyBinding::new("cmd-0", ToggleSidebar, Some("Workspace")),
        // As the Mac app's: Show Panel, Show Changes, Show Files.
        KeyBinding::new("cmd-alt-0", TogglePanel, Some("Workspace")),
        KeyBinding::new("cmd-alt-i", ShowChanges, Some("Workspace")),
        KeyBinding::new("cmd-alt-f", ShowFiles, Some("Workspace")),
        KeyBinding::new("cmd-shift-l", ToggleAppearance, Some("Workspace")),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}
