// The resident's sink prepares the words sent to a native agent from a saved
// conversation. No native process is needed to check the handover itself.
use kuvo_agents::{
    live_asks::LiveAsks,
    runtime::AdapterCall,
    session::{Decision, PermissionAsk, Preflight, TurnSink},
    session_store::SessionStore,
    turn_sink::{ResidentSink, TurnPolicy},
};
use serde_json::json;
use std::sync::Arc;

struct Policy;
impl TurnPolicy for Policy {
    fn preflight(&self, _: PermissionAsk, _: bool) -> AdapterCall<'_, Preflight> {
        Box::pin(async { Ok(Preflight::Deny) })
    }

    fn permission(&self, _: &PermissionAsk) -> AdapterCall<'_, Option<Decision>> {
        Box::pin(async { Ok(Some(Decision::Reject)) })
    }
}

#[tokio::test]
async fn a_message_added_while_another_agent_works_is_still_part_of_the_request() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("ses_saved");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("session.json"),
        br#"{"id":"ses_saved","location":{"directory":"root"}}"#,
    )
    .unwrap();
    let history = json!([
        {"type":"user","id":"msg_open","text":"Original request"},
        {"type":"assistant","id":"msg_old","model":{"providerID":"claude-code","id":"haiku"},"content":[{"type":"text","text":"Old answer"}]},
        {"type":"user","id":"msg_current","text":"Build the parser."},
        {"type":"assistant","id":"msg_work","model":{"providerID":"claude-code","id":"haiku"},"content":[{"type":"text","text":"Created the parser."},{"type":"tool","name":"edit","state":{"status":"completed","input":{"path":"parser.rs"}}}]},
        {"type":"user","id":"msg_taken","text":"Already given to the next agent."},
        {"type":"user","id":"msg_added","text":"Use tabs too."}
    ]);
    std::fs::write(folder.join("messages.json"), history.to_string()).unwrap();
    std::fs::write(
        folder.join("agent.json"),
        json!({"threads":{"codex-cli":{"state":{},"seen":"msg_old","seenSteers":["msg_taken"]}}})
            .to_string(),
    )
    .unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    let asks = LiveAsks::start();
    let (sink, _) = ResidentSink::channel(
        "ses_saved".into(),
        "codex-cli".into(),
        store,
        None,
        asks.for_turn("ses_saved".into(), "msg_current".into()),
        None,
        Arc::new(Policy),
    );
    let context = sink.handover("msg_current", false).await.unwrap().unwrap();
    assert_eq!(
        context,
        "Another agent started on the message below before it stopped. What it did is already done; what the user added while it worked is part of what they ask:\n\n[Agent]\n(Claude Code, haiku)\nCreated the parser.\n\n[File change]\nparser.rs\n\n[You, while it worked]\nUse tabs too."
    );
}
