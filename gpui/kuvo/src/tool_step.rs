//! Human-language presentation of non-engine tool steps (Kuvo tools and MCP tools).
//!
//! Only what a person would say is shown:
//! - Closed rows pair the tool's icon with plain words naming the target naturally
//!   (e.g. `"Messaged Mira"`, `"#231 → In review"`, `"Wake-up today 19:30 · Check whether the
//!   release build passed"`, `"Canceled the wake-up · Check groundwork branch"`).
//! - Opened details show only the human content (the message text, the Work Queue note,
//!   the wake-up's note; for unknown MCP tools, the main text argument and main text result).
//! - Internal IDs (`ses_…`, `wake_…`, request IDs, UUIDs), raw parameter names/flags
//!   (`to`, `wait`, `true`/`false`, `item`, `status`, `summary`), `"Sent"`/`"Returned"`
//!   headings, boilerplate confirmations (`"Delivered"`, `"Scheduled …"`), relative deltas
//!   (`"+2h"`), and timezone offsets (`"(+03:00)"`) are never shown.

use gpui::SharedString;
use kuvo_ui::IconName;
use serde_json::Value;

use crate::time;

/// How a tool step's closed row reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presentation {
    pub icon: IconName,
    pub label: SharedString,
    pub target: Option<SharedString>,
    pub target_mono: bool,
}

/// Human-only content shown when a Kuvo or MCP tool step is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub primary: String,
    pub secondary: Option<String>,
}

/// Whether `name` is one of the coding engine's built-in tools (whose labels and outputs/diffs
/// are defined by `protocol/vectors/tool-steps.json` in [`crate::entries`]).
pub fn is_engine_tool(name: &str) -> bool {
    matches!(
        name,
        "shell"
            | "bash"
            | "read"
            | "edit"
            | "multiedit"
            | "patch"
            | "apply_patch"
            | "write"
            | "grep"
            | "glob"
            | "list"
            | "ls"
            | "webfetch"
            | "websearch"
            | "task"
            | "subagent"
            | "execute"
            | "skill"
            | "question"
            | "codesearch"
            | "lsp"
            | "batch"
            | "plan_enter"
            | "plan_exit"
            | "context"
            | "invalid"
            | "todowrite"
            | "todoread"
    )
}

/// Strips the `kuvo-browser_` (or `kuvo_`) MCP prefix when matching known Kuvo tools.
fn kuvo_tool_name(name: &str) -> &str {
    name.strip_prefix("kuvo-browser_")
        .or_else(|| name.strip_prefix("kuvo_"))
        .unwrap_or(name)
}

/// Joins text blocks from `state.content`.
pub fn printed(state: &Value) -> String {
    let blocks = state
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let texts: Vec<&str> = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    texts.join("\n")
}

/// Plain error text for a failed tool step, without a repeated `"Error:"` prefix or internal IDs.
pub fn clean_error(raw: &str) -> String {
    let mut text = raw.trim();
    loop {
        let lower = text.to_ascii_lowercase();
        let next = [
            "mcp error:",
            "tool error:",
            "error:",
            "failed:",
            "exception:",
        ]
        .iter()
        .find_map(|prefix| lower.starts_with(prefix).then(|| text[prefix.len()..].trim_start()));
        match next {
            Some(stripped) if !stripped.is_empty() => text = stripped,
            _ => break,
        }
    }
    strip_parenthetical_ids(text)
}

