//! Your settings as someone uses them, against the stand-in box: General, Appearance and
//! Notifications open with their rows, each change does what it says at once and is kept in the
//! app's own settings file (read again as the app opens again), and the notifications this device
//! shows of what your computers tell go by them. Clicks go through the window; each test keeps its
//! settings in a folder of its own.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{AppContext as _, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::Home;
use kuvo::area::Area;
use kuvo::notifications::{Effects, Notifier};
use kuvo::settings::{SettingsView, Side};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo::yours::editors::Editor;
use kuvo::yours::{self, Store};
use kuvo_box::BoxClient;
use kuvo_box::areas::Waiting;
use kuvo_box::notices::Told;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

/// The app's settings file, in a folder of the test's own.
fn kept() -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("desktop").join(yours::FILE);
    (folder, path)
}

fn open<'a>(cx: &'a mut TestAppContext, path: &PathBuf) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let path = path.clone();
    cx.update(|cx| {
        yours::init(Some(path), cx);
        Theme::init(yours::appearance(Appearance::Light, cx), cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(StandIn::at(Replies::Never, NOW));
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1800.)));
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
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    bounds(cx, selector).is_some()
}

fn settings(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<SettingsView> {
    workspace.read_with(cx, |workspace, cx| workspace.area_view(Area::Settings).and_then(|view| view.read(cx).settings().cloned()).expect("Settings"))
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()))
}

/// What the app would read as it opens again.
fn next_run(path: &PathBuf) -> yours::Yours {
    Store::read(Some(path.clone())).now().clone()
}

fn editors() -> Vec<Editor> {
    vec![Editor { id: "com.apple.TextEdit".into(), name: "TextEdit".into() }, Editor { id: "dev.zed.Zed".into(), name: "Zed".into() }]
}

#[gpui::test]
fn general_and_notifications_open_with_their_rows_and_the_rest_say_theyre_coming(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let (workspace, cx) = open(cx, &path);
    click(cx, "rail-settings");
    let view = settings(&workspace, cx);
    assert_eq!(view.read_with(cx, |view, _| view.side()), Side::Yours);
    assert_eq!(title(&workspace, cx).as_deref(), Some("General"));
    assert!(shown(cx, "row-editor"));
    click(cx, "setting-notifications");
    for row in ["row-notify-sessions", "row-notify-sound", "row-notify-dock", "row-notify-system"] {
        assert!(shown(cx, row), "{row}");
    }
    click(cx, "setting-appearance");
    assert!(shown(cx, "row-theme"));
    // The others aren't in the desktop app yet, and say so.
    click(cx, "settings-later");
    for later in ["voice", "shortcuts", "panel", "updates", "privacy"] {
        click(cx, &format!("setting-{later}"));
        assert!(shown(cx, "placeholder"), "{later}");
    }
}

#[gpui::test]
fn the_editor_picked_is_kept_and_names_files_open_button(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let (workspace, cx) = open(cx, &path);
    click(cx, "rail-settings");
    let view = settings(&workspace, cx);
    view.update(cx, |view, cx| view.set_editors(editors(), cx));
    cx.run_until_parked();
    // Each file's own app until one's picked.
    assert_eq!(view.read_with(cx, |view, _| view.row_detail("row-editor")).as_deref(), Some("From Files, for a file on this Mac."));
    click(cx, "row-editor-menu");
    click(cx, "menu-item-TextEdit");
    assert_eq!(next_run(&path).general.editor.as_deref(), Some("com.apple.TextEdit"));
    // A file of this Mac's computer opens in it, from Files.
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_signup", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-alt-f");
    cx.run_until_parked();
    let root = kuvo::stand_in::git::website();
    click(cx, &format!("tree-{root}/README.md"));
    assert!(shown(cx, "file-open-in"));
    let named = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).files().read(cx).editor().map(|editor| editor.name.clone()));
    // (TextEdit is on every Mac.)
    assert_eq!(named.as_deref(), Some("TextEdit"));
    // Back to each file's own app: the button opens with it.
    cx.update(|_, cx| yours::change(yours::Change::Editor(None), cx));
    cx.run_until_parked();
    assert_eq!(workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).files().read(cx).editor().cloned()), None);
    assert!(shown(cx, "file-open-in"));
    assert_eq!(next_run(&path).general.editor, None);
}

