//! The desktop app's session menu (Draft B), row hover buttons, inline popovers, archived filter,
//! and keyboard shortcuts against the stand-in box.

use gpui::{
    Bounds, Entity, Focusable as _, Modifiers, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext, px, size,
};
use kuvo::session_menu::{ActivePopover, PopoverAnchor, build_menu_items};
use kuvo::sidebar::SHOWN;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_box::session_actions::Action;
use kuvo_box::transport::{Closer, Request, Response, Stream, Transport};
use kuvo_ui::menu::MenuItem;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    open_with_client(cx, BoxClient::new(stand_in))
}

fn open_with_client(cx: &mut TestAppContext, client: BoxClient) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
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

#[derive(Clone)]
struct CustomActionsBox {
    inner: StandIn,
    actions: serde_json::Value,
}

impl Transport for CustomActionsBox {
    fn send(&self, request: &Request) -> kuvo_box::Result<Response> {
        if request.method == "GET" && request.route().ends_with("/actions") && request.route().starts_with("kuvo/sessions/") {
            let session = request.route().split('/').nth(2).unwrap_or_default();
            let mut value = self.actions.clone();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("session".into(), serde_json::json!(session));
            }
            let _ = self.inner.send(request);
            return Ok(Response {
                status: 200,
                content_type: Some("application/json".into()),
                body: value.to_string().into_bytes(),
            });
        }
        self.inner.send(request)
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> kuvo_box::Result<Stream> {
        self.inner.open(request, hold)
    }

    fn here(&self) -> bool {
        self.inner.here()
    }

    fn streams(&self) -> bool {
        self.inner.streams()
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
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

    // Active search excludes archived sessions in both named and in-conversations results (MUST 1).
    click(cx, "sidebar-search");
    cx.simulate_input("risotto");
    cx.run_until_parked();
    assert!(bounds(cx, "found-ses_recipes").is_none(), "archived chat must stay hidden in active in-conversations search");
    click(cx, "clear-search");
    click(cx, "sidebar-search");
    cx.simulate_input("Dark mode");
    cx.run_until_parked();
    assert!(bounds(cx, "session-found-ses_dark").is_none(), "archived session must stay hidden in active named search");
    click(cx, "clear-search");

    // The Archived filter (no longer offered beside the search) shows the chip and placeholder
    // "Search archived".
    workspace.update_in(cx, |workspace, window, cx| workspace.set_archived_filter(true, window, cx));
    cx.run_until_parked();
    assert!(bounds(cx, "showing-archived").is_some());
    assert_eq!(
        workspace.read_with(cx, |workspace, cx| workspace.search().read(cx).placeholder_text().to_owned()),
        "Search archived"
    );
    assert!(bounds(cx, "archived-project-Website").is_some());
    assert!(bounds(cx, "session-archived-project-ses_dark").is_some());
    assert!(bounds(cx, "session-archived-chat-ses_recipes").is_some());
    assert!(bounds(cx, "session-project-ses_signup").is_none(), "active sessions hidden when Archived is on");

    // Hovering an archived row shows Unarchive and Delete… buttons driven by the box's actions.
    let archived_dark = bounds(cx, "session-archived-project-ses_dark").expect("archived ses_dark");
    cx.simulate_mouse_move(archived_dark.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "unarchive-session-archived-project:ses_dark").is_some());
    assert!(bounds(cx, "delete-session-archived-project:ses_dark").is_some());
    assert!(bounds(cx, "pin-session-archived-project:ses_dark").is_none());

    // Archived search includes archived named/conversation hits and excludes active sessions (MUST 1).
    click(cx, "sidebar-search");
    cx.simulate_input("risotto");
    cx.run_until_parked();
    assert!(bounds(cx, "found-ses_recipes").is_some(), "archived conversation hit shows when Archived is on");
    click(cx, "clear-search");
    click(cx, "sidebar-search");
    cx.simulate_input("Sign-up");
    cx.run_until_parked();
    assert!(bounds(cx, "session-found-ses_signup").is_none(), "active session hidden by Archived search filter");
    click(cx, "clear-search");

