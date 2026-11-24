// Exercise the move bundle and durable ownership storage through their public readers and writers.

use kuvo_peers::moves::{
    manifest::{Error as ManifestError, Manifest},
    receipt::{Bundle, Decided, Decision, Error as ReceiptError, Payloads, Receipts},
};
use kuvo_store::files;
use serde_json::{Value, json};

#[test]
fn older_manifests_keep_their_conversations_and_write_the_version_their_fields_need() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("move.json");
    let fields = json!({
        "kind": "chat", "thing": "ses_saved", "name": "Saved conversation", "wasPaused": true,
        "exports": [{"info": {"id": "ses_saved"}, "messages": [{"id": "msg_saved"}]}],
        "additionalField": {"kept": true},
    });
    for (version, written) in [
        (None, 1),
        (Some(0), 1),
        (Some(1), 1),
        (Some(2), 2),
        (Some(3), 3),
    ] {
        let mut input = fields.clone();
        if let Some(version) = version {
            input["formatVersion"] = json!(version);
        }
        if written >= 2 {
            input["sessionNotes"] = json!({ "ses_saved": { "text": "Remember this." } });
        }
        if written >= 3 {
            input["forwardEvidence"] = json!({});
        }
        let kept = Manifest::decode(&serde_json::to_vec(&input).unwrap()).unwrap();
        kept.write(&path).unwrap();
        input["formatVersion"] = json!(written);
        assert_eq!(read(&path), input, "input version {version:?}");
    }
    for stamp in [json!(4), json!(-1), json!(null), json!("3")] {
        let mut input = fields.clone();
        input["formatVersion"] = stamp;
        assert!(matches!(
            Manifest::decode(&serde_json::to_vec(&input).unwrap()),
            Err(ManifestError::Version(_))
        ));
    }
}

#[test]
fn a_destination_cannot_change_a_committed_decision_after_restart() {
    let folder = tempfile::tempdir().unwrap();
    let run = folder.path().join("run");
    let id = "mov_decided";
    let peer = "source";
    let switched = Decision {
        decided: Decided::Switched,
        at: 7,
        peer: Some(peer.into()),
        last: Some("msg_saved".into()),
        bundle: Bundle {
            generation: Some(2),
            hash: Some("H".repeat(43)),
        },
        payloads: Some(Payloads {
            sessions: vec![],
            queue: vec![],
        }),
    };
    let sessions = ["ses_saved".into()];
    Receipts::open(&run)
        .record(id, &switched, &sessions)
        .unwrap();
    let path = run.join("moves/decisions/mov_decided.json");
    let bytes = files::read(&path).unwrap().unwrap();
    assert_eq!(
        read(&path),
        json!({
            "as": "switched", "at": 7, "peer": peer, "last": "msg_saved", "generation": 2,
            "hash": "H".repeat(43), "payloads": {"sessions": [], "queue": []}, "formatVersion": 1,
        })
    );
    assert_eq!(
        read(&run.join("moves/decision-hints/mov_decided.json")),
        json!({
            "as": "switched", "at": 7, "sessions": ["ses_saved"], "formatVersion": 1,
        })
    );

    let restarted = Receipts::open(&run);
    assert_eq!(restarted.for_peer(id, "other").unwrap(), None);
    restarted.seal(id).unwrap();
    assert_eq!(
        restarted.for_peer(id, peer).unwrap(),
        Some(switched.clone())
    );
    let mut retry = switched;
    retry.at = 90;
    restarted.record(id, &retry, &sessions).unwrap();
    retry.bundle.generation = Some(3);
    assert!(matches!(
        restarted.record(id, &retry, &sessions),
        Err(ReceiptError::Conflict)
    ));
    retry.bundle.generation = Some(2);
    retry.decided = Decided::Dropped;
    assert!(matches!(
        restarted.record(id, &retry, &sessions),
        Err(ReceiptError::Conflict)
    ));
    assert_eq!(files::read(&path).unwrap().unwrap(), bytes);
}

#[test]
fn a_move_decided_by_an_older_computer_still_answers_after_its_routing_hint_has_expired() {
    let folder = tempfile::tempdir().unwrap();
    let run = folder.path().join("run");
    let path = run.join("moves/ended.json");
    let old = json!({ "mov_old": { "as": "dropped", "at": 1, "sessions": ["ses_old"] } });
    files::write_atomic(&path, &serde_json::to_vec(&old).unwrap()).unwrap();
    let receipts = Receipts::open(&run);
    let decision = receipts
        .for_peer("mov_old", "a known peer")
        .unwrap()
        .unwrap();
    assert_eq!(decision.decided, Decided::Dropped);
    assert_eq!(decision.bundle, Bundle::default());
    receipts.seal("mov_old").unwrap();
    receipts.record("mov_old", &decision, &[]).unwrap();
    assert_eq!(read(&path), old);
    assert!(!run.join("moves/decisions/mov_old.json").exists());
}

#[test]
fn protects_attachments_covers_active_moves_departures_and_recent_drops() {
    let folder = tempfile::tempdir().unwrap();
    let run = folder.path().join("run");
    let now = 1_760_000_000_000i64;
    let expired = now - 3 * 24 * 3_600_000;
    files::write_atomic(
        &run.join("moves/mov_active1.json"),
        &serde_json::to_vec(&json!({
            "id": "mov_active1", "thing": "ses_moving1", "peer": "other", "role": "from",
            "stage": "copying", "sessions": ["ses_moving1"], "started": now,
        }))
        .unwrap(),
    )
    .unwrap();
    files::write_atomic(
        &run.join("moves/moved.json"),
        &serde_json::to_vec(&json!([
            { "session": "ses_moved1", "key": "other", "at": now },
            { "session": "ses_oldMoved", "key": "other", "at": expired },
        ]))
        .unwrap(),
    )
    .unwrap();
    files::write_atomic(
        &run.join("moves/ended.json"),
        &serde_json::to_vec(&json!({
            "mov_dropLegacy": { "as": "dropped", "at": now, "sessions": ["ses_droppedLegacy"] },
            "mov_dropOld": { "as": "dropped", "at": expired, "sessions": ["ses_droppedOld"] },
        }))
        .unwrap(),
    )
    .unwrap();
    let receipts = Receipts::open(&run);
    receipts
        .record(
            "mov_dropModern",
            &Decision {
                decided: Decided::Dropped,
                at: now,
                peer: Some("other".into()),
                last: None,
                bundle: Bundle::default(),
                payloads: None,
            },
            &["ses_droppedModern".into()],
        )
        .unwrap();

    for session in [
        "ses_moving1",
        "ses_moved1",
        "ses_droppedLegacy",
        "ses_droppedModern",
    ] {
        assert!(
            kuvo_peers::moves::protects_attachments(&run, session, now),
            "{session} should stay protected"
        );
    }
    for session in ["ses_oldMoved", "ses_droppedOld", "ses_unrelated"] {
        assert!(
            !kuvo_peers::moves::protects_attachments(&run, session, now),
            "{session} should not stay protected"
        );
    }
    // Fail closed if a future-version drop hint cannot be read.
    files::write_atomic(
        &run.join("moves/decision-hints/mov_future.json"),
        &serde_json::to_vec(&json!({ "formatVersion": 99, "as": "dropped", "at": now, "sessions": [] })).unwrap(),
    )
    .unwrap();
    assert!(kuvo_peers::moves::protects_attachments(
        &run,
        "ses_unrelated",
        now
    ));
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_slice(&files::read(path).unwrap().unwrap()).unwrap()
}
