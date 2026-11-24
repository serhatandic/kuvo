//! What waits to go to a coding agent, kept in the conversation's
//! `agent.json` (`inbox`), and the rules for what goes next.

use crate::session::{Prompt, PromptFile, SteerReceipt};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{io::Read, path::PathBuf};

/// How much of an attached text file goes into the message.
const TEXT_FILE_LIMIT: u64 = 200_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Into the turn under way where its agent takes one partway, else next.
    #[default]
    Steer,
    /// After the turn under way, in line.
    Queue,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Queue => "queue",
        }
    }
}

/// A message handed to the agent whose acceptance isn't known yet. Saved
/// before the handoff, so a restart settles it by the receipt rather than
/// sending it again.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SavedReceipt {
    pub provider: String,
    pub receipt: ReceiptFields,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReceiptFields {
    pub thread: String,
    pub turn: String,
    pub message: String,
}

impl From<SteerReceipt> for ReceiptFields {
    fn from(receipt: SteerReceipt) -> Self {
        Self {
            thread: receipt.thread,
            turn: receipt.turn,
            message: receipt.message,
        }
    }
}

impl From<ReceiptFields> for SteerReceipt {
    fn from(receipt: ReceiptFields) -> Self {
        Self {
            thread: receipt.thread,
            turn: receipt.turn,
            message: receipt.message,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Synthetic {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

/// One message in line, in the bridge's saved shape. Keys this resident
/// doesn't act on (`adopt`, `compact`, `resume`) stay as they were.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub files: Vec<Value>,
    #[serde(default)]
    pub delivery: Delivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_receipt: Option<SavedReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<Synthetic>,
    /// Kept for a resident that takes this one's place: it goes first there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Input {
    pub fn new(id: String, text: String, files: Vec<Value>, delivery: Delivery) -> Self {
        Self {
            id,
            text,
            files,
            delivery,
            steer_receipt: None,
            metadata: None,
            synthetic: None,
            held: false,
            other: Map::new(),
        }
    }

    pub fn is_compaction(&self) -> bool {
        self.other.get("compact").and_then(Value::as_bool) == Some(true)
    }

    /// As the inbox lists it to clients.
    pub fn listed(&self, session: &str) -> Value {
        let (kind, payload) = self.item();
        serde_json::json!({
            "id": self.id, "sessionID": session, "type": kind,
            "payload": payload, "delivery": self.delivery.as_str(),
        })
    }

    /// The item an enqueue event carries.
    pub fn item(&self) -> (&'static str, Value) {
        if self.is_compaction() {
            return ("compaction", Value::Object(Map::new()));
        }
        let mut payload = Map::new();
        payload.insert("text".into(), Value::String(self.text.clone()));
        if let Some(synthetic) = &self.synthetic {
            if let Some(description) = &synthetic.description {
                payload.insert("description".into(), Value::String(description.clone()));
            }
            if let Some(metadata) = &synthetic.metadata {
                payload.insert("metadata".into(), Value::Object(metadata.clone()));
            }
            return ("synthetic", Value::Object(payload));
        }
        payload.insert("files".into(), Value::Array(self.files.clone()));
        if let Some(metadata) = &self.metadata {
            payload.insert("metadata".into(), Value::Object(metadata.clone()));
        }
        ("user", Value::Object(payload))
    }

    /// The message as you sent it, as its row shows it.
    pub fn prompt_as_sent(&self) -> Prompt {
        Prompt {
            id: self.id.clone(),
            text: self.text.clone(),
            files: self.files.iter().filter_map(prompt_file).collect(),
            synthetic: self.synthetic.is_some(),
        }
    }

    /// What the agent is handed: a text file attached goes in the words, as
    /// for the message that starts a turn; other files go as files.
    pub fn prompt(&self) -> Prompt {
        let files = self
            .files
            .iter()
            .filter_map(prompt_file)
            .collect::<Vec<_>>();
        let (text, files) = with_text_files(&self.text, files);
        Prompt {
            id: self.id.clone(),
            text,
            files,
            synthetic: self.synthetic.is_some(),
        }
    }
}

/// What a client sees change in a conversation's line.
#[derive(Clone, Debug)]
pub enum InboxEvent {
    Enqueued {
        session: String,
        input: Box<Input>,
    },
    /// It left the line into the conversation: this row is in its transcript.
    Delivered {
        session: String,
        id: String,
        row: Value,
    },
    Cancelled {
        session: String,
        id: String,
    },
    DeliveryChanged {
        session: String,
        id: String,
        delivery: Delivery,
    },
    /// Nothing runs, and what was to go has gone.
    Idle {
        session: String,
    },
}

/// What the line does next once nothing runs. `ended` is how the turn that
/// just ended went: after one that succeeded the next in line goes, steered
/// ones first. `go` is a Send Now or a Resume waiting for the session to be
/// free. One being handed over waits for its answer.
pub fn next_in_line(
    line: &[Input],
    ended_succeeded: bool,
    go: bool,
    handing: impl Fn(&str) -> bool,
) -> Option<usize> {
    let free = |input: &Input| input.steer_receipt.is_none() && !handing(&input.id);
    let mut next = None;
    if ended_succeeded || go {
        next = line
            .iter()
            .position(|input| input.delivery == Delivery::Steer && free(input));
    }
    if ended_succeeded && next.is_none() {
        next = line.iter().position(free);
    }
    next
}

fn prompt_file(file: &Value) -> Option<PromptFile> {
    let uri = file.get("uri")?.as_str()?.to_owned();
    let name = file.get("name").and_then(Value::as_str).map(str::to_owned);
    let mime = mime(&uri);
    if let Some(rest) = uri.strip_prefix("data:") {
        let (head, payload) = rest.split_once(',').unwrap_or((rest, ""));
        let data = if head.ends_with(";base64") {
            payload.to_owned()
        } else {
            STANDARD.encode(percent_decode_str(payload).collect::<Vec<u8>>())
        };
        return Some(PromptFile {
            uri,
            mime,
            name,
            path: None,
            data: Some(data),
        });
    }
    let path = uri
        .starts_with("file:")
        .then(|| url::Url::parse(&uri).ok()?.to_file_path().ok())
        .flatten();
    Some(PromptFile {
        uri,
        mime,
        name,
        path,
        data: None,
    })
}

fn mime(uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("data:") {
        let kind = rest.split([';', ',']).next().unwrap_or_default();
        return if kind.is_empty() { "text/plain" } else { kind }.to_owned();
    }
    let extension = uri
        .split('?')
        .next()
        .unwrap_or_default()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
    .to_owned()
}

/// Text files in the words, as the engine puts them; the rest stay files.
pub fn with_text_files(text: &str, files: Vec<PromptFile>) -> (String, Vec<PromptFile>) {
    let mut others = Vec::new();
    let mut parts = Vec::new();
    for file in files {
        let contents = (!file.mime.starts_with("image/") && file.mime != "application/pdf")
            .then(|| text_contents(&file))
            .flatten();
        let Some(contents) = contents else {
            others.push(file);
            continue;
        };
        let shown = file
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .or_else(|| file.name.clone())
            .unwrap_or_else(|| "pasted".into());
        parts.push(format!("<file path=\"{shown}\">\n{contents}\n</file>"));
    }
    let text = if parts.is_empty() {
        text.to_owned()
    } else {
        format!("{text}\n\n{}", parts.join("\n\n"))
    };
    (text, others)
}

fn text_contents(file: &PromptFile) -> Option<String> {
    let bytes = match (&file.data, &file.path) {
        (Some(data), _) => STANDARD.decode(data).ok()?,
        (None, Some(path)) => read_small(path)?,
        (None, None) => return None,
    };
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn read_small(path: &PathBuf) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > TEXT_FILE_LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(TEXT_FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, delivery: Delivery) -> Input {
        Input::new(id.into(), id.into(), Vec::new(), delivery)
    }

    #[test]
    fn after_a_turn_that_succeeded_steered_ones_go_first_then_the_rest() {
        let line = [input("a", Delivery::Queue), input("b", Delivery::Steer)];
        assert_eq!(next_in_line(&line, true, false, |_| false), Some(1));
        assert_eq!(next_in_line(&line[..1], true, false, |_| false), Some(0));
        // After a Stop or an error nothing goes unless asked.
        assert_eq!(next_in_line(&line, false, false, |_| false), None);
        assert_eq!(next_in_line(&line, false, true, |_| false), Some(1));
        assert_eq!(next_in_line(&line[..1], false, true, |_| false), None);
        // One being handed to the agent, or whose handoff is uncertain, waits.
        assert_eq!(next_in_line(&line, true, false, |id| id == "b"), Some(0));
        let mut doubtful = line.clone();
        doubtful[1].steer_receipt = Some(SavedReceipt {
            provider: "codex-cli".into(),
            receipt: ReceiptFields {
                thread: "t".into(),
                turn: "u".into(),
                message: "b".into(),
            },
            order: None,
        });
        assert_eq!(next_in_line(&doubtful, false, true, |_| false), None);
    }

    #[test]
    fn the_saved_shape_is_the_bridges_and_keeps_what_it_doesnt_know() {
        let saved = serde_json::json!({
            "id": "msg_1", "text": "hi", "files": [{"uri": "file:///tmp/a.png"}],
            "delivery": "queue", "metadata": {"kuvo": {"note": {"from": "Goal"}}},
            "steerReceipt": {"provider": "codex-cli",
                "receipt": {"thread": "t", "turn": "u", "message": "msg_1"}, "order": ["o", 3]},
            "adopt": {"providerID": "codex-cli", "id": "gpt"}, "held": true,
        });
        let input: Input = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(input.delivery, Delivery::Queue);
        assert!(input.held);
        assert_eq!(serde_json::to_value(&input).unwrap(), saved);
        let (kind, payload) = input.item();
        assert_eq!(kind, "user");
        assert_eq!(payload["metadata"]["kuvo"]["note"]["from"], "Goal");
    }

    #[test]
    fn an_attached_text_file_goes_in_the_words_and_a_picture_stays_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes.txt");
        std::fs::write(&notes, "remember this").unwrap();
        let mut input = input("msg_files", Delivery::Steer);
        input.text = "look".into();
        input.files = vec![
            serde_json::json!({"uri": url::Url::from_file_path(&notes).unwrap().to_string()}),
            serde_json::json!({"uri": "data:image/png;base64,AAAA", "name": "shot.png"}),
            serde_json::json!({"uri": "data:text/plain,hello%20there", "name": "pasted.txt"}),
        ];
        let prompt = input.prompt();
        assert_eq!(
            prompt.text,
            format!(
                "look\n\n<file path=\"{}\">\nremember this\n</file>\n\n<file path=\"pasted.txt\">\nhello there\n</file>",
                notes.display()
            )
        );
        assert_eq!(prompt.files.len(), 1);
        assert_eq!(prompt.files[0].mime, "image/png");
        assert_eq!(prompt.files[0].data.as_deref(), Some("AAAA"));
    }
}
