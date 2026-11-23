//! Which file a viewer asks for: a `file://` address (`uri`), or for its text, a full path in
//! base64url (`path64`), as `kuvo/file`, `kuvo/file/info` and `kuvo/file/text` name one.

use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The parameters of a request's query, as a browser's `URLSearchParams` reads them: the first
/// value of each name wins.
pub(crate) struct Query(Vec<(String, String)>);

impl Query {
    pub(crate) fn of(path: &str) -> Self {
        let query = path.split_once('?').map_or("", |(_, query)| query);
        Self(
            form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect(),
        )
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Why a request names no file this box can look for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unnamed {
    /// Neither a `file://` address nor (for text) a path.
    NotAnAddress,
    /// A `path64` that isn't a full path in canonical base64url.
    NotAFullPath,
}

impl Unnamed {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::NotAnAddress => "That isn’t a file:// address.",
            Self::NotAFullPath => "Name the file by its full path.",
        }
    }
}

/// The file a request names: its `uri` first; for text with no `uri` at all, its `path64`.
pub(crate) fn named_file(query: &Query, accepts_path: bool) -> Result<PathBuf, Unnamed> {
    let uri = query.get("uri").filter(|uri| !uri.is_empty());
    let given = (accepts_path && query.get("uri").is_none())
        .then(|| query.get("path64"))
        .flatten();
    if let Some(file) = uri.and_then(file_url_path) {
        return Ok(file);
    }
    if uri.is_none()
        && let Some(file) = given.and_then(decoded_path)
    {
        return Ok(file);
    }
    Err(if given.is_some() {
        Unnamed::NotAFullPath
    } else {
        Unnamed::NotAnAddress
    })
}

/// A `path64`: canonical unpadded base64url of a UTF-8 absolute path with no NUL in it, resolved.
fn decoded_path(given: &str) -> Option<PathBuf> {
    if given.is_empty()
        || !given
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(given).ok()?;
    if URL_SAFE_NO_PAD.encode(&bytes) != given {
        return None;
    }
    let path = String::from_utf8(bytes).ok()?;
    if path.contains('\0') || !Path::new(&path).is_absolute() {
        return None;
    }
    Some(resolved(Path::new(&path)))
}

/// An absolute path with `.` and `..` worked out and no trailing separator, as Node's
/// `path.resolve` gives it.
fn resolved(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The path a `file://` address names, as Node's `fileURLToPath` reads it on this platform; none
/// for another scheme, a host where this platform takes none, an escaped separator, or an escape
/// that isn't UTF-8.
fn file_url_path(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    platform::file_url_path(url.host_str().unwrap_or(""), url.path())
}

/// `%XX` escapes decoded, refusing a malformed one or bytes that aren't UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn escapes_separator(path: &str, separators: &[&str]) -> bool {
    let lower = path.to_ascii_lowercase();
    separators.iter().any(|escaped| lower.contains(escaped))
}

#[cfg(not(windows))]
mod platform {
    use std::path::PathBuf;

    pub(super) fn file_url_path(host: &str, path: &str) -> Option<PathBuf> {
        if !host.is_empty() || super::escapes_separator(path, &["%2f"]) {
            return None;
        }
        super::percent_decoded(path).map(PathBuf::from)
    }
}

#[cfg(windows)]
mod platform {
    use std::path::PathBuf;

    pub(super) fn file_url_path(host: &str, path: &str) -> Option<PathBuf> {
        if super::escapes_separator(path, &["%2f", "%5c"]) {
            return None;
        }
        let path = super::percent_decoded(path)?.replace('/', "\\");
        if !host.is_empty() {
            return Some(PathBuf::from(format!("\\\\{host}{path}")));
        }
        let bytes = path.as_bytes();
        let drive = bytes.len() >= 3 && bytes[1].is_ascii_alphabetic() && bytes[2] == b':';
        drive.then(|| PathBuf::from(&path[1..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(query: &str, accepts_path: bool) -> Result<PathBuf, Unnamed> {
        named_file(&Query::of(&format!("kuvo/file?{query}")), accepts_path)
    }

    #[cfg(not(windows))]
    #[test]
    fn file_addresses_read_as_node_reads_them() {
        assert_eq!(
            named(
                "uri=file%3A%2F%2F%2Ftmp%2Fa%252520%2520%2523%25C5%259F.txt",
                false
            ),
            Ok("/tmp/a%20 #ş.txt".into())
        );
        assert_eq!(
            named("uri=file%3A%2F%2Flocalhost%2Ftmp%2Fx", false),
            Ok("/tmp/x".into())
        );
        assert_eq!(
            named("uri=file%3A%2F%2F%2Ftmp%2Fa%2Fb%2F", false),
            Ok("/tmp/a/b/".into())
        );
        for refused in [
            "uri=https%3A%2F%2Fexample.com",
            "uri=file%3A%2F%2Fhost%2Fx",
            "uri=file%3A%2F%2F%2Fa%252Fb",
            "uri=file%3A%2F%2F%2F%25zz",
            "uri=file%3A%2F%2F%2F%25ff",
            "",
        ] {
            assert_eq!(
                named(refused, false),
                Err(Unnamed::NotAnAddress),
                "{refused}"
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn text_takes_a_full_path_in_base64url_only_without_an_address() {
        let encode = |path: &str| URL_SAFE_NO_PAD.encode(path);
        assert_eq!(
            named(&format!("path64={}", encode("/tmp/../tmp/./a//b/")), true),
            Ok("/tmp/a/b".into())
        );
        assert_eq!(
            named(&format!("path64={}", encode("/tmp/x")), false),
            Err(Unnamed::NotAnAddress)
        );
        assert_eq!(
            named(&format!("uri=&path64={}", encode("/tmp/x")), true),
            Err(Unnamed::NotAnAddress)
        );
        for refused in [
            encode("relative"),
            encode("/a\0b"),
            format!("{}=", encode("/tmp/x")),
            "cmVsYXRpdmU".into(),
            "_w".into(),
        ] {
            assert_eq!(
                named(&format!("path64={refused}"), true),
                Err(Unnamed::NotAFullPath),
                "{refused}"
            );
        }
    }
}
