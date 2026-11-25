//! Work Queue and in-flight session work that keeps a computer from joining
//! another setup. A running resident keeps lock files and empty lists in these
//! folders, so only kept numbers, items, labels and pending exchanges count.

use std::collections::HashSet;

use serde_json::Value;

use crate::{Error, storage::Files};

const QUEUE: &str = "run/work-queue";
/// How long a forwarded queue request is waited for, then sent again.
const FORWARD_WAIT: u64 = 20_000;
const FORWARD_FILE_WAIT: u64 = 60_000;
const FORWARD_AGAIN: u64 = 5_000;

pub(super) fn has_queue_or_session_work(files: &Files, now: u64) -> Result<bool, Error> {
    Ok(numbers_in_use(files)?
        || kept_items(files)?
        || labels_kept(files)?
        || files.read::<Value>(&format!("{QUEUE}/categories.json"))?.is_some_and(|value| !value.is_null())
        || unanswered_asks(files)?
        || waiting_forwards(files, now)?
        || files
            .read::<Value>("run/work/wakes.json")?
            .is_some_and(|wakes| wakes.as_object().is_some_and(|wakes| !wakes.is_empty()))
        || background_work(files)?)
}

fn numbers_in_use(files: &Files) -> Result<bool, Error> {
    let Some(numbers) = files.read::<Value>(&format!("{QUEUE}/numbers.json"))? else {
        return Ok(false);
    };
    let filled = |field: &str| match &numbers[field] {
        Value::Array(rows) => !rows.is_empty(),
        Value::Object(rows) => !rows.is_empty(),
        _ => false,
    };
    let asking = numbers["asking"]["id"].is_string()
        && numbers["asking"]["count"].is_number()
        && numbers["asking"]["since"].is_number();
    Ok(numbers["next"].as_u64().is_some_and(|next| next > 1)
        || asking
        || ["leases", "used", "given", "granted"].into_iter().any(filled))
}

fn kept_items(files: &Files) -> Result<bool, Error> {
    let folder = format!("{QUEUE}/queues");
    for name in files.names(&folder)? {
        if !name.ends_with(".json") {
            continue;
        }
        let Some(queue) = files.read::<Value>(&format!("{folder}/{name}"))? else {
            continue;
        };
        let items = queue["items"]
            .as_array()
            .ok_or(Error::Invalid("Work Queue store"))?;
        if items.iter().any(|item| item["id"].is_string()) {
            return Ok(true);
        }
        if !items.is_empty() {
            return Err(Error::Invalid("Work Queue item"));
        }
    }
    Ok(false)
}

fn labels_kept(files: &Files) -> Result<bool, Error> {
    let Some(labels) = files.read::<Value>(&format!("{QUEUE}/labels.json"))? else {
        return Ok(false);
    };
    Ok(["labels", "removed"]
        .into_iter()
        .any(|field| labels[field].as_array().is_some_and(|rows| !rows.is_empty())))
}

fn unanswered_asks(files: &Files) -> Result<bool, Error> {
    let Some(asks) = files.read::<Value>("run/work/asks.json")? else {
        return Ok(false);
    };
    let Some(asks) = asks["asks"].as_array() else {
        return Ok(true);
    };
    Ok(asks.iter().any(|ask| {
        !ask.is_object()
            || ask["answered"] != true
            || (ask["late"] == true && ask["toldLate"] != true)
    }))
}

fn legacy_version(record: &Value) -> bool {
    matches!(record.get("formatVersion"), None | Some(Value::Null))
        || record["formatVersion"] == 0
        || record["formatVersion"] == 1
}

/// A forwarded request still waits while its spool is unknown or its act is in time.
fn pending_forward(spool: Option<&Value>, now: u64) -> bool {
    let Some(spool) = spool.filter(|value| value.is_object()) else {
        return true;
    };
    let scoped = spool.get("scopedAct").is_some();
    if scoped && (spool.get("act").is_some() || !spool["scopedAct"].is_object()) {
        return true;
    }
    let act = if scoped { &spool["scopedAct"] } else { &spool["act"] };
    let records = [spool, act, if scoped { &spool["grant"] } else { &Value::Null }];
    if !records
        .iter()
        .all(|record| !record.is_object() || legacy_version(record))
    {
        return true;
    }
    let Some(at) = act["at"].as_u64() else {
        return false;
    };
    let wait = if act["action"] == "file" || act["area"] == "backup" {
        FORWARD_FILE_WAIT
    } else if (act["area"] == "setup" || act["area"] == "addComputer") && act["method"] != "GET" {
        2 * FORWARD_WAIT + FORWARD_AGAIN
    } else {
        FORWARD_WAIT
    };
    now.saturating_sub(at) <= wait + FORWARD_AGAIN
}

fn waiting_forwards(files: &Files, now: u64) -> Result<bool, Error> {
    let folder = format!("{QUEUE}/forward");
    let names: HashSet<String> = files.names(&folder)?.into_iter().collect();
    for name in &names {
        let Some(id) = name
            .strip_suffix(".json")
            .filter(|id| !id.ends_with(".answer"))
            .filter(|id| {
                id.strip_prefix("wqa_").is_some_and(|rest| {
                    (16..=40).contains(&rest.len())
                        && rest
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                })
            })
        else {
            continue;
        };
        let answer = format!("{id}.answer.json");
        let waiting = if names.contains(&answer) {
            files
                .read::<Value>(&format!("{folder}/{answer}"))?
                .is_none_or(|answer| !legacy_version(&answer))
        } else {
            pending_forward(files.read::<Value>(&format!("{folder}/{name}"))?.as_ref(), now)
        };
        if waiting {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Background commands, subagents or tasks a session still has running.
fn background_work(files: &Files) -> Result<bool, Error> {
    let Some(places) = files.read::<Value>("run/work/background.json")? else {
        return Ok(false);
    };
    Ok(places.as_object().is_some_and(|places| {
        places.values().any(|place| {
            place["sessions"].as_object().is_some_and(|sessions| {
                sessions.values().any(|work| {
                    ["commands", "subagents", "tasks"]
                        .into_iter()
                        .any(|kind| work[kind].as_array().is_some_and(|rows| !rows.is_empty()))
                })
            })
        })
    }))
}
