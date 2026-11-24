use std::{collections::BTreeMap, io, path::Path, time::Duration};

use kuvo_store::{
    files,
    locks::{OnBusy, StaleLock},
};
use kuvo_wire::keys::{decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::directory::ComputerKey;

#[derive(Deserialize, Serialize)]
pub(super) struct Home {
    key: String,
    at: Number,
}

impl Home {
    pub(super) fn newer_than(&self, at: i64) -> bool {
        self.at.as_f64().is_some_and(|stamp| stamp >= at as f64)
    }

    pub(super) fn key(&self) -> Option<ComputerKey> {
        decode_exact(&self.key)
    }
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
