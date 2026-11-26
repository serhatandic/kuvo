//! Snapshot marks live in agent.json beside native conversation slots; the
//! trees and ignored paths live in the shared repository's own object store.

use super::{SessionStore, TRANSCRIPT, open_transcript, read_agent, write_messages};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Deserialize, Serialize)]
struct Note {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    failed: bool,
}

fn notes(agent: &Map<String, Value>) -> BTreeMap<String, Note> {
    agent
        .get("snapshots")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(id, value)| {
            if let Some(before) = value.as_str() {
                return Some((
                    id.clone(),
                    Note {
                        before: Some(before.into()),
                        failed: true,
                        ..Note::default()
                    },
                ));
            }
            serde_json::from_value(value.clone())
                .ok()
                .map(|note| (id.clone(), note))
        })
        .collect()
}

impl SessionStore {
    async fn snapshot_agent(&self, id: &str) -> io::Result<Map<String, Value>> {
        let folder = self.folder(id).ok_or(ErrorKind::NotFound)?;
        tokio::task::spawn_blocking(move || read_agent(&folder)).await?
    }

    async fn snapshot_edit(
        &self,
        id: &str,
        edit: impl FnOnce(&mut Map<String, Value>) + Send + 'static,
    ) -> io::Result<()> {
        let saved = self
            .mutate_existing(id, move |folder| {
                let mut agent = read_agent(folder)?;
                edit(&mut agent);
                files::write_atomic(&folder.join("agent.json"), &serde_json::to_vec(&agent)?)
            })
            .await?;
        if saved {
            Ok(())
        } else {
            Err(ErrorKind::NotFound.into())
        }
    }

    /// A snapshot failure is recorded for Undo; it does not stop the agent's work.
    pub(crate) async fn snapshot_start(
        &self,
        id: &str,
        turn: &str,
        directory: &Path,
    ) -> io::Result<bool> {
        let at = crate::turn_projection::now();
        let note = match self.snapshots.take(directory).await {
            Ok(Some(before)) => Note {
                before: Some(before),
                at: Some(at),
                ..Note::default()
            },
            Ok(None) => return Ok(false),
            Err(error) => {
                eprintln!("Kuvo could not keep repository files before a message: {error}");
                Note {
                    at: Some(at),
                    failed: true,
                    ..Note::default()
                }
            }
        };
        let turn = turn.to_owned();
        self.snapshot_edit(id, move |agent| {
            let mut all = notes(agent);
            all.insert(turn, note);
            agent.insert("snapshots".into(), json!(all));
        })
        .await
        .map(|_| true)
    }

    /// Finish the notes of this turn before the idle row is published.
    pub(crate) async fn snapshot_finish(&self, id: &str, turn: &str) -> io::Result<()> {
        let Some(mut note) = notes(&self.snapshot_agent(id).await?).remove(turn) else {
            return Ok(());
        };
        if note.failed || note.files.is_some() {
            return Ok(());
        }
        let Some(before) = note.before.as_deref() else {
            return Ok(());
        };
        let directory = self
            .read(id)
            .await?
            .and_then(|header| header.directory().map(PathBuf::from))
            .ok_or(ErrorKind::NotFound)?;
        let after = async {
            let after = self
                .snapshots
                .take(&directory)
                .await?
                .ok_or_else(|| io::Error::other("The repository is no longer here."))?;
            self.snapshots.changed(&directory, before, &after).await
        }
        .await;
        match after {
            Ok(files) => note.files = Some(files),
            Err(error) => {
                eprintln!("Kuvo could not keep repository files after a message: {error}");
                note.failed = true;
            }
        }
        let turn = turn.to_owned();
        self.snapshot_edit(id, move |agent| {
            let mut all = notes(agent);
            all.insert(turn, note);
            agent.insert("snapshots".into(), json!(all));
        })
        .await
    }

