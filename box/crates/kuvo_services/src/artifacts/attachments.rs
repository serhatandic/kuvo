//! Message blocks (`<attached-files>`, `<from-phone>`) and structured attachment path
//! visitation/relocation matching `shared/message-block.ts` and `shared/attachment-paths.ts`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};
use url::Url;

pub const FROM_PHONE_HEADING: &str = "The user sent this from Kuvo Code on their iPhone.";
pub const ATTACHED_HEADINGS: [&str; 3] = [
    "The attached files are on disk here (what's in them is in this message already):",
    "Open these attached files with your tools if you need them:",
    "These attached files stayed on the user's Mac (they couldn't be read, or copied to this machine):",
];
pub const UNSENT_SUFFIX: &str = " (on the user's Mac: it couldn't be copied to this machine)";

pub struct MessageBlock {
    pub start: &'static str,
    pub end: &'static str,
    pub headings: &'static [&'static str],
}

pub const FROM_PHONE_BLOCK: MessageBlock = MessageBlock {
    start: "<from-phone>",
    end: "</from-phone>",
    headings: &[FROM_PHONE_HEADING],
};

pub const ATTACHED_BLOCK: MessageBlock = MessageBlock {
    start: "<attached-files>",
    end: "</attached-files>",
    headings: &ATTACHED_HEADINGS,
};

struct FoundBlock<'a> {
    open: usize,
    lines: Vec<&'a str>,
}

