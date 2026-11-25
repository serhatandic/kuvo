//! What a tool came back with, and Claude Code's own account of it (`tool_use_result`): a
//! command's output and notes, an edit's change, the pictures it brought back.

use std::{fs, path::Path};

use serde_json::{Map, Value};

use crate::{
    file_change::{file_change, patch_change},
    pictures::{file_url, picture_data, picture_url},
    session::{ToolOutput, ToolState, TurnError},
};

use super::projection::items;

const OUTPUT_LIMIT: usize = 30_000;

/// The finished state of a call to `native` with `raw` input (shown as `input`), and its
/// metadata (an edit's `files`).
pub(super) fn outcome(
    native: &str,
    raw: &Value,
    input: &Value,
    block: &Value,
    structured: Option<&Value>,
) -> (ToolState, Map<String, Value>) {
    let mut text = limited(tool_text(&block["content"]));
    let mut pictures = tool_pictures(&block["content"], raw);
    let mut metadata = Map::new();
    if block["is_error"] == true {
        let content = with_pictures(text.clone(), pictures);
        let message = if text.is_empty() {
            "The tool failed.".into()
        } else {
            text
        };
        let error = TurnError {
            kind: "tool".into(),
            message,
            status: None,
            limit: None,
        };
        let state = ToolState::Failed {
            input: input.clone(),
            content,
            error,
        };
        return (state, metadata);
    }
    let files = diffs(native, raw, structured);
    if !files.is_empty() {
        metadata.insert("files".into(), Value::Array(files));
    }
    if native == "Bash"
        && let Some(structured) = structured.filter(|value| value.is_object())
    {
        let stdout = structured["stdout"].as_str();
        if structured["isImage"] == true
            && let Some(picture) = stdout.and_then(|out| picture_url(out.trim()))
        {
            pictures.push(picture);
        } else if structured["isImage"] != true {
            let output = [stdout, structured["stderr"].as_str()]
                .into_iter()
                .flatten()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if !output.is_empty() {
                text = limited(output);
            }
        }
        let notes = [
            (structured["interrupted"] == true).then(|| "It was stopped.".to_owned()),
            structured["returnCodeInterpretation"]
                .as_str()
                .filter(|note| !note.is_empty())
                .map(str::to_owned),
            structured["persistedOutputPath"]
                .as_str()
                .filter(|path| !path.is_empty())
                .map(|path| format!("The rest of what it printed is in {path}.")),
        ];
        let notes = notes.into_iter().flatten().collect::<Vec<_>>();
        if !notes.is_empty() {
            text = std::iter::once(text)
                .filter(|text| !text.is_empty())
                .chain(notes)
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    let state = ToolState::Completed {
        input: input.clone(),
        content: with_pictures(text, pictures),
    };
    (state, metadata)
}

fn limited(mut text: String) -> String {
    if text.chars().count() > OUTPUT_LIMIT {
        text = text.chars().take(OUTPUT_LIMIT).collect::<String>() + "\n…";
    }
    text
}

fn with_pictures(text: String, pictures: Vec<ToolOutput>) -> Vec<ToolOutput> {
    std::iter::once(ToolOutput::Text(text))
        .chain(pictures)
        .collect()
}

fn tool_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.into();
    }
    items(content)
        .iter()
        .filter_map(|part| match part["type"].as_str() {
            Some("tool_reference") => Some(format!(
                "Loaded {}",
                part["tool_name"].as_str().unwrap_or("a tool")
            )),
            Some("search_result") => Some(
                [part["title"].as_str(), part["source"].as_str()]
                    .into_iter()
                    .flatten()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" — "),
            ),
            Some("image" | "document") => None,
            _ => part["text"].as_str().map(str::to_owned),
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_pictures(content: &Value, raw: &Value) -> Vec<ToolOutput> {
    items(content)
        .iter()
        .filter_map(|part| match part["type"].as_str()? {
            "image" if part["source"]["type"] == "base64" => picture_data(
                part["source"]["data"].as_str()?,
                part["source"]["media_type"].as_str()?,
            ),
            "image" => picture_url(part["source"]["url"].as_str()?),
            "document" => {
                let path = raw["file_path"].as_str()?;
                Some(ToolOutput::File {
                    uri: file_url(path),
                    mime: "application/pdf".into(),
                    name: path.rsplit('/').next().map(str::to_owned),
                })
            }
            _ => None,
        })
        .collect()
}

/// What an edit did, from Claude Code's own account of it.
fn diffs(native: &str, raw: &Value, structured: Option<&Value>) -> Vec<Value> {
    if !matches!(native, "Edit" | "MultiEdit" | "Write" | "NotebookEdit") {
        return Vec::new();
    }
    let structured = structured.unwrap_or(&Value::Null);
    let path = [
        &structured["filePath"],
        &raw["file_path"],
        &raw["notebook_path"],
    ]
    .into_iter()
    .find_map(Value::as_str)
    .unwrap_or_default();
    if path.is_empty() {
        return Vec::new();
    }
    if native == "Write" && structured["type"] == "create" {
        let content = structured["content"]
            .as_str()
            .or(raw["content"].as_str())
            .unwrap_or_default();
        return vec![file_change(path, None, Some(content))];
    }
    let hunks = items(&structured["structuredPatch"]);
    if !hunks.is_empty() {
        let patch = hunks
            .iter()
            .map(|hunk| {
                let lines = items(&hunk["lines"])
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "@@ -{},{} +{},{} @@\n{lines}\n",
                    number(&hunk["oldStart"]),
                    number(&hunk["oldLines"]),
                    number(&hunk["newStart"]),
                    number(&hunk["newLines"]),
                )
            })
            .collect::<String>();
        return vec![patch_change(path, &patch, None)];
    }
    if let Some(original) = structured["originalFile"].as_str()
        && Path::new(path).exists()
    {
        let now = fs::read_to_string(path).unwrap_or_default();
        return vec![file_change(path, Some(original), Some(&now))];
    }
    if native == "NotebookEdit"
        && (structured["old_source"].is_string() || structured["new_source"].is_string())
    {
        return vec![file_change(
            path,
            Some(structured["old_source"].as_str().unwrap_or_default()),
            Some(structured["new_source"].as_str().unwrap_or_default()),
        )];
    }
    Vec::new()
}

fn number(value: &Value) -> String {
    match value {
        Value::Number(number) => number.to_string(),
        other => other.as_str().unwrap_or("undefined").to_owned(),
    }
}
