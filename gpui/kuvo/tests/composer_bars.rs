//! The composer’s permissions (in the model picker, #975), Local or Worktree for a new session, and the conversation’s Activity,
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

/// Opens the permissions, in the model picker over the composer.
fn permissions(cx: &mut VisualTestContext) {
    if !shown(cx, "model-picker") {
        click(cx, "model-chip");
    }
    click(cx, "model-permissions");
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
    permissions(cx);
    assert!(shown(cx, "model-permission-askFirst") && shown(cx, "model-permission-acceptEdits") && shown(cx, "model-permission-fullAccess"));
    click(cx, "model-permission-acceptEdits");
    assert_eq!(stand_in.approval_of("ses_signup").as_deref(), Some("acceptEdits"));
    assert_eq!(mode_label(&workspace, cx).as_deref(), Some("Accept Edits"));
}

#[gpui::test]
fn full_access_asks_once_on_this_computer_and_cancel_leaves_everything_as_it_was(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    permissions(cx);
    click(cx, "model-permission-fullAccess");
    assert!(shown(cx, "full-access-question"), "asked first, in the composer");
    assert_eq!((stand_in.full_access(), stand_in.approval_of("ses_signup")), (false, None), "nothing changed on the box yet");
    click(cx, "full-access-cancel");
    assert!(!shown(cx, "full-access-question"));
    assert_eq!((stand_in.full_access(), mode_label(&workspace, cx).as_deref()), (false, Some("Auto")));

    permissions(cx);
    click(cx, "model-permission-fullAccess");
    click(cx, "full-access-allow");
    assert!(stand_in.full_access(), "switched on for this computer");
    assert_eq!(stand_in.approval_of("ses_signup").as_deref(), Some("fullAccess"));
    assert_eq!(mode_label(&workspace, cx).as_deref(), Some("Full Access"));
    // Confirmed here: picking it again doesn't ask.
    permissions(cx);
    click(cx, "model-permission-askFirst");
    permissions(cx);
    click(cx, "model-permission-fullAccess");
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
    permissions(cx);
    click(cx, "model-permission-askFirst");
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
    // Codex Quiet omits empty sections instead of showing an "Outputs" heading over an empty body.
    assert!(!shown(cx, "activity-empty") && !shown(cx, "activity-outputs"));
    assert!(shown(cx, "activity-subagents") && shown(cx, "activity-processes"));
    // Its subagent is counted in the Subagents row, not listed as a session of its own.
    assert!(!shown(cx, "activity-wakes") && !shown(cx, "activity-sessions") && !shown(cx, "activity-session-ses_sub"));

    // A process opens what it prints in a tab of its own in the side panel, as the box reads it.
    click(cx, "activity-process-sh_dev");
    assert!(!shown(cx, "conversation-activity-popover"));
    let showing = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).active().map(|tab| tab.kind.clone()));
    assert_eq!(showing, Some(kuvo::panel::Kind::Process { session: "ses_signup".into(), command: "sh_dev".into() }));
    assert!(shown(cx, "process-text"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/work/background/ses_signup/commands/sh_dev/output").len(), 1);
    // Stopped there, it says it ended, its last output kept.
    click(cx, "process-stop");
    assert_eq!(stand_in.stopped_commands(), vec!["sh_dev".to_owned()]);
    assert!(shown(cx, "process-text") && shown(cx, "process-note"));
    // (The bars read again, as their follow would hand them what the box says.)
    let bars = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).bars().clone());
    bars.update(cx, |bars, cx| bars.read_at_once(cx));
    cx.run_until_parked();
    click(cx, "conversation-activity-button");
    assert!(!shown(cx, "activity-processes") && shown(cx, "activity-subagents"));

    // The Subagents row opens the side panel's Subagents tab, the conversation staying.
    click(cx, "activity-subagents-row");
    let showing = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).active().map(|tab| tab.kind.clone()));
    assert_eq!(showing, Some(kuvo::panel::Kind::Subagents));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_signup".into()), "the conversation stays");
    assert!(!shown(cx, "conversation-activity-popover") && shown(cx, "subagent-ses_sub"));
}

