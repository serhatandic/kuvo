//! OpenCode conversations from before it was a coding agent here (the engine's), in OpenCode's own
//! database: listed from it, read-only, without starting OpenCode; taken into the agents' store
//! the first time one is opened, from `opencode session export` (whose rows are the ones
//! OpenCode's server answered, so the app draws them as before). Each keeps its `ses_…` ID, which
//! is also its ACP session, so its next turn resumes it. Nothing in OpenCode's database is copied
//! over, rewritten or deleted here.

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Row};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use super::OpenCodeProgram;
use crate::{accounts::AgentKind, session_store::SessionStore};

/// How long an export may take: a long conversation is a few seconds.
const EXPORT: Duration = Duration::from_secs(120);
const BUSY: Duration = Duration::from_secs(2);
const COLUMNS: &str = "id, parent_id, project_id, directory, title, metadata, permission, agent, model, \
    cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, \
    idle_outcome, time_created, time_updated, time_busy, time_idle, time_viewed, time_compacting, time_archived";

/// The computer's OpenCode conversations not yet in the agents' store.
#[derive(Clone)]
pub struct OpenCodeHistory(Arc<Inner>);

struct Inner {
    database: PathBuf,
    program: Option<OpenCodeProgram>,
    store: SessionStore,
    /// One conversation taken at a time.
    taking: Mutex<()>,
}

impl OpenCodeHistory {
    /// OpenCode's database in the box's `home` (its `data`), the pinned program, if it's here, to
    /// export with, and the store they go to.
    pub fn new(home: &Path, program: Option<OpenCodeProgram>, store: SessionStore) -> Self {
        Self(Arc::new(Inner {
            database: home.join("data/opencode/opencode.db"),
            program,
            store,
            taking: Mutex::new(()),
        }))
    }

    /// Its conversations the store doesn't have (`kept`), each as its header: the fields OpenCode's
    /// server gave it, marked as OpenCode's.
    pub async fn listed(&self, kept: BTreeSet<String>) -> io::Result<Vec<Map<String, Value>>> {
        let database = self.0.database.clone();
        tokio::task::spawn_blocking(move || {
            let Some(db) = open(&database)? else {
                return Ok(Vec::new());
            };
            let mut query = db
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM session_v2 ORDER BY time_created, id"
                ))
                .map_err(io::Error::other)?;
            let rows = query
                .query_map([], header)
                .map_err(io::Error::other)?
                .filter_map(Result::ok)
                .filter(|header| header["id"].as_str().is_some_and(|id| !kept.contains(id)))
                .collect();
            Ok(rows)
        })
        .await?
    }

    /// One conversation's header, as [`OpenCodeHistory::listed`] gives it; none where OpenCode has
    /// no such conversation.
    pub async fn header(&self, id: &str) -> io::Result<Option<Map<String, Value>>> {
        let (database, id) = (self.0.database.clone(), id.to_owned());
        tokio::task::spawn_blocking(move || {
            let Some(db) = open(&database)? else {
                return Ok(None);
            };
            db.query_row(
                &format!("SELECT {COLUMNS} FROM session_v2 WHERE id = ?1"),
                [&id],
                header,
            )
            .optional()
            .map_err(io::Error::other)
        })
        .await?
    }

    /// Takes `id` into the store, if it's one of OpenCode's and isn't there yet. False when
    /// OpenCode has no such conversation.
    pub async fn take(&self, id: &str) -> io::Result<bool> {
        let _one = self.0.taking.lock().await;
        if self.0.store.read(id).await?.is_some() {
            return Ok(true);
        }
        let (database, session) = (self.0.database.clone(), id.to_owned());
        let Some(count) = tokio::task::spawn_blocking(move || count(&database, &session)).await??
        else {
            return Ok(false);
        };
        let program = self.0.program.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "OpenCode isn’t on this computer.")
        })?;
        let mut export = export(program, id).await?;
        let messages = match export.get_mut("messages").map(Value::take) {
            Some(Value::Array(messages)) => messages,
            _ => return Err(unreadable()),
        };
        // (One whose rows don't all come is read again next time, never kept short.)
        if messages.len() != count {
            return Err(io::Error::other(format!(
                "OpenCode exported {} of the conversation’s {count} messages.",
                messages.len()
            )));
        }
        let Some(Value::Object(mut info)) = export.get_mut("info").map(Value::take) else {
            return Err(unreadable());
        };
        if info.get("id").and_then(Value::as_str) != Some(id) {
            return Err(unreadable());
        }
        mark(&mut info);
        self.0.store.create(info).await?;
        self.0.store.write_messages(id, messages).await?;
        let mut state = Map::new();
        state.insert("acpSession".into(), id.into());
        self.0
            .store
            .save_state(id, AgentKind::OpenCode.provider(), state)
            .await?;
        Ok(true)
    }
}

fn unreadable() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "OpenCode’s export couldn’t be read.",
    )
}

