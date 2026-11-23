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
fn a_folded_sidebar_stays_folded_after_switching_areas_and_a_restart(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        let here = workspace(&home, cx);
        here.update_in(cx, |workspace, window, cx| workspace.toggle_sidebar(window, cx));
        here.update_in(cx, |workspace, window, cx| workspace.rail_pressed(Area::Bots, window, cx));
        here.update_in(cx, |workspace, window, cx| workspace.rail_pressed(Area::Sessions, window, cx));
        cx.run_until_parked();
    }
    // (Kept as it went: nothing waits for a quit.)
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["sidebarFolded"], true);
    let (home, cx) = open(cx, &path);
    assert_eq!(workspace(&home, cx).read_with(cx, |workspace, _| (workspace.area(), workspace.sidebar_shown())), (Area::Sessions, false));
    assert!(!shown(cx, "area-sidebar"));
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
    assert!(shown(cx, "bot-chat-title"));
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
fn another_computers_conversation_opens_again_in_the_one_window(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let sharing = || {
        let stand_in = StandIn::at(Replies::Never, NOW);
        stand_in.share_setup_lists();
        stand_in
    };
    {
        let (home, cx) = open_over(cx, &path, sharing(), |_, _| {});
        // devbox's conversation, from its project in the one list.
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_releases", window, cx));
        cx.run_until_parked();
        assert_eq!(shown_key(&home, cx), home.read_with(cx, |home, _| home.boxes().nearest_key()));
    }
    let (home, cx) = open_over(cx, &path, sharing(), |_, _| {});
    assert_eq!(shown_key(&home, cx), home.read_with(cx, |home, _| home.boxes().nearest_key()), "one window, whichever computer");
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_releases"));
}

#[gpui::test]
fn a_computer_kept_from_before_the_one_window_leaves_the_window_as_it_is(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Kept by an app that still had the computer picker: studio-mini was picked.
    std::fs::write(&path, json!({ "place": { "computer": "key_studio" } }).to_string()).unwrap();
    let mut waited = true;
    let (home, cx) = open_meanwhile(cx, &path, |home, cx| {
        // Drawn at once: nothing waits for your computers to be known.
        waited = home.read_with(cx, |home, _| home.is_restoring());
    });
    assert!(!waited);
    assert_eq!(shown_key(&home, cx), home.read_with(cx, |home, _| home.boxes().nearest_key()));
    assert!(!shown(cx, "window-restoring"));
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
    // goes where a notification clicked does): shown in the one window, read through devbox.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    let (home, cx) = open_over(cx, &path, stand_in, |home, cx| {
        let notifier = home.read_with(cx, |home, _| home.notifier().clone());
        notifier.update(cx, |notifier, cx| notifier.clicked("link|key_devbox|ses_releases", cx));
        cx.run_until_parked();
    });
    assert_eq!(shown_key(&home, cx), home.read_with(cx, |home, _| home.boxes().nearest_key()));
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_releases"));
}

/// Opens `bot`'s conversation from Bots, as a click on its row would.
fn open_bot(home: &Entity<Home>, bot: &str, cx: &mut VisualTestContext) {
    let here = workspace(home, cx);
    here.update_in(cx, |workspace, window, cx| workspace.show_area(Area::Bots, window, cx));
    cx.run_until_parked();
    let session = here.read_with(cx, |workspace, cx| match workspace.area_view(Area::Bots).unwrap().read(cx).bots() {
        kuvo::areas::Reading::Ready(bots) => bots.bots.iter().find(|each| each.id == bot).map(|each| each.session.clone())
            .or_else(|| bots.elsewhere.iter().find(|each| each.id == bot).map(|each| each.session.clone())).unwrap(),
        _ => panic!("bots read"),
    });
    here.update_in(cx, |workspace, window, cx| workspace.open_bot_chat(bot, &session, None, window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "bot-chat") && !shown(cx, "bot-head"), "its conversation on screen, not its page");
}

/// Bots read again, as the box's follow does once it answers.
fn bots_read_again(home: &Entity<Home>, cx: &mut VisualTestContext) {
    let view = workspace(home, cx).read_with(cx, |workspace, _| workspace.area_view(Area::Bots).unwrap().clone());
    view.update(cx, |view, cx| view.shown(cx));
    cx.run_until_parked();
}

/// A stand-in whose Mira lives on devbox, another of your computers.
fn mira_on_devbox() -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.move_bot("bot_mira", "devbox");
    stand_in.peer_box("key_devbox").unwrap().add_bot("bot_mira", "Mira", json!({ "color": "lime", "shape": "pebble", "face": "calm" }));
    stand_in
}

fn bot_chat(home: &Entity<Home>, cx: &mut VisualTestContext) -> Option<String> {
    workspace(home, cx).read_with(cx, |workspace, _| workspace.bot_chat().map(|(bot, _)| bot.to_owned()))
}

#[gpui::test]
fn a_bots_chat_opens_again_when_its_box_answers_only_after_the_window_opens(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        open_bot(&home, "bot_mira", cx);
    }
    // The app opens before this Mac's box answers (the box still starting): Bots can't be read yet.
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.bots_readable(false);
    let (home, cx) = open_over(cx, &path, stand_in.clone(), |_, _| {});
    assert_eq!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.area()), Area::Bots);
    stand_in.bots_readable(true);
    bots_read_again(&home, cx);
    assert_eq!(bot_chat(&home, cx).as_deref(), Some("bot_mira"), "its conversation, not its page");
    assert!(shown(cx, "bot-chat") && !shown(cx, "bot-head"), "its conversation on screen, not its page");
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["bot"], "bot_mira");
}

