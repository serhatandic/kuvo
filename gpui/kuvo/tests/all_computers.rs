//! One window for all your computers (the picker removed, docs/architecture/message-delivery.md,
//! "Removing the computer picker"): another computer's sessions show in Sessions as the nearest
//! box's setup list says them, and one opens through that computer's own box; while that computer
//! is away its rows stay, saying it's updating or when it was last seen, and don't open. The boxes
//! are the stand-in and its peer (`devbox`); clicks go through the window.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::sidebar::Line;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};

const NOW: f64 = 1_794_650_000_000.0;
const MINUTE: f64 = 60_000.0;
/// The peer's Billing project, as the lists key another computer's project.
const BILLING: &str = "key_devbox:/home/sam/billing";

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(2000.)));
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

/// The sessions under a project in the sidebar.
fn under(workspace: &Entity<Workspace>, project: &str, cx: &mut VisualTestContext) -> Vec<String> {
    let lines = workspace.read_with(cx, |workspace, cx| kuvo::sidebar::lines(workspace, cx));
    let Some(at) = lines.iter().position(|line| *line == Line::Project(project.into())) else { return Vec::new() };
    lines[at + 1..].iter().take_while(|line| !matches!(line, Line::Project(_) | Line::Header(_))).filter_map(|line| match line {
        Line::Session { id, .. } => Some(id.clone()),
        _ => None,
    }).collect()
}

#[gpui::test]
fn another_computers_sessions_show_under_its_project_and_open_through_that_computer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    let (workspace, cx) = open(cx, stand_in.clone());
    assert_eq!(under(&workspace, BILLING, cx), ["ses_releases", "ses_invoices"]);
    assert_eq!(stand_in.seen_at("GET", "kuvo/setup/sessions/list/events").len(), 1);
    assert!(stand_in.seen_at("GET", "kuvo/sessions/list").is_empty());
    assert!(bounds(cx, "project-Billing").is_some(), "its project is named as on its computer");

    let before = stand_in.seen().len();
    click(cx, "session-project-ses_releases");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_releases".into()));
    // (Read at once, as the app's follows would read it.)
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_releases", window, cx));
    cx.run_until_parked();
    // Its conversation was read on devbox, and nothing of it asked of this computer's box.
    assert!(!devbox.seen_at("GET", "api/session/ses_releases/message").is_empty(), "read where it runs: {:?} / here: {:?}", devbox.seen().iter().map(|s| s.path.clone()).collect::<Vec<_>>(), stand_in.seen()[before..].iter().map(|s| s.path.clone()).collect::<Vec<_>>());
    assert!(stand_in.seen()[before..].iter().all(|seen| !seen.path.contains("ses_releases")), "this computer's box isn't asked of another's session");
}

#[gpui::test]
fn a_computer_switching_to_a_new_release_keeps_its_rows_saying_updating(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    stand_in.setup_away("key_devbox", NOW - 2.0 * MINUTE, true);
    let (workspace, cx) = open(cx, stand_in.clone());
    // Its rows stay, as its last copy has them, saying so: never a "can't reach" in their place.
    assert_eq!(under(&workspace, BILLING, cx), ["ses_releases", "ses_invoices"]);
    assert!(bounds(cx, "session-said-ses_releases-Updating…").is_some(), "says it's updating");
    // Nothing it can't do now is offered: the row doesn't open.
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    let before = devbox.seen().len();
    click(cx, "session-project-ses_releases");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), None);
    assert_eq!(devbox.seen().len(), before, "an away computer isn't asked");
}

#[gpui::test]
fn an_away_computers_rows_say_when_it_was_last_seen(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    stand_in.setup_away("key_devbox", NOW - 3.0 * 60.0 * MINUTE, false);
    let (workspace, cx) = open(cx, stand_in);
    assert_eq!(under(&workspace, BILLING, cx), ["ses_releases", "ses_invoices"]);
    assert!(bounds(cx, "session-said-ses_releases-seen 3h").is_some(), "says when it was last seen");
}
