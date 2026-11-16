//! A message's parts (PROTOCOL.md, "A message's parts", `messages.send`): what was written, and what
//! went with it (the keyword it was sent with, the widget it answers, comments on the changes, files
//! its note names, agents it names with "&", that it came from a phone, or that another agent wrote
//! it). A client sends them as fields ([`Sending`], `kuvo/sessions/<id>/send`) and the box writes the
//! text the agent reads; the box keeps them with the message (`metadata.kuvo.parts`), and reads them
//! from the text of one from before (`kuvo/messages/parts`). A client draws from these, and parses
//! no text. Held to `protocol/vectors/message-parts.json`, as KuvoClient's `MessageParts` is.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths;

/// What a box answers `kuvo/sessions/<id>/send` and `kuvo/messages/parts` with in `features`.
pub const FEATURE: &str = "messages.send";

/// The route a message (or a command) goes to `session` by.
pub fn send_route(session: &str) -> String {
    format!("kuvo/sessions/{}/send", paths::segment(session))
}

/// The route that reads the parts of messages from before.
pub const READ_ROUTE: &str = "kuvo/messages/parts";

/// How many texts one read takes.
pub const READ_MOST: usize = 500;

/// A comment on one line of the changes: its file, the line's number on its side (a removed line's
/// on the old side), the line as it read, and what was said of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub file: String,
    pub line: i64,
    pub side: Side,
    pub snippet: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    New,
    Old,
}

/// An agent: as it was named, and its address ("unknown" for a message that can't be answered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    pub session: String,
}

/// A message's parts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Parts {
    /// What was written ("Can you review?"; a command's, as typed).
    pub words: String,
    /// The keyword it was sent with, as typed ("/loop every hour").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    /// The title of the widget it answers ("" for one without a title).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    /// Files its note names that it doesn't carry: sent as paths, or left behind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// The agents it names with "&".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<Agent>,
    /// Sent from a phone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub phone: bool,
    /// Another agent wrote it: who (and how to answer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<Agent>,
    /// It ran a command (`keyword`, "/review"), with `words` as its arguments.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub command: bool,
}

impl Parts {
    /// Just words: a message as it's shown while its parts aren't known.
    pub fn words(words: impl Into<String>) -> Parts {
        Parts { words: words.into(), ..Parts::default() }
    }

    /// The parts kept with a message (`metadata.kuvo.parts`), or none for one from before. Read
    /// leniently, as KuvoClient reads them: what doesn't read is left out, not the whole.
    pub fn kept(metadata: Option<&Value>) -> Option<Parts> {
        let parts = metadata?.get("kuvo")?.get("parts")?.as_object()?;
        let words = parts.get("words")?.as_str()?.to_owned();
        let string = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
        let list = |name: &str| parts.get(name).and_then(Value::as_array).cloned().unwrap_or_default();
        let agent = |value: &Value| Some(Agent { name: string(value.get("name"))?, session: string(value.get("session"))? });
        let comments = list("comments")
            .iter()
            .filter_map(|each| {
                Some(Comment {
                    file: string(each.get("file"))?,
                    line: each.get("line")?.as_i64()?,
                    side: if each.get("side").and_then(Value::as_str) == Some("old") { Side::Old } else { Side::New },
                    snippet: string(each.get("snippet"))?,
                    text: string(each.get("text"))?,
                })
            })
            .collect();
        Some(Parts {
            words,
            keyword: string(parts.get("keyword")),
            widget: string(parts.get("widget")),
            comments,
            paths: list("paths").iter().filter_map(|path| path.as_str().map(str::to_owned)).collect(),
            agents: list("agents").iter().filter_map(agent).collect(),
            phone: parts.get("phone") == Some(&Value::Bool(true)),
            mail: parts.get("mail").and_then(agent),
            command: parts.get("command") == Some(&Value::Bool(true)),
        })
    }
}

/// A file a message carries, as the engine takes it: `file://` on the box's own disk, or `data:`
/// inline (which the box keeps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl File {
    /// A file on the box's own disk (this Mac's own box, beside the app): its `file://` address,
    /// named as it's called. The box decides what the model reads of it, and notes the rest as paths.
    pub fn at(path: &std::path::Path) -> File {
        let encoded: Vec<String> = path.to_string_lossy().split('/').map(crate::paths::segment).collect();
        File { uri: format!("file://{}", encoded.join("/")), name: path.file_name().map(|name| name.to_string_lossy().into_owned()) }
    }
}

/// The most a file sent to another computer with a message weighs, and all of a message's files
/// (as 0.7's `Attachments.maxMachineBytes` and `maxMessageBytes`): the box there keeps them.
pub const CARRIED_FILE_MOST: u64 = 50 * 1024 * 1024;
pub const CARRIED_MESSAGE_MOST: u64 = 100 * 1024 * 1024;

