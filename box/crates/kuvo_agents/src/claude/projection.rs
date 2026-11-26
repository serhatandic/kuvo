//! Project Claude Code's own turn (not its subagents') to ordered, replayable resident turn
//! events, as the bridge tells it.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::session::{
    Limit, LimitScope, LimitWindow, ModelRef, PlanLimitReading, TextKind, ToolState, TurnError,
    TurnEvent, Usage, VendorState,
};

use super::{diagnostic::redact, results, tools::kuvo_tool};

/// The message kinds Claude Code sends that this bridge knows (shown, used or bookkeeping).
const MESSAGES: &[&str] = &[
    "assistant",
    "user",
    "stream_event",
    "result",
    "system",
    "rate_limit_event",
    "auth_status",
    "conversation_reset",
    "tool_progress",
    "tool_use_summary",
    "prompt_suggestion",
    "command_lifecycle",
];

/// Its `system` messages' kinds that this bridge knows.
const SYSTEM: &[&str] = &[
    "init",
    "compact_boundary",
    "status",
    "api_retry",
    "control_request_progress",
    "model_refusal_fallback",
    "model_refusal_no_fallback",
    "local_command_output",
    "hook_started",
    "hook_progress",
    "hook_response",
    "plugin_install",
    "task_notification",
    "task_started",
    "task_updated",
    "task_progress",
    "background_tasks_changed",
    "thinking_tokens",
    "session_state_changed",
    "worker_shutting_down",
    "commands_changed",
    "notification",
    "files_persisted",
    "memory_recall",
    "elicitation_complete",
    "permission_denied",
    "mirror_error",
    "informational",
    "task_summary",
    "post_turn_summary",
];

#[derive(Default)]
pub(super) struct Projection {
    streamed: HashSet<String>,
    calls: HashMap<String, Call>,
    /// The streamed message's tool calls, by their block's index.
    block_tools: HashMap<u64, String>,
    /// The streamed message's id, for its blocks' keys.
    message: Option<String>,
    usage: Option<Usage>,
    stop_reason: Option<String>,
    context_model: Option<String>,
    used: Option<u64>,
    /// A summary you asked for (`/compact`): its row is the turn's, whichever way it ends.
    explicit_compaction: bool,
    compact_failed: bool,
    /// Signed in with a key: what a turn cost is said once it ends.
    uses_key: bool,
    problem: Option<String>,
    limit: Option<Limit>,
    sign_in_turned_away: bool,
    synthetic: u64,
}

struct Call {
    native: String,
    raw: Value,
    name: String,
    input: Value,
}

impl Projection {
    pub fn new(explicit_compaction: bool, uses_key: bool) -> Self {
        Self {
            explicit_compaction,
            uses_key,
            ..Self::default()
        }
    }

