//! Shared SQLite connection policy. The owner of each database supplies its schema.

use crate::format;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{path::Path, time::Duration};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("opening SQLite at {}: {source}", path.display())]
    Open {
        path: std::path::PathBuf,
        source: rusqlite::Error,
    },
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Format(#[from] format::Error),
    #[error("SQLite at {} did not enter WAL mode (reported {mode})", path.display())]
    Journal {
        path: std::path::PathBuf,
        mode: String,
    },
}

pub fn open_index(path: &Path, busy: Duration) -> Result<Connection, Error> {
    connect(path, OpenFlags::SQLITE_OPEN_READ_ONLY, busy)
}

/// Adopt each schema step in one immediate transaction before enabling WAL.
/// Passing a future user_version fails without changing the database or its journal mode.
pub fn open_wal(
    path: &Path,
    busy: Duration,
    family: &str,
    steps: &[fn(&Connection) -> rusqlite::Result<()>],
) -> Result<Connection, Error> {
    let mut connection = connect(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        busy,
    )?;
    let current = steps.len() as u32;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let from: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let version = format::check_version(family, Some(&Value::from(from)), 0, current)?;
    for step in &steps[version as usize..] {
        step(&transaction)?;
    }
    if version != current {
        transaction.pragma_update(None, "user_version", current)?;
    }
    transaction.commit()?;
    let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::Journal {
            path: path.into(),
            mode,
        });
    }
    Ok(connection)
}

fn connect(path: &Path, flags: OpenFlags, busy: Duration) -> Result<Connection, Error> {
    let db = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(
        |source| Error::Open {
            path: path.into(),
            source,
        },
    )?;
    db.busy_timeout(busy)?;
    Ok(db)
}

pub const INSIGHTS_BUSY: Duration = Duration::from_secs(3);

/// Open a private staged copy for a move. The engine still owns its schema.
pub fn open_for_move(path: &Path, busy: Duration) -> Result<Connection, Error> {
    let connection = connect(path, OpenFlags::SQLITE_OPEN_READ_WRITE, busy)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    Ok(connection)
}
