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

/// The narrow legacy array reader for Insights. Serde skips unknown map fields
/// without allocating their strings, so a tool's large output or a user's
/// inline picture never becomes a conversation-sized JSON value.
pub fn legacy_insight(file: &std::path::Path) -> std::io::Result<Vec<Value>> {
    use serde::{Deserialize, Serialize};

    #[derive(Default, Deserialize, Serialize)]
    #[serde(default)]
    struct Message {
        #[serde(rename = "type")]
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        time: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tokens: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<Vec<Part>>,
    }
    #[derive(Default, Deserialize, Serialize)]
    #[serde(default)]
    struct Metadata {
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    }
    #[derive(Default, Deserialize, Serialize)]
    #[serde(default)]
    struct Part {
        #[serde(rename = "type")]
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<State>,
    }
    #[derive(Default, Deserialize, Serialize)]
    #[serde(default)]
    struct State {
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(
            skip_serializing_if = "Option::is_none",
            deserialize_with = "input_map"
        )]
        input: Option<Input>,
    }
    #[derive(Default, Deserialize, Serialize)]
    #[serde(default)]
    struct Input {
        #[serde(skip_serializing_if = "Option::is_none")]
        skill: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_path: Option<Value>,
        #[serde(rename = "filePath")]
        #[serde(skip_serializing_if = "Option::is_none")]
        camel_path: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<Value>,
    }

    // A streaming tool's input is a string, and other tools may use arrays or
    // scalars. None of these can name a skill through object fields.
    fn input_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Input>, D::Error> {
        use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer};
        struct OnlyMap;
        impl<'de> Visitor<'de> for OnlyMap {
            type Value = Option<Input>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a tool input")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Input::deserialize(MapAccessDeserializer::new(map)).map(Some)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(None)
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(None)
            }
        }
        d.deserialize_any(OnlyMap)
    }

    let file = match std::fs::File::open(file) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let rows: Vec<Message> = serde_json::from_reader(std::io::BufReader::new(file))?;
    rows.into_iter()
        .map(|row| serde_json::to_value(row).map(|value| project(&value, View::Insight)))
        .filter_map(|row| row.transpose())
        .collect::<serde_json::Result<Vec<_>>>()
        .map_err(Into::into)
}
