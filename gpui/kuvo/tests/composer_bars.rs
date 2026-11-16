//! The composer's mode menu, Local or Worktree for a new session, and the bars over the composer,
//! as someone uses them against the stand-in box (nothing real is reached): picking a mode sets it
//! on the box, Full Access asks once on this computer first; a new session starts in a worktree
//! when picked; and the bars list what the box says runs for a conversation (a bot's sessions, its
//! subagents, the commands it left running), open each, stop each, and go once nothing runs. Clicks
//! go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::area::Area;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_box::models::StartIn;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

/// 2026-11-14, 09:53 UTC.
const NOW: f64 = 1_794_650_000_000.0;
const WEBSITE: &str = "/Users/sam/Projects/website";

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(1200.)));
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn conversation<'a>(cx: &'a mut TestAppContext, stand_in: &StandIn, session: &str) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session_now(session, window, cx));
    cx.run_until_parked();
    (workspace, cx)
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

fn mode_label(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, cx| workspace.conversation().and_then(|view| view.read(cx).composer().read(cx).approval()).map(|shown| shown.approval.title().to_owned()))
}

/// A command left running and a subagent at work, as the box lists them.
fn running(stand_in: &StandIn) {
    stand_in.set_background(
        "ses_signup",
        json!({
            "commands": [{ "id": "sh_dev", "command": "npm run dev", "started": NOW - 95_000.0, "folder": WEBSITE }],
            "subagents": [{ "session": "ses_sub", "since": NOW - 30_000.0, "title": "Check the other forms" }],
        }),
        &[("sh_dev", "VITE v6 ready in 412 ms\n➜  Local: http://localhost:5173/")],
    );
}

#[gpui::test]
fn picking_a_mode_sets_it_on_the_box_and_the_chip_says_the_boxs_answer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_approvals(true, None);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    assert_eq!(mode_label(&workspace, cx).as_deref(), Some("Auto"), "this computer's default, as its box says");
    click(cx, "mode-chip");
    assert!(shown(cx, "menu-item-Ask First") && shown(cx, "menu-item-Accept Edits") && shown(cx, "menu-item-Full Access"));
    click(cx, "menu-item-Accept Edits");
    assert_eq!(stand_in.approval_of("ses_signup").as_deref(), Some("acceptEdits"));
    assert_eq!(mode_label(&workspace, cx).as_deref(), Some("Accept Edits"));
}

#[gpui::test]
fn full_access_asks_once_on_this_computer_and_cancel_leaves_everything_as_it_was(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "mode-chip");
    click(cx, "menu-item-Full Access");
    assert!(shown(cx, "full-access-question"), "asked first, in the composer");
    assert_eq!((stand_in.full_access(), stand_in.approval_of("ses_signup")), (false, None), "nothing changed on the box yet");
    click(cx, "full-access-cancel");
    assert!(!shown(cx, "full-access-question"));
    assert_eq!((stand_in.full_access(), mode_label(&workspace, cx).as_deref()), (false, Some("Auto")));

    click(cx, "mode-chip");
    click(cx, "menu-item-Full Access");
    click(cx, "full-access-allow");
    assert!(stand_in.full_access(), "switched on for this computer");
    assert_eq!(stand_in.approval_of("ses_signup").as_deref(), Some("fullAccess"));
    assert_eq!(mode_label(&workspace, cx).as_deref(), Some("Full Access"));
    // Confirmed here: picking it again doesn't ask.
    click(cx, "mode-chip");
    click(cx, "menu-item-Ask First");
    click(cx, "mode-chip");
    click(cx, "menu-item-Full Access");
    assert!(!shown(cx, "full-access-question"));
    assert_eq!(stand_in.approval_of("ses_signup").as_deref(), Some("fullAccess"));
}