/// The files attached to a message, as its box takes them. `here`: the box is on this computer,
/// which opens them where they are (`File::at`), but for those the client keeps only a while
/// (`passing`: a pasted picture), which go as another computer's do. Another computer can't open
/// them, so each goes with its bytes (`data:`, as a phone sends them; a big picture made smaller as
/// the phone makes it, `smaller_picture`), and its box keeps it there and notes where. One that can't be read, or is too large to send, is said, by name, in plain
/// words: the message doesn't go without it.
pub fn carried(paths: &[std::path::PathBuf], here: bool, passing: &dyn Fn(&std::path::Path) -> bool) -> Result<Vec<File>, String> {
    use base64::Engine as _;
    let mut total = 0u64;
    let mut files = Vec::new();
    for path in paths {
        if here && !passing(path) {
            files.push(File::at(path));
            continue;
        }
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
        let size = std::fs::metadata(path).map_err(|_| format!("“{name}” couldn’t be read."))?.len();
        if size > CARRIED_FILE_MOST {
            return Err(format!("“{name}” is over 50 MB, more than can go to another computer with a message."));
        }
        total += size;
        if total > CARRIED_MESSAGE_MOST {
            return Err("These files together are over 100 MB, more than can go to another computer with one message.".into());
        }
        let bytes = std::fs::read(path).map_err(|_| format!("“{name}” couldn’t be read."))?;
        let (bytes, mime, name) = match smaller_picture(&bytes, mime_of(path)) {
            Some(smaller) => (smaller, "image/jpeg", jpeg_name(&name)),
            None => (bytes, mime_of(path), name),
        };
        files.push(File { uri: format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)), name: Some(name) });
    }
    Ok(files)
}

/// A picture over this many bytes goes smaller (the phone's `Attachment.keptAsIs`).
pub const PICTURE_KEPT_MOST: usize = 4 * 1024 * 1024;
/// A picture made smaller is at most this many pixels on its longest side, a JPEG of this quality
/// (the phone's `maxSide` and its 0.85).
pub const PICTURE_SIDE_MOST: u32 = 2048;
const PICTURE_QUALITY: u8 = 85;

/// A picture as it goes to another computer, as the phone sends one: a PNG or JPEG of 4 MB or less
/// as it is (none here); anything bigger, or of another kind, a JPEG at most 2048 px on its longest
/// side. A Retina screenshot (5 MB as a PNG) goes as a few hundred KB. None for what isn't a
/// picture this can read (it goes as it is).
pub fn smaller_picture(bytes: &[u8], mime: &str) -> Option<Vec<u8>> {
    let format = match mime {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        "image/gif" => image::ImageFormat::Gif,
        "image/webp" => image::ImageFormat::WebP,
        "image/tiff" => image::ImageFormat::Tiff,
        "image/bmp" => image::ImageFormat::Bmp,
        _ => return None,
    };
    // (A GIF may move: it goes as it is, as long as it isn't too big to send.)
    if bytes.len() <= PICTURE_KEPT_MOST && matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif | image::ImageFormat::WebP) {
        return None;
    }
    let picture = image::load_from_memory_with_format(bytes, format).ok()?;
    let picture = if picture.width().max(picture.height()) > PICTURE_SIDE_MOST { picture.resize(PICTURE_SIDE_MOST, PICTURE_SIDE_MOST, image::imageops::FilterType::Triangle) } else { picture };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, PICTURE_QUALITY).encode_image(&picture.to_rgb8()).ok()?;
    Some(out)
}

/// `name` as a JPEG's: its extension `.jpg`.
fn jpeg_name(name: &str) -> String {
    let stem = std::path::Path::new(name).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_owned());
    format!("{stem}.jpg")
}

/// A file's type, by its extension, as a message carries it (words as `text/plain`, which the
/// engine reads as words; anything not known as bytes, which its box keeps for the agent to open).
pub fn mime_of(path: &std::path::Path) -> &'static str {
    let extension = path.extension().map(|extension| extension.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" | "markdown" | "csv" | "tsv" | "log" | "json" | "yaml" | "yml" | "toml" | "xml" | "html" | "htm" | "css" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "rs" | "py" | "rb" | "go" | "swift" | "kt" | "java" | "c" | "h" | "cpp" | "hpp" | "m" | "sh" | "zsh" | "sql" | "ini" | "conf" | "env" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Where the files a message sent are, for its note: with their contents (`included`), as paths for
/// the agent to open, and those left behind (they couldn't be read or sent).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Notes {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub included: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unsent: Vec<String>,
}

impl Notes {
    pub fn is_empty(&self) -> bool {
        self.included.is_empty() && self.paths.is_empty() && self.unsent.is_empty()
    }
}

/// How a message goes to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// To the agent now, read at its next step.
    Steer,
    /// After the run under way ends.
    Queue,
}

/// What a client sends: what was written and what goes with it, as fields; the box writes the text
/// the agent reads. `command`: run that command, `words` its arguments. `named`: the sessions picked
/// from the "&" list as it was written.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Sending {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub words: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<File>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widget: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub named: Vec<String>,
    #[serde(skip_serializing_if = "Notes::is_empty")]
    pub notes: Notes,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl Sending {
    pub fn words(words: impl Into<String>) -> Sending {
        Sending { words: words.into(), ..Sending::default() }
    }
}

#[cfg(test)]
mod carried_tests {
    use super::*;

    #[test]
    fn files_go_where_they_are_to_this_computers_box_and_with_their_bytes_to_another() {
        let folder = std::env::temp_dir().join(format!("kuvo-carried-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let picture = folder.join("a b.png");
        std::fs::write(&picture, [1u8, 2, 3]).unwrap();
        let here = carried(std::slice::from_ref(&picture), true, &|_| false).unwrap();
        assert!(here[0].uri.starts_with("file://") && here[0].uri.ends_with("a%20b.png"));
        let there = carried(std::slice::from_ref(&picture), false, &|_| false).unwrap();
        assert_eq!(there, [File { uri: "data:image/png;base64,AQID".into(), name: Some("a b.png".into()) }]);
        // One kept here only a while (a pasted picture) goes with its bytes to this computer's too.
        assert_eq!(carried(std::slice::from_ref(&picture), true, &|_| true).unwrap(), there);
        let gone = carried(&[folder.join("gone.png")], false, &|_| false).unwrap_err();
        assert_eq!(gone, "“gone.png” couldn’t be read.");
        std::fs::remove_dir_all(&folder).ok();
    }
}
