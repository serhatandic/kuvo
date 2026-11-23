use super::paths::{PathMap, serialize_json};
use crate::{Disk, Error, entries, metadata};
use kuvo_store::{files, format::check_version};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{io, path::Path};

pub(super) fn rewrite(stage: &Disk, map: &PathMap) -> Result<(), Error> {
    let settings = stage.path("artifacts/settings.json")?;
    if let Some(before) = regular_file(&settings)?
        && let Ok(text) = std::str::from_utf8(&before)
        && map.names_old(text)
        && let Ok(mut record) = serde_json::from_str::<Value>(text)
    {
        check_version(
            "Artifact storage settings",
            record.get("formatVersion"),
            0,
            1,
        )?;
        if map.json(&mut record) {
            let after = serialize_json(&record, &before).map_err(io::Error::other)?;
            files::rewrite_mac_entry(&settings, &after)?;
        }
    }

    let logs = stage.path("artifacts/log")?;
    if metadata(&logs)?.is_some_and(|meta| !meta.is_dir()) {
        return Ok(());
    }
    for log in entries(&logs)? {
        let file = log.path();
        let extension = file.extension().and_then(|part| part.to_str());
        if matches!(extension, Some("jsonl" | "ndjson"))
            && let Some(before) = regular_file(&file)?
            && let Ok(text) = std::str::from_utf8(&before)
            && let Some(after) = rewrite_log(text, map)?
        {
            files::rewrite_mac_entry(&file, after.as_bytes())?;
        }
    }
    Ok(())
}

fn regular_file(file: &Path) -> Result<Option<Vec<u8>>, Error> {
    if metadata(file)?.is_some_and(|meta| meta.is_file()) {
        return Ok(files::read(file)?);
    }
    Ok(None)
}

fn rewrite_log(text: &str, map: &PathMap) -> Result<Option<String>, Error> {
    let mut after = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let Some(replacement) = rewrite_line(line, map)? else {
            after.push_str(line);
            continue;
        };
        after.push_str(&replacement);
        if line.ends_with('\n') {
            after.push('\n');
        }
    }
    Ok((after != text).then_some(after))
}

fn rewrite_line(line: &str, map: &PathMap) -> Result<Option<String>, Error> {
    if !map.names_old(line) {
        return Ok(None);
    }
    let Ok(mut record) = serde_json::from_str::<Value>(line) else {
        return Ok(None);
    };
    check_version("Artifact index", record.get("formatVersion"), 0, 1)?;
    if let Some(artifact) = record.get_mut("a").and_then(Value::as_object_mut)
        && let Some(path) = artifact.get("path").and_then(Value::as_str)
        && artifact.get("id").and_then(Value::as_str) == Some(artifact_id(path).as_str())
        && let Some(next) = map.absolute(path)
    {
        artifact.insert("id".into(), artifact_id(&next).into());
    }
    Ok(map.json(&mut record).then(|| record.to_string()))
}

fn artifact_id(path: &str) -> String {
    let hash = Sha256::digest(path.as_bytes());
    format!("art_{}", hex::encode(&hash[..12]))
}
