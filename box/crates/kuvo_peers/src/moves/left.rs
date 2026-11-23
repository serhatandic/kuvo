//! Retired source backups still belong to the move that placed them in trash.

use std::{
    io,
    path::{Component, Path, PathBuf},
};

use kuvo_store::{files, format::ObjectFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Journal, Kind, read_json};
use crate::reach::Millis;

const LEFT: ObjectFormat = ObjectFormat::additive("Move cleanup entry");
pub(super) const KEEP_FOR: Millis = 7 * 24 * 3_600_000;
pub(super) const ASK_EVERY: Millis = 3_600_000;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Backup {
    pub id: String,
    pub thing: String,
    kind: Kind,
    pub to: String,
    at: Millis,
    pub sessions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ran_at: Option<Millis>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

pub(super) fn read(run: &Path) -> io::Result<Vec<Backup>> {
    let Some(Value::Array(rows)) = read_json(&run.join("moves/left.json"))? else {
        return Ok(vec![]);
    };
    rows.into_iter()
        .map(|entry| {
            let fields = LEFT.decode(entry).map_err(io::Error::other)?.fields;
            Ok(serde_json::from_value(fields.into())?)
        })
        .collect()
}

fn save(run: &Path, rows: Vec<Backup>) -> io::Result<()> {
    let path = run.join("moves/left.json");
    if rows.is_empty() {
        return files::remove_file_synced(&path);
    }
    let rows = rows
        .into_iter()
        .map(|row| {
            let Value::Object(fields) = serde_json::to_value(row)? else {
                unreachable!()
            };
            Ok(LEFT.encode(fields))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    files::write_atomic_through(&path, &serde_json::to_vec(&rows)?, run)
}

/// The intended trash entry is bound before the rename. A restart can find
/// it even when the source directory was already renamed to trash.
pub(super) fn keep(run: &Path, record: &mut Journal, backup: &Path, at: Millis) -> io::Result<()> {
    let home = run.parent().expect("resident has a home");
    let trash_root = home.join("trash");
    let target = match record.extra.get("trash").and_then(Value::as_str) {
        Some(target) => PathBuf::from(target),
        None => {
            let target = trash_root
                .join(format!("{at}-{}", std::process::id()))
                .join(
                    backup
                        .file_name()
                        .ok_or_else(|| io::Error::other("The backup has no name"))?,
                );
            record.extra.insert(
                "trash".into(),
                target
                    .to_str()
                    .ok_or_else(|| io::Error::other("The trash path is not UTF-8"))?
                    .into(),
            );
            record.save(run)?;
            target
        }
    };
    if !in_trash(&trash_root, &target) {
        return Err(io::Error::other(
            "The move backup is outside its trash folder",
        ));
    }
    let folder = target.parent().expect("trash entry has a folder");
    if backup.try_exists()? {
        files::create_private_dirs(folder)?;
        files::replace_synced(backup, &target)?;
    }
    if !target.try_exists()? {
        return Err(io::Error::other(
            "The retired original's backup went missing",
        ));
    }
    let mut rows = read(run)?;
    if rows.iter().any(|row| row.id == record.id) {
        return Ok(());
    }
    rows.push(Backup {
        id: record.id.clone(),
        thing: record.thing.clone(),
        kind: record.kind.unwrap_or(Kind::Chat),
        to: record.peer.clone(),
        at,
        sessions: record.sessions.clone(),
        trash: Some(target),
        ran_at: None,
        extra: Default::default(),
    });
    save(run, rows)
}

pub(super) fn note_ran(run: &Path, id: &str, from: &str, at: Millis) -> io::Result<()> {
    let mut rows = read(run)?;
    if let Some(row) = rows
        .iter_mut()
        .find(|row| row.id == id && row.to == from && row.ran_at.is_none())
    {
        row.ran_at = Some(at.max(row.at));
        save(run, rows)?;
    }
    Ok(())
}

pub(super) fn forget(run: &Path, id: &str) -> io::Result<()> {
    let mut rows = read(run)?;
    if let Some(index) = rows.iter().position(|row| row.id == id) {
        let row = rows.remove(index);
        save(run, rows)?;
        if let Some(path) = row.trash {
            let trash_root = run.parent().expect("resident has a home").join("trash");
            if in_trash(&trash_root, &path) {
                files::remove_tree_synced(&path)?;
                if let Some(parent) = path.parent().filter(|parent| *parent != trash_root) {
                    let _ = files::remove_empty_dir_synced(parent);
                }
            }
        }
    }
    Ok(())
}

fn in_trash(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|tail| {
        !tail.as_os_str().is_empty()
            && tail
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
    })
}