/// Extracts and cleans the error message of a failed tool step.
pub fn tool_error(state: &Value) -> Option<String> {
    let raw = state
        .get("error")
        .and_then(|error| {
            error
                .as_str()
                .map(str::to_owned)
                .or_else(|| error.get("message").and_then(Value::as_str).map(str::to_owned))
        })
        .or_else(|| {
            let status = state.get("status").and_then(Value::as_str).unwrap_or("");
            if status == "failed" || status == "error" {
                let text = printed(state);
                (!text.trim().is_empty()).then_some(text)
            } else {
                None
            }
        })?;
    let cleaned = clean_error(&raw);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Closed-row presentation for a non-engine tool call (Kuvo tools and unknown MCP tools).
/// Returns `None` for built-in engine tools and `show_file`, which `entries.rs` handles directly.
pub fn present(
    name: &str,
    title: Option<&str>,
    doing: Option<&str>,
    state: &Value,
    busy: bool,
    created_ms: f64,
    now_ms: f64,
) -> Option<Presentation> {
    if is_engine_tool(name) || name == "kuvo-browser_show_file" {
        return None;
    }
    let input = state.get("input").unwrap_or(&Value::Null);
    match kuvo_tool_name(name) {
        "send_message" => Some(present_send_message(title, state, input, busy)),
        "queue_update" => Some(present_queue_update(input, busy)),
        "queue_add" => Some(present_queue_add(state, input, busy)),
        "queue_hand" => Some(present_queue_hand(input, busy)),
        "queue_remove" => Some(present_queue_remove(input, busy)),
        "queue_show" => Some(present_queue_show(input, busy)),
        "queue_list" => Some(present_queue_list(input, busy)),
        "schedule_wake_up" => Some(present_schedule_wake_up(state, input, busy, created_ms, now_ms)),
        "cancel_wake_up" => Some(present_cancel_wake_up(state, input, busy)),
        "watch_for_change" => Some(present_watch(input, busy)),
        "notify_user" => Some(present_notify_user(input, busy)),
        "read_conversation" => Some(present_read_conversation(title, input, busy)),
        _ => Some(present_unknown(name, title, doing, input, busy)),
    }
}

/// Human-only details shown when a non-engine tool step is opened.
pub fn note(name: &str, state: &Value) -> Option<Note> {
    if is_engine_tool(name) || name == "kuvo-browser_show_file" {
        return None;
    }
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");
    if matches!(status, "running" | "streaming" | "pending") {
        return None;
    }
    let input = state.get("input").unwrap_or(&Value::Null);
    match kuvo_tool_name(name) {
        "send_message" => note_send_message(state, input),
        "queue_update" => note_from_fields(input, &["note", "progress_note", "progressNote", "body", "comment", "description"]),
        "queue_add" => note_from_fields(input, &["body", "description", "note"]),
        "queue_hand" => note_from_fields(input, &["note", "message", "reason"]),
        "queue_remove" | "queue_show" | "queue_list" => None,
        "schedule_wake_up" => note_schedule_wake_up(input),
        "cancel_wake_up" => None,
        "watch_for_change" => note_watch(input),
        "notify_user" => note_from_fields(input, &["body", "message", "text", "note"]),
        "read_conversation" => None,
        "start_session" => note_from_fields(input, &["prompt", "message", "text", "task"]),
        "stop_session" | "compact_session" | "list_sessions" | "list_agents" | "list_wake_ups"
        | "list_models" | "list_projects" | "plan_limits" | "use_limit_reset" | "queue_labels"
        | "vault_items" => None,
        _ => note_unknown(state, input),
    }
}

// MARK: Known Kuvo tools

fn present_send_message(title: Option<&str>, state: &Value, input: &Value, busy: bool) -> Presentation {
    let recipient = resolve_recipient(title, state, input);
    let label = match (busy, recipient) {
        (true, Some(name)) => format!("Messaging {name}"),
        (false, Some(name)) => format!("Messaged {name}"),
        (true, None) => "Messaging an agent".to_owned(),
        (false, None) => "Messaged an agent".to_owned(),
    };
    Presentation {
        icon: IconName::Send,
        label: label.into(),
        target: None,
        target_mono: false,
    }
}

fn note_send_message(state: &Value, input: &Value) -> Option<Note> {
    let primary = first_text_field(input, &["text", "message", "body", "content"])?;
    let reply = extract_message_reply(state);
    Some(Note { primary, secondary: reply })
}

fn extract_message_reply(state: &Value) -> Option<String> {
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");
    if status == "failed" || status == "error" {
        return None;
    }
    let raw = printed(state);
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_confirmation(trimmed) {
        return None;
    }
    // `send_message(wait: true)` prints "<Name> answered:\n\n<reply>".
    if let Some((head, rest)) = trimmed.split_once('\n') {
        if head.trim_end().ends_with("answered:") {
            let reply = rest.trim();
            if !reply.is_empty() && !is_confirmation(reply) {
                return Some(reply.to_owned());
            }
            return None;
        }
    }
    if contains_internal_id(trimmed) {
        return None;
    }
    Some(trimmed.to_owned())
}

fn resolve_recipient(title: Option<&str>, state: &Value, input: &Value) -> Option<String> {
    for field in ["to", "recipient", "agent", "bot", "name"] {
        if let Some(raw) = input.get(field).and_then(Value::as_str) {
            if let Some(name) = clean_person(raw) {
                return Some(name);
            }
        }
    }
    for field in ["toName", "to_name", "recipient", "name"] {
        if let Some(raw) = state.pointer(&format!("/metadata/{field}")).and_then(Value::as_str) {
            if let Some(name) = clean_person(raw) {
                return Some(name);
            }
        }
    }
    if let Some(title) = title {
        for prefix in ["Sent a message to ", "Sending a message to ", "Messaged ", "Messaging "] {
            if let Some(rest) = title.strip_prefix(prefix) {
                if let Some(name) = clean_person(rest) {
                    return Some(name);
                }
            }
        }
    }
    let out = printed(state);
    let first = out.lines().next().unwrap_or("").trim();
    if let Some(rest) = first.strip_prefix("Delivered to ") {
        let rest = rest.trim_end_matches('.');
        if let Some(name) = clean_person(rest) {
            return Some(name);
        }
    }
    if let Some(who) = first.strip_suffix(" answered:") {
        if let Some(name) = clean_person(who) {
            return Some(name);
        }
    }
    None
}

fn present_queue_update(input: &Value, busy: bool) -> Presentation {
    let item = format_item_number(input);
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(format_queue_status);
    let title = first_short_field(input, &["title"]);
    let working = input.get("working").and_then(Value::as_bool);
    let has_note = first_text_field(input, &["note", "progress_note", "progressNote", "body", "comment"]).is_some();

    let (label, target) = match (item, status, working) {
        (Some(item), Some(status), _) => (format!("{item} → {status}"), title.map(|t| format!("· {t}"))),
        (Some(item), None, Some(true)) => (format!("Started {item}"), title.map(|t| format!("· {t}"))),
        (Some(item), None, Some(false)) => (format!("Paused {item}"), title.map(|t| format!("· {t}"))),
        (Some(item), None, None) if title.is_some() => (item, title.map(|t| format!("· {t}"))),
        (Some(item), None, None) if has_note => (format!("Noted on {item}"), None),
        (Some(item), None, None) => (if busy { format!("Updating {item}") } else { format!("Updated {item}") }, None),
        (None, Some(status), _) => (format!("Work Queue → {status}"), title.map(|t| format!("· {t}"))),
        (None, None, _) => (
            if busy { "Updating Work Queue" } else { "Updated Work Queue" }.to_owned(),
            title.map(|t| format!("· {t}")),
        ),
    };
    Presentation {
        icon: IconName::Queue,
        label: label.into(),
        target: target.map(SharedString::from),
        target_mono: false,
    }
}

fn present_queue_add(state: &Value, input: &Value, busy: bool) -> Presentation {
    let item = state
        .pointer("/metadata/number")
        .and_then(Value::as_u64)
        .map(|n| format!("#{n}"))
        .or_else(|| extract_hash_number(&printed(state)));
    let title = first_short_field(input, &["title"]);
    let label = match (busy, item) {
        (true, _) => "Adding to Work Queue".to_owned(),
        (false, Some(item)) => format!("Added {item}"),
        (false, None) => "Added to Work Queue".to_owned(),
    };
    Presentation {
        icon: IconName::Queue,
        label: label.into(),
        target: title.map(|t| SharedString::from(format!("· {t}"))),
        target_mono: false,
    }
}

fn present_queue_hand(input: &Value, busy: bool) -> Presentation {
    let item = format_item_number(input);
    let to = ["to", "assignee", "agent", "bot"]
        .iter()
        .find_map(|field| input.get(*field).and_then(Value::as_str).and_then(clean_person));
    let verb = if busy { "Handing" } else { "Handed" };
    let label = match (item, to) {
        (Some(item), Some(to)) => format!("{verb} {item} to {to}"),
        (Some(item), None) => format!("{verb} {item}"),
        (None, Some(to)) => format!("{verb} to {to}"),
        (None, None) => if busy { "Handing off Work Queue item" } else { "Handed off Work Queue item" }.to_owned(),
    };
    Presentation {
        icon: IconName::Queue,
        label: label.into(),
        target: None,
        target_mono: false,
    }
}

fn present_queue_remove(input: &Value, busy: bool) -> Presentation {
    let label = match format_item_number(input) {
        Some(item) => format!("{} {item}", if busy { "Removing" } else { "Removed" }),
        None => if busy { "Removing from Work Queue" } else { "Removed from Work Queue" }.to_owned(),
    };
    Presentation {
        icon: IconName::Queue,
        label: label.into(),
        target: None,
        target_mono: false,
    }
}

fn present_queue_show(input: &Value, busy: bool) -> Presentation {
    let label = match format_item_number(input) {
        Some(item) => format!("{} {item}", if busy { "Checking" } else { "Checked" }),
        None => if busy { "Checking Work Queue" } else { "Checked Work Queue" }.to_owned(),
    };
    Presentation {
        icon: IconName::Queue,
        label: label.into(),
        target: None,
        target_mono: false,
    }
}

fn present_queue_list(input: &Value, busy: bool) -> Presentation {
    let filter = input
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(format_queue_status)
        .or_else(|| first_short_field(input, &["query", "search", "label"]));
    Presentation {
        icon: IconName::Queue,
        label: if busy { "Checking Work Queue" } else { "Checked Work Queue" }.into(),
        target: filter.map(|f| SharedString::from(format!("· {f}"))),
        target_mono: false,
    }
}

fn present_schedule_wake_up(
    state: &Value,
    input: &Value,
    busy: bool,
    created_ms: f64,
    now_ms: f64,
) -> Presentation {
    let when = resolve_wake_when(state, input, created_ms, now_ms);
    let summary = wake_summary(input);
    let label = match (busy, when) {
        (_, Some(when)) => format!("Wake-up {when}"),
        (true, None) => "Scheduling a wake-up".to_owned(),
        (false, None) => "Wake-up".to_owned(),
    };
    Presentation {
        icon: IconName::Alarm,
        label: label.into(),
        target: summary.map(|s| SharedString::from(format!("· {s}"))),
        target_mono: false,
    }
}

fn wake_summary(input: &Value) -> Option<String> {
    first_short_field(input, &["summary", "title"]).or_else(|| {
        first_text_field(input, &["note", "message"])
            .map(|note| single_line_excerpt(&note, 80))
            .filter(|line| !line.is_empty())
    })
}

fn note_schedule_wake_up(input: &Value) -> Option<Note> {
    let note = first_text_field(input, &["note", "message"])?;
    // Don't open if the entire note is already displayed verbatim on the closed row.
    if input.get("summary").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).is_none() {
        if let Some(summary) = wake_summary(input) {
            if summary == note {
                return None;
            }
        }
    }
    Some(Note { primary: note, secondary: None })
}

