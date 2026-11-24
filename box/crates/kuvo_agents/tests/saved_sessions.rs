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
        br#"{"thread":"native-id","generation":2}"#,
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
    let context = store.context("ses_legacy").await.unwrap().unwrap();
    assert_eq!(context.session_id, "ses_legacy");
    assert_eq!(context.directory, std::path::Path::new("project"));
    assert_eq!(context.state["thread"], "native-id");
    assert_eq!(context.state["generation"], 2);
    assert!(
        store
            .save_state("ses_legacy", Default::default())
            .await
            .unwrap()
    );
    assert!(
        store
            .context("ses_legacy")
            .await
            .unwrap()
            .unwrap()
            .state
            .is_empty()
    );
    assert!(store.delete("ses_legacy").await.unwrap());
    assert!(!store.save_state("ses_legacy", context.state).await.unwrap());
    assert!(!saved.exists());
    assert!(store.read("ses_../../ses_legacy").await.unwrap().is_none());
    assert!(store.read("ses_future").await.is_err());
    assert_eq!(std::fs::read(future.join("session.json")).unwrap(), unknown);
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
