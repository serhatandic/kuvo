use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, px, size};
use kuvo::{
    Workspace,
    area::Area,
    stand_in::{Replies, StandIn, moves::Scenario},
};
use kuvo_box::{
    BoxClient,
    moves::{Kind, Target},
};
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

fn open<'a>(
    cx: &'a mut TestAppContext,
    stand_in: &StandIn,
    scenario: Scenario,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    stand_in.set_move_scenario(scenario);
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.read_at_once(cx);
        workspace.show_area(Area::Bots, window, cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} isn’t drawn"))
        .center();
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();
}

fn move_menu(cx: &mut VisualTestContext) {
    let at = cx
        .debug_bounds("bot-bot_mira")
        .expect("Mira's row")
        .center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn bot_move_confirms_and_the_row_follows_the_receipt(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, 1_794_650_000_000.0);
    let (workspace, cx) = open(cx, &stand_in, Scenario::Ready);
    click(cx, "bot-bot_mira");

    move_menu(cx);
    click(cx, "menu-item-MacBook Air");
    assert!(cx.debug_bounds("move-confirm").is_none());
    click(cx, "menu-item-Studio");
    assert!(cx.debug_bounds("move-confirm").is_some());
    assert!(stand_in.seen_at("POST", "kuvo/moves").is_empty());
    click(cx, "move-cancel");

    move_menu(cx);
    click(cx, "menu-item-Studio");
    click(cx, "move-go");
    let sent = stand_in.seen_at("POST", "kuvo/moves");
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].body,
        Some(json!({"kind":"bot","id":"bot_mira","to":"key_studio","receipt":true}))
    );
    assert!(cx.debug_bounds("move-done").is_some());
    assert!(cx.debug_bounds("bot-bot_mira").is_none());
    assert!(cx.debug_bounds("elsewhere-bot_mira").is_some());
    workspace.update_in(cx, |workspace, _, cx| {
        let bots = workspace.area_view(Area::Bots).unwrap();
        assert_eq!(bots.read(cx).selected(), Some("elsewhere:bot_mira"));
        assert!(workspace.bot_chat().is_none());
    });
}

#[gpui::test]
fn bot_move_failure_can_retry_with_a_fresh_confirmation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, 1_794_650_000_000.0);
    let (_, cx) = open(cx, &stand_in, Scenario::Failed);

    move_menu(cx);
    click(cx, "menu-item-Studio");
    click(cx, "move-go");
    assert!(cx.debug_bounds("move-failed").is_some());
    assert!(cx.debug_bounds("bot-bot_mira").is_some());
    click(cx, "move-again");
    assert!(cx.debug_bounds("move-confirm").is_some());
    assert_eq!(stand_in.seen_at("GET", "kuvo/moves/places").len(), 2);
    assert_eq!(stand_in.seen_at("POST", "kuvo/moves").len(), 1);
}

#[gpui::test]
fn bot_move_draws_the_sources_transfer_progress(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, 1_794_650_000_000.0);
    let (workspace, cx) = open(cx, &stand_in, Scenario::Copying);
    workspace.update_in(cx, |workspace, _, cx| {
        let target = Target::new(Kind::Bot, "bot_mira");
        let moving = workspace.moves().read(cx).moving(&target).unwrap();
        assert_eq!(moving.progress.as_deref(), Some("Moving to Studio…"));
        assert_eq!(moving.fraction(), Some(0.4375));
    });
    assert!(cx.debug_bounds("move-progress").is_some());
}

#[gpui::test]
fn bot_move_controls_also_open_for_chats_and_group_chats(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, 1_794_650_000_000.0);
    let (workspace, cx) = open(cx, &stand_in, Scenario::Ready);
    let chat = workspace.update_in(cx, |workspace, window, cx| {
        let chat = workspace.listing().sidebar().chats[0].clone();
        workspace.open_read_at_once(&chat, window, cx);
        chat
    });
    cx.run_until_parked();
    click(cx, "session-more");
    click(cx, "menu-item-Move to…");
    assert!(cx.debug_bounds("menu-item-Studio").is_some());
    assert!(
        stand_in
            .seen_at("GET", "kuvo/moves/places")
            .iter()
            .any(|request| request.path == format!("kuvo/moves/places?kind=chat&id={chat}"))
    );
    cx.simulate_keystrokes("escape");

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.show_area(Area::Bots, window, cx);
        workspace
            .area_view(Area::Bots)
            .cloned()
            .unwrap()
            .update(cx, |bots, cx| {
                bots.select(Some("group:grp_launch".into()), cx);
            });
    });
    cx.run_until_parked();
    click(cx, "bot-move-trigger");
    assert!(cx.debug_bounds("menu-item-Studio").is_some());
    assert!(
        stand_in
            .seen_at("GET", "kuvo/moves/places")
            .iter()
            .any(|request| request.path == "kuvo/moves/places?kind=group&id=grp_launch")
    );
}

#[gpui::test]
fn bot_move_from_other_computers_uses_the_bots_source(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, 1_794_650_000_000.0);
    stand_in.move_bot("bot_mira", "devbox");
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    devbox.set_move_scenario(Scenario::Failed);
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let boxes = kuvo::home::stand_in_boxes(stand_in.clone());
    let first = boxes.nearest().unwrap();
    let window = cx.add_window(move |window, cx| kuvo::Home::new(boxes, first, false, window, cx));
    let home = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(900.)));
    home.update_in(cx, |home, window, cx| {
        home.read_at_once(cx);
        home.workspace().clone().update(cx, |workspace, cx| {
            workspace.show_area(Area::Bots, window, cx)
        });
    });
    cx.run_until_parked();

    click(cx, "elsewhere-bot_mira");
    click(cx, "bot-move-trigger");
    click(cx, "menu-item-Studio");
    click(cx, "move-go");
    assert!(cx.debug_bounds("move-failed").is_some());
    click(cx, "move-again");
    assert!(cx.debug_bounds("move-confirm").is_some());
    assert_eq!(devbox.seen_at("POST", "kuvo/moves").len(), 1);
    assert_eq!(devbox.seen_at("GET", "kuvo/moves/receipt").len(), 1);
    assert_eq!(devbox.seen_at("GET", "kuvo/moves/places").len(), 2);
    assert!(stand_in.seen_at("POST", "kuvo/moves").is_empty());
    assert!(stand_in.seen_at("GET", "kuvo/moves/places").is_empty());
}