fn present_cancel_wake_up(state: &Value, input: &Value, busy: bool) -> Presentation {
    let summary = resolve_cancel_wake_summary(state, input);
    let label = if busy { "Canceling the wake-up" } else { "Canceled the wake-up" };
    Presentation {
        icon: IconName::Alarm,
        label: label.into(),
        target: summary.map(|s| SharedString::from(format!("· {s}"))),
        target_mono: false,
    }
}

fn resolve_cancel_wake_summary(state: &Value, input: &Value) -> Option<String> {
    if let Some(s) = first_short_field(input, &["summary", "title", "note"]) {
        return Some(s);
    }
    for field in ["summary", "title", "note"] {
        if let Some(raw) = state.pointer(&format!("/metadata/{field}")).and_then(Value::as_str) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() && !is_internal_id(trimmed) {
                return Some(single_line_excerpt(trimmed, 80));
            }
        }
    }
    // Check printed output for human summary attached to a cancel confirmation:
    // e.g. "Canceled wake_4b19 (Check groundwork branch)" or "Canceled: Check groundwork branch".
    let out = printed(state);
    let line = out.lines().next().unwrap_or("").trim().trim_end_matches('.');
    if let Some(paren) = extract_human_parenthetical(line) {
        return Some(paren);
    }
    if let Some(quoted) = extract_quoted_phrase(line) {
        return Some(quoted);
    }
    if let Some((_, after)) = line.split_once(':') {
        let after = after.trim();
        if !after.is_empty() && !is_internal_id(after) && !contains_internal_id(after) {
            return Some(single_line_excerpt(after, 80));
        }
    }
    None
}

fn present_watch(input: &Value, busy: bool) -> Presentation {
    let summary = first_short_field(input, &["summary", "title", "note"]);
    Presentation {
        icon: IconName::Alarm,
        label: if busy { "Setting up a watch" } else { "Watch" }.into(),
        target: summary.map(|s| SharedString::from(format!("· {s}"))),
        target_mono: false,
    }
}

fn note_watch(input: &Value) -> Option<Note> {
    let note = first_text_field(input, &["note", "check", "prompt"])?;
    if first_short_field(input, &["summary", "title"]).as_deref() == Some(note.as_str()) {
        return None;
    }
    Some(Note { primary: note, secondary: None })
}

fn present_notify_user(input: &Value, busy: bool) -> Presentation {
    let title = first_short_field(input, &["title", "summary"]);
    Presentation {
        icon: IconName::Bell,
        label: if busy { "Notifying you" } else { "Notified you" }.into(),
        target: title.map(|t| SharedString::from(format!("· {t}"))),
        target_mono: false,
    }
}

fn present_read_conversation(title: Option<&str>, input: &Value, busy: bool) -> Presentation {
    let who = ["agent", "bot", "name", "to", "session"]
        .iter()
        .find_map(|field| input.get(*field).and_then(Value::as_str).and_then(clean_person));
    let label = match (busy, who) {
        (true, Some(name)) => format!("Reading {name}’s conversation"),
        (false, Some(name)) => format!("Read {name}’s conversation"),
        (true, None) => title.unwrap_or("Reading a conversation").to_owned(),
        (false, None) => title.unwrap_or("Read a conversation").to_owned(),
    };
    Presentation {
        icon: IconName::Message,
        label: label.into(),
        target: None,
        target_mono: false,
    }
}

fn note_from_fields(input: &Value, fields: &[&str]) -> Option<Note> {
    first_text_field(input, fields).map(|primary| Note { primary, secondary: None })
}

