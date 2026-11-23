//! Undo a predecessor's private file placements before it cleared its source.
use super::{ACTS, JOURNAL, STAGING, STATE, now, stamp, write_json};
use crate::{Disk, Error, json_at, metadata};
use kuvo_store::{files, format::ObjectFormat, layout};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize)]
struct Record {
    v: u32,
    way: String,
    step: String,
    from: PathBuf,
    to: PathBuf,
    began: u64,
    run: Option<Value>,
    placements: Vec<Placement>,
}

#[derive(Deserialize)]
struct Placement {
    staged: PathBuf,
    to: PathBuf,
}

pub(super) fn previous(disk: &Disk, account: &Path) -> Result<Option<PathBuf>, Error> {
    let journal = disk.path(JOURNAL)?;
    if !metadata(&journal)?.is_some_and(|file| file.is_file()) {
        return Ok(None);
    }
    let Some(record) = json_at::<Record>(&journal).ok().flatten() else {
        return Ok(None);
    };
    if record.v != 1
        || record.way != "move"
        || !can_undo(&record.step)
        || record.to != disk.0
        || record.from.parent() != Some(account.join("Library/Application Support").as_path())
        || record
            .from
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_none_or(|id| !id.starts_with("run.kuvo.code"))
    {
        return Ok(None);
    }
    Ok(Some(record.from))
}

fn can_undo(step: &str) -> bool {
    matches!(
        step,
        "plan"
            | "install"
            | "keys"
            | "clone"
            | "upgrade"
            | "database"
            | "rewrite"
            | "check"
            | "place"
            | "fix"
    )
}

#[derive(Deserialize)]
#[serde(tag = "act", rename_all = "lowercase")]
enum Action {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Made {
        path: PathBuf,
        #[serde(default)]
        folder: bool,
    },
    Wrote,
    Keychain,
}

/// The old journal has no file identities for external writes. Limit replay
/// here to its home and the private staging and archive directories.
pub(super) fn undo(
    disk: &Disk,
    source: &Path,
    layout: layout::Layout,
) -> Result<Option<String>, Error> {
    let path = disk.path(JOURNAL)?;
    if !metadata(&path)?.is_some_and(|entry| entry.is_file()) {
        return Ok(None);
    }
    let Some(record) = json_at::<Record>(&path).ok().flatten() else {
        return Ok(None);
    };
    if record.v != 1
        || record.way != "move"
        || record.from != source
        || record.to != disk.0
        || !can_undo(&record.step)
        || metadata(&disk.path("updates/rehome/keychain.json")?)?.is_some()
    {
        return Ok(None);
    }

    let acts = disk.path(ACTS)?;
    if metadata(&acts)?.is_some_and(|entry| !entry.is_file()) {
        return Ok(None);
    }
    let bytes = files::read(&acts)?.unwrap_or_default();
    let mut actions = Vec::new();
    let format = ObjectFormat::additive("Rehome journal action");
    for line in bytes.split(|byte| *byte == b'\n') {
        // A torn append never preceded a filesystem change.
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let Some(fields) = format.decode(value).ok().map(|record| record.fields) else {
            return Ok(None);
        };
        let Ok(action) = serde_json::from_value::<Action>(Value::Object(fields)) else {
            return Ok(None);
        };
        actions.push(action);
    }

    let stage = disk.0.join(STAGING);
    let before = disk
        .0
        .join("trash")
        .join(format!("earlier-box-{}", stamp(record.began)?));
    let old_layout = disk.0.join(layout::FILE);
    let marker = stage.join("work").join(layout::FILE);
    if !matches!(layout, layout::Layout::Unmarked | layout::Layout::Older { version: 0 })
        && !(layout == layout::Layout::Current && actions.iter().any(|action| {
            matches!(action, Action::Rename { from, to } if from == &marker && to == &old_layout)
        }))
    {
        return Ok(None);
    }

    for action in &actions {
        match action {
            Action::Rename { from, to }
                if (record
                    .placements
                    .iter()
                    .enumerate()
                    .any(|(index, planned)| {
                        planned_rename(disk, &stage, index, planned, from, to)
                    }))
                    || (from == &marker && to == &old_layout)
                    || (ordinary_home_path(disk, from)
                        && from
                            .strip_prefix(&disk.0)
                            .is_ok_and(|name| to == &before.join(name)))
                    || (from == &disk.0.join("updates/rehome/new")
                        && to == &before.join("new-box-mark")) =>
            {
                for path in [from, to] {
                    checked(disk, path)?;
                }
            }
            Action::Made { path, folder: true } if ordinary_home_path(disk, path) => {
                checked(disk, path)?;
            }
            Action::Made {
                path,
                folder: false,
            } if path == &disk.0.join("box/kind") || path == &disk.0.join("box/work.json") => {
                checked(disk, path)?;
            }
            _ => return Ok(None),
        }
    }
    if !super::source::recorded_workers(disk)?.is_empty()
        || super::processes::no_open_writers(&disk.0).is_err()
    {
        return Ok(None);
    }

    let state_path = disk.path(STATE)?;
    let old_state = ObjectFormat::additive("Rehome state").read(&state_path)?;
    let displacements: std::collections::BTreeMap<_, _> = actions
        .iter()
        .filter_map(|action| match action {
            Action::Rename { from, to } if to.starts_with(&before) => Some((from, to)),
            _ => None,
        })
        .collect();
    for action in actions.iter().rev() {
        match action {
            Action::Rename { from, to } => {
                let original = checked(disk, from)?;
                let moved = checked(disk, to)?;
                if from.starts_with(&stage)
                    && let Some(kept) = displacements.get(to)
                    && metadata(&checked(disk, kept)?)?.is_none()
                {
                    // The earlier installation's own file is already in place.
                    continue;
                }
                if metadata(&original)?.is_none() && metadata(&moved)?.is_some() {
                    files::create_private_dirs(
                        original.parent().expect("a journal entry has a parent"),
                    )?;
                    files::place_mac_entry(&moved, &original)?;
                }
            }
            Action::Made { path, folder: true } => {
                let path = checked(disk, path)?;
                if let Err(error) = files::remove_empty_dir_synced(&path)
                    && error.kind() != io::ErrorKind::DirectoryNotEmpty
                {
                    return Err(error.into());
                }
            }
            Action::Made {
                path,
                folder: false,
            } => {
                let path = checked(disk, path)?;
                if metadata(&path)?.is_some_and(|entry| entry.is_file()) {
                    let now = fs::read(&path)?;
                    let made = if path.ends_with("box/kind") {
                        now == b"mac\n"
                    } else {
                        serde_json::from_slice::<Value>(&now)
                            .is_ok_and(|value| value == serde_json::json!({"keepsAwake":false}))
                    };
                    if !made {
                        return Err(Error::Busy(path));
                    }
                    files::remove_file_synced(&path)?;
                }
            }
            Action::Wrote | Action::Keychain => unreachable!("external actions were refused"),
        }
    }
    let old_state = old_state.map(|record| Value::Object(record.fields));
    let message = record_undo(&state_path, old_state, &record)?;
    files::remove_file_synced(&acts)?;
    files::remove_tree_synced(&disk.path(STAGING)?)?;
    files::remove_file_synced(&path)?;
    Ok(Some(message))
}

