//! Stored move state shared with the TypeScript resident.

pub mod archive;
mod attachment_paths;
mod background;
mod driver;
mod forward_evidence;
mod handoff;
mod left;
mod listing;
pub mod manifest;
mod notices;
pub mod receipt;
mod session_files;
mod session_notes;
pub mod transfer;
mod wake_archive;
mod wake_folder;

use std::{collections::BTreeSet, io, path::Path};

use kuvo_store::{files, format::ObjectFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    directory::{ComputerKey, MoveHint, MoveLocation, Ownership},
    lists::matching_paths,
    reach::Millis,
};

pub(crate) use driver::Handle;
pub(crate) use driver::start;
pub(crate) use listing::{is_mac, plain_name, register, register_start};
pub(crate) use notices::{Announcer, Notices};

const JOURNAL: ObjectFormat = ObjectFormat::additive("Move journal");
const DEPARTURE: ObjectFormat = ObjectFormat::additive("Moved conversation");
pub(crate) const ROUTING_FOR: Millis = 2 * 24 * 3_600_000;

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    sent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting: Option<bool>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Journal {
    // Older kept ownership fixtures can lack presentation fields.
    #[serde(default)]
    id: String,
    thing: String,
    peer: String,
    role: Role,
    stage: Stage,
    sessions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<String>,
    started: Millis,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    dropping: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    complete: bool,
    #[serde(rename = "cleanupPending", skip_serializing_if = "Option::is_none")]
    cleanup_pending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<Kind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation: Option<Activation>,
    #[serde(rename = "setUpSince", skip_serializing_if = "Option::is_none")]
    set_up_since: Option<Millis>,
    #[serde(rename = "setUpAt", skip_serializing_if = "Option::is_none")]
    set_up_at: Option<Millis>,
    #[serde(
        rename = "verifyPayloads",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    verify_payloads: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    payloads: Option<receipt::Payloads>,
    #[serde(flatten)]
    bundle: receipt::Bundle,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Activation {
    at: Millis,
    #[serde(skip_serializing_if = "Option::is_none")]
    last: Option<String>,
}

impl Journal {
    fn read(run: &Path, id: &str) -> io::Result<Option<Self>> {
        if move_id(id).is_none() {
            return Ok(None);
        }
        let Some(stored) = JOURNAL
            .read(&journal_file(run, id))
            .map_err(io::Error::other)?
        else {
            return Ok(None);
        };
        let record: Self = serde_json::from_value(stored.fields.into())?;
        Ok((record.id == id).then_some(record))
    }

    fn save(&self, run: &Path) -> io::Result<()> {
        let path = journal_file(run, &self.id);
        JOURNAL.read(&path).map_err(io::Error::other)?;
        let fields = serde_json::to_value(self)?;
        let Value::Object(fields) = fields else {
            unreachable!("move journal serializes to an object");
        };
        let data = serde_json::to_vec(&JOURNAL.encode(fields))?;
        files::write_atomic_through(&path, &data, run)
    }

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

    fn shown_waiting(&self, waiting: &BTreeSet<String>) -> Option<Shown> {
        let mut movement = self.shown()?;
        movement.waiting = waiting.contains(&self.id).then_some(true);
        Some(movement)
    }
}

fn journal_file(run: &Path, id: &str) -> std::path::PathBuf {
    run.join("moves").join(format!("{id}.json"))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    arrived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
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

pub(crate) fn recent_home_hints(
    run: &Path,
    now: Millis,
    memory: Millis,
) -> impl Iterator<Item = MoveHint> {
    departure_rows(run)
        .filter_map(Result::ok)
        .filter(move |row| row.queue.is_none() && now.saturating_sub(row.at) < memory)
        .map(|row| MoveHint {
            session: row.session,
            key: row.key,
            at: row.at,
        })
}

pub(crate) fn recently_arrived(
    run: &Path,
    session: &str,
    here: &str,
    now: Millis,
) -> Result<bool, kuvo_store::format::Error> {
    Ok(departures(run)?.into_iter().any(|row| {
        row.session == session
            && row.key == here
            && row.arrived == Some(true)
            && now.saturating_sub(row.at) < ROUTING_FOR
    }))
}

pub(crate) fn locate(run: &Path, ownership: &mut Ownership) {
    apply_owners(
        ownership,
        departure_rows(run).filter_map(Result::ok),
        journal_rows(run).filter_map(Result::ok).collect(),
    );
}

/// A new owner can speak for a switched root before it has published it.
pub(crate) fn switched_sender(run: &Path, session: &str, key: &ComputerKey) -> Option<String> {
    let peer = kuvo_wire::keys::encode(key);
    journals(run)
        .ok()?
        .into_iter()
        .filter(|row| {
            row.live()
                && row.role == Role::From
                && row.stage.switched()
                && row.peer == peer
                && matches!(row.kind, Some(Kind::Chat | Kind::Bot))
                && row.sessions.first().is_some_and(|root| root == session)
        })
        .min_by_key(|row| row.started)
        .and_then(|row| row.name)
}

/// Admission refuses unreadable durable state. The display projection still
/// lists the known records while a newer resident's record is on disk.
pub(crate) fn locate_checked(run: &Path, ownership: &mut Ownership) -> io::Result<()> {
    let folder = run.join("moves");
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut active = Vec::new();
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .is_some_and(|id| move_id(id).is_some())
        {
            continue;
        }
        let Some(row) = JOURNAL.read(&entry.path()).map_err(io::Error::other)? else {
            continue;
        };
        active.push(serde_json::from_value(row.fields.into())?);
    }
    let mut moved = Vec::new();
    if let Some(rows) = read_json(&folder.join("moved.json"))? {
        let Value::Array(rows) = rows else {
            return Err(io::Error::other("The move departure records aren’t a list"));
        };
        for row in rows {
            let fields = DEPARTURE.decode(row).map_err(io::Error::other)?.fields;
            moved.push(serde_json::from_value(fields.into())?);
        }
    }
    apply_owners(ownership, moved, active);
    Ok(())
}

fn apply_owners(
    ownership: &mut Ownership,
    departures: impl IntoIterator<Item = Departure>,
    mut active: Vec<Journal>,
) {
    let record = crate::lists::record;
    for departure in departures {
        if departure.queue.is_none()
            && let Some(key) = kuvo_wire::keys::decode_exact(&departure.key)
        {
            ownership
                .journal
                .entry(record(&departure.session))
                .or_insert(MoveLocation {
                    key,
                    departed_at: Some(departure.at),
                    retiring: false,
                });
        }
    }
    let mut switching = BTreeSet::new();
    // Keep the known ownership fences even if a different move was written by
    // a newer resident. Client reads and proof replies still check all versions.
    active.sort_by_key(|record| record.started);
    for movement in active.into_iter().filter(Journal::live) {
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
                ownership.journal.insert(
                    record(&id),
                    MoveLocation {
                        key: destination,
                        departed_at: None,
                        retiring: movement.stage == Stage::Finishing,
                    },
                );
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
