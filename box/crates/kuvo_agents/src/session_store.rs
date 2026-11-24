//! The session header and vendor state stored with each coding conversation.

use crate::session::{SessionContext, VendorState};
use kuvo_store::{files, format::ObjectFormat, locks::OwnedLock, sqlite};
use serde_json::{Map, Value};
use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    time::Duration,
};

mod metadata;
mod projection;

pub use metadata::SessionPatch;

const FORMAT: ObjectFormat = ObjectFormat::additive("Coding-agent session");
const TRANSCRIPT: &str = "messages-v1.sqlite";

#[derive(Clone, Copy)]
pub enum MessageOrder {
    OldestFirst,
    NewestFirst,
}

pub struct MessagePage {
    pub messages: Vec<Value>,
    pub more: bool,
}

pub struct SessionHeader {
    id: String,
    fields: Map<String, Value>,
}

impl SessionHeader {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn directory(&self) -> Option<&str> {
        self.fields.get("location")?.get("directory")?.as_str()
    }

    /// The conversation that started this one, for a subagent's.
    pub fn parent(&self) -> Option<&str> {
        self.fields.get("parentID")?.as_str()
    }

    pub fn into_fields(self) -> Map<String, Value> {
        self.fields
    }
}

#[derive(Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn read(&self, id: &str) -> io::Result<Option<SessionHeader>> {
        let Some(folder) = self.folder(id) else {
            return Ok(None);
        };
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || read_header(&folder, &id)).await?
    }

    pub async fn headers(&self) -> io::Result<Vec<SessionHeader>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let entries = match fs::read_dir(&store.root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(error),
            };
            let mut headers = Vec::new();
            for entry in entries {
                let entry = entry?;
                let Ok(id) = entry.file_name().into_string() else {
                    continue;
                };
                if valid_id(&id)
                    && let Ok(Some(header)) = read_header(&entry.path(), &id)
                {
                    headers.push(header);
                }
            }
            headers.sort_by(|left, right| left.id().cmp(right.id()));
            Ok(headers)
        })
        .await?
    }

    pub async fn context(&self, id: &str) -> io::Result<Option<SessionContext>> {
        let Some(folder) = self.folder(id) else {
            return Ok(None);
        };
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let Some(header) = read_header(&folder, &id)? else {
                return Ok(None);
            };
            let directory = header.directory().ok_or_else(|| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!("The saved session {id} has no working directory."),
                )
            })?;
            Ok(Some(SessionContext {
                session_id: id,
                directory: PathBuf::from(directory),
                state: read_state(&folder)?,
            }))
        })
        .await?
    }

    pub async fn message_page(
        &self,
        id: &str,
        after: Option<String>,
        limit: u32,
        order: MessageOrder,
    ) -> io::Result<Option<MessagePage>> {
        let Some(folder) = self.folder(id) else {
            return Ok(None);
        };
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if read_header(&folder, &id)?.is_none() {
                return Ok(None);
            }
            let db = folder.join(TRANSCRIPT);
            let messages = if db.try_exists()? {
                query_messages(&db, after.as_deref(), limit, order)?
            } else {
                legacy_page(&folder, after.as_deref(), limit, order)?
            };
            let mut page = MessagePage {
                messages,
                more: false,
            };
            page.more = page.messages.len() > limit as usize;
            page.messages.truncate(limit as usize);
            Ok(Some(page))
        })
        .await?
    }

    /// Save vendor state only while the session still exists. A late native
    /// frame after deletion must never recreate its previous session folder.
    pub async fn save_state(&self, id: &str, state: VendorState) -> io::Result<bool> {
        self.mutate_existing(id, move |folder| {
            let body = serde_json::to_vec(&state)?;
            files::write_atomic(&folder.join("agent.json"), &body)
        })
        .await
    }

    /// Update a transcript by message ID. Updating an existing step keeps its
    /// position, including when a tool result arrives after the step ended.
    pub async fn write_messages(&self, id: &str, rows: Vec<Value>) -> io::Result<bool> {
        self.mutate_existing(id, move |folder| write_messages(folder, rows))
            .await
    }

    pub async fn delete(&self, id: &str) -> io::Result<bool> {
        self.mutate_existing(id, |folder| fs::remove_dir_all(folder))
            .await
    }

    async fn mutate_existing(
        &self,
        id: &str,
        change: impl FnOnce(&Path) -> io::Result<()> + Send + 'static,
    ) -> io::Result<bool> {
        let Some(folder) = self.folder(id) else {
            return Ok(false);
        };
        let lock = self.root.join(format!(".{id}.lock"));
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _guard = OwnedLock::acquire(&lock, Duration::from_secs(1))?;
            if read_header(&folder, &id)?.is_none() {
                return Ok(false);
            }
            change(&folder)?;
            Ok(true)
        })
        .await?
    }

    fn folder(&self, id: &str) -> Option<PathBuf> {
        valid_id(id).then(|| self.root.join(id))
    }
}

fn read_header(folder: &Path, id: &str) -> io::Result<Option<SessionHeader>> {
    Ok(FORMAT
        .read(&folder.join("session.json"))
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?
        .filter(|saved| saved.fields.get("id").and_then(Value::as_str) == Some(id))
        .map(|saved| SessionHeader {
            id: id.to_owned(),
            fields: saved.fields,
        }))
}

fn read_state(folder: &Path) -> io::Result<VendorState> {
    // Match the bridge's treatment of old or incomplete vendor markers.
    Ok(files::read(&folder.join("agent.json"))?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default())
}

