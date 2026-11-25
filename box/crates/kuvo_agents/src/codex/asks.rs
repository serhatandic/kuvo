//! What Codex asks you: a command or a change to approve, more access, a question of its own or an
//! MCP server's form. Always is kept by the resident, never as Codex's own for its session: that
//! would let what comes after through without your rules.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::{
    rpc::RpcError,
    session::{
        Answer, Decision, Field, FieldKind, FormOption, PermissionAsk, QuestionAsk, TurnSink,
    },
};

use super::projection::{Change, TOOLS_SERVER, change_diff, unwrap_shell};

/// The answer Codex hears when no turn here has the thread asking.
pub(super) fn unasked(method: &str) -> Result<Value, RpcError> {
    Ok(match method {
        "item/tool/requestUserInput" => json!({ "answers": {} }),
        "mcpServer/elicitation/request" => json!({ "action": "decline" }),
        _ if method.ends_with("Approval") || method.ends_with("/requestApproval") => {
            if method.starts_with("item/") {
                json!({ "decision": "decline" })
            } else {
                json!({ "decision": "denied" })
            }
        }
        _ => json!({}),
    })
}

fn ask(
    id: &str,
    call: Option<&Value>,
    action: &str,
    resources: Vec<String>,
    message: &Value,
) -> PermissionAsk {
    PermissionAsk {
        id: id.to_owned(),
        tool_call_id: call.and_then(Value::as_str).map(str::to_owned),
        action: action.to_owned(),
        resources,
        save: Vec::new(),
        message: message.as_str().map(str::to_owned),
        metadata: Map::new(),
        sandboxed: Some(false),
        already_weighed: false,
        unclassified: false,
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Codex's request, answered through `sink` (`changes`: the file changes its turn has shown, by
/// item). `planning`: in Plan, another server's MCP tool still asks.
pub(super) async fn answer(
    sink: Arc<dyn TurnSink>,
    id: String,
    method: &str,
    params: Value,
    changes: Vec<Change>,
    planning: bool,
) -> Result<Value, RpcError> {
    let failed = |error: std::io::Error| RpcError::new(-32603, error.to_string());
    let rejected = |decision: Decision| decision == Decision::Reject;
    match method {
        "item/commandExecution/requestApproval" => {
            // A command, or a connection to a site it wants (its sandbox keeps it off the network).
            let command = unwrap_shell(params["command"].as_str().unwrap_or_default());
            let host = params["networkApprovalContext"]["host"]
                .as_str()
                .filter(|_| command.is_empty());
            let request = match host {
                Some(host) => {
                    let mut request = ask(
                        &id,
                        Some(&params["itemId"]),
                        "webfetch",
                        vec![host.to_owned()],
                        &params["reason"],
                    );
                    request
                        .message
                        .get_or_insert_with(|| format!("Codex wants to reach {host}."));
                    request
                }
                None => {
                    let resource = if command.is_empty() {
                        "*".into()
                    } else {
                        command
                    };
                    ask(
                        &id,
                        Some(&params["itemId"]),
                        "shell",
                        vec![resource],
                        &params["reason"],
                    )
                }
            };
            let decision = sink.permission(request).await.map_err(failed)?;
            Ok(json!({ "decision": if rejected(decision) { "decline" } else { "accept" } }))
        }
        "item/fileChange/requestApproval" => {
            let files: Vec<Value> = changes.iter().map(change_diff).collect();
            // (A rename's new name too: it's written there.)
            let resources = if changes.is_empty() {
                vec![params["grantRoot"].as_str().unwrap_or("*").to_owned()]
            } else {
                changes
                    .iter()
                    .flat_map(|change| {
                        std::iter::once(change.path.clone()).chain(change.moved_to.clone())
                    })
                    .collect()
            };
            let mut request = ask(
                &id,
                Some(&params["itemId"]),
                "edit",
                resources,
                &params["reason"],
            );
            if !files.is_empty() {
                request.metadata.insert("files".into(), Value::Array(files));
            }
            let decision = sink.permission(request).await.map_err(failed)?;
            Ok(json!({ "decision": if rejected(decision) { "decline" } else { "accept" } }))
        }
        "item/permissions/requestApproval" => {
            let asked = &params["permissions"];
            let mut paths = strings(&asked["fileSystem"]["read"]);
            paths.extend(strings(&asked["fileSystem"]["write"]));
            let network = asked["network"]["enabled"] == true;
            let (action, resources) = match (paths.is_empty(), network) {
                (false, _) => ("external_directory", paths),
                (true, true) => ("webfetch", vec!["the network".to_owned()]),
                (true, false) => ("external_directory", vec!["*".to_owned()]),
            };
            let decision = sink
                .permission(ask(
                    &id,
                    Some(&params["itemId"]),
                    action,
                    resources,
                    &params["reason"],
                ))
                .await
                .map_err(failed)?;
            Ok(if rejected(decision) {
                json!({ "permissions": {}, "scope": "turn" })
            } else {
                json!({ "permissions": asked, "scope": "turn" })
            })
        }
        "execCommandApproval" => {
            let command = match &params["command"] {
                Value::Array(parts) => parts
                    .iter()
                    .map(|part| {
                        part.as_str()
                            .map_or_else(|| part.to_string(), str::to_owned)
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            let decision = sink
                .permission(ask(
                    &id,
                    Some(&params["callId"]),
                    "shell",
                    vec![command],
                    &params["reason"],
                ))
                .await
                .map_err(failed)?;
            Ok(json!({ "decision": if rejected(decision) { "denied" } else { "approved" } }))
        }
        "applyPatchApproval" => {
            // (Each file, and a rename's new name: it's written there.)
            let mut paths = Vec::new();
            for (file, change) in params["fileChanges"].as_object().into_iter().flatten() {
                paths.push(file.clone());
                for moved in [&change["move_path"], &change["update"]["move_path"]] {
                    if let Some(moved) = moved.as_str() {
                        paths.push(moved.to_owned());
                    }
                }
            }
            if paths.is_empty() {
                paths.push("*".into());
            }
            let decision = sink
                .permission(ask(
                    &id,
                    Some(&params["callId"]),
                    "edit",
                    paths,
                    &params["reason"],
                ))
                .await
                .map_err(failed)?;
            Ok(json!({ "decision": if rejected(decision) { "denied" } else { "approved" } }))
        }
        "item/tool/requestUserInput" => {
            let questions = params["questions"].as_array().cloned().unwrap_or_default();
            let fields = questions
                .iter()
                .map(|question| {
                    let options: Vec<FormOption> = question["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|option| FormOption {
                            value: option["label"].as_str().unwrap_or_default().into(),
                            label: option["label"].as_str().unwrap_or_default().into(),
                            description: option["description"].as_str().map(str::to_owned),
                        })
                        .collect();
                    let custom = question["isOther"].as_bool().unwrap_or(options.is_empty());
                    Field {
                        key: question["id"].as_str().unwrap_or_default().into(),
                        title: Some(question["header"].as_str().unwrap_or("Question").into()),
                        description: question["question"].as_str().map(str::to_owned),
                        required: true,
                        kind: FieldKind::String {
                            options,
                            custom,
                            // A secret (a password, a key): hidden as it's typed.
                            format: (question["isSecret"] == true).then(|| "password".into()),
                        },
                    }
                })
                .collect();
            let title = match questions.as_slice() {
                [one] => one["header"].as_str().unwrap_or("Question").to_owned(),
                _ => "Questions".to_owned(),
            };
            let answer = sink
                .question(QuestionAsk {
                    id,
                    title,
                    fields,
                    tool_call_id: params["itemId"].as_str().map(str::to_owned),
                    metadata: Map::new(),
                })
                .await
                .map_err(failed)?;
            let mut answers = Map::new();
            for question in &questions {
                let key = question["id"].as_str().unwrap_or_default();
                // (Turned down by its box with words: that's what it hears, never as a secret.)
                let value = match &answer {
                    Answer::Submitted(values) => values.get(key).cloned(),
                    Answer::Dismissed {
                        reason: Some(words),
                    } if question["isSecret"] != true => Some(json!(words)),
                    Answer::Dismissed { .. } => None,
                };
                let said: Vec<Value> = match value {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(items)) => {
                        items.iter().map(|item| json!(text(item))).collect()
                    }
                    Some(other) => vec![json!(text(&other))],
                };
                answers.insert(key.to_owned(), json!({ "answers": said }));
            }
            Ok(json!({ "answers": answers }))
        }
        "mcpServer/elicitation/request" => elicitation(sink, id, params, planning)
            .await
            .map_err(failed),
        _ => Err(RpcError::not_found(method)),
    }
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

async fn elicitation(
    sink: Arc<dyn TurnSink>,
    id: String,
    params: Value,
    planning: bool,
) -> std::io::Result<Value> {
    let server = params["serverName"].as_str().unwrap_or("mcp").to_owned();
    // Codex asking before an MCP tool runs: your permissions answer it, as any agent's tool. In
    // Plan, another server's tool, which may change things, is still asked about below.
    if params["_meta"]["codex_approval_kind"] == "mcp_tool_call"
        && (!planning || server == TOOLS_SERVER)
    {
        let named = params["message"].as_str().and_then(|message| {
            let start = message.find("tool \"")? + 6;
            let end = message[start..].find('"')?;
            Some(message[start..start + end].to_owned())
        });
        let tool = params["_meta"]["tool_name"]
            .as_str()
            .map(str::to_owned)
            .or(named)
            .unwrap_or_else(|| "tool".into());
        let mut request = ask(
            &id,
            None,
            &format!("{server}_{tool}"),
            vec!["*".into()],
            &params["message"],
        );
        request.unclassified = server != TOOLS_SERVER;
        let decision = sink.permission(request).await?;
        return Ok(
            json!({ "action": if decision == Decision::Reject { "decline" } else { "accept" }, "content": null }),
        );
    }
    // An MCP server asks you something (a form), or to open a page (a sign-in).
    let named = params["serverName"].as_str().unwrap_or("A tool");
    let title = format!("{named} asks");
    let metadata = json!({ "kind": "mcp-elicitation", "server": named })
        .as_object()
        .cloned()
        .unwrap_or_default();
    if params["mode"] == "url" {
        let answer = sink
            .question(QuestionAsk {
                id,
                title,
                fields: vec![Field {
                    key: "open".into(),
                    title: Some(
                        params["message"]
                            .as_str()
                            .unwrap_or("Open this page to go on.")
                            .into(),
                    ),
                    description: None,
                    required: false,
                    kind: FieldKind::External {
                        url: params["url"].as_str().map(str::to_owned),
                    },
                }],
                tool_call_id: None,
                metadata,
            })
            .await?;
        return Ok(
            json!({ "action": if matches!(answer, Answer::Submitted(_)) { "accept" } else { "decline" } }),
        );
    }
    let schema = &params["requestedSchema"];
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let properties = schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut fields: Vec<Field> = properties
        .iter()
        .map(|(key, field)| elicitation_field(key, field, required.contains(&key.as_str())))
        .collect();
    // Its message goes with the form's first field; a form with no fields is a yes or no.
    if let (Some(first), Some(message)) = (fields.first_mut(), params["message"].as_str()) {
        first.description = Some(match &first.description {
            Some(more) if !more.is_empty() => format!("{message}\n\n{more}"),
            _ => message.to_owned(),
        });
    }
    if fields.is_empty() {
        fields.push(Field {
            key: "ok".into(),
            title: Some(params["message"].as_str().unwrap_or("Go on?").into()),
            description: None,
            required: true,
            kind: FieldKind::Boolean,
        });
    }
    let Answer::Submitted(answer) = sink
        .question(QuestionAsk {
            id,
            title,
            fields,
            tool_call_id: None,
            metadata,
        })
        .await?
    else {
        return Ok(json!({ "action": "decline" }));
    };
    let mut content = Map::new();
    for (key, field) in &properties {
        let Some(value) = answer
            .get(key)
            .filter(|value| !value.is_null() && **value != json!(""))
        else {
            continue;
        };
        let value = match field["type"].as_str() {
            Some("number" | "integer") => json!(text(value).parse::<f64>().ok()),
            Some("boolean") => json!(*value == json!(true) || *value == json!("true")),
            _ => value.clone(),
        };
        content.insert(key.clone(), value);
    }
    Ok(json!({ "action": "accept", "content": content }))
}

/// An MCP server's form field, as a card's.
pub(crate) fn elicitation_field(key: &str, field: &Value, required: bool) -> Field {
    let title = Some(field["title"].as_str().unwrap_or(key).to_owned());
    let description = field["description"].as_str().map(str::to_owned);
    let option = |option: &Value| FormOption {
        value: text(&option["const"]),
        label: option["title"]
            .as_str()
            .map_or_else(|| text(&option["const"]), str::to_owned),
        description: None,
    };
    let kind = match field["type"].as_str() {
        Some("boolean") => FieldKind::Boolean,
        Some("array") => {
            let items = &field["items"];
            let options = match items["anyOf"]
                .as_array()
                .or_else(|| items["oneOf"].as_array())
            {
                Some(named) => named.iter().map(option).collect(),
                None => strings(&items["enum"])
                    .into_iter()
                    .map(|value| FormOption {
                        value: value.clone(),
                        label: value,
                        description: None,
                    })
                    .collect(),
            };
            FieldKind::MultiSelect {
                options,
                custom: false,
            }
        }
        _ => {
            let options: Option<Vec<FormOption>> = match field["oneOf"]
                .as_array()
                .or_else(|| field["anyOf"].as_array())
            {
                Some(named) => Some(named.iter().map(option).collect()),
                None => field["enum"].as_array().map(|values| {
                    values
                        .iter()
                        .enumerate()
                        .map(|(index, value)| FormOption {
                            value: text(value),
                            label: field["enumNames"][index]
                                .as_str()
                                .map_or_else(|| text(value), str::to_owned),
                            description: None,
                        })
                        .collect()
                }),
            };
            FieldKind::String {
                custom: options.is_none(),
                options: options.unwrap_or_default(),
                format: (field["format"] == "password").then(|| "password".into()),
            }
        }
    };
    Field {
        key: key.to_owned(),
        title,
        description,
        required,
        kind,
    }
}
