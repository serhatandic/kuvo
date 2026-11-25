//! What kind of thing an artifact file is, and its media type, matching `artifacts/kinds.ts`.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind { Memory, Skill, Image, Page, Document, Download, Code, File }

impl Kind {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "memory" => Some(Self::Memory), "skill" => Some(Self::Skill),
            "image" => Some(Self::Image), "page" => Some(Self::Page),
            "document" => Some(Self::Document), "download" => Some(Self::Download),
            "code" => Some(Self::Code), "file" => Some(Self::File),
            _ => None,
        }
    }
}

fn extension(path: &str) -> String {
    let name = basename(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[dot + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

pub(crate) fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

fn is_binary_ext(ext: &str) -> bool {
    matches!(
        ext,
        "zip" | "gz" | "tgz" | "tar" | "dmg" | "pkg" | "mp4" | "mov" | "mp3" | "wav" | "m4a"
            | "woff" | "woff2" | "ttf" | "otf" | "sqlite" | "db" | "bin" | "exe" | "dylib"
            | "so" | "o" | "a" | "class" | "jar" | "wasm"
    )
}

pub fn type_of(path: &str, binary: bool) -> &'static str {
    match extension(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "rtf" => "application/rtf",
        "pages" => "application/vnd.apple.pages",
        "numbers" => "application/vnd.apple.numbers",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        other if is_binary_ext(other) || binary => "application/octet-stream",
        _ => "text/plain",
    }
}

pub fn is_binary(bytes: &[u8]) -> bool { bytes[..bytes.len().min(8192)].contains(&0) }

pub fn is_prose(path: &str) -> bool {
    matches!(extension(path).as_str(), "md" | "markdown" | "txt") || basename(path) == "job.json"
}

pub fn is_text(mime: &str) -> bool {
    mime.starts_with("text/") || matches!(mime, "application/rtf" | "application/json" | "image/svg+xml")
}

pub(crate) fn strip_folder_prefix<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    path.strip_prefix(folder.trim_end_matches(std::path::MAIN_SEPARATOR))?
        .strip_prefix(std::path::MAIN_SEPARATOR)
}

pub fn kind_of(path: &str, bots: Option<&Path>, download: bool) -> Kind {
    if download { return Kind::Download; }
    let parts: Vec<&str> = path.split(std::path::MAIN_SEPARATOR).collect();
    if let Some(at) = parts.iter().rposition(|&p| p == "skills")
        && at > 0 && at + 2 < parts.len()
        && matches!(parts[at - 1], ".opencode" | ".claude" | "opencode")
    {
        return Kind::Skill;
    }
    if let Some(bots) = bots && let Some(inside) = strip_folder_prefix(path, &bots.to_string_lossy()) {
        let segs: Vec<&str> = inside.split(std::path::MAIN_SEPARATOR).collect();
        if (segs.len() == 2 && matches!(segs[1], "MEMORY.md" | "job.json"))
            || (segs.len() == 3 && segs[1] == "groups" && segs[2].ends_with(".md"))
        {
            return Kind::Memory;
        }
    }
    match extension(path).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg" | "bmp" | "tiff" => Kind::Image,
        "html" | "htm" => Kind::Page,
        "md" | "markdown" | "txt" | "pdf" | "docx" | "doc" | "rtf" | "pages" | "csv" | "tsv"
            | "xlsx" | "xls" | "numbers" | "pptx" | "key" | "odt" | "ods" => Kind::Document,
        ext if is_binary_ext(ext) => Kind::File,
        _ => Kind::Code,
    }
}
