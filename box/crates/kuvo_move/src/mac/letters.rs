use crate::{Disk, Error, json_at, save_json};
use kuvo_store::format::{ObjectFormat, read_additive_list};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const LEGACY: ObjectFormat = ObjectFormat::additive("Legacy Mac import object");
const TWO_DAYS_MS: u64 = 172_800_000;

#[derive(Serialize)]
struct Letter<'a> {
    from: Option<&'a str>,
    to: &'a str,
    text: String,
    wait: bool,
    at: Value,
    peer: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    answers: Option<&'a str>,
}

pub(super) fn upgrade(source: &Path, staging: &Disk) -> Result<usize, Error> {
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_millis(),
    )
    .map_err(std::io::Error::other)?;
    let waiting = read_list(&source.join("agent-messages.json"))?;
    let mailed = read_list(&source.join("agent-letters.json"))?;
    let seen = read_object(&source.join("agent-letters-seen.json"))?;
    if waiting.is_empty() && mailed.is_empty() && seen.is_none() {
        return Ok(0);
    }
    let hosts = read_object(&source.join("phone-link.json"))?.unwrap_or_default();
    let machines = hosts.get("machines").and_then(Value::as_object);
    let mut dropped = 0;
    for (entries, pending) in [(mailed, false), (waiting, true)] {
        for entry in entries.iter().filter(|entry| entry.is_object()) {
            let from = entry.get("from").and_then(Value::as_str);
            let to = entry.get("to").and_then(Value::as_str);
            let text = entry.get("text").and_then(Value::as_str);
            let machine = entry.get("machine").and_then(Value::as_str);
            let peer = machine
                .and_then(|name| machines?.get(name))
                .filter(|host| {
                    host.get("peers").and_then(Value::as_bool) == Some(true)
                        && host.get("anotherMac").and_then(Value::as_bool) != Some(true)
                })
                .and_then(|host| host.get("key")?.as_str())
                .filter(|key| !key.is_empty());
            let (Some(to), Some(text), Some(peer)) = (to, text, peer) else {
                dropped += 1;
                continue;
            };
            let id = if pending {
                let hash = Sha256::digest(format!("{}\0{to}\0{text}", from.unwrap_or("")));
                format!("wtg_{}", hex::encode(&hash[..16]))
            } else if let Some(id) = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| letter_id(id))
            {
                id.to_owned()
            } else {
                dropped += 1;
                continue;
            };
            // Bound the string by the UTF-16 length the retired app used.
            let mut size = 0;
            let text = text
                .chars()
                .take_while(|ch| {
                    size += ch.len_utf16();
                    size <= 32_000
                })
                .collect();
            let at = entry
                .get("at")
                .filter(|at| at.is_number())
                .cloned()
                .unwrap_or_else(|| now.into());
            let file = staging.path(Path::new("run/outbox/sending").join(format!("{id}.json")))?;
            save_json(
                &file,
                &Letter {
                    from,
                    to,
                    text,
                    wait: false,
                    at,
                    peer,
                    answers: entry.get("answers").and_then(Value::as_str),
                },
            )?;
        }
    }

    if let Some(seen) = seen {
        let file = staging.path("run/letters-seen.json")?;
        let existing: Map<String, Value> = json_at(&file).ok().flatten().unwrap_or_default();
        let mut marks = Map::new();
        for (id, at) in existing.into_iter().chain(seen) {
            if letter_id(&id)
                && let Some(number) = at.as_f64()
                && (now as f64 - number) < TWO_DAYS_MS as f64
            {
                let previous = marks.entry(id).or_insert_with(|| 0.into());
                if number > previous.as_f64().unwrap_or_default() {
                    *previous = at;
                }
            }
        }
        if !marks.is_empty() {
            save_json(&file, &marks)?;
        }
    }
    Ok(dropped)
}

fn letter_id(id: &str) -> bool {
    (6..=80).contains(&id.len())
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_'))
}

fn read_list(file: &Path) -> Result<Vec<Value>, Error> {
    match json_at::<Value>(file).ok().flatten() {
        Some(value @ Value::Array(_)) => Ok(read_additive_list("Legacy Mac import list", value)?),
        Some(value @ Value::Object(_)) => {
            let _ = LEGACY.decode(value)?;
            Ok(Vec::new())
        }
        _ => Ok(Vec::new()),
    }
}

fn read_object(file: &Path) -> Result<Option<Map<String, Value>>, Error> {
    match json_at::<Value>(file).ok().flatten() {
        Some(value @ Value::Object(_)) => Ok(Some(LEGACY.decode(value)?.fields)),
        Some(value @ Value::Array(_)) => {
            let _ = read_additive_list("Legacy Mac import list", value)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}
