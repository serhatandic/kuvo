//! Durable letter retries through the actual files and mail envelope. The engine boundary stands
//! in for the normal idempotent submission endpoint.

use std::{collections::HashSet, fs};

use kuvo_peers::outbox::{Delivered, Delivery, Id, Letter, Outbox, Receipts, Settlement};
use serde_json::json;

const NOW: u64 = 1_760_000_000_000;
const DESTINATION: [u8; 32] = [8; 32];

fn id(value: &str) -> Id {
    value.to_owned().try_into().unwrap()
}

fn letter(at: u64, text: &str) -> Letter {
    serde_json::from_value(json!({ "to": "ses_test", "text": text, "at": at })).unwrap()
}

fn receipt(id: &Id) -> Delivered {
    serde_json::from_value(json!({ "id": id, "ok": true })).unwrap()
}

#[test]
fn a_letter_survives_both_restarts_and_an_unheard_acknowledgment() {
    let sender = tempfile::tempdir().unwrap();
    let receiver = tempfile::tempdir().unwrap();
    let id = id("ltr_repeat");
    let letter = letter(NOW, "How did the check go?");
    let mut outbox = Outbox::open(sender.path()).unwrap();
    assert!(outbox.enqueue(&id, &letter).unwrap());
    assert!(outbox.due(NOW, false).unwrap().is_empty());
    assert_eq!(outbox.due(NOW, true).unwrap()[0].id, id);

    let first = outbox.attempt(&id, &DESTINATION, NOW).unwrap().unwrap();
    assert_eq!(first.0["letter"]["text"], letter.text);
    assert!(outbox.due(NOW + 59_999, true).unwrap().is_empty());
    assert_eq!(
        outbox.settle(&[9; 32], &receipt(&id), NOW).unwrap(),
        Settlement::Ignored
    );

    let mut seen = Receipts::open(receiver.path()).unwrap();
    let Delivery::Submit { prompt } = seen.begin(&id, false, NOW).unwrap() else {
        panic!("the receiver has never seen the letter");
    };
    let mut engine = HashSet::from([prompt.clone()]);
    // The message reached the engine. The receiver went down before saving the receipt.
    drop(seen);
    drop(outbox);

    let mut outbox = Outbox::open(sender.path()).unwrap();
    let resent = outbox
        .attempt(&id, &DESTINATION, NOW + 60_000)
        .unwrap()
        .unwrap();
    assert_eq!(first.0["letter"], resent.0["letter"]);
    let mut seen = Receipts::open(receiver.path()).unwrap();
    let Delivery::Submit { prompt: retried } = seen.begin(&id, false, NOW + 60_000).unwrap() else {
        panic!("the engine's acceptance was unheard");
    };
    assert_eq!(retried, prompt);
    assert!(
        !engine.insert(retried),
        "the engine has this message ID already"
    );
    seen.accepted(&id, false, NOW + 60_000).unwrap();
    drop(seen);
    assert_eq!(
        Receipts::open(receiver.path())
            .unwrap()
            .begin(&id, false, NOW + 60_001)
            .unwrap(),
        Delivery::Seen
    );
    assert_eq!(engine.len(), 1);

    assert_eq!(
        outbox
            .settle(&DESTINATION, &receipt(&id), NOW + 60_000)
            .unwrap(),
        Settlement::Delivered
    );
    drop(outbox);
    let mut outbox = Outbox::open(sender.path()).unwrap();
    assert!(outbox.due(NOW + 120_000, true).unwrap().is_empty());
    assert!(!outbox.enqueue(&id, &letter).unwrap());
}

