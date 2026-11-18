//! What macOS allows this Mac's agents, as someone meets it in the window once it's set up, against
//! the stand-in box (nothing asks macOS anything, System Settings is never opened, and the tile's
//! window is a test's own): nothing brought up over the main window, and Settings › This Mac's
//! Open System Settings with the drag tile, going once the box says it's allowed. The step that
//! asks while setting up is in `first_run.rs` and `launch.rs`.

use std::time::Duration;

use gpui::{Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px, size};
use kuvo::permissions::HeadsUp;
use kuvo::stand_in::permissions::Permissions;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;
const ACCESSIBILITY: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
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

/// A stand-in for the helper's bundle, in a folder of the tests' own: there, as the box says.
fn helper() -> String {
    static MADE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let bundle = MADE.get_or_init(|| tempfile::tempdir().unwrap()).path().join("Kuvo Helper.app");
    std::fs::create_dir_all(bundle.join("Contents")).unwrap();
    bundle.to_str().unwrap().to_owned()
}

/// A Mac's own box whose helper says neither permission is allowed.
fn nothing_allowed() -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_permissions(|permissions| *permissions = Permissions { bundle: helper(), ..Permissions::nothing_allowed() });
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

fn tile_open(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    heads_up(workspace, cx).read_with(cx, |heads_up, _| heads_up.tile().is_some())
}

fn wait(cx: &mut VisualTestContext, how_long: Duration) {
    cx.executor().advance_clock(how_long);
    cx.run_until_parked();
}

/// Settings › This Mac, open.
fn this_mac(cx: &mut VisualTestContext) {
    click(cx, "rail-settings");
    if !shown(cx, "setting-computer") {
        click(cx, "settings-foot");
    }
    click(cx, "setting-computer");
}

#[gpui::test]
fn nothing_is_brought_up_over_the_main_window(cx: &mut TestAppContext) {
    // A Mac's own Kuvo whose helper says neither is allowed, past its first run: the window opens
    // on its sessions, with nothing over them and System Settings never opened.
    let (workspace, cx) = open(cx, nothing_allowed());
    assert!(shown(cx, "rail-settings"));
    assert!(!shown(cx, "permissions-step") && !shown(cx, "permission-screenRecording-open"));
    assert!(heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.asks()), "the box still says they're not allowed");
    assert_eq!(cx.opened_url(), None);
}

/// Settings › This Mac is where a permission is looked up and allowed after setting up: its row
/// opens System Settings with the tile to drag beside it. One tile serves them all: put away once
/// the box says one's allowed, the same window shown again (where it was) for the next, and gone
/// once none is left to add.
#[gpui::test]
fn settings_this_mac_opens_the_permission_with_the_tile(cx: &mut TestAppContext) {
    let stand_in = nothing_allowed();
    let (workspace, cx) = open(cx, stand_in.clone());
    this_mac(cx);
    assert!(shown(cx, "row-allows-screenRecording") && shown(cx, "row-allows-accessibility"));
    click(cx, "row-allows-accessibility-open-system-settings");
    assert_eq!(cx.opened_url().as_deref(), Some(ACCESSIBILITY));
    let tile = heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.tile()).expect("the tile");
    assert_eq!(tile.read_with(cx, |tile, _| tile.bundle().clone()).unwrap().to_str(), Some(helper().as_str()));
    assert_eq!(heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.asking().map(str::to_owned)).as_deref(), Some("accessibility"));
    // Not allowed yet, a few seconds on: it stays.
    wait(cx, 3 * SECOND);
    assert!(tile_open(&workspace, cx) && cx.windows().len() == 2);
    // Allowed in System Settings: the box says so, and the tile is put away by itself, its
    // window kept for Screen Recording, still to add.
    stand_in.set_permissions(|permissions| permissions.accessibility = true);
    wait(cx, SECOND);
    assert!(!tile_open(&workspace, cx));
    assert_eq!(heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.kept_tile()), Some(tile));
    wait(cx, 3 * SECOND);
    assert_eq!(cx.windows().len(), 2);
    // Screen Recording next: the same window, not a new one in a new place.
    click(cx, "row-allows-screenRecording-open-system-settings");
    assert_eq!(heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.tile()), Some(tile));
    assert_eq!(cx.windows().len(), 2);
    // The Files row can use the same tile to grant Full Disk Access.
    stand_in.set_permissions(|permissions| permissions.screen_recording = true);
    wait(cx, SECOND);
    assert!(!tile_open(&workspace, cx));
    click(cx, "row-allows-fullDiskAccess-open-system-settings");
    assert_eq!(cx.opened_url().as_deref(), Some("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles"));
    assert_eq!(heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.tile()), Some(tile));
    stand_in.set_permissions(|permissions| permissions.full_disk_access = true);
    wait(cx, SECOND);
    assert!(!tile_open(&workspace, cx) && heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.kept_tile()).is_none());
    wait(cx, SECOND);
    assert_eq!(cx.windows().len(), 1);
}

#[gpui::test]
fn the_helper_s_icon_is_what_s_dragged_and_the_tile_closes_when_you_close_it(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, nothing_allowed());
    this_mac(cx);
    click(cx, "row-allows-screenRecording-open-system-settings");
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
    // Closed by its own button: it goes, and nothing's asked of the box any more.
    click(&mut tile_cx, "permission-tile-close");
    cx.run_until_parked();
    assert!(!tile_open(&workspace, cx));
    assert!(heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.asking().is_none() && heads_up.kept_tile().is_none()));
    wait(cx, SECOND);
    assert_eq!(cx.windows().len(), 1);
}

#[gpui::test]
fn another_computer_s_permissions_arent_asked_about_here(cx: &mut TestAppContext) {
    // System Settings can't be opened on another computer from this one.
    let stand_in = nothing_allowed();
    stand_in.set_elsewhere(true);
    let (workspace, cx) = open(cx, stand_in.clone());
    assert!(!heads_up(&workspace, cx).read_with(cx, |heads_up, _| heads_up.asks()));
    assert!(stand_in.seen_at("GET", "kuvo/box/permissions").is_empty());
}