#[gpui::test]
fn another_computers_bots_chat_opens_again_not_its_page(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open_over(cx, &path, mira_on_devbox(), |_, _| {});
        open_bot(&home, "bot_mira", cx);
    }
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["bot"], "bot_mira");
    let (home, cx) = open_over(cx, &path, mira_on_devbox(), |_, _| {});
    assert_eq!(bot_chat(&home, cx).as_deref(), Some("bot_mira"), "its conversation, not its page");
    assert!(shown(cx, "bot-chat") && !shown(cx, "bot-head"), "its conversation on screen, not its page");
}

/// The user's (11-23, beta.46): with Kuvo Foreman, a bot on another computer, open, the title
/// said "Bots". It names the bot open, wherever it lives.
#[gpui::test]
fn a_bot_on_another_computer_is_named_in_the_title_while_its_chat_is_open(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let (home, cx) = open_over(cx, &path, mira_on_devbox(), |_, _| {});
    open_bot(&home, "bot_mira", cx);
    let title = workspace(&home, cx).read_with(cx, |workspace, cx| workspace.title(cx).0.map(|title| title.to_string()));
    assert_eq!(title.as_deref(), Some("Mira"));
}

#[gpui::test]
fn another_computers_bots_chat_opens_again_once_the_box_answers(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open_over(cx, &path, mira_on_devbox(), |_, _| {});
        open_bot(&home, "bot_mira", cx);
    }
    let stand_in = mira_on_devbox();
    stand_in.bots_readable(false);
    let (home, cx) = open_over(cx, &path, stand_in.clone(), |_, _| {});
    stand_in.bots_readable(true);
    bots_read_again(&home, cx);
    assert_eq!(bot_chat(&home, cx).as_deref(), Some("bot_mira"), "its conversation, not its page");
    assert!(shown(cx, "bot-chat") && !shown(cx, "bot-head"), "its conversation on screen, not its page");
}

#[gpui::test]
fn the_work_queue_opens_again_in_front(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.rail_pressed(Area::Queue, window, cx));
        cx.run_until_parked();
    }
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["area"], "queue");
    let (home, cx) = open(cx, &path);
    assert_eq!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.area()), Area::Queue);
    assert!(shown(cx, "rail-queue"));
}

/// A wheel's turn over the conversation, as the window routes it (up: positive).
fn wheel(home: &Entity<Home>, cx: &mut VisualTestContext, up: f32) {
    let view = workspace(home, cx).read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    let keys: Vec<String> = view.read_with(cx, |view, _| view.entries().iter().map(|entry| entry.key().to_owned()).collect());
    let at = keys.iter().rev().find_map(|key| cx.debug_bounds(Box::leak(format!("entry-{key}").into_boxed_str()))).expect("a message on screen").center();
    cx.simulate_event(gpui::ScrollEvent { position: at, delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(up))), ..Default::default() });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// The row at the top of the open conversation, and whether it follows the latest.
fn reading(home: &Entity<Home>, cx: &mut VisualTestContext) -> (String, bool) {
    let view = workspace(home, cx).read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    view.read_with(cx, |view, _| {
        let top = view.list().list_state().logical_scroll_top();
        (view.entries().get(top.item_ix).map(|entry| entry.key().to_owned()).unwrap_or_default(), view.list().is_following())
    })
}

#[gpui::test]
fn a_conversation_read_up_from_its_end_opens_again_where_it_was_read(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    let read_up = {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_lighthouse", window, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        for _ in 0..6 {
            wheel(&home, cx, 400.);
        }
        let (row, following) = reading(&home, cx);
        assert!(!following, "read up from the end");
        row
    };
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["reading"]["ses_lighthouse"]["row"], read_up.as_str(), "kept as it went");

    let (home, cx) = open(cx, &path);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(opened(&home, cx).as_deref(), Some("ses_lighthouse"));
    assert_eq!(reading(&home, cx), (read_up.clone(), false), "where it was read, not its end");
    assert!(shown(cx, &format!("entry-{read_up}")));
}

#[gpui::test]
fn a_conversation_at_its_end_opens_again_at_its_end(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_lighthouse", window, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        wheel(&home, cx, 400.);
        let jump = cx.debug_bounds("jump-to-latest").expect("the way back to the latest");
        cx.simulate_click(jump.center(), gpui::Modifiers::default());
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(reading(&home, cx).1, "back at the end");
    }
    assert_eq!(file(&path)["place"]["computers"]["nearest"]["reading"]["ses_lighthouse"], Value::Null);
    let (home, cx) = open(cx, &path);
    assert!(reading(&home, cx).1, "following its latest");
}

#[gpui::test]
fn the_side_panel_opens_again_as_it_was(cx: &mut TestAppContext) {
    let (_folder, path) = kept();
    {
        let (home, cx) = open(cx, &path);
        workspace(&home, cx).update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-alt-0");
        cx.run_until_parked();
        assert!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.is_panel_open()));
    }
    let (home, cx) = open(cx, &path);
    assert!(workspace(&home, cx).read_with(cx, |workspace, _| workspace.is_panel_open()), "open, as it was");
    assert!(shown(cx, "side-panel"));
}