impl MessageBlock {
    fn find<'a>(&self, text: &'a str) -> Option<FoundBlock<'a>> {
        let body = text.trim_end_matches(char::is_whitespace);
        let end_needle = format!("\n{}", self.end);
        if !body.ends_with(&end_needle) {
            return None;
        }
        let close = body.len() - self.end.len();
        let opening = format!("{}\n", self.start);
        let mut before = close;
        loop {
            if before < opening.len() {
                return None;
            }
            let open = text[..before].rfind(&opening)?;
            before = open;
            if open != 0 && !text[..open].ends_with('\n') {
                continue;
            }
            let inner = &text[open + opening.len()..close];
            let mut lines: Vec<&str> = inner.split('\n').collect();
            if !lines.is_empty() {
                lines.pop();
            }
            if let Some(&first) = lines.first()
                && self.headings.contains(&first)
            {
                return Some(FoundBlock { open, lines });
            }
        }
    }

    pub fn start_of(&self, text: &str) -> Option<usize> {
        self.find(text).map(|found| found.open)
    }

    pub fn map_lines(&self, text: &str, map: impl FnOnce(&[&str]) -> Vec<String>) -> String {
        let Some(found) = self.find(text) else {
            return text.to_owned();
        };
        let at = found.open + self.start.len() + 1;
        let orig_len = found.lines.join("\n").len();
        let mapped = map(&found.lines).join("\n");
        let mut out = String::with_capacity(text.len() - orig_len + mapped.len());
        out.push_str(&text[..at]);
        out.push_str(&mapped);
        out.push_str(&text[at + orig_len..]);
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentReference {
    pub path: String,
    pub stayed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentFormat {
    Transcript,
    Queues,
    Prompt,
    Part,
}

fn is_absolute_path(file: &str) -> bool {
    if file.starts_with('/') {
        return true;
    }
    Path::new(file).is_absolute()
}

fn map_note(text: &str, resolve: &mut impl FnMut(AttachmentReference) -> Option<String>) -> String {
    let phone = FROM_PHONE_BLOCK.start_of(text).unwrap_or(text.len());
    let head = ATTACHED_BLOCK.map_lines(&text[..phone], |lines| {
        let mut stayed = false;
        lines
            .iter()
            .map(|&line| {
                if ATTACHED_HEADINGS.contains(&line) {
                    stayed = line == ATTACHED_HEADINGS[2];
                    return line.to_owned();
                }
                let unsent = line.ends_with(UNSENT_SUFFIX);
                let file = if unsent {
                    &line[..line.len() - UNSENT_SUFFIX.len()]
                } else {
                    line
                };
                if !is_absolute_path(file) {
                    return line.to_owned();
                }
                let target = resolve(AttachmentReference {
                    path: file.to_owned(),
                    stayed: stayed || unsent,
                });
                if !stayed && !unsent && let Some(target) = target {
                    target
                } else {
                    line.to_owned()
                }
            })
            .collect()
    });
    format!("{head}{}", &text[phone..])
}

pub fn attachment_note(text: &str) -> Vec<AttachmentReference> {
    let mut files = Vec::new();
    map_note(text, &mut |file| {
        files.push(file);
        None
    });
    files
}

fn file_url_to_path(uri: &str) -> Option<PathBuf> {
    let parsed = Url::parse(uri).ok()?;
    if !parsed.scheme().eq_ignore_ascii_case("file") {
        return None;
    }
    parsed.to_file_path().ok()
}

fn map_file(
    value: &Value,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    let Some(obj) = value.as_object() else {
        return value.clone();
    };
    let in_source = obj
        .get("source")
        .and_then(Value::as_object)
        .is_some_and(|src| src.get("uri").and_then(Value::as_str).is_some());
    let address = if in_source {
        obj["source"].as_object().unwrap()
    } else {
        obj
    };
    let key = if address.get("uri").and_then(Value::as_str).is_some() {
        "uri"
    } else {
        "url"
    };
    let Some(given) = address.get(key).and_then(Value::as_str) else {
        return value.clone();
    };
    let is_uri = given.len() >= 5 && given[..5].eq_ignore_ascii_case("file:");
    let file = if is_uri {
        let Some(path) = file_url_to_path(given) else {
            if let Some(cb) = invalid.as_deref_mut() {
                cb();
            }
            return value.clone();
        };
        path.to_string_lossy().into_owned()
    } else {
        given.to_owned()
    };
    if !is_absolute_path(&file) {
        return value.clone();
    }
    if file.contains(['\0', '\r', '\n']) {
        if let Some(cb) = invalid.as_deref_mut() {
            cb();
        }
        return value.clone();
    }
    let Some(target) = resolve(AttachmentReference {
        path: file.clone(),
        stayed: false,
    }) else {
        return value.clone();
    };
    if target == file {
        return value.clone();
    }
    let replacement = if is_uri {
        let Ok(original) = Url::parse(given) else {
            return value.clone();
        };
        let Ok(mut updated) = Url::from_file_path(&target) else {
            return value.clone();
        };
        updated.set_query(original.query());
        updated.set_fragment(original.fragment());
        updated.to_string()
    } else {
        target
    };
    let mut mapped_address = address.clone();
    mapped_address.insert(key.to_owned(), Value::String(replacement));
    if in_source {
        let mut mapped_outer = obj.clone();
        mapped_outer.insert("source".to_owned(), Value::Object(mapped_address));
        Value::Object(mapped_outer)
    } else {
        Value::Object(mapped_address)
    }
}

fn kept_parts(given: Option<&Value>) -> Option<&Map<String, Value>> {
    given?.as_object()?.get("kuvo")?.as_object()?.get("parts")?.as_object()
}

fn map_note_field(
    value: &Value,
    notes: &mut HashMap<String, String>,
    words: Option<&str>,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
) -> Value {
    let Some(text) = value.as_str() else {
        return value.clone();
    };
    let before_phone = FROM_PHONE_BLOCK.start_of(text).unwrap_or(text.len());
    if let Some(words) = words
        && (text == words || text[..before_phone].trim() == words.trim())
    {
        return value.clone();
    }
    let mapped = map_note(text, &mut |reference| {
        let to = resolve(reference.clone());
        if !reference.stayed && let Some(target) = &to {
            notes.insert(reference.path, target.clone());
        }
        to
    });
    Value::String(mapped)
}

fn map_metadata(given: &Value, notes: &HashMap<String, String>) -> Value {
    let Some(obj) = given.as_object() else {
        return given.clone();
    };
    let Some(kuvo) = obj.get("kuvo").and_then(Value::as_object) else {
        return given.clone();
    };
    let Some(parts) = kuvo.get("parts").and_then(Value::as_object) else {
        return given.clone();
    };
    let Some(paths) = parts.get("paths").and_then(Value::as_array) else {
        return given.clone();
    };
    let mapped_paths: Vec<Value> = paths
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .and_then(|s| notes.get(s))
                .map_or_else(|| entry.clone(), |to| Value::String(to.clone()))
        })
        .collect();
    if mapped_paths == *paths {
        return given.clone();
    }
    let mut new_parts = parts.clone();
    new_parts.insert("paths".to_owned(), Value::Array(mapped_paths));
    let mut new_kuvo = kuvo.clone();
    new_kuvo.insert("parts".to_owned(), Value::Object(new_parts));
    let mut new_obj = obj.clone();
    new_obj.insert("kuvo".to_owned(), Value::Object(new_kuvo));
    Value::Object(new_obj)
}