/// A coding agent's command its turn waits on (a long script run in the foreground), as the box
/// lists it while it runs: under Processes, with what it printed on click, and Stop there.
#[gpui::test]
fn a_command_running_in_the_foreground_is_listed_under_processes_and_stops_there(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_background(
        "ses_signup",
        json!({ "commands": [{ "id": "shl_triage", "command": "bash scripts/triage.sh --all", "started": NOW - 200_000.0, "agents": true }], "subagents": [] }),
        &[("shl_triage", "checked 12 of 40 reports")],
    );
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    // Not "Background processes": it runs in the foreground.
    assert!(shown(cx, "activity-processes-title-Processes"), "the section is Processes");
    assert!(shown(cx, "activity-process-shl_triage"));
    click(cx, "activity-process-shl_triage");
    let showing = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).active().map(|tab| tab.kind.clone()));
    assert_eq!(showing, Some(kuvo::panel::Kind::Process { session: "ses_signup".into(), command: "shl_triage".into() }));
    assert_eq!(stand_in.seen_at("GET", "kuvo/work/background/ses_signup/commands/shl_triage/output").len(), 1);
    click(cx, "process-stop");
    assert_eq!(stand_in.stopped_commands(), vec!["shl_triage".to_owned()]);
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
    // One calm note, never section headings over nothing.
    assert!(shown(cx, "activity-empty"));
    for hidden in ["activity-outputs", "activity-sessions", "activity-subagents", "activity-side-chats", "activity-processes", "activity-wakes", "conversation-activity-running"] {
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
    assert!(!shown(cx, "activity-empty") && !shown(cx, "activity-output-art_000000000000000000000002"));
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
    assert!(shown(cx, "activity-subagents") && !shown(cx, "activity-processes"));
    assert!(shown(cx, "activity-subagents-row"));
    let view = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).bars().clone());
    assert!(view.read_with(cx, |bars, _| bars.background().subagents[0].asking));

    stand_in.set_background("ses_signup", json!({ "commands": [{ "id": "sh_1", "command": "sleep 180", "started": NOW }], "subagents": [] }), &[]);
    view.update(cx, |bars, cx| bars.read_at_once(cx));
    cx.run_until_parked();
    // (Opened again: a popover read once, as tests read, keeps the subagents it read then.)
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "conversation-activity-running") && shown(cx, "activity-processes") && !shown(cx, "activity-subagents"));

    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [] }), &[]);
    view.update(cx, |bars, cx| bars.read_at_once(cx));
    cx.run_until_parked();
    assert!(!shown(cx, "conversation-activity-running") && !shown(cx, "activity-processes"));
}

/// Its subagents in one row, as Codex's: each model's mark once, how many work and are done; no
/// row for each, and no disc behind the marks.
#[gpui::test]
fn subagents_show_in_one_row_with_their_marks_and_counts(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_background("ses_signup", json!({
        "subagents": [
            { "session": "ses_sub", "since": NOW, "title": "Check the forms", "model": "GPT-5.4" },
            { "session": "ses_sub2", "since": NOW, "title": "Read the logs", "model": "Claude Sonnet 4.6" },
        ],
    }), &[]);
    stand_in.set_subagents_done("ses_signup", (0..6).map(|ix| json!({
        "session": format!("ses_done{ix}"), "title": format!("Done {ix}"), "started": NOW - 9_000., "ended": NOW - 1_000.,
        "state": "finished", "model": if ix % 2 == 0 { "Claude Sonnet 4.6" } else { "GPT-5.4" },
    })).collect());
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-subagents-row"));
    assert!(shown(cx, "activity-subagents-mark-codex") && shown(cx, "activity-subagents-mark-claude"));
    assert!(!shown(cx, "activity-session-ses_sub") && !shown(cx, "activity-session-ses_done0"));
    assert!(shown(cx, "activity-subagents-label-2 working · 6 done"));
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
    assert!(shown(cx, "wake-group-wakeups") && !shown(cx, "wake-group-watches"));
    // Bot-started sessions show their own agent/provider mark, not the bot's face.
    assert!(shown(cx, "activity-row-avatar-ses_signup-claude"));
    assert!(!shown(cx, "activity-row-avatar-ses_signup-bot-bot_mira"));
    assert!(shown(cx, "activity-finished-icon-check"));
    assert!(shown(cx, "activity-session-ses_cms"));
    click(cx, "activity-session-ses_cms");
    assert_eq!(workspace.read_with(cx, |workspace, _| (workspace.area(), workspace.opened().map(str::to_owned))), (Area::Sessions, Some("ses_cms".into())));
}

