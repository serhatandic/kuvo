//! Serialize engine note changes with legacy writers on the same computer.

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

use http::{Method, StatusCode};
use kuvo_server::Refusal;
use kuvo_store::locks::{OnBusy, StaleLock};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Engine, api, lifetime::Lease, session_move::PromptWrite, sessions, start_session::Permission,
    turns::Turn,
};

const NOTE_STALE: Duration = Duration::from_secs(65);
const OLD_STALE: Duration = Duration::from_secs(120);
const OLD_WAIT: Duration = Duration::from_secs(30);

pub(crate) struct NoteTurn {
    // Release the compatible lock before handing off the resident's turn.
    _lock: Option<StaleLock>,
    _turn: Turn,
}

#[derive(Default)]
pub struct NoteEdits {
    /// Keys owned by this writer, with None to remove one.
    pub kuvo: BTreeMap<String, Option<Value>>,
    pub top: BTreeMap<String, Option<Value>>,
    pub permissions: Option<Vec<Permission>>,
}

impl NoteEdits {
    pub(crate) fn change(self, metadata: &mut Map<String, Value>) -> Option<Value> {
        let before = metadata.clone();
        for (key, value) in self.top {
            replace(metadata, key, value);
        }
        if !self.kuvo.is_empty() {
            let own = metadata.entry("kuvo").or_insert_with(|| json!({}));
            if !own.is_object() {
                *own = json!({});
            }
            let own = own.as_object_mut().expect("Kuvo's notes are an object");
            for (key, value) in self.kuvo {
                replace(own, key, value);
            }
        }
        let changed = *metadata != before;
        (changed || self.permissions.is_some()).then(|| {
            let mut patch = json!({});
            if changed {
                patch["metadata"] = json!(metadata);
            }
            if let Some(permissions) = self.permissions {
                patch["permissions"] = json!(permissions);
            }
            patch
        })
    }
}

fn replace(map: &mut Map<String, Value>, key: String, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key, value);
    } else {
        map.remove(&key);
    }
}

/// Owns the note turn while one or more edits to the same session are applied.
pub struct NoteWriter<'a> {
    engine: &'a Engine,
    lease: Lease,
    route: String,
    _write: PromptWrite,
    _turn: NoteTurn,
}

impl NoteWriter<'_> {
    /// The session as the engine has it now, read in this turn.
    pub async fn current(&self) -> Result<Value, Refusal> {
        sessions::read(self.engine, &self.lease, &self.route).await
    }

    pub async fn edit(&mut self, edits: NoteEdits) -> Result<bool, Refusal> {
        let current = sessions::read(self.engine, &self.lease, &self.route).await?;
        let mut metadata = current
            .get("metadata")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let Some(patch) = edits.change(&mut metadata) else {
            return Ok(false);
        };
        self.engine
            .call::<Value>(&self.lease, Method::PATCH, &self.route, Some(&patch))
            .await
            .map_err(|error| Refusal::new(StatusCode::BAD_GATEWAY, error.to_string()))?;
        Ok(true)
    }
}

impl Engine {
    pub async fn note_writer(&self, session: &str) -> Result<NoteWriter<'_>, Refusal> {
        if !sessions::valid_id(session) {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "Choose a conversation.",
            ));
        }
        let turn = self.note_turn(session).await;
        let write = crate::session_move::admit(self, session, None).await?;
        let lease = api::start(self).await?;
        Ok(NoteWriter {
            engine: self,
            lease,
            route: sessions::path(session),
            _write: write,
            _turn: turn,
        })
    }

    /// Kuvo's notes on a conversation changed wherever it's kept: with the coding agents, else on
    /// the engine. False when nothing changed.
    pub(crate) async fn edit_notes(&self, session: &str, edits: NoteEdits) -> Result<bool, Refusal> {
        if self.agent_sessions.is_some() {
            let _turn = self.note_turn(session).await;
            let _write = crate::session_move::admit(self, session, None).await?;
            if let Some((source, header)) = self.agent_session(session).await? {
                let mut metadata = header
                    .get("metadata")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let Some(mut patch) = edits.change(&mut metadata) else {
                    return Ok(false);
                };
                let patch = crate::AgentSessionPatch {
                    title: None,
                    permissions: patch.get_mut("permissions").map(Value::take),
                    metadata: patch
                        .get_mut("metadata")
                        .and_then(|metadata| metadata.as_object_mut().map(std::mem::take)),
                };
                let saved = source.update(session, patch).await.map_err(|error| {
                    Refusal::new(StatusCode::SERVICE_UNAVAILABLE, error.to_string())
                })?;
                return saved.map(|_| true).ok_or_else(|| {
                    Refusal::new(StatusCode::NOT_FOUND, "That conversation isn't here.")
                });
            }
        }
        self.note_writer(session).await?.edit(edits).await
    }

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