    /// All rows starting at the requested user message, in their saved order.
    /// The SQLite path reads only the IDs; no tool outputs or pictures.
    async fn after_user(&self, id: &str, message: &str) -> io::Result<Vec<String>> {
        let folder = self.folder(id).ok_or(ErrorKind::NotFound)?;
        let message = message.to_owned();
        tokio::task::spawn_blocking(move || {
            let db = folder.join(TRANSCRIPT);
            let missing = || io::Error::new(ErrorKind::NotFound, "No such message of yours.");
            if db.exists() {
                let db = open_transcript(&db)?;
                let body: String = db.query_row("SELECT body FROM message WHERE id=?1", [&message], |row| row.get(0)).map_err(|_| missing())?;
                let row: Value = serde_json::from_str(&body)?;
                if row["type"] != "user" { return Err(missing()); }
                let mut stmt = db.prepare("SELECT id FROM message WHERE position >= (SELECT position FROM message WHERE id=?1) ORDER BY position").map_err(io::Error::other)?;
                stmt.query_map([message], |row| row.get(0)).map_err(io::Error::other)?.collect::<Result<Vec<String>, _>>().map_err(io::Error::other)
            } else {
                let bytes = files::read(&folder.join("messages.json"))?.ok_or_else(missing)?;
                let rows: Vec<Value> = serde_json::from_slice(&bytes)?;
                let start = rows.iter().position(|row| row["id"] == message && row["type"] == "user").ok_or_else(missing)?;
                Ok(rows.into_iter().skip(start).filter_map(|row| row["id"].as_str().map(str::to_owned)).collect())
            }
        }).await?
    }