// MARK: Unknown / MCP tool fallback

/// Summarizes an unknown or third-party MCP tool for the closed row:
/// - Icon: `Globe` for browser tools, `Sliders` otherwise.
/// - Verb: the box's plain title (`doing` while running, `title` when done), falling back to
///   `"Using a tool"` / `"Used a tool"` (never the raw tool ID).
/// - Target: the single most meaningful short string argument (skipping internal IDs,
///   technical/flag keys, non-strings, JSON blobs, and multiline or long text).
fn present_unknown(
    name: &str,
    title: Option<&str>,
    doing: Option<&str>,
    input: &Value,
    busy: bool,
) -> Presentation {
    let icon = unknown_icon(name);
    let label = if busy {
        doing.or(title).unwrap_or("Using a tool")
    } else {
        title.unwrap_or("Used a tool")
    };
    let short_arg = pick_short_arg(input);
    let target = short_arg.map(|arg| {
        if label.contains(" · ") {
            SharedString::from(arg)
        } else {
            SharedString::from(format!("· {arg}"))
        }
    });
    Presentation {
        icon,
        label: SharedString::from(label.to_owned()),
        target,
        target_mono: false,
    }
}

fn unknown_icon(name: &str) -> IconName {
    let short = kuvo_tool_name(name);
    if short.starts_with("browser_") {
        IconName::Globe
    } else if short.starts_with("vault_") || short.starts_with("sandbox_") {
        IconName::Shield
    } else if short.contains("session") || short.contains("agent") {
        IconName::Bot
    } else if short.contains("wake") {
        IconName::Alarm
    } else {
        IconName::Sliders
    }
}

/// Summarizes an unknown or third-party MCP tool when opened:
/// main text argument + main text result only, never a key/value dump or raw JSON blob.
fn note_unknown(state: &Value, input: &Value) -> Option<Note> {
    let short_arg = pick_short_arg(input);
    let arg = pick_main_text_arg(input, short_arg.as_deref());
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");
    let result = if status == "failed" || status == "error" {
        None
    } else {
        pick_main_text_result(state, short_arg.as_deref(), arg.as_deref())
    };
    match (arg, result) {
        (Some(primary), secondary) => Some(Note { primary, secondary }),
        (None, Some(primary)) => Some(Note { primary, secondary: None }),
        (None, None) => None,
    }
}

/// Priority keys for a closed row's single short argument on unknown tools.
const SHORT_ARG_KEYS: &[&str] = &[
    "title", "name", "subject", "summary", "query", "q", "search", "url", "repo", "repository",
    "project", "channel", "branch", "path", "file", "target", "to", "label", "team", "topic",
    "issue", "pr", "command",
];

/// Priority keys for an opened row's main human text argument on unknown tools.
const LONG_ARG_KEYS: &[&str] = &[
    "description", "body", "text", "content", "message", "note", "prompt", "comment", "details",
    "markdown", "instructions", "reason", "summary", "query",
];

fn pick_short_arg(input: &Value) -> Option<String> {
    let map = input.as_object()?;
    for key in SHORT_ARG_KEYS {
        if let Some(val) = map.get(*key).and_then(Value::as_str) {
            if let Some(clean) = clean_short_value(val) {
                return Some(clean);
            }
        }
    }
    for (key, val) in map {
        if is_internal_key(key) {
            continue;
        }
        if let Some(val) = val.as_str() {
            if let Some(clean) = clean_short_value(val) {
                return Some(clean);
            }
        }
    }
    None
}

fn pick_main_text_arg(input: &Value, short_arg: Option<&str>) -> Option<String> {
    let map = input.as_object()?;
    for key in LONG_ARG_KEYS {
        if let Some(val) = map.get(*key).and_then(Value::as_str) {
            if let Some(clean) = clean_long_value(val) {
                if short_arg != Some(clean.as_str()) {
                    return Some(clean);
                }
            }
        }
    }
    // Fallback: longest non-ID, non-JSON human string argument not already shown on the closed row.
    map.iter()
        .filter(|(key, _)| !is_internal_key(key))
        .filter_map(|(_, val)| val.as_str().and_then(clean_long_value))
        .filter(|val| short_arg != Some(val.as_str()))
        .filter(|val| val.contains(char::is_whitespace) || val.chars().count() > 24)
        .max_by_key(|val| val.chars().count())
}

fn pick_main_text_result(state: &Value, short_arg: Option<&str>, main_arg: Option<&str>) -> Option<String> {
    let raw = printed(state);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // If an MCP tool returned raw JSON, pull out a single human text field rather than dumping JSON.
    let candidate = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        extract_human_from_json(trimmed)?
    } else {
        let kept: Vec<&str> = trimmed
            .lines()
            .map(str::trim_end)
            .filter(|line| {
                let t = line.trim();
                !is_internal_id(t) && !is_confirmation(t)
            })
            .collect();
        kept.join("\n").trim().to_owned()
    };
    if candidate.is_empty()
        || is_confirmation(&candidate)
        || is_internal_id(&candidate)
        || short_arg == Some(candidate.as_str())
        || main_arg == Some(candidate.as_str())
    {
        return None;
    }
    Some(strip_parenthetical_ids(&candidate))
}

