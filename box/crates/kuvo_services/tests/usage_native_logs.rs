//! The same incremental native JSONL reader used by the box, only temporary fixtures.
use kuvo_services::usage::native_logs::{Cursor, Kind, read};
use serde_json::json;
use std::{fs, io::Write};

fn line(value: serde_json::Value) -> String {
    format!("{value}\n")
}
#[test]
fn claude_only_complete_new_lines_advance_the_cursor_and_a_rewrite_is_detected() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("claude.jsonl");
    let entry = |id: &str, input: u64| json!({"timestamp":"2026-11-24T00:00:00Z","sessionId":"fixture","message":{"id":id,"model":"claude-opus-4-6","usage":{"input_tokens":input,"output_tokens":4,"cache_read_input_tokens":12,"cache_creation":{"ephemeral_5m_input_tokens":2,"ephemeral_1h_input_tokens":3}}}});
    let first = line(entry("one", 10));
    let incomplete = entry("two", 20).to_string();
    fs::write(&file, format!("{first}{incomplete}")).unwrap();
    let initial = read(&file, Kind::Claude, &Cursor::default(), 0)
        .unwrap()
        .unwrap();
    assert!(initial.rewritten);
    assert_eq!(initial.events.len(), 1);
    assert_eq!(initial.cursor.offset, first.len() as u64);
    assert_eq!(initial.events[0].1.counts.processed(), 31);
    let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
    writer.write_all(b"\n").unwrap();
    drop(writer);
    let changed = read(&file, Kind::Claude, &initial.cursor, 0)
        .unwrap()
        .unwrap();
    assert!(!changed.rewritten);
    assert_eq!(changed.events.len(), 1);
    assert_eq!(changed.events[0].1.message.as_deref(), Some("two"));
    assert_eq!(changed.events[0].1.counts.input, 20);
    assert!(
        read(&file, Kind::Claude, &changed.cursor, 0)
            .unwrap()
            .is_none()
    );
    fs::write(&file, line(entry("new", 8))).unwrap();
    let rewritten = read(&file, Kind::Claude, &changed.cursor, 0)
        .unwrap()
        .unwrap();
    assert!(rewritten.rewritten);
    assert_eq!(rewritten.events[0].1.message.as_deref(), Some("new"));
}
#[test]
fn codex_carries_totals_model_and_tier_across_a_saved_append_cursor() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("codex.jsonl");
    let counted = |input: u64, output: u64| {
        line(
            json!({"type":"event_msg","timestamp":"2026-11-24T00:01:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":10,"output_tokens":output}}}}),
        )
    };
    fs::write(&file,format!("{}{}{}{}",line(json!({"type":"session_meta","timestamp":"2026-11-24T00:00:00Z","payload":{"id":"thread"}})),line(json!({"type":"turn_context","payload":{"model":"gpt-5.5"}})),line(json!({"type":"event_msg","payload":{"type":"thread_settings_applied","service_tier":"priority"}})),counted(100,20))).unwrap();
    let first = read(&file, Kind::Codex, &Cursor::default(), 0)
        .unwrap()
        .unwrap();
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.events[0].1.counts.input, 90);
    let restored =
        serde_json::from_str::<Cursor>(&serde_json::to_string(&first.cursor).unwrap()).unwrap();
    let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
    write!(writer, "{}{}", counted(100, 20), counted(120, 30)).unwrap();
    drop(writer);
    let next = read(&file, Kind::Codex, &restored, 0).unwrap().unwrap();
    assert_eq!(next.events.len(), 1);
    assert_eq!(next.events[0].1.session.as_deref(), Some("thread"));
    assert_eq!(next.events[0].1.model.as_deref(), Some("gpt-5.5"));
    assert!(next.events[0].1.counts.fast);
    assert_eq!(next.events[0].1.counts.input, 20);
    assert_eq!(next.events[0].1.counts.output, 10);
}
#[test]
fn grok_counts_each_model_and_uses_its_reported_ticks() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("grok.jsonl");
    fs::write(&file,line(json!({"params":{"sessionId":"fixture","_meta":{"agentTimestampMs":1795478400000u64,"eventId":"call"},"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-a":{"inputTokens":100,"cachedReadTokens":30,"cacheCreationTokens":20,"outputTokens":4,"costUsdTicks":1000000000},"grok-b":{"inputTokens":30,"outputTokens":10}}}}}}))).unwrap();
    let next = read(&file, Kind::Grok, &Cursor::default(), 0)
        .unwrap()
        .unwrap();
    assert_eq!(next.events.len(), 2);
    let a = &next.events[0].1;
    assert_eq!(a.model.as_deref(), Some("grok-a"));
    assert_eq!(a.counts.input, 50);
    assert_eq!(a.counts.cache_read, 30);
    assert_eq!(a.counts.cache_write_5m, 20);
    assert_eq!(a.billed, Some(0.1));
    assert_ne!(
        a.dedup_key(Kind::Grok),
        next.events[1].1.dedup_key(Kind::Grok)
    );
}