    /// Stage Undo: only files changed by the marked turns follow the transcript.
    /// The per-conversation line owns exclusion from prompting during this work.
    pub(crate) async fn stage_revert(&self, id: &str, message: &str) -> io::Result<Value> {
        let header = self.read(id).await?.ok_or(ErrorKind::NotFound)?;
        let directory = PathBuf::from(header.directory().ok_or(ErrorKind::NotFound)?);
        let header = header.into_fields();
        let after = self.after_user(id, message).await?;
        let agent = self.snapshot_agent(id).await?;
        let notes = notes(&agent);
        let mut paths = BTreeSet::new();
        let mut kept = false;
        for row in after {
            let Some(note) = notes.get(&row) else {
                continue;
            };
            kept = true;
            let files = note.files.as_ref().filter(|_| !note.failed && note.before.is_some()).ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "Kuvo couldn't keep what the files were around one of these messages, so Undo can't put them back."))?;
            paths.extend(files.iter().cloned());
        }
        let before = notes.get(message).filter(|note| note.before.is_some());
        if kept && before.is_none() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "This message went into the turn before it: undo from that one.",
            ));
        }
        let paths: Vec<_> = paths.into_iter().collect();
        let mut diffs = vec![];
        if let Some(before) = before.filter(|_| !paths.is_empty()) {
            let tree = before.before.as_deref().expect("known tree");
            let at = crate::turn_projection::now();
            let now =
                self.snapshots.take(&directory).await?.ok_or_else(|| {
                    io::Error::other("Kuvo couldn't keep the files as they are now.")
                })?;
            let prior_revert = header.get("revert").is_some_and(|value| !value.is_null());
            let saved = now.clone();
            let union = paths.clone();
            self.snapshot_edit(id, move |agent| {
                if !prior_revert {
                    agent.remove("revertNow");
                    agent.remove("revertAt");
                    agent.remove("revertFiles");
                }
                agent.entry("revertNow").or_insert(json!(saved));
                agent.entry("revertAt").or_insert(json!(at));
                let mut files: BTreeSet<String> = agent
                    .get("revertFiles")
                    .and_then(|value| serde_json::from_value(value.clone()).ok())
                    .unwrap_or_default();
                files.extend(union);
                agent.insert("revertFiles".into(), json!(files));
            })
            .await?;
            let restored = async {
                let diffs = self
                    .snapshots
                    .changes(&directory, tree, &now, &paths)
                    .await?;
                self.snapshots
                    .restore(&directory, tree, &paths, before.at)
                    .await?;
                Ok::<_, io::Error>(diffs)
            }
            .await;
            match restored {
                Ok(changes) => diffs = changes,
                Err(error) => {
                    let _ = self
                        .snapshots
                        .restore(&directory, &now, &paths, Some(at))
                        .await;
                    // Restore the earlier saved redo marks too.
                    self.snapshot_edit(id, move |saved| {
                        for key in ["revertNow", "revertAt", "revertFiles"] {
                            if let Some(value) = agent.get(key) {
                                saved.insert(key.into(), value.clone());
                            } else {
                                saved.remove(key);
                            }
                        }
                    })
                    .await?;
                    return Err(error);
                }
            }
        }
        let revert = json!({"messageID": message, "files": diffs});
        let shown = revert.clone();
        self.change_header(id, move |header| {
            header.insert("revert".into(), shown);
        })
        .await?;
        Ok(revert)
    }

    pub(crate) async fn clear_revert(&self, id: &str) -> io::Result<()> {
        let header = self.read(id).await?.ok_or(ErrorKind::NotFound)?;
        let directory = PathBuf::from(header.directory().ok_or(ErrorKind::NotFound)?);
        let header = header.into_fields();
        if header.get("revert").is_none_or(Value::is_null) {
            return Ok(());
        }
        let agent = self.snapshot_agent(id).await?;
        if let Some(tree) = agent.get("revertNow").and_then(Value::as_str) {
            let files: Vec<String> = agent
                .get("revertFiles")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default();
            self.snapshots
                .restore(
                    &directory,
                    tree,
                    &files,
                    agent.get("revertAt").and_then(Value::as_u64),
                )
                .await?;
        }
        self.change_header(id, |header| {
            header.remove("revert");
        })
        .await?;
        self.snapshot_edit(id, |agent| {
            for key in ["revertNow", "revertAt", "revertFiles"] {
                agent.remove(key);
            }
        })
        .await
    }

    /// A new message after Undo takes the cut transcript and all native slots
    /// away: the next native conversation sees only the surviving history.
    pub(crate) async fn commit_revert(&self, id: &str) -> io::Result<Option<String>> {
        let header = self
            .read(id)
            .await?
            .ok_or(ErrorKind::NotFound)?
            .into_fields();
        let Some(cut) = header
            .get("revert")
            .and_then(|value| value["messageID"].as_str())
            .map(str::to_owned)
        else {
            return Ok(None);
        };
        let cut_row = cut.clone();
        self.mutate_existing(id, move |folder| {
            let db = folder.join(TRANSCRIPT);
            if !db.exists() { write_messages(folder, vec![])?; }
            let mut db = open_transcript(&db)?;
            let tx = db.transaction().map_err(io::Error::other)?;
            let mut stmt = tx.prepare("SELECT id FROM message WHERE position >= (SELECT position FROM message WHERE id=?1)").map_err(io::Error::other)?;
            let removed = stmt.query_map([&cut_row], |row| row.get::<_, String>(0)).map_err(io::Error::other)?.collect::<Result<BTreeSet<_>, _>>().map_err(io::Error::other)?;
            drop(stmt);
            tx.execute("DELETE FROM message WHERE position >= (SELECT position FROM message WHERE id=?1)", [&cut_row]).map_err(io::Error::other)?;
            tx.commit().map_err(io::Error::other)?;
            let mut agent = read_agent(folder)?;
            let mut notes = notes(&agent);
            notes.retain(|id, _| !removed.contains(id));
            agent.insert("snapshots".into(), json!(notes));
            agent.insert("threads".into(), json!({}));
            for key in ["revertNow", "revertAt", "revertFiles"] { agent.remove(key); }
            for key in super::LEGACY { agent.remove(key); }
            files::write_atomic(&folder.join("agent.json"), &serde_json::to_vec(&agent)?)
        }).await?;
        self.change_header(id, |header| {
            header.remove("revert");
        })
        .await?;
        Ok(Some(cut))
    }
}
