use super::{engine, invalid, paths::PathMap};
use crate::Error;
use kuvo_store::sqlite;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
    time::Duration,
};

const PATHS: &[(&str, &str)] = &[
    ("project", "worktree"),
    ("session_v2", "directory"),
    ("worktree", "directory"),
    ("project_directory", "directory"),
    ("permission", "resource"),
];
const STRUCTURED: &[(&str, &str)] = &[
    ("project", "sandboxes"),
    ("session_v2", "revert"),
    ("session_v2", "summary_diffs"),
    ("session_v2", "permission"),
    ("session_v2", "metadata"),
    ("workspace", "binding"),
];
const ATTACHMENTS: &[(&str, &str)] = &[("session_message", "data"), ("session_inbox", "payload")];
const PROJECT_IDS: &[(&str, &str)] = &[
    ("session_v2", "project_id"),
    ("permission", "project_id"),
    ("worktree", "project_id"),
    ("project_directory", "project_id"),
    ("event_sequence", "aggregate_id"),
    ("event", "aggregate_id"),
];

#[derive(Default)]
pub(super) struct Rekey {
    pub projects: Vec<(String, String)>,
    pub snapshots: BTreeSet<(String, String)>,
}

pub(super) fn rewrite(path: &Path, map: &PathMap) -> Result<Rekey, Error> {
    let mut db = sqlite::open_for_move(path, Duration::from_secs(3))?;
    let Some((count, last)) = engine::schema(&db)? else {
        return Ok(Rekey::default());
    };
    if (count, last.as_str()) != engine::PINNED {
        return Err(invalid("Only a pinned engine database can be rewritten."));
    }
    db.pragma_update(None, "foreign_keys", false)?;
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let tables: HashSet<_> = transaction
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    let broken_before = broken_links(&transaction)?;
    let snapshots = source_folders(&transaction, &tables, map)?;
    let projects = project_ids(&transaction, &tables, map)?;
    for &(table, column) in PATHS {
        if tables.contains(table) {
            column_rows(&transaction, table, column, |value| map.absolute(value))?;
        }
    }
    for (columns, attachments_only) in [(STRUCTURED, false), (ATTACHMENTS, true)] {
        for &(table, column) in columns {
            if tables.contains(table) {
                column_rows(&transaction, table, column, |text| {
                    let mut value = serde_json::from_str(text).ok()?;
                    let changed = if attachments_only {
                        map_attachments(&mut value, map)
                    } else {
                        map.json(&mut value)
                    };
                    changed.then(|| value.to_string())
                })?;
            }
        }
    }
    if !broken_links(&transaction)?.is_subset(&broken_before) {
        return Err(invalid("Rewriting the staged database broke foreign keys."));
    }
    transaction.commit()?;
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(Rekey {
        projects,
        snapshots,
    })
}

pub(super) fn leftovers(path: &Path, map: &PathMap) -> Result<Vec<String>, Error> {
    let db = sqlite::open_index(path, Duration::from_secs(3))?;
    let mut notes = Vec::new();
    let mut tables =
        db.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")?;
    for table in tables.query_map([], |row| row.get::<_, String>(0))? {
        let table = table?;
        let quoted = table.replace('"', "\"\"");
        let mut columns = db.prepare(&format!("PRAGMA table_info(\"{quoted}\")"))?;
        for column in columns.query_map([], |row| row.get::<_, String>(1))? {
            let column = column?;
            let name = column.replace('"', "\"\"");
            let mut query = db.prepare(&format!(
                "SELECT \"{name}\" FROM \"{quoted}\" WHERE typeof(\"{name}\") = 'text'"
            ))?;
            let mut rows = query.query([])?;
            let mut mentioned = 0;
            while let Some(row) = rows.next()? {
                let text: String = row.get(0)?;
                if !map.names_old(&text) {
                    continue;
                }
                let field = (table.as_str(), column.as_str());
                let missed = if PATHS.contains(&field) {
                    map.absolute(&text).is_some()
                } else if STRUCTURED.contains(&field) || ATTACHMENTS.contains(&field) {
                    serde_json::from_str::<Value>(&text)
                        .ok()
                        .is_some_and(|mut value| {
                            if ATTACHMENTS.contains(&field) {
                                map_attachments(&mut value, map)
                            } else {
                                map.json(&mut value)
                            }
                        })
                } else {
                    false
                };
                if missed {
                    return Err(invalid(&format!(
                        "The staged database still names its old folder in {table}.{column}."
                    )));
                }
                mentioned += 1;
            }
            if mentioned > 0 {
                let file = path.file_name().unwrap_or_default().to_string_lossy();
                notes.push(format!("{file}: {table}.{column} ({mentioned})"));
            }
        }
    }
    Ok(notes)
}