fn query_messages(
    db: &Path,
    after: Option<&str>,
    limit: u32,
    order: MessageOrder,
) -> io::Result<Vec<Value>> {
    let db = sqlite::open_index(db, Duration::from_secs(1)).map_err(io::Error::other)?;
    let (direction, compared) = match order {
        MessageOrder::OldestFirst => ("ASC", ">"),
        MessageOrder::NewestFirst => ("DESC", "<"),
    };
    let limit = i64::from(limit) + 1;
    let sql = if after.is_some() {
        format!(
            "SELECT body FROM message WHERE position {compared} (
                SELECT position FROM message WHERE id = ?1 ORDER BY position {direction} LIMIT 1
            ) ORDER BY position {direction} LIMIT ?2"
        )
    } else {
        format!("SELECT body FROM message ORDER BY position {direction} LIMIT ?1")
    };
    let mut query = db.prepare(&sql).map_err(io::Error::other)?;
    let mut rows = match after {
        Some(id) => query.query((id, limit)),
        None => query.query([limit]),
    }
    .map_err(io::Error::other)?;
    let mut messages = Vec::new();
    while let Some(row) = rows.next().map_err(io::Error::other)? {
        let body: String = row.get(0).map_err(io::Error::other)?;
        messages.push(serde_json::from_str(&body)?);
    }
    Ok(messages)
}

fn write_messages(folder: &Path, rows: Vec<Value>) -> io::Result<()> {
    let destination = folder.join(TRANSCRIPT);
    // Seed a staging database when adopting a legacy array. An interrupted
    // import leaves the legacy transcript authoritative until the rename.
    let staging = (!destination.try_exists()?)
        .then(|| tempfile::tempdir_in(folder))
        .transpose()?;
    let file = staging
        .as_ref()
        .map_or_else(|| destination.clone(), |dir| dir.path().join(TRANSCRIPT));
    let mut db = sqlite::open_wal(
        &file,
        Duration::from_secs(1),
        "Coding-agent transcript",
        &[|db| {
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS message(
                    position INTEGER PRIMARY KEY, id TEXT, body TEXT NOT NULL,
                    insight TEXT, artifacts TEXT
                );
                CREATE INDEX IF NOT EXISTS message_id ON message(id);",
            )
        }],
    )
    .map_err(io::Error::other)?;
    db.pragma_update(None, "synchronous", "FULL")
        .map_err(io::Error::other)?;
    let transaction = db.transaction().map_err(io::Error::other)?;
    if staging.is_some()
        && let Some(bytes) = files::read(&folder.join("messages.json"))?
    {
        let legacy: Vec<Value> = serde_json::from_slice(&bytes)?;
        let mut insert = transaction
            .prepare_cached(
                "INSERT INTO message(position, id, body, insight, artifacts)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(io::Error::other)?;
        for (position, body) in legacy.iter().enumerate() {
            insert
                .execute((
                    position,
                    body["id"].as_str(),
                    serde_json::to_string(body)?,
                    encoded_projection(body, projection::View::Insight)?,
                    encoded_projection(body, projection::View::Artifacts)?,
                ))
                .map_err(io::Error::other)?;
        }
    }
    {
        let mut write = transaction
            .prepare_cached(
                "INSERT INTO message(position, id, body, insight, artifacts)
                 VALUES (COALESCE(
                     (SELECT position FROM message WHERE id = ?1 ORDER BY position DESC LIMIT 1),
                     (SELECT COALESCE(MAX(position) + 1, 0) FROM message)
                 ), ?1, ?2, ?3, ?4)
                 ON CONFLICT(position) DO UPDATE SET
                     body = excluded.body, insight = excluded.insight, artifacts = excluded.artifacts
                 WHERE body <> excluded.body OR insight IS NOT excluded.insight OR artifacts IS NOT excluded.artifacts",
            )
            .map_err(io::Error::other)?;
        for body in rows {
            let id = body["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidInput, "The transcript message has no ID.")
                })?;
            write
                .execute((
                    id,
                    serde_json::to_string(&body)?,
                    encoded_projection(&body, projection::View::Insight)?,
                    encoded_projection(&body, projection::View::Artifacts)?,
                ))
                .map_err(io::Error::other)?;
        }
    }
    transaction.commit().map_err(io::Error::other)?;
    if staging.is_some() {
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(io::Error::other)?;
        drop(db);
        files::write_generated_new(&destination, |target| {
            io::copy(&mut fs::File::open(file)?, target).map(|_| ())
        })?;
    }
    Ok(())
}

fn encoded_projection(body: &Value, view: projection::View) -> io::Result<Option<String>> {
    projection::project(body, view)
        .map(|value| serde_json::to_string(&value).map_err(Into::into))
        .transpose()
}

fn legacy_page(
    folder: &Path,
    after: Option<&str>,
    limit: u32,
    order: MessageOrder,
) -> io::Result<Vec<Value>> {
    let Some(body) = files::read(&folder.join("messages.json"))? else {
        return Ok(Vec::new());
    };
    let mut messages: Vec<Value> = serde_json::from_slice(&body)?;
    if matches!(order, MessageOrder::NewestFirst) {
        messages.reverse();
    }
    let start = match after {
        Some(id) => match messages.iter().position(|row| row["id"] == id) {
            Some(position) => position + 1,
            None => return Ok(Vec::new()),
        },
        None => 0,
    };
    Ok(messages
        .into_iter()
        .skip(start)
        .take((limit as usize).saturating_add(1))
        .collect())
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    })
}