fn extract_human_from_json(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object()?;
    for key in ["message", "summary", "text", "content", "result", "description", "title", "url"] {
        if let Some(val) = obj.get(key).and_then(Value::as_str) {
            let trimmed = val.trim();
            if !trimmed.is_empty() && !is_internal_id(trimmed) && !is_confirmation(trimmed) {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

fn clean_short_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('\n')
        || trimmed.chars().count() < 2
        || trimmed.chars().count() > 90
        || looks_like_json(trimmed)
        || is_flag_word(trimmed)
        || is_internal_id(trimmed)
    {
        return None;
    }
    let cleaned = strip_parenthetical_ids(trimmed);
    (!cleaned.is_empty() && !is_internal_id(&cleaned)).then_some(cleaned)
}

fn clean_long_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || looks_like_json(trimmed) || is_flag_word(trimmed) || is_internal_id(trimmed) {
        return None;
    }
    Some(trimmed.to_owned())
}

// MARK: Human time formatting for wake-ups

fn resolve_wake_when(state: &Value, input: &Value, created_ms: f64, now_ms: f64) -> Option<String> {
    let anchor = if created_ms > 0.0 { created_ms } else { now_ms };
    if let Some(raw) = input.get("when").and_then(Value::as_str) {
        if let Some(when) = human_when(raw, anchor, now_ms) {
            return Some(when);
        }
    }
    if let Some(due) = state.pointer("/metadata/due").and_then(Value::as_f64) {
        let due_ms = if due < 100_000_000_000.0 { due * 1000.0 } else { due };
        return Some(format_local_ms(due_ms, now_ms));
    }
    // `kuvo-wake` prints "Scheduled <epoch>-<rand> for ..."
    let out = printed(state);
    if let Some(epoch_ms) = extract_scheduled_epoch_ms(&out) {
        return Some(format_local_ms(epoch_ms, now_ms));
    }
    if let Some(every) = input.get("every").and_then(Value::as_str) {
        if let Some(phrase) = human_every(every) {
            return Some(phrase);
        }
    }
    None
}

/// Formats a wake-up time expression in plain local form (`"today 19:30"`, `"tomorrow 09:00"`,
/// `"Nov 24 09:00"`), never relative deltas (`"+2h"`) and never timezone offsets (`"(+03:00)"`).
pub fn human_when(raw: &str, anchor_ms: f64, now_ms: f64) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 1. Relative delta: "+2h", "2h", "+30m", "+1h30m", "+1d", "in 2h", "in 30 min".
    if let Some(delta_secs) = parse_relative_seconds(trimmed) {
        return Some(format_local_ms(anchor_ms + delta_secs as f64 * 1000.0, now_ms));
    }
    // 2. ISO-8601 date + time: "2026-11-19T19:30", "2026-11-19T16:30:00Z", "2026-11-19T19:30:00+03:00".
    if let Some(formatted) = parse_iso_datetime(trimmed, now_ms) {
        return Some(formatted);
    }
    // 3. Explicit day word + clock, or bare clock: "today 19:30", "today at 19:30", "tomorrow 09:00", "19:30".
    if let Some(formatted) = parse_day_clock(trimmed, anchor_ms, now_ms) {
        return Some(formatted);
    }
    None
}

fn parse_relative_seconds(raw: &str) -> Option<i64> {
    let s = raw
        .trim()
        .strip_prefix("in ")
        .or_else(|| raw.trim().strip_prefix("In "))
        .unwrap_or(raw.trim());
    let s = s.strip_prefix('+').unwrap_or(s).trim();
    if s.is_empty() || !s.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    // Reject bare HH:MM or YYYY-MM-DD.
    if s.contains(':') || s.contains('-') {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut total: i64 = 0;
    let mut parsed_any = false;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let num_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == num_start {
            return None;
        }
        let amount: i64 = s[num_start..i].parse().ok()?;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let unit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let unit = s[unit_start..i].to_ascii_lowercase();
        let mult = match unit.as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86_400,
            "w" | "wk" | "wks" | "week" | "weeks" => 7 * 86_400,
            _ => return None,
        };
        total = total.checked_add(amount.checked_mul(mult)?)?;
        parsed_any = true;
    }
    (parsed_any && total > 0).then_some(total)
}

fn parse_day_clock(raw: &str, anchor_ms: f64, now_ms: f64) -> Option<String> {
    let lower = raw.trim().to_ascii_lowercase();
    let (day_hint, clock_part) = if let Some(rest) = lower.strip_prefix("today") {
        (Some(0i64), rest.trim().strip_prefix("at ").unwrap_or(rest.trim()))
    } else if let Some(rest) = lower.strip_prefix("tomorrow") {
        (Some(1i64), rest.trim().strip_prefix("at ").unwrap_or(rest.trim()))
    } else if let Some(rest) = lower.strip_prefix("yesterday") {
        (Some(-1i64), rest.trim().strip_prefix("at ").unwrap_or(rest.trim()))
    } else {
        (None, lower.as_str())
    };
    let (hh, mm) = parse_hh_mm(clock_part)?;
    let anchor_sec = (anchor_ms / 1000.0).floor() as i64;
    let anchor_local = anchor_sec + crate::pasted::utc_offset(anchor_sec);
    let anchor_day = anchor_local.div_euclid(86_400);
    let anchor_min = (anchor_local.rem_euclid(86_400)) / 60;

    let target_day = match day_hint {
        Some(delta) => anchor_day + delta,
        None => {
            if hh * 60 + mm > anchor_min {
                anchor_day
            } else {
                anchor_day + 1
            }
        }
    };
    let now_day = local_day(now_ms);
    Some(format_local_day_time(target_day, hh, mm, now_day))
}

fn parse_iso_datetime(raw: &str, now_ms: f64) -> Option<String> {
    let (date_part, rest) = raw.split_once('T').or_else(|| raw.split_once(' '))?;
    let day = time::parse_day(date_part)?;
    // Strip optional trailing timezone ("Z", "+03:00", "-0500") or fractional seconds.
    let rest = rest.trim();
    let (time_core, tz_offset) = split_iso_tz(rest)?;
    let mut parts = time_core.split(':');
    let hh: i64 = parts.next()?.parse().ok()?;
    let mm: i64 = parts.next()?.parse().ok()?;
    let ss: i64 = match parts.next() {
        Some(s) => s.split('.').next().unwrap_or(s).parse().ok()?,
        None => 0,
    };
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) || !(0..=59).contains(&ss) {
        return None;
    }
    if let Some(offset_secs) = tz_offset {
        let utc_secs = day * 86_400 + hh * 3600 + mm * 60 + ss - offset_secs;
        Some(format_local_ms(utc_secs as f64 * 1000.0, now_ms))
    } else {
        Some(format_local_day_time(day, hh, mm, local_day(now_ms)))
    }
}

fn split_iso_tz(s: &str) -> Option<(&str, Option<i64>)> {
    if let Some(core) = s.strip_suffix('Z').or_else(|| s.strip_suffix('z')) {
        return Some((core, Some(0)));
    }
    // Look for +HH:MM / -HH:MM / +HHMM / -HHMM after the leading HH:MM.
    if s.len() >= 5 {
        if let Some(pos) = s[5..].rfind(['+', '-']).map(|i| i + 5) {
            let core = &s[..pos];
            let tz = &s[pos..];
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let digits = &tz[1..];
            let (h, m) = if let Some((h, m)) = digits.split_once(':') {
                (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?)
            } else if digits.len() == 4 {
                (digits[..2].parse::<i64>().ok()?, digits[2..].parse::<i64>().ok()?)
            } else {
                return None;
            };
            if (0..=23).contains(&h) && (0..=59).contains(&m) {
                return Some((core, Some(sign * (h * 3600 + m * 60))));
            }
            return None;
        }
    }
    Some((s, None))
}