fn visit_part(
    entry: &Value,
    user: bool,
    notes: &mut HashMap<String, String>,
    words: Option<&str>,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    let Some(obj) = entry.as_object() else {
        return entry.clone();
    };
    match obj.get("type").and_then(Value::as_str) {
        Some("file") => map_file(entry, resolve, invalid),
        Some("tool") => {
            let Some(state) = obj.get("state").and_then(Value::as_object) else {
                return entry.clone();
            };
            let Some(content) = state.get("content").and_then(Value::as_array) else {
                return entry.clone();
            };
            let mapped_content: Vec<Value> = content
                .iter()
                .map(|item| {
                    if item.get("type").and_then(Value::as_str) == Some("file") {
                        map_file(item, resolve, invalid)
                    } else {
                        item.clone()
                    }
                })
                .collect();
            let mut new_state = state.clone();
            new_state.insert("content".to_owned(), Value::Array(mapped_content));
            let mut new_obj = obj.clone();
            new_obj.insert("state".to_owned(), Value::Object(new_state));
            Value::Object(new_obj)
        }
        Some("text") if user => {
            let mut new_obj = obj.clone();
            if let Some(text) = obj.get("text") {
                new_obj.insert(
                    "text".to_owned(),
                    map_note_field(text, notes, words, resolve),
                );
            }
            Value::Object(new_obj)
        }
        _ => entry.clone(),
    }
}

fn visit_message(
    entry: &Value,
    prompt: bool,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    let Some(obj) = entry.as_object() else {
        return entry.clone();
    };
    if !prompt && matches!(obj.get("type").and_then(Value::as_str), Some("file" | "tool")) {
        let mut notes = HashMap::new();
        return visit_part(entry, false, &mut notes, None, resolve, invalid);
    }
    let info = if !prompt {
        obj.get("info").and_then(Value::as_object)
    } else {
        None
    };
    if info.is_some()
        && let Some(messages) = obj.get("messages").and_then(Value::as_array)
    {
        let mapped: Vec<Value> = messages
            .iter()
            .map(|one| visit_message(one, false, resolve, invalid))
            .collect();
        let mut new_obj = obj.clone();
        new_obj.insert("messages".to_owned(), Value::Array(mapped));
        return Value::Object(new_obj);
    }
    let role = info
        .and_then(|i| i.get("role"))
        .or_else(|| obj.get("role"))
        .or_else(|| obj.get("type"))
        .and_then(Value::as_str);
    let user = prompt
        || role == Some("user")
        || (role.is_none()
            && obj
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("msg_"))
            && obj.get("text").and_then(Value::as_str).is_some());
    let mut notes = HashMap::new();
    let words = kept_parts(obj.get("metadata"))
        .and_then(|parts| parts.get("words"))
        .or_else(|| {
            info.and_then(|i| kept_parts(i.get("metadata")))
                .and_then(|parts| parts.get("words"))
        })
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut new_obj = obj.clone();
    if user && let Some(text) = obj.get("text") {
        new_obj.insert(
            "text".to_owned(),
            map_note_field(text, &mut notes, words.as_deref(), resolve),
        );
    }
    if user && let Some(files) = obj.get("files").and_then(Value::as_array) {
        let mapped: Vec<Value> = files
            .iter()
            .map(|item| map_file(item, resolve, invalid))
            .collect();
        new_obj.insert("files".to_owned(), Value::Array(mapped));
    }
    if !prompt && let Some(parts) = obj.get("parts").and_then(Value::as_array) {
        let mapped: Vec<Value> = parts
            .iter()
            .map(|one| visit_part(one, user, &mut notes, words.as_deref(), resolve, invalid))
            .collect();
        new_obj.insert("parts".to_owned(), Value::Array(mapped));
    }
    if !prompt && let Some(content) = obj.get("content").and_then(Value::as_array) {
        let mapped: Vec<Value> = content
            .iter()
            .map(|one| visit_part(one, user, &mut notes, words.as_deref(), resolve, invalid))
            .collect();
        new_obj.insert("content".to_owned(), Value::Array(mapped));
    }
    if user && let Some(metadata) = obj.get("metadata") {
        new_obj.insert("metadata".to_owned(), map_metadata(metadata, &notes));
    }
    if user && let Some(info_obj) = info {
        let mut new_info = info_obj.clone();
        if let Some(metadata) = info_obj.get("metadata") {
            new_info.insert("metadata".to_owned(), map_metadata(metadata, &notes));
        }
        new_obj.insert("info".to_owned(), Value::Object(new_info));
    }
    Value::Object(new_obj)
}

