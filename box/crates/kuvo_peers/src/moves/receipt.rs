//! Immutable destination decisions. Routing hints may expire while receipts still answer retries.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_store::{files, format::ObjectFormat, locks::OwnedLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    manifest::{QueueFile, SessionFile},
    move_id, read_json,
};
use crate::{directory::ComputerKey, reach::Millis};

const RECEIPT: ObjectFormat = ObjectFormat::additive("Move ownership receipt");
const HINT: ObjectFormat = ObjectFormat::additive("Move routing hint");

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Decided {
    Switched,
    Dropped,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Bundle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Payloads {
    pub sessions: Vec<SessionFile>,
    pub queue: Vec<QueueFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Decision {
    #[serde(rename = "as")]
    pub decided: Decided,
    pub at: Millis,
    #[serde(flatten)]
    pub bundle: Bundle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payloads: Option<Payloads>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("That isn’t a move ID")]
    Id,
    #[error("A move’s ownership decision cannot change")]
    Conflict,
    #[error("A move’s ownership receipt went missing")]
    Missing,
    #[error("A move’s ownership receipt could not be read")]
    Invalid(#[source] serde_json::Error),
    #[error(transparent)]
    Storage(#[from] io::Error),
    #[error(transparent)]
    Format(#[from] kuvo_store::format::Error),
}

pub struct Receipts<'a> {
    run: &'a Path,
}

impl<'a> Receipts<'a> {
    pub fn open(run: &'a Path) -> Self {
        Self { run }
    }

    pub(super) fn is_decided(&self, id: &str) -> Result<bool, Error> {
        Ok(valid(id) && self.read(id)?.is_some())
    }

    pub fn for_peer(&self, id: &str, peer: &str) -> Result<Option<Decision>, Error> {
        if !valid(id) {
            return Ok(None);
        }
        Ok(self
            .read(id)?
            .map(|(decision, _)| decision)
            .filter(|decision| decision.peer.as_deref().is_none_or(|key| peer == key)))
    }

    /// The sender's previous computer already knows it from the switch.
    pub(crate) fn introduced_by_move(
        &self,
        session: &str,
        peer: &ComputerKey,
        imported: &str,
    ) -> Result<bool, Error> {
        let Some((id, bundle)) = imported.split_once(':') else {
            return Ok(false);
        };
        let Some((generation, hash)) = bundle.split_once(':') else {
            return Ok(false);
        };
        let Ok(generation) = generation.parse() else {
            return Ok(false);
        };
        let peer = kuvo_wire::keys::encode(peer);
        let Some(receipt) = self.for_peer(id, &peer)? else {
            return Ok(false);
        };
        if receipt.decided != Decided::Switched
            || receipt.peer.as_deref() != Some(peer.as_str())
            || receipt.bundle.generation != Some(generation)
            || receipt.bundle.hash.as_deref() != Some(hash)
        {
            return Ok(false);
        }
        let hint = self
            .run
            .join("moves/decision-hints")
            .join(format!("{id}.json"));
        let Some(hint) = HINT.read(&hint)? else {
            return Ok(false);
        };
        Ok(hint
            .fields
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|sessions| sessions.first())
            .and_then(Value::as_str)
            == Some(session))
    }

    /// Seal an existing decision before answering a replay. A rename may have
    /// succeeded even if the earlier directory sync returned an error.
    pub fn seal(&self, id: &str) -> Result<(), Error> {
        let (_, file) = if valid(id) { self.read(id)? } else { None }.ok_or(Error::Missing)?;
        files::sync_file_through(&file, self.run)?;
        Ok(())
    }

    pub fn record(&self, id: &str, decision: &Decision, sessions: &[String]) -> Result<(), Error> {
        if !valid(id) {
            return Err(Error::Id);
        }
        let root = self.run.join("moves");
        let _lock = OwnedLock::acquire(
            &root.join("decisions").join(format!(".{id}.lock")),
            Duration::from_secs(2),
        )?;
        if let Some((before, path)) = self.read(id)? {
            if before.decided != decision.decided || before.bundle != decision.bundle {
                return Err(Error::Conflict);
            }
            files::sync_file_through(&path, self.run)?;
            return Ok(());
        }
        if !sessions.is_empty() {
            #[derive(Serialize)]
            struct Hint<'a> {
                #[serde(rename = "as")]
                decided: Decided,
                at: Millis,
                sessions: &'a [String],
            }

            let path = root.join("decision-hints").join(format!("{id}.json"));
            HINT.read(&path)?;
            self.save(
                &path,
                HINT,
                &Hint {
                    decided: decision.decided,
                    at: decision.at,
                    sessions,
                },
            )?;
        }
        self.save(&self.path(id), RECEIPT, decision)
    }

    fn read(&self, id: &str) -> Result<Option<(Decision, PathBuf)>, Error> {
        let path = self.path(id);
        if let Some(record) = RECEIPT.read(&path)? {
            return Ok(Some((decode(record)?, path)));
        }
        let legacy = self.run.join("moves/ended.json");
        match read_json(&legacy)? {
            Some(Value::Object(mut records)) => records
                .remove(id)
                .map(|value| Ok((decode(RECEIPT.decode(value)?)?, legacy)))
                .transpose(),
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Legacy move decisions could not be read",
            )
            .into()),
            None => Ok(None),
        }
    }

    fn save(
        &self,
        path: &Path,
        format: ObjectFormat,
        record: &impl Serialize,
    ) -> Result<(), Error> {
        let Value::Object(fields) = serde_json::to_value(record).map_err(Error::Invalid)? else {
            unreachable!("a receipt or hint serializes to an object");
        };
        let bytes = serde_json::to_vec(&format.encode(fields)).map_err(Error::Invalid)?;
        files::write_atomic_through(path, &bytes, self.run)?;
        Ok(())
    }

    fn path(&self, id: &str) -> PathBuf {
        self.run.join("moves/decisions").join(format!("{id}.json"))
    }
}

