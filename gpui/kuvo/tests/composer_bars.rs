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
    // Codex Quiet omits empty sections instead of showing an "Outputs" heading over an empty body.
    assert!(!shown(cx, "No outputs yet") && !shown(cx, "activity-outputs"));
    assert!(shown(cx, "activity-sessions") && shown(cx, "activity-processes"));
    assert!(!shown(cx, "activity-wakes") && shown(cx, "activity-session-ses_sub"));

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
    assert!(!shown(cx, "activity-processes") && shown(cx, "activity-sessions"));

    // A subagent opens in the side panel's Subagents tab, its conversation shown there.
    click(cx, "activity-session-ses_sub");
    let opened = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).subagents().and_then(|view| view.read(cx).opened().map(str::to_owned)));
    assert_eq!(opened.as_deref(), Some("ses_sub"));
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)), Some("ses_signup".into()), "the conversation stays");
    assert!(!shown(cx, "conversation-activity-popover") && shown(cx, "subagent-transcript"));
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
fn activity_shows_live_provider_faces_inline_and_expands_overflow_active_sessions(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let sessions = [
        ("ses_sub", "codex-cli", "codex"),
        ("ses_sub2", "claude-code", "claude"),
        ("ses_sub3", "codex-cli", "codex"),
        ("ses_sub4", "claude-code", "claude"),
        ("ses_sub5", "codex-cli", "codex"),
        ("ses_sub6", "claude-code", "claude"),
    ];
    for (id, provider, _) in sessions {
        stand_in.set_model_state(id, json!({ "model": { "providerID": provider, "id": "example" }, "runs": true }));
    }
    stand_in.set_background("ses_signup", json!({
        "subagents": sessions.iter().map(|(id, _, _)| json!({ "session": id, "since": NOW })).collect::<Vec<_>>(),
    }), &[]);
    let (_, cx) = conversation(cx, &stand_in, "ses_signup");
    click(cx, "conversation-activity-button");
    // First 5 live subagents render directly inline with their provider avatars; 6th sits behind "+1 more active".
    for (id, _, service) in &sessions[..5] {
        assert!(shown(cx, &format!("activity-row-avatar-{id}-{service}")));
    }
    assert!(!shown(cx, "activity-row-avatar-ses_sub6-claude"));
    assert!(shown(cx, "activity-toggle-sessions"));
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

/// The Stage viewer covers the whole window, title bar included: its name pill starts past the
/// window's own buttons at the top left (the user's beta.37: they sat over "Outputs"), and Esc
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
