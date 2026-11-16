//! The pictures a conversation shows, as both apps find them (KuvoClient's `ConversationPictures`,
//! PROTOCOL.md, "show_file"): those a message of yours carried, or sent as paths; and those a
//! finished step brought to show you: an image it made (`generate_image`), a file it showed you
//! (`kuvo-browser_show_file`) that's a picture, a page's screenshot it took to show you
//! (`kuvo-browser_browser_screenshot` with `show`). A step's own screenshots, taken for itself,
//! stay in its step. Their bytes come with the message (`data:`), or from the conversation's
//! computer (`kuvo/file`), never by a path on this one.

use base64::Engine as _;
use serde_json::Value;

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::messages::UserMessage;
use crate::paths;
use crate::transport::Request;

/// One picture in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// The same for as long as the conversation has it: "<message>#<n>" for a message's nth file,
    /// "<message>@<path>" for one it sent as a path, "<step>-<n>" for a step's nth output,
    /// "<step>-shown" for a file shown.
    pub id: String,
    pub source: Source,
    /// Its name, where the message or step gives one.
    pub name: Option<String>,
    /// The words the agent showed it with.
    pub caption: Option<String>,
}

/// Where a picture's bytes are. (Each picture is read by its own id: the same path shown again
/// later, changed since, is read again.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// In the message itself: a `data:` address, or its base64 alone (shared, not copied: a photo's
    /// is megabytes).
    Inline(std::sync::Arc<str>),
    /// A file on the conversation's computer: its `file://` address.
    File(String),
    /// A file on this computer, sent from here: a message of yours on its way (or whose copy
    /// came without it), drawn from what you sent until its box's copy has it.
    Local(std::path::PathBuf),
}

/// The pictures among the files a message of yours sent from here (`files`, in the order sent),
/// as its box's copy will name them (`<message>#<n>`), read from this computer meanwhile.
pub fn of_sending(message: &str, files: &[std::path::PathBuf]) -> Vec<Picture> {
    files
        .iter()
        .enumerate()
        .filter(|(_, path)| is_picture_path(&path.to_string_lossy()))
        .map(|(n, path)| Picture {
            id: format!("{message}#{n}"),
            source: Source::Local(path.clone()),
            name: path.file_name().map(|name| name.to_string_lossy().into_owned()),
            caption: None,
        })
        .collect()
}

/// The extensions read as pictures.
const PICTURES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif", "ico", "svg"];

/// Whether a file is a picture, by its name.
pub fn is_picture_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, extension)| !stem.is_empty() && PICTURES.contains(&extension.to_ascii_lowercase().as_str()))
}

