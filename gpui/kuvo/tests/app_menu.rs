//! The menu bar, as someone uses it: macOS fills the menu named "Window" itself (the window's Fill,
//! Center and Move & Resize items, with fn-Control-arrow), each item shows its key, and choosing
//! one reaches what it names. A choice goes as the Mac sends it: to the window in front, from what
//! has the keyboard there (`App::dispatch_action`), and an item is enabled where that's answered.
//! Against the stand-in box (nothing real is reached).

use gpui::{
    Action, Entity, KeyContext, Keystroke, MenuItem, OperatingSystem, OsAction, OwnedMenu, OwnedMenuItem, SystemMenuType,
    TestAppContext, VisualTestContext, px, size,
};
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;

/// The app's keys and its menu bar, set as the app sets them.
fn start(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
        kuvo::app_menu::init(cx);
    });
}

/// The app's window on the stand-in box, its landing view's field holding the keyboard.
fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    start(cx);
    let client = BoxClient::new(StandIn::at(Replies::Never, NOW));
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    // (The window in front, as on the Mac: its menu bar is this window's.)
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1200.), px(800.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        window.focus(&gpui::Focusable::focus_handle(workspace.landing().read(cx), cx), cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn menu<'a>(menus: &'a [OwnedMenu], name: &str) -> &'a OwnedMenu {
    menus.iter().find(|menu| menu.name.as_ref() == name).unwrap_or_else(|| panic!("no {name} menu"))
}

/// A menu's items as they read, "—" for a line between them.
fn names(menus: &[OwnedMenu], name: &str) -> Vec<String> {
    menu(menus, name)
        .items
        .iter()
        .map(|item| match item {
            OwnedMenuItem::Separator => "—".to_owned(),
            OwnedMenuItem::Submenu(submenu) => submenu.name.to_string(),
            OwnedMenuItem::SystemMenu(system) => system.name.to_string(),
            OwnedMenuItem::Action { name, .. } => name.clone(),
        })
        .collect()
}

