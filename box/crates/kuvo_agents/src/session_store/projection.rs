//! Transcript columns read by usage and artifacts without decoding tool output.

use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum View {
    Insight,
    Artifacts,
}

pub(super) fn project(body: &Value, view: View) -> Option<Value> {
    let kind = body["type"].as_str()?;
    let mut row = fields(body, &["type", "id", "time"]);
    match kind {
        "user" if view == View::Insight => {
            row.insert("text".into(), json!(""));
            row.insert(
                "metadata".into(),
                fields(&body["metadata"], &["source"]).into(),
            );
        }
        "user" => {
            let files = body["files"].as_array().map(|files| {
                files
                    .iter()
                    .filter(|file| {
                        let uri = if file["source"]["type"] == "uri" {
                            &file["source"]["uri"]
                        } else {
                            file.get("uri")
                                .filter(|value| !value.is_null())
                                .unwrap_or(&file["url"])
                        };
                        uri.as_str().is_some_and(|uri| uri.starts_with("file:"))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            });
            let text = body["text"]
                .as_str()
                .filter(|text| text.contains("<attached-files>"))
                .unwrap_or_default();
            if files.as_ref().is_none_or(Vec::is_empty) && text.is_empty() {
                return None;
            }
            row.insert("text".into(), json!(text));
            if let Some(files) = files {
                row.insert("files".into(), files.into());
            }
        }
        "assistant" => {
            let id = body["id"].as_str()?;
            let content = body["content"]
                .as_array()?
                .iter()
                .filter_map(|part| part_of(part, id, view))
                .collect::<Vec<_>>();
            if view == View::Artifacts && content.is_empty() {
                return None;
            }
            row.extend(fields(body, &["model", "agent", "tokens"]));
            row.insert("content".into(), content.into());
        }
        _ => return None,
    }
    Some(row.into())
}

fn part_of(part: &Value, message: &str, view: View) -> Option<Value> {
    if view == View::Artifacts
        && part["type"] == "text"
        && part["text"]
            .as_str()
            .is_some_and(|text| text.contains(":codex-"))
    {
        return Some(part.clone());
    }
    if part["type"] != "tool" {
        return None;
    }
    let name = part["name"].as_str()?;
    let state = &part["state"];
    let status = state["status"].as_str()?;
    let (input, content) = match view {
        View::Insight
            if status == "completed"
                && matches!(
                    name.to_ascii_lowercase().as_str(),
                    "skill" | "read" | "read_file"
                ) =>
        {
            (
                fields(
                    &state["input"],
                    &["skill", "name", "file_path", "filePath", "path"],
                ),
                Vec::new(),
            )
        }
        View::Artifacts
            if artifact(name)
                || part["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with(&format!("cited-{message}-"))) =>
        {
            let files = state["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|file| {
                    matches!(status, "completed" | "error")
                        && file["type"] == "file"
                        && file["uri"]
                            .as_str()
                            .is_some_and(|uri| uri.starts_with("file:"))
                })
                .cloned()
                .collect();
            (fields(&state["input"], &["path"]), files)
        }
        _ => return None,
    };
    let mut kept = fields(part, &["type", "id", "name"]);
    kept.insert(
        "state".into(),
        json!({ "status": "completed", "input": input, "content": content }),
    );
    Some(kept.into())
}

fn artifact(name: &str) -> bool {
    ["show_file", "generate_image"].iter().any(|suffix| {
        name.strip_suffix(suffix)
            .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('_'))
    })
}

fn fields(value: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|key| Some(((*key).to_owned(), value.get(*key)?.clone())))
        .collect()
}
