//! Preserve ordinary note histories with the session. The editor owns their
//! merge rules. Placement checks the revision records and the move receipt.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_store::{files, locks::OwnedLock};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const MAX_NOTE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Notes {
    version: u32,
    #[serde(rename = "move", skip_serializing_if = "Option::is_none")]
    movement: Option<String>,
    notes: BTreeMap<String, History>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Deserialize, Serialize)]
struct History {
    versions: Vec<Revision>,
    choices: Vec<Choice>,
    receipts: Vec<Receipt>,
    additions: Map<String, Value>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Deserialize, Serialize)]
struct Revision {
    revision: String,
    deleted: bool,
    fields: Map<String, Value>,
    passages: Option<Map<String, Value>>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Deserialize, Serialize)]
struct Choice {
    id: String,
    field: String,
    base: String,
    accepted: String,
    proposed: String,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    digest: String,
    decision: NoteDecision,
}

#[derive(Deserialize, Serialize)]
struct NoteDecision {
    operation: String,
    revision: String,
    status: NoteStatus,
    choices: Vec<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum NoteStatus {
    Committed,
    Choice,
    Deleted,
    Refresh,
}

fn check(raw: Value) -> io::Result<Notes> {
    let notes: Notes = serde_json::from_value(raw).map_err(|_| invalid())?;
    if notes.version != 1
        || notes.notes.iter().any(|(id, history)| {
            !safe_id(id, 80)
                || history.versions.is_empty()
                || history.versions.iter().any(|version| {
                    !key(&version.revision)
                        || version
                            .passages
                            .as_ref()
                            .is_some_and(|value| !passages(value))
                        || if version.deleted {
                            !version.fields.is_empty()
                        } else {
                            version.fields.len() != 1
                                || !version
                                    .fields
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .is_some_and(text)
                        }
                })
                || history.choices.iter().any(|choice| {
                    !key(&choice.id)
                        || choice.field != "text"
                        || ![&choice.base, &choice.accepted, &choice.proposed]
                            .into_iter()
                            .all(|value| text(value))
                })
                || history.receipts.iter().any(|receipt| {
                    !key(&receipt.digest)
                        || !key(&receipt.decision.operation)
                        || !key(&receipt.decision.revision)
                        || !receipt.decision.choices.iter().all(|id| key(id))
                })
        })
    {
        return Err(invalid());
    }
    Ok(notes)
}

fn passages(value: &Map<String, Value>) -> bool {
    let Some(fields) = value.get("fields").and_then(Value::as_object) else {
        return false;
    };
    let Some(hunks) = fields.get("text").and_then(Value::as_array) else {
        return false;
    };
    if fields.len() != 1 || !value.get("base").and_then(Value::as_str).is_some_and(key) {
        return false;
    }
    let mut end = 0;
    for hunk in hunks {
        let Some(start) = hunk["start"].as_u64() else {
            return false;
        };
        let Some(next_end) = hunk["end"].as_u64() else {
            return false;
        };
        let Some(lines) = hunk["text"].as_array() else {
            return false;
        };
        if start < end
            || next_end < start
            || next_end > 9_007_199_254_740_991
            || !lines.iter().all(Value::is_string)
        {
            return false;
        }
        if let Some(pieces) = hunk.get("pieces") {
            let Some(pieces) = pieces.as_array() else {
                return false;
            };
            if start != next_end
                || pieces.is_empty()
                || pieces.iter().any(|piece| {
                    !piece["text"].as_str().is_some_and(text)
                        || piece
                            .get("operation")
                            .is_some_and(|operation| !operation.as_str().is_some_and(key))
                })
            {
                return false;
            }
            if pieces
                .iter()
                .filter_map(|piece| piece["text"].as_str())
                .collect::<String>()
                != lines.iter().filter_map(Value::as_str).collect::<String>()
            {
                return false;
            }
        }
        end = next_end;
    }
    true
}

fn key(value: &str) -> bool {
    value.len() == 43 && safe_id(value, 43)
}
fn text(value: &str) -> bool {
    value.encode_utf16().count() <= 20_000
}
fn safe_id(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "This computer cannot read that session's ordinary notes.",
    )
}

fn note_file(run: &Path, session: &str) -> io::Result<PathBuf> {
    if !session
        .strip_prefix("ses_")
        .is_some_and(|tail| safe_id(tail, 95))
    {
        return Err(invalid());
    }
    Ok(run
        .join("work/notes")
        .join(format!("{session}.ordinary.json")))
}

fn read(path: &Path) -> io::Result<Option<Value>> {
    files::read(path)?
        .map(|bytes| {
            if bytes.len() > MAX_NOTE_BYTES {
                return Err(invalid());
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            check(value.clone())?;
            Ok(value)
        })
        .transpose()
}

fn lock(file: &Path) -> io::Result<OwnedLock> {
    let mut suffix = file.as_os_str().to_os_string();
    suffix.push(".lock");
    OwnedLock::acquire(&PathBuf::from(suffix), Duration::from_secs(3))
}

pub(super) fn pack(run: &Path, sessions: &[String]) -> io::Result<Option<BTreeMap<String, Value>>> {
    let mut notes = BTreeMap::new();
    for session in sessions {
        let file = note_file(run, session)?;
        if file.try_exists()? {
            let _guard = lock(&file)?;
            if let Some(value) = read(&file)? {
                notes.insert(session.clone(), value);
            }
        }
    }
    Ok((!notes.is_empty()).then_some(notes))
}

pub(super) fn place(
    run: &Path,
    sessions: &[String],
    notes: BTreeMap<String, Value>,
    movement: &str,
) -> io::Result<()> {
    let mut checked = Vec::new();
    for (id, mut value) in notes {
        if !sessions.contains(&id) {
            return Err(io::Error::other(
                "The move includes notes for another session.",
            ));
        }
        let file = note_file(run, &id)?;
        check(value.clone())?;
        value["move"] = movement.into();
        let data = serde_json::to_vec(&value)?;
        if data.len() > MAX_NOTE_BYTES {
            return Err(invalid());
        }
        checked.push((file, data));
    }
    for (file, data) in checked {
        let _guard = lock(&file)?;
        if read(&file)?.is_some_and(|prior| prior["move"].as_str() != Some(movement)) {
            return Err(io::Error::other(
                "Ordinary notes for that session already exist at the destination.",
            ));
        }
        files::write_atomic_through(&file, &data, run)?;
    }
    Ok(())
}

pub(super) fn discard(run: &Path, sessions: &[String], movement: Option<&str>) -> io::Result<()> {
    for id in sessions {
        let file = note_file(run, id)?;
        if !file.try_exists()? {
            continue;
        }
        let _guard = lock(&file)?;
        if let Some(value) = read(&file)?
            && movement.is_none_or(|movement| value["move"].as_str() == Some(movement))
        {
            files::remove_file_synced(&file)?;
        }
    }
    Ok(())
}