    /// One native message. `noticed` is what this Claude Code has said once already.
    pub fn frame(
        &mut self,
        frame: &Value,
        model: &ModelRef,
        state: &mut VendorState,
        noticed: &mut HashSet<String>,
    ) -> Vec<TurnEvent> {
        if frame["parent_tool_use_id"].is_string() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let kind = frame["type"].as_str().unwrap_or_default();
        match kind {
            "stream_event" => self.stream(&frame["event"], &mut out),
            "assistant" => self.assistant(frame, &mut out),
            "user" => self.tools_done(frame, &mut out),
            "rate_limit_event" => self.rate_limit(&frame["rate_limit_info"], noticed, &mut out),
            "system" => self.system(frame, model, state, noticed, &mut out),
            "conversation_reset" => {
                self.used = None;
                state.remove("claudeCost");
                if let Some(native) = frame["new_conversation_id"].as_str() {
                    state.insert("claudeSession".into(), native.into());
                    state.insert("claudeStarted".into(), true.into());
                } else {
                    state.remove("claudeSession");
                    state.remove("claudeStarted");
                }
                out.push(TurnEvent::ContextCleared);
                out.push(TurnEvent::Notice(
                    "Claude Code started its conversation over.".into(),
                ));
            }
            "auth_status" => {
                if let Some(error) = frame["error"].as_str() {
                    self.problem = Some(error.into());
                } else if let Some(output) = frame["output"].as_array()
                    && !output.is_empty()
                {
                    let words = output
                        .iter()
                        .map(|line| {
                            line.as_str()
                                .map_or_else(|| line.to_string(), str::to_owned)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    out.push(TurnEvent::Notice(words));
                }
            }
            "result" => self.result(frame, model, state, &mut out),
            kind if !MESSAGES.contains(&kind) => unknown(kind, frame, noticed, &mut out),
            _ => {}
        }
        out
    }

    pub fn child_is_background(&self, id: &str) -> bool {
        self.calls.get(id).is_some_and(|call| call.raw["run_in_background"] == true)
    }

    /// Why the turn the `result` ends failed, or none: it went through.
    /// The model the conversation's last request ran on, as Claude Code said.
    pub fn context_model(&self) -> Option<&str> {
        self.context_model.as_deref()
    }

    pub fn failure(&self, frame: &Value) -> Option<TurnError> {
        let succeeded =
            frame["subtype"] == "success" && frame["is_error"] != true && !self.compact_failed;
        if succeeded {
            return None;
        }
        let mut limit = self.limit.clone();
        let mut problem = self.problem.clone();
        // Its words for a limit, when it stopped: "Claude AI usage limit reached|<when it resets>".
        let text = frame["result"].as_str().unwrap_or_default();
        let reached = Regex::new(r"(?i)usage limit reached\|?(\d+)?")
            .expect("limit words")
            .captures(text);
        let status = frame["api_error_status"]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok());
        if reached.is_some() || status == Some(429) {
            let resets_at = reached
                .and_then(|reached| reached.get(1)?.as_str().parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1000));
            let limit = limit.get_or_insert(Limit {
                scope: LimitScope::Provider,
                resets_at: None,
            });
            limit.resets_at = limit.resets_at.or(resets_at);
            problem.get_or_insert_with(|| "You've reached your Claude usage limit for now.".into());
        }
        if let Some(limit) = limit {
            return Some(TurnError {
                kind: "limit".into(),
                message: problem.unwrap_or_else(|| "You've reached a Claude usage limit.".into()),
                status: None,
                limit: Some(limit),
            });
        }
        let errors = frame["errors"]
            .as_array()
            .filter(|errors| !errors.is_empty())
            .map(|errors| {
                errors
                    .iter()
                    .map(|error| {
                        error
                            .as_str()
                            .map_or_else(|| error.to_string(), str::to_owned)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        let message = problem
            .or_else(|| (!text.trim().is_empty()).then(|| redact(text.trim())))
            .or(errors)
            .unwrap_or_else(|| {
                match frame["subtype"].as_str() {
                    Some("error_max_turns") => "Claude Code stopped after too many steps.",
                    Some("error_max_budget_usd") => "Claude Code reached its spending limit.",
                    _ => "Claude Code stopped with an error.",
                }
                .into()
            });
        // A sign-in Claude turned away: one to sign in again for.
        if self.sign_in_turned_away {
            return Some(sign_in_error(message));
        }
        Some(TurnError {
            kind: "agent".into(),
            message,
            status: None,
            limit: None,
        })
    }

    fn stream(&mut self, event: &Value, out: &mut Vec<TurnEvent>) {
        let index = event["index"].as_u64();
        match event["type"].as_str() {
            Some("message_start") => {
                let message = &event["message"];
                let id = message["id"].as_str().map(str::to_owned);
                if let Some(id) = &id {
                    self.streamed.insert(id.clone());
                }
                self.message = id;
                self.block_tools.clear();
                self.context_model = message["model"].as_str().map(str::to_owned);
                let mut usage = usage(&message["usage"]);
                usage.reasoning = 0;
                self.usage = Some(usage);
                self.stop_reason = None;
                out.push(TurnEvent::StepStarted { model: None });
            }
            Some("content_block_start") => {
                let block = &event["content_block"];
                let kind = block["type"].as_str().unwrap_or_default();
                if matches!(kind, "tool_use" | "server_tool_use" | "mcp_tool_use") {
                    if let (Some(index), Some(id)) = (index, block["id"].as_str()) {
                        let native = native_name(block);
                        let (name, _) = kuvo_tool(&native, &json!({}));
                        self.block_tools.insert(index, id.into());
                        out.push(tool(id, &name, ToolState::Streaming(String::new())));
                        self.calls.entry(id.into()).or_insert(Call {
                            native,
                            raw: json!({}),
                            name,
                            input: json!({}),
                        });
                    }
                } else if kind == "redacted_thinking" {
                    self.text(out, TextKind::Reasoning, index, HIDDEN_THINKING);
                } else if kind.ends_with("_tool_result") {
                    self.server_result(block, out);
                }
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => self.text(
                        out,
                        TextKind::Text,
                        index,
                        delta["text"].as_str().unwrap_or_default(),
                    ),
                    Some("thinking_delta") => self.text(
                        out,
                        TextKind::Reasoning,
                        index,
                        delta["thinking"].as_str().unwrap_or_default(),
                    ),
                    Some("input_json_delta") => {
                        if let Some(id) = index.and_then(|index| self.block_tools.get(&index))
                            && let Some(call) = self.calls.get(id)
                        {
                            let delta = delta["partial_json"].as_str().unwrap_or_default();
                            out.push(tool(id, &call.name, ToolState::Streaming(delta.into())));
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(usage) = &mut self.usage
                    && let Some(output) = event["usage"]["output_tokens"].as_u64()
                {
                    usage.output = output;
                }
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.into());
                }
            }
            Some("message_stop") => {
                self.used = self.usage.as_ref().and_then(step_context);
                out.push(TurnEvent::Context {
                    used: self.used,
                    window: None,
                });
                let finish = match self.stop_reason.take() {
                    Some(reason) if reason == "tool_use" => "tool-calls".into(),
                    Some(reason) => reason,
                    None => "stop".into(),
                };
                out.push(TurnEvent::StepEnded {
                    usage: self.usage.take(),
                    finish: Some(finish),
                    error: None,
                });
            }
            _ => {}
        }
    }

    fn assistant(&mut self, frame: &Value, out: &mut Vec<TurnEvent>) {
        let message = &frame["message"];
        let content = items(&message["content"]);
        let id = message["id"].as_str();
        let streamed = id.is_some_and(|id| self.streamed.contains(id));
        let error = frame["error"].as_str().or(message["error"].as_str());
        if error.is_none()
            && let Some(model) = message["model"]
                .as_str()
                .filter(|model| !model.starts_with('<'))
        {
            self.context_model = Some(model.into());
        }
        if error.is_none()
            && self.context_model.is_some()
            && !streamed
            && message["usage"].is_object()
        {
            self.used = step_context(&usage(&message["usage"]));
            out.push(TurnEvent::Context {
                used: self.used,
                window: None,
            });
        }
        match error {
            Some("authentication_failed") => {
                self.problem = Some(SIGN_IN_TURNED_AWAY.into());
                self.sign_in_turned_away = true;
            }
            Some(error) => {
                // Its own words when it has them ("prompt is too long: 210000 > 200000").
                let said = content
                    .iter()
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str())
                    .filter(|text| !text.is_empty())
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("\n");
                let said = said.trim();
                self.problem.get_or_insert_with(|| {
                    if said.is_empty() {
                        api_error_words(error)
                    } else {
                        redact(said)
                    }
                });
            }
            None => {}
        }
        // Its words for an error the turn fails with: said once, as its failure.
        let failing = error.is_some();
        let key = id.map(str::to_owned);
        for (index, block) in content.iter().enumerate() {
            let kind = block["type"].as_str().unwrap_or_default();
            if matches!(kind, "tool_use" | "server_tool_use" | "mcp_tool_use") {
                let Some(call) = block["id"].as_str() else {
                    continue;
                };
                let native = native_name(block);
                let raw = if block["input"].is_object() {
                    block["input"].clone()
                } else {
                    json!({})
                };
                let (name, input) = kuvo_tool(&native, &raw);
                out.push(tool(
                    call,
                    &name,
                    ToolState::Running {
                        input: input.clone(),
                    },
                ));
                self.calls.insert(
                    call.into(),
                    Call {
                        native,
                        raw,
                        name,
                        input,
                    },
                );
            } else if kind.ends_with("_tool_result") {
                self.server_result(block, out);
            } else if !streamed {
                let index = Some(index as u64);
                let previous = std::mem::replace(&mut self.message, key.clone());
                match kind {
                    "text" if !failing => self.text(
                        out,
                        TextKind::Text,
                        index,
                        block["text"].as_str().unwrap_or_default(),
                    ),
                    "thinking" => self.text(
                        out,
                        TextKind::Reasoning,
                        index,
                        block["thinking"].as_str().unwrap_or_default(),
                    ),
                    "redacted_thinking" => {
                        self.text(out, TextKind::Reasoning, index, HIDDEN_THINKING)
                    }
                    "compaction" => out.push(TurnEvent::CompactionEnded { succeeded: true }),
                    _ => {}
                }
                self.message = previous;
            }
        }
    }

    fn text(&mut self, out: &mut Vec<TurnEvent>, kind: TextKind, index: Option<u64>, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let block = index.map(|index| match &self.message {
            Some(message) => format!("{message}:{index}"),
            None => index.to_string(),
        });
        out.push(TurnEvent::TextDelta {
            kind,
            block,
            delta: delta.into(),
        });
    }

    fn tools_done(&mut self, frame: &Value, out: &mut Vec<TurnEvent>) {
        let results = items(&frame["message"]["content"])
            .iter()
            .filter(|block| block["type"] == "tool_result")
            .collect::<Vec<_>>();
        // Its own account of the tool, when the message has one result only.
        let structured = (results.len() == 1)
            .then(|| frame.get("tool_use_result"))
            .flatten();
        for result in results {
            let Some(id) = result["tool_use_id"].as_str() else {
                continue;
            };
            let Some(call) = self.calls.get(id) else {
                continue;
            };
            let (state, metadata) =
                results::outcome(&call.native, &call.raw, &call.input, result, structured);
            out.push(TurnEvent::Tool {
                id: id.into(),
                name: call.name.clone(),
                state,
                metadata,
            });
        }
    }

    /// A tool the API ran itself (a web search, code it ran) came back with this.
    fn server_result(&mut self, block: &Value, out: &mut Vec<TurnEvent>) {
        let Some(id) = block["tool_use_id"].as_str() else {
            return;
        };
        let kind = block["type"].as_str().unwrap_or_default();
        let (name, input) = match self.calls.get(id) {
            Some(call) => (call.name.clone(), call.input.clone()),
            None => (kind.trim_end_matches("_tool_result").to_owned(), json!({})),
        };
        let content = &block["content"];
        let failed = (content.is_object()
            && content["type"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("_error")))
            || block["is_error"] == true;
        let parts = match content {
            Value::Array(parts) => parts.iter().collect::<Vec<_>>(),
            Value::Null => Vec::new(),
            other => vec![other],
        };
        let lines = parts
            .into_iter()
            .filter_map(|part| {
                if part["type"] == "web_search_result" {
                    Some(
                        [part["title"].as_str(), part["url"].as_str()]
                            .into_iter()
                            .flatten()
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join(" — "),
                    )
                } else if let Some(text) = part["text"].as_str() {
                    Some(text.into())
                } else if part["stdout"].is_string() || part["stderr"].is_string() {
                    Some(
                        [part["stdout"].as_str(), part["stderr"].as_str()]
                            .into_iter()
                            .flatten()
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                } else if part["type"] == "web_fetch_result" {
                    Some(part["url"].as_str().unwrap_or_default().into())
                } else {
                    part["error_code"].as_str().map(str::to_owned)
                }
            })
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        let text = lines.join("\n").chars().take(30_000).collect::<String>();
        let state = if failed {
            ToolState::Failed {
                input,
                content: Vec::new(),
                error: TurnError {
                    kind: "tool".into(),
                    message: if text.is_empty() {
                        "The tool failed.".into()
                    } else {
                        text
                    },
                    status: None,
                    limit: None,
                },
            }
        } else {
            ToolState::Completed {
                input,
                content: vec![crate::session::ToolOutput::Text(text)],
            }
        };
        out.push(tool(id, &name, state));
    }

    fn rate_limit(
        &mut self,
        info: &Value,
        noticed: &mut HashSet<String>,
        out: &mut Vec<TurnEvent>,
    ) {
        // Where the plan stands, for its limits, whichever turn heard it (the thread says under
        // which sign-in).
        let windows = heard_windows(info);
        if !windows.is_empty() {
            out.push(TurnEvent::PlanLimit(PlanLimitReading {
                windows,
                sign_in: None,
            }));
        }
        // A limit being used up: said once per threshold it passes, and again once it's close.
        if info["status"] == "allowed_warning" {
            let (text, close) = limit_warning(info);
            let key = format!(
                "warn:{}:{}:{}",
                info["rateLimitType"].as_str().unwrap_or("undefined"),
                number_text(&info["surpassedThreshold"]),
                if close { "close" } else { "" }
            );
            notice_once(noticed, key, text, out);
            return;
        }
        // Over the plan's limit, unless extra usage you pay for carries on.
        if info["status"] != "rejected"
            || info["overageInUse"] == true
            || info["isUsingOverage"] == true
        {
            return;
        }
        let kind = info["rateLimitType"].as_str();
        let scope = if matches!(kind, Some("seven_day_opus" | "seven_day_sonnet")) {
            LimitScope::Model
        } else {
            LimitScope::Provider
        };
        self.limit = Some(Limit {
            scope,
            resets_at: info["resetsAt"]
                .as_f64()
                .map(|seconds| (seconds * 1000.0) as u64),
        });
        self.problem = Some(limit_words(kind).into());
    }

    fn system(
        &mut self,
        frame: &Value,
        model: &ModelRef,
        state: &mut VendorState,
        noticed: &mut HashSet<String>,
        out: &mut Vec<TurnEvent>,
    ) {
        let subtype = frame["subtype"].as_str().unwrap_or_default();
        match subtype {
            "init" => {
                if let Some(id) = frame["session_id"].as_str() {
                    state.insert("claudeSession".into(), id.into());
                    state.insert("claudeStarted".into(), true.into());
                }
            }
            "compact_boundary" => {
                let used = frame["compact_metadata"]["post_tokens"].as_u64();
                self.used = used;
                out.push(match used {
                    Some(used) => TurnEvent::Context {
                        used: Some(used),
                        window: None,
                    },
                    None => TurnEvent::ContextCleared,
                });
                if !self.explicit_compaction {
                    out.push(TurnEvent::CompactionEnded { succeeded: true });
                }
            }
            "status" => {
                // One it makes by itself: `compacting` as it begins (and again while it waits),
                // then a `null` status with how it went. A bare `null` one says only that it isn't
                // summarizing now.
                if self.explicit_compaction {
                    if frame["compact_result"] == "failed" {
                        self.compact_failed = true;
                        self.problem = Some(format!(
                            "Claude Code couldn't summarize the conversation{}.",
                            frame["compact_error"]
                                .as_str()
                                .map_or_else(String::new, |error| {
                                    format!(
                                        ": {}",
                                        redact(error.strip_suffix('.').unwrap_or(error))
                                    )
                                })
                        ));
                    }
                    return;
                }
                if frame["status"] == "compacting" {
                    out.push(TurnEvent::CompactionStarted);
                } else if let Some(result @ ("success" | "failed")) =
                    frame["compact_result"].as_str()
                {
                    out.push(TurnEvent::CompactionEnded {
                        succeeded: result == "success",
                    });
                }
            }
            "api_retry" => {
                let overloaded =
                    frame["error"] == "overloaded" || frame["error_status"].as_u64() == Some(529);
                let attempts = match frame["max_retries"].as_u64().filter(|most| *most > 0) {
                    Some(most) => format!(" ({}/{most})", number_text(&frame["attempt"])),
                    None => String::new(),
                };
                let text = format!(
                    "Claude's API {}: trying again{attempts}.",
                    if overloaded {
                        "is overloaded"
                    } else {
                        "had a problem"
                    }
                );
                notice_once(noticed, format!("retry:{}", model.id), text, out);
            }
            "local_command_output" => {
                // What a command you typed for Claude Code itself printed (/cost, /usage).
                if let Some(content) = frame["content"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                {
                    let key = self.synthetic_block();
                    out.push(TurnEvent::TextDelta {
                        kind: TextKind::Text,
                        block: Some(key),
                        delta: content.into(),
                    });
                }
            }
            "informational" => {
                if frame["level"] != "info"
                    && let Some(content) = frame["content"].as_str()
                {
                    out.push(TurnEvent::Notice(content.into()));
                }
            }
            "notification" => {
                if frame["priority"] != "low"
                    && let Some(text) = frame["text"].as_str()
                {
                    out.push(TurnEvent::Notice(text.into()));
                }
            }
            "model_refusal_fallback" | "model_refusal_no_fallback" => {
                if let Some(content) = frame["content"].as_str() {
                    out.push(TurnEvent::Notice(content.into()));
                }
            }
            "plugin_install" => {
                if frame["status"] == "failed" {
                    let name = frame["name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .map_or_else(String::new, |name| format!(" ({name})"));
                    let error = frame["error"]
                        .as_str()
                        .filter(|error| !error.is_empty())
                        .map_or_else(String::new, |error| format!(": {error}"));
                    out.push(TurnEvent::Notice(format!(
                        "A Claude Code plugin didn't install{name}{error}."
                    )));
                }
            }
            "hook_response" => {
                let exit = frame["exit_code"].as_i64().is_some_and(|code| code != 0);
                if frame["outcome"] == "error" || exit {
                    let hook = frame["hook_name"]
                        .as_str()
                        .or(frame["hook_event"].as_str())
                        .unwrap_or_default();
                    let said = frame["stderr"].as_str().unwrap_or_default().trim();
                    let words = if said.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", redact(said))
                    };
                    out.push(TurnEvent::Notice(format!(
                        "Your {hook} hook failed{words}."
                    )));
                }
            }
            "memory_recall" => {
                let id = format!("call_memory_{}", self.synthetic_block());
                let input = json!({"description":"Recalled from memory"});
                let paths = items(&frame["paths"])
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if paths.is_empty() {
                    frame["content"].as_str().unwrap_or_default().to_owned()
                } else {
                    paths
                };
                out.push(tool(
                    &id,
                    "memory",
                    ToolState::Running {
                        input: input.clone(),
                    },
                ));
                out.push(tool(
                    &id,
                    "memory",
                    ToolState::Completed {
                        input,
                        content: vec![crate::session::ToolOutput::Text(text)],
                    },
                ));
            }
            subtype if !SYSTEM.contains(&subtype) => {
                unknown(&format!("system:{subtype}"), frame, noticed, out)
            }
            _ => {}
        }
    }

    fn result(
        &mut self,
        frame: &Value,
        model: &ModelRef,
        state: &mut VendorState,
        out: &mut Vec<TurnEvent>,
    ) {
        if let Some(total) = frame["total_cost_usd"].as_f64() {
            let before = state
                .get("claudeCost")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            // Smaller than before: it started counting again (a new conversation).
            let spent = if total >= before {
                total - before
            } else {
                total
            };
            state.insert("claudeCost".into(), Value::from(total));
            if spent > 0.0 && self.uses_key {
                out.push(TurnEvent::Cost(spent));
            }
        }
        let used_model = self
            .context_model
            .clone()
            .unwrap_or_else(|| model.id.clone());
        let window = frame["modelUsage"][&used_model]["contextWindow"]
            .as_u64()
            .filter(|window| *window > 0);
        if let Some(size) = window {
            state.insert("claudeWindow".into(), json!({"model":model.id,"size":size}));
            out.push(TurnEvent::Context {
                used: self.used,
                window,
            });
        }
    }

    fn synthetic_block(&mut self) -> String {
        self.synthetic += 1;
        format!("kuvo:{}", self.synthetic)
    }
}

const SIGN_IN_TURNED_AWAY: &str = "Claude ended this sign-in, and Claude Code couldn't renew it.";
const HIDDEN_THINKING: &str = "(The model kept this thinking to itself.)";

fn sign_in_error(message: String) -> TurnError {
    TurnError {
        kind: "signin".into(),
        message,
        status: None,
        limit: None,
    }
}

/// A note, once per kind for this Claude Code.
fn notice_once(noticed: &mut HashSet<String>, key: String, text: String, out: &mut Vec<TurnEvent>) {
    if noticed.insert(key) {
        out.push(TurnEvent::Notice(text));
    }
}

/// Something Claude Code sent that this bridge doesn't know (a newer one's): its words shown once,
/// and logged once.
fn unknown(kind: &str, frame: &Value, noticed: &mut HashSet<String>, out: &mut Vec<TurnEvent>) {
    let words = ["content", "text", "message", "summary"]
        .into_iter()
        .filter_map(|field| frame[field].as_str())
        .find(|words| !words.trim().is_empty());
    if let Some(words) = words {
        notice_once(
            noticed,
            format!("unknown:{kind}:{words}"),
            words.into(),
            out,
        );
    }
    if noticed.insert(format!("log:{kind}")) {
        let said = redact(&frame.to_string());
        eprintln!(
            "Claude Code sent {kind}, which Kuvo Code doesn't know: {}",
            said.chars().take(300).collect::<String>()
        );
    }
}

fn api_error_words(error: &str) -> String {
    match error {
        "billing_error" => "Claude's API says there's a billing problem with this account.".into(),
        "rate_limit" => "Claude's API is rate limiting this model.".into(),
        "invalid_request" => "Claude's API turned the request away.".into(),
        "server_error" => "Claude's API had a problem.".into(),
        "max_output_tokens" => "The reply ran out of room.".into(),
        error => format!("Claude's API reported an error ({error})."),
    }
}

fn limit_words(kind: Option<&str>) -> &'static str {
    match kind {
        Some("five_hour") => "You've reached your Claude usage limit for the next few hours.",
        Some("seven_day") => "You've reached your weekly Claude limit.",
        Some("seven_day_opus") => "You've reached this week's Opus limit on your Claude plan.",
        Some("seven_day_sonnet") => "You've reached this week's Sonnet limit on your Claude plan.",
        _ => "You've reached your Claude usage limit for now.",
    }
}

/// What a turn says when Claude Code warns of a limit: how much of which one is used, and whether
/// that's close (only once nearly all of it is).
fn limit_warning(info: &Value) -> (String, bool) {
    const CLOSE: f64 = 0.9;
    let kind = info["rateLimitType"].as_str();
    let limit = match kind {
        Some("five_hour") => "your 5-hour Claude limit",
        Some("overage") => "your Claude extra usage limit",
        Some("seven_day") => "your weekly Claude limit",
        Some("seven_day_sonnet") => "your weekly Sonnet limit",
        Some("seven_day_opus") => "your weekly Opus limit",
        Some("seven_day_overage_included") => "your weekly Fable limit",
        _ => "your Claude usage limit",
    };
    let stated = [
        &info["utilization"],
        &info["unifiedWindows"][kind.unwrap_or_default()]["utilization"],
    ]
    .into_iter()
    .find_map(|value| value.as_f64().filter(|value| value.is_finite()));
    if let Some(stated) = stated {
        // (Never all of it till it is: 99.6% is 99%.)
        let percent = if stated < 1.0 {
            ((stated.max(0.0) * 100.0).round() as u64).min(99)
        } else {
            100
        };
        if percent == 100 {
            return (format!("You've used all of {limit}."), true);
        }
        let close = percent as f64 >= CLOSE * 100.0;
        let text = if close {
            format!("You're close to {limit}: {percent}% used.")
        } else {
            format!("You've used {percent}% of {limit}.")
        };
        return (text, close);
    }
    // No figure: the threshold it passed, at least.
    match info["surpassedThreshold"]
        .as_f64()
        .filter(|passed| *passed > 0.0)
    {
        None => (format!("Claude Code warned about {limit}."), false),
        Some(passed) if passed >= CLOSE => (format!("You're close to {limit}."), true),
        Some(passed) => (
            format!(
                "You've used over {}% of {limit}.",
                (passed * 100.0).round() as u64
            ),
            false,
        ),
    }
}

/// A number as JavaScript writes it in a template (`0.5`, `1`, `undefined`).
fn number_text(value: &Value) -> String {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(float) if float.fract() == 0.0 && float.abs() < 1e15 => {
                format!("{}", float as i64)
            }
            _ => number.to_string(),
        },
        Value::Null => "undefined".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn native_name(block: &Value) -> String {
    let name = block["name"].as_str().unwrap_or_default();
    if block["type"] == "mcp_tool_use" {
        format!(
            "mcp__{}__{name}",
            block["server_name"].as_str().unwrap_or("mcp")
        )
    } else {
        name.into()
    }
}

pub(super) fn items(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn usage(value: &Value) -> Usage {
    Usage {
        input: value["input_tokens"].as_u64().unwrap_or_default(),
        output: value["output_tokens"].as_u64().unwrap_or_default(),
        cache_read: value["cache_read_input_tokens"]
            .as_u64()
            .unwrap_or_default(),
        cache_write: value["cache_creation_input_tokens"]
            .as_u64()
            .unwrap_or_default(),
        ..Usage::default()
    }
}

/// How much of the context a request's usage fills: none when it doesn't say its input.
fn step_context(usage: &Usage) -> Option<u64> {
    let used = usage.input + usage.output + usage.reasoning + usage.cache_read + usage.cache_write;
    (used > 0).then_some(used)
}

fn tool(id: &str, name: &str, state: ToolState) -> TurnEvent {
    TurnEvent::Tool {
        id: id.into(),
        name: name.into(),
        state,
        metadata: Map::new(),
    }
}

/// The limits Claude Code tells of, by its name for each: the plan's five hours and week, and
/// each model's own week.
fn heard_shape(id: &str) -> Option<(&'static str, u32, Option<&'static str>)> {
    Some(match id {
        "five_hour" => ("five_hour", 300, None),
        "seven_day" => ("seven_day", 10_080, None),
        "seven_day_sonnet" => ("seven_day_sonnet", 10_080, Some("Sonnet")),
        "seven_day_opus" => ("seven_day_opus", 10_080, Some("Opus")),
        "seven_day_overage_included" => ("seven_day_fable", 10_080, Some("Fable")),
        _ => return None,
    })
}

/// A number as Claude Code may write it: a number, or one in words.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|number| number.is_finite())
}

/// Where each of the plan's limits stands, as a rate-limit event says: every window it names
/// (Claude Code 2.1.3xx tells them all), or else the one it's about.
pub(super) fn heard_windows(info: &Value) -> Vec<LimitWindow> {
    let window =
        |shape: (&str, u32, Option<&str>), used: f64, resets_at: Option<u64>| LimitWindow {
            id: shape.0.to_owned(),
            used,
            minutes: Some(shape.1),
            resets_at,
            model: shape.2.map(str::to_owned),
        };
    if let Some(unified) = info["unifiedWindows"].as_object() {
        let windows: Vec<_> = unified
            .iter()
            .filter_map(|(id, each)| {
                let shape = heard_shape(id)?;
                let used = number(&each["utilization"])?;
                let resets_at = number(&each["resetsAt"])
                    .filter(|seconds| *seconds > 0.0)
                    .map(|seconds| (seconds * 1000.0) as u64);
                Some(window(shape, used, resets_at))
            })
            .collect();
        if !windows.is_empty() {
            return windows;
        }
    }
    let Some(shape) = info["rateLimitType"].as_str().and_then(heard_shape) else {
        return Vec::new();
    };
    let resets_at = info["resetsAt"]
        .as_f64()
        .map(|seconds| (seconds * 1000.0) as u64);
    // How much is used, as it says; turned away with no word, all of it.
    let used = match info["utilization"].as_f64() {
        Some(used) => used,
        None if info["status"] == "rejected" => 1.0,
        None => return Vec::new(),
    };
    vec![window(shape, used, resets_at)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_limit_event_says_where_each_window_stands() {
        // As Claude Code 2.1.3xx tells it: every window, from the response's headers.
        let info = json!({
            "status": "allowed_warning", "rateLimitType": "seven_day", "utilization": 0.81,
            "resetsAt": 1_794_200_000, "surpassedThreshold": 0.75,
            "unifiedWindows": {
                "five_hour": { "utilization": 0.3, "resetsAt": 1_794_100_000 },
                "seven_day": { "utilization": 0.81, "resetsAt": 1_794_200_000, "status": "allowed_warning" },
                "something_new": { "utilization": 0.5 }
            }
        });
        let windows = heard_windows(&info);
        assert_eq!(
            windows
                .iter()
                .map(|window| (
                    window.id.as_str(),
                    window.used,
                    window.minutes,
                    window.resets_at
                ))
                .collect::<Vec<_>>(),
            [
                ("five_hour", 0.3, Some(300), Some(1_794_100_000_000)),
                ("seven_day", 0.81, Some(10_080), Some(1_794_200_000_000)),
            ]
        );
        // An older one names only the limit it's about; turned away without a figure, all of it.
        let only = heard_windows(
            &json!({ "status": "allowed", "rateLimitType": "five_hour", "utilization": 0.8 }),
        );
        assert_eq!(
            (only[0].id.as_str(), only[0].used, only[0].resets_at),
            ("five_hour", 0.8, None)
        );
        let turned = heard_windows(
            &json!({ "status": "rejected", "rateLimitType": "seven_day_overage_included", "resetsAt": 10 }),
        );
        assert_eq!(
            turned,
            [LimitWindow {
                id: "seven_day_fable".into(),
                used: 1.0,
                minutes: Some(10_080),
                resets_at: Some(10_000),
                model: Some("Fable".into())
            }]
        );
        assert!(
            heard_windows(&json!({ "status": "allowed", "rateLimitType": "five_hour" })).is_empty()
        );
        assert!(
            heard_windows(&json!({ "rateLimitType": "elsewhere", "utilization": 0.1 })).is_empty()
        );
    }
}
