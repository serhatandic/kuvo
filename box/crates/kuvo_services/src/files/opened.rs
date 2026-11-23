//! A file opened for a viewer: its type by its extension, and why one can't be opened.

use std::fs::{File, Metadata};
use std::io;
use std::path::Path;

/// Each extension a viewer knows, and the type it's sent as; any other is
/// `application/octet-stream`.
const TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("bmp", "image/bmp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("ico", "image/vnd.microsoft.icon"),
    ("svg", "image/svg+xml"),
    ("pdf", "application/pdf"),
    ("mp4", "video/mp4"),
    ("mov", "video/quicktime"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("csv", "text/csv"),
    ("json", "application/json"),
    ("html", "text/html"),
    ("css", "text/css"),
    ("js", "text/javascript"),
    ("xml", "application/xml"),
    ("ts", "text/plain"),
    ("tsx", "text/plain"),
    ("jsx", "text/plain"),
    ("rs", "text/plain"),
    ("py", "text/plain"),
    ("sh", "text/plain"),
    ("zsh", "text/plain"),
    ("bash", "text/plain"),
    ("log", "text/plain"),
    ("yaml", "text/yaml"),
    ("yml", "text/yaml"),
    ("toml", "text/plain"),
];

const UNKNOWN: &str = "application/octet-stream";

/// The type a file is sent as, by its extension, whatever its case.
pub(crate) fn mime_of(file: &Path) -> &'static str {
    let Some(extension) = file.extension().and_then(|extension| extension.to_str()) else {
        return UNKNOWN;
    };
    let extension = extension.to_lowercase();
    TYPES
        .iter()
        .find(|(known, _)| *known == extension)
        .map_or(UNKNOWN, |(_, mime)| mime)
}

/// What a viewer makes of a file, by its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FileKind {
    Picture,
    Pdf,
    Text,
    Other,
}

impl FileKind {
    pub(crate) fn of(mime: &str) -> Self {
        if mime.starts_with("image/") {
            Self::Picture
        } else if mime == "application/pdf" {
            Self::Pdf
        } else if mime.starts_with("text/")
            || mime == "application/json"
            || mime == "application/xml"
        {
            Self::Text
        } else {
            Self::Other
        }
    }
}

/// Why a file can't be opened for a viewer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unopened {
    /// Not there, or not a file (a folder, a device).
    Missing,
    /// There but not to be read here (permissions, a loop of links).
    Refused,
}

impl From<io::Error> for Unopened {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Self::Missing,
            _ => Self::Refused,
        }
    }
}

/// A regular file, opened for reading, with what its open handle says of it.
pub(crate) struct Opened {
    pub(crate) file: File,
    pub(crate) metadata: Metadata,
}

/// Opens a regular file without opening a folder or a special file (a pipe would block): checked
/// by its path first, then again by its handle.
pub(crate) fn open_regular(path: &Path) -> Result<Opened, Unopened> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(Unopened::Missing);
    }
    let file = platform::open_without_waiting(path)?;
    let metadata = file.metadata().map_err(Unopened::from)?;
    if !metadata.is_file() {
        return Err(Unopened::Missing);
    }
    Ok(Opened { file, metadata })
}

#[cfg(unix)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    pub(super) fn open_without_waiting(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
    }

    /// Whether anyone may run it.
    pub(crate) fn executable(metadata: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
}

#[cfg(not(unix))]
mod platform {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    pub(super) fn open_without_waiting(path: &Path) -> io::Result<File> {
        File::open(path)
    }

    /// Windows files have no execute bits to keep.
    pub(crate) fn executable(_metadata: &std::fs::Metadata) -> bool {
        false
    }
}

pub(crate) use platform::executable;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_kinds_by_extension() {
        for (name, kind, mime) in [
            ("a #ş.PNG", FileKind::Picture, "image/png"),
            ("a.pdf", FileKind::Pdf, "application/pdf"),
            ("a.json", FileKind::Text, "application/json"),
            ("a.constructor", FileKind::Other, UNKNOWN),
            (".bashrc", FileKind::Other, UNKNOWN),
            ("a.", FileKind::Other, UNKNOWN),
        ] {
            assert_eq!(mime_of(Path::new(name)), mime, "{name}");
            assert_eq!(FileKind::of(mime), kind, "{name}");
        }
    }
}
