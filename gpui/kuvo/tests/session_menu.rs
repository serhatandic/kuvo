//! The desktop app's session menu (Draft B), row hover buttons, inline popovers, archived filter,
//! and keyboard shortcuts against the stand-in box.

use gpui::{
    Bounds, Entity, Focusable as _, Modifiers, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext, px, size,
};
use kuvo::session_menu::{ActivePopover, PopoverAnchor, build_menu_items};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_box::session_actions::{Action, Actions};
use kuvo_ui::menu::MenuItem;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let rect = bounds(cx, selector).unwrap_or_else(|| panic!("{selector:?} isn't drawn"));
    cx.simulate_click(rect.center(), Modifiers::default());
    cx.run_until_parked();
}

fn right_click(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_mouse_move(at, None, Modifiers::default());
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn row_hover_buttons_only_appear_under_pointer_and_pin_archive_work(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, cx) = open(cx, stand_in.clone());

    // Without pointer over the row, Pin and Archive buttons are absent from the layout tree.
    assert!(bounds(cx, "pin-session-project:ses_dark").is_none());
    assert!(bounds(cx, "archive-session-project:ses_dark").is_none());

    // Hovering the row reveals both buttons with clear separation between them.
    let dark = bounds(cx, "session-project-ses_dark").expect("ses_dark row");
    cx.simulate_mouse_move(dark.center(), None, Modifiers::default());
    cx.run_until_parked();

    let pin_btn = bounds(cx, "pin-session-project:ses_dark").expect("pin hover button");
    let archive_btn = bounds(cx, "archive-session-project:ses_dark").expect("archive hover button");
    assert!(
        archive_btn.left() - pin_btn.right() >= px(8.),
        "Pin and Archive buttons must stay clearly separated: pin={pin_btn:?}, archive={archive_btn:?}"
    );

    // Pin raises ses_dark once to the top of Website (above ses_signup), with no duplicate Pinned section.
    let signup_before = bounds(cx, "session-project-ses_signup").expect("ses_signup row");
    assert!(signup_before.top() < dark.top());
    cx.simulate_click(pin_btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_dark/pin").len(), 1);
    assert!(bounds(cx, "header-Pinned").is_none(), "Draft B has no separate Pinned section");

    let dark_pinned = bounds(cx, "session-project-ses_dark").expect("pinned ses_dark row");
    let signup_after = bounds(cx, "session-project-ses_signup").expect("ses_signup row");
    assert!(dark_pinned.top() < signup_after.top(), "pinned row stays at top of its project");

    // Moving the pointer off the row removes hover buttons immediately.
    cx.simulate_mouse_move(point_outside_sidebar(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "pin-session-project:ses_dark").is_none());
    assert!(bounds(cx, "archive-session-project:ses_dark").is_none());

    // Opening ses_dark and archiving via the row's hover button takes no confirmation.
    click(cx, "session-project-ses_dark");
    let dark_row = bounds(cx, "session-project-ses_dark").expect("ses_dark row");
    cx.simulate_mouse_move(dark_row.center(), None, Modifiers::default());
    cx.run_until_parked();
    click(cx, "archive-session-project:ses_dark");
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_dark/archive").len(), 1);
    assert!(bounds(cx, "confirm-delete-popover").is_none());
    assert!(bounds(cx, "session-project-ses_dark").is_none(), "archived session leaves active sidebar");
    assert!(bounds(cx, "is-archived").is_some(), "open archived session shows Archived chip in title bar");
}

#[gpui::test]
fn archived_filter_chip_search_unarchive_and_engine_archived_work(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    client.archive_session("ses_dark", true).unwrap();
    stand_in.set_engine_archived("ses_recipes", true);
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();

    // Toggling the Archived filter shows the chip and placeholder "Search archived".
    click(cx, "toggle-archived");
    assert!(bounds(cx, "showing-archived").is_some());
    assert_eq!(
        workspace.read_with(cx, |workspace, cx| workspace.search().read(cx).placeholder_text().to_owned()),
        "Search archived"
    );
    assert!(bounds(cx, "archived-project-Website").is_some());
    assert!(bounds(cx, "session-archived-project-ses_dark").is_some());
    assert!(bounds(cx, "session-archived-chat-ses_recipes").is_some());
    assert!(bounds(cx, "session-project-ses_signup").is_none(), "active sessions hidden when Archived is on");

    // Hovering an archived row shows Unarchive and Delete… buttons instead of Pin/Archive.
    let archived_dark = bounds(cx, "session-archived-project-ses_dark").expect("archived ses_dark");
    cx.simulate_mouse_move(archived_dark.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "unarchive-session-archived-project:ses_dark").is_some());
    assert!(bounds(cx, "delete-session-archived-project:ses_dark").is_some());
    assert!(bounds(cx, "pin-session-archived-project:ses_dark").is_none());

    // Search filters to archived sessions only while Archived is active.
    click(cx, "sidebar-search");
    cx.simulate_input("Sign-up");
    cx.run_until_parked();
    assert!(bounds(cx, "session-found-ses_signup").is_none(), "active session hidden by Archived search filter");
    click(cx, "clear-search");

    // Engine-archived session omits Unarchive in actions and refuses direct unarchive with 409 banner.
    let actions = client.session_actions("ses_recipes").unwrap();
    assert!(!actions.actions.iter().any(|a| a.id == "unarchive"));
    let archived_recipes = bounds(cx, "session-archived-chat-ses_recipes").expect("archived recipes");
    cx.simulate_mouse_move(archived_recipes.center(), None, Modifiers::default());
    cx.run_until_parked();
    click(cx, "unarchive-session-archived-chat:ses_recipes");
    assert!(bounds(cx, "session-action-error").is_some());
    click(cx, "dismiss-session-error");
    assert!(bounds(cx, "session-action-error").is_none());

    // Clicking the chip's remove button turns Archived off and restores active sessions.
    click(cx, "showing-archived-remove");
    assert!(bounds(cx, "showing-archived").is_none());
    assert!(bounds(cx, "session-project-ses_signup").is_some());
}

