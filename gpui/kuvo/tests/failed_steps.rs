//! A step that failed, as someone reads a conversation (#949): one short line in the step's own
//! style, the box's words ("Ran 1 command · blocked"), with no error, tag or path under it; all it
//! said shows only once it's opened, by a click through the window. Against the stand-in box.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
use kuvo::stand_in::{Replies, StandIn};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::{Value, json};

const NOW: f64 = 1_794_650_000_000.0;
const REFUSED: &str = "Blocked: sleep 240 followed by: cd /Users/me/work scripts/heavy.sh status tail -3 /private/tmp/claude-501/-Users-me-work/28e29ba5/tasks/bcj6wfy9b.output. To wait for a condition, use Monitor with an until-loop (e.g. `until <check>; do sleep 2; done`). To wait for a command you started, use run_in_background: true. Do not chain shorter sleeps to work around this block.";

fn user(id: &str, at: f64, text: &str) -> Value {
    json!({ "type": "user", "id": id, "time": { "created": at }, "text": text })
}

fn assistant(id: &str, at: f64, tool: Value) -> Value {
    json!({ "type": "assistant", "id": id, "time": { "created": at, "completed": at + 1_000.0 }, "agent": "build", "finish": "stop", "content": [tool] })
}

/// A conversation with a command that ran, one Claude Code's harness refused (as its box words
/// it), an integration's that timed out, and one refused as a box from before it worded failures
/// gives it.
fn stage() -> StandIn {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_facts("ses_dark", |facts| facts["title"] = json!("Failed steps"));
    let raw = format!("<tool_use_error>{REFUSED}</tool_use_error>");
    stand_in.add_message("ses_dark", user("u1", NOW - 60_000.0, "Build it."));
    stand_in.add_message("ses_dark", assistant("ok", NOW - 59_000.0, json!({
        "type": "tool", "id": "t1", "name": "shell",
        "state": { "status": "completed", "input": { "command": "sleep 240; cd /Users/me/work && scripts/heavy.sh status 2>&1 | grep rust" }, "content": [{ "type": "text", "text": "rust: free" }] }
    })));
    stand_in.add_message("ses_dark", user("u2", NOW - 50_000.0, "Again."));
    stand_in.add_message("ses_dark", assistant("bad", NOW - 49_000.0, json!({
        "type": "tool", "id": "t2", "name": "shell",
        "state": { "status": "error", "input": { "command": "sleep 240; cd /Users/me/work && scripts/heavy.sh status" }, "error": { "type": "tool", "message": raw }, "content": [{ "type": "text", "text": raw }] },
        "failure": { "line": "Ran 1 command · blocked", "reason": "blocked", "error": REFUSED }
    })));
    stand_in.add_message("ses_dark", user("u3", NOW - 40_000.0, "Issues?"));
    stand_in.add_message("ses_dark", assistant("mcp", NOW - 39_000.0, json!({
        "type": "tool", "id": "t3", "name": "linear_list_issues", "title": "Linear · List issues", "doing": "Linear · List issues",
        "state": { "status": "error", "input": {}, "error": "MCP error: Error: Request timed out" },
        "failure": { "line": "Linear · List issues · timed out", "reason": "timed out", "error": "Request timed out" }
    })));
    stand_in.add_message("ses_dark", user("u4", NOW - 30_000.0, "Once more."));
    stand_in.add_message("ses_dark", assistant("older", NOW - 29_000.0, json!({
        "type": "tool", "id": "t4", "name": "read",
        "state": { "status": "error", "input": { "filePath": "/Users/me/work/gone.ts" }, "error": { "type": "tool", "message": "<tool_use_error>File does not exist. Note: your current working directory is /Users/me/work.</tool_use_error>" } }
    })));
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
    cx.simulate_resize(size(px(1080.), px(1400.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        workspace.open_read_at_once("ses_dark", window, cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

fn drawn(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    bounds(cx, selector).unwrap_or_else(|| panic!("{selector} is not drawn"))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = drawn(cx, selector);
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

/// How far its entry goes on under step `row`: nothing drawn there but the entry's own room.
fn under(cx: &mut VisualTestContext, entry: &str, row: &str) -> Pixels {
    drawn(cx, &format!("entry-{entry}")).bottom() - drawn(cx, row).bottom()
}

#[gpui::test]
fn a_failed_step_is_one_short_line_until_its_opened(cx: &mut TestAppContext) {
    let (_workspace, cx) = open(cx, stage());

    // Closed, each failed step is a row like the step that ran: one line, nothing under it.
    let ran = drawn(cx, "tool-ok:0");
    let room = under(cx, "ok", "tool-ok:0");
    for (entry, row) in [("bad", "tool-bad:0"), ("mcp", "tool-mcp:0"), ("older", "tool-older:0")] {
        assert_eq!(drawn(cx, row).size.height, ran.size.height, "{row} is one line");
        assert_eq!(under(cx, entry, row), room, "nothing shows under {row} while it's closed");
    }
    assert!(bounds(cx, "step-error").is_none() && bounds(cx, "output-bad:0").is_none(), "no error shows closed");

    // Opened, a command shows itself and all its harness said, as its output.
    click(cx, "tool-bad:0");
    assert!(bounds(cx, "output-bad:0").is_some(), "the refused command opens to what was said");
    assert!(bounds(cx, "step-error").is_none(), "said once, in its output");

    // Another tool's opens to its error, in words.
    click(cx, "tool-mcp:0");
    assert!(bounds(cx, "step-error").is_some(), "the timed-out call opens to why");
    assert!(under(cx, "mcp", "tool-mcp:0") > room);

    // A box from before it worded failures: still nothing under the row, the error once opened.
    click(cx, "tool-older:0");
    assert!(under(cx, "older", "tool-older:0") > room, "it opens to its error");
}
