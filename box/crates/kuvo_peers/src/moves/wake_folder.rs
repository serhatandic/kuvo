//! A chat's wake-up directory is paused by the move that owns it.

use std::{
    io,
    path::{Path, PathBuf},
};

use kuvo_store::files;
use serde_json::Value;

use super::{Journal, move_id};
use crate::reach::Millis;

pub(super) fn at(run: &Path, session: &str) -> io::Result<PathBuf> {
    if !super::driver::session_id(session) {
        return Err(invalid());
    }
    let home = run.parent().expect("resident has a home");
    let root = std::env::var_os("KUVO_WAKES")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("wakes"));
    Ok(root.join(session))
}

pub(super) fn original(run: &Path, row: &Journal) -> io::Result<Option<PathBuf>> {
    let Some(folder) = row.extra.get("folder").and_then(Value::as_str) else {
        return Ok(None);
    };
    let expected = at(run, &row.thing)?;
    if Path::new(folder) == expected {
        Ok(Some(expected))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The move names a different wake-up folder.",
        ))
    }
}

pub(super) fn hold(run: &Path, row: &mut Journal, now: Millis) -> io::Result<()> {
    let folder = original(run, row)?.unwrap_or(at(run, &row.thing)?);
    let Some(metadata) = metadata(&folder)? else {
        return Ok(());
    };
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The chat’s wake-up folder is unavailable.",
        ));
    }
    let paused = before(&folder)?;
    let held = paused.as_deref().and_then(owner);
    if held.is_some_and(|held| {
        held != row.id && !(held == "moving" && row.extra.contains_key("holdSince"))
    }) {
        return Err(io::Error::other(super::driver::MOVING));
    }
    if row.extra.get("pauseRecorded") != Some(&Value::Bool(true)) && held.is_none() {
        row.extra
            .insert("wasPaused".into(), paused.is_some().into());
        if let Some(paused) = paused {
            row.extra.insert("pausedAs".into(), paused.into());
        }
    }
    row.extra
        .insert("folder".into(), folder.to_str().ok_or_else(invalid)?.into());
    row.extra.insert("pauseRecorded".into(), true.into());
    row.extra.insert("holdSince".into(), now.into());
    row.save(run)?;
    files::write_atomic_through(&folder.join("paused"), marker(row).as_bytes(), &folder)
}

pub(super) fn place(run: &Path, row: &mut Journal, staged: &Path, chats: &Path) -> io::Result<()> {
    let place = at(run, &row.thing)?;
    if metadata(&place)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Its wake-up folder is on this computer already.",
        ));
    }
    files::write_atomic_through(&staged.join("paused"), marker(row).as_bytes(), staged)?;
    files::write_atomic_through(
        &staged.join("directory"),
        chats.to_str().ok_or_else(invalid)?.as_bytes(),
        staged,
    )?;
    row.extra.insert("placed".into(), true.into());
    row.save(run)?;
    files::create_private_dirs(place.parent().ok_or_else(invalid)?)?;
    files::replace_synced(staged, &place)
}

pub(super) fn release(run: &Path, row: &Journal) -> io::Result<()> {
    if !row.extra.contains_key("folder") && row.extra.get("placed") != Some(&Value::Bool(true)) {
        return Ok(());
    }
    let folder = at(run, &row.thing)?;
    let paused = before(&folder)?;
    if paused.as_deref().and_then(owner) != Some(row.id.as_str()) {
        return Ok(());
    }
    let path = folder.join("paused");
    if row.extra.get("wasPaused") == Some(&Value::Bool(true)) {
        let was = row
            .extra
            .get("pausedAs")
            .and_then(Value::as_str)
            .unwrap_or_default();
        files::write_atomic_through(&path, was.as_bytes(), &folder)
    } else {
        files::remove_file_synced(&path)
    }
}

pub(super) fn discard(run: &Path, row: &Journal) -> io::Result<()> {
    if row.extra.get("placed") != Some(&Value::Bool(true)) {
        return Ok(());
    }
    let folder = at(run, &row.thing)?;
    if before(&folder)?.as_deref().and_then(owner) != Some(row.id.as_str()) {
        return Ok(());
    }
    let home = run.parent().expect("resident has a home");
    let trash = home.join("trash").join(&row.id);
    files::create_private_dirs(&trash)?;
    files::replace_synced(&folder, &trash.join(&row.thing))
}

fn before(folder: &Path) -> io::Result<Option<String>> {
    let Some(meta) = metadata(folder)? else {
        return Ok(None);
    };
    if !meta.is_dir() {
        return Err(invalid());
    }
    let paused = folder.join("paused");
    if metadata(&paused)?.is_some_and(|meta| !meta.is_file()) {
        return Err(invalid());
    }
    files::read(&paused)?
        .map(|bytes| String::from_utf8(bytes).map_err(io::Error::other))
        .transpose()
}

fn metadata(path: &Path) -> io::Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn owner(value: &str) -> Option<&str> {
    let value = value.trim();
    if value == "moving" {
        Some(value)
    } else {
        value
            .strip_prefix("moving:")
            .filter(|id| move_id(id).is_some())
    }
}

fn marker(row: &Journal) -> String {
    format!("moving:{}", row.id)
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "The move names an invalid wake-up folder.",
    )
}