fn parse_hh_mm(s: &str) -> Option<(i64, i64)> {
    // Strip any trailing timezone or parenthetical like "(+03:00)".
    let core = s.split_whitespace().next().unwrap_or("");
    let (h, m) = core.split_once(':')?;
    if !(1..=2).contains(&h.len()) || m.len() != 2 {
        return None;
    }
    let hh: i64 = h.parse().ok()?;
    let mm: i64 = m.parse().ok()?;
    ((0..=23).contains(&hh) && (0..=59).contains(&mm)).then_some((hh, mm))
}

fn local_day(ms: f64) -> i64 {
    let sec = (ms / 1000.0).floor() as i64;
    (sec + crate::pasted::utc_offset(sec)).div_euclid(86_400)
}

fn format_local_ms(due_ms: f64, now_ms: f64) -> String {
    let sec = (due_ms / 1000.0).floor() as i64;
    let local = sec + crate::pasted::utc_offset(sec);
    let day = local.div_euclid(86_400);
    let of_day = local.rem_euclid(86_400);
    format_local_day_time(day, of_day / 3600, (of_day % 3600) / 60, local_day(now_ms))
}

fn format_local_day_time(day: i64, hh: i64, mm: i64, now_day: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    match day - now_day {
        0 => format!("today {hh:02}:{mm:02}"),
        1 => format!("tomorrow {hh:02}:{mm:02}"),
        -1 => format!("yesterday {hh:02}:{mm:02}"),
        _ => {
            let ((year, month, date), (this_year, _, _)) = (time::civil(day), time::civil(now_day));
            let month = MONTHS[(month as usize).clamp(1, 12) - 1];
            if year == this_year {
                format!("{month} {date} {hh:02}:{mm:02}")
            } else {
                format!("{month} {date}, {year} {hh:02}:{mm:02}")
            }
        }
    }
}

fn extract_scheduled_epoch_ms(out: &str) -> Option<f64> {
    let rest = out.trim().strip_prefix("Scheduled ")?;
    let token = rest.split_whitespace().next()?;
    let epoch_str = token.split('-').next()?;
    if epoch_str.len() == 10 && epoch_str.chars().all(|c| c.is_ascii_digit()) {
        let secs: f64 = epoch_str.parse().ok()?;
        return Some(secs * 1000.0);
    }
    None
}

fn human_every(raw: &str) -> Option<String> {
    let secs = parse_relative_seconds(raw)?;
    if secs % 86_400 == 0 {
        let d = secs / 86_400;
        return Some(if d == 1 { "every day".into() } else { format!("every {d} days") });
    }
    if secs % 3600 == 0 {
        let h = secs / 3600;
        return Some(if h == 1 { "every hour".into() } else { format!("every {h} hours") });
    }
    if secs % 60 == 0 {
        let m = secs / 60;
        return Some(if m == 1 { "every minute".into() } else { format!("every {m} min") });
    }
    None
}

// MARK: Cleaning & ID/boilerplate filtering helpers

/// Whether `value` looks like an internal/machine identifier that should never be shown to users.
pub fn is_internal_id(value: &str) -> bool {
    let s = value.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    // Known Kuvo & MCP prefixes (`ses_…`, `wake_…`, `wqi_…`, `req_…`, `call_…`, etc.).
    const PREFIXES: &[&str] = &[
        "ses_", "wake_", "msg_", "prt_", "wqi_", "bot_", "grp_", "lbl_", "key_", "art_", "per_",
        "req_", "sh_", "job_", "ask_", "call_", "toolu_", "tool_", "fc_", "tsk_", "run_", "evt_",
        "chk_", "usr_", "org_", "proj_", "conv_", "item_", "id_",
    ];
    if PREFIXES.iter().any(|p| s.starts_with(p) && s.len() > p.len()) {
        return true;
    }
    // `kuvo-wake` IDs: `<10-digit-epoch>-<alnum>`.
    if let Some((left, right)) = s.split_once('-') {
        if left.len() == 10
            && left.chars().all(|c| c.is_ascii_digit())
            && !right.is_empty()
            && right.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return true;
        }
    }
    // Standard UUID (`8-4-4-4-12` hex).
    if s.len() == 36 {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 5
            && [8, 4, 4, 4, 12]
                .iter()
                .zip(&parts)
                .all(|(len, part)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return true;
        }
    }
    // Long pure-hex token (>= 12 chars) or opaque mixed alnum token (>= 16 chars with digits).
    if s.len() >= 12 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    if s.len() >= 16
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().any(|c| c.is_ascii_alphabetic())
        && !s.contains('.')
        && !s.contains('/')
    {
        return true;
    }
    // Bare long numeric ID (5+ digits without `#`).
    if s.len() >= 5 && s.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

fn contains_internal_id(text: &str) -> bool {
    text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | ':' | '.'))
        .any(is_internal_id)
}

fn is_internal_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "id" | "session"
            | "session_id"
            | "sessionid"
            | "request_id"
            | "requestid"
            | "call_id"
            | "callid"
            | "token"
            | "cursor"
            | "offset"
            | "limit"
            | "page"
            | "wait"
            | "timeout"
            | "format"
            | "encoding"
            | "computer"
            | "computer_key"
            | "computerkey"
            | "box"
            | "ref"
            | "hash"
            | "sha"
            | "uuid"
            | "key"
            | "cancelkey"
            | "cancel_key"
    ) || lower.ends_with("_id")
        || lower.ends_with("_key")
        || lower.ends_with("_token")
        || (key.len() > 2 && (key.ends_with("Id") || key.ends_with("ID") || key.ends_with("Key")))
}

fn is_flag_word(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "null" | "none" | "default" | "auto" | "ok"
    )
}

fn looks_like_json(s: &str) -> bool {
    (s.starts_with('{') && s.ends_with('}')) || (s.starts_with('[') && s.ends_with(']'))
}

