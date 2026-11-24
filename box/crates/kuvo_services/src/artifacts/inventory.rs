//! Inventory of artifact versions and retained content-addressed copies,
//! matching `box/src/link/artifacts/inventory.ts`.

use std::{collections::BTreeMap, fs, io, path::Path};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_store::format::ObjectFormat;
use serde_json::{Map, Value};

pub const ARTIFACT_INDEX_FORMAT: ObjectFormat = ObjectFormat::additive("Artifact index");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyExpectation {
    pub expected_size: u64,
    pub expected_sha: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error(transparent)]
    Format(#[from] kuvo_store::format::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{0}")]
    Invalid(&'static str),
}

/// Latest entry per artifact version, in the same journal order the store replays.
pub fn artifact_versions(root: &Path) -> Result<BTreeMap<String, Map<String, Value>>, InventoryError> {
    let log = root.join("log");
    let mut versions = BTreeMap::new();
    let Ok(meta) = fs::symlink_metadata(&log) else {
        return Ok(versions);
    };
    if !meta.is_dir() {
        return Ok(versions);
    }
    let mut names: Vec<String> = fs::read_dir(&log)?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        if !name.ends_with(".jsonl") || name.starts_with('.') {
            continue;
        }
        let file = log.join(&name);
        let Ok(file_meta) = fs::symlink_metadata(&file) else {
            continue;
        };
        if !file_meta.is_file() {
            continue;
        }
        let text = fs::read_to_string(&file)?;
        for line in text.split('\n') {
            if line.is_empty() {
                continue;
            }
            let Ok(saved) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if !saved.is_object() {
                continue;
            }
            let decoded = ARTIFACT_INDEX_FORMAT.decode(saved)?;
            let a = decoded.fields.get("a").and_then(Value::as_object);
            let v = decoded.fields.get("v").and_then(Value::as_object);
            if let (Some(a), Some(v)) = (a, v)
                && let Some(id) = a.get("id").and_then(Value::as_str)
                && let Some(n) = v.get("n").and_then(Value::as_u64)
            {
                versions.insert(format!("artifacts/{id}/versions/{n}"), v.clone());
            }
        }
    }
    Ok(versions)
}

pub fn retained_artifact_copies(
    root: &Path,
) -> Result<BTreeMap<String, CopyExpectation>, InventoryError> {
    let mut copies: BTreeMap<String, CopyExpectation> = BTreeMap::new();
    for version in artifact_versions(root)?.values() {
        if version.get("kept").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let hex_str = version
            .get("hash")
            .and_then(Value::as_str)
            .and_then(|h| h.strip_prefix("sha256:"))
            .filter(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        let size = version.get("size").and_then(Value::as_u64);
        let (Some(hex_str), Some(expected_size)) = (hex_str, size) else {
            return Err(InventoryError::Invalid(
                "A retained artifact version has an invalid inventory.",
            ));
        };
        let raw = hex::decode(hex_str)
            .map_err(|_| InventoryError::Invalid("A retained artifact version has an invalid inventory."))?;
        let name = format!("artifacts/copies/{}/{hex_str}", &hex_str[..2]);
        if let Some(existing) = copies.get(&name)
            && existing.expected_size != expected_size
        {
            return Err(InventoryError::Invalid(
                "Retained artifact versions disagree about their byte count.",
            ));
        }
        copies.insert(
            name,
            CopyExpectation {
                expected_size,
                expected_sha: URL_SAFE_NO_PAD.encode(raw),
            },
        );
    }
    Ok(copies)
}
