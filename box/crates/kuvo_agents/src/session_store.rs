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

mod line;
mod metadata;
mod pictures;
mod projection;
mod repair;

pub use line::Line;
pub(crate) use repair::close_rows;
pub use repair::OpenTurn;

pub use metadata::{NewSession, SessionPatch};

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

    /// How its last turn ended: `succeeded`, `failed` or `interrupted`.
    pub fn outcome(&self) -> Option<&str> {
        self.fields.get("outcome")?.as_str()
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
    /// What the agents keep going besides each conversation's turn, given to each thread.
    background: crate::background::Background,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            background: crate::background::Background::default(),
        }
    }

    /// What the agents keep going besides each conversation's own turn.
    pub fn background(&self) -> &crate::background::Background {
        &self.background
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

    /// What a turn of `provider`'s agent opens the conversation with: its
    /// own slot of the saved state, beside the inbox and the other agents'.
    pub async fn context(&self, id: &str, provider: &str) -> io::Result<Option<SessionContext>> {
        let Some(folder) = self.folder(id) else {
            return Ok(None);
        };
        let id = id.to_owned();
        let provider = provider.to_owned();
        let background = self.background.clone();
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
            let state = slot_state(&folder, &header, &read_agent(&folder)?, &provider)?;
            Ok(Some(SessionContext {
                session_id: id,
                directory: PathBuf::from(directory),
                state,
                background,
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

    /// Save `provider`'s vendor state into its slot only while the session
    /// still exists. A late native frame after deletion must never recreate
    /// its previous session folder. The inbox and other agents' slots stay.
    pub async fn save_state(
        &self,
        id: &str,
        provider: &str,
        state: VendorState,
    ) -> io::Result<bool> {
        let session = id.to_owned();
        let provider = provider.to_owned();
        self.mutate_existing(id, move |folder| {
            let header = read_header(folder, &session)?.ok_or(ErrorKind::NotFound)?;
            let mut agent = read_agent(folder)?;
            let threads = slots(folder, &header, &mut agent)?;
            let slot = threads
                .entry(provider)
                .or_insert_with(|| Value::Object(Map::new()));
            if !slot.is_object() {
                *slot = Value::Object(Map::new());
            }
            if let Some(slot) = slot.as_object_mut() {
                slot.insert("state".into(), Value::Object(state));
            }
            files::write_atomic(&folder.join("agent.json"), &serde_json::to_vec(&agent)?)
        })
        .await
    }

    /// Update a transcript by message ID. Updating an existing step keeps its
    /// position, including when a tool result arrives after the step ended.
    /// Kuvo Code's instructions for the conversation (its worktree, the
    /// computer it moved to, its project's folders), as its agent is told
    /// them: each entry in order, a paragraph apart.
    pub async fn instructions(&self, id: &str) -> io::Result<String> {
        let Some(folder) = self.folder(id) else {
            return Ok(String::new());
        };
        tokio::task::spawn_blocking(move || {
            let agent = read_agent(&folder)?;
            Ok(agent
                .get("kuvoInstructions")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|entries| entries.values())
                .filter_map(Value::as_str)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"))
        })
        .await?
    }

    pub async fn write_messages(&self, id: &str, mut rows: Vec<Value>) -> io::Result<bool> {
        let attachments = self.attachments();
        let session = id.to_owned();
        self.mutate_existing(id, move |folder| {
            if let Some(attachments) = &attachments {
                for row in &mut rows {
                    pictures::retain(attachments, &session, row);
                }
            }
            write_messages(folder, rows)
        })
        .await
    }

    /// Where a box keeps attachments (`<home>/attachments`), for a store in the box's own
    /// `agents/sessions`.
    fn attachments(&self) -> Option<PathBuf> {
        let agents = self.root.parent()?;
        (self.root.file_name()? == "sessions" && agents.file_name()? == "agents")
            .then(|| agents.parent().map(|home| home.join("attachments")))
            .flatten()
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
        Ok(self.mutate_existing_with(id, change).await?.is_some())
    }

    async fn mutate_existing_with<T: Send + 'static>(
        &self,
        id: &str,
        change: impl FnOnce(&Path) -> io::Result<T> + Send + 'static,
    ) -> io::Result<Option<T>> {
        let Some(folder) = self.folder(id) else {
            return Ok(None);
        };
        let lock = self.root.join(format!(".{id}.lock"));
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _guard = OwnedLock::acquire(&lock, Duration::from_secs(1))?;
            if read_header(&folder, &id)?.is_none() {
                return Ok(None);
            }
            change(&folder).map(Some)
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

/// Native conversation keys an agent kept at the top of `agent.json` before
/// each agent had its own slot.
const LEGACY: [&str; 7] = [
    "claudeSession",
    "claudeStarted",
    "codexThread",
    "acpSession",
    "acpInstructed",
    "cursorAgent",
    "cursorInstructed",
];

/// The bridge's per-conversation record of its agents (`agent.json`). Like
/// the bridge, an unreadable record reads as empty.
fn read_agent(folder: &Path) -> io::Result<Map<String, Value>> {
    Ok(files::read(&folder.join("agent.json"))?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default())
}

fn slot_state(
    folder: &Path,
    header: &SessionHeader,
    agent: &Map<String, Value>,
    provider: &str,
) -> io::Result<VendorState> {
    let state = |slot: Option<&Value>| {
        slot.and_then(|slot| slot.get("state"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    if let Some(threads) = agent.get("threads").filter(|threads| truthy(threads)) {
        return Ok(state(threads.get(provider)));
    }
    let legacy = legacy_state(agent);
    if legacy.is_empty() || legacy_owner(folder, header)?.as_deref() != Some(provider) {
        return Ok(VendorState::new());
    }
    Ok(legacy)
}

/// The slots, made from the single agent's state of before if need be.
fn slots<'a>(
    folder: &Path,
    header: &SessionHeader,
    agent: &'a mut Map<String, Value>,
) -> io::Result<&'a mut Map<String, Value>> {
    if !agent.get("threads").is_some_and(truthy) {
        let legacy = legacy_state(agent);
        let mut threads = Map::new();
        if !legacy.is_empty()
            && let Some(owner) = legacy_owner(folder, header)?
        {
            threads.insert(owner, serde_json::json!({ "state": legacy }));
        }
        for key in LEGACY {
            agent.shift_remove(key);
        }
        agent.insert("threads".into(), Value::Object(threads));
    }
    let threads = agent
        .get_mut("threads")
        .expect("the slots were just made");
    if !threads.is_object() {
        *threads = Value::Object(Map::new());
    }
    Ok(threads.as_object_mut().expect("the slots are an object"))
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
        && value.as_str() != Some("")
        && value.as_f64() != Some(0.0)
}

fn legacy_state(agent: &Map<String, Value>) -> VendorState {
    LEGACY
        .into_iter()
        .filter_map(|key| Some((key.to_owned(), agent.get(key)?.clone())))
        .collect()
}

/// The agent that wrote the conversation's replies (the session's model may
/// have moved since), or its model's for one without replies.
fn legacy_owner(folder: &Path, header: &SessionHeader) -> io::Result<Option<String>> {
    let replied = last_assistant_provider(folder)?;
    let owner = replied.or_else(|| {
        header.fields.get("model")?.get("providerID")?.as_str().map(str::to_owned)
    });
    Ok(owner.filter(|owner| !owner.is_empty() && owner != "auto"))
}

fn last_assistant_provider(folder: &Path) -> io::Result<Option<String>> {
    let db = folder.join(TRANSCRIPT);
    if db.try_exists()? {
        let db = sqlite::open_index(&db, Duration::from_secs(1)).map_err(io::Error::other)?;
        let mut query = db
            .prepare(
                "SELECT body FROM message WHERE json_extract(body, '$.type') = 'assistant'
                 ORDER BY position DESC LIMIT 1",
            )
            .map_err(io::Error::other)?;
        let mut rows = query.query([]).map_err(io::Error::other)?;
        return match rows.next().map_err(io::Error::other)? {
            Some(row) => {
                let body: String = row.get(0).map_err(io::Error::other)?;
                let body: Value = serde_json::from_str(&body)?;
                Ok(body["model"]["providerID"].as_str().map(str::to_owned))
            }
            None => Ok(None),
        };
    }
    let Some(raw) = files::read(&folder.join("messages.json"))? else {
        return Ok(None);
    };
    let rows: Vec<Value> = serde_json::from_slice(&raw)?;
    Ok(rows
        .iter()
        .rev()
        .find(|row| row["type"] == "assistant")
        .and_then(|row| row["model"]["providerID"].as_str().map(str::to_owned)))
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

/// The transcript database, for writing.
fn open_transcript(file: &Path) -> io::Result<rusqlite::Connection> {
    sqlite::open_wal(
        file,
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
    .map_err(io::Error::other)
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
    let mut db = open_transcript(&file)?;
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