/// A file's `file://` address, each part of its path escaped.
pub fn file_uri(path: &str) -> String {
    let encoded: Vec<String> = path.split('/').map(paths::segment).collect();
    format!("file://{}", encoded.join("/"))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// The pictures a message of yours carried, in order, then those it sent as paths (its parts name
/// them), each once.
pub fn of_user(user: &UserMessage) -> Vec<Picture> {
    let mut pictures = Vec::new();
    let mut own = Vec::new();
    for (n, file) in user.files.iter().enumerate() {
        let uri = file.pointer("/source/uri").and_then(Value::as_str).or_else(|| file.get("url").and_then(Value::as_str)).map(str::to_owned);
        if let Some(path) = uri.as_deref().and_then(|uri| uri.strip_prefix("file://")) {
            own.push(percent_decoded(path));
        }
        let name = text(file, "name").or_else(|| text(file, "filename")).or_else(|| uri.as_deref().filter(|uri| uri.starts_with("file://")).and_then(|uri| uri.rsplit('/').next()).map(percent_decoded));
        let mime = text(file, "mime").unwrap_or_default().to_ascii_lowercase();
        let is_picture = mime.starts_with("image/")
            || uri.as_deref().is_some_and(|uri| uri.get(..11).is_some_and(|start| start.eq_ignore_ascii_case("data:image/")))
            || ((mime.is_empty() || mime == "application/octet-stream") && name.as_deref().is_some_and(is_picture_path));
        if !is_picture {
            continue;
        }
        let source = match (text(file, "data"), uri) {
            (Some(data), _) => Source::Inline(if mime.is_empty() { data.into() } else { format!("data:{mime};base64,{data}").into() }),
            (None, Some(uri)) if uri.starts_with("data:") => Source::Inline(uri.into()),
            (None, Some(uri)) if uri.starts_with("file://") => Source::File(uri),
            _ => continue,
        };
        pictures.push(Picture { id: format!("{}#{n}", user.id), source, name, caption: None });
    }
    let mut seen: Vec<String> = Vec::new();
    for path in user.parts().paths {
        if !is_picture_path(&path) || own.contains(&path) || seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        pictures.push(Picture { id: format!("{}@{path}", user.id), name: path.rsplit('/').next().map(str::to_owned), source: Source::File(file_uri(&path)), caption: None });
    }
    pictures
}

/// Whether a step brings a picture to show you in the conversation's flow.
pub fn is_made(name: &str, state: &Value) -> bool {
    name == "generate_image" || name == "kuvo-browser_show_file" || (name == "kuvo-browser_browser_screenshot" && state.pointer("/input/show") == Some(&Value::Bool(true)))
}

/// The pictures a finished step `step` (tool `name`, its `state`) brought to show you: the file it
/// showed, if it's a picture; the images it made, or the screenshot it took to show you. None for
/// any other step, nor one at work or failed.
pub fn of_step(step: &str, name: &str, state: &Value) -> Vec<Picture> {
    if !is_made(name, state) || state.get("status").and_then(Value::as_str) != Some("completed") {
        return Vec::new();
    }
    let caption = state.pointer("/input/caption").and_then(Value::as_str).filter(|caption| !caption.is_empty()).map(str::to_owned);
    if name == "kuvo-browser_show_file" {
        let Some(path) = state.pointer("/input/path").and_then(Value::as_str).filter(|path| is_picture_path(path)) else { return Vec::new() };
        return vec![Picture { id: format!("{step}-shown"), source: Source::File(file_uri(path)), name: path.rsplit('/').next().map(str::to_owned), caption }];
    }
    outputs(step, state, caption)
}

/// A picture written in a reply's words (`![alt](url)`), as `id`: one in it (`data:`), or a file on
/// the conversation's computer (`file://`, or a full path). None for any other (a web address,
/// a path from nowhere), which isn't read.
pub fn of_address(id: String, url: &str, alt: &str) -> Option<Picture> {
    let source = if url.starts_with("data:image/") {
        Source::Inline(url.into())
    } else if url.starts_with("file:///") {
        Source::File(url.to_owned())
    } else if url.starts_with('/') {
        Source::File(file_uri(url))
    } else {
        return None;
    };
    let name = if alt.is_empty() { url.rsplit('/').next().filter(|_| !url.starts_with("data:")).map(str::to_owned) } else { Some(alt.to_owned()) };
    Some(Picture { id, source, name, caption: None })
}

/// The pictures a step brought back for itself (a page's screenshot it took, a picture it opened),
/// shown inside the step once it's opened: any finished step's but one that shows its pictures in
/// the flow (`of_step`).
pub fn of_step_own(step: &str, name: &str, state: &Value) -> Vec<Picture> {
    if is_made(name, state) || !matches!(state.get("status").and_then(Value::as_str), Some("completed" | "failed" | "error")) {
        return Vec::new();
    }
    outputs(step, state, None)
}

/// Whether a step brought back pictures for itself (`of_step_own`), without reading them.
pub fn has_own(name: &str, state: &Value) -> bool {
    !is_made(name, state)
        && matches!(state.get("status").and_then(Value::as_str), Some("completed" | "failed" | "error"))
        && state.get("content").or_else(|| state.get("attachments")).and_then(Value::as_array).is_some_and(|parts| {
            parts.iter().any(|part| part.get("mime").and_then(Value::as_str).is_some_and(|mime| mime.to_ascii_lowercase().starts_with("image/")))
        })
}

fn outputs(step: &str, state: &Value, caption: Option<String>) -> Vec<Picture> {
    let Some(parts) = state.get("content").or_else(|| state.get("attachments")).and_then(Value::as_array) else { return Vec::new() };
    parts
        .iter()
        .enumerate()
        .filter_map(|(n, part)| {
            let mime = text(part, "mime").unwrap_or_default().to_ascii_lowercase();
            let uri = text(part, "uri").or_else(|| text(part, "url"))?;
            if !mime.starts_with("image/") {
                return None;
            }
            let source = if uri.starts_with("data:") { Source::Inline(uri.into()) } else if uri.starts_with("file://") { Source::File(uri) } else { return None };
            Some(Picture { id: format!("{step}-{n}"), source, name: text(part, "name").or_else(|| text(part, "filename")), caption: caption.clone() })
        })
        .collect()
}

/// A `data:` address's bytes (or base64 alone).
pub fn inline_bytes(data: &str) -> Option<Vec<u8>> {
    let encoded = match data.strip_prefix("data:") {
        Some(rest) => {
            let (head, body) = rest.split_once(',')?;
            if !head.ends_with(";base64") {
                return Some(percent_decoded(body).into_bytes());
            }
            body
        }
        None => data,
    };
    base64::engine::general_purpose::STANDARD.decode(encoded.trim()).ok()
}

fn percent_decoded(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = path.get(at + 1..at + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The most of a picture a conversation reads.
pub const PICTURE_MOST: usize = 25 * 1024 * 1024;

impl BoxClient {
    /// A file on this box's computer (`kuvo/file`), by its `file://` address: its bytes, at most
    /// `max`, and its type.
    pub fn file_bytes(&self, uri: &str, max: usize) -> Result<(Vec<u8>, Option<String>)> {
        let response = self.send(&Request::get(format!("kuvo/file?uri={}&max={max}", paths::query_value(uri))))?;
        if !response.is_success() {
            return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
        }
        Ok((response.body, response.content_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(files: Value, text: &str) -> UserMessage {
        serde_json::from_value(json!({ "id": "msg_1", "time": { "created": 1.0 }, "text": text, "files": files })).unwrap()
    }

    #[test]
    fn a_messages_pictures_are_its_picture_files_then_the_pictures_it_sent_as_paths() {
        let message = user(
            json!([
                { "mime": "image/png", "name": "shot.png", "source": { "type": "uri", "uri": "file:///Users/sam/shot%201.png" } },
                { "mime": "text/plain", "name": "notes.txt", "source": { "type": "uri", "uri": "file:///Users/sam/notes.txt" } },
                { "mime": "image/jpeg", "data": "AAAA" },
            ]),
            "Look",
        );
        let pictures = of_user(&message);
        assert_eq!(pictures.iter().map(|picture| picture.id.as_str()).collect::<Vec<_>>(), ["msg_1#0", "msg_1#2"]);
        assert_eq!(pictures[0].source, Source::File("file:///Users/sam/shot%201.png".into()));
        assert_eq!(pictures[1].source, Source::Inline("data:image/jpeg;base64,AAAA".into()));
    }

    #[test]
    fn a_step_shows_a_picture_only_once_its_done_and_only_one_it_shows_you() {
        let shown = json!({ "status": "completed", "input": { "path": "/home/sam/chart one.png", "caption": "The chart" }, "content": [] });
        let pictures = of_step("prt_1", "kuvo-browser_show_file", &shown);
        assert_eq!(pictures, [Picture { id: "prt_1-shown".into(), source: Source::File("file:///home/sam/chart%20one.png".into()), name: Some("chart one.png".into()), caption: Some("The chart".into()) }]);
        assert!(of_step("prt_1", "kuvo-browser_show_file", &json!({ "status": "completed", "input": { "path": "/home/sam/report.pdf" } })).is_empty());
        assert!(of_step("prt_1", "kuvo-browser_show_file", &json!({ "status": "running", "input": { "path": "/a.png" } })).is_empty());
        let own = json!({ "status": "completed", "input": {}, "content": [{ "type": "file", "uri": "data:image/png;base64,AAAA", "mime": "image/png" }] });
        assert!(of_step("prt_2", "kuvo-browser_browser_screenshot", &own).is_empty(), "taken for itself");
        let for_you = json!({ "status": "completed", "input": { "show": true }, "content": [{ "type": "text", "text": "ok" }, { "type": "file", "uri": "data:image/png;base64,AAAA", "mime": "image/png" }] });
        assert_eq!(of_step("prt_2", "kuvo-browser_browser_screenshot", &for_you)[0].id, "prt_2-1");
        // One taken for itself shows inside the step instead.
        assert_eq!(of_step_own("prt_2", "kuvo-browser_browser_screenshot", &own)[0].id, "prt_2-0");
        assert!(of_step_own("prt_2", "kuvo-browser_browser_screenshot", &for_you).is_empty());
    }

    #[test]
    fn inline_pictures_decode() {
        assert_eq!(inline_bytes("data:image/png;base64,AAEC"), Some(vec![0, 1, 2]));
        assert_eq!(inline_bytes("AAEC"), Some(vec![0, 1, 2]));
    }
}
