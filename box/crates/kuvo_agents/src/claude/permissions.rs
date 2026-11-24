use std::{io, sync::Arc};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::session::{
    Answer, Decision, Field, FieldKind, FormOption, PermissionAsk, Preflight, QuestionAsk, TurnSink,
};

#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(super) enum Request {
    CanUseTool {
        tool_name: String,
        input: Value,
        tool_use_id: Option<String>,
        #[serde(default)]
        permission_suggestions: Vec<Value>,
        title: Option<String>,
        description: Option<String>,
    },
    HookCallback {
        callback_id: String,
        input: Hook,
        tool_use_id: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(super) struct Hook {
    hook_event_name: String,
    tool_name: Option<String>,
    tool_input: Option<Value>,
    tool_use_id: Option<String>,
}

impl Request {
    pub fn stable_id<'a>(&'a self, request: &'a str) -> &'a str {
        match self {
            Self::CanUseTool {
                tool_use_id: Some(id),
                ..
            } => id,
            // A hook cancellation must not withdraw a separate can_use_tool ask.
            _ => request,
        }
    }

    /// What Claude Code is told when the turn asking this is stopped: no.
    pub fn stopped(&self) -> Option<Value> {
        match self {
            Self::CanUseTool {
                tool_name,
                tool_use_id,
                ..
            } => Some(deny(
                if tool_name == "AskUserQuestion" {
                    "The user dismissed the question."
                } else {
                    "The user declined this."
                },
                tool_use_id.clone(),
            )),
            _ => None,
        }
    }

    pub async fn answer(
        self,
        sink: Option<Arc<dyn TurnSink>>,
        id: String,
        native_plan: bool,
    ) -> io::Result<Value> {
        match self {
            Self::HookCallback {
                callback_id,
                input,
                tool_use_id,
            } => {
                if callback_id != "kuvo-pre-tool-use" || input.hook_event_name != "PreToolUse" {
                    return Err(io::Error::other("Unknown Claude Code hook."));
                }
                let tool = input.tool_name.unwrap_or_default();
                let input_data = input.tool_input.unwrap_or(Value::Null);
                let ask = permission(
                    id,
                    &tool,
                    &input_data,
                    tool_use_id.or(input.tool_use_id),
                    None,
                );
                let verdict = match sink {
                    Some(sink) => sink.preflight(ask, native_plan).await?,
                    None => Preflight::Deny,
                };
                Ok(match verdict {
                    Preflight::NoOverride => json!({}),
                    Preflight::Ask => json!({"hookSpecificOutput": {
                        "hookEventName":"PreToolUse", "permissionDecision":"ask"
                    }}),
                    Preflight::Deny => json!({"hookSpecificOutput": {
                        "hookEventName":"PreToolUse", "permissionDecision":"deny",
                        "permissionDecisionReason":"Kuvo Code's settings don't allow this (Settings › Permissions, or this session's rules)."
                    }}),
                })
            }
            Self::CanUseTool {
                tool_name,
                input,
                tool_use_id,
                permission_suggestions,
                title,
                description,
            } => {
                let Some(sink) = sink else {
                    return Ok(deny("There's no turn under way for this.", tool_use_id));
                };
                if tool_name == "ExitPlanMode" {
                    return Ok(if native_plan {
                        deny(
                            "The plan is shown to the user. Stop here and wait: they'll switch out of plan mode and ask you to build it when they're ready.",
                            tool_use_id,
                        )
                    } else {
                        allow(input, tool_use_id)
                    });
                }
                if tool_name == "AskUserQuestion" {
                    return answer_question(sink, id, input, tool_use_id).await;
                }
                let ask = permission(
                    id,
                    &tool_name,
                    &input,
                    tool_use_id.clone(),
                    title.or(description),
                );
                let decision = sink.permission(ask).await?;
                if decision == Decision::Reject {
                    return Ok(deny("The user declined this.", tool_use_id));
                }
                let mut response = allow(input, tool_use_id);
                if decision == Decision::Always {
                    response["updatedPermissions"] = Value::Array(
                        permission_suggestions
                            .into_iter()
                            .filter_map(|suggestion| {
                                let mut suggestion = suggestion.as_object()?.clone();
                                suggestion.insert("destination".into(), "session".into());
                                Some(Value::Object(suggestion))
                            })
                            .collect(),
                    );
                }
                Ok(response)
            }
            Self::Other => Err(io::Error::other("Unknown Claude Code control request.")),
        }
    }
}

pub(super) fn tool_name(name: &str) -> String {
    match name {
        "Bash" | "code_execution" | "bash_code_execution" => "shell".into(),
        "Edit" | "MultiEdit" | "NotebookEdit" => "edit".into(),
        "Task" | "Agent" => "subagent".into(),
        "AskUserQuestion" => "question".into(),
        "ExitPlanMode" => "plan".into(),
        "web_search" => "websearch".into(),
        "web_fetch" => "webfetch".into(),
        name => name
            .strip_prefix("mcp__")
            .and_then(|qualified| qualified.split_once("__"))
            .map_or_else(
                || name.to_ascii_lowercase(),
                |(server, tool)| format!("{server}_{tool}"),
            ),
    }
}

fn permission(
    id: String,
    tool: &str,
    input: &Value,
    tool_call_id: Option<String>,
    message: Option<String>,
) -> PermissionAsk {
    let (action, resource) = match tool {
        "Bash" => ("shell".into(), input["command"].as_str()),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => (
            "edit".into(),
            input["file_path"]
                .as_str()
                .or(input["notebook_path"].as_str()),
        ),
        "Read" | "Glob" | "Grep" | "LS" => (
            "read".into(),
            input["file_path"].as_str().or(input["path"].as_str()),
        ),
        "WebFetch" => ("webfetch".into(), input["url"].as_str()),
        "WebSearch" => ("websearch".into(), input["query"].as_str()),
        name => (tool_name(name), None),
    };
    PermissionAsk {
        id,
        tool_call_id,
        action,
        resources: vec![resource.unwrap_or("*").into()],
        save: Vec::new(),
        message,
        metadata: Map::new(),
        sandboxed: None,
        already_weighed: false,
        unclassified: false,
    }
}

fn deny(message: &str, id: Option<String>) -> Value {
    with_tool_id(json!({"behavior":"deny", "message":message}), id)
}

fn allow(input: Value, id: Option<String>) -> Value {
    with_tool_id(json!({"behavior":"allow", "updatedInput":input}), id)
}

fn with_tool_id(mut response: Value, id: Option<String>) -> Value {
    if let Some(id) = id {
        response["toolUseID"] = id.into();
    }
    response
}

#[derive(Deserialize)]
struct Question {
    question: String,
    header: Option<String>,
    #[serde(rename = "multiSelect", default)]
    multi_select: bool,
    #[serde(default)]
    options: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    label: String,
    description: Option<String>,
}

async fn answer_question(
    sink: Arc<dyn TurnSink>,
    id: String,
    mut input: Value,
    tool_call_id: Option<String>,
) -> io::Result<Value> {
    let questions: Vec<Question> = serde_json::from_value(input["questions"].clone())?;
    let fields = questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let options = question
                .options
                .iter()
                .map(|option| FormOption {
                    value: option.label.clone(),
                    label: option.label.clone(),
                    description: option.description.clone(),
                })
                .collect();
            Field {
                key: format!("q{index}"),
                title: question
                    .header
                    .clone()
                    .or_else(|| Some(format!("Question {}", index + 1))),
                description: Some(question.question.clone()),
                required: true,
                kind: if question.multi_select {
                    FieldKind::MultiSelect {
                        options,
                        custom: true,
                    }
                } else {
                    FieldKind::String {
                        options,
                        custom: true,
                        format: None,
                    }
                },
            }
        })
        .collect();
    let title = match questions.as_slice() {
        [only] => only.header.clone().unwrap_or_else(|| "Question".into()),
        _ => "Questions".into(),
    };
    let answer = sink
        .question(QuestionAsk {
            id,
            title,
            fields,
            tool_call_id: tool_call_id.clone(),
            metadata: Map::new(),
        })
        .await?;
    match answer {
        Answer::Dismissed { reason } => Ok(deny(
            reason
                .as_deref()
                .unwrap_or("The user dismissed the question."),
            tool_call_id,
        )),
        Answer::Submitted(values) => {
            let answers: Map<String, Value> = questions
                .into_iter()
                .enumerate()
                .map(|(index, question)| {
                    let value = match &values.get(&format!("q{index}")) {
                        Some(Value::Array(items)) => items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                        Some(Value::String(value)) => value.clone(),
                        _ => String::new(),
                    };
                    (question.question, Value::String(value))
                })
                .collect();
            input["answers"] = Value::Object(answers);
            Ok(allow(input, tool_call_id))
        }
    }
}
