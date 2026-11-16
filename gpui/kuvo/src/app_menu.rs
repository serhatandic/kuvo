//! The menu bar, as Kuvo Code 0.7's (`swift/KuvoCode/App/KuvoCodeApp.swift`, `AppCommands`), with
//! the items whose actions this app has: Kuvo, File, Edit, View, Window and Help.
//!
//! An item is enabled where its action is answered: Edit's in a text field or a conversation's
//! text, View's in the app's window. Its key is the one bound to its action (`crate::bind_keys`).
//! The menu named "Window" is the one macOS fills itself: the window's Fill, Center and Move &
//! Resize items, with their fn-Control-arrow keys, and the list of windows.

use gpui::{App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, Window, actions};
use kuvo_ui::text_field::{Copy, Cut, Paste, Redo, SelectAll, Undo};

use crate::area::{Back, Forward, OpenHelp, ShowAccounts, ShowBots, ShowComputers, ShowQueue, ShowSessions, ShowSettings};
use crate::{NewSession, Quit, ShowChanges, ShowFiles, ToggleAppearance, TogglePanel, ToggleSidebar};

actions!(app_menu, [About, Hide, HideOthers, ShowAll, Minimize, Zoom, BringAllToFront]);

/// The menu's own keys (the rest are bound where their actions are answered).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
}

/// Answers the menu's app and window items, and sets the menu bar. After the keys are bound,
/// so each item shows its key.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &About, cx| about(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| on_active_window(cx, |window| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx| on_active_window(cx, |window| window.zoom_window()));
    cx.on_action(|_: &BringAllToFront, cx| bring_all_to_front(cx));
    // Help, from a window without the workspace too (the one setting up Kuvo).
    cx.on_action(|_: &OpenHelp, cx| cx.open_url(crate::area::HELP));
    cx.set_menus(menus());
}

/// The menu bar's menus.
pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Kuvo").items([
            MenuItem::action("About Kuvo", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", ShowSettings),
            MenuItem::action("Accounts…", ShowAccounts),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Kuvo", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Kuvo", Quit),
        ]),
        Menu::new("File").items([MenuItem::action("New Session", NewSession)]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            // Both fold away and bring back, as 0.7's Show Sessions and Show Panel did.
            MenuItem::action("Show Sidebar", ToggleSidebar),
            MenuItem::action("Show Panel", TogglePanel),
            MenuItem::action("Show Changes", ShowChanges),
            MenuItem::action("Show Files", ShowFiles),
            MenuItem::separator(),
            MenuItem::action("Sessions", ShowSessions),
            MenuItem::action("Bots", ShowBots),
            MenuItem::action("Work Queue", ShowQueue),
            MenuItem::action("Computers", ShowComputers),
            MenuItem::separator(),
            MenuItem::action("Back", Back),
            MenuItem::action("Forward", Forward),
            MenuItem::separator(),
            MenuItem::action("Switch Light and Dark", ToggleAppearance),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Bring All to Front", BringAllToFront),
        ]),
        Menu::new("Help").items([MenuItem::action("Kuvo Help", OpenHelp)]),
    ]
}

/// Acts on the window in front, once the action that asked is through with it.
fn on_active_window(cx: &mut App, act: impl FnOnce(&mut Window) + 'static) {
    cx.defer(move |cx| {
        if let Some(window) = cx.active_window() {
            window.update(cx, |_, window, _| act(window)).ok();
        }
    });
}

/// The Mac's own About panel: the app's name, icon and version, as 0.7 showed it.
fn about(cx: &mut App) {
    #[cfg(target_os = "macos")]
    if let Some(main) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(main).orderFrontStandardAboutPanel(None);
        return;
    }
    cx.activate(true);
}

/// The app's windows in front of other apps' (where the system can't, the app's own brought forward).
fn bring_all_to_front(cx: &mut App) {
    #[cfg(target_os = "macos")]
    if let Some(main) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(main).arrangeInFront(None);
        return;
    }
    cx.activate(true);
}