#[gpui::test]
fn row_right_click_title_bar_menu_and_held_session_follow_box_actions(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_signup", window, cx);
    });
    cx.run_until_parked();

    // Title bar "•••" opens the full box-driven menu.
    click(cx, "session-more");
    assert!(workspace.read_with(cx, |workspace, _| workspace.session_menu().title_menu.is_open()));
    assert!(!stand_in.seen_at("GET", "kuvo/sessions/ses_signup/actions").is_empty());
    // Keyboard down -> Pin (top entry) -> Return chooses Pin.
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_signup/pin").len(), 1);

    // Right-click on a sidebar row opens the same box-driven menu on that row.
    let dark = bounds(cx, "session-project-ses_dark").expect("ses_dark row");
    right_click(cx, dark.center());
    assert!(
        workspace.read_with(cx, |workspace, _| workspace
            .session_menu()
            .row_menus
            .get("session-project:ses_dark")
            .is_some_and(|handle| handle.is_open()))
    );
    // "Down, Down, Enter" picks "Rename…" (second entry after Pin).
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some(), "Rename from row right-click opens row popover");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none());

    // Held session: menu prepends held header and disables all entries; unknown action IDs are skipped.
    stand_in.hold_session("ses_dark", Some("Moving to Studio…"));
    let actions = workspace
        .read_with(cx, |workspace, _| workspace.client().session_actions("ses_dark"))
        .unwrap();
    assert_eq!(actions.held.as_deref(), Some("Moving to Studio…"));
    let mut with_unknown = actions.clone();
    with_unknown.actions.insert(
        1,
        Action {
            id: "future_box_action".into(),
            label: "Future".into(),
            destructive: false,
            confirm: None,
        },
    );
    let items = build_menu_items(
        &with_unknown,
        "ses_dark".into(),
        PopoverAnchor::TitleBar,
        workspace.downgrade(),
    );
    assert!(matches!(&items[0], MenuItem::Header(text) if text.as_ref() == "Moving to Studio…"));
    assert!(
        !items.iter().any(|item| matches!(item, MenuItem::Entry(e) if e.label().starts_with("Future"))),
        "unknown action IDs must be skipped cleanly"
    );
    assert!(
        items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Entry(entry) => Some(entry),
                _ => None,
            })
            .all(|entry| entry.is_disabled()),
        "held session disables all entries"
    );

    // Triggering a held session's menu keeps entries disabled even when navigated via keyboard.
    right_click(cx, dark.center());
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none(), "disabled entries on held session do nothing");
}

