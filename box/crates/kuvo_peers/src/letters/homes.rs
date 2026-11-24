use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    time::Duration,
};

use kuvo_store::{
    files,
    locks::{OnBusy, StaleLock},
};
use kuvo_wire::keys::{decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::{directory::ComputerKey, outbox::NOTE_MEMORY, reach::Millis, setup_lists::View};

#[derive(Deserialize, Serialize)]
pub(super) struct Home {
    key: String,
    at: Number,
}

impl Home {
    fn timestamp(&self) -> f64 {
        self.at.as_f64().unwrap_or(f64::NEG_INFINITY)
    }

    pub(super) fn newer_than(&self, at: i64) -> bool {
        self.at.as_f64().is_some_and(|stamp| stamp >= at as f64)
    }

    pub(super) fn key(&self) -> Option<ComputerKey> {
        decode_exact(&self.key)
    }
}

/// Retain moves for delayed notes, after the live announcements expire.
pub(super) fn learn_moves(run: &Path, view: &View, now: Millis) -> io::Result<()> {
    let work = run.join("work");
    let file = work.join("homes.json");
    let peer_hints = view
        .roster
        .members
        .iter()
        .filter_map(|peer| view.announced.get(&encode(&peer.key)))
        .flat_map(|said| &said.moved)
        .filter(|hint| now.saturating_sub(hint.at) < crate::moves::ROUTING_FOR)
        .cloned();
    let mut newest = BTreeMap::<String, Home>::new();
    for hint in crate::moves::recent_home_hints(run, now, NOTE_MEMORY as Millis).chain(peer_hints) {
        if newest
            .get(&hint.session)
            .is_none_or(|old| !old.newer_than(hint.at))
        {
            newest.insert(
                hint.session,
                Home {
                    key: hint.key,
                    at: hint.at.into(),
                },
            );
        }
    }
    if newest.is_empty() && !file.try_exists()? {
        return Ok(());
    }
    let local: BTreeSet<_> = view
        .announced_agents()
        .into_iter()
        .map(|agent| agent.session)
        .collect();
    let here = view.roster.here.as_ref().map(|key| encode(key));
    let _guard = StaleLock::acquire(
        &work.join(".homes.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    let mut homes = read(run)?;
    let mut changed = false;
    for (session, home) in newest {
        if homes
            .get(&session)
            .is_none_or(|old| old.timestamp() < home.timestamp())
            && !local.contains(&session)
        {
            if Some(&home.key) != here.as_ref() {
                homes.insert(session, home);
                changed = true;
            } else {
                changed |= homes.remove(&session).is_some();
            }
        }
    }
    let before = homes.len();
    homes.retain(|session, home| !local.contains(session) && Some(&home.key) != here.as_ref());
    if changed || before != homes.len() {
        files::write_secret(&file, &serde_json::to_vec(&homes)?)?;
    }
    Ok(())
}

pub(super) fn read(run: &Path) -> io::Result<BTreeMap<String, Home>> {
    let Some(bytes) = files::read(&run.join("work/homes.json"))? else {
        return Ok(BTreeMap::new());
    };
    let stored: BTreeMap<String, Value> = serde_json::from_slice(&bytes).unwrap_or_default();
    Ok(stored
        .into_iter()
        .filter_map(|(session, home)| Some((session, serde_json::from_value(home).ok()?)))
        .collect())
}

pub(crate) fn remember<'a>(
    run: &Path,
    sessions: impl IntoIterator<Item = &'a str>,
    key: &ComputerKey,
    at: u64,
) -> io::Result<()> {
    let mut sessions = sessions.into_iter().peekable();
    if sessions.peek().is_none() {
        return Ok(());
    }
    let work = run.join("work");
    let _guard = StaleLock::acquire(
        &work.join(".homes.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    let mut homes = read(run)?;
    let mut changed = false;
    for session in sessions {
        if homes
            .get(session)
            .and_then(|old| old.at.as_f64())
            .is_none_or(|old| at as f64 >= old)
        {
            homes.insert(
                session.into(),
                Home {
                    key: encode(key),
                    at: at.into(),
                },
            );
            changed = true;
        }
    }
    if changed {
        files::write_secret(&work.join("homes.json"), &serde_json::to_vec(&homes)?)?;
    }
    Ok(())
}
