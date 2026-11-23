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
    // (Through this computer's box, as its follow reads it: the box carries it from devbox.)
    assert!(stand_in.seen()[before..].iter().filter(|seen| seen.path.contains("ses_releases")).all(|seen| seen.path.starts_with("kuvo/sessions/ses_releases/location") || seen.path.starts_with("kuvo/peer-session/key_devbox/ses_releases/")), "this computer's box only carries another's session");
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
    // Switching isn't away (the user, 11-23): the row opens as it always does.
    click(cx, "session-project-ses_releases");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_releases".into()));
}

/// The open conversation, read again as a new view reads it (gone to the landing and back).
fn read_again(workspace: &Entity<Workspace>, session: &str, cx: &mut VisualTestContext) -> Entity<kuvo::conversation::ConversationView> {
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.new_session(None, window, cx);
        workspace.open_read_at_once(session, window, cx);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| workspace.conversation().cloned()).expect("it's open")
}

/// A conversation on a computer that's restarting, updating or off shows its last transcript,
/// readable, with one quiet line, never a page-wide "It may be asleep or offline." (the user,
/// 11-23); its composer still takes messages; once it answers, the transcript is its own again.
#[gpui::test]
fn an_away_computers_conversation_shows_its_last_messages_and_a_quiet_line(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.share_setup_lists();
    stand_in.enable_delivery();
    let devbox = stand_in.peer_box("key_devbox").unwrap();
    let (workspace, cx) = open(cx, stand_in.clone());
    let view = read_again(&workspace, "ses_releases", cx);
    // (Its messages' rows: the way back to earlier ones waits until it answers, and whether it's
    // at work isn't known meanwhile.)
    let rows = |view: &kuvo::conversation::ConversationView| view.entries().iter().filter(|entry| !matches!(entry, kuvo::entries::Entry::Earlier | kuvo::entries::Entry::Working { .. })).map(|entry| entry.key().to_owned()).collect::<Vec<_>>();
    let messages = view.read_with(cx, |view, _| rows(view));
    assert!(!messages.is_empty(), "read while devbox answers");
    assert!(view.read_with(cx, |view, _| view.away().is_none()));

    // devbox restarts and stays off: its last copy, kept by this computer's box, with the line.
    devbox.set_down(true);
    stand_in.setup_away("key_devbox", NOW - 5.0 * MINUTE, false);
    let view = read_again(&workspace, "ses_releases", cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.problem(), None, "no page-wide note");
        assert_eq!(rows(view), messages, "its last transcript, whole");
        let note = view.composer().read(cx).note().map(|note| note.text.to_string());
        assert_eq!(note.as_deref(), Some("devbox isn’t answering · seen 5m"));
    });
    assert!(bounds(cx, "composer-card").is_some(), "the composer is there to write in");

    // Back: its own transcript, and the line goes.
    devbox.set_down(false);
    let view = read_again(&workspace, "ses_releases", cx);
    view.read_with(cx, |view, cx| {
        assert!(view.away().is_none());
        assert_eq!(rows(view), messages);
        assert_eq!(view.composer().read(cx).note().map(|note| note.text.to_string()), None);
    });
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