/// Its database, read-only; none where it has none (or none of today's sessions).
fn open(database: &Path) -> io::Result<Option<Connection>> {
    if !database.is_file() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(io::Error::other)?;
    db.busy_timeout(BUSY).map_err(io::Error::other)?;
    let current: bool = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_v2'",
            [],
            |row| row.get::<_, i64>(0).map(|count| count > 0),
        )
        .map_err(io::Error::other)?;
    Ok(current.then_some(db))
}

/// How many messages OpenCode has for `id`; none when it has no such conversation.
fn count(database: &Path, id: &str) -> io::Result<Option<usize>> {
    let Some(db) = open(database)? else {
        return Ok(None);
    };
    let known = db
        .query_row("SELECT 1 FROM session_v2 WHERE id = ?1", [id], |_| Ok(()))
        .optional()
        .map_err(io::Error::other)?;
    if known.is_none() {
        return Ok(None);
    }
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM session_message WHERE session_id = ?1",
            [id],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    Ok(Some(usize::try_from(count).unwrap_or_default()))
}

/// A conversation's row as OpenCode's server gave it (`Session.Info`): its fields where set.
fn header(row: &Row<'_>) -> rusqlite::Result<Map<String, Value>> {
    let text = |at: usize| row.get::<_, Option<String>>(at);
    let number = |at: usize| row.get::<_, Option<i64>>(at);
    let parsed = |at: usize| -> rusqlite::Result<Option<Value>> {
        Ok(text(at)?.and_then(|json| serde_json::from_str(&json).ok()))
    };
    let mut info = Map::new();
    info.insert("id".into(), row.get::<_, String>(0)?.into());
    if let Some(parent) = text(1)? {
        info.insert("parentID".into(), parent.into());
    }
    info.insert("projectID".into(), text(2)?.unwrap_or_default().into());
    if let Some(agent) = text(7)? {
        info.insert("agent".into(), agent.into());
    }
    if let Some(model) = parsed(8)? {
        info.insert("model".into(), model);
    }
    info.insert(
        "cost".into(),
        json!(row.get::<_, Option<f64>>(9)?.unwrap_or_default()),
    );
    let tokens = |at: usize| number(at).map(Option::unwrap_or_default);
    info.insert(
        "tokens".into(),
        json!({ "input": tokens(10)?, "output": tokens(11)?, "reasoning": tokens(12)?,
            "cache": { "read": tokens(13)?, "write": tokens(14)? } }),
    );
    if let Some(outcome) = text(15)? {
        info.insert("outcome".into(), outcome.into());
    }
    let mut time = Map::new();
    for (key, at) in [
        ("created", 16),
        ("updated", 17),
        ("busy", 18),
        ("idle", 19),
        ("viewed", 20),
        ("compacting", 21),
        ("archived", 22),
    ] {
        if let Some(when) = number(at)? {
            time.insert(key.into(), when.into());
        }
    }
    info.insert("time".into(), Value::Object(time));
    if let Some(title) = text(4)? {
        info.insert("title".into(), title.into());
    }
    let mut metadata = parsed(5)?;
    if let Some(Value::Object(metadata)) = &mut metadata {
        info.insert("metadata".into(), Value::Object(std::mem::take(metadata)));
    }
    if let Some(permissions) = parsed(6)? {
        info.insert("permissions".into(), permissions);
    }
    info.insert(
        "location".into(),
        json!({ "directory": text(3)?.unwrap_or_default() }),
    );
    mark(&mut info);
    Ok(info)
}

/// Marked as the OpenCode agent's, as the store marks its own.
fn mark(info: &mut Map<String, Value>) {
    let metadata = info
        .entry("metadata")
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    metadata["kuvoAgent"] = AgentKind::OpenCode.name().into();
}

/// `opencode session export <id>` on its own server (`--standalone`: never OpenCode's background
/// service, so nothing stays running).
async fn export(program: &OpenCodeProgram, id: &str) -> io::Result<Value> {
    use kuvo_os::process::{Spawn, Stdio};
    use std::io::Read;
    let mut spawn = Spawn::new(program.executable.clone(), program.env.clone());
    spawn.args = ["session", "export", id, "--standalone"]
        .map(Into::into)
        .into();
    spawn.stdout = Stdio::Pipe;
    let printed = tokio::task::spawn_blocking(move || -> io::Result<(bool, Vec<u8>)> {
        let mut child = spawn.spawn()?;
        let mut stdout = child.stdout.take().ok_or_else(unreadable)?;
        let reading = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let deadline = std::time::Instant::now() + EXPORT;
        while !child.has_exited().unwrap_or(true) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = child.stop(Duration::from_secs(3), |_| {})?;
        let bytes = reading.join().map_err(|_| unreadable())??;
        Ok((status.success(), bytes))
    })
    .await??;
    match printed {
        (true, bytes) => serde_json::from_slice(&bytes).map_err(|_| unreadable()),
        (false, _) => Err(io::Error::other(
            "OpenCode couldn’t export the conversation.",
        )),
    }
}

#[cfg(test)]
#[path = "opencode_history_tests.rs"]
mod tests;