type BrokenLink = (String, Option<i64>, String, i64);

fn broken_links(db: &Connection) -> rusqlite::Result<HashSet<BrokenLink>> {
    db.prepare("PRAGMA foreign_key_check")?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect()
}

fn folder_id(path: &str) -> String {
    hex::encode(Sha1::digest(format!("directory:{path}")))
}

fn source_folders(
    db: &Connection,
    tables: &HashSet<String>,
    map: &PathMap,
) -> Result<BTreeSet<(String, String)>, Error> {
    let mut folders = BTreeSet::new();
    for (table, id, path) in [
        ("project", "id", "worktree"),
        ("worktree", "project_id", "directory"),
        ("project_directory", "project_id", "directory"),
        ("session_v2", "project_id", "directory"),
    ] {
        if !tables.contains(table) {
            continue;
        }
        let mut query = db.prepare(&format!("SELECT \"{id}\", \"{path}\" FROM \"{table}\""))?;
        for row in query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })? {
            let (id, path) = row?;
            if let Some(path) = path
                && map.absolute(&path).is_some()
            {
                folders.insert((id, path));
            }
        }
    }
    Ok(folders)
}

fn project_ids(
    db: &Connection,
    tables: &HashSet<String>,
    map: &PathMap,
) -> Result<Vec<(String, String)>, Error> {
    if !tables.contains("project") {
        return Ok(Vec::new());
    }
    let mut projects = db.prepare("SELECT id, worktree FROM project")?;
    let mut moved = Vec::new();
    for row in projects.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })? {
        let (id, path) = row?;
        if let Some(path) = path
            && let Some(to) = map.absolute(&path)
            && folder_id(&path) == id
        {
            moved.push((id, folder_id(&to)));
        }
    }
    for (from, to) in &moved {
        db.execute("UPDATE project SET id = ? WHERE id = ?", [to, from])?;
        for &(table, column) in PROJECT_IDS {
            if tables.contains(table) {
                db.execute(
                    &format!("UPDATE \"{table}\" SET \"{column}\" = ? WHERE \"{column}\" = ?"),
                    [to, from],
                )?;
            }
        }
    }
    Ok(moved)
}

fn column_rows(
    db: &Connection,
    table: &str,
    column: &str,
    map: impl Fn(&str) -> Option<String>,
) -> rusqlite::Result<()> {
    let mut ids = db.prepare(&format!(
        "SELECT rowid FROM \"{table}\" WHERE (?1 IS NULL OR rowid > ?1)
         AND typeof(\"{column}\") = 'text' ORDER BY rowid LIMIT 128"
    ))?;
    let mut read = db.prepare(&format!(
        "SELECT \"{column}\" FROM \"{table}\" WHERE rowid = ?"
    ))?;
    let mut write = db.prepare(&format!(
        "UPDATE \"{table}\" SET \"{column}\" = ? WHERE rowid = ?"
    ))?;
    let mut after: Option<i64> = None;
    loop {
        let batch: Vec<i64> = ids
            .query_map([after], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let Some(last) = batch.last().copied() else {
            break;
        };
        for id in batch {
            let value: String = read.query_row([id], |row| row.get(0))?;
            if let Some(mapped) = map(&value) {
                write.execute(rusqlite::params![mapped, id])?;
            }
        }
        after = Some(last);
    }
    Ok(())
}

fn map_attachments(value: &mut Value, map: &PathMap) -> bool {
    let mut changed = false;
    if let Some(files) = value.get_mut("files").and_then(Value::as_array_mut) {
        for file in files {
            changed |= map_uri(file, map);
            if let Some(source) = file.get_mut("source") {
                changed |= map_uri(source, map);
            }
        }
    }
    changed
}

fn map_uri(value: &mut Value, map: &PathMap) -> bool {
    let Some(value) = value.get_mut("uri") else {
        return false;
    };
    let Some(mapped) = value.as_str().and_then(|uri| map.uri(uri)) else {
        return false;
    };
    *value = mapped.into();
    true
}
