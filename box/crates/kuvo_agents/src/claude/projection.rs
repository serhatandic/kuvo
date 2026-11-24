//! Project native messages to ordered, replayable resident turn events.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::session::{
    Limit, LimitScope, ModelRef, TextKind, ToolOutput, ToolState, TurnError, TurnEvent, Usage,
    VendorState,
};

use super::{diagnostic::redact, permissions::tool_name};

#[derive(Default)]
pub(super) struct Projection {
    streamed: HashSet<String>,
    tools: HashMap<String, Tool>,
    blocks: HashMap<u64, String>,
    step: Option<Usage>,
    model: Option<String>,
    used: Option<u64>,
    finish: Option<String>,
    explicit_compaction: bool,
    compacting: bool,
    compact_failed: bool,
    error: Option<TurnError>,
    notices: HashSet<String>,
}

struct Tool {
    name: String,
    input: Value,
}

impl Projection {
    pub fn new(explicit_compaction: bool) -> Self {
        Self {
            explicit_compaction,
            ..Self::default()
        }
    }

    pub fn frame(
        &mut self,
        frame: &Value,
        model: &ModelRef,
        state: &mut VendorState,
    ) -> Vec<TurnEvent> {
        if frame["parent_tool_use_id"].is_string() {
            return Vec::new();
        }
        let mut events = Vec::new();
        match frame["type"].as_str() {
            Some("stream_event") => self.stream(&frame["event"], model, &mut events),
            Some("assistant") => self.assistant(frame, model, &mut events),
            Some("user") => self.tools_done(&frame["message"]["content"], &mut events),
            Some("conversation_reset") => {
                self.used = None;
                state.remove("claudeCost");
                if let Some(native) = frame["new_conversation_id"].as_str() {
                    state.insert("claudeSession".into(), native.into());
                    state.insert("claudeStarted".into(), true.into());
                } else {
                    state.remove("claudeSession");
                    state.remove("claudeStarted");
                }
                events.push(TurnEvent::ContextCleared);
                events.push(TurnEvent::Notice(
                    "Claude Code started its conversation over.".into(),
                ));
            }
            Some("system") => self.system(frame, state, &mut events),
            Some("rate_limit_event") => self.limit(&frame["rate_limit_info"]),
            Some("auth_status") if frame["error"].is_string() => {
                self.error = Some(authentication_error());
            }
            Some("result") => {
                if self.compacting {
                    self.compacting = false;
                    events.push(TurnEvent::CompactionEnded { succeeded: false });
                }
                let window = self
                    .model
                    .as_ref()
                    .and_then(|model| frame["modelUsage"][model]["contextWindow"].as_u64());
                if let Some(size) = window {
                    state.insert("claudeWindow".into(), json!({"model":model.id,"size":size}));
                    events.push(TurnEvent::Context {
                        used: self.used,
                        window,
                    });
                }
                if let Some(total) = frame["total_cost_usd"].as_f64() {
                    state.insert("claudeCost".into(), Value::from(total));
                }
            }
            _ => {}
        }
        events
    }

