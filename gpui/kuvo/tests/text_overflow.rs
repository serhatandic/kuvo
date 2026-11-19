//! Real-view text containment: a conversation's steps-group summary line and expanded step rows
//! truncate with an ellipsis inside their column, and a question card's answer options wrap cleanly
//! inside the card instead of running past the right edge.

use gpui::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::entries::Fold;
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme, theme::space};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

fn stage_overflow_session() -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_facts("ses_dark", |facts| {
        facts["title"] = json!("Outputs popover rework & image viewer");
    });
    stand_in.add_message("ses_dark", json!({
        "type": "user", "id": "msg_ov_u1", "time": { "created": NOW - 120_000.0 },
        "text": "Queue the Outputs popover rework, check the image viewer branch, and ask which draft direction to pursue first."
    }));
    stand_in.add_message("ses_dark", json!({
        "type": "assistant", "id": "msg_ov_a1", "time": { "created": NOW - 110_000.0, "completed": NOW - 95_000.0 },
        "agent": "build", "finish": "stop",
        "content": [
            { "type": "tool", "id": "ov_1", "name": "kuvo-browser_queue_list", "title": "Listed the Work Queue", "doing": "Listing the Work Queue", "state": { "status": "completed", "input": { "search": "image viewer" }, "content": [{ "type": "text", "text": "2 items" }] } },
            { "type": "tool", "id": "ov_2", "name": "shell", "state": { "status": "completed", "input": { "command": "cd ~/Projects/kuvo-ai && git worktree add -q -b kuvo/900d-popover-viewer ~/.kuvo-code/tmp/sol/900d main" }, "content": [{ "type": "text", "text": "" }] } },
            { "type": "tool", "id": "ov_3", "name": "kuvo-browser_queue_add", "title": "Added \"Outputs popover rework (cluttered, hundreds of sessions, unclear wake-ups)\" to the Work Queue", "doing": "Adding to the Work Queue", "state": { "status": "completed", "input": { "title": "Outputs popover rework (cluttered, hundreds of sessions, unclear wake-ups)" }, "content": [{ "type": "text", "text": "Added #932" }] } },
            { "type": "tool", "id": "ov_4", "name": "kuvo-browser_start_session", "title": "Started a session", "doing": "Starting a session", "state": { "status": "completed", "input": { "title": "Drafts: one image viewer + a calmer Outputs popover (Gemini Neon)" }, "content": [{ "type": "text", "text": "Started" }] } },
            { "type": "tool", "id": "ov_5", "name": "shell", "state": { "status": "completed", "input": { "command": "cd ~/.kuvo-code/bots/bot_drisrnywfr/workspace/notes && echo \"- 11-20 00:3x: user OK'd drafts\" >> log.md" }, "content": [{ "type": "text", "text": "" }] } },
            { "type": "text", "text": "Queued #932 and set up the worktree. Before building the unified image viewer, which behavior do you want?" }
        ]
    }));
    stand_in.set_asks("ses_dark", vec![json!({
        "id": "frm_ov", "kind": "question", "session": "ses_dark", "title": "Question",
        "questions": [{
            "key": "q0",
            "title": "Unified image viewer direction",
            "question": "Which viewer layout should replace the lightbox across conversations, the Outputs popover, Work Queue attachments, and the side panel?",
            "options": [
                {
                    "value": "stage",
                    "label": "Stage (floating dark gallery)",
                    "description": "Always-dark scrim with edge chevrons, bottom filmstrip for multi-image galleries, and floating zoom + action pills"
                },
                {
                    "value": "sheet",
                    "label": "Sheet (theme-native Quick Look dialog)",
                    "description": "Centered elevated card matching Kuvo's light and dark themes with zero chrome overlapping the image canvas"
                },
                {
                    "value": "short",
                    "label": "Keep current",
                    "description": ""
                }
            ],
            "multiple": false, "custom": true, "required": true
        }]
    })]);
    stand_in
}

fn open(cx: &mut TestAppContext, stand_in: StandIn) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1080.), px(820.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        workspace.open_read_at_once("ses_dark", window, cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

#[gpui::test]
fn steps_summary_step_rows_and_question_options_stay_within_their_containers(cx: &mut TestAppContext) {
    let (workspace, cx) = open(cx, stage_overflow_session());

    // Open the steps group so both its summary line and individual step rows are rendered.
    workspace.update_in(cx, |workspace, _, cx| {
        let view = workspace.conversation().expect("conversation");
        view.update(cx, |view, cx| {
            view.toggle(Fold::Group("msg_ov_a1".to_owned()), cx);
        });
    });
    cx.run_until_parked();

    let entry = bounds(cx, "entry-msg_ov_a1");
    let entry_inner_right = entry.right() - space::S6;

    // (a) Conversation steps-group summary line stays within its column on one line.
    let group_row = bounds(cx, "group-msg_ov_a1");
    let group_text = bounds(cx, "group-msg_ov_a1-text");
    assert!(
        group_row.right() <= entry_inner_right + px(0.5),
        "steps-group summary row overflowed: row={group_row:?}, inner_right={entry_inner_right:?}"
    );
    assert!(
        group_text.right() <= entry_inner_right + px(0.5),
        "steps-group summary text overflowed: text={group_text:?}, inner_right={entry_inner_right:?}"
    );
    assert!(
        group_row.size.height <= px(28.),
        "steps-group summary should remain single-line, got {:?}",
        group_row.size.height
    );

    // Individual expanded step rows also stay within the column.
    for n in 0..5 {
        let row_sel = format!("tool-msg_ov_a1:{n}");
        let text_sel = format!("tool-msg_ov_a1:{n}-text");
        let step_row = bounds(cx, &row_sel);
        let step_text = bounds(cx, &text_sel);
        assert!(
            step_row.right() <= entry_inner_right + px(0.5),
            "{row_sel} overflowed: row={step_row:?}, inner_right={entry_inner_right:?}"
        );
        assert!(
            step_text.right() <= entry_inner_right + px(0.5),
            "{text_sel} overflowed: text={step_text:?}, inner_right={entry_inner_right:?}"
        );
    }

    // (b) Question card answer options stay within the AskCard and wrap onto multiple lines.
    let card = bounds(cx, "ask-card");
    let opt0 = bounds(cx, "ask-option-0");
    let opt0_text = bounds(cx, "ask-option-0-text");
    let opt1 = bounds(cx, "ask-option-1");
    let opt1_text = bounds(cx, "ask-option-1-text");
    let opt2 = bounds(cx, "ask-option-2");

    for (name, b) in [
        ("ask-option-0", opt0),
        ("ask-option-0-text", opt0_text),
        ("ask-option-1", opt1),
        ("ask-option-1-text", opt1_text),
        ("ask-option-2", opt2),
    ] {
        assert!(
            b.left() >= card.left() - px(0.5) && b.right() <= card.right() + px(0.5),
            "{name} overflowed ask-card: item={b:?}, card={card:?}"
        );
    }
    assert!(
        opt0.size.height > opt2.size.height,
        "long question option should wrap vertically: long={opt0:?}, short={opt2:?}"
    );
    assert!(
        opt1.size.height > opt2.size.height,
        "long question option should wrap vertically: long={opt1:?}, short={opt2:?}"
    );
}
