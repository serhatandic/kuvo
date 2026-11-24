//! Session-scoped forwarding decisions and replay fences. The move carries
//! these records with their original addresses, so a retry keeps its identity.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_store::{
    declarations::{self, Owner},
    files,
    format::ObjectFormat,
    locks::OwnedLock,
};
use kuvo_wire::keys::encode;
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const OWNED: ObjectFormat = ObjectFormat::additive("Owned data");

#[derive(Deserialize)]
struct DraftFence {
    setup: String,
    session: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Forward {
    kind: String,
    setup: String,
    operation: String,
    owner: String,
    listed_by: Option<String>,
    submitter: Submitter,
    area: String,
    record: String,
    #[serde(rename = "ref")]
    reference: Option<Reference>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Submitter {
    Computer,
    Paired { key: String, pairing: String },
    Ssh { key: String },
}

#[derive(Deserialize)]
struct Reference {
    kind: String,
    id: Option<String>,
    queue: Option<String>,
}

enum Scope {
    Session { setup: String, id: String },
    QueueItem { setup: String, id: String },
    Other,
}

impl Scope {
    fn follows(&self, setup: &str, sessions: &[String], item: Option<&str>) -> bool {
        match self {
            Self::Session { setup: family, id } => family == setup && sessions.contains(id),
            Self::QueueItem { setup: family, id } => family == setup && item == Some(id.as_str()),
            Self::Other => false,
        }
    }
}

pub(super) fn pack(
    run: &Path,
    setup: Option<&str>,
    sessions: &[String],
) -> io::Result<Option<BTreeMap<String, Value>>> {
    pack_for(run, setup, sessions, None)
}

pub fn pack_queue_evidence(
    run: &Path,
    setup: Option<&str>,
    item: &str,
) -> io::Result<Option<BTreeMap<String, Value>>> {
    pack_for(run, setup, &[], Some(item))
}

fn pack_for(
    run: &Path,
    setup: Option<&str>,
    sessions: &[String],
    item: Option<&str>,
) -> io::Result<Option<BTreeMap<String, Value>>> {
    let Some(setup) = setup else {
        return Ok(None);
    };
    let root = run.join("owned-data");
    let _guard = OwnedLock::acquire(&root.join(".mutations.lock"), Duration::from_secs(3))?;
    let mut entries = BTreeMap::new();
    for (key, path) in records(&root.join("mutations"))? {
        let Some(object) = OWNED.read(&path).map_err(io::Error::other)? else {
            continue;
        };
        if address(&key, &object.fields)?.follows(setup, sessions, item) {
            entries.insert(key, OWNED.encode(object.fields));
        }
    }
    // Before the replay fence acquired a typed address, the stable submission
    // identified the same mutation record by its original digest.
    for (key, path) in records(&root.join("submissions"))? {
        let Some(object) = OWNED.read(&path).map_err(io::Error::other)? else {
            continue;
        };
        let Some(saved) = object.fields.get("submission") else {
            continue;
        };
        let Some(session) = saved["session"]
            .as_str()
            .filter(|id| sessions.iter().any(|session| session == id))
        else {
            continue;
        };
        let Some(handle) = saved["id"].as_str() else {
            return Err(invalid());
        };
        let [claimed_setup, origin, id] = handle
            .split('.')
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|_| invalid())?;
        if claimed_setup != setup {
            continue;
        }
        if saved["setup"] != setup
            || saved["origin"] != origin
            || !valid_key(origin)
            || !id_safe(id, 80)
            || key != digest(&["draft-submission", setup, session, handle])
        {
            return Err(invalid());
        }
        let shadow = digest(&["typed-draft", setup, session, id]);
        if entries.contains_key(&shadow) {
            continue;
        }
        if let Some(mut record) = OWNED
            .read(&root.join("mutations").join(format!("{shadow}.json")))
            .map_err(io::Error::other)?
            && record
                .fields
                .get("digest")
                .and_then(Value::as_str)
                .is_some_and(|value| value == format!("scoped:{key}"))
            && !record.fields.contains_key("draftReplayFence")
            && !record.fields.contains_key("answer")
        {
            record.fields.insert(
                "draftReplayFence".into(),
                serde_json::json!({"setup":setup,"session":session,"id":id}),
            );
            entries.insert(shadow, OWNED.encode(record.fields));
        }
    }
    Ok((!entries.is_empty()).then_some(entries))
}

pub(super) fn place(
    run: &Path,
    setup: Option<&str>,
    sessions: &[String],
    records: BTreeMap<String, Value>,
) -> io::Result<()> {
    place_for(run, setup, sessions, None, records)
}

pub fn place_queue_evidence(
    run: &Path,
    setup: Option<&str>,
    item: &str,
    records: BTreeMap<String, Value>,
) -> io::Result<()> {
    place_for(run, setup, &[], Some(item), records)
}

fn place_for(
    run: &Path,
    setup: Option<&str>,
    sessions: &[String],
    item: Option<&str>,
    records: BTreeMap<String, Value>,
) -> io::Result<()> {
    let setup = setup.ok_or_else(invalid)?;
    let root = run.join("owned-data");
    let _guard = OwnedLock::acquire(&root.join(".mutations.lock"), Duration::from_secs(3))?;
    let mut write = Vec::new();
    for (key, raw) in records {
        if !valid_key(&key) {
            return Err(invalid());
        }
        let incoming = OWNED.decode(raw).map_err(io::Error::other)?.fields;
        if !address(&key, &incoming)?.follows(setup, sessions, item) {
            return Err(io::Error::other(
                "The move includes a forwarded decision for another record.",
            ));
        }
        let path = root.join("mutations").join(format!("{key}.json"));
        if let Some(prior) = OWNED.read(&path).map_err(io::Error::other)? {
            if prior.fields == incoming {
                continue;
            }
            if identity(&prior.fields) == identity(&incoming)
                && (!prior.fields.contains_key("answer") || !incoming.contains_key("answer"))
            {
                if !incoming.contains_key("answer") {
                    continue;
                }
            } else if !(incoming.contains_key("draftReplayFence")
                && !prior.fields.contains_key("draftReplayFence")
                && prior.fields.get("digest") == incoming.get("digest")
                && !incoming.contains_key("answer")
                && !prior.fields.contains_key("answer"))
            {
                return Err(io::Error::other(
                    "A different forwarded decision already exists at the destination.",
                ));
            }
        }
        write.push((path, incoming));
    }
    for (path, fields) in write {
        files::write_atomic_through(&path, &serde_json::to_vec(&OWNED.encode(fields))?, run)?;
    }
    Ok(())
}

fn identity(row: &Map<String, Value>) -> Map<String, Value> {
    row.iter()
        .filter(|(field, _)| !matches!(field.as_str(), "answer" | "at"))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect()
}

fn address(key: &str, fields: &Map<String, Value>) -> io::Result<Scope> {
    let stored = fields
        .get("digest")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(value) = fields.get("draftReplayFence") {
        let fence: DraftFence = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
        if !setup_id(&fence.setup)
            || !fence
                .session
                .strip_prefix("ses_")
                .is_some_and(|id| id_safe(id, 95))
            || !id_safe(&fence.id, 80)
            || key != digest(&["typed-draft", &fence.setup, &fence.session, &fence.id])
            || !stored.strip_prefix("scoped:").is_some_and(valid_key)
            || fields.contains_key("answer")
        {
            return Err(invalid());
        }
        return Ok(Scope::Session {
            setup: fence.setup,
            id: fence.session,
        });
    }
    let Some(value) = fields.get("forward") else {
        return Ok(Scope::Other);
    };
    let forward: Forward = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    if !setup_id(&forward.setup)
        || !id_safe(&forward.operation, 80)
        || !valid_key(&forward.owner)
        || !forward.listed_by.as_deref().is_none_or(valid_key)
        || forward.area.is_empty()
        || forward.record.is_empty()
        || !match forward.submitter {
            Submitter::Computer => true,
            Submitter::Paired { key, pairing } => {
                valid_key(&key) && (pairing.is_empty() || id_safe(&pairing, 32))
            }
            Submitter::Ssh { key } => valid_key(&key),
        }
    {
        return Err(invalid());
    }
    let origin = forward.kind == "forwardedOperationOrigins";
    let family = if origin {
        "forward-origin"
    } else {
        "scoped-forward"
    };
    if key != digest(&[family, &forward.setup, &forward.operation]) {
        return Err(invalid());
    }
    if forward.reference.as_ref().is_some_and(|reference| {
        !matches!(
            reference.kind.as_str(),
            "queueItems"
                | "queueLabels"
                | "groupSettings"
                | "sessionQueue"
                | "sessionSchedules"
                | "setupNames"
                | "setupTimeZone"
                | "setupStarredModels"
                | "setupPersonalities"
                | "membershipAndAccess"
                | "clientIdentity"
                | "setupLeadership"
        ) || !reference.id.as_deref().is_none_or(|id| id_safe(id, 100))
            || !reference.queue.as_deref().is_none_or(|id| id_safe(id, 100))
    }) {
        return Err(invalid());
    }
    if origin {
        return Ok(Scope::Other);
    }
    let scope = forward.reference.as_ref().and_then(|reference| {
        declarations::data(&reference.kind)?
            .owner
            .owner(reference.queue.as_deref())
    });
    let policy = if forward
        .reference
        .as_ref()
        .is_some_and(|reference| reference.kind == "queueItems")
    {
        "forwardedQueueDecisions"
    } else {
        match scope {
            Some(Owner::Primary) => "forwardedPrimaryDecisions",
            Some(Owner::SessionComputer) => "forwardedSessionDecisions",
            Some(Owner::GroupHost) => "forwardedGroupDecisions",
            _ => "forwardedComputerDecisions",
        }
    };
    if forward.kind != policy || !valid_key(stored) {
        return Err(invalid());
    }
    if policy == "forwardedQueueDecisions"
        && let Some(id) = forward
            .reference
            .as_ref()
            .and_then(|reference| reference.id.clone())
    {
        return Ok(Scope::QueueItem {
            setup: forward.setup,
            id,
        });
    }
    if policy == "forwardedSessionDecisions"
        && let Some(id) = forward.reference.and_then(|reference| reference.id)
    {
        if !id_safe(&id, 100) {
            return Err(invalid());
        }
        return Ok(Scope::Session {
            setup: forward.setup,
            id,
        });
    }
    Ok(Scope::Other)
}

fn records(folder: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && let Some(key) = path
                .file_stem()
                .and_then(|key| key.to_str())
                .filter(|key| valid_key(key))
        {
            found.push((key.to_owned(), path));
        }
    }
    Ok(found)
}

fn digest(parts: &[&str]) -> String {
    encode(&Sha256::digest(
        serde_json::to_vec(parts).expect("string tuples serialize"),
    ))
}

fn valid_key(value: &str) -> bool {
    value.len() == 43 && id_safe(value, 43)
}
fn setup_id(value: &str) -> bool {
    value
        .strip_prefix("set_")
        .is_some_and(|id| id.len() == 22 && id_safe(id, 22))
}

fn id_safe(id: &str, max: usize) -> bool {
    (1..=max).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn invalid() -> io::Error {
    io::Error::other("Unreadable forwarded decisions in the move.")
}