/// Boilerplate confirmations that add no human information once the step succeeded.
fn is_confirmation(text: &str) -> bool {
    let t = text.trim().trim_end_matches('.');
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "ok" | "done" | "success" | "succeeded" | "delivered" | "sent" | "saved" | "queued"
            | "scheduled" | "canceled" | "cancelled" | "updated" | "deleted" | "removed" | "true"
    ) {
        return true;
    }
    if t.lines().count() == 1 {
        for prefix in [
            "delivered to ",
            "message delivered",
            "message sent",
            "scheduled ",
            "canceled ",
            "cancelled ",
            "updated #",
            "removed #",
            "deleted #",
        ] {
            if lower.starts_with(prefix) {
                return true;
            }
        }
    }
    false
}

/// Cleans a person/agent name: strips leading `&`/`@` and parenthetical internal IDs (`(ses_…)`),
/// returning `None` if nothing human remains.
pub fn clean_person(raw: &str) -> Option<String> {
    let stripped = strip_parenthetical_ids(raw);
    let stripped = stripped
        .split(" · ")
        .next()
        .unwrap_or(&stripped)
        .trim()
        .trim_start_matches(['&', '@'])
        .trim();
    if stripped.is_empty() || is_internal_id(stripped) {
        return None;
    }
    Some(stripped.to_owned())
}

/// Removes parenthetical internal IDs such as `" (ses_1ac77df1a001YU3j)"` or `" [wake_4b19]"`.
fn strip_parenthetical_ids(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest.as_bytes()[open] == b'(' { ')' } else { ']' };
        if let Some(rel_close) = rest[open + 1..].find(close_char) {
            let inside = rest[open + 1..open + 1 + rel_close].trim();
            if is_internal_id(inside) {
                out.push_str(rest[..open].trim_end());
                rest = &rest[open + 1 + rel_close + 1..];
                continue;
            }
        }
        out.push_str(&rest[..open + 1]);
        rest = &rest[open + 1..];
    }
    out.push_str(rest);
    out.trim().to_owned()
}

fn extract_human_parenthetical(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        let close = after.find(')')?;
        let inside = after[..close].trim();
        if !inside.is_empty() && !is_internal_id(inside) && !contains_internal_id(inside) {
            return Some(single_line_excerpt(inside, 80));
        }
        rest = &after[close + 1..];
    }
    None
}

fn extract_quoted_phrase(text: &str) -> Option<String> {
    for (open, close) in [('“', '”'), ('"', '"')] {
        if let Some(start) = text.find(open) {
            let after = &text[start + open.len_utf8()..];
            if let Some(end) = after.find(close) {
                let inside = after[..end].trim();
                if !inside.is_empty() && !is_internal_id(inside) {
                    return Some(single_line_excerpt(inside, 80));
                }
            }
        }
    }
    None
}

fn format_item_number(input: &Value) -> Option<String> {
    for field in ["item", "number", "id"] {
        let Some(val) = input.get(field) else { continue };
        if let Some(n) = val.as_u64() {
            return Some(format!("#{n}"));
        }
        if let Some(s) = val.as_str() {
            let s = s.trim();
            if let Some(num) = s.strip_prefix('#') {
                if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                    return Some(format!("#{num}"));
                }
            } else if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
                return Some(format!("#{s}"));
            } else if let Some(num) = s.strip_prefix("wqi_") {
                if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                    return Some(format!("#{num}"));
                }
            }
        }
    }
    None
}

fn extract_hash_number(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#');
        if let Some(num) = token.strip_prefix('#') {
            if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                return Some(format!("#{num}"));
            }
        }
    }
    None
}

fn format_queue_status(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "needs-you" | "needs_you" | "needs you" => "Needs you".to_owned(),
        "in-review" | "in_review" | "in review" => "In review".to_owned(),
        "in-progress" | "in_progress" | "in progress" => "In progress".to_owned(),
        "waiting" => "Waiting".to_owned(),
        "todo" | "to-do" | "to_do" | "to do" => "To do".to_owned(),
        "backlog" => "Backlog".to_owned(),
        "done" => "Done".to_owned(),
        "canceled" | "cancelled" => "Canceled".to_owned(),
        _ => {
            let trimmed = raw.trim();
            let mut chars = trimmed.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

fn first_text_field(input: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        let val = input.get(*field).and_then(Value::as_str)?.trim();
        (!val.is_empty() && !is_internal_id(val)).then(|| val.to_owned())
    })
}

fn first_short_field(input: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        let val = input.get(*field).and_then(Value::as_str)?.trim();
        (!val.is_empty() && !is_internal_id(val)).then(|| single_line_excerpt(val, 80))
    })
}

