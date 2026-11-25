//! Skill name normalization, path/ID validation, YAML frontmatter parsing, and trash IDs.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillAbout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillTrashEntry {
    pub folder: String,
    pub deleted: i64,
}

pub fn is_skill_folder(name: &str) -> bool {
    let len = name.encode_utf16().count();
    (1..=128).contains(&len) && !name.starts_with('.') && !name.chars().any(|c| matches!(c, '/' | '\\' | '\0'))
}

pub fn is_item_id(id: &str) -> bool {
    let b = id.as_bytes();
    let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    (1..=64).contains(&b.len()) && alnum(b[0]) && alnum(b[b.len() - 1]) && b.iter().all(|&c| alnum(c) || c == b'-')
}

pub fn is_relative_file(file: &str) -> bool {
    if file.is_empty() || file.len() > 240 || file.starts_with('/') || file.ends_with('/') || file.contains('\\') || file.contains('\0') {
        return false;
    }
    let parts: Vec<&str> = file.split('/').collect();
    (1..=16).contains(&parts.len())
        && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.starts_with('.') && !p.chars().any(char::is_control))
}

pub fn is_github_folder(folder: &str) -> bool {
    if folder.is_empty() { return true; }
    let normalized = folder
        .split('/')
        .map(|p| match p.strip_prefix('.') {
            Some(r) if !r.is_empty() && !r.starts_with('.') => format!("_{r}"),
            _ => p.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("/");
    is_relative_file(&normalized)
}

/// Strict `decodeURIComponent` equivalent (rejects malformed `%XX` escapes and invalid UTF-8).
pub fn decode_uri_component(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = (bytes.get(i + 1).copied()? as char).to_digit(16)?;
            let lo = (bytes.get(i + 2).copied()? as char).to_digit(16)?;
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn skill_name(text: &str) -> String {
    let lower = text.nfc().collect::<String>().to_lowercase();
    let (mut words, mut cur) = (Vec::new(), String::new());
    for ch in lower.chars() {
        if ch.is_alphabetic() || ch.is_numeric() {
            if (ch as u32) < 128 { cur.push(ch); }
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() { words.push(cur); }
    let joined = words.join("-");
    joined[..joined.len().min(64)].trim_end_matches('-').to_owned()
}

pub fn skill_trash_entry(entry: &str) -> Option<SkillTrashEntry> {
    if entry.encode_utf16().count() > 141 { return None; }
    let (folder, secs) = entry.rsplit_once('-')?;
    if !(1..=12).contains(&secs.len()) || !secs.bytes().all(|b| b.is_ascii_digit()) || !is_skill_folder(folder) {
        return None;
    }
    Some(SkillTrashEntry { folder: folder.into(), deleted: secs.parse().ok()? })
}

pub fn skill_about(markdown: &str) -> SkillAbout {
    let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut about = SkillAbout::default();
    if lines.first().map(|l| l.trim()) != Some("---") { return about; }
    let mut i = 1;
    while i < lines.len() && lines[i].trim() != "---" {
        let line = lines[i];
        if !line.starts_with(' ') && let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let mut val = v.trim().to_owned();
            if matches!(val.as_str(), ">" | "|" | ">-" | "|-") {
                let mut parts = Vec::new();
                while i + 1 < lines.len() && lines[i + 1].starts_with(' ') {
                    i += 1;
                    parts.push(lines[i].trim());
                }
                val = parts.join(" ");
            }
            if val.starts_with('"') && val.ends_with('"') && val.len() >= 2 {
                val = serde_json::from_str::<String>(&val).unwrap_or_else(|_| val[1..val.len() - 1].into());
            } else if val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2 {
                val = val[1..val.len() - 1].replace("''", "'");
            }
            if key == "name" { about.name = Some(val); }
            else if key == "description" { about.description = Some(val); }
        }
        i += 1;
    }
    about
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_names_match_route_fixture_vectors() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/fixtures/routes/names/skill-names.json");
        let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for case in parsed["cases"].as_array().unwrap() {
            assert_eq!(skill_name(case["typed"].as_str().unwrap()), case["name"].as_str().unwrap());
        }
    }

    #[test]
    fn parses_frontmatter_and_trash_ids() {
        let about = skill_about("---\nname: \"release-notes\"\ndescription: >-\n  Writes the\n  release notes\n---\nBody\n");
        assert_eq!(about.name.as_deref(), Some("release-notes"));
        assert_eq!(about.description.as_deref(), Some("Writes the release notes"));
        assert_eq!(skill_trash_entry("release-notes-1761400000"), Some(SkillTrashEntry { folder: "release-notes".into(), deleted: 1761400000 }));
        assert!(skill_trash_entry(".hidden-1761400000").is_none());
        assert!(decode_uri_component("a+b%20c").as_deref() == Some("a+b c"));
        assert!(decode_uri_component("%ZZ").is_none());
    }
}
