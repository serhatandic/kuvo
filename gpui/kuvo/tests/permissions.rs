//! What macOS allows this Mac's agents, as someone meets it in the window, against the stand-in
//! box (nothing asks macOS anything, System Settings is never opened, and the tile's window is a
//! test's own): the heads-up for a permission the box says to bring up, Open System Settings with
//! the drag tile, the tile going once the box says it's allowed, and Not Now kept by the box.

use std::time::Duration;

use gpui::{Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px, size};
use kuvo::permissions::HeadsUp;
use kuvo::stand_in::permissions::Permissions;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;
const SCREEN: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
const SECOND: Duration = Duration::from_secs(1);

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

/// A Mac's own box whose helper says neither permission is allowed.
fn nothing_allowed() -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_permissions(|permissions| *permissions = Permissions::nothing_allowed());
    stand_in
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn heads_up(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<HeadsUp> {
    workspace.read_with(cx, |workspace, _| workspace.permissions().clone())
}

/// The permission the heads-up is about, by the box's name for it.
fn about(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    heads_up(workspace, cx).read_with(cx, |heads_up, _| heads_up.shown().map(|permission| permission.name.clone()))
}

fn tile_open(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    heads_up(workspace, cx).read_with(cx, |heads_up, _| heads_up.tile().is_some())
}

fn wait(cx: &mut VisualTestContext, how_long: Duration) {
    cx.executor().advance_clock(how_long);
    cx.run_until_parked();
}

#[gpui::test]
fn nothing_is_brought_up_where_the_box_says_nothing_needs_you(cx: &mut TestAppContext) {
    // A box that runs under no such program (the stand-in's sample), and one with both allowed.
    let (_, cx) = open(cx, StandIn::at(Replies::Never, NOW));
    assert!(!shown(cx, "permission-heads-up"));
    let allowed = StandIn::at(Replies::Never, NOW);
    allowed.set_permissions(|permissions| *permissions = Permissions { screen_recording: true, accessibility: true, ..Permissions::nothing_allowed() });
    let (_, cx) = open(cx, allowed);
    assert!(!shown(cx, "permission-heads-up"));
    // One whose helper hasn't said yet: nothing's brought up on a guess.
    let unknown = StandIn::at(Replies::Never, NOW);
    unknown.set_permissions(|permissions| *permissions = Permissions { known: false, ..Permissions::nothing_allowed() });
    let (_, cx) = open(cx, unknown);
    assert!(!shown(cx, "permission-heads-up"));
}

#[gpui::test]
fn open_system_settings_opens_the_box_s_pane_with_the_helper_to_drag_beside_it(cx: &mut TestAppContext) {
    let stand_in = nothing_allowed();
    let (workspace, cx) = open(cx, stand_in.clone());
    assert!(shown(cx, "permission-heads-up"));
    assert_eq!(about(&workspace, cx).as_deref(), Some("Screen Recording"));
    assert!(!tile_open(&workspace, cx));
    click(cx, "permission-open");
    // The pane the box named, and the tile, holding the helper's bundle as the box said it.
    assert_eq!(cx.opened_url().as_deref(), Some(SCREEN));
    let tile = heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.tile()).expect("the tile");
    assert_eq!(tile.read_with(cx, |tile, _| tile.bundle().clone()).unwrap().to_str(), Some("/Users/sam/.kuvo-code/bin/Kuvo Helper.app"));
    // Not allowed yet, a few seconds on: it stays.
    wait(cx, 3 * SECOND);
    assert!(tile_open(&workspace, cx) && cx.windows().len() == 2);
    // Dragged in and switched on: the box says so at its next ask, and the tile goes by itself.
    stand_in.set_permissions(|permissions| permissions.screen_recording = true);
    wait(cx, SECOND);
    assert!(!tile_open(&workspace, cx));
    assert_eq!(cx.windows().len(), 1);
    // The next one the box brings up takes its place.
    assert_eq!(about(&workspace, cx).as_deref(), Some("Accessibility"));
    stand_in.set_permissions(|permissions| permissions.accessibility = true);
    click(cx, "permission-open");
    wait(cx, SECOND);
    assert!(!shown(cx, "permission-heads-up") && !tile_open(&workspace, cx));
}

#[gpui::test]
fn the_helper_s_icon_is_what_s_dragged_and_the_tile_closes_when_you_close_it(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, nothing_allowed());
    click(cx, "permission-open");
    let tile = heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.tile()).expect("the tile");
    let mut tile_cx = VisualTestContext::from_window(tile.into(), cx);
    tile_cx.run_until_parked();
    assert!(shown(&mut tile_cx, "permission-tile-title"));
    // Pressed on the icon and moved: a drag of it begins (GPUI hands one that leaves the window
    // to macOS as the bundle's file).
    let icon = bounds(&mut tile_cx, "permission-tile-icon").expect("the icon").center();
    tile_cx.simulate_mouse_down(icon, MouseButton::Left, Modifiers::default());
    tile_cx.simulate_mouse_move(icon + point(px(12.), px(12.)), MouseButton::Left, Modifiers::default());
    assert!(tile_cx.update(|_, cx| cx.has_active_drag()));
    tile_cx.simulate_mouse_up(icon + point(px(12.), px(12.)), MouseButton::Left, Modifiers::default());
    tile_cx.run_until_parked();
    // Closed by its own button: it goes, and the heads-up stays for another try.
    click(&mut tile_cx, "permission-tile-close");
    cx.run_until_parked();
    assert!(!tile_open(&workspace, cx));
    assert!(shown(cx, "permission-heads-up"));
    assert_eq!(about(&workspace, cx).as_deref(), Some("Screen Recording"));
}

#[gpui::test]
fn not_now_is_kept_by_the_box_and_not_brought_up_at_the_next_open(cx: &mut TestAppContext) {
    let stand_in = nothing_allowed();
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "permission-later");
    let sent = stand_in.seen_at("POST", "kuvo/box/permissions");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body.as_ref().map(|body| body["later"].clone()), Some(serde_json::json!("screenRecording")));
    // The next one, then none; System Settings was never opened.
    assert_eq!(about(&workspace, cx).as_deref(), Some("Accessibility"));
    click(cx, "permission-later");
    assert!(!shown(cx, "permission-heads-up"));
    assert_eq!(cx.opened_url(), None);
    // The app opened again on the same box: still not brought up.
    let (_, cx) = open(cx, stand_in);
    assert!(!shown(cx, "permission-heads-up"));
}

#[gpui::test]
fn not_now_the_box_cant_take_says_so_and_keeps_the_heads_up(cx: &mut TestAppContext) {
    let stand_in = nothing_allowed();
    let (workspace, cx) = open(cx, stand_in.clone());
    stand_in.set_permissions(|permissions| permissions.fails = true);
    click(cx, "permission-later");
    assert!(shown(cx, "permission-heads-up") && shown(cx, "permission-heads-up-problem"));
    assert_eq!(about(&workspace, cx).as_deref(), Some("Screen Recording"));
}

#[gpui::test]
fn another_computer_s_permissions_arent_brought_up_here(cx: &mut TestAppContext) {
    // System Settings can't be opened on another computer from this one.
    let stand_in = nothing_allowed();
    stand_in.set_elsewhere(true);
    let (_, cx) = open(cx, stand_in.clone());
    assert!(!shown(cx, "permission-heads-up"));
    assert!(stand_in.seen_at("GET", "kuvo/box/permissions").is_empty());
}