pub fn visit_attachment_paths(
    value: &Value,
    format: AttachmentFormat,
    mut resolve: impl FnMut(AttachmentReference) -> Option<String>,
    mut invalid: Option<&mut dyn FnMut()>,
) -> Value {
    visit_attachment_paths_inner(value, format, &mut resolve, &mut invalid)
}

fn visit_attachment_paths_inner(
    value: &Value,
    format: AttachmentFormat,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    match format {
        AttachmentFormat::Part => {
            let mut notes = HashMap::new();
            visit_part(value, false, &mut notes, None, resolve, invalid)
        }
        AttachmentFormat::Prompt => visit_message(value, true, resolve, invalid),
        AttachmentFormat::Queues => {
            let Some(obj) = value.as_object() else {
                return value.clone();
            };
            let mut out = Map::new();
            for (id, queue) in obj {
                if let Some(q_obj) = queue.as_object() {
                    let mut new_q = q_obj.clone();
                    if let Some(waiting) = q_obj.get("waiting").and_then(Value::as_array) {
                        let mapped: Vec<Value> = waiting
                            .iter()
                            .map(|entry| {
                                if let Some(e_obj) = entry.as_object() {
                                    let mut new_e = e_obj.clone();
                                    if let Some(message) = e_obj.get("message") {
                                        new_e.insert(
                                            "message".to_owned(),
                                            visit_message(message, true, resolve, invalid),
                                        );
                                    }
                                    Value::Object(new_e)
                                } else {
                                    entry.clone()
                                }
                            })
                            .collect();
                        new_q.insert("waiting".to_owned(), Value::Array(mapped));
                    }
                    out.insert(id.clone(), Value::Object(new_q));
                } else {
                    out.insert(id.clone(), queue.clone());
                }
            }
            Value::Object(out)
        }
        AttachmentFormat::Transcript => {
            if let Some(arr) = value.as_array() {
                let mut out = Vec::with_capacity(arr.len());
                for entry in arr {
                    if entry.is_array() {
                        out.push(visit_attachment_paths_inner(
                            entry,
                            AttachmentFormat::Transcript,
                            resolve,
                            invalid,
                        ));
                    } else {
                        out.push(visit_message(entry, false, resolve, invalid));
                    }
                }
                Value::Array(out)
            } else {
                visit_message(value, false, resolve, invalid)
            }
        }
    }
}

