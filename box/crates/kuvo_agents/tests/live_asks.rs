use kuvo_agents::{
    live_asks::{AskKey, LiveAsks, Reply},
    runtime::AdapterCall,
    session::{
        Answer, Decision, Delivery, EventEnvelope, PermissionAsk, Position, Preflight, QuestionAsk,
        TurnEvent, TurnSink,
    },
    session_store::SessionStore,
    turn_sink::{ResidentSink, TurnPolicy},
};
use std::{io::ErrorKind, sync::Arc, time::Duration};

fn permission(id: &str) -> PermissionAsk {
    PermissionAsk {
        id: id.into(),
        tool_call_id: Some("call_tool".into()),
        action: "edit".into(),
        resources: vec!["notes.txt".into()],
        save: Vec::new(),
        message: None,
        metadata: Default::default(),
        sandboxed: None,
        already_weighed: false,
        unclassified: false,
    }
}

#[tokio::test]
async fn cancellation_and_answers_belong_to_the_turn_that_asked() {
    let asks = LiveAsks::start();
    let first = Arc::new(asks.for_turn("ses_first".into(), "turn_1".into()));
    let second = Arc::new(asks.for_turn("ses_second".into(), "turn_2".into()));
    let mut pending = asks.pending();
    let first_reply = {
        let first = first.clone();
        tokio::spawn(async move { first.permission(permission("same_id")).await })
    };
    let second_reply = {
        let second = second.clone();
        tokio::spawn(async move { second.permission(permission("same_id")).await })
    };
    tokio::time::timeout(
        Duration::from_secs(1),
        pending.wait_for(|asks| asks.len() == 2),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        second
            .permission(permission("same_id"))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
    first.cancel("same_id").await.unwrap();
    assert_eq!(first_reply.await.unwrap().unwrap(), Decision::Reject);
    assert!(!second_reply.is_finished());
    let key = AskKey {
        session: "ses_second".into(),
        turn: "turn_2".into(),
        id: "same_id".into(),
    };
    asks.reply(key.clone(), Reply::Permission(Decision::Once))
        .await
        .unwrap();
    assert_eq!(second_reply.await.unwrap().unwrap(), Decision::Once);
    assert!(pending.borrow().is_empty());
    assert_eq!(
        asks.reply(key, Reply::Permission(Decision::Always))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn ending_a_turn_withdraws_its_asks_and_rejects_late_asks() {
    let asks = LiveAsks::start();
    let turn = Arc::new(asks.for_turn("ses_one".into(), "turn_1".into()));
    let mut pending = asks.pending();
    let reply = {
        let turn = turn.clone();
        tokio::spawn(async move {
            turn.question(QuestionAsk {
                id: "question".into(),
                title: "Where to go next?".into(),
                fields: Vec::new(),
                tool_call_id: None,
                metadata: Default::default(),
            })
            .await
        })
    };
    tokio::time::timeout(
        Duration::from_secs(1),
        pending.wait_for(|asks| !asks.is_empty()),
    )
    .await
    .unwrap()
    .unwrap();
    turn.end().await.unwrap();
    assert!(matches!(
        reply.await.unwrap().unwrap(),
        Answer::Dismissed { reason: None }
    ));
    assert!(pending.borrow().is_empty());
    assert_eq!(
        turn.permission(permission("arrived_late")).await.unwrap(),
        Decision::Reject
    );
    assert!(pending.borrow().is_empty());
}

#[tokio::test]
async fn the_turn_sink_waits_for_a_checkpoint_and_saves_the_native_conversation() {
    struct Policy;
    impl TurnPolicy for Policy {
        fn preflight(&self, _ask: PermissionAsk, _: bool) -> AdapterCall<'_, Preflight> {
            Box::pin(async { Ok(Preflight::Ask) })
        }

        fn permission(&self, _ask: &PermissionAsk) -> AdapterCall<'_, Option<Decision>> {
            Box::pin(async { Ok(None) })
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("ses_saved");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join("session.json"),
        br#"{"id":"ses_saved","location":{"directory":"root"}}"#,
    )
    .unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    let asks = LiveAsks::start();
    let (sink, mut writes) = ResidentSink::channel(
        "ses_saved".into(),
        "codex-cli".into(),
        store.clone(),
        asks.for_turn("ses_saved".into(), "turn_1".into()),
        Arc::new(Policy),
    );
    let mut pending = asks.pending();
    let reporting = {
        let sink = sink.clone();
        tokio::spawn(async move {
            sink.event(EventEnvelope {
                position: Some(Position {
                    sequence: 7,
                    ordinal: 0,
                }),
                delivery: Delivery::Replay,
                event: TurnEvent::Notice("I was already working".into()),
            })
            .await
            .unwrap();
            sink.save_state(
                serde_json::from_value(serde_json::json!({"nativeID":"saved"})).unwrap(),
            )
            .await
            .unwrap();
            sink.permission(permission("read_or_write")).await.unwrap()
        })
    };
    let event = tokio::time::timeout(Duration::from_secs(1), writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.event.position.unwrap().sequence, 7);
    assert!(!reporting.is_finished());
    assert!(pending.borrow().is_empty());
    event.finish(Ok(()));

    tokio::time::timeout(
        Duration::from_secs(1),
        pending.wait_for(|asks| asks.len() == 1),
    )
    .await
    .unwrap()
    .unwrap();
    let context = store
        .context("ses_saved", "codex-cli")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(context.state["nativeID"], "saved");
    let ask = pending.borrow().keys().next().unwrap().clone();
    asks.reply(ask, Reply::Permission(Decision::Once))
        .await
        .unwrap();
    assert_eq!(reporting.await.unwrap(), Decision::Once);
    sink.retire().await.unwrap();
    assert_eq!(
        sink.preflight(permission("late"), true).await.unwrap(),
        Preflight::Deny
    );
}