#[gpui::test]
fn bot_sessions_show_provider_icons_check_for_finished_and_fit_stop_all(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_bot_work(
        "bot_mira",
        vec![
            json!({
                "session": "ses_signup", "title": "Fix the sign-up form",
                "folder": WEBSITE, "project": WEBSITE, "startedBy": "bot_mira",
                "depth": 1, "state": "working", "started": NOW - 60_000., "tokens": 12_000,
                "model": { "providerID": "claude-code@work", "id": "claude-opus-4-6" }
            }),
            json!({
                "session": "ses_cms", "title": "Move the blog to the new CMS",
                "folder": WEBSITE, "project": WEBSITE, "startedBy": "bot_mira",
                "depth": 1, "state": "asking", "started": NOW - 120_000., "tokens": 18_000,
                "asks": "Wants to run a command",
                "model": { "providerID": "codex-cli", "id": "gpt-5.4" }
            }),
            json!({
                "session": "ses_gem", "title": "Audit onboarding spacing",
                "folder": WEBSITE, "project": WEBSITE, "startedBy": "bot_mira",
                "depth": 1, "state": "working", "started": NOW - 30_000., "tokens": 9_000,
                "model": { "providerID": "google", "id": "gemini-3-pro" }
            }),
            json!({
                "session": "ses_ag", "title": "Verify antigravity flow",
                "folder": WEBSITE, "project": WEBSITE, "startedBy": "bot_mira",
                "depth": 1, "state": "working", "started": NOW - 20_000., "tokens": 6_000,
                "model": { "providerID": "antigravity", "id": "gemini-3.1-pro" }
            }),
            json!({
                "session": "ses_grok", "title": "Check rate limits",
                "folder": WEBSITE, "project": WEBSITE, "startedBy": "bot_mira",
                "depth": 1, "state": "working", "started": NOW - 10_000., "tokens": 4_000,
                "model": { "providerID": "auto", "id": "grok-4" }
            }),
            json!({
                "session": "ses_dark", "title": "Dark mode for settings",
                "folder": WEBSITE, "project": WEBSITE, "startedBy": "bot_mira",
                "depth": 1, "state": "finished", "started": NOW - 3_600_000., "tokens": 22_000,
                "model": { "providerID": "codex-cli", "id": "gpt-5.4" }
            }),
        ],
    );
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    click(cx, "bot-bot_mira");
    click(cx, "conversation-activity-button");

    for (session, service) in [
        ("ses_signup", "claude"),
        ("ses_cms", "codex"),
        ("ses_gem", "google"),
        ("ses_ag", "antigravity"),
        ("ses_grok", "grok"),
    ] {
        assert!(
            shown(cx, &format!("activity-row-avatar-{session}-{service}")),
            "expected {service} icon for {session}"
        );
        assert!(
            !shown(cx, &format!("activity-row-avatar-{session}-bot-bot_mira")),
            "did not expect bot face for {session}"
        );
    }

    assert!(shown(cx, "activity-finished-sessions"));
    assert!(shown(cx, "activity-finished-icon-check"));

    // Pinned, each session at work keeps its provider's mark, as in the popover.
    click(cx, "activity-pin");
    if shown(cx, "activity-fold-pinned-sessions") && !shown(cx, "activity-pinned-session-ses_signup") {
        click(cx, "activity-fold-pinned-sessions");
    }
    for (session, service) in [("ses_signup", "claude"), ("ses_cms", "codex"), ("ses_grok", "grok")] {
        if shown(cx, &format!("activity-pinned-session-{session}")) {
            assert!(shown(cx, &format!("activity-pinned-avatar-{session}-{service}")), "expected {service} mark pinned for {session}");
        }
    }
    assert!(shown(cx, "activity-pinned-avatar-ses_grok-grok"), "a pinned row shows its provider's mark, not a dot");
    click(cx, "activity-unpin");
    click(cx, "conversation-activity-button");

    let popover = bounds(cx, "conversation-activity-popover").expect("activity popover");
    let header = bounds(cx, "activity-sessions-header").expect("sessions header");
    let stop_all = bounds(cx, "activity-stop-all").expect("Stop All button");
    assert!(
        stop_all.right() <= header.right() && stop_all.right() <= popover.right(),
        "Stop All must not clip past header or popover: stop_all={stop_all:?} header={header:?} popover={popover:?}"
    );
    assert!(
        stop_all.size.width >= px(52.),
        "Stop All should keep its full label width without collapsing: {stop_all:?}"
    );
}

#[gpui::test]
fn stop_all_keeps_recent_sessions_openable_after_the_last_one_ends(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone());
    click(cx, "rail-bots");
    click(cx, "bot-bot_mira");
    click(cx, "conversation-activity-button");
    // Live sessions show inline; finished sessions collapse into one disclosure row.
    assert!(shown(cx, "activity-stop-all") && shown(cx, "activity-finished-sessions"));
    assert!(!shown(cx, "activity-session-ses_dark"));
    click(cx, "activity-stop-all");
    assert_eq!(stand_in.seen_at("POST", "kuvo/bots/bot_mira/stop-all").len(), 1);
    assert!(!shown(cx, "conversation-activity-running") && !shown(cx, "activity-stop-all"));
    assert!(shown(cx, "activity-finished-sessions") && !shown(cx, "activity-session-ses_signup"));
    click(cx, "activity-finished-sessions");
    assert!(shown(cx, "activity-finished-view") && shown(cx, "activity-session-ses_signup") && shown(cx, "activity-session-ses_dark"));
    click(cx, "activity-session-ses_dark");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).as_deref(), Some("ses_dark"));
}

