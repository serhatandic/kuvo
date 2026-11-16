//! The window opens where it was left (`yours::Place`, kept in this device's settings file as
//! you go): the computer it showed, each computer's area and what was open in it, the sidebar
//! folded or not. Against a stand-in of two computers (this one and devbox), each "restart" a new
//! window reading the same file. What's gone since, or a computer not reached, leaves the list
//! quietly; a notification's place wins over last time's.

use std::path::{Path, PathBuf};

use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use kuvo::Home;
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

const NOW: f64 = 1_794_650_000_000.0;

fn kept() -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("desktop").join(kuvo::yours::FILE);
    (folder, path)
}

/// The app opened (again): your settings read from `path`, a window over a fresh stand-in;
/// `meanwhile` happens before your computers' list and the box's lists are read.
fn open_meanwhile<'a>(cx: &'a mut TestAppContext, path: &Path, meanwhile: impl FnOnce(&Entity<Home>, &mut VisualTestContext)) -> (Entity<Home>, &'a mut VisualTestContext) {
    open_over(cx, path, StandIn::at(Replies::Never, NOW), meanwhile)
}

fn open_over<'a>(cx: &'a mut TestAppContext, path: &Path, stand_in: StandIn, meanwhile: impl FnOnce(&Entity<Home>, &mut VisualTestContext)) -> (Entity<Home>, &'a mut VisualTestContext) {
    let path = path.to_owned();
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
        kuvo::yours::init(Some(path), cx);
    });
    let boxes = kuvo::home::stand_in_boxes(stand_in);
    let first = boxes.nearest().expect("the nearest box");
    let window = cx.add_window(move |window, cx| Home::new(boxes, first, false, window, cx));
    let home = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1200.), px(800.)));
    home.update_in(cx, |home, _, cx| home.set_now(NOW, cx));
    meanwhile(&home, cx);
    home.update_in(cx, |home, _, cx| home.read_at_once(cx));
    cx.run_until_parked();
    (home, cx)
}

fn open<'a>(cx: &'a mut TestAppContext, path: &Path) -> (Entity<Home>, &'a mut VisualTestContext) {
    open_meanwhile(cx, path, |_, _| {})
}

fn workspace(home: &Entity<Home>, cx: &mut VisualTestContext) -> Entity<Workspace> {
    home.read_with(cx, |home, _| home.workspace().clone())
}

fn opened(home: &Entity<Home>, cx: &mut VisualTestContext) -> Option<String> {
    workspace(home, cx).read_with(cx, |workspace, _| workspace.opened().map(str::to_owned))
}

fn shown_key(home: &Entity<Home>, cx: &mut VisualTestContext) -> Option<String> {
    home.read_with(cx, |home, _| home.shown_key())
}

fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str())).is_some()
}

fn file(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[gpui::test]
fn a_conversation_and_folded_sidebar_open_again_after_a_restart(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| {
            workspace.open_read_at_once("ses_limits", window, cx);
            workspace.toggle_sidebar(window, cx);
        });
        cx.run_until_parked();
    }
    // (Kept as it went: nothing waits for a quit.)
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["session"], "ses_limits");

    let (home, cx) = open(cx, &path);
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_limits"));
    let here = workspace(&home, cx);
    assert_eq!(here.read_with(cx, |workspace, _| (workspace.area(), workspace.sidebar_shown())), (Area::Sessions, false));
    assert!(!shown(cx, "restoring") && !shown(cx, "window-restoring"));
}

#[gpui::test]
fn the_area_in_front_and_a_bots_conversation_open_again(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        let here = workspace(&home, cx);
        here.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
        here.update_in(cx, |workspace, window, cx| workspace.show_area(Area::Bots, window, cx));
        cx.run_until_parked();
        let session = here.read_with(cx, |workspace, cx| {
            let Some(view) = workspace.area_view(Area::Bots) else { unreachable!() };
            match view.read(cx).bots() {
                kuvo::areas::Reading::Ready(bots) => bots.bots.iter().find(|bot| bot.id == "bot_mira").unwrap().session.clone(),
                _ => panic!("bots read"),
            }
        });
        here.update_in(cx, |workspace, window, cx| workspace.open_bot_chat("bot_mira", &session, None, window, cx));
        cx.run_until_parked();
    }
    let (home, cx) = open(cx, &path);
    let here = workspace(&home, cx);
    assert_eq!(here.read_with(cx, |workspace, _| (workspace.area(), workspace.bot_chat().map(|(bot, _)| bot.to_owned()))), (Area::Bots, Some("bot_mira".into())));
    assert!(shown(cx, "bot-chat-strip"));
    // Sessions' conversation is there behind it, as it was.
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_limits"));
}