#[gpui::test]
fn another_computers_file_has_no_open_button(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let path = path.clone();
    cx.update(|cx| {
        yours::init(Some(path), cx);
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_elsewhere(true);
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(820.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        workspace.open_read_at_once("ses_signup", window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-alt-f");
    cx.run_until_parked();
    let root = kuvo::stand_in::git::website();
    click(cx, &format!("tree-{root}/README.md"));
    assert!(shown(cx, "file-viewer") && !shown(cx, "file-open-in"));
}

#[gpui::test]
fn notification_switches_are_kept_and_system_settings_opens(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let (_workspace, cx) = open(cx, &path);
    click(cx, "rail-settings");
    click(cx, "setting-notifications");
    click(cx, "row-notify-sessions-switch");
    click(cx, "row-notify-sound-switch");
    let kept = next_run(&path).notifications;
    assert!(!kept.sessions && !kept.sound && kept.dock);
    click(cx, "row-notify-dock-switch");
    click(cx, "row-notify-sessions-switch");
    let kept = next_run(&path).notifications;
    assert!(kept.sessions && !kept.sound && !kept.dock);
    click(cx, "row-notify-system-open");
    assert!(cx.opened_url().is_some_and(|url| url.starts_with("x-apple.systempreferences:com.apple.Notifications-Settings.extension")));
}

#[gpui::test]
fn the_theme_picked_is_kept_and_the_next_run_opens_in_it(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let (_workspace, cx) = open(cx, &path);
    click(cx, "rail-settings");
    click(cx, "setting-appearance");
    click(cx, "row-theme-menu");
    click(cx, "menu-item-Dark");
    assert!(cx.update(|_, cx| kuvo_ui::ActiveTheme::theme(cx).appearance.is_dark()));
    assert_eq!(next_run(&path).appearance.theme, Some(yours::Theme::Dark));
    // ⌘⇧L is kept too.
    cx.simulate_keystrokes("cmd-shift-l");
    cx.run_until_parked();
    assert_eq!(next_run(&path).appearance.theme, Some(yours::Theme::Light));
    // A Mac in dark mode still opens in your pick.
    cx.update(|_, cx| {
        yours::init(Some(path.clone()), cx);
        assert_eq!(yours::appearance(Appearance::Dark, cx), Appearance::Light);
    });
}

#[gpui::test]
fn a_settings_file_that_cant_be_read_opens_with_the_defaults(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not json").unwrap();
    let (_workspace, cx) = open(cx, &path);
    click(cx, "rail-settings");
    click(cx, "setting-notifications");
    assert!(shown(cx, "row-notify-sessions"));
    click(cx, "row-notify-sound-switch");
    assert!(!next_run(&path).notifications.sound, "a change writes a good file");
}

// MARK: Notifications

/// A window of the app's, in front.
struct Blank;

impl gpui::Render for Blank {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

/// What the system was asked: the Dock's counts in turn, and sounds made.
#[derive(Default)]
struct Desk {
    badges: Vec<usize>,
    sounds: usize,
}

fn notifier(cx: &mut TestAppContext) -> (Entity<Notifier>, Rc<RefCell<Desk>>) {
    let desk = Rc::new(RefCell::new(Desk::default()));
    let (badges, sounds) = (desk.clone(), desk.clone());
    cx.update(|cx| {
        yours::init(None, cx);
        cx.set_app_identity("run.kuvo.code", "Kuvo");
    });
    let effects = Effects { badge: Box::new(move |count| badges.borrow_mut().badges.push(count)), sound: Box::new(move || sounds.borrow_mut().sounds += 1), ..Effects::none() };
    let notifier = cx.new(|cx| Notifier::new(effects, cx));
    (notifier, desk)
}

fn told(telling: bool, notices: &[(&str, &str, Option<&str>)]) -> Told {
    let notices: Vec<_> = notices
        .iter()
        .map(|(id, category, quiet)| {
            let mut data = json!({ "session": "ses_1" });
            if let Some(quiet) = quiet {
                data["quiet"] = json!(quiet);
            }
            json!({ "id": id, "at": 1, "notice": { "title": "Fixer", "subtitle": "kuvo-ai", "body": "Done: the tests pass.", "category": category, "data": data } })
        })
        .collect();
    serde_json::from_value(json!({ "telling": telling, "notices": notices })).unwrap()
}

#[gpui::test]
fn what_a_computer_tells_shows_once_and_none_from_before_the_app(cx: &mut TestAppContext) {
    let (notifier, desk) = notifier(cx);
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_old", "finished", None)]), cx));
    assert!(cx.shown_system_notifications().is_empty(), "told before the app was there");
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_old", "finished", None), ("ntc_1", "finished", None)]), cx));
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!((shown[0].title.as_ref(), shown[0].body.as_ref()), ("Fixer · kuvo-ai", "Done: the tests pass."));
    assert_eq!(desk.borrow().sounds, 1);
    // Told again as it changes: nothing new, nothing more.
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_1", "finished", None)]), cx));
    assert_eq!(cx.shown_system_notifications().len(), 1);
    // What a goal needs of you, quietly: no sound.
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_1", "finished", None), ("ntc_2", "blocked", Some("1"))]), cx));
    assert_eq!((cx.shown_system_notifications().len(), desk.borrow().sounds), (2, 1));
    // A computer whose news isn't told now (its bridge isn't running) shows none.
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_devbox", told(true, &[]), cx));
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_devbox", told(false, &[("ntc_3", "question", None)]), cx));
    assert_eq!(cx.shown_system_notifications().len(), 2);
}

#[gpui::test]
fn your_notification_settings_decide_what_this_device_shows(cx: &mut TestAppContext) {
    let (notifier, desk) = notifier(cx);
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[]), cx));
    // Without a sound.
    cx.update(|cx| yours::change(yours::Change::Sound(false), cx));
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_1", "permission", None)]), cx));
    assert_eq!((cx.shown_system_notifications().len(), desk.borrow().sounds), (1, 0));
    // Sessions off: none of theirs; your limits' notices (set on each computer's Usage) still.
    cx.update(|cx| yours::change(yours::Change::Sessions(false), cx));
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_1", "permission", None), ("ntc_2", "finished", None), ("ntc_3", "usage", None)]), cx));
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[1].tag.as_ref(), "ntc_3");
}

#[gpui::test]
fn nothing_shows_while_kuvo_is_in_front(cx: &mut TestAppContext) {
    let (notifier, _desk) = notifier(cx);
    let window = cx.add_window(|_, _| Blank);
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[]), cx));
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_1", "finished", None)]), cx));
    assert!(cx.shown_system_notifications().is_empty(), "the window shows it already");
    cx.deactivate_window();
    notifier.update(cx, |notifier, cx| notifier.take_notices("key_mac", told(true, &[("ntc_1", "finished", None), ("ntc_2", "finished", None)]), cx));
    assert_eq!(cx.shown_system_notifications().len(), 1);
}