#[gpui::test]
fn a_git_backed_output_labels_the_current_file(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_output("ses_signup", json!({
        "id": "art_000000000000000000000001",
        "path": format!("{WEBSITE}/src/components/SignupForm.tsx"),
        "name": "SignupForm.tsx", "kind": "code", "type": "text/plain",
        "latest": { "n": 3, "reason": "git" },
    }), &[]);
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    click(cx, "activity-output-art_000000000000000000000001");
    assert!(shown(cx, "activity-current-file"));
    assert_eq!(stand_in.seen_at("GET", "kuvo/file/text").len(), 1);
}

#[gpui::test]
fn activity_outputs_media_strip_recent_files_and_see_all_stays_bounded(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    // 1x1 PNG bytes so thumbnail decoding succeeds via the existing artifact_bytes route.
    let png = {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 122, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    for i in 1..=5 {
        stand_in.add_output("ses_signup", json!({
            "id": format!("art_img_{i:024}"),
            "path": format!("/tmp/shot-{i}.png"),
            "name": format!("shot-{i}.png"),
            "kind": "image", "type": "image/png",
            "latest": { "n": 1 },
        }), &png);
    }
    for i in 1..=5 {
        stand_in.add_output("ses_signup", json!({
            "id": format!("art_doc_{i:024}"),
            "path": format!("/tmp/report-{i}.md"),
            "name": format!("report-{i}.md"),
            "kind": "document", "type": "text/markdown",
            "latest": { "n": 1 },
        }), b"# Report");
    }

    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");

    let trigger = bounds(cx, "conversation-activity-button").unwrap();
    let popover = bounds(cx, "conversation-activity-popover").unwrap();
    assert!(popover.top() >= trigger.bottom(), "popover must not cover its own button");
    assert!(popover.size.height <= px(480.), "popover height must stay bounded, got {:?}", popover.size.height);

    // Media strip: first 3 images + "+N images" tile; remaining images hidden in overview.
    assert!(shown(cx, "activity-media-strip"));
    for i in 1..=3 {
        assert!(shown(cx, &format!("activity-output-art_img_{i:024}")));
    }
    assert!(!shown(cx, &format!("activity-output-art_img_{:024}", 4)));
    assert!(shown(cx, "activity-more-images"));

    // Recent files: first 3 non-image files + "+N more files" row; remaining files hidden in overview.
    for i in 1..=3 {
        assert!(shown(cx, &format!("activity-output-art_doc_{i:024}")));
    }
    assert!(!shown(cx, &format!("activity-output-art_doc_{:024}", 4)));
    assert!(shown(cx, "activity-more-files"));
    assert!(shown(cx, "activity-see-all-outputs"));

    // Only the 3 visible images should have fetched thumbnail bytes.
    assert_eq!(stand_in.seen_at("GET", &format!("kuvo/artifacts/art_img_{:024}/versions/1", 1)).len(), 1);
    assert_eq!(stand_in.seen_at("GET", &format!("kuvo/artifacts/art_img_{:024}/versions/1", 4)).len(), 0);

    // Clicking "See all" opens the scrollable all-outputs view showing every item, and Back returns.
    click(cx, "activity-see-all-outputs");
    assert!(shown(cx, "activity-all-outputs"));
    assert!(shown(cx, &format!("activity-output-art_img_{:024}", 5)));
    assert!(shown(cx, &format!("activity-output-art_doc_{:024}", 5)));
    click(cx, "activity-outputs-back");
    assert!(!shown(cx, "activity-all-outputs"));
    assert!(shown(cx, "activity-media-strip"));
}

#[gpui::test]
fn activity_outputs_media_strip_keeps_shared_prefix_captions_distinct_with_full_name_tooltip(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let png = {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 122, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let names = [
        ("art_img_000000000000000000000001", "screens-insights-overview-light.png", "insights-light"),
        ("art_img_000000000000000000000002", "screens-insights-overview-dark.png", "insights-dark"),
        ("art_img_000000000000000000000003", "screens-computer-pairing-waiting-light.png", "computer-light"),
    ];
    for (id, name, _) in names {
        stand_in.add_output("ses_signup", json!({
            "id": id,
            "path": format!("/tmp/{name}"),
            "name": name,
            "kind": "image", "type": "image/png",
            "latest": { "n": 1 },
        }), &png);
    }

    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-media-strip"));

    for (_, _, expected_caption) in names {
        assert!(
            shown(cx, &format!("activity-output-caption-{expected_caption}")),
            "expected readable distinct caption {expected_caption}"
        );
    }

    // Hovering a thumbnail shows the full filename in a tooltip.
    let first_tile = bounds(cx, "activity-output-art_img_000000000000000000000001").unwrap();
    cx.simulate_mouse_move(first_tile.center(), None, gpui::Modifiers::default());
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(shown(cx, "tooltip-screens-insights-overview-light.png"));
}

#[gpui::test]
fn activity_more_images_and_docs_filter_to_their_kind_and_images_open_stage_viewer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let png = {
        let img = image::RgbImage::from_pixel(1280, 640, image::Rgb([32, 140, 240]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    for i in 1..=5 {
        stand_in.add_output("ses_signup", json!({
            "id": format!("art_img_{i:024}"),
            "path": format!("/tmp/chart-{i}.png"),
            "name": format!("chart-{i}.png"),
            "kind": "image", "type": "image/png",
            "latest": { "n": 1 },
        }), &png);
    }
    for i in 1..=5 {
        stand_in.add_output("ses_signup", json!({
            "id": format!("art_doc_{i:024}"),
            "path": format!("/tmp/notes-{i}.md"),
            "name": format!("notes-{i}.md"),
            "kind": "document", "type": "text/markdown",
            "latest": { "n": 1 },
        }), b"# Notes");
    }
    stand_in.add_output("ses_signup", json!({
        "id": "art_csv_000000000000000000000001",
        "path": "/tmp/metrics.csv",
        "name": "metrics.csv",
        "kind": "data", "type": "text/csv",
        "latest": { "n": 1 },
    }), b"a,b\n1,2\n");

    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");

    // "+2 images" opens the filtered Images grid (all 5 images, zero markdown or CSV files).
    click(cx, "activity-more-images");
    assert!(shown(cx, "activity-all-outputs") && shown(cx, "activity-images-grid"));
    assert!(shown(cx, "activity-output-art_img_000000000000000000000005"));
    assert!(!shown(cx, "activity-output-art_doc_000000000000000000000001"));
    assert!(!shown(cx, "activity-output-art_csv_000000000000000000000001"));

    // Clicking an image in the filtered grid opens the unified Stage viewer and closes the popover.
    click(cx, "activity-output-art_img_000000000000000000000002");
    assert!(!shown(cx, "conversation-activity-popover"));
    assert!(shown(cx, "picture-viewer") && shown(cx, "picture-next") && shown(cx, "picture-prev"));
    assert!(shown(cx, "picture-dimensions-1280x640"), "Stage shows the original instead of the resized thumbnail");
    click(cx, "picture-next");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "picture-viewer"));

    // Reopen popover: "+N docs" pill opens Docs ONLY (no images, no CSV).
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-more-docs"));
    click(cx, "activity-more-docs");
    assert!(shown(cx, "activity-all-outputs") && !shown(cx, "activity-images-grid"));
    assert!(shown(cx, "activity-output-art_doc_000000000000000000000005"));
    assert!(!shown(cx, "activity-output-art_img_000000000000000000000001"));
    assert!(!shown(cx, "activity-output-art_csv_000000000000000000000001"));

    // Back to overview and clicking a thumbnail directly opens Stage too.
    click(cx, "activity-outputs-back");
    click(cx, "activity-output-art_img_000000000000000000000001");
    assert!(shown(cx, "picture-viewer"));
    click(cx, "picture-close");
    assert!(!shown(cx, "picture-viewer"));
}

#[gpui::test]
fn activity_wakes_renders_distinct_wakeups_and_watches_subgroups(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_bot_wakes(
        "bot_mira",
        vec![json!({
            "id": "wk_1", "due": NOW / 1000. + 600., "summary": "Send morning briefing", "cancelKey": "wk_1"
        })],
        vec![json!({
            "letters": "ci", "next": NOW / 1000. + 180., "every": "5m", "summary": "Check CI status"
        })],
    );
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    click(cx, "bot-bot_mira");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-wakes"));
    assert!(shown(cx, "wake-group-wakeups") && shown(cx, "wake-wk_1"));
    assert!(shown(cx, "wake-group-watches") && shown(cx, "watch-ci"));
}

#[gpui::test]
fn stage_viewer_opens_from_composer_attachments_and_side_panel_images(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let png = {
        let img = image::RgbImage::from_pixel(12, 12, image::Rgb([220, 80, 60]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let tmp = std::env::temp_dir().join("kuvo-stage-attach-test.png");
    std::fs::write(&tmp, &png).unwrap();

    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");

    // 1. Composer attachment thumbnail opens Stage viewer.
    workspace.update_in(cx, |workspace, _, cx| {
        let composer = workspace.conversation().unwrap().read(cx).composer().clone();
        composer.update(cx, |composer, cx| composer.attach([tmp.clone()], cx));
    });
    cx.run_until_parked();
    assert!(shown(cx, "attached-picture-0"));
    click(cx, "attached-picture-0");
    assert!(shown(cx, "picture-viewer"));
    click(cx, "picture-close");
    assert!(!shown(cx, "picture-viewer"));
    let _ = std::fs::remove_file(&tmp);

    // 2. Files side panel image opens Stage viewer via "View Image".
    cx.simulate_keystrokes("cmd-alt-f");
    cx.run_until_parked();
    click(cx, &format!("tree-{WEBSITE}/public"));
    click(cx, &format!("tree-{WEBSITE}/public/favicon.png"));
    assert!(shown(cx, "files-open-image"));
    click(cx, "files-open-image");
    assert!(shown(cx, "picture-viewer"));
    click(cx, "picture-close");
    assert!(!shown(cx, "picture-viewer"));

    // 3. Changes side panel binary image opens Stage viewer via "View Image".
    stand_in.set_git_files(
        WEBSITE,
        vec![json!({ "path": "public/favicon.png", "group": "unstaged", "status": "modified", "scope": "unstaged" })],
    );
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    let changes = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).changes().unwrap());
    changes.update(cx, |view, cx| view.take(client.git_files(WEBSITE), cx));
    cx.run_until_parked();
    // (The first file shows open, under its header.)
    assert!(shown(cx, "changes-open-image"));
    click(cx, "changes-open-image");
    assert!(shown(cx, "picture-viewer"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "picture-viewer"));
}

/// The Stage viewer covers the whole window, title bar included: its name pill is centred, clear of
/// the window's own buttons at the top left (the user's beta.37: they sat over "Outputs"), and Esc
/// still closes it, with no key label beside its close button.
#[gpui::test]
fn stage_viewer_leaves_room_for_the_window_buttons_and_esc_closes_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let png = {
        let img = image::RgbImage::from_pixel(12, 12, image::Rgb([60, 80, 220]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("stage-inset.png");
    std::fs::write(&path, &png).unwrap();
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    workspace.update_in(cx, |workspace, _, cx| {
        let composer = workspace.conversation().unwrap().read(cx).composer().clone();
        composer.update(cx, |composer, cx| composer.attach([path.clone()], cx));
    });
    cx.run_until_parked();
    click(cx, "attached-picture-0");
    let pill = bounds(cx, "picture-identity-pill").expect("the viewer's name pill");
    assert!(pill.origin.x >= px(kuvo::title_bar::LIGHTS), "starts at {:?}, under the window's buttons", pill.origin.x);
    // Centred over the picture, as Preview titles it (#948): its middle is the window's.
    let middle = pill.origin.x + pill.size.width / 2.;
    assert!((f32::from(middle) - 640.).abs() <= 1., "its middle is at {middle:?}, not the window's");
    let close = bounds(cx, "picture-close").expect("its close button");
    assert_eq!(close.size.width, close.size.height, "a plain round close button, no Esc label");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!shown(cx, "picture-viewer"));
}

/// The Outputs popover scrolls as one (the user's beta.37: its Wake-ups & Watches list scrolled
/// inside it, the two fighting): a long schedule shows a few rows of each, then Show All.
#[gpui::test]
fn the_outputs_popover_scrolls_as_one_with_a_long_schedule_folded(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let wakes = (0..6).map(|ix| json!({ "id": format!("wk_{ix}"), "due": NOW / 1000. + 600. * (ix as f64 + 1.), "summary": format!("Briefing {ix}"), "cancelKey": format!("wk_{ix}") })).collect();
    stand_in.set_bot_wakes("bot_mira", wakes, vec![json!({ "letters": "ci", "next": NOW / 1000. + 180., "every": "5m", "summary": "Check CI status" })]);
    let (_, cx) = open(cx, stand_in);
    click(cx, "rail-bots");
    click(cx, "bot-bot_mira");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "wake-wk_0") && shown(cx, "wake-wk_2") && !shown(cx, "wake-wk_3"), "the first few");
    assert!(shown(cx, "watch-ci"));
    let entries = bounds(cx, "wake-entries").expect("the schedule");
    let last = bounds(cx, "watch-ci").expect("its last row");
    assert!(last.origin.y + last.size.height <= entries.origin.y + entries.size.height + px(1.), "laid out whole, not clipped by a scroll of its own");
    click(cx, "wake-show-all");
    assert!(shown(cx, "wake-wk_5") && !shown(cx, "wake-show-all"));
}

/// Each section folds from its heading, as Codex's do: its rows go, its heading and counts stay, and
/// it stays folded when the popover opens again, until unfolded.
#[gpui::test]
fn a_section_folds_from_its_heading_and_stays_folded(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-process-sh_dev") && shown(cx, "activity-subagents-row"));
    click(cx, "activity-fold-processes");
    assert!(shown(cx, "activity-processes-header") && !shown(cx, "activity-process-sh_dev"));
    assert!(shown(cx, "activity-subagents-row"), "the others stay open");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-processes-header") && !shown(cx, "activity-process-sh_dev"), "still folded");
    click(cx, "activity-fold-processes");
    assert!(shown(cx, "activity-process-sh_dev"));
}

/// Activity says how full the conversation's context is, in its box's numbers: used of the model's
/// window, a mark where the box's "Summarize long conversations at" setting summarizes it, and,
/// near there, that it will.
#[gpui::test]
fn activity_shows_the_boxs_context_with_its_summarize_mark(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_context("ses_signup", json!({ "used": 382_000, "window": 1_000_000, "summarizeAt": 400_000 }));
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-context-words-382k of 1M"));
    let bar = bounds(cx, "activity-context-bar").unwrap();
    let tick = bounds(cx, "activity-context-tick").unwrap();
    let at = (tick.left() - bar.left()) / bar.size.width;
    assert!((at - 0.4).abs() < 0.01, "the mark sits at 400k of 1M, not {at}");
    assert!(shown(cx, "activity-context-near"), "382k is near 400k: it says it summarizes there");

    // Read again as it's opened: far from the mark, it says no more than the numbers.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    stand_in.set_context("ses_signup", json!({ "used": 12_000, "window": 1_000_000, "summarizeAt": 400_000 }));
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-context-words-12k of 1M") && !shown(cx, "activity-context-near"));
}

/// The summarize point is picked right in Activity, under the context bar: the box's setting
/// (Settings › Conversations), sent to the conversation's box, which then puts the mark there. The
/// bar shows from 0 before anything's measured.
#[gpui::test]
fn picking_a_summarize_point_in_activity_sends_it_to_the_box(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.add_settings_section(
        json!({ "id": "conversations", "title": "Conversations", "group": "Agents & Tools", "icon": "message", "line": "When supported agents summarize long conversations on this computer." }),
        json!({ "id": "conversations", "title": "Conversations", "groups": [{ "id": "context", "title": "Context", "rows": [{
            "id": "compaction", "title": "Summarize long conversations at", "kind": "choice", "value": "400000", "can": true,
            "choices": [{ "id": "200000", "title": "200k tokens" }, { "id": "400000", "title": "400k tokens" }, { "id": "600000", "title": "600k tokens" }, { "id": "never", "title": "Never (large windows)" }] }] }] }),
    );
    let choices = json!([{ "id": "200000", "title": "200k" }, { "id": "400000", "title": "400k", "line": "Default" }, { "id": "600000", "title": "600k" }, { "id": "never", "title": "Never" }]);
    stand_in.set_summarize("ses_signup", json!({ "value": "400000", "label": "Summarize at", "choices": choices, "line": "All conversations" }));
    stand_in.set_context("ses_signup", json!({ "used": 0, "window": 1_000_000, "summarizeAt": 400_000 }));
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-context-words-0 of 1M") && shown(cx, "activity-context-bar"), "the bar shows from 0");
    assert!(shown(cx, "activity-summarize-at-400k"), "the point in use, in the box's words");

    click(cx, "activity-summarize-at-400k");
    assert!(shown(cx, "menu-item-200k") && shown(cx, "menu-item-Never"));
    click(cx, "menu-item-600k");
    assert!(shown(cx, "conversation-activity-popover"), "picking keeps Activity open");
    assert!(shown(cx, "activity-summarize-at-600k"), "the box's setting is 600k now, as it reads it");
    assert!(!shown(cx, "activity-summarize-failure"));
}

/// Pinned, Activity is a slim card at the conversation's top right that stays as you chat, beside
/// the column, never over it.
#[gpui::test]
fn pinned_activity_stays_open_beside_the_conversation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    stand_in.set_context("ses_signup", json!({ "used": 184_000, "window": 1_000_000, "summarizeAt": 400_000 }));
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    click(cx, "activity-pin");
    assert!(!shown(cx, "conversation-activity-popover"));
    assert!(shown(cx, "activity-pinned-card") && shown(cx, "activity-context-words-184k of 1M"));
    assert!(shown(cx, "activity-pinned-processes"));

    // Chatting doesn't close it, as a click outside closes the popover.
    click(cx, "composer-card");
    cx.simulate_input("Also check the sign-up form");
    cx.run_until_parked();
    let card = bounds(cx, "activity-pinned-card").expect("still pinned as you chat");
    let composer = bounds(cx, "composer-card").unwrap();
    assert!(card.left() >= composer.right(), "beside the conversation, not over it");

    // Unpin closes it: no Close beside it.
    assert!(shown(cx, "activity-unpin") && !shown(cx, "activity-pinned-close"), "the pinned card has no Close");
}

/// Unpin closes the pinned card, and nothing opens in its place: the conversation widens once.
#[gpui::test]
fn unpinning_activity_closes_the_card_without_opening_the_popover(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    click(cx, "activity-pin");
    let pinned = bounds(cx, "composer-card").unwrap();
    click(cx, "activity-unpin");
    assert!(!shown(cx, "activity-pinned-card"), "the card is gone");
    assert!(!shown(cx, "conversation-activity-popover"), "no popover opens in its place");
    let unpinned = bounds(cx, "composer-card").unwrap();
    assert!(unpinned.center().x > pinned.center().x, "the conversation widens back: {pinned:?} → {unpinned:?}");
}

/// Four documents it made, as the box lists them (more than its overview shows).
fn made(stand_in: &StandIn) {
    for i in 1..=4 {
        stand_in.add_output("ses_signup", json!({
            "id": format!("art_doc_{i:024}"), "path": format!("/tmp/report-{i}.md"), "name": format!("report-{i}.md"),
            "kind": "document", "type": "text/markdown", "latest": { "n": 1 },
        }), b"# Report");
    }
}

/// While pinned, a row opens inside the docked card, never the popover over the conversation,
/// and the conversation doesn't move.
#[gpui::test]
fn pinned_activity_opens_outputs_inside_the_card_without_moving_the_conversation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    made(&stand_in);
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    click(cx, "activity-pin");
    let before = bounds(cx, "composer-card").unwrap();
    click(cx, "activity-view-Full");
    assert!(shown(cx, "activity-see-all-outputs"), "Full lists what it made");
    click(cx, "activity-see-all-outputs");
    assert!(shown(cx, "activity-pinned-card") && shown(cx, "activity-all-outputs"), "all outputs show in the pinned card");
    assert!(!shown(cx, "conversation-activity-popover"), "never the popover");
    let after = bounds(cx, "composer-card").unwrap();
    assert_eq!(after, before, "the conversation keeps its width: {before:?} → {after:?}");
}