/// What choosing `menu` › `name` does.
fn action(menu: &str, name: &str) -> Box<dyn Action> {
    let menus = kuvo::app_menu::menus();
    let found = menus.iter().find(|each| each.name.as_ref() == menu).unwrap_or_else(|| panic!("no {menu} menu"));
    found
        .items
        .iter()
        .find_map(|item| match item {
            MenuItem::Action { name: each, action, .. } if each.as_ref() == name => Some(action.boxed_clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {menu} › {name}"))
}

/// Whether `menu` › `name` is enabled now, as the Mac asks before it shows the menu.
fn enabled(cx: &mut VisualTestContext, menu: &str, name: &str) -> bool {
    let action = action(menu, name);
    cx.update(|window, cx| window.is_action_available(action.as_ref(), cx) || cx.is_action_available(action.as_ref()))
}

/// Chooses `menu` › `name`, once it's enabled.
fn choose(cx: &mut VisualTestContext, menu: &str, name: &str) {
    assert!(enabled(cx, menu, name), "{menu} › {name} is enabled");
    let action = action(menu, name);
    cx.update(|window, cx| window.dispatch_action(action, cx));
    cx.run_until_parked();
}

/// The key the Mac shows beside an item, by GPUI's own rule (`gpui_macos` `create_menu_item`,
/// mirrored here as GPUI has no way to ask it): the first bound to its action that applies in a
/// Workspace, Pane and Editor context, else the first bound at all.
fn shown_key(cx: &mut TestAppContext, menu: &str, name: &str) -> Option<String> {
    let action = action(menu, name);
    cx.update(|cx| {
        let mut window = KeyContext::new_with_defaults(OperatingSystem::Mac);
        window.add("Workspace");
        let mut pane = KeyContext::new_with_defaults(OperatingSystem::Mac);
        pane.add("Pane");
        let mut editor = KeyContext::new_with_defaults(OperatingSystem::Mac);
        editor.add("Editor");
        pane.extend(&editor);
        window.extend(&pane);
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let applies = keymap.bindings_for_action(action.as_ref()).find(|binding| binding.predicate().is_none_or(|predicate| predicate.eval(&[window.clone()])));
        let shown = applies
            .or_else(|| keymap.bindings_for_action(action.as_ref()).next())
            .map(|binding| binding.keystrokes().iter().map(|keystroke| keystroke.inner().unparse(OperatingSystem::Mac)).collect::<Vec<_>>().join(" "));
        shown
    })
}

fn key(source: &'static str) -> Option<String> {
    Some(Keystroke::parse(source).unwrap().unparse(OperatingSystem::Mac))
}

#[gpui::test]
fn the_menu_bar_has_kuvo_file_edit_view_window_and_help_and_names_its_window_menu_window(cx: &mut TestAppContext) {
    start(cx);
    let menus = cx.update(|cx| cx.get_menus()).expect("the menu bar is set");
    let titles: Vec<&str> = menus.iter().map(|menu| menu.name.as_ref()).collect();
    // "Window" by that name: macOS adds Fill, Center, Move & Resize (fn-Control-arrow) to it.
    assert_eq!(titles, ["Kuvo", "File", "Edit", "View", "Window", "Help"]);
    assert_eq!(names(&menus, "Window"), ["Minimize", "Zoom", "—", "Bring All to Front"]);
    assert_eq!(
        names(&menus, "Kuvo"),
        ["About Kuvo", "—", "Settings…", "Accounts…", "—", "Services", "—", "Hide Kuvo", "Hide Others", "Show All", "—", "Quit Kuvo"]
    );
    // Services is the system's own menu, filled by macOS.
    assert!(menu(&menus, "Kuvo").items.iter().any(|item| matches!(item, OwnedMenuItem::SystemMenu(system) if system.name.as_ref() == "Services" && system.menu_type == SystemMenuType::Services)));
    assert_eq!(names(&menus, "File"), ["New Session", "Open Folder…"]);
    assert_eq!(names(&menus, "Edit"), ["Undo", "Redo", "—", "Cut", "Copy", "Paste", "Select All"]);
    // Edit's items are the system's own, so a save panel or a field the system draws gets them too.
    let os_actions: Vec<(String, bool)> = menu(&menus, "Edit")
        .items
        .iter()
        .filter_map(|item| match item {
            OwnedMenuItem::Action { name, os_action, .. } => Some((
                name.clone(),
                *os_action
                    == Some(match name.as_str() {
                        "Undo" => OsAction::Undo,
                        "Redo" => OsAction::Redo,
                        "Cut" => OsAction::Cut,
                        "Copy" => OsAction::Copy,
                        "Paste" => OsAction::Paste,
                        _ => OsAction::SelectAll,
                    }),
            )),
            _ => None,
        })
        .collect();
    assert!(os_actions.iter().all(|(_, system)| *system), "each Edit item is the system's: {os_actions:?}");
    assert_eq!(
        names(&menus, "View"),
        // (View › Library comes once the window in front shows a computer whose Kuvo has it:
        // tests/library.rs.)
        ["Show Sidebar", "Show Panel", "Show Changes", "Show Files", "—", "Bots", "Sessions", "Work Queue", "Marketplace", "Computers", "—", "Back", "Forward", "—", "Switch Light and Dark"]
    );
    assert_eq!(names(&menus, "Help"), ["Kuvo Help"]);
}

/// Where the app updates itself, Kuvo › Check for Updates… is after About, as 0.7's, and looks now.
#[gpui::test]
fn check_for_updates_follows_about_where_the_app_updates_itself_and_looks_now(cx: &mut TestAppContext) {
    let updater = std::rc::Rc::new(kuvo::updates::stand_in::StandIn::new("1.0.0", Some(NOW)));
    let installed = updater.clone();
    cx.update(|cx| {
        kuvo::updates::Updates::install(installed, None, cx);
    });
    start(cx);
    let menus = cx.update(|cx| cx.get_menus()).expect("the menu bar is set");
    assert_eq!(names(&menus, "Kuvo")[..4], ["About Kuvo", "Check for Updates…", "—", "Settings…"]);
    cx.update(|cx| cx.dispatch_action(&kuvo::CheckForUpdates));
    cx.run_until_parked();
    assert_eq!(*updater.asked.borrow(), ["check"]);
}

#[gpui::test]
fn each_item_shows_the_mac_apps_key(cx: &mut TestAppContext) {
    start(cx);
    for (menu, name, expected) in [
        ("Kuvo", "Settings…", "cmd-,"),
        ("Kuvo", "Accounts…", "cmd-shift-k"),
        ("Kuvo", "Hide Kuvo", "cmd-h"),
        ("Kuvo", "Hide Others", "cmd-alt-h"),
        ("Kuvo", "Quit Kuvo", "cmd-q"),
        ("File", "New Session", "cmd-n"),
        ("File", "Open Folder…", "cmd-o"),
        ("Edit", "Undo", "cmd-z"),
        ("Edit", "Redo", "cmd-shift-z"),
        ("Edit", "Cut", "cmd-x"),
        ("Edit", "Copy", "cmd-c"),
        ("Edit", "Paste", "cmd-v"),
        ("Edit", "Select All", "cmd-a"),
        ("View", "Show Sidebar", "cmd-b"),
        ("View", "Show Panel", "cmd-alt-0"),
        ("View", "Show Changes", "cmd-alt-i"),
        ("View", "Show Files", "cmd-alt-f"),
        ("View", "Work Queue", "cmd-alt-u"),
        ("View", "Back", "cmd-["),
        ("Window", "Minimize", "cmd-m"),
    ] {
        assert_eq!(shown_key(cx, menu, name), key(expected), "{menu} › {name}");
    }
    assert_eq!(shown_key(cx, "Window", "Zoom"), None, "Zoom has no key, as on the Mac");
}

#[gpui::test]
fn view_show_sidebar_folds_the_sidebar_away_and_brings_it_back(cx: &mut TestAppContext) {
    let (_, cx) = open(cx);
    assert!(cx.debug_bounds("area-sidebar").is_some());
    choose(cx, "View", "Show Sidebar");
    assert!(cx.debug_bounds("area-sidebar").is_none(), "folded away");
    choose(cx, "View", "Show Sidebar");
    assert!(cx.debug_bounds("area-sidebar").is_some(), "brought back");
}

#[gpui::test]
fn kuvo_settings_and_view_work_queue_open_their_areas(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx);
    choose(cx, "Kuvo", "Settings…");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Settings);
    choose(cx, "View", "Work Queue");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Queue);
    choose(cx, "View", "Sessions");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.area()), Area::Sessions);
}

