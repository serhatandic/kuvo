//! Read reviewable work from the held recovery area.
use std::{fs, path::Path};

use kuvo_store::format::{ObjectFormat, check_version};
use serde_json::{Value, json};

use super::{
    Backups, Error, Result,
    inventory::stat,
    recovery::{output, record_key},
};

pub(super) fn list(backups: &Backups) -> Result<Value> {
    let backup = backups.root().join("backup");
    let Some(journal) = ObjectFormat::additive("Restore journal")
        .read(&backup.join("restore.json"))
        .map_err(|error| Error::Detail(error.to_string()))?
    else {
        return Ok(json!({"waiting": [], "queueItems": []}));
    };
    if journal.fields.get("state").and_then(Value::as_str) != Some("restored") {
        return Ok(json!({"waiting": [], "queueItems": []}));
    }
    let id = journal
        .fields
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| super::PointId::try_from(id.to_owned()).ok());
    let Some(id) = id else {
        return Ok(json!({"waiting": [], "queueItems": []}));
    };
    let recovery = output(&backup, &format!("recovered/{id}"))?;
    let waiting = waiting(&recovery)?;
    let items = items(backups.root(), &backup, &recovery, &id)?;
    Ok(json!({ "waiting": waiting, "queueItems": items }))
}

fn waiting(recovery: &Path) -> Result<Vec<Value>> {
    let invalid = || Error::Message("The queue index could not be read");
    let file = output(recovery, "home/run/queues.json")?;
    let Some(bytes) = kuvo_store::files::read(&file)? else {
        return Ok(Vec::new());
    };
    let book = serde_json::from_slice::<Value>(&bytes)?
        .as_object()
        .cloned()
        .ok_or_else(invalid)?;
    let mut waiting = Vec::new();
    for (session, queue) in book {
        let queue = queue.as_object().ok_or_else(invalid)?;
        check_version("Conversation queue", queue.get("formatVersion"), 0, 1)
            .map_err(|error| Error::Detail(error.to_string()))?;
        let Some(messages) = queue.get("waiting") else {
            continue;
        };
        for item in messages.as_array().ok_or_else(invalid)? {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| {
                    id.strip_prefix("msg_").is_some_and(|id| {
                        (1..=80).contains(&id.len())
                            && id.bytes().all(|b| b.is_ascii_alphanumeric())
                    })
                })
                .ok_or_else(invalid)?;
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            let payload = output(recovery, &format!("home/run/waiting/{id}.json"))?;
            let at = item
                .get("at")
                .filter(|at| at.is_number())
                .cloned()
                .unwrap_or_else(|| json!(0));
            let mut kept = json!({ "session": session, "id": id, "text": text, "at": at,
                "paused": true, "review": true, "available": stat(&payload)?.is_some() });
            if let Some(from) = item.get("from").and_then(Value::as_str) {
                kept["from"] = json!(from);
            }
            let files: Vec<_> = item
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !files.is_empty() {
                kept["files"] = json!(files);
            }
            waiting.push(kept);
        }
    }
    Ok(waiting)
}

fn items(
    home: &Path,
    backup: &Path,
    recovery: &Path,
    point: &super::PointId,
) -> Result<Vec<Value>> {
    let folder = output(recovery, "work-queue-items")?;
    if !stat(&folder)?.is_some_and(|info| info.is_dir()) {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(folder)?.collect::<std::io::Result<Vec<_>>>()?;
    files.sort_unstable_by_key(|entry| entry.file_name());
    let mut items = Vec::new();
    for entry in files {
        if !entry.file_type()?.is_file()
            || !entry
                .file_name()
                .to_string_lossy()
                .strip_suffix(".json")
                .is_some_and(|stem| super::url_token(stem, 43))
        {
            continue;
        }
        let item: Value = serde_json::from_reader(super::archive::regular(&entry.path())?)?;
        let Some(id) = item.get("id").and_then(Value::as_str).filter(|id| {
            id.strip_prefix("wqi_")
                .is_some_and(|token| super::url_token(token, 20))
        }) else {
            continue;
        };
        let Some(queue) = item.get("owner").and_then(Value::as_str) else {
            continue;
        };
        let key = record_key(&[id]);
        let review = output(backup, &format!("reviewed/{point}/work-queue-{key}.json"))?;
        if stat(&review)?.is_some() {
            continue;
        }
        let mut listed = Vec::new();
        for file in item
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = file.get("name").and_then(Value::as_str) else {
                continue;
            };
            let file = output(recovery, &format!("home/run/work-queue/files/{id}/{name}"))?;
            listed.push(json!({ "name": name, "available": stat(&file)?.is_some_and(|info| info.is_file()) }));
        }
        let current_fence = |family| {
            output(home, &format!("run/owned-data/edit-{family}/{key}.json"))
                .and_then(|file| Ok(stat(&file)?.is_some()))
        };
        let can_keep =
            queue == "usr_you" && !current_fence("deletions")? && !current_fence("departures")?;
        items.push(json!({ "id": id, "queue": queue, "title": item.get("title").and_then(Value::as_str).unwrap_or(""),
            "body": item.get("body").and_then(Value::as_str).unwrap_or(""), "canKeep": can_keep, "files": listed }));
    }
    Ok(items)
}