    // Engine-archived session omits Unarchive on hover (only Delete… from box actions), while direct unarchive 409 shows banner.
    let actions = client.session_actions("ses_recipes").unwrap();
    assert!(!actions.actions.iter().any(|a| a.id == "unarchive"));
    let archived_recipes = bounds(cx, "session-archived-chat-ses_recipes").expect("archived recipes");
    cx.simulate_mouse_move(archived_recipes.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "unarchive-session-archived-chat:ses_recipes").is_none(), "engine-archived row must not offer Unarchive");
    assert!(bounds(cx, "delete-session-archived-chat:ses_recipes").is_some(), "engine-archived row still offers Delete…");
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.archive_session("ses_recipes", false, window, cx);
    });
    cx.run_until_parked();
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

    // Held/away session: hover shows NO hover buttons, menu prepends held header and disables all entries.
    stand_in.hold_session("ses_dark", Some("Moving to Studio…"));
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    cx.simulate_mouse_move(point_outside_sidebar(), None, Modifiers::default());
    cx.run_until_parked();
    let held_dark = bounds(cx, "session-project-ses_dark").expect("held ses_dark row");
    cx.simulate_mouse_move(held_dark.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "pin-session-project:ses_dark").is_none(), "held/away row must not render Pin hover button");
    assert!(bounds(cx, "archive-session-project:ses_dark").is_none(), "held/away row must not render Archive hover button");
    assert!(bounds(cx, "delete-session-project:ses_dark").is_none(), "held/away row must not render Delete hover button");

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
        false,
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
    right_click(cx, held_dark.center());
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none(), "disabled entries on held session do nothing");
}

#[gpui::test]
fn row_hover_buttons_use_cached_box_actions_labels_and_held_state(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let custom = CustomActionsBox {
        inner: stand_in.clone(),
        actions: serde_json::json!({
            "actions": [
                { "id": "pin", "label": "Stick to top" },
                { "id": "archive", "label": "Put away" }
            ]
        }),
    };
    let (workspace, cx) = open_with_client(cx, BoxClient::new(custom));

    // Hover once fetches actions; leaving and re-hovering reuses cached actions until a change invalidates them.
    let dark = bounds(cx, "session-project-ses_dark").expect("ses_dark row");
    cx.simulate_mouse_move(dark.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("GET", "kuvo/sessions/ses_dark/actions").len(), 1);
    assert!(bounds(cx, "pin-session-project:ses_dark").is_some());
    assert!(bounds(cx, "archive-session-project:ses_dark").is_some());
    assert_eq!(
        workspace.read_with(cx, |w, _| w.session_menu().cached_actions("ses_dark").map(|a| a.actions[0].label.clone())),
        Some("Stick to top".into())
    );

    cx.simulate_mouse_move(point_outside_sidebar(), None, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_mouse_move(dark.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("GET", "kuvo/sessions/ses_dark/actions").len(), 1, "hover actions are cached per row");

    // Pinning invalidates and refreshes cached actions on change.
    click(cx, "pin-session-project:ses_dark");
    assert_eq!(stand_in.seen_at("GET", "kuvo/sessions/ses_dark/actions").len(), 2);

    // An unrelated session changing in a list update neither clears nor refetches ses_dark.
    stand_in.send(&Request::post("kuvo/sessions/ses_signup/pin", serde_json::json!({ "pinned": true }))).unwrap();
    workspace.update_in(cx, |workspace, _, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert_eq!(
        stand_in.seen_at("GET", "kuvo/sessions/ses_dark/actions").len(),
        2,
        "unrelated list updates must not refetch hovered ses_dark actions"
    );
    // (Pinned, ses_signup now sits above it: the pointer goes to where ses_dark is now.)
    let moved = bounds(cx, "session-project-ses_dark").expect("ses_dark row");
    cx.simulate_mouse_move(moved.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("GET", "kuvo/sessions/ses_dark/actions").len(), 2, "its cached actions, not read again");
    assert!(bounds(cx, "pin-session-project:ses_dark").is_some(), "hover buttons stay visible on unrelated updates");

    // Even while ses_dark is stale or an async refetch is in flight, old hover buttons stay visible (no flicker).
    let seq = workspace.update_in(cx, |workspace, _, cx| {
        workspace.session_menu_mut().invalidate_session("ses_dark");
        let seq = workspace.session_menu_mut().begin_fetch("ses_dark");
        cx.notify();
        seq
    });
    cx.run_until_parked();
    assert!(
        bounds(cx, "pin-session-project:ses_dark").is_some()
            && bounds(cx, "archive-session-project:ses_dark").is_some(),
        "cached hover buttons stay drawn while a refetch is in flight"
    );
    let fresh = workspace.read_with(cx, |w, _| w.client().session_actions("ses_dark")).unwrap();
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.session_menu_mut().finish_fetch("ses_dark", seq, Ok(fresh));
        cx.notify();
    });
    cx.run_until_parked();
    assert!(bounds(cx, "pin-session-project:ses_dark").is_some());
}

