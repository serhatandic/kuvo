#[tokio::test]
async fn saved_session_headers_keep_vendor_state_and_ignore_incomplete_folders() {
    use kuvo_agents::session_store::SessionStore;

    let temp = tempfile::tempdir().unwrap();
    let saved = temp.path().join("ses_legacy");
    std::fs::create_dir(&saved).unwrap();
    std::fs::write(
        saved.join("session.json"),
        br#"{"id":"ses_legacy","location":{"directory":"project"},"vendorExtension":true}"#,
    )
    .unwrap();
    std::fs::write(
        saved.join("agent.json"),
        br#"{"threads":{"codex-cli":{"state":{"thread":"native-id","generation":2},"seen":"msg_1"}},"inbox":[{"id":"msg_2"}]}"#,
    )
    .unwrap();
    std::fs::create_dir(temp.path().join("ses_empty")).unwrap();
    let future = temp.path().join("ses_future");
    std::fs::create_dir(&future).unwrap();
    let unknown = br#"{"id":"ses_future","formatVersion":100}"#;
    std::fs::write(future.join("session.json"), unknown).unwrap();

    let store = SessionStore::new(temp.path().to_path_buf());
    let headers = store.headers().await.unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].id(), "ses_legacy");
    assert_eq!(
        headers.into_iter().next().unwrap().into_fields()["vendorExtension"],
        true
    );
    let context = store
        .context("ses_legacy", "codex-cli")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(context.session_id, "ses_legacy");
    assert_eq!(context.directory, std::path::Path::new("project"));
    assert_eq!(context.state["thread"], "native-id");
    assert_eq!(context.state["generation"], 2);
    // Another agent's turn in the same conversation starts with nothing of its own.
    let other = store.context("ses_legacy", "claude-code").await.unwrap().unwrap();
    assert!(other.state.is_empty());
    assert!(
        store
            .save_state("ses_legacy", "codex-cli", Default::default())
            .await
            .unwrap()
    );
    assert!(
        store
            .context("ses_legacy", "codex-cli")
            .await
            .unwrap()
            .unwrap()
            .state
            .is_empty()
    );
    // The inbox and the slot's other notes stay where the bridge keeps them.
    assert_eq!(
        std::fs::read_to_string(saved.join("agent.json")).unwrap(),
        r#"{"threads":{"codex-cli":{"state":{},"seen":"msg_1"}},"inbox":[{"id":"msg_2"}]}"#
    );
    assert!(store.delete("ses_legacy").await.unwrap());
    assert!(
        !store
            .save_state("ses_legacy", "codex-cli", context.state)
            .await
            .unwrap()
    );
    assert!(!saved.exists());
    assert!(store.read("ses_../../ses_legacy").await.unwrap().is_none());
    assert!(store.read("ses_future").await.is_err());
    assert_eq!(std::fs::read(future.join("session.json")).unwrap(), unknown);
}

#[tokio::test]
async fn a_single_agents_older_state_becomes_the_slot_of_the_agent_that_replied() {
    use kuvo_agents::session_store::SessionStore;
    use serde_json::json;

    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("ses_old");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("session.json"),
        json!({"id":"ses_old","location":{"directory":"p"},"model":{"providerID":"claude-code","id":"opus"}})
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        folder.join("messages.json"),
        json!([{"id":"msg_a","type":"assistant","model":{"providerID":"codex-cli","id":"gpt"},"content":[]}])
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        folder.join("agent.json"),
        br#"{"inbox":[],"codexThread":"thr_1","snapshots":{}}"#,
    )
    .unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    // Codex replied last, so the old single state is its own.
    let codex = store.context("ses_old", "codex-cli").await.unwrap().unwrap();
    assert_eq!(codex.state, *json!({"codexThread":"thr_1"}).as_object().unwrap());
    let claude = store.context("ses_old", "claude-code").await.unwrap().unwrap();
    assert!(claude.state.is_empty());
    let mut state = codex.state;
    state.insert("codexThread".into(), json!("thr_2"));
    assert!(store.save_state("ses_old", "claude-code", Default::default()).await.unwrap());
    assert_eq!(
        std::fs::read_to_string(folder.join("agent.json")).unwrap(),
        r#"{"inbox":[],"snapshots":{},"threads":{"codex-cli":{"state":{"codexThread":"thr_1"},"seen":"msg_a"},"claude-code":{"state":{}}}}"#
    );
    assert!(store.save_state("ses_old", "codex-cli", state).await.unwrap());
    let reread = store.context("ses_old", "codex-cli").await.unwrap().unwrap();
    assert_eq!(reread.state["codexThread"], "thr_2");
}

