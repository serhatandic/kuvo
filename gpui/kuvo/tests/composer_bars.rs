//! The composer’s mode menu, Local or Worktree for a new session, and the conversation’s Activity,
//! as someone uses them against the stand-in box (nothing real is reached): picking a mode sets it
//! on the box, Full Access asks once on this computer first; a new session starts in a worktree
//! when picked; and Activity lists what the box says runs for a conversation (a bot's sessions, its
//! subagents, the commands it left running), open each, and offer controls to stop work. Clicks
//! go through the window as AppKit hands them on.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
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
fn activity_shows_sections_and_opens_a_process_and_subagent(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    assert!(!shown(cx, "work-bars"));
    assert!(shown(cx, "conversation-activity-running"));
    click(cx, "conversation-activity-button");
    let trigger = bounds(cx, "conversation-activity-button").unwrap();
    let popover = bounds(cx, "conversation-activity-popover").unwrap();
    assert!(popover.top() >= trigger.bottom());
    assert!(shown(cx, "No outputs yet") && shown(cx, "activity-sessions") && shown(cx, "activity-processes"));
    assert!(!shown(cx, "activity-wakes") && !shown(cx, "activity-session-ses_sub"));

    click(cx, "activity-process-sh_dev");
    assert!(shown(cx, "command-output"));
    assert_eq!(stand_in.seen_at("GET", "api/shell/sh_dev/output").len(), 2);
    click(cx, "activity-process-sh_dev");
    assert!(!shown(cx, "command-output"));
    click(cx, "activity-process-sh_dev");
    click(cx, "activity-stop-process");
    assert_eq!(stand_in.stopped_commands(), vec!["sh_dev".to_owned()]);
    assert!(!shown(cx, "activity-processes") && shown(cx, "activity-sessions"));

    click(cx, "activity-toggle-sessions");
    click(cx, "activity-session-ses_sub");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_sub".into()));
    assert!(!shown(cx, "conversation-activity-popover"));
}

#[gpui::test]
fn empty_activity_opens_with_keyboard_and_escape_restores_focus(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_recipes");
    let activity = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).bars().clone());
    let focus = activity.read_with(cx, |activity, cx| activity.focus_handle(cx));
    cx.update(|window, cx| window.focus(&focus, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse("enter").unwrap() });
    cx.run_until_parked();
    assert!(shown(cx, "No outputs yet"));
    for hidden in ["activity-sessions", "activity-processes", "activity-wakes", "conversation-activity-running"] {
        assert!(!shown(cx, hidden), "{hidden}");
    }
    assert!(!cx.update(|window, _| focus.is_focused(window)));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "conversation-activity-popover"));
    assert!(cx.update(|window, _| focus.is_focused(window)));
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "conversation-activity-popover"));
    click(cx, "conversation-activity-button");
    assert!(!shown(cx, "conversation-activity-popover"));
}

#[gpui::test]
fn activity_lists_this_conversations_outputs_and_opens_the_recorded_version(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    for (session, id, name) in [("ses_signup", "art_000000000000000000000001", "report.md"), ("ses_recipes", "art_000000000000000000000002", "recipes.md")] {
        stand_in.add_output(session, json!({
            "id": id, "path": format!("/tmp/{name}"), "name": name, "kind": "document",
            "type": "text/markdown", "latest": { "n": 3 },
        }), b"# Report");
    }
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    assert!(!shown(cx, "No outputs yet") && !shown(cx, "activity-output-art_000000000000000000000002"));
    click(cx, "activity-output-art_000000000000000000000001");
    assert!(shown(cx, "activity-output-preview"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/artifacts/art_000000000000000000000001/versions/3").len(), 1);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "conversation-activity-popover"));
}

#[gpui::test]
fn a_subagent_waiting_on_you_is_discoverable_without_a_running_dot(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [{ "session": "ses_sub", "since": NOW, "title": "Check the other forms", "asking": true }] }), &[]);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    assert!(!shown(cx, "conversation-activity-running"));
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-sessions") && !shown(cx, "activity-processes"));
    click(cx, "activity-toggle-sessions");
    assert!(shown(cx, "activity-session-ses_sub"));
    let view = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).bars().clone());
    assert!(view.read_with(cx, |bars, _| bars.background().subagents[0].asking));

    stand_in.set_background("ses_signup", json!({ "commands": [{ "id": "sh_1", "command": "sleep 180", "started": NOW }], "subagents": [] }), &[]);
    view.update(cx, |bars, cx| bars.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "conversation-activity-running") && shown(cx, "activity-processes") && !shown(cx, "activity-sessions"));

    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [] }), &[]);
    view.update(cx, |bars, cx| bars.read_at_once(cx));
    cx.run_until_parked();
    assert!(!shown(cx, "conversation-activity-running") && !shown(cx, "activity-processes"));
}

#[gpui::test]
fn activity_stacks_three_provider_faces_and_shows_each_face_when_expanded(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let sessions = [("ses_sub", "codex-cli", "codex"), ("ses_sub2", "claude-code", "claude"),
        ("ses_sub3", "codex-cli", "codex"), ("ses_sub4", "claude-code", "claude")];
    for (id, provider, _) in sessions {
        stand_in.set_model_state(id, json!({ "model": { "providerID": provider, "id": "example" }, "runs": true }));
    }
    stand_in.set_background("ses_signup", json!({
        "subagents": sessions.iter().map(|(id, _, _)| json!({ "session": id, "since": NOW })).collect::<Vec<_>>(),
    }), &[]);
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    let first = bounds(cx, "activity-stack-avatar-ses_sub-codex").unwrap();
    let second = bounds(cx, "activity-stack-avatar-ses_sub2-claude").unwrap();
    assert_eq!(first.size, size(px(20.), px(20.)));
    assert_eq!(first.right() - second.left(), px(6.));
    assert!(shown(cx, "activity-stack-avatar-ses_sub3-codex"));
    assert!(!shown(cx, "activity-stack-avatar-ses_sub4-claude"));
    assert!(shown(cx, "activity-avatar-overflow"));
    click(cx, "activity-toggle-sessions");
    for (id, _, service) in sessions {
        assert!(shown(cx, &format!("activity-row-avatar-{id}-{service}")));
    }
}

#[gpui::test]
fn a_bots_conversation_shows_its_sessions_and_wake_schedule(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_bot_wakes("bot_mira", vec![json!({
        "id": "build", "due": NOW / 1000. + 180., "summary": "Review the build", "cancelKey": "build"
    })], vec![]);
    let (workspace, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    click(cx, "bot-bot_mira");
    assert!(shown(cx, "conversation-activity-running"));
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-sessions") && shown(cx, "activity-wakes") && shown(cx, "wake-build"));
    assert!(shown(cx, "activity-stack-avatar-ses_signup-bot-bot_mira"));
    assert!(!shown(cx, "activity-session-ses_cms"));
    click(cx, "activity-toggle-sessions");
    assert!(shown(cx, "activity-row-avatar-ses_signup-bot-bot_mira"));
    click(cx, "activity-session-ses_cms");
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.opened().map(str::to_owned))), (Area::Sessions, Some("ses_cms".into())));
}