pub(super) fn dropped_here(
    run: &Path,
    now: Millis,
) -> Result<std::collections::BTreeSet<String>, Error> {
    let mut sessions = std::collections::BTreeSet::new();
    let collect = |record: kuvo_store::format::Object,
                   sessions: &mut std::collections::BTreeSet<String>| {
        if record.fields.get("as").and_then(Value::as_str) != Some("dropped") {
            return;
        }
        let Some(at) = record.fields.get("at").and_then(Value::as_i64) else {
            return;
        };
        if now.saturating_sub(at) >= super::ROUTING_FOR {
            return;
        }
        for id in record
            .fields
            .get("sessions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            sessions.insert(id.to_owned());
        }
    };
    let hints = run.join("moves/decision-hints");
    match std::fs::read_dir(&hints) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                if path.extension().is_some_and(|ext| ext == "json")
                    && let Some(record) = HINT.read(&path)?
                {
                    collect(record, &mut sessions);
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let legacy = run.join("moves/ended.json");
    if let Some(Value::Object(records)) = read_json(&legacy)? {
        for value in records.into_values() {
            if value.is_object() {
                collect(RECEIPT.decode(value)?, &mut sessions);
            }
        }
    }
    Ok(sessions)
}

fn decode(record: kuvo_store::format::Object) -> Result<Decision, Error> {
    serde_json::from_value(record.fields.into()).map_err(Error::Invalid)
}

fn valid(id: &str) -> bool {
    move_id(id).is_some_and(|tail| tail.len() <= 40)
}

/// Replay already committed decisions over the same authenticated peer-mail
/// exchange. A live activation must finish before it can acknowledge a switch.
pub(crate) fn replay(
    run: &Path,
    peer: &str,
    given: &Value,
    now: Millis,
) -> Result<Option<kuvo_wire::mail::Envelope>, Error> {
    use super::{Role, journals};
    use serde_json::json;

    let Some(op @ ("switch" | "decision?" | "drop")) = given.get("op").and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let Some(id) = given
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid(id))
    else {
        return Ok(None);
    };

    if let Some(record) = journals(run)?.into_iter().find(|record| record.id == id) {
        if record.peer != peer
            || record.activation.is_some()
            || (op != "switch" && !record.bundle.matches(given))
        {
            return Ok(None);
        }
        // Replaying a drop must not take a copy back. If a journal claims a
        // live copy, the transfer actor must perform that step.
        if op == "drop" && record.role == Role::To && !record.dropping {
            return Ok(None);
        }
    }

    let receipts = Receipts::open(run);
    let Some(decision) = receipts
        .for_peer(id, peer)?
        .filter(|decision| decision.bundle.matches(given))
    else {
        return Ok(None);
    };
    receipts.seal(id)?;
    let mut movement = serde_json::to_value(&decision.bundle).map_err(Error::Invalid)?;
    movement["id"] = json!(id);
    match decision.decided {
        Decided::Switched => {
            movement["op"] = json!("switched");
            if decision.payloads.is_some() {
                movement["verifiedPayloads"] = json!(true);
            }
        }
        Decided::Dropped => {
            movement["op"] = json!("dropped");
            if decision.peer.as_ref().is_some_and(|peer| !peer.is_empty()) {
                movement["neverSwitched"] = json!(true);
            }
        }
    }
    Ok(Some(kuvo_wire::mail::Envelope(serde_json::Map::from_iter(
        [("at".into(), json!(now)), ("move".into(), movement)],
    ))))
}

impl Bundle {
    pub(super) fn matches(&self, given: &Value) -> bool {
        let generation_matches = match (self.generation, given.get("generation")) {
            (None, None) => true,
            (Some(generation), Some(given)) => given.as_f64() == Some(generation as f64),
            _ => false,
        };
        let hash = given.get("hash");
        generation_matches
            && ((self.generation.is_none() && hash.is_none())
                || match (self.hash.as_deref(), hash) {
                    (None, None) => true,
                    (Some(expected), Some(given)) => given.as_str() == Some(expected),
                    _ => false,
                })
    }
}