#[tokio::test]
async fn saved_transcripts_page_both_legacy_arrays_and_bridge_sqlite() {
    use kuvo_agents::session_store::{MessageOrder, SessionStore};
    use kuvo_store::sqlite;
    use serde_json::json;
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("ses_saved");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("session.json"), br#"{"id":"ses_saved"}"#).unwrap();
    let messages = [
        json!({"id":"msg_first", "type":"user", "text":"First"}),
        json!({"id":"msg_second", "type":"assistant", "content":[]}),
        json!({"id":"msg_third", "type":"user", "text":"Third"}),
    ];
    std::fs::write(
        folder.join("messages.json"),
        serde_json::to_vec(&messages).unwrap(),
    )
    .unwrap();
    let store = SessionStore::new(temp.path().to_owned());

    for sqlite_layout in [false, true] {
        if sqlite_layout {
            // Same schema as the bridge writer, beside a stale rollback copy.
            let db = sqlite::open_wal(
                &folder.join("messages-v1.sqlite"),
                Duration::from_secs(1),
                "Kuvo transcript",
                &[|db| {
                    db.execute_batch(
                        "CREATE TABLE message(
                            position INTEGER PRIMARY KEY, id TEXT, body TEXT NOT NULL,
                            insight TEXT, artifacts TEXT
                        );
                        CREATE INDEX message_id ON message(id);",
                    )
                }],
            )
            .unwrap();
            for (position, message) in messages.iter().enumerate() {
                db.execute(
                    "INSERT INTO message(position, id, body) VALUES (?1, ?2, ?3)",
                    (position, message["id"].as_str(), message.to_string()),
                )
                .unwrap();
            }
            std::fs::write(folder.join("messages.json"), b"[]").unwrap();
        }
        let newest = store
            .message_page("ses_saved", None, 2, MessageOrder::NewestFirst)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(newest.messages, [messages[2].clone(), messages[1].clone()]);
        assert!(newest.more);
        let following = store
            .message_page(
                "ses_saved",
                Some("msg_second".into()),
                2,
                MessageOrder::OldestFirst,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(following.messages, [messages[2].clone()]);
        assert!(!following.more);
        let missing = store
            .message_page(
                "ses_saved",
                Some("msg_gone".into()),
                2,
                MessageOrder::NewestFirst,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(missing.messages.is_empty());
        assert!(!missing.more);
    }
}

#[tokio::test]
async fn transcript_updates_preserve_order_and_cannot_return_after_deletion() {
    use kuvo_agents::session_store::{MessageOrder, SessionStore};
    use kuvo_store::sqlite;
    use serde_json::{Value, json};
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("ses_saved");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("session.json"), br#"{"id":"ses_saved"}"#).unwrap();
    let first = json!({"id":"msg_first","type":"user","text":"private content"});
    let mut answer = json!({
        "id":"msg_step","type":"assistant","content":[{
            "type":"tool","id":"call_one","name":"Read",
            "state":{"status":"running","input":{"file_path":"notes.txt","extra":"private content"}}
        }]
    });
    let idle = json!({"id":"msg_idle","type":"idle","outcome":"succeeded"});
    std::fs::write(
        folder.join("messages.json"),
        serde_json::to_vec(&[first.clone(), answer.clone()]).unwrap(),
    )
    .unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    assert!(
        store
            .write_messages("ses_saved", vec![idle.clone()])
            .await
            .unwrap()
    );
    answer["content"][0]["state"]["status"] = "completed".into();
    assert!(
        store
            .write_messages("ses_saved", vec![answer.clone()])
            .await
            .unwrap()
    );
    let complete = store
        .message_page("ses_saved", None, 10, MessageOrder::OldestFirst)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(complete.messages, [first, answer.clone(), idle]);
    {
        let db =
            sqlite::open_index(&folder.join("messages-v1.sqlite"), Duration::from_secs(1)).unwrap();
        let read: String = db
            .query_row(
                "SELECT insight FROM message WHERE id = 'msg_step'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let projected: Value = serde_json::from_str(&read).unwrap();
        assert_eq!(
            projected["content"][0]["state"]["input"],
            json!({"file_path":"notes.txt"})
        );
    }
    assert!(store.delete("ses_saved").await.unwrap());
    assert!(
        !store
            .write_messages("ses_saved", vec![answer])
            .await
            .unwrap()
    );
    assert!(!folder.exists());
}

#[tokio::test]
async fn a_client_change_keeps_the_bridges_own_notes_in_place() {
    use kuvo_agents::session_store::{SessionPatch, SessionStore};
    use serde_json::{Value, json};

    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("ses_notes");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("session.json"),
        serde_json::to_vec(&json!({"id":"ses_notes","title":"Before","time":{"created":1,"updated":2},
            "metadata":{"kuvoAgent":"claude","kuvo":{"messages":"subagent","old":true}}}))
        .unwrap(),
    )
    .unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    let given = json!({"kuvo":{"approval":"auto"},"kuvoAgent":"codex","kuvo.pinned":true});
    let Value::Object(given) = given else { unreachable!() };
    let saved = store
        .update(
            "ses_notes",
            SessionPatch {
                title: Some("After".into()),
                metadata: Some(given),
                ..SessionPatch::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved["title"], "After");
    assert!(saved["time"]["updated"].as_u64().unwrap() > 2);
    // The client's order stays; the agent and the subagent's address are the bridge's.
    assert_eq!(
        serde_json::to_string(&saved["metadata"]).unwrap(),
        r#"{"kuvo":{"approval":"auto","messages":"subagent"},"kuvoAgent":"claude","kuvo.pinned":true}"#
    );
    let reread = store.read("ses_notes").await.unwrap().unwrap().into_fields();
    assert_eq!(reread["metadata"], saved["metadata"]);
    assert!(
        store
            .update("ses_gone", SessionPatch::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn deleting_a_conversation_takes_its_subagents_unless_a_fork_shows_them() {
    use kuvo_agents::{
        permissions::ConfigLocation, resident::ResidentAgents, runtime::Runtime,
        session_runs::SessionRuns, session_store::SessionStore,
    };
    use serde_json::json;
    use std::collections::HashMap;

    let scratch = tempfile::tempdir().unwrap();
    let home = kuvo_store::home::Home::new(scratch.path().to_owned()).unwrap();
    let root = scratch.path().join("agents/sessions");
    let header = |id: &str, parent: Option<&str>| {
        let folder = root.join(id);
        std::fs::create_dir_all(&folder).unwrap();
        let mut fields = json!({"id": id, "location": {"directory": "project"}});
        if let Some(parent) = parent {
            fields["parentID"] = json!(parent);
        }
        std::fs::write(folder.join("session.json"), fields.to_string()).unwrap();
    };
    header("ses_lead", None);
    header("ses_helper", Some("ses_lead"));
    header("ses_shared", Some("ses_lead"));
    header("ses_fork", None);
    header("ses_other", None);
    let store = SessionStore::new(root.clone());
    let step = json!({"id":"msg_step","type":"assistant","content":[
        {"type":"tool","state":{"metadata":{"sessionID":"ses_shared"}}}]});
    assert!(store.write_messages("ses_fork", vec![step]).await.unwrap());

    let (agents, _) = ResidentAgents::start(Runtime::for_home(&home), HashMap::new())
        .await
        .unwrap();
    let runs = SessionRuns::start(
        agents,
        store.clone(),
        scratch.path().to_owned(),
        ConfigLocation::Machine,
    );
    assert!(runs.delete("ses_lead".into()).await.unwrap());
    let left = store
        .headers()
        .await
        .unwrap()
        .iter()
        .map(|header| header.id().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(left, ["ses_fork", "ses_other", "ses_shared"]);
    assert!(!runs.delete("ses_lead".into()).await.unwrap());
}

#[test]
fn insights_read_a_legacy_reply_without_keeping_its_large_tool_output() {
    use kuvo_agents::session_store::legacy_insight;
    use serde_json::json;
    use std::io::Write;

    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("messages.json");
    let mut transcript = std::fs::File::create(&file).unwrap();
    write!(transcript, r#"[{{"type":"assistant","id":"msg_usage","agent":"ada","time":{{"created":1,"completed":2}},
        "model":{{"id":"claude-test","providerID":"claude-code"}},"tokens":{{"input":12,"output":3}},
        "content":[{{"type":"tool","id":"large-result","name":"shell","state":{{"status":"completed","content":[{{"type":"text","text":""#).unwrap();
    let block = "a".repeat(1024 * 1024);
    for _ in 0..48 {
        transcript.write_all(block.as_bytes()).unwrap();
    }
    write!(transcript, r#""}}]}}}},{{"type":"tool","id":"unfinished","name":"read","state":{{"status":"streaming","input":"{{"}}}},{{"type":"tool","id":"skill","name":"skill","state":{{"status":"completed","input":{{"skill":"paper \"résumé\""}},"content":[]}}}}]}}]"#).unwrap();
    drop(transcript);

    let rows = legacy_insight(&file).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["tokens"], json!({"input":12,"output":3}));
    assert_eq!(rows[0]["content"], json!([{"type":"tool","id":"skill","name":"skill","state":{"status":"completed","input":{"skill":"paper \"résumé\""},"content":[]}}]));
}
