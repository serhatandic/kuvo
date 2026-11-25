//! Pictures and files a transcript carries as data, kept as files of their own beside the box's
//! attachments (`<home>/attachments/<session>/transcript-pictures/<sha256>.<ext>`), so the
//! transcript holds where they are rather than their bytes.

use std::path::Path;

use base64::Engine;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const FOLDER: &str = "transcript-pictures";
const LARGEST_URI: usize = 35 * 1024 * 1024;
const LARGEST_FILE: usize = 25 * 1024 * 1024;

/// A message row with its data pictures kept as files under `attachments`.
pub(super) fn retain(attachments: &Path, session: &str, row: &mut Value) {
    if !valid_session(session) {
        return;
    }
    match row["type"].as_str() {
        Some("user") => {
            if let Some(files) = row.get_mut("files").and_then(Value::as_array_mut) {
                for file in files {
                    retain_attachment(attachments, session, file);
                }
            }
        }
        Some("assistant") => {
            let Some(parts) = row.get_mut("content").and_then(Value::as_array_mut) else {
                return;
            };
            for part in parts {
                let state = &mut part["state"];
                if part_is_finished_tool(state)
                    && let Some(outputs) = state.get_mut("content").and_then(Value::as_array_mut)
                {
                    for output in outputs {
                        retain_output(attachments, session, output);
                    }
                }
            }
        }
        _ => {}
    }
}

fn part_is_finished_tool(state: &Value) -> bool {
    matches!(state["status"].as_str(), Some("completed" | "error"))
}

fn valid_session(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|rest| {
        (1..=120).contains(&rest.len())
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}

fn retain_attachment(attachments: &Path, session: &str, file: &mut Value) {
    let mime = file["mime"].as_str().unwrap_or_default().to_owned();
    let uri = match (file["data"].as_str(), &file["source"]) {
        (Some(data), _) => format!("data:{mime};base64,{data}"),
        (None, source) if source["type"] == "uri" => match source["uri"].as_str() {
            Some(uri) => uri.to_owned(),
            None => return,
        },
        _ => return,
    };
    let Some(kept) = retain_uri(attachments, session, &uri, &mime) else {
        return;
    };
    if let Some(fields) = file.as_object_mut() {
        fields.remove("data");
        let mut source = Map::new();
        source.insert("type".into(), "uri".into());
        source.insert("uri".into(), kept.into());
        fields.insert("source".into(), Value::Object(source));
    }
}

fn retain_output(attachments: &Path, session: &str, output: &mut Value) {
    if output["type"] != "file" {
        return;
    }
    let (Some(uri), mime) = (
        output["uri"].as_str(),
        output["mime"].as_str().unwrap_or_default(),
    ) else {
        return;
    };
    if let Some(kept) = retain_uri(attachments, session, uri, mime) {
        output["uri"] = kept.into();
    }
}

/// A `data:` URI's bytes saved as a file, and that file's URL; none leaves the URI as it is.
fn retain_uri(attachments: &Path, session: &str, uri: &str, mime: &str) -> Option<String> {
    let rest = uri.strip_prefix("data:")?;
    let comma = rest.find(',')?;
    if uri.len() - (comma + 5) > LARGEST_URI {
        return None;
    }
    let (head, payload) = (&rest[..comma], &rest[comma + 1..]);
    let data = if head.to_ascii_lowercase().ends_with(";base64") {
        if !payload
            .trim_end_matches('=')
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
            || payload.len() - payload.trim_end_matches('=').len() > 2
        {
            return None;
        }
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload))
            .ok()?
    } else {
        percent_decode_str(payload).collect()
    };
    if data.is_empty() || data.len() > LARGEST_FILE {
        return None;
    }
    let hash = hex::encode(Sha256::digest(&data));
    let file = attachments
        .join(session)
        .join(FOLDER)
        .join(format!("{hash}.{}", extension(mime)));
    if !file.exists() {
        // A full disk must leave the original picture readable.
        let folder = file.parent()?;
        std::fs::create_dir_all(folder).ok()?;
        kuvo_store::files::write_secret(&file, &data).ok()?;
    }
    Some(file_url(&file))
}

fn extension(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "image/heif" => "heif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "application/json" => "json",
        _ => "bin",
    }
}

fn file_url(path: &Path) -> String {
    const PATH: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}');
    let text = path.to_string_lossy().replace('\\', "/");
    let text = if text.starts_with('/') {
        text
    } else {
        format!("/{text}")
    };
    format!("file://{}", utf8_percent_encode(&text, PATH))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_a_tools_picture_as_a_file_once() {
        let root = tempfile::tempdir().unwrap();
        let mut row = json!({"type":"assistant","content":[{"type":"tool","state":{
            "status":"completed","content":[{"type":"text","text":""},{"type":"file","uri":"data:image/png;base64,CCCC","mime":"image/png"}]
        }}]});
        retain(root.path(), "ses_1", &mut row);
        let uri = row["content"][0]["state"]["content"][1]["uri"]
            .as_str()
            .unwrap();
        assert!(uri.starts_with("file://"), "{uri}");
        assert!(uri.ends_with(&format!(
            "/ses_1/{FOLDER}/{}.png",
            hex::encode(Sha256::digest([8, 32, 130]))
        )));
        let mut running = json!({"type":"assistant","content":[{"type":"tool","state":{
            "status":"running","content":[{"type":"file","uri":"data:image/png;base64,CCCC","mime":"image/png"}]
        }}]});
        retain(root.path(), "ses_1", &mut running);
        assert!(
            running["content"][0]["state"]["content"][0]["uri"]
                .as_str()
                .unwrap()
                .starts_with("data:")
        );
    }
}
