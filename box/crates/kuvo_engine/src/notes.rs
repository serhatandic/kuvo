//! Serialize engine note changes with legacy writers on the same computer.

use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

use kuvo_store::locks::{OnBusy, StaleLock};
use sha2::{Digest, Sha256};

use crate::{Engine, turns::Turn};

const NOTE_STALE: Duration = Duration::from_secs(65);
const OLD_STALE: Duration = Duration::from_secs(120);
const OLD_WAIT: Duration = Duration::from_secs(30);

pub(crate) struct NoteTurn {
    // Release the compatible lock before handing off the resident's turn.
    _lock: Option<StaleLock>,
    _turn: Turn,
}

impl Engine {
    pub(crate) async fn note_turn(&self, session: &str) -> NoteTurn {
        let turn = self.turn(session).await;
        let run = self.home.join("run");
        let old = run.join("work/.notes.lock");
        let until = tokio::time::Instant::now() + OLD_WAIT;
        while tokio::time::Instant::now() < until
            && tokio::fs::metadata(&old)
                .await
                .is_ok_and(|metadata| !stale(&metadata, OLD_STALE))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let digest = hex::encode(Sha256::digest(session.as_bytes()));
        let path = run.join("notes").join(format!("{}.lock", &digest[..32]));
        let lock = tokio::task::spawn_blocking(move || {
            StaleLock::acquire(&path, NOTE_STALE, NOTE_STALE, OnBusy::Proceed)
        })
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
        NoteTurn {
            _lock: lock,
            _turn: turn,
        }
    }
}

pub(crate) fn clear_stale(home: &Path) {
    let Ok(locks) = fs::read_dir(home.join("run/notes")) else {
        return;
    };
    for entry in locks.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "lock")
            && entry
                .metadata()
                .is_ok_and(|metadata| metadata.is_dir() && stale(&metadata, NOTE_STALE))
        {
            let _ = fs::remove_dir(path);
        }
    }
}

fn stale(metadata: &fs::Metadata, after: Duration) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > after)
}
