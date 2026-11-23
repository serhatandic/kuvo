//! A file on this computer that an agent's reply or tool names (PROTOCOL.md, `kuvo/file`): its
//! bytes for a client to show (`kuvo/file`), its text where it's text (`kuvo/file/text`), or its
//! name, size and kind (`kuvo/file/info`).

mod address;
mod opened;

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use http::StatusCode;
use http::header::{CONTENT_LENGTH, HeaderValue};
use kuvo_protocol::JsonStatus;
use kuvo_protocol::routes::{get_kuvo_file, get_kuvo_file_info, get_kuvo_file_text};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::AsyncReadExt;

use address::{Query, named_file};
use opened::{FileKind, Opened, Unopened, mime_of, open_regular};

/// The features these routes answer.
pub const FEATURES: &[&str] = &["file", "file.info", "file.text"];

/// The biggest file `kuvo/file` sends.
pub const FILE_LIMIT: u64 = 25 * 1024 * 1024;
/// The biggest `kuvo/file/text` reads into one answer.
pub const TEXT_LIMIT: u64 = 4 * 1024 * 1024;

const CHANGED: &str = "The file changed while it was being read.";

pub fn register(registry: &mut Registry) {
    registry.register::<get_kuvo_file_info::Spec, _, _>(Access::Any, |call| {
        let query = Query::of(&call.request.path);
        async move { blocking(move || info(&query)).await }
    });
    registry.register::<get_kuvo_file_text::Spec, _, _>(Access::Any, |call| {
        let query = Query::of(&call.request.path);
        async move { blocking(move || text(&query)).await }
    });
    registry.register::<get_kuvo_file::Spec, _, _>(Access::Any, |call| {
        let query = Query::of(&call.request.path);
        async move { blocking(move || bytes(&query)).await.map(Download::reply) }
    });
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Refusal> + Send + 'static,
) -> Result<T, Refusal> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
}

/// An answer of the route's declared shape, built from this module's own types.
fn typed<R, const STATUS: u16>(value: impl Serialize) -> Result<RouteReply<R>, Refusal>
where
    R: JsonStatus<STATUS>,
    <R as JsonStatus<STATUS>>::Body: DeserializeOwned,
{
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map(RouteReply::json::<STATUS>)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn file_named(query: &Query, accepts_path: bool) -> Result<PathBuf, Refusal> {
    named_file(query, accepts_path)
        .map_err(|why| Refusal::new(StatusCode::BAD_REQUEST, why.message()))
}

fn name_of(file: &Path) -> String {
    file.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A file that isn't there (or isn't a file), or can't be opened here, as the bytes and text
/// routes say it.
fn unopened(why: Unopened) -> Refusal {
    match why {
        Unopened::Missing => Refusal::json(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "message": "That file isn’t there anymore.", "reason": "fileNotFound" }),
        ),
        Unopened::Refused => refused(),
    }
}

fn refused() -> Refusal {
    Refusal::new(StatusCode::FORBIDDEN, "That file can’t be opened here.")
}

fn too_big(message: &'static str) -> Refusal {
    Refusal::new(StatusCode::PAYLOAD_TOO_LARGE, message)
}

#[derive(Serialize)]
#[serde(untagged)]
enum Info {
    There {
        exists: bool,
        name: String,
        size: u64,
        kind: FileKind,
        mime: &'static str,
    },
    Missing {
        exists: bool,
        name: String,
        size: (),
        kind: (),
        mime: (),
    },
}

fn info(query: &Query) -> Result<RouteReply<get_kuvo_file_info::Spec>, Refusal> {
    let file = file_named(query, false)?;
    let name = name_of(&file);
    let answer = match open_regular(&file) {
        Ok(Opened { metadata, .. }) => {
            let mime = mime_of(&file);
            Info::There {
                exists: true,
                name,
                size: metadata.len(),
                kind: FileKind::of(mime),
                mime,
            }
        }
        Err(Unopened::Missing) => Info::Missing {
            exists: false,
            name,
            size: (),
            kind: (),
            mime: (),
        },
        Err(Unopened::Refused) => return Err(refused()),
    };
    typed::<_, 200>(answer)
}

/// The most a client asked for (`max`, a whole number of bytes as JavaScript's `Number` reads it),
/// or `most` where it asked for none or for something that isn't a finite number.
fn asked_most(query: &Query, most: u64) -> f64 {
    let Some(given) = query.get("max") else {
        return most as f64;
    };
    let given = js_number(given).floor();
    if given.is_finite() {
        given
    } else {
        most as f64
    }
}