#[gpui::test]
fn actions_held_without_away_flag_hides_hover_buttons(cx: &mut TestAppContext) {
    // Even when a box holds a session without `away: true` in listing, actions.held suppresses hover buttons.
    let held_box = CustomActionsBox {
        inner: StandIn::at(Replies::Never, NOW),
        actions: serde_json::json!({
            "held": "Moving right now…",
            "actions": [{ "id": "pin", "label": "Pin" }]
        }),
    };
    let (_held_workspace, cx) = open_with_client(cx, BoxClient::new(held_box));
    let dark = bounds(cx, "session-project-ses_dark").expect("ses_dark row");
    cx.simulate_mouse_move(dark.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds(cx, "pin-session-project:ses_dark").is_none(), "actions.held hides row hover buttons");
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
        window.focus(&workspace.focus_handle(cx), cx);
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
fn delete_never_deletes_without_confirm_popover(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let no_confirm = CustomActionsBox {
        inner: stand_in.clone(),
        actions: serde_json::json!({
            "actions": [{
                "id": "delete",
                "label": "Delete",
                "destructive": true
            }]
        }),
    };
    let (workspace, cx) = open_with_client(cx, BoxClient::new(no_confirm));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_recipes", window, cx);
        window.focus(&workspace.focus_handle(cx), cx);
    });
    cx.run_until_parked();

    // When the box gives confirm: None, ⇧⌘⌫ refuses to delete, opens no confirm popover with client wording,
    // and surfaces the box error instead.
    cx.simulate_keystrokes("cmd-shift-backspace");
    cx.run_until_parked();
    assert!(stand_in.seen_at("DELETE", "kuvo/sessions/ses_recipes").is_empty());
    assert!(bounds(cx, "confirm-delete-popover").is_none());
    assert!(workspace.read_with(cx, |w, _| w.session_menu().active_popover.is_none()));
    assert!(bounds(cx, "session-action-error").is_some());
    assert!(bounds(cx, "session-chat-ses_recipes").is_some());
    click(cx, "dismiss-session-error");
    assert!(bounds(cx, "session-action-error").is_none());

    // Direct run_delete_action(_, None, ...) likewise shows the error and never deletes.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.run_delete_action("ses_recipes", None, PopoverAnchor::TitleBar, window, cx);
    });
    cx.run_until_parked();
    assert!(stand_in.seen_at("DELETE", "kuvo/sessions/ses_recipes").is_empty());
    assert!(bounds(cx, "confirm-delete-popover").is_none());
    assert!(workspace.read_with(cx, |w, _| w.session_menu().active_popover.is_none()));
    assert!(bounds(cx, "session-action-error").is_some());
}