/// The pinned summary lists only what's live: its context, the sessions and processes running,
/// and subagents while one runs. What it made is in Full.
#[gpui::test]
fn the_pinned_summary_lists_only_whats_live(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    running(&stand_in);
    made(&stand_in);
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    assert!(shown(cx, "activity-outputs-header"), "the popover lists outputs");
    click(cx, "activity-pin");
    assert!(shown(cx, "activity-pinned-processes") && shown(cx, "activity-pinned-subagents"));
    assert!(!shown(cx, "activity-pinned-outputs") && !shown(cx, "activity-outputs-header"), "no Outputs in the summary");
}

/// The pin is one setting, kept: another conversation opens with its Activity pinned, in the view
/// it was left in, and so does the app after a restart.
#[gpui::test]
fn the_pinned_activity_is_remembered_across_conversations(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let folder = tempfile::tempdir().unwrap();
    let kept = folder.path().join("yours.json");
    let path = kept.clone();
    cx.update(move |cx| kuvo::yours::init(Some(path), cx));
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    click(cx, "activity-pin");
    click(cx, "activity-view-Full");
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session_now("ses_dark", window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "activity-pinned-card"), "still pinned in another conversation");
    assert!(shown(cx, "insights-on-Full"), "in Full, as it was left");
    workspace.update_in(cx, |workspace, window, cx| workspace.open_session_now("ses_signup", window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "activity-pinned-card"), "and back in the first");
    // After a restart, the app reads it as it was left.
    let read = kuvo::yours::Store::read(Some(kept)).now().activity;
    assert_eq!(read, kuvo::yours::ActivityPin { pinned: true, full: true });
}