#[gpui::test]
fn a_new_session_starts_in_a_worktree_and_in_the_mode_picked(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_approvals(true, None);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.new_session(None, window, cx);
        workspace.pick_place(StartIn::Project(WEBSITE.into()), cx);
        workspace.landing().clone().update(cx, |composer, cx| composer.read_at_once(cx));
    });
    cx.run_until_parked();
    assert!(shown(cx, "worktree-chip"), "a project's new session offers Local or Worktree");
    click(cx, "worktree-chip");
    click(cx, "menu-item-Worktree");
    click(cx, "mode-chip");
    click(cx, "menu-item-Ask First");
    workspace.update_in(cx, |workspace, window, cx| window.focus(&gpui::Focusable::focus_handle(workspace.landing().read(cx), cx), cx));
    cx.simulate_input("Fix the footer");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let starts = stand_in.starts();
    assert_eq!(starts.last().map(|start| (start["worktree"].clone(), start["approval"].clone())), Some((json!(true), json!("askFirst"))));

    // A chat has no project to make a worktree of: nothing to pick.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.new_session(None, window, cx);
        workspace.pick_place(StartIn::Chat, cx);
    });
    cx.run_until_parked();
    assert!(!shown(cx, "worktree-chip"));
}

#[gpui::test]
fn the_bars_list_what_runs_open_each_and_go_once_nothing_runs(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    assert!(shown(cx, "work-bars") && shown(cx, "bar-background") && shown(cx, "bar-subagents"));
    assert!(shown(cx, "bar-row-sh_dev") && shown(cx, "bar-row-ses_sub"));

    // Output shows what it printed, as the engine has it.
    click(cx, "bar-output-sh_dev");
    let words = workspace.read_with(cx, |workspace, cx| workspace.conversation().and_then(|view| view.read(cx).bars().read(cx).output_words()));
    assert_eq!(words.map(|(text, _)| text), Some("VITE v6 ready in 412 ms\n➜  Local: http://localhost:5173/".to_owned()));
    assert!(shown(cx, "command-output"));
    click(cx, "bar-output-sh_dev");
    assert!(!shown(cx, "command-output"));

    // Folded, a bar keeps its line and what its one is.
    click(cx, "fold-background");
    assert!(shown(cx, "bar-background") && !shown(cx, "bar-row-sh_dev"));
    click(cx, "fold-background");

    // Stop: the box stops it, and its row goes.
    click(cx, "bar-stop-sh_dev");
    assert_eq!(stand_in.stopped_commands(), vec!["sh_dev".to_owned()]);
    assert!(!shown(cx, "bar-background") && shown(cx, "bar-subagents"));

    // Open: the subagent's own conversation.
    click(cx, "bar-open-ses_sub");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_sub".into()));

    // Back to the conversation; once nothing runs, the card goes.
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session_now("ses_signup", window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "work-bars"));
    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [] }), &[]);
    workspace.update_in(cx, |workspace, _, cx| {
        if let Some(view) = workspace.conversation() {
            view.read(cx).bars().clone().update(cx, |bars, cx| bars.read_at_once(cx));
        }
    });
    cx.run_until_parked();
    assert!(!shown(cx, "work-bars"));
}

#[gpui::test]
fn a_subagent_waiting_on_you_says_so(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [{ "session": "ses_sub", "since": NOW, "title": "Check the other forms", "asking": true }] }), &[]);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    assert!(shown(cx, "bar-subagents") && !shown(cx, "bar-background"));
    let subagents = workspace.read_with(cx, |workspace, cx| workspace.conversation().map(|view| view.read(cx).bars().read(cx).background().subagents.clone()).unwrap_or_default());
    assert!(subagents[0].asking);
}

#[gpui::test]
fn a_bots_conversation_shows_its_sessions_at_work_and_opens_each(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    workspace.update_in(cx, |workspace, window, cx| workspace.open_bot_chat("bot_mira", "ses_mira", None, window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "bar-work"), "its sessions at work, over its composer");
    let at_work = workspace.read_with(cx, |workspace, cx| workspace.bot_chat().map(|(_, view)| view.read(cx).bars().read(cx).at_work().iter().map(|session| session.session.clone()).collect::<Vec<_>>()).unwrap_or_default());
    assert_eq!(at_work, vec!["ses_signup".to_owned(), "ses_cms".to_owned()], "working and waiting on you, as its box says");
    assert!(shown(cx, "bar-row-ses_signup") && shown(cx, "bar-row-ses_cms"));
    let about = workspace.read_with(cx, |workspace, cx| workspace.bot_chat().and_then(|(_, view)| view.read(cx).bars().read(cx).about_of("ses_signup")));
    assert_eq!(about.as_deref(), Some("Claude Opus 5.5 · High · Claude · website"), "its model by name, its effort, the account and its project");
    click(cx, "bar-open-ses_cms");
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.opened().map(str::to_owned))), (Area::Sessions, Some("ses_cms".into())));
}