#[gpui::test]
fn rename_popover_saves_on_enter_and_click_away_escapes_cleanly_and_keeps_errors(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_signup", window, cx);
        workspace.start_rename("ses_signup", PopoverAnchor::TitleBar, window, cx);
    });
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());

    // Escape discards without calling the box.
    cx.simulate_input("Should not save");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none());
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_signup/rename").is_empty());

    // Empty name leaves it unchanged without calling the box.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.start_rename("ses_signup", PopoverAnchor::TitleBar, window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("backspace");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none());
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_signup/rename").is_empty());

    // Held/refusal on submit keeps popover open with inline box error.
    stand_in.hold_session("ses_signup", Some("This session is moving right now."));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.start_rename("ses_signup", PopoverAnchor::TitleBar, window, cx);
    });
    cx.run_until_parked();
    cx.simulate_input("Sign-up polish");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());
    assert!(bounds(cx, "rename-error").is_some());
    let err = workspace.read_with(cx, |workspace, cx| match &workspace.session_menu().active_popover {
        Some(ActivePopover::Rename { view, .. }) => view.read(cx).error().map(str::to_owned),
        _ => None,
    });
    assert_eq!(err.as_deref(), Some("This session is moving right now."));

    // Release hold and click outside (mouse down out) -> saves on click away!
    stand_in.hold_session("ses_signup", None);
    cx.simulate_click(point_outside_sidebar(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none());
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_signup/rename").len(), 2);
    assert_eq!(
        workspace.read_with(cx, |workspace, _| {
            workspace.listing().facts("ses_signup").and_then(|f| f.title.clone())
        }),
        Some("Sign-up polish".into())
    );
}

#[gpui::test]
fn delete_confirm_shows_worktree_warning_refusals_and_returns_to_landing(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_dark", window, cx);
    });
    cx.run_until_parked();

    // ⇧⌘⌫ opens Delete… confirm popover with the box's exact worktree & uncommitted changes words.
    cx.simulate_keystrokes("cmd-shift-backspace");
    cx.run_until_parked();
    assert!(bounds(cx, "confirm-delete-popover").is_some());
    let popover_anchor = workspace.read_with(cx, |workspace, _| {
        workspace.session_menu().active_popover.as_ref().map(|p| p.anchor().clone())
    });
    assert_eq!(popover_anchor, Some(PopoverAnchor::TitleBar));

    // Held session refusal renders inline inside the confirm popover and keeps the session.
    stand_in.hold_session("ses_dark", Some("This session is moving to another computer."));
    click(cx, "delete-go");
    assert!(bounds(cx, "confirm-delete-popover").is_some());
    assert!(bounds(cx, "delete-error").is_some());
    assert!(bounds(cx, "session-project-ses_dark").is_some());

    // Release hold and confirm deletion: session leaves sidebar and workspace returns to Landing.
    stand_in.hold_session("ses_dark", None);
    click(cx, "delete-go");
    assert_eq!(stand_in.seen_at("DELETE", "kuvo/sessions/ses_dark").len(), 2);
    assert!(bounds(cx, "confirm-delete-popover").is_none());
    assert!(bounds(cx, "session-project-ses_dark").is_none());
    assert!(workspace.read_with(cx, |workspace, _| workspace.is_landing()));
}

#[gpui::test]
fn keyboard_shortcuts_respect_textfield_cmd_backspace_and_walked_rows(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_recipes", window, cx);
    });
    cx.run_until_parked();

    // Focus the composer's TextField: ⌘⌫ deletes typed text rather than archiving the session.
    let composer = workspace.read_with(cx, |workspace, cx| {
        workspace.conversation().unwrap().read(cx).composer().clone()
    });
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Hello draft");
    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert_eq!(field.read_with(cx, |f, _| f.text().to_owned()), "");
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/archive").is_empty());

    // With focus outside a TextField (on the workspace), ⌘⌫ archives the open session.
    cx.update(|window, cx| window.focus(&workspace.focus_handle(cx), cx));
    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/archive").len(), 1);

    // Walking the sidebar to ses_dark and pressing ⌘R anchors the Rename popover under that row.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_search(window, cx);
    });
    cx.run_until_parked();
    // Walk: down 1 (needs:ses_limits) -> down 2 (needs:ses_cms) -> down 3 (project:website)
    // -> down 4 (session-project:ses_signup) -> down 5 (session-project:ses_dark).
    cx.simulate_keystrokes("down down down down down cmd-r");
    cx.run_until_parked();
    assert_eq!(
        workspace.read_with(cx, |workspace, _| {
            workspace.session_menu().active_popover.as_ref().map(|p| p.anchor().clone())
        }),
        Some(PopoverAnchor::Row("session-project:ses_dark".into()))
    );
    assert!(bounds(cx, "rename-popover").is_some());

    // Toggling the Archived filter closes the row-anchored popover and lets shortcuts work again.
    click(cx, "toggle-archived");
    assert!(bounds(cx, "rename-popover").is_none());
    assert!(workspace.read_with(cx, |workspace, _| workspace.session_menu().active_popover.is_none()));
}

fn point_outside_sidebar() -> Point<Pixels> {
    gpui::point(px(900.), px(400.))
}

#[allow(dead_code)]
fn _unused_actions_type_check(_: Actions) {}