#[gpui::test]
fn keyboard_shortcuts_respect_textfield_walked_rows_and_popover_cleanup(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_recipes", window, cx);
    });
    cx.run_until_parked();

    // Focus the composer's TextField: ⌘R, ⌘⌫, and ⇧⌘⌫ never trigger session actions while typing.
    let composer = workspace.read_with(cx, |workspace, cx| {
        workspace.conversation().unwrap().read(cx).composer().clone()
    });
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Hello draft");
    cx.simulate_keystrokes("cmd-r cmd-shift-backspace cmd-backspace");
    cx.run_until_parked();
    assert_eq!(field.read_with(cx, |f, _| f.text().to_owned()), "");
    assert!(bounds(cx, "rename-popover").is_none());
    assert!(bounds(cx, "confirm-delete-popover").is_none());
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/archive").is_empty());

    // Focus the sidebar search field: ⌘R and ⇧⌘⌫ are ignored there too.
    click(cx, "sidebar-search");
    cx.simulate_input("test");
    cx.simulate_keystrokes("cmd-r cmd-shift-backspace");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none());
    assert!(bounds(cx, "confirm-delete-popover").is_none());
    click(cx, "clear-search");

    // Walk to ses_recipes while ses_dark is open in the background, then archive it via ⌘⌫:
    // archive_session clears walked & hovered_row, and while sidebar list focus stays with nothing walked,
    // a follow-up ⌘⌫ / ⌘R / ⇧⌘⌫ acts on nothing instead of hitting the background ses_dark.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_dark", window, cx);
        workspace.set_hovered_row(Some("session-chat:ses_recipes".into()), cx);
        workspace.focus_search(window, cx);
    });
    cx.run_until_parked();
    // Walk: 1(needs:limits), 2(needs:cms), 3(proj:website), 4..7(website sessions), 8(proj:api),
    // 9..10(api), 11(proj:mobile), 12(ses_push), 13(proj:infra), 14(ses_recipes).
    for _ in 0..14 {
        cx.simulate_keystrokes("down");
    }
    cx.run_until_parked();
    assert_eq!(workspace.read_with(cx, |w, _| w.walked().map(str::to_owned)), Some("session-chat:ses_recipes".into()));
    cx.simulate_keystrokes("cmd-backspace");
    cx.run_until_parked();
    assert_eq!(stand_in.seen_at("POST", "kuvo/sessions/ses_recipes/archive").len(), 1);
    assert_eq!(workspace.read_with(cx, |w, _| w.walked().map(str::to_owned)), None);
    assert_eq!(workspace.read_with(cx, |w, _| w.session_menu().hovered_row.clone()), None);

    // Pressing ⌘⌫ (or ⌘R / ⇧⌘⌫) again right after archiving the walked row acts on nothing.
    cx.simulate_keystrokes("cmd-backspace cmd-r cmd-shift-backspace");
    cx.run_until_parked();
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_dark/archive").is_empty());
    assert!(bounds(cx, "rename-popover").is_none());
    assert!(bounds(cx, "confirm-delete-popover").is_none());

    // Moving focus out of the sidebar list (e.g. into the open conversation) acts on open ses_dark again.
    workspace.update_in(cx, |workspace, window, cx| {
        window.focus(&workspace.focus_handle(cx), cx);
    });
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none());

    // If walked holds a key missing from sidebar lines (e.g. its project collapsed), shortcuts return None instead of hitting opened().
    let website_dir = format!("{}/Projects/website", kuvo::stand_in::HOME);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_signup", window, cx);
        workspace.focus_search(window, cx);
    });
    cx.run_until_parked();
    // (Needs you's two, the project, ses_signup at work and ses_cms waiting on you, then ses_dark.)
    cx.simulate_keystrokes("down down down down down down");
    cx.run_until_parked();
    assert_eq!(workspace.read_with(cx, |w, _| w.walked().map(str::to_owned)), Some("session-project:ses_dark".into()));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.toggle_project(&website_dir, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-r cmd-shift-backspace cmd-backspace");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_none());
    assert!(bounds(cx, "confirm-delete-popover").is_none());
    assert!(stand_in.seen_at("POST", "kuvo/sessions/ses_signup/archive").is_empty());
}