pub fn map_attachment_paths(
    value: &Value,
    paths: &HashMap<String, String>,
    format: AttachmentFormat,
) -> Value {
    if paths.is_empty() {
        return value.clone();
    }
    visit_attachment_paths(
        value,
        format,
        |reference| {
            if reference.stayed {
                None
            } else {
                paths.get(&reference.path).cloned()
            }
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn engine_and_native_attachment_fields_relocate_while_quotes_stay_exact() {
        let from = "/source/attachments/ses_example/proof ü.bin";
        let to = "/destination/attachments/ses_example/proof ü.bin";
        let uri = Url::from_file_path(from).unwrap().to_string();
        let target = Url::from_file_path(to).unwrap().to_string();
        let mappings: HashMap<String, String> = [(from.into(), to.into())].into_iter().collect();
        let quote = format!("I wrote {from} yesterday and saved \"{uri}\".");
        let block = format!(
            "<attached-files>\n{}\n{from}\n</attached-files>",
            ATTACHED_HEADINGS[1]
        );
        let phone =
            "<from-phone>\nThe user sent this from Kuvo Code on their iPhone.\n</from-phone>";
        let text = format!("{quote}\n\n{block}\n \n{phone}\n \t");
        let relocated = text.replace(&block, &block.replace(from, to));

        let user = json!({
            "type": "user",
            "text": text,
            "metadata": { "kuvo": { "parts": { "words": quote, "paths": [from], "comments": [{ "text": quote }] } } },
            "files": [{ "source": { "type": "uri", "uri": from } }, { "uri": format!("{uri}?pick=1#page=2"), "name": quote }],
        });
        let tool = json!({
            "type": "tool",
            "state": {
                "input": { "files": [{ "uri": uri }] },
                "output": quote,
                "content": [{ "type": "text", "text": block }, { "type": "file", "uri": uri }],
            }
        });
        let assistant = json!({
            "type": "assistant",
            "text": text,
            "content": [{ "type": "text", "text": block }, tool],
        });
        let shell = json!({ "type": "shell", "command": format!("cat \"{from}\""), "output": { "output": from } });
        let engine = json!({ "info": { "role": "user" }, "parts": [{ "type": "text", "text": text }, { "type": "file", "url": uri }] });
        let bare = json!({ "type": "user", "text": from });
        let exports = json!({ "info": { "title": from }, "messages": [user, assistant, shell, engine, bare] });

        let result = map_attachment_paths(&exports, &mappings, AttachmentFormat::Transcript);
        assert_eq!(result["messages"][0]["text"], relocated);
        assert_eq!(result["messages"][0]["metadata"]["kuvo"]["parts"]["paths"], json!([to]));
        assert_eq!(result["messages"][0]["files"][0]["source"]["uri"], to);
        assert_eq!(result["messages"][0]["files"][1]["uri"], format!("{target}?pick=1#page=2"));
        assert_eq!(result["messages"][1]["content"][0]["text"], block);
        assert_eq!(result["messages"][1]["content"][1]["state"]["input"]["files"][0]["uri"], uri);
        assert_eq!(result["messages"][1]["content"][1]["state"]["content"][1]["uri"], target);
        assert_eq!(result["messages"][2], shell);
        assert_eq!(result["messages"][3]["parts"][0]["text"], relocated);
        assert_eq!(result["messages"][3]["parts"][1]["url"], target);
        assert_eq!(result["messages"][4], bare);
    }

    #[test]
    fn queued_prose_escaped_blocks_and_unsent_paths_stay_as_written() {
        let from = "/source/attachments/ses_example/proof ü.bin";
        let to = "/destination/attachments/ses_example/proof ü.bin";
        let uri = Url::from_file_path(from).unwrap().to_string();
        let target = Url::from_file_path(to).unwrap().to_string();
        let mappings: HashMap<String, String> = [(from.into(), to.into())].into_iter().collect();
        let quote = format!("I wrote {from} yesterday and saved \"{uri}\".");
        let block = format!(
            "<attached-files>\n{}\n{from}\n</attached-files>",
            ATTACHED_HEADINGS[1]
        );
        let phone =
            "<from-phone>\nThe user sent this from Kuvo Code on their iPhone.\n</from-phone>";
        let text = format!("{quote}\n\n{block}\n \n{phone}\n \t");
        let relocated = text.replace(&block, &block.replace(from, to));

        let stayed = block.replace(from, &format!("{from}{UNSENT_SUFFIX}"));
        let escaped = block.replace("<attached-files>", "<attached-files> ");
        let waiting = json!([
            { "shown": { "text": quote }, "message": { "text": text, "files": [{ "uri": uri }] } },
            { "shown": { "text": stayed }, "message": { "text": stayed } },
            { "shown": { "text": escaped }, "message": { "text": escaped } },
            { "shown": { "text": block }, "message": { "text": format!("{block}\n\n{phone}"), "metadata": { "kuvo": { "parts": { "words": block } } } } },
        ]);
        let queues = json!({ "ses_example": { "paused": true, "waiting": waiting } });
        let mapped = map_attachment_paths(&queues, &mappings, AttachmentFormat::Queues);
        assert_eq!(mapped["ses_example"]["waiting"][0]["message"]["text"], relocated);
        assert_eq!(mapped["ses_example"]["waiting"][0]["message"]["files"][0]["uri"], target);
        assert_eq!(mapped["ses_example"]["waiting"][1], waiting[1]);
        assert_eq!(mapped["ses_example"]["waiting"][2], waiting[2]);
        assert_eq!(mapped["ses_example"]["waiting"][3], waiting[3]);

        let mut seen = Vec::new();
        visit_attachment_paths(
            &queues,
            AttachmentFormat::Queues,
            |file| {
                seen.push(file);
                None
            },
            None,
        );
        assert_eq!(
            seen,
            vec![
                AttachmentReference { path: from.into(), stayed: false },
                AttachmentReference { path: from.into(), stayed: false },
                AttachmentReference { path: from.into(), stayed: true },
            ]
        );
        assert_eq!(
            map_attachment_paths(&json!({ "type": "text", "text": text }), &mappings, AttachmentFormat::Part)["text"],
            text
        );
    }
}
