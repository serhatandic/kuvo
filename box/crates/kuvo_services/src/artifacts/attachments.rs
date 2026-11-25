//! Message blocks (`<attached-files>`, `<from-phone>`) and structured attachment path
//! visitation/relocation matching `shared/message-block.ts` and `shared/attachment-paths.ts`.

use std::{collections::HashMap, path::Path};

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

impl MessageBlock {
    fn find<'a>(&self, text: &'a str) -> Option<(usize, Vec<&'a str>)> {
        let body = text.trim_end_matches(char::is_whitespace);
        if !body.ends_with(&format!("\n{}", self.end)) { return None; }
        let close = body.len() - self.end.len();
        let opening = format!("{}\n", self.start);
        let mut before = close;
        while before >= opening.len() {
            let open = text[..before].rfind(&opening)?;
            before = open;
            if open != 0 && !text[..open].ends_with('\n') { continue; }
            let mut lines: Vec<&str> = text[open + opening.len()..close].split('\n').collect();
            lines.pop();
            if lines.first().is_some_and(|first| self.headings.contains(first)) {
                return Some((open, lines));
            }
        }
        None
    }

    pub fn start_of(&self, text: &str) -> Option<usize> { self.find(text).map(|(open, _)| open) }

    pub fn map_lines(&self, text: &str, map: impl FnOnce(&[&str]) -> Vec<String>) -> String {
        let Some((open, lines)) = self.find(text) else { return text.to_owned() };
        let at = open + self.start.len() + 1;
        let orig_len = lines.join("\n").len();
        format!("{}{}{}", &text[..at], map(&lines).join("\n"), &text[at + orig_len..])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentReference { pub path: String, pub stayed: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentFormat { Transcript, Queues, Prompt, Part }

fn is_abs(file: &str) -> bool { file.starts_with('/') || Path::new(file).is_absolute() }

fn with_field(obj: &Map<String, Value>, key: &str, val: Value) -> Value {
    let mut out = obj.clone();
    out.insert(key.to_owned(), val);
    Value::Object(out)
}

fn map_note(text: &str, resolve: &mut impl FnMut(AttachmentReference) -> Option<String>) -> String {
    let phone = FROM_PHONE_BLOCK.start_of(text).unwrap_or(text.len());
    let head = ATTACHED_BLOCK.map_lines(&text[..phone], |lines| {
        let mut stayed = false;
        lines.iter().map(|&line| {
            if ATTACHED_HEADINGS.contains(&line) {
                stayed = line == ATTACHED_HEADINGS[2];
                return line.to_owned();
            }
            let (file, unsent) = match line.strip_suffix(UNSENT_SUFFIX) {
                Some(f) => (f, true),
                None => (line, false),
            };
            if !is_abs(file) { return line.to_owned(); }
            let target = resolve(AttachmentReference { path: file.to_owned(), stayed: stayed || unsent });
            if !stayed && !unsent && let Some(target) = target { target } else { line.to_owned() }
        }).collect()
    });
    format!("{head}{}", &text[phone..])
}

pub fn attachment_note(text: &str) -> Vec<AttachmentReference> {
    let mut files = Vec::new();
    map_note(text, &mut |file| { files.push(file); None });
    files
}

fn map_file(
    value: &Value,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    let Some(obj) = value.as_object() else { return value.clone() };
    let source = obj.get("source").and_then(Value::as_object)
        .filter(|s| s.get("uri").and_then(Value::as_str).is_some());
    let address = source.unwrap_or(obj);
    let key = if address.get("uri").and_then(Value::as_str).is_some() { "uri" } else { "url" };
    let Some(given) = address.get(key).and_then(Value::as_str) else { return value.clone() };
    let is_uri = given.len() >= 5 && given[..5].eq_ignore_ascii_case("file:");
    let file = if is_uri {
        let parsed = Url::parse(given).ok()
            .filter(|u| u.scheme().eq_ignore_ascii_case("file"))
            .and_then(|u| u.to_file_path().ok());
        let Some(path) = parsed else {
            if let Some(cb) = invalid.as_deref_mut() { cb(); }
            return value.clone();
        };
        path.to_string_lossy().into_owned()
    } else {
        given.to_owned()
    };
    if !is_abs(&file) { return value.clone(); }
    if file.contains(['\0', '\r', '\n']) {
        if let Some(cb) = invalid.as_deref_mut() { cb(); }
        return value.clone();
    }
    let Some(target) = resolve(AttachmentReference { path: file.clone(), stayed: false }) else {
        return value.clone();
    };
    if target == file { return value.clone(); }
    let replacement = if is_uri {
        let (Ok(orig), Ok(mut updated)) = (Url::parse(given), Url::from_file_path(&target)) else {
            return value.clone();
        };
        updated.set_query(orig.query());
        updated.set_fragment(orig.fragment());
        updated.to_string()
    } else {
        target
    };
    let updated_addr = with_field(address, key, Value::String(replacement));
    if source.is_some() { with_field(obj, "source", updated_addr) } else { updated_addr }
}

fn map_note_field(
    value: &Value,
    notes: &mut HashMap<String, String>,
    words: Option<&str>,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
) -> Value {
    let Some(text) = value.as_str() else { return value.clone() };
    let before_phone = FROM_PHONE_BLOCK.start_of(text).unwrap_or(text.len());
    if words.is_some_and(|w| text == w || text[..before_phone].trim() == w.trim()) {
        return value.clone();
    }
    Value::String(map_note(text, &mut |r| {
        let to = resolve(r.clone());
        if !r.stayed && let Some(target) = &to { notes.insert(r.path, target.clone()); }
        to
    }))
}

fn map_metadata(given: &Value, notes: &HashMap<String, String>) -> Value {
    let Some(obj) = given.as_object() else { return given.clone() };
    let Some(kuvo) = obj.get("kuvo").and_then(Value::as_object) else { return given.clone() };
    let Some(parts) = kuvo.get("parts").and_then(Value::as_object) else { return given.clone() };
    let Some(paths) = parts.get("paths").and_then(Value::as_array) else { return given.clone() };
    let mapped: Vec<Value> = paths.iter()
        .map(|e| e.as_str().and_then(|s| notes.get(s)).map_or_else(|| e.clone(), |to| Value::String(to.clone())))
        .collect();
    if mapped == *paths { return given.clone(); }
    with_field(obj, "kuvo", with_field(kuvo, "parts", with_field(parts, "paths", Value::Array(mapped))))
}

fn visit_part(
    entry: &Value,
    user: bool,
    notes: &mut HashMap<String, String>,
    words: Option<&str>,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    let Some(obj) = entry.as_object() else { return entry.clone() };
    match obj.get("type").and_then(Value::as_str) {
        Some("file") => map_file(entry, resolve, invalid),
        Some("tool") => {
            let Some(state) = obj.get("state").and_then(Value::as_object) else { return entry.clone() };
            let Some(content) = state.get("content").and_then(Value::as_array) else { return entry.clone() };
            let mapped = content.iter().map(|i| {
                if i.get("type").and_then(Value::as_str) == Some("file") { map_file(i, resolve, invalid) } else { i.clone() }
            }).collect();
            with_field(obj, "state", with_field(state, "content", Value::Array(mapped)))
        }
        Some("text") if user => obj.get("text")
            .map_or_else(|| entry.clone(), |t| with_field(obj, "text", map_note_field(t, notes, words, resolve))),
        _ => entry.clone(),
    }
}

fn visit_message(
    entry: &Value,
    prompt: bool,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    let Some(obj) = entry.as_object() else { return entry.clone() };
    if !prompt && matches!(obj.get("type").and_then(Value::as_str), Some("file" | "tool")) {
        return visit_part(entry, false, &mut HashMap::new(), None, resolve, invalid);
    }
    let info = (!prompt).then(|| obj.get("info").and_then(Value::as_object)).flatten();
    if info.is_some() && let Some(messages) = obj.get("messages").and_then(Value::as_array) {
        let mapped = messages.iter().map(|m| visit_message(m, false, resolve, invalid)).collect();
        return with_field(obj, "messages", Value::Array(mapped));
    }
    let role = info.and_then(|i| i.get("role")).or_else(|| obj.get("role")).or_else(|| obj.get("type")).and_then(Value::as_str);
    let user = prompt || role == Some("user")
        || (role.is_none()
            && obj.get("id").and_then(Value::as_str).is_some_and(|id| id.starts_with("msg_"))
            && obj.get("text").and_then(Value::as_str).is_some());
    let words = entry.pointer("/metadata/kuvo/parts/words")
        .or_else(|| info.and_then(|_| entry.pointer("/info/metadata/kuvo/parts/words")))
        .and_then(Value::as_str).map(str::to_owned);
    let (mut notes, mut out) = (HashMap::new(), obj.clone());
    if user && let Some(text) = obj.get("text") {
        out.insert("text".into(), map_note_field(text, &mut notes, words.as_deref(), resolve));
    }
    if user && let Some(files) = obj.get("files").and_then(Value::as_array) {
        out.insert("files".into(), Value::Array(files.iter().map(|f| map_file(f, resolve, invalid)).collect()));
    }
    if !prompt {
        for field in ["parts", "content"] {
            if let Some(arr) = obj.get(field).and_then(Value::as_array) {
                let mapped = arr.iter().map(|p| visit_part(p, user, &mut notes, words.as_deref(), resolve, invalid)).collect();
                out.insert(field.into(), Value::Array(mapped));
            }
        }
    }
    if user && let Some(meta) = obj.get("metadata") {
        out.insert("metadata".into(), map_metadata(meta, &notes));
    }
    if user && let Some(info_obj) = info {
        let mapped_info = info_obj.get("metadata")
            .map_or_else(|| Value::Object(info_obj.clone()), |m| with_field(info_obj, "metadata", map_metadata(m, &notes)));
        out.insert("info".into(), mapped_info);
    }
    Value::Object(out)
}

pub fn visit_attachment_paths(
    value: &Value,
    format: AttachmentFormat,
    mut resolve: impl FnMut(AttachmentReference) -> Option<String>,
    mut invalid: Option<&mut dyn FnMut()>,
) -> Value {
    visit_inner(value, format, &mut resolve, &mut invalid)
}

fn visit_inner(
    value: &Value,
    format: AttachmentFormat,
    resolve: &mut dyn FnMut(AttachmentReference) -> Option<String>,
    invalid: &mut Option<&mut dyn FnMut()>,
) -> Value {
    match format {
        AttachmentFormat::Part => visit_part(value, false, &mut HashMap::new(), None, resolve, invalid),
        AttachmentFormat::Prompt => visit_message(value, true, resolve, invalid),
        AttachmentFormat::Queues => {
            let Some(obj) = value.as_object() else { return value.clone() };
            Value::Object(obj.iter().map(|(id, q)| {
                let mapped = q.as_object().and_then(|qo| {
                    let waiting = qo.get("waiting")?.as_array()?;
                    let items = waiting.iter().map(|e| {
                        e.as_object().and_then(|eo| eo.get("message").map(|m| with_field(eo, "message", visit_message(m, true, resolve, invalid))))
                            .unwrap_or_else(|| e.clone())
                    }).collect();
                    Some(with_field(qo, "waiting", Value::Array(items)))
                }).unwrap_or_else(|| q.clone());
                (id.clone(), mapped)
            }).collect())
        }
        AttachmentFormat::Transcript => match value.as_array() {
            Some(arr) => Value::Array(arr.iter().map(|e| {
                if e.is_array() { visit_inner(e, AttachmentFormat::Transcript, resolve, invalid) }
                else { visit_message(e, false, resolve, invalid) }
            }).collect()),
            None => visit_message(value, false, resolve, invalid),
        },
    }
}

pub fn map_attachment_paths(value: &Value, paths: &HashMap<String, String>, format: AttachmentFormat) -> Value {
    if paths.is_empty() { return value.clone(); }
    visit_attachment_paths(value, format, |r| (!r.stayed).then(|| paths.get(&r.path).cloned()).flatten(), None)
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
        let block = format!("<attached-files>\n{}\n{from}\n</attached-files>", ATTACHED_HEADINGS[1]);
        let phone = "<from-phone>\nThe user sent this from Kuvo Code on their iPhone.\n</from-phone>";
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
        let block = format!("<attached-files>\n{}\n{from}\n</attached-files>", ATTACHED_HEADINGS[1]);
        let phone = "<from-phone>\nThe user sent this from Kuvo Code on their iPhone.\n</from-phone>";
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
        visit_attachment_paths(&queues, AttachmentFormat::Queues, |file| { seen.push(file); None }, None);
        assert_eq!(seen, vec![
            AttachmentReference { path: from.into(), stayed: false },
            AttachmentReference { path: from.into(), stayed: false },
            AttachmentReference { path: from.into(), stayed: true },
        ]);
        assert_eq!(
            map_attachment_paths(&json!({ "type": "text", "text": text }), &mappings, AttachmentFormat::Part)["text"],
            text
        );
    }
}
