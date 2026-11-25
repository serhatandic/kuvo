//! A turn an earlier resident left unfinished: its steps closed, its tools
//! stopped, and its end written.

use super::{SessionStore, TRANSCRIPT, read_header};
use kuvo_store::{files, sqlite};
use serde_json::{Value, json};
use std::{
    io::{self, ErrorKind},
    path::Path,
    time::Duration,
};

/// The last turn of a conversation whose end isn't written yet.
pub struct OpenTurn {
    /// The row that began it: your message, a note from the app, or a summary.
    pub start: Value,
    /// What it wrote since, oldest first.
    pub rows: Vec<Value>,
}

/// Rows that begin a turn.
fn begins_turn(row: &Value) -> bool {
    matches!(
        row["type"].as_str(),
        Some("user" | "synthetic" | "compaction")
    )
}

impl SessionStore {
    /// The conversation's last turn when it has no end yet. None when it
    /// ended, or when the conversation isn't here.
    pub async fn open_turn(&self, id: &str) -> io::Result<Option<OpenTurn>> {
        let Some(folder) = self.folder(id) else {
            return Ok(None);
        };
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if read_header(&folder, &id)?.is_none() {
                return Ok(None);
            }
            open_turn(&folder)
        })
        .await?
    }

    /// A turn the resident stopped in the middle of (it quit, or was
    /// replaced and the next couldn't take the turn up): its steps closed,
    /// its tools stopped, and its end marked interrupted. A summary that
    /// waited for it is dropped, unless it was held for the next resident.
    /// Whether anything changed.
    pub async fn repair(&self, id: &str) -> io::Result<bool> {
        let Some(open) = self.open_turn(id).await? else {
            return Ok(false);
        };
        let mut rows = open.rows;
        let mut changed = close_rows(&mut rows);
        let at = rows
            .last()
            .or(Some(&open.start))
            .and_then(|row| {
                row["time"]["completed"]
                    .as_u64()
                    .or_else(|| row["time"]["created"].as_u64())
            })
            .unwrap_or_else(crate::turn_projection::now);
        changed.push(json!({
            "type":"idle", "id":SessionStore::make_id("msg")?, "time":{"created":at},
            "outcome":"interrupted", "reason":"restart",
        }));
        self.edit_line(id, |line| {
            let before = line.inputs.len();
            line.inputs
                .retain(|input| !input.is_compaction() || input.held);
            ((), line.inputs.len() != before)
        })
        .await?;
        if !self.write_rows(id, changed).await? {
            return Ok(false);
        }
        self.change_header(id, move |header| {
            header.insert("outcome".into(), json!("interrupted"));
            let time = header
                .entry("time")
                .or_insert_with(|| json!({}));
            if let Some(time) = time.as_object_mut() {
                time.insert("idle".into(), json!(at));
            }
        })
        .await
    }

    /// Take rows out of a conversation's transcript.
    pub(crate) async fn remove_messages(&self, id: &str, ids: Vec<String>) -> io::Result<bool> {
        if ids.is_empty() {
            return Ok(true);
        }
        self.mutate_existing(id, move |folder| {
            let file = folder.join(TRANSCRIPT);
            if !file.try_exists()? {
                return Ok(());
            }
            let mut db = super::open_transcript(&file)?;
            let transaction = db.transaction().map_err(io::Error::other)?;
            for id in &ids {
                transaction
                    .execute("DELETE FROM message WHERE id = ?1", [id])
                    .map_err(io::Error::other)?;
            }
            transaction.commit().map_err(io::Error::other)
        })
        .await
    }
}

fn open_turn(folder: &Path) -> io::Result<Option<OpenTurn>> {
    let file = folder.join(TRANSCRIPT);
    let mut after = Vec::new();
    let mut visit = |row: Value| -> Option<Option<OpenTurn>> {
        if row["type"] == "idle" {
            return Some(None);
        }
        if begins_turn(&row) {
            after.reverse();
            return Some(Some(OpenTurn {
                start: row,
                rows: std::mem::take(&mut after),
            }));
        }
        after.push(row);
        None
    };
    if file.try_exists()? {
        let db = sqlite::open_index(&file, Duration::from_secs(1)).map_err(io::Error::other)?;
        let mut statement = db
            .prepare("SELECT body FROM message ORDER BY position DESC")
            .map_err(io::Error::other)?;
        let mut rows = statement.query([]).map_err(io::Error::other)?;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let body: String = row.get(0).map_err(io::Error::other)?;
            if let Some(found) = visit(serde_json::from_str(&body)?) {
                return Ok(found);
            }
        }
        return Ok(None);
    }
    let Some(bytes) = files::read(&folder.join("messages.json"))? else {
        return Ok(None);
    };
    let rows: Vec<Value> = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
    for row in rows.into_iter().rev() {
        if let Some(found) = visit(row) {
            return Ok(found);
        }
    }
    Ok(None)
}

/// Close what a turn cut short left open: its steps, and the tools, commands
/// and summaries under way in it. The rows that changed.
pub(crate) fn close_rows(rows: &mut [Value]) -> Vec<Value> {
    let mut changed = Vec::new();
    for row in rows.iter_mut() {
        let mut touched = false;
        match row["type"].as_str() {
            Some("assistant") => {
                if row["time"]["completed"].is_null() {
                    let at = row["time"]["streamed"]
                        .as_u64()
                        .or_else(|| row["time"]["created"].as_u64());
                    row["time"]["completed"] = json!(at);
                    touched = true;
                }
                if let Some(parts) = row["content"].as_array_mut() {
                    for part in parts {
                        let status = part["state"]["status"].as_str();
                        if part["type"] != "tool" || !matches!(status, Some("running" | "streaming"))
                        {
                            continue;
                        }
                        let running = status == Some("running");
                        let mut state = json!({
                            "status":"error",
                            "input": if running { part["state"]["input"].clone() } else { json!({}) },
                            "error":{"type":"aborted","message":"Stopped."},
                            "content":[],
                        });
                        if running && !part["state"]["metadata"].is_null() {
                            state["metadata"] = part["state"]["metadata"].clone();
                        }
                        part["state"] = state;
                        touched = true;
                    }
                }
            }
            Some("compaction" | "shell") if row["status"] == "running" => {
                row["status"] = json!("failed");
                row["time"]["completed"] = row["time"]["created"].clone();
                touched = true;
            }
            _ => {}
        }
        if touched {
            changed.push(row.clone());
        }
    }
    changed
}