#[gpui::test]
fn the_dock_counts_what_waits_on_every_computer_while_its_switch_is_on(cx: &mut TestAppContext) {
    let (notifier, desk) = notifier(cx);
    notifier.update(cx, |notifier, cx| notifier.take_waiting("key_mac", Waiting { sessions: 2, bots: 1, queue: 4 }, cx));
    notifier.update(cx, |notifier, cx| notifier.take_waiting("key_devbox", Waiting { sessions: 1, bots: 0, queue: 0 }, cx));
    assert_eq!(desk.borrow().badges.last(), Some(&4));
    cx.update(|cx| yours::change(yours::Change::Dock(false), cx));
    cx.run_until_parked();
    assert_eq!(desk.borrow().badges.last(), Some(&0));
    cx.update(|cx| yours::change(yours::Change::Dock(true), cx));
    cx.run_until_parked();
    assert_eq!(desk.borrow().badges.last(), Some(&4));
    // Unchanged, the Dock isn't asked again.
    let asked = desk.borrow().badges.len();
    notifier.update(cx, |notifier, cx| notifier.take_waiting("key_devbox", Waiting { sessions: 1, bots: 0, queue: 2 }, cx));
    assert_eq!(desk.borrow().badges.len(), asked);
}

#[gpui::test]
fn clicking_a_notification_opens_its_computer_at_its_session(cx: &mut TestAppContext) {
    cx.update(|cx| {
        yours::init(None, cx);
        cx.set_app_identity("run.kuvo.code", "Kuvo");
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    let boxes = kuvo::home::stand_in_boxes(stand_in);
    let first = boxes.nearest().expect("the nearest box");
    let window = cx.add_window(move |window, cx| Home::new(boxes, first, false, window, cx));
    let home = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    home.update_in(cx, |home, _, cx| {
        home.set_now(NOW, cx);
        home.read_at_once(cx);
    });
    cx.run_until_parked();
    cx.deactivate_window();
    let notifier = home.read_with(cx, |home, _| home.notifier().clone());
    let mut news = told(true, &[]);
    notifier.update(cx, |notifier, cx| notifier.take_notices("", news.clone(), cx));
    news = serde_json::from_value(json!({ "telling": true, "notices": [{ "id": "ntc_1", "at": 1, "notice": { "title": "Signup form", "body": "Done.", "category": "finished", "data": { "session": "ses_signup" } } }] })).unwrap();
    notifier.update(cx, |notifier, cx| notifier.take_notices("", news, cx));
    assert_eq!(cx.shown_system_notifications().len(), 1);
    notifier.update(cx, |notifier, cx| notifier.clicked("ntc_1", cx));
    cx.run_until_parked();
    let workspace = home.read_with(cx, |home, _| home.workspace().clone());
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).as_deref(), Some("ses_signup"));
}