#[gpui::test]
fn active_popover_clears_when_anchor_leaves(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    let website_dir = format!("{}/Projects/website", kuvo::stand_in::HOME);

    // 1. new_session clears TitleBar popover and keeps shortcuts un-wedged.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_dark", window, cx);
        workspace.start_rename("ses_dark", PopoverAnchor::TitleBar, window, cx);
    });
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.new_session(None, window, cx);
    });
    cx.run_until_parked();
    assert!(workspace.read_with(cx, |w, _| w.session_menu().active_popover.is_none()));

    // 2. open_session on the ALREADY-open session clears active_popover.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_read_at_once("ses_dark", window, cx);
        workspace.start_rename("ses_dark", PopoverAnchor::TitleBar, window, cx);
    });
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_session("ses_dark", window, cx);
    });
    cx.run_until_parked();
    assert!(workspace.read_with(cx, |w, _| w.session_menu().active_popover.is_none()));
    // Confirm shortcut still opens rename cleanly afterward (not wedged).
    workspace.update_in(cx, |workspace, window, cx| {
        window.focus(&workspace.focus_handle(cx), cx);
    });
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());

    // 3. toggle_project clears Row popover whose project collapsed.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.start_rename("ses_dark", PopoverAnchor::Row("session-project:ses_dark".into()), window, cx);
        workspace.toggle_project(&website_dir, cx);
    });
    cx.run_until_parked();
    assert!(workspace.read_with(cx, |w, _| w.session_menu().active_popover.is_none()));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.toggle_project(&website_dir, cx);
    });
    cx.run_until_parked();

    // 4. search_changed clears Row popover whose row left the filtered lines.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.start_rename("ses_dark", PopoverAnchor::Row("session-project:ses_dark".into()), window, cx);
    });
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.search().clone().update(cx, |field, cx| field.set_text("Sign-up", cx));
    });
    cx.run_until_parked();
    assert!(workspace.read_with(cx, |w, _| w.session_menu().active_popover.is_none()));
    click(cx, "clear-search");

    // 5. toggle_all clears Row popover when collapsing "Show more" hides its row.
    // Create enough extra sessions in Website so there are > SHOWN sessions.
    for _ in 0..4 {
        stand_in
            .send(&Request::post(
                "kuvo/sessions",
                serde_json::json!({ "project": website_dir }),
            ))
            .unwrap();
    }
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.read_at_once(cx);
        workspace.toggle_all(&website_dir, cx);
    });
    cx.run_until_parked();
    let hidden_id = "ses_lighthouse";
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.start_rename(hidden_id, PopoverAnchor::Row(format!("session-project:{hidden_id}")), window, cx);
    });
    cx.run_until_parked();
    assert!(bounds(cx, "rename-popover").is_some());
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.toggle_all(&website_dir, cx);
    });
    cx.run_until_parked();
    assert!(
        workspace.read_with(cx, |w, _| w.session_menu().active_popover.is_none()),
        "toggle_all hiding row (> {SHOWN}) clears its popover"
    );
}

fn point_outside_sidebar() -> Point<Pixels> {
    gpui::point(px(900.), px(400.))
}


#[gpui::test]
fn the_sidebar_search_has_no_archived_filter_button(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, cx) = open(cx, stand_in);
    assert!(bounds(cx, "sidebar-search").is_some(), "the sidebar's search is drawn");
    assert!(bounds(cx, "toggle-archived").is_none(), "no filter button beside the search (Settings › Archived Sessions lists them)");
}

#[gpui::test]
fn add_project_at_the_projects_heading_lists_the_folder_picked(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, cx) = open(cx, stand_in.clone());
    assert!(bounds(cx, "project-Garden").is_none());

    click(cx, "add-project");
    assert!(cx.did_prompt_for_paths(), "the system's folder picker opens");
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple, "it picks one folder");
        Some(vec![std::path::PathBuf::from("/Users/you/Code/Garden")])
    });
    cx.run_until_parked();

    let sent: Vec<String> = stand_in.seen_at("PUT", "kuvo/projects").into_iter().map(|seen| seen.path).collect();
    assert_eq!(sent, ["kuvo/projects?directory=%2FUsers%2Fyou%2FCode%2FGarden"]);
    assert!(bounds(cx, "project-Garden").is_some(), "the folder is listed under Projects");
}
