//! A source's departure and a destination's activation survive either
//! resident stopping between journal, ownership, and receipt writes.

use std::{collections::BTreeSet, io, path::Path, time::Duration};

use kuvo_store::{
    files,
    locks::{OnBusy, StaleLock},
};
use serde_json::Value;

use super::{
    Activation, DEPARTURE, Departure, Journal, Role, Stage, departures, journal_file, journals,
    receipt::{Decided, Decision, Receipts},
};
use crate::reach::Millis;

pub(super) fn begin_activation(
    run: &Path,
    record: &mut Journal,
    at: Millis,
    last: Option<String>,
) -> io::Result<bool> {
    if record.role != Role::To
        || record.dropping
        || !record.complete
        || record.set_up_since.is_some()
        || record.sessions.is_empty()
    {
        return Ok(false);
    }
    if record.activation.is_none() {
        record.activation = Some(Activation { at, last });
        record.save(run)?;
    }
    Ok(true)
}

/// Claim the routed ownership without lifting the prompt fence. Queue
/// restoration still runs under the activation journal after this write.
pub(super) fn claim_destination(run: &Path, record: &Journal, here: &str) -> io::Result<()> {
    let activation = record.activation.as_ref().ok_or_else(no_activation)?;
    if record.to.as_deref() != Some(here)
        || journals(run)
            .map_err(io::Error::other)?
            .iter()
            .any(|other| {
                other.id != record.id
                    && other.live()
                    && other.step.as_deref() != Some("check")
                    && (record.covers(&other.thing)
                        || other.sessions.iter().any(|session| record.covers(session)))
            })
    {
        return Err(io::Error::other("Waiting for the accepted move ownership."));
    }
    note_owners(run, record, here, activation.at, true)
}

/// Called after held records and queues are safe to run. A lost wire reply
/// can be recovered from the immutable receipt without reopening the bundle.
pub(super) fn commit_destination(run: &Path, record: &Journal) -> io::Result<()> {
    let activation = record.activation.as_ref().ok_or_else(no_activation)?;
    let payloads = if record.verify_payloads {
        Some(
            record
                .payloads
                .clone()
                .ok_or_else(|| io::Error::other("The move’s file inventory is unavailable."))?,
        )
    } else {
        None
    };
    Receipts::open(run)
        .record(
            &record.id,
            &Decision {
                decided: Decided::Switched,
                at: activation.at,
                last: activation.last.clone(),
                peer: Some(record.peer.clone()),
                bundle: record.bundle.clone(),
                payloads,
            },
            &record.sessions,
        )
        .map_err(io::Error::other)
}

pub(super) fn retire_source(run: &Path, record: &mut Journal, at: Millis) -> io::Result<()> {
    if record.role != Role::From || !record.stage.switched() {
        return Err(io::Error::other("Ownership has not been switched"));
    }
    if record.stage != Stage::Finishing {
        record.stage = Stage::Finishing;
        record.step = Some("done".into());
        record.save(run)?;
    }
    note_owners(run, record, &record.peer, at, false)
}

pub(super) fn remove_completed(run: &Path, record: &Journal) -> io::Result<()> {
    // Remove the staged bytes first. A restart can proceed from the receipt
    // even if durable retirement of the move journal itself was interrupted.
    files::remove_tree_synced(&run.join("moves").join(&record.id))?;
    files::remove_file_synced(&journal_file(run, &record.id))
}

fn note_owners(
    run: &Path,
    record: &Journal,
    key: &str,
    at: Millis,
    arrived: bool,
) -> io::Result<()> {
    write_owners(
        run,
        std::iter::once(&record.thing)
            .chain(record.sessions.iter())
            .map(String::as_str),
        key,
        at,
        arrived,
        None,
    )
}

/// Keep the owning queue of an item that has switched computers.
pub fn note_queue_placement(
    run: &Path,
    item: &str,
    queue: &str,
    key: &str,
    at: Millis,
    arrived: bool,
) -> io::Result<()> {
    write_owners(run, [item], key, at, arrived, Some(queue))
}

fn write_owners<'a>(
    run: &Path,
    ids: impl IntoIterator<Item = &'a str>,
    key: &str,
    at: Millis,
    arrived: bool,
    queue: Option<&str>,
) -> io::Result<()> {
    let root = run.join("moves");
    let _lock = StaleLock::acquire(
        &root.join(".departures.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    let arrived = arrived.then_some(true);
    let mut rows = departures(run).map_err(io::Error::other)?;
    let mut changed = BTreeSet::new();
    let mut add = Vec::new();
    for id in ids {
        if !rows.iter().any(|row| {
            row.session == id
                && row.key == key
                && row.arrived == arrived
                && row.queue.as_deref() == queue
        }) && changed.insert(id)
        {
            add.push(Departure {
                session: id.into(),
                key: key.into(),
                at,
                arrived,
                queue: queue.map(str::to_owned),
            });
        }
    }
    if add.is_empty() {
        return Ok(());
    }
    rows.retain(|row| !changed.contains(row.session.as_str()));
    rows.extend(add);
    let encoded = rows
        .into_iter()
        .map(|row| {
            let Value::Object(fields) = serde_json::to_value(row)? else {
                unreachable!("a departure serializes to an object");
            };
            Ok(DEPARTURE.encode(fields))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    files::write_atomic_through(
        &root.join("moved.json"),
        &serde_json::to_vec(&encoded)?,
        run,
    )
}

fn no_activation() -> io::Error {
    io::Error::other("The move has no durable activation")
}

impl Journal {
    pub(super) fn covers(&self, id: &str) -> bool {
        self.thing == id || self.sessions.iter().any(|session| session == id)
    }
}