fn single_line_excerpt(text: &str, max_chars: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let count = line.chars().count();
    if count <= max_chars {
        line.to_owned()
    } else {
        let cut: String = line.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 2026-11-14T09:53:20Z
    const NOW: f64 = 1_794_650_000_000.0;

    #[test]
    fn send_message_shows_person_and_message_text_only() {
        let state = json!({
            "status": "completed",
            "input": {
                "to": "&Mira (ses_1ac77df1a001YU3j)",
                "wait": false,
                "text": "Groundwork is merged and Flow is wired in."
            },
            "content": [{ "type": "text", "text": "Delivered to &Mira (ses_1ac77df1a001YU3j)." }]
        });
        let p = present("kuvo-browser_send_message", Some("Sent a message"), Some("Sending a message"), &state, false, NOW, NOW).unwrap();
        assert_eq!(p.icon, IconName::Send);
        assert_eq!(p.label.as_ref(), "Messaged Mira");
        assert_eq!(p.target, None);

        let n = note("kuvo-browser_send_message", &state).unwrap();
        assert_eq!(n.primary, "Groundwork is merged and Flow is wired in.");
        assert_eq!(n.secondary, None, "Delivered confirmation is never shown");

        // When `to` is only an internal id, falls back to box title / never shows ses_…
        let only_id = json!({
            "status": "completed",
            "input": { "to": "ses_1ac77df1a001YU3j", "text": "Ready for review." },
            "content": [{ "type": "text", "text": "Delivered." }]
        });
        let p2 = present("kuvo-browser_send_message", Some("Sent a message to Mira"), None, &only_id, false, NOW, NOW).unwrap();
        assert_eq!(p2.label.as_ref(), "Messaged Mira");
        assert!(!p2.label.contains("ses_"));
    }

    #[test]
    fn queue_update_shows_item_arrow_status_and_note_only() {
        let state = json!({
            "status": "completed",
            "input": {
                "item": "231",
                "status": "in-review",
                "note": "Flow view wired; light and dark shots rendered."
            },
            "content": [{ "type": "text", "text": "Updated #231 — status set to In review." }]
        });
        let p = present("kuvo-browser_queue_update", Some("Updated a Work Queue item"), None, &state, false, NOW, NOW).unwrap();
        assert_eq!(p.icon, IconName::Queue);
        assert_eq!(p.label.as_ref(), "#231 → In review");
        assert_eq!(p.target, None);

        let n = note("kuvo-browser_queue_update", &state).unwrap();
        assert_eq!(n.primary, "Flow view wired; light and dark shots rendered.");
        assert_eq!(n.secondary, None);
    }

    #[test]
    fn schedule_wake_up_formats_human_time_and_hides_confirmation() {
        let state = json!({
            "status": "completed",
            "input": {
                "when": "today 19:30",
                "summary": "Check whether the release build passed",
                "note": "Check CI for kuvo/900-batch-c-drafts and report any failing checks."
            },
            "content": [{ "type": "text", "text": "Scheduled wake_8f2a for today at 19:30 (+03:00)." }]
        });
        let p = present("kuvo-browser_schedule_wake_up", Some("Scheduled a wake-up"), None, &state, false, NOW, NOW).unwrap();
        assert_eq!(p.icon, IconName::Alarm);
        assert_eq!(p.label.as_ref(), "Wake-up today 19:30");
        assert_eq!(p.target.as_deref(), Some("· Check whether the release build passed"));

        let n = note("kuvo-browser_schedule_wake_up", &state).unwrap();
        assert_eq!(n.primary, "Check CI for kuvo/900-batch-c-drafts and report any failing checks.");
        assert_eq!(n.secondary, None);
    }

    #[test]
    fn human_when_never_emits_relative_deltas_or_offsets() {
        let rel = human_when("+2h", NOW, NOW).unwrap();
        assert!(rel.starts_with("today ") || rel.starts_with("tomorrow "), "got {rel}");
        assert!(!rel.contains('+'));
        assert!(!rel.contains("03:00"));

        let tomorrow = human_when("tomorrow 09:00", NOW, NOW).unwrap();
        assert_eq!(tomorrow, "tomorrow 09:00");

        let plus_day = human_when("+1d", NOW, NOW).unwrap();
        assert!(plus_day.starts_with("tomorrow "), "got {plus_day}");

        let future = human_when("2026-11-24T09:00", NOW, NOW).unwrap();
        assert_eq!(future, "Nov 24 09:00");

        let iso_offset = human_when("2026-11-24T09:00:00+03:00", NOW, NOW).unwrap();
        assert!(!iso_offset.contains('+'));
        assert!(!iso_offset.contains("(+"));
    }

    #[test]
    fn cancel_wake_up_hides_wake_id_and_shows_summary() {
        let state = json!({
            "status": "completed",
            "input": { "id": "wake_4b19" },
            "content": [{ "type": "text", "text": "Canceled wake_4b19 (Check groundwork branch)." }]
        });
        let p = present("kuvo-browser_cancel_wake_up", Some("Canceled a wake-up"), None, &state, false, NOW, NOW).unwrap();
        assert_eq!(p.icon, IconName::Alarm);
        assert_eq!(p.label.as_ref(), "Canceled the wake-up");
        assert_eq!(p.target.as_deref(), Some("· Check groundwork branch"));
        assert_eq!(note("kuvo-browser_cancel_wake_up", &state), None);
    }

    #[test]
    fn clean_error_strips_error_prefix_and_internal_ids() {
        assert_eq!(clean_error("Error: #999 not found"), "#999 not found");
        assert_eq!(clean_error("MCP error: Error: Item #999 not found (wqi_999)"), "Item #999 not found");
        assert_eq!(clean_error("#999 not found"), "#999 not found");
    }

    #[test]
    fn unknown_mcp_tool_picks_single_short_arg_and_main_text_only() {
        let state = json!({
            "status": "completed",
            "input": {
                "team_id": "team_9f2a8b1c",
                "draft": false,
                "priority": 2,
                "title": "Tighten dark mode borders",
                "description": "Hairline borders on cards blend into the page background in dark appearance."
            },
            "content": [{ "type": "text", "text": "Created KUV-412 in Design Backlog." }]
        });
        let p = present("linear_create_issue", Some("Linear · Create issue"), Some("Linear · Creating issue"), &state, false, NOW, NOW).unwrap();
        assert_eq!(p.icon, IconName::Sliders);
        assert_eq!(p.label.as_ref(), "Linear · Create issue");
        assert_eq!(p.target.as_deref(), Some("Tighten dark mode borders"));

        let n = note("linear_create_issue", &state).unwrap();
        assert_eq!(n.primary, "Hairline borders on cards blend into the page background in dark appearance.");
        assert_eq!(n.secondary.as_deref(), Some("Created KUV-412 in Design Backlog."));
    }

    #[test]
    fn unknown_mcp_tool_never_dumps_raw_json_or_internal_ids() {
        let state = json!({
            "status": "completed",
            "input": { "session": "ses_1", "wait": true },
            "content": [{ "type": "text", "text": "{\"id\":\"req_82ab12cd\",\"ok\":true}" }]
        });
        let p = present("kuvo-browser_start_session", Some("Started a session"), Some("Starting a session"), &state, true, NOW, NOW).unwrap();
        assert_eq!(p.label.as_ref(), "Starting a session");
        assert_eq!(p.target, None);
        assert_eq!(note("kuvo-browser_start_session", &state), None);
    }

    #[test]
    fn internal_id_detector_catches_kuvo_uuids_and_tokens() {
        assert!(is_internal_id("ses_1"));
        assert!(is_internal_id("ses_1ac77df1a001YU3jx6hlwNrIkp"));
        assert!(is_internal_id("wake_4b19"));
        assert!(is_internal_id("1794657200-a3f9"));
        assert!(is_internal_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_internal_id("Mira"));
        assert!(!is_internal_id("#231"));
        assert!(!is_internal_id("KUV-412"));
        assert!(!is_internal_id("Check groundwork branch"));
    }
}