    pub fn failure(&self, frame: &Value) -> Option<TurnError> {
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.kind == "signin")
        {
            return self.error.clone();
        }
        if frame["subtype"] == "success" && frame["is_error"] != true && !self.compact_failed {
            return None;
        }
        self.error.clone().or_else(|| {
            let status = frame["api_error_status"]
                .as_u64()
                .and_then(|value| u16::try_from(value).ok());
            if status == Some(429) {
                return Some(TurnError {
                    kind: "limit".into(),
                    message: "You've reached your Claude usage limit for now.".into(),
                    status,
                    limit: Some(Limit {
                        scope: LimitScope::Provider,
                        resets_at: None,
                    }),
                });
            }
            Some(TurnError {
                kind: "agent".into(),
                message: frame["result"]
                    .as_str()
                    .filter(|message| !message.trim().is_empty())
                    .map(redact)
                    .unwrap_or_else(|| {
                        match frame["subtype"].as_str() {
                            Some("error_max_turns") => "Claude Code reached its turn limit.",
                            Some("error_max_budget_usd") => {
                                "Claude Code reached its spending limit."
                            }
                            _ => "Claude Code could not finish this turn.",
                        }
                        .into()
                    }),
                status,
                limit: None,
            })
        })
    }

    fn stream(&mut self, event: &Value, model: &ModelRef, out: &mut Vec<TurnEvent>) {
        let index = event["index"].as_u64();
        match event["type"].as_str() {
            Some("message_start") => {
                if self.compacting {
                    self.compacting = false;
                    out.push(TurnEvent::CompactionEnded { succeeded: false });
                }
                let message = &event["message"];
                if let Some(id) = message["id"].as_str() {
                    self.streamed.insert(id.into());
                }
                self.model = message["model"].as_str().map(str::to_owned);
                self.step = Some(usage(&message["usage"]));
                self.finish = None;
                self.blocks.clear();
                out.push(TurnEvent::StepStarted {
                    model: self.model.as_ref().map(|native| ModelRef {
                        id: native.clone(),
                        ..model.clone()
                    }),
                });
            }
            Some("content_block_start") => {
                let block = &event["content_block"];
                if let Some(tool) = block_tool(block) {
                    if let (Some(index), Some(id)) = (index, block["id"].as_str()) {
                        self.blocks.insert(index, id.into());
                        out.push(tool_event(id, &tool, ToolState::Streaming(String::new())));
                        self.tools.insert(id.into(), tool);
                    }
                } else {
                    block_text(block, index.map(|index| index.to_string()), out);
                }
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                let block = index.map(|index| index.to_string());
                match delta["type"].as_str() {
                    Some("text_delta") => text(out, TextKind::Text, block, delta["text"].as_str()),
                    Some("thinking_delta") => {
                        text(out, TextKind::Reasoning, block, delta["thinking"].as_str())
                    }
                    Some("input_json_delta") => {
                        if let Some(id) = index.and_then(|index| self.blocks.get(&index))
                            && let Some(tool) = self.tools.get(id)
                            && let Some(delta) = delta["partial_json"].as_str()
                        {
                            out.push(tool_event(id, tool, ToolState::Streaming(delta.to_owned())));
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(usage) = &mut self.step
                    && let Some(output) = event["usage"]["output_tokens"].as_u64()
                {
                    usage.output = output;
                }
                if let Some(finish) = event["delta"]["stop_reason"].as_str() {
                    self.finish = Some(
                        if finish == "tool_use" {
                            "tool-calls"
                        } else {
                            finish
                        }
                        .into(),
                    );
                }
            }
            Some("message_stop") => self.end_step(out),
            _ => {}
        }
    }

    fn assistant(&mut self, frame: &Value, model: &ModelRef, out: &mut Vec<TurnEvent>) {
        let message = &frame["message"];
        let error = frame["error"].as_str().or(message["error"].as_str());
        if let Some(error) = error {
            self.error = Some(if error == "authentication_failed" {
                authentication_error()
            } else {
                let words = items(&message["content"])
                    .iter()
                    .filter_map(|block| {
                        (block["type"] == "text")
                            .then(|| block["text"].as_str())
                            .flatten()
                    })
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                TurnError {
                    kind: "agent".into(),
                    message: if words.is_empty() {
                        "Claude's API reported an error.".into()
                    } else {
                        redact(&words)
                    },
                    limit: None,
                    status: None,
                }
            });
            return;
        }
        let complete = !message["id"]
            .as_str()
            .is_some_and(|id| self.streamed.contains(id));
        if complete {
            self.model = message["model"].as_str().map(str::to_owned);
            out.push(TurnEvent::StepStarted {
                model: self.model.as_ref().map(|native| ModelRef {
                    id: native.clone(),
                    ..model.clone()
                }),
            });
            self.step = message.get("usage").map(usage);
        }
        for (index, block) in items(&message["content"]).iter().enumerate() {
            if let Some(tool) = block_tool(block) {
                if let Some(id) = block["id"].as_str() {
                    out.push(tool_event(
                        id,
                        &tool,
                        ToolState::Running {
                            input: tool.input.clone(),
                        },
                    ));
                    self.tools.insert(id.into(), tool);
                }
            } else if complete {
                block_text(block, Some(index.to_string()), out);
            }
        }
        if complete {
            self.end_step(out);
        }
    }

    fn end_step(&mut self, out: &mut Vec<TurnEvent>) {
        let usage = self.step.take();
        if let Some(usage) = &usage {
            self.used = Some(
                usage
                    .input
                    .saturating_add(usage.output)
                    .saturating_add(usage.cache_read)
                    .saturating_add(usage.cache_write),
            );
            out.push(TurnEvent::Context {
                used: self.used,
                window: None,
            });
        }
        out.push(TurnEvent::StepEnded {
            usage,
            finish: self.finish.take().or_else(|| Some("stop".into())),
            error: None,
        });
    }

    fn tools_done(&mut self, content: &Value, out: &mut Vec<TurnEvent>) {
        for result in items(content)
            .iter()
            .filter(|block| block["type"] == "tool_result")
        {
            let Some(id) = result["tool_use_id"].as_str() else {
                continue;
            };
            let Some(tool) = self.tools.remove(id) else {
                continue;
            };
            let content = outputs(&result["content"]);
            let state = if result["is_error"] == true {
                let message = content
                    .iter()
                    .find_map(|part| match part {
                        ToolOutput::Text(text) if !text.trim().is_empty() => Some(text.to_owned()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "The tool failed.".into());
                ToolState::Failed {
                    input: tool.input.clone(),
                    content,
                    error: TurnError {
                        kind: "tool".into(),
                        message,
                        status: None,
                        limit: None,
                    },
                }
            } else {
                ToolState::Completed {
                    input: tool.input.clone(),
                    content,
                }
            };
            out.push(tool_event(id, &tool, state));
        }
    }

    fn system(&mut self, frame: &Value, state: &mut VendorState, out: &mut Vec<TurnEvent>) {
        match frame["subtype"].as_str() {
            Some("init") => {
                if let Some(id) = frame["session_id"].as_str() {
                    state.insert("claudeSession".into(), id.into());
                    state.insert("claudeStarted".into(), true.into());
                }
            }
            Some("status") if frame["status"] == "compacting" => {
                if !self.compacting {
                    self.compacting = true;
                    out.push(TurnEvent::CompactionStarted);
                }
            }
            Some("status") if self.compacting && frame["compact_result"] == "failed" => {
                self.compacting = false;
                self.compact_failed = self.explicit_compaction;
                let message = frame["compact_error"].as_str().map(redact).map_or_else(
                    || "Claude Code couldn't summarize the conversation.".into(),
                    |error| format!("Claude Code couldn't summarize the conversation: {}", error),
                );
                if self.explicit_compaction {
                    self.error = Some(TurnError {
                        kind: "agent".into(),
                        message,
                        status: None,
                        limit: None,
                    });
                } else {
                    out.push(TurnEvent::Notice(message));
                }
                out.push(TurnEvent::CompactionEnded { succeeded: false });
            }
            Some("compact_boundary") => {
                self.compacting = false;
                self.used = frame["compact_metadata"]["post_tokens"].as_u64();
                out.push(
                    self.used
                        .map_or(TurnEvent::ContextCleared, |used| TurnEvent::Context {
                            used: Some(used),
                            window: None,
                        }),
                );
                out.push(TurnEvent::CompactionEnded { succeeded: true });
            }
            Some("local_command_output") => {
                text(out, TextKind::Text, None, frame["content"].as_str());
            }
            Some("api_retry") => {
                let attempt = frame["attempt"].as_u64().unwrap_or_default();
                let message = format!("Claude Code is retrying its request (attempt {attempt}).");
                if self.notices.insert(message.clone()) {
                    out.push(TurnEvent::Notice(message));
                }
            }
            _ => {}
        }
    }

    fn limit(&mut self, info: &Value) {
        if info["status"] != "rejected"
            || info["overageInUse"] == true
            || info["isUsingOverage"] == true
        {
            return;
        }
        let scope = if matches!(
            info["rateLimitType"].as_str(),
            Some("seven_day_opus" | "seven_day_sonnet")
        ) {
            LimitScope::Model
        } else {
            LimitScope::Provider
        };
        self.error = Some(TurnError {
            kind: "limit".into(),
            message: "You've reached a Claude usage limit.".into(),
            status: Some(429),
            limit: Some(Limit {
                scope,
                resets_at: info["resetsAt"]
                    .as_u64()
                    .map(|time| time.saturating_mul(1000)),
            }),
        });
    }
}

fn authentication_error() -> TurnError {
    TurnError {
        kind: "signin".into(),
        message: "Claude ended this sign-in, and Claude Code couldn't renew it.".into(),
        status: Some(401),
        limit: None,
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

fn block_tool(block: &Value) -> Option<Tool> {
    let raw = block["name"].as_str()?;
    let name = match block["type"].as_str()? {
        "tool_use" | "server_tool_use" => tool_name(raw),
        "mcp_tool_use" => tool_name(&format!(
            "mcp__{}__{raw}",
            block["server_name"].as_str().unwrap_or("mcp")
        )),
        _ => return None,
    };
    let input = block["input"].clone();
    let input = if let Value::Object(mut input) = input {
        for (key, target) in [
            ("file_path", "path"),
            ("notebook_path", "path"),
            ("old_string", "oldString"),
            ("new_string", "newString"),
            ("replace_all", "replaceAll"),
            ("run_in_background", "background"),
            ("subagent_type", "agent"),
        ] {
            if let Some(value) = input.remove(key) {
                input.insert(target.into(), value);
            }
        }
        Value::Object(input)
    } else {
        input
    };
    Some(Tool { name, input })
}

fn tool_event(id: &str, tool: &Tool, state: ToolState) -> TurnEvent {
    TurnEvent::Tool {
        id: id.into(),
        name: tool.name.clone(),
        state,
        metadata: Map::new(),
    }
}

fn text(out: &mut Vec<TurnEvent>, kind: TextKind, block: Option<String>, delta: Option<&str>) {
    if let Some(delta) = delta.filter(|delta| !delta.is_empty()) {
        out.push(TurnEvent::TextDelta {
            kind,
            block,
            delta: delta.into(),
        });
    }
}

fn block_text(block: &Value, key: Option<String>, out: &mut Vec<TurnEvent>) {
    match block["type"].as_str() {
        Some("text") => text(out, TextKind::Text, key, block["text"].as_str()),
        Some("thinking") => text(out, TextKind::Reasoning, key, block["thinking"].as_str()),
        Some("redacted_thinking") => text(
            out,
            TextKind::Reasoning,
            key,
            Some("(The model kept this thinking to itself.)"),
        ),
        _ => {}
    }
}

fn outputs(content: &Value) -> Vec<ToolOutput> {
    if let Some(text) = content.as_str() {
        return vec![ToolOutput::Text(text.into())];
    }
    items(content)
        .iter()
        .filter_map(|block| match block["type"].as_str()? {
            "text" => block["text"]
                .as_str()
                .map(|text| ToolOutput::Text(text.into())),
            "image" if block["source"]["type"] == "base64" => {
                let mime = block["source"]["media_type"].as_str()?;
                let data = block["source"]["data"].as_str()?;
                Some(ToolOutput::File {
                    uri: format!("data:{mime};base64,{data}"),
                    mime: mime.into(),
                    name: None,
                })
            }
            _ => None,
        })
        .collect()
}