/// The pinned card lists a few sessions at work on a line each; many fold to one line saying how
/// many work and wait, until opened.
#[gpui::test]
fn pinned_activity_folds_sessions_when_many_are_at_work(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let sessions = |n: usize| (0..n).map(|ix| json!({
        "session": format!("ses_w{ix}"), "title": format!("Task {ix}"), "state": if ix == 0 { "asking" } else { "working" },
    })).collect::<Vec<_>>();
    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [], "activity": { "working": 1, "asking": 1, "sessions": sessions(2) } }), &[]);
    let (workspace, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    click(cx, "activity-pin");
    assert!(shown(cx, "activity-pinned-session-ses_w0") && shown(cx, "activity-pinned-session-ses_w1"));

    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [], "activity": { "working": 4, "asking": 1, "sessions": sessions(5) } }), &[]);
    let bars = workspace.read_with(cx, |workspace, cx| workspace.conversation().unwrap().read(cx).bars().clone());
    bars.update(cx, |bars, cx| bars.read_at_once(cx));
    cx.run_until_parked();
    assert!(shown(cx, "activity-pinned-sessions-meta-5 · 4 working · 1 waiting"));
    assert!(!shown(cx, "activity-pinned-session-ses_w0"), "folded while many are at work");
    click(cx, "activity-fold-pinned-sessions");
    assert!(shown(cx, "activity-pinned-session-ses_w4"));
}
