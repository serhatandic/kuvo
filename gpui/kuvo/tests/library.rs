//! The Library as someone uses it, against the stand-in box (nothing real is reached): opened from
//! the rail on what every computer handed over, narrowed by a bot or a kind through the box's own
//! filters (the box decides; the window draws), named in the title bar by what's picked, and an item
//! removed from it through its menu, the box told. Clicks go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

/// 2026-11-14, 09:53 UTC.
const NOW: f64 = 1_794_650_000_000.0;

fn open(cx: &mut TestAppContext, empty: bool) -> (StandIn, Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.library_sample(empty);
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1240.), px(1400.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    click(cx, "rail-library");
    (stand_in, workspace, cx)
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

fn right_click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen")).center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
}

fn title(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (Option<String>, Option<String>) {
    workspace.read_with(cx, |workspace, cx| {
        let (title, beside) = workspace.title(cx);
        (title.map(|title| title.to_string()), beside.map(|beside| beside.to_string()))
    })
}

/// The queries the Library was read with, in order (a window that isn't live reads it once each time).
fn asked(stand_in: &StandIn) -> Vec<String> {
    stand_in.seen_at("GET", "kuvo/library").into_iter().map(|seen| seen.path.split_once('?').map_or(String::new(), |(_, query)| query.to_owned())).collect()
}

#[gpui::test]
fn the_library_opens_from_the_rail_on_what_every_computer_handed_over(cx: &mut TestAppContext) {
    let (stand_in, workspace, cx) = open(cx, false);
    assert_eq!(title(&workspace, cx), (Some("Library".into()), None));
    assert_eq!(asked(&stand_in), vec![String::new()]);
    // The newest first, this Mac's and the Mac Studio's alike; the sidebar by source.
    assert!(shown(cx, "library-item-lib_01"));
    assert!(shown(cx, "library-item-lib_02"));
    for source in ["library-source-all", "library-source-bot_foreman", "library-source-bot_builder", "library-source-yours"] {
        assert!(shown(cx, source), "{source}");
    }
}

#[gpui::test]
fn a_bot_or_a_kind_narrows_it_by_the_boxs_own_filter(cx: &mut TestAppContext) {
    let (stand_in, workspace, cx) = open(cx, false);
    click(cx, "library-source-bot_foreman");
    assert_eq!(title(&workspace, cx), (Some("Foreman".into()), Some("Library".into())));
    assert_eq!(asked(&stand_in).last().map(String::as_str), Some("source=bot_foreman"));
    assert!(shown(cx, "library-item-lib_04"));
    assert!(!shown(cx, "library-item-lib_01"));

    click(cx, "library-tab-documents");
    assert_eq!(asked(&stand_in).last().map(String::as_str), Some("source=bot_foreman&kind=document"));
    assert!(shown(cx, "library-item-lib_19"));
    assert!(!shown(cx, "library-item-lib_04"));

    click(cx, "library-source-yours");
    assert_eq!(title(&workspace, cx), (Some("Your sessions".into()), Some("Library".into())));
    assert!(shown(cx, "library-item-lib_06"));
}

#[gpui::test]
fn remove_from_library_takes_it_out_and_tells_the_box(cx: &mut TestAppContext) {
    let (stand_in, _workspace, cx) = open(cx, false);
    right_click(cx, "library-item-lib_01");
    assert!(shown(cx, "menu-item-Open"));
    assert!(shown(cx, "menu-item-Show in conversation"));
    click(cx, "menu-item-Remove from Library");
    assert!(!shown(cx, "library-item-lib_01"));
    assert_eq!(stand_in.seen_at("POST", "kuvo/library/lib_01/hide").len(), 1);
    assert!(shown(cx, "library-item-lib_02"));
}

#[gpui::test]
fn with_nothing_handed_over_it_says_what_will_be_there(cx: &mut TestAppContext) {
    let (_stand_in, _workspace, cx) = open(cx, true);
    assert!(shown(cx, "library-empty"));
    assert!(shown(cx, "library-source-all"));
}

/// View's items, as the menu bar shows them now.
fn view_items(cx: &mut VisualTestContext) -> Vec<String> {
    let menus = cx.update(|_, cx| cx.get_menus()).expect("the menu bar is set");
    let view = menus.iter().find(|menu| menu.name.as_ref() == "View").expect("View");
    view.items.iter().filter_map(|item| match item {
        gpui::OwnedMenuItem::Action { name, .. } => Some(name.clone()),
        _ => None,
    }).collect()
}

#[gpui::test]
fn where_the_computers_kuvo_has_no_library_none_of_it_shows(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
        kuvo::app_menu::init(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_lacks(&["library"]);
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, _| window.activate_window());
    cx.simulate_resize(size(px(1240.), px(900.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        window.focus(&gpui::Focusable::focus_handle(workspace, cx), cx);
    });
    cx.run_until_parked();
    // No rail button, no View › Library, and ⌃7 does nothing: nothing to explain or update.
    assert!(shown(cx, "rail-queue"));
    assert!(!shown(cx, "rail-library"));
    assert!(!view_items(cx).contains(&"Library".to_owned()));
    cx.simulate_keystrokes("ctrl-7");
    cx.run_until_parked();
    assert_ne!(workspace.read_with(cx, |workspace, _| workspace.area()), kuvo::area::Area::Library);
    assert!(!shown(cx, "held-area"));
    assert!(stand_in.seen_at("GET", "kuvo/library").is_empty());
}

#[gpui::test]
fn where_it_has_one_view_offers_the_library(cx: &mut TestAppContext) {
    cx.update(|cx| kuvo::app_menu::init(cx));
    let (_stand_in, _workspace, cx) = open(cx, false);
    assert!(view_items(cx).contains(&"Library".to_owned()));
}

#[gpui::test]
fn a_document_opens_in_the_side_panel_beside_its_conversation(cx: &mut TestAppContext) {
    let (_stand_in, workspace, cx) = open(cx, false);
    // The term sheet, a PDF on this Mac: in its own tab, with Open for its app here.
    right_click(cx, "library-item-lib_06");
    click(cx, "menu-item-Open");
    let showing = workspace.read_with(cx, |workspace, cx| (workspace.area(), workspace.is_panel_open(), workspace.panel().read(cx).active().map(|tab| tab.kind.clone())));
    assert_eq!(showing, (kuvo::area::Area::Sessions, true, Some(kuvo::panel::Kind::File("/Users/sample/Library Items/Series A Term Sheet.pdf".into()))));
}

/// Which area is shown, the bot whose conversation Bots shows, and the session open in Sessions.
fn landed(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (kuvo::area::Area, Option<String>, Option<String>) {
    workspace.read_with(cx, |workspace, _| {
        (workspace.area(), workspace.bot_chat().map(|(bot, _)| bot.to_owned()), workspace.opened().map(str::to_owned))
    })
}

#[gpui::test]
fn show_in_conversation_on_a_bots_item_opens_the_bots_conversation_in_bots(cx: &mut TestAppContext) {
    let (_stand_in, workspace, cx) = open(cx, false);
    // Foreman's picture, handed over in its own conversation (`ses_foreman`, a bot's as the box lists it).
    right_click(cx, "library-item-lib_04");
    click(cx, "menu-item-Show in conversation");
    assert_eq!(landed(&workspace, cx), (kuvo::area::Area::Bots, Some("bot_foreman".into()), None));
}

#[gpui::test]
fn show_in_conversation_on_a_sessions_item_opens_it_in_sessions(cx: &mut TestAppContext) {
    let (_stand_in, workspace, cx) = open(cx, false);
    right_click(cx, "library-item-lib_06");
    click(cx, "menu-item-Show in conversation");
    assert_eq!(landed(&workspace, cx), (kuvo::area::Area::Sessions, None, Some("ses_yours".into())));
}
