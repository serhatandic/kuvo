//! Stored move state shared with the TypeScript resident.

mod listing;
pub mod manifest;
mod notices;
pub mod receipt;

use std::{collections::BTreeSet, io, path::Path};

use kuvo_store::{files, format::ObjectFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{directory::Ownership, lists::matching_paths, reach::Millis};

pub(crate) use listing::register;
pub(crate) use notices::{Announcer, Notices};

const JOURNAL: ObjectFormat = ObjectFormat::additive("Move journal");
const DEPARTURE: ObjectFormat = ObjectFormat::additive("Moved conversation");
const ROUTING_FOR: Millis = 2 * 24 * 3600_000;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Role {
    From,
    To,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Stage {
    Holding,
    Copying,
    Switching,
    Finishing,
}

impl Stage {
    fn switched(self) -> bool {
        matches!(self, Self::Switching | Self::Finishing)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Chat,
    Bot,
    Group,
}

#[derive(Clone, Deserialize, Serialize)]
struct Shown {
    id: String,
    kind: Kind,
    thing: String,
    name: String,
    from: String,
    to: String,
    role: Role,
    stage: Stage,
    sessions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting: Option<bool>,
}

#[derive(Deserialize)]
struct Journal {
    // Older kept ownership fixtures can lack presentation fields.
    #[serde(default)]
    id: String,
    thing: String,
    peer: String,
    role: Role,
    stage: Stage,
    sessions: Vec<String>,
    step: Option<String>,
    started: Millis,
    failed: Option<String>,
    #[serde(default)]
    dropping: bool,
    #[serde(rename = "cleanupPending")]
    cleanup_pending: Option<bool>,
    note: Option<String>,
    kind: Option<Kind>,
    name: Option<String>,
    from: Option<String>,
    to: Option<String>,
    sent: Option<f64>,
    size: Option<f64>,
    activation: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    bundle: receipt::Bundle,
}

impl Journal {
    fn live(&self) -> bool {
        self.failed.as_deref().is_none_or(str::is_empty)
    }

    fn listed(&self) -> bool {
        self.live()
            && !(self.role == Role::To && self.dropping
                || self.role == Role::From && self.stage == Stage::Finishing)
    }

    fn away(&self) -> bool {
        self.live() && (self.role == Role::From) == self.stage.switched()
    }

    fn shown(&self) -> Option<Shown> {
        Some(Shown {
            id: self.id.clone(),
            kind: self.kind?,
            thing: self.thing.clone(),
            name: self.name.clone()?,
            from: self.from.clone()?,
            to: self.to.clone()?,
            role: self.role,
            stage: self.stage,
            sessions: self.sessions.clone(),
            sent: self.sent,
            size: self.size,
            waiting: None,
        })
    }
}

fn journal_rows(run: &Path) -> impl Iterator<Item = Result<Journal, kuvo_store::format::Error>> {
    matching_paths(&run.join("moves"), "mov_", Some(".json")).filter_map(|(_, path)| match JOURNAL
        .read(&path)
    {
        Ok(Some(object)) => serde_json::from_value::<Journal>(object.fields.into())
            .ok()
            .map(Ok),
        Err(error @ kuvo_store::format::Error::Unsupported { .. }) => Some(Err(error)),
        _ => None,
    })
}

fn journals(run: &Path) -> Result<Vec<Journal>, kuvo_store::format::Error> {
    let mut records = journal_rows(run).collect::<Result<Vec<_>, _>>()?;
    records.sort_by_key(|record| record.started);
    Ok(records)
}

#[derive(Deserialize, Serialize)]
struct Departure {
    session: String,
    key: String,
    at: Millis,
    #[serde(skip_serializing)]
    arrived: Option<bool>,
    #[serde(skip_serializing)]
    queue: Option<String>,
}

fn departure_rows(
    run: &Path,
) -> impl Iterator<Item = Result<Departure, kuvo_store::format::Error>> {
    let entries =
        crate::kept::read::<Vec<Value>>(&run.join("moves/moved.json")).unwrap_or_default();
    entries
        .into_iter()
        .filter(Value::is_object)
        .filter_map(|entry| match DEPARTURE.decode(entry) {
            Ok(object) => serde_json::from_value(object.fields.into()).ok().map(Ok),
            Err(error) => Some(Err(error)),
        })
}

fn departures(run: &Path) -> Result<Vec<Departure>, kuvo_store::format::Error> {
    departure_rows(run).collect()
}

pub(crate) fn locate(run: &Path, ownership: &mut Ownership) {
    let record = crate::lists::record;
    for departure in departure_rows(run).filter_map(Result::ok) {
        if departure.queue.is_none()
            && let Some(key) = kuvo_wire::keys::decode_exact(&departure.key)
        {
            ownership
                .journal
                .entry(record(&departure.session))
                .or_insert(key);
        }
    }
    let mut switching = BTreeSet::new();
    // Keep the known ownership fences even if a different move was written by
    // a newer resident. Client reads and proof replies still check all versions.
    let mut active: Vec<_> = journal_rows(run)
        .filter_map(Result::ok)
        .filter(Journal::live)
        .collect();
    active.sort_by_key(|record| record.started);
    for movement in active {
        let destination = if movement.stage.switched() && movement.role == Role::From {
            kuvo_wire::keys::decode_exact(&movement.peer)
        } else {
            None
        };
        for id in std::iter::once(movement.thing).chain(movement.sessions) {
            if movement.step.as_deref() != Some("check") {
                ownership.frozen.insert(id.clone());
            }
            if let Some(destination) = destination
                && switching.insert(id.clone())
            {
                ownership.journal.insert(record(&id), destination);
            }
        }
    }
}

fn read_json(path: &Path) -> io::Result<Option<Value>> {
    files::read(path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::from))
        .transpose()
}

fn move_id(id: &str) -> Option<&str> {
    id.strip_prefix("mov_")
        .filter(|tail| !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// Backup needs the complete local ownership fences, including older departures
/// that no longer appear in the list shown to other computers.
#[derive(PartialEq, Eq)]
pub struct BackupFences {
    frozen: BTreeSet<String>,
    owners: std::collections::BTreeMap<String, String>,
}

impl BackupFences {
    pub fn read(run: &Path) -> Result<Self, kuvo_store::format::Error> {
        let mut frozen = BTreeSet::new();
        for movement in journals(run)?.into_iter().filter(Journal::live) {
            if movement.step.as_deref() != Some("check") {
                frozen.insert(movement.thing);
                frozen.extend(movement.sessions);
            }
        }
        let mut owners = std::collections::BTreeMap::new();
        for record in departures(run)? {
            if record.queue.is_none() {
                owners.entry(record.session).or_insert(record.key);
            }
        }
        Ok(Self { frozen, owners })
    }

    pub fn permits(&self, id: &str, local: impl FnOnce(&str) -> bool) -> bool {
        !self.frozen.contains(id) && self.owners.get(id).is_none_or(|owner| local(owner))
    }

    pub fn owner(&self, id: &str) -> Option<&str> {
        self.owners.get(id).map(String::as_str)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.frozen
            .iter()
            .chain(self.owners.keys())
            .map(String::as_str)
    }
}
