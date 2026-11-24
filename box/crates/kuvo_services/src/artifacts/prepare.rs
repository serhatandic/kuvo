//! Startup preparation for artifacts and library: reading saved conversations for citations
//! (`artifacts/citations-v1.json`) and one-time library backfill (`library/backfill.json`),
//! matching `box/src/link/artifacts/prepare.ts`.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::store::ShownCitation;

#[derive(Clone, Debug)]
pub struct SavedSessionHeader {
    pub folder: PathBuf,
    pub id: String,
    pub parent_id: Option<String>,
    pub directory: Option<String>,
    pub metadata: Option<Value>,
}

pub fn read_saved_session_headers(sessions_dir: &Path) -> Vec<SavedSessionHeader> {
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let folder = entry.path();
        if !folder.is_dir() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(folder.join("session.json")) else {
            continue;
        };
        let Ok(val) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(id) = val.get("id").and_then(Value::as_str) else {
            continue;
        };
        out.push(SavedSessionHeader {
            folder,
            id: id.to_owned(),
            parent_id: val
                .get("parentID")
                .and_then(Value::as_str)
                .map(str::to_owned),
            directory: val
                .pointer("/location/directory")
                .and_then(Value::as_str)
                .map(str::to_owned),
            metadata: val.get("metadata").cloned(),
        });
    }
    out
}

pub fn saved_messages(folder: &Path) -> Vec<Value> {
    let sqlite_path = folder.join("transcript-v1.sqlite");
    if sqlite_path.is_file()
        && let Ok(conn) = rusqlite::Connection::open_with_flags(
            &sqlite_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        && let Ok(mut stmt) = conn.prepare("SELECT body FROM messages ORDER BY seq ASC")
        && let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0))
    {
        return rows
            .filter_map(Result::ok)
            .filter_map(|body| serde_json::from_str::<Value>(&body).ok())
            .collect();
    }
    let json_path = folder.join("messages.json");
    let Ok(raw) = fs::read_to_string(&json_path) else {
        return Vec::new();
    };
    let Ok(val) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    match val {
        Value::Array(items) => items,
        Value::Object(mut map) => match map.remove("messages") {
            Some(Value::Array(items)) => items,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn file_stamp(path: &Path) -> Option<(u64, i128)> {
    let meta = fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i128);
    Some((meta.len(), mtime_ns))
}

fn revision_key(stamps: &[Option<(u64, i128)>]) -> String {
    let mut hasher = Sha256::new();
    for stamp in stamps {
        match stamp {
            Some((size, mtime)) => {
                hasher.update(format!("{size}:{mtime};").as_bytes());
            }
            None => {
                hasher.update(b"-;");
            }
        }
    }
    hex::encode(hasher.finalize())
}

fn may_have_citations(file: &Path) -> bool {
    let Ok(mut f) = File::open(file) else {
        return false;
    };
    let mut buf = vec![0u8; 64 * 1024];
    let mut overlap = 0usize;
    loop {
        let Ok(n) = f.read(&mut buf[overlap..]) else {
            return false;
        };
        if n == 0 {
            return false;
        }
        let len = overlap + n;
        let slice = &buf[..len];
        if slice.windows(7).any(|w| w == b":codex-")
            || slice.windows(6).any(|w| w == b"cited-")
            || slice.windows(2).any(|w| w == b"\\u")
        {
            return true;
        }
        let next_overlap = len.min(6);
        let start = len - next_overlap;
        buf.copy_within(start..len, 0);
        overlap = next_overlap;
    }
}

pub fn citation_update(
    sessions_dir: &Path,
    previous: &HashMap<String, String>,
) -> (Vec<ShownCitation>, HashMap<String, String>) {
    let mut shown: HashMap<String, i64> = HashMap::new();
    let mut checkpoints: HashMap<String, String> = HashMap::new();
    for session in read_saved_session_headers(sessions_dir) {
        let files = [
            file_stamp(&session.folder.join("session.json")),
            file_stamp(&session.folder.join("transcript-v1.sqlite")),
            file_stamp(&session.folder.join("transcript-v1.sqlite-wal")),
            file_stamp(&session.folder.join("messages.json")),
        ];
        let revision = revision_key(&files);
        let changed = previous.get(&session.id).map(String::as_str) != Some(revision.as_str());
        if changed
            && (files[1].is_some()
                || (files[3].is_some() && may_have_citations(&session.folder.join("messages.json"))))
        {
            let dir = session.directory.as_deref().unwrap_or("/");
            for message in saved_messages(&session.folder) {
                if message.get("type").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let msg_id = message.get("id").and_then(Value::as_str).unwrap_or("");
                let prefix = format!("cited-{msg_id}-");
                let at = message
                    .pointer("/time/completed")
                    .or_else(|| message.pointer("/time/created"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let Some(content) = message.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for part in content {
                    if part.get("type").and_then(Value::as_str) != Some("tool") {
                        continue;
                    }
                    let Some(part_id) = part.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    if !part_id.starts_with(&prefix) {
                        continue;
                    }
                    let Some(raw_path) = part.pointer("/state/input/path").and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let path = Path::new(dir)
                        .join(raw_path)
                        .to_string_lossy()
                        .into_owned();
                    let entry = shown.entry(path).or_insert(i64::MIN);
                    if at > *entry {
                        *entry = at;
                    }
                }
            }
        }
        checkpoints.insert(session.id, revision);
    }
    let citations = shown
        .into_iter()
        .map(|(path, at_ms)| ShownCitation { path, at_ms })
        .collect();
    (citations, checkpoints)
}

pub fn load_citation_checkpoints(artifacts_root: &Path) -> HashMap<String, String> {
    let path = artifacts_root.join("citations-v1.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_citation_checkpoints(artifacts_root: &Path, checkpoints: &HashMap<String, String>) {
    let path = artifacts_root.join("citations-v1.json");
    let map: Map<String, Value> = checkpoints
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    if let Ok(encoded) = serde_json::to_string(&Value::Object(map)) {
        let _ = kuvo_store::files::write_secret(&path, encoded.as_bytes());
    }
}
