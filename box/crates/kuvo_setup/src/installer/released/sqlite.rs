//! Fingerprint committed SQLite rows using the deployed TypeScript encoding.

use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

use kuvo_wire::keys::encode;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SCAN_TIME: Duration = Duration::from_millis(1500);

/// The engine makes a database before a fresh SSH applicant has joined. A
/// read-only snapshot distinguishes its imported sign-ins from saved sessions.
pub(in crate::installer) fn staged_database_has_no_sessions(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    for directory in [Some(parent), parent.parent()].into_iter().flatten() {
        if !directory
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir())
        {
            return false;
        }
    }
    if !path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_file())
    {
        return false;
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match std::fs::symlink_metadata(sidecar) {
            Ok(metadata) if metadata.is_file() => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            _ => return false,
        }
    }
    let read = || -> rusqlite::Result<bool> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut db = Connection::open_with_flags(path, flags)?;
        db.busy_timeout(Duration::ZERO)?;
        db.pragma_update(None, "trusted_schema", false)?;
        let tx = db.transaction()?;
        for table in ["session", "session_v2"] {
            let present: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )?;
            if present
                && tx.query_row(
                    &format!("SELECT EXISTS (SELECT 1 FROM {table})"),
                    [],
                    |row| row.get::<_, bool>(0),
                )?
            {
                return Ok(false);
            }
        }
        Ok(true)
    };
    read().unwrap_or(false)
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn check_time(deadline: Instant) -> io::Result<()> {
    if Instant::now() > deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "retained database scan took too long",
        ))
    } else {
        Ok(())
    }
}

fn sqlite_error(error: rusqlite::Error) -> io::Error {
    let kind = match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::OperationInterrupted) => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}

pub(super) fn stamp(hash: &mut Sha256, databases: &[(std::path::PathBuf, bool)]) -> io::Result<()> {
    let deadline = Instant::now() + SCAN_TIME;
    for (path, immutable) in databases {
        stamp_database(hash, path, *immutable, deadline)?;
    }
    Ok(())
}

fn stamp_database(
    hash: &mut Sha256,
    path: &Path,
    immutable: bool,
    deadline: Instant,
) -> io::Result<()> {
    check_time(deadline)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut db = if immutable {
        let mut uri = reqwest::Url::from_file_path(path)
            .map_err(|_| io::Error::other("invalid retained database path"))?;
        uri.set_query(Some("mode=ro&immutable=1"));
        Connection::open_with_flags(uri.as_str(), flags | OpenFlags::SQLITE_OPEN_URI)
    } else {
        Connection::open_with_flags(path, flags)
    }
    .map_err(sqlite_error)?;
    db.busy_timeout(Duration::ZERO).map_err(sqlite_error)?;
    db.pragma_update(None, "trusted_schema", false)
        .map_err(sqlite_error)?;
    db.progress_handler(1000, Some(move || Instant::now() >= deadline));
    let version =
        |db: &Connection| db.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0));
    let before = version(&db).map_err(sqlite_error)?;
    let tx = db.transaction().map_err(sqlite_error)?;
    let (schema, encoded) = schema(&tx).map_err(sqlite_error)?;
    let user: i64 = tx
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sqlite_error)?;
    let app: i64 = tx
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(sqlite_error)?;
    hash.update(
        json!([path, encoded, {"user_version": user}, {"application_id": app}]).to_string(),
    );
    for name in schema {
        stamp_table(hash, &tx, &name, deadline)?;
    }
    tx.commit().map_err(sqlite_error)?;
    if version(&db).map_err(sqlite_error)? != before {
        return Err(io::Error::other(
            "retained database changed during the scan",
        ));
    }
    Ok(())
}

fn schema(db: &Connection) -> rusqlite::Result<(Vec<String>, Vec<Value>)> {
    let mut select =
        db.prepare("SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name")?;
    let mut rows = select.query([])?;
    let mut tables = Vec::new();
    let mut encoded = Vec::new();
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let name: String = row.get(1)?;
        let table: String = row.get(2)?;
        if matches!(
            table.as_str(),
            "sqlite_stat1" | "sqlite_stat2" | "sqlite_stat3" | "sqlite_stat4"
        ) {
            continue;
        }
        let sql: Option<String> = row.get(3)?;
        encoded.push(json!({"type": kind, "name": name, "tbl_name": table, "sql": sql}));
        if kind == "table" {
            tables.push(name);
        }
    }
    Ok((tables, encoded))
}

fn stamp_table(
    hash: &mut Sha256,
    db: &Connection,
    name: &str,
    deadline: Instant,
) -> io::Result<()> {
    check_time(deadline)?;
    let mut fields = db
        .prepare("SELECT name FROM pragma_table_xinfo(?) ORDER BY cid")
        .map_err(sqlite_error)?;
    let columns: Vec<_> = fields
        .query_map([name], |row| row.get::<_, String>(0))
        .and_then(Iterator::collect)
        .map_err(sqlite_error)?;
    // Let SQLite render each value so i64, floats and blobs match Bun's SQL expressions exactly.
    let expressions = columns
        .iter()
        .map(|name| {
            let column = quote(name);
            format!(
                "CASE typeof({column})
            WHEN 'null' THEN 'n' WHEN 'integer' THEN 'i' || CAST({column} AS TEXT)
            WHEN 'real' THEN 'r' || printf('%!.26g', {column})
            WHEN 'text' THEN 't' || hex({column}) ELSE 'b' || hex({column}) END"
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut select = db
        .prepare(&format!("SELECT {expressions} FROM {}", quote(name)))
        .map_err(sqlite_error)?;
    let mut rows = select.query([]).map_err(sqlite_error)?;
    let mut digests = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite_error)? {
        check_time(deadline)?;
        let values: Vec<_> = (0..columns.len())
            .map(|i| row.get::<_, String>(i))
            .collect::<rusqlite::Result<_>>()
            .map_err(sqlite_error)?;
        digests.push(encode(&Sha256::digest(json!(values).to_string())));
    }
    check_time(deadline)?;
    hash.update(json!([name]).to_string());
    digests.sort_unstable();
    for digest in digests {
        hash.update(digest);
    }
    Ok(())
}