/// A string as JavaScript's `Number` reads it (NaN for what it doesn't).
fn js_number(text: &str) -> f64 {
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if text.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return u128::from_str_radix(digits, radix).map_or(f64::NAN, |n| n as f64);
        }
    }
    let lower = text.to_ascii_lowercase();
    // Rust reads `inf`, `nan` and `infinity` in any case; JavaScript only `Infinity`.
    if lower.trim_start_matches(['+', '-']).starts_with(['i', 'n'])
        && text.trim_start_matches(['+', '-']) != "Infinity"
    {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

/// Whether a file is text a viewer shows: UTF-8 with no NUL byte in it; a byte-order mark at its
/// start isn't part of the text.
#[derive(Serialize)]
#[serde(untagged)]
enum Text {
    Not {
        text: bool,
    },
    Utf8 {
        text: bool,
        encoding: &'static str,
        content: String,
    },
}

fn text_of(bytes: Vec<u8>) -> Text {
    if bytes.contains(&0) {
        return Text::Not { text: false };
    }
    let bytes = bytes
        .strip_prefix(b"\xEF\xBB\xBF")
        .map_or(bytes.clone(), <[u8]>::to_vec);
    match String::from_utf8(bytes) {
        Ok(content) => Text::Utf8 {
            text: true,
            encoding: "utf-8",
            content,
        },
        Err(_) => Text::Not { text: false },
    }
}

fn text(query: &Query) -> Result<RouteReply<get_kuvo_file_text::Spec>, Refusal> {
    let file = file_named(query, true)?;
    let Opened { file, metadata } = open_regular(&file).map_err(unopened)?;
    let max = asked_most(query, TEXT_LIMIT);
    let size = metadata.len();
    if size > TEXT_LIMIT || size as f64 > max {
        return Err(too_big("That file is too big to show here."));
    }
    // One byte past the end shows a file that grew or reports no size (as /proc's do).
    let end = max.min(TEXT_LIMIT as f64) as u64;
    let mut read = Vec::new();
    file.take(end + 1)
        .read_to_end(&mut read)
        .map_err(|_| refused())?;
    if read.len() as u64 > end {
        return Err(too_big("That file is too big to show here."));
    }
    typed::<_, 200>(text_of(read))
}

/// A file's bytes about to be sent: as long as its handle said, with its type.
struct Download {
    file: std::fs::File,
    size: u64,
    content_type: &'static str,
}

fn bytes(query: &Query) -> Result<Download, Refusal> {
    let file = file_named(query, false)?;
    let Opened {
        file: handle,
        metadata,
    } = open_regular(&file).map_err(unopened)?;
    let size = metadata.len();
    if size > FILE_LIMIT {
        return Err(too_big("That file is too big to open here."));
    }
    if size as f64 > asked_most(query, FILE_LIMIT) {
        return Err(too_big("That file is too big to show here."));
    }
    // The source's mode on the same answer as its bytes, where asked (a copy to another computer).
    let content_type = match (
        query.get("withMode") == Some("1"),
        opened::executable(&metadata),
    ) {
        (true, true) => "application/vnd.kuvo.file; executable=1",
        (true, false) => "application/vnd.kuvo.file; executable=0",
        (false, _) => mime_of(&file),
    };
    Ok(Download {
        file: handle,
        size,
        content_type,
    })
}

impl Download {
    /// Streams the length announced, even if the file grows, and breaks the answer if it shrinks.
    fn reply(self) -> RouteReply<get_kuvo_file::Spec> {
        let file = tokio::fs::File::from_std(self.file);
        let pieces =
            futures_util::stream::unfold((file, self.size), |(mut file, left)| async move {
                if left == 0 {
                    return None;
                }
                let mut piece = vec![0; left.min(32 * 1024) as usize];
                match file.read(&mut piece).await {
                    Ok(0) => Some((Err(io::Error::other(CHANGED)), (file, 0))),
                    Ok(read) => {
                        piece.truncate(read);
                        Some((Ok(Bytes::from(piece)), (file, left - read as u64)))
                    }
                    Err(error) => Some((Err(error), (file, 0))),
                }
            });
        let mut reply = RouteReply::stream::<200>(self.content_type, pieces);
        reply
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from(self.size));
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_reads_as_javascript_numbers_do() {
        for (given, read) in [
            ("12.9", 12.0),
            ("", 0.0),
            (" 7 ", 7.0),
            ("1e3", 1000.0),
            ("0x10", 16.0),
            ("-3", -3.0),
        ] {
            assert_eq!(js_number(given).floor(), read, "{given}");
        }
        for given in ["abc", "inf", "nan", "-0x10", "1_000"] {
            assert!(js_number(given).is_nan(), "{given}");
        }
        assert!(js_number("Infinity").is_infinite());
    }

    #[test]
    fn text_is_utf8_without_nul_and_without_its_mark() {
        assert!(
            matches!(text_of(b"\xEF\xBB\xBFhi".to_vec()), Text::Utf8 { content, .. } if content == "hi")
        );
        assert!(matches!(text_of(b"a\0b".to_vec()), Text::Not { .. }));
        assert!(matches!(text_of(vec![0xff, 0xfe, b'a']), Text::Not { .. }));
    }
}