#[test]
fn expired_and_refused_letters_cannot_be_sent_from_the_outbox_again() {
    let run = tempfile::tempdir().unwrap();
    let expired = id("ltr_expired");
    let refused = id("ltr_refused");
    let mut outbox = Outbox::open(run.path()).unwrap();
    let letter = letter(NOW - 172_800_001, "Old");
    assert!(outbox.enqueue(&expired, &letter).unwrap());
    assert!(outbox.due(NOW, true).unwrap().is_empty());
    assert!(!outbox.enqueue(&expired, &letter).unwrap());

    outbox.enqueue(&refused, &self::letter(NOW, "New")).unwrap();
    outbox
        .attempt(&refused, &DESTINATION, NOW)
        .unwrap()
        .unwrap();
    let mut response = receipt(&refused);
    response.ok = false;
    response.retry = Some(true);
    assert_eq!(
        outbox.settle(&DESTINATION, &response, NOW).unwrap(),
        Settlement::Retry
    );
    assert_eq!(outbox.due(NOW + 60_000, true).unwrap()[0].id, refused);
    response.retry = Some(false);
    response.why = Some("That conversation doesn't take messages.".into());
    assert_eq!(
        outbox
            .settle(&DESTINATION, &response, NOW + 60_000)
            .unwrap(),
        Settlement::Refused
    );
    drop(outbox);
    assert!(
        Outbox::open(run.path())
            .unwrap()
            .due(NOW + 120_000, true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_cut_short_settlement_is_reconciled_before_the_next_send() {
    let run = tempfile::tempdir().unwrap();
    let id = id("ltr_interrupted");
    let mut outbox = Outbox::open(run.path()).unwrap();
    outbox
        .enqueue(&id, &letter(NOW, "Do not send again"))
        .unwrap();
    drop(outbox);
    // Cut power after writing the terminal state, before removing the pending entry.
    let sending = run.path().join("outbox/sending");
    fs::write(
        sending.join("ltr_interrupted.failed"),
        br#"{"letter":{},"why":"Refused"}"#,
    )
    .unwrap();
    let mut outbox = Outbox::open(run.path()).unwrap();
    assert!(outbox.due(NOW, true).unwrap().is_empty());
    assert!(!sending.join("ltr_interrupted.json").exists());
    assert!(sending.join("ltr_interrupted.failed").exists());
}

#[test]
fn legacy_letters_are_sent_oldest_first_and_never_split_a_character() {
    let run = tempfile::tempdir().unwrap();
    let sending = run.path().join("outbox/sending");
    fs::create_dir_all(&sending).unwrap();
    let first_id = id("ltr_first");
    let mut first = letter(NOW, &format!("{}{}", "x".repeat(23_999), "👋"));
    first.wait = true;
    fs::write(
        sending.join("ltr_first.json"),
        serde_json::to_vec(&first).unwrap(),
    )
    .unwrap();
    let mut outbox = Outbox::open(run.path()).unwrap();
    let second_id = id("ltr_second");
    outbox
        .enqueue(&second_id, &letter(NOW + 1, "second"))
        .unwrap();
    let ids: Vec<_> = outbox
        .due(NOW + 1, true)
        .unwrap()
        .into_iter()
        .map(|pending| pending.id)
        .collect();
    assert_eq!(ids, [first_id.clone(), second_id]);
    let wire = outbox
        .attempt(&first_id, &DESTINATION, NOW + 1)
        .unwrap()
        .unwrap();
    assert_eq!(wire.0["letter"]["text"], "x".repeat(23_999));
    assert_eq!(
        outbox
            .settle(&DESTINATION, &receipt(&first_id), NOW + 1)
            .unwrap(),
        Settlement::WaitingForAnswer
    );
    assert!(run.path().join("replies/ltr_first.wait").exists());
    assert!(!outbox.enqueue(&first_id, &first).unwrap());
}

#[test]
fn a_note_reseals_on_retry_and_is_remembered_longer_than_an_ordinary_letter() {
    let sender = tempfile::tempdir().unwrap();
    let receiver = tempfile::tempdir().unwrap();
    let mut outbox = Outbox::open(sender.path()).unwrap();
    let id = id("ltr_note");
    let mut note = letter(NOW, "The conversation moved");
    note.note = Some(json!({ "words": note.text }).as_object().unwrap().clone());
    assert!(outbox.enqueue(&id, &note).unwrap());
    let sent = outbox
        .attempt(&id, &DESTINATION, NOW + 3 * 86_400_000)
        .unwrap()
        .unwrap();
    assert_eq!(sent.0["letter"]["made"], NOW);
    assert_eq!(sent.0["letter"]["at"], NOW + 3 * 86_400_000);

    let mut receipts = Receipts::open(receiver.path()).unwrap();
    receipts.accepted(&id, true, NOW).unwrap();
    let ordinary = self::id("ltr_old_letter");
    receipts.accepted(&ordinary, false, NOW).unwrap();
    assert_eq!(
        receipts.begin(&id, true, NOW + 3 * 86_400_000).unwrap(),
        Delivery::Seen
    );
    assert!(matches!(
        receipts
            .begin(&ordinary, false, NOW + 3 * 86_400_000)
            .unwrap(),
        Delivery::Submit { .. }
    ));
}

#[test]
fn a_refusal_becomes_one_failure_notice_even_if_the_resident_restarts() {
    use kuvo_peers::outbox::FailureContext;
    let run = tempfile::tempdir().unwrap();
    let id = id("ltr_refusal");
    let mut sent = letter(NOW, "What happened?");
    sent.from = Some("ses_sender".into());
    let mut outbox = Outbox::open(run.path()).unwrap();
    outbox.enqueue(&id, &sent).unwrap();
    outbox.attempt(&id, &DESTINATION, NOW).unwrap().unwrap();
    let mut receipt = receipt(&id);
    receipt.ok = false;
    receipt.why = Some("That conversation doesn't take messages.".into());
    assert_eq!(
        outbox.settle(&DESTINATION, &receipt, NOW).unwrap(),
        Settlement::Refused
    );
    drop(outbox);

    let mut outbox = Outbox::open(run.path()).unwrap();
    let where_it_went = |_: &Id, _: &Letter| FailureContext {
        conversation: "Planning".into(),
        computer: "Studio".into(),
        tool_waiting: false,
    };
    for now in [NOW, NOW + 1] {
        outbox
            .finish_failures(&[7; 32], now, where_it_went)
            .unwrap();
    }
    let queued = outbox.due(NOW + 2, true).unwrap();
    assert_eq!(queued.len(), 1);
    assert_ne!(queued[0].id, id);
    let notice = &queued[0].letter;
    assert_eq!(notice.to, "ses_sender");
    assert_eq!(
        notice.text,
        "[Not delivered] Your message to Planning on Studio wasn't delivered: Studio said \"That conversation doesn't take messages.\""
    );
    assert_eq!(notice.note.as_ref().unwrap()["words"], notice.text);
}

#[test]
fn a_mail_replayed_after_one_day_is_refused_before_delivery() {
    use kuvo_peers::outbox::IncomingLetter;
    let at = NOW - 86_400_001;
    let mut wire = json!({"id":"ltr_old","to":"ses_a","text":"A replay","wait":false,"at":at});
    let letter: IncomingLetter = serde_json::from_value(wire.clone()).unwrap();
    let refusal = letter.refusal(NOW).unwrap();
    assert_eq!((refusal.ok, refusal.retry), (false, None));
    assert_eq!(refusal.why.as_deref(), Some("It's too old to deliver."));
    wire["note"] = json!({"from":"Your other computers","words":"A replay","delivery":"queue"});
    let letter: IncomingLetter = serde_json::from_value(wire).unwrap();
    assert_eq!(letter.refusal(NOW).unwrap().retry, Some(true));
}

#[test]
fn failed_relay_send_does_not_block_immediate_retry() {
    let run = tempfile::tempdir().unwrap();
    let id = id("ltr_retry01");
    let mut outbox = Outbox::open(run.path()).unwrap();
    outbox.enqueue(&id, &letter(NOW, "Hello")).unwrap();
    assert!(outbox.attempt(&id, &DESTINATION, NOW).unwrap().is_some());
    assert!(outbox.due(NOW + 1_000, true).unwrap().is_empty());

    outbox.cancel_try(&id);
    assert_eq!(outbox.due(NOW + 1_000, true).unwrap().len(), 1);
    assert!(
        outbox
            .attempt(&id, &DESTINATION, NOW + 1_000)
            .unwrap()
            .is_some()
    );
}