#[gpui::test]
fn the_edit_menu_selects_cuts_and_pastes_in_a_text_field(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx);
    let text = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| workspace.read_with(cx, |workspace, cx| workspace.landing().read(cx).field().read(cx).text().to_owned());
    cx.simulate_input("fix the sign-up form");
    choose(cx, "Edit", "Select All");
    choose(cx, "Edit", "Cut");
    assert_eq!(text(&workspace, cx), "");
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("fix the sign-up form"));
    choose(cx, "Edit", "Paste");
    choose(cx, "Edit", "Paste");
    assert_eq!(text(&workspace, cx), "fix the sign-up formfix the sign-up form");
    choose(cx, "Edit", "Undo");
    assert_eq!(text(&workspace, cx), "fix the sign-up form");
    choose(cx, "Edit", "Redo");
    assert_eq!(text(&workspace, cx), "fix the sign-up formfix the sign-up form");
}

#[gpui::test]
fn edit_items_are_off_where_nothing_takes_them(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx);
    // The keyboard on the window itself, in no field or conversation.
    workspace.update_in(cx, |workspace, window, cx| window.focus(&gpui::Focusable::focus_handle(workspace, cx), cx));
    cx.run_until_parked();
    for name in ["Undo", "Cut", "Paste"] {
        assert!(!enabled(cx, "Edit", name), "Edit › {name} is off");
    }
    // The window's own items stay on.
    assert!(enabled(cx, "View", "Show Sidebar"));
    assert!(enabled(cx, "Window", "Minimize"));
}

#[gpui::test]
fn help_opens_kuvos_documentation(cx: &mut TestAppContext) {
    let (_, cx) = open(cx);
    choose(cx, "Help", "Kuvo Help");
    assert_eq!(cx.opened_url().as_deref(), Some("https://www.kuvo.run/code/docs/"));
}