fn planned_rename(
    disk: &Disk,
    stage: &Path,
    index: usize,
    plan: &Placement,
    from: &Path,
    to: &Path,
) -> bool {
    let Ok(relative) = plan.to.strip_prefix(&disk.0) else {
        return false;
    };
    if relative.components().count() != 1 || !ordinary_home_path(disk, &plan.to) {
        return false;
    }
    let Some(name) = relative.to_str() else {
        return false;
    };
    if plan.staged != stage.join(format!("{index}-{name}")) {
        return false;
    }
    let Ok(child) = from.strip_prefix(&plan.staged) else {
        return false;
    };
    let placed = if child.as_os_str().is_empty() {
        plan.to.clone()
    } else {
        plan.to.join(child)
    };
    to == placed || numbered_sibling(&placed, to)
}

fn numbered_sibling(file: &Path, candidate: &Path) -> bool {
    if file.parent() != candidate.parent() || file.extension() != candidate.extension() {
        return false;
    }
    let Some(name) = file.file_stem().and_then(std::ffi::OsStr::to_str) else {
        return false;
    };
    candidate
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|sibling| sibling.strip_prefix(name))
        .and_then(|suffix| suffix.strip_prefix(' '))
        .and_then(|suffix| suffix.parse::<u64>().ok())
        .is_some_and(|number| number > 1)
}

fn record_undo(path: &Path, state: Option<Value>, move_record: &Record) -> Result<String, Error> {
    let mut record = state.unwrap_or_else(|| json!({}));
    let at = now();
    let standalone = move_record.run.is_some();
    let message = if standalone {
        "Kuvo was stopped while moving your sessions, and left everything as it was.".to_owned()
    } else {
        format!(
            "It stopped part way ({}), so it went back.",
            move_record.step
        )
    };
    if record["counted"] != move_record.began {
        record["tries"] = json!(
            record["tries"]
                .as_u64()
                .unwrap_or_default()
                .saturating_add(1)
        );
    }
    record["state"] = json!("failed");
    record["way"] = json!("move");
    record["nextTry"] = json!((!standalone).then_some(at + 3_600_000));
    record["asked"] = Value::Null;
    record["failed"] = json!({"at":at,"step":move_record.step,"message":message});
    record["counted"] = json!(move_record.began);
    record["formatVersion"] = json!(1);
    write_json(path, &record, true)?;
    Ok(message)
}

fn checked(disk: &Disk, path: &Path) -> Result<PathBuf, Error> {
    let relative = path
        .strip_prefix(&disk.0)
        .ok()
        .filter(|path| {
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        })
        .ok_or_else(|| super::invalid("An older journal has a path outside the move."))?;
    Ok(disk.path(relative)?)
}

fn ordinary_home_path(disk: &Disk, path: &Path) -> bool {
    let Ok(name) = path.strip_prefix(&disk.0) else {
        return false;
    };
    let Some(Component::Normal(first)) = name.components().next() else {
        return false;
    };
    first != "desktop"
        && !first.to_string_lossy().starts_with(".folder-upgrade.lock")
        && !name.starts_with("updates/rehome")
        && !name.starts_with("updates/install")
        && name != Path::new("updates")
}