#[gpui::test]
fn what_is_gone_since_leaves_the_list_quietly(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let place = json!({ "computers": { "nearest": { "area": "sessions", "session": "ses_gone", "picked": { "bots": "bot:bot_gone", "queue": "item:itm_gone" } } } });
    std::fs::write(&path, json!({ "place": place }).to_string()).unwrap();
    let (home, cx) = open(cx, &path);
    let here = workspace(&home, cx);
    assert_eq!(here.read_with(cx, |workspace, _| (workspace.area(), workspace.is_landing(), workspace.is_restoring())), (Area::Sessions, true, false));
    assert!(!shown(cx, "restoring"), "the landing view, not an empty one");
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["session"], Value::Null, "no longer kept");
    // A row gone from its list: the list, not a page saying it's gone.
    here.update_in(cx, |workspace, window, cx| workspace.show_area(Area::Bots, window, cx));
    cx.run_until_parked();
    let picked = here.read_with(cx, |workspace, cx| workspace.area_view(Area::Bots).unwrap().read(cx).selected().map(str::to_owned));
    assert_eq!(picked, None);
    assert!(!shown(cx, "bot-gone"));
}

#[gpui::test]
fn each_computer_opens_where_it_was_and_the_window_on_the_computer_it_showed(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
        // devbox, from the switcher, at one of its own conversations.
        workspace(&home, cx).update(cx, |_, cx| cx.emit(kuvo::workspace::Switch::To("key_devbox".into())));
        cx.run_until_parked();
        assert_eq!(shown_key(&home, cx).as_deref(), Some("key_devbox"));
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_releases", window, cx));
        cx.run_until_parked();
    }
    let (home, cx) = open(cx, &path);
    assert_eq!(shown_key(&home, cx).as_deref(), Some("key_devbox"), "the computer it showed");
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_releases"));
    assert!(shown(cx, "entry-msg_d2"));
    // This computer's window, where it was on this one.
    let back = home.read_with(cx, |home, _| home.boxes().nearest_key()).unwrap();
    home.update_in(cx, |home, window, cx| assert!(home.switch_to(&back, window, cx)));
    cx.run_until_parked();
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_limits"));
}

#[gpui::test]
fn a_computer_not_reached_leaves_the_window_on_this_one(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // studio-mini is away: this app doesn't reach it.
    std::fs::write(&path, json!({ "place": { "computer": "key_studio" } }).to_string()).unwrap();
    let mut waited = false;
    let (home, cx) = open_meanwhile(cx, &path, |home, cx| {
        // Before your computers are known: nothing drawn, not this computer's window first.
        waited = home.read_with(cx, |home, _| home.is_restoring());
    });
    assert!(waited);
    assert!(!home.read_with(cx, |home, _| home.is_restoring()));
    assert_eq!(shown_key(&home, cx), home.read_with(cx, |home, _| home.boxes().nearest_key()));
    assert!(!shown(cx, "window-restoring"));
    // Still kept, for when it's reached again.
    assert_eq!(file(&path)["place"]["computer"], "key_studio");
}

#[gpui::test]
fn a_notifications_place_wins_over_last_times(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let place = json!({ "computer": "key_devbox", "computers": { "nearest": { "area": "sessions", "session": "ses_cms" }, "key_devbox": { "area": "sessions", "session": "ses_releases" } } });
    std::fs::write(&path, json!({ "place": place }).to_string()).unwrap();
    let (home, cx) = open_meanwhile(cx, &path, |home, cx| {
        let notifier = home.read_with(cx, |home, _| home.notifier().clone());
        notifier.update(cx, |notifier, cx| notifier.clicked("ntc_1||ses_limits", cx));
        cx.run_until_parked();
    });
    assert_eq!(shown_key(&home, cx), home.read_with(cx, |home, _| home.boxes().nearest_key()), "not last time's computer");
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_limits"), "the notification's session, not last time's");
    assert_eq!(file(&path)["place"]["computer"], Value::Null);
}

#[gpui::test]
fn the_sidebar_brought_back_by_a_search_stays_back_after_a_restart(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| {
            workspace.toggle_sidebar(window, cx);
            // (Typing in the landing view's composer.)
            window.focus(&workspace.landing().focus_handle(cx), cx);
        });
        cx.run_until_parked();
        assert!(!workspace(&home, cx).read_with(cx, |workspace, _| workspace.sidebar_shown()));
        cx.simulate_keystrokes("cmd-shift-f");
        cx.run_until_parked();
        assert!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.sidebar_shown()));
    }
    let (home, cx) = open(cx, &path);
    assert!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.sidebar_shown()));
}

#[gpui::test]
fn an_archived_conversation_opens_again_as_it_is_still_there(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
        cx.run_until_parked();
    }
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_engine_archived("ses_limits", true);
    let (home, cx) = open_over(cx, &path, stand_in, |_, _| {});
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_limits"));
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["session"], "ses_limits");
}

#[gpui::test]
fn a_link_to_another_computer_as_the_app_opens_shows_that_computers_session(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    // The menu bar's Kuvo Helper opened the app for devbox's session (`kuvo-code://open`, which
    // goes where a notification clicked does), before your computers' list was read: shown once
    // it's reached, not lost to this Mac's window.
    let (home, cx) = open_meanwhile(cx, &path, |home, cx| {
        let notifier = home.read_with(cx, |home, _| home.notifier().clone());
        notifier.update(cx, |notifier, cx| notifier.clicked("link|key_devbox|ses_releases", cx));
        cx.run_until_parked();
    });
    assert_eq!(shown_key(&home, cx).as_deref(), Some("key_devbox"));
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_releases"));
}
