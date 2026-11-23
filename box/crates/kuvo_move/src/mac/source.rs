use super::{BESIDE, preferences, process_record};
use crate::{Disk, Error, entries, metadata};
use kuvo_store::{files, format};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

const LEGACY_INPUTS: &[&str] = &[
    "bots.json",
    "groups.json",
    "personalities.json",
    "personalities-removed.json",
    "agent-messages.json",
    "agent-letters.json",
    "agent-letters-seen.json",
    "phone-link.json",
];

pub(super) fn validate_legacy(source: &Path) -> Result<(), Error> {
    let disk = Disk(source.into());
    for name in LEGACY_INPUTS {
        validate_record(&disk.path(name)?)?;
    }
    for bot in entries(&disk.path("bots")?)? {
        if bot.file_type()?.is_dir() {
            validate_record(&bot.path().join("bot.json"))?;
        }
    }
    for chat in entries(&disk.path("groups")?)? {
        if chat.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            validate_record(&chat.path())?;
        }
    }
    Ok(())
}

fn validate_record(file: &Path) -> Result<(), Error> {
    if !metadata(file)?.is_some_and(|meta| meta.is_file()) {
        return Ok(());
    }
    let Some(value) = files::read(file)?.and_then(|bytes| serde_json::from_slice(&bytes).ok())
    else {
        return Ok(());
    };
    match value {
        value @ serde_json::Value::Array(_) => {
            format::read_additive_list("Legacy Mac import list", value)?;
        }
        value @ serde_json::Value::Object(_) => {
            format::ObjectFormat::additive("Legacy Mac import object").decode(value)?;
        }
        _ => {}
    }
    Ok(())
}

#[derive(PartialEq, Eq, Serialize)]
pub(super) enum Entry {
    Directory,
    File {
        device: u64,
        inode: u64,
        size: u64,
        written: SystemTime,
    },
    Link(PathBuf),
}

pub(super) fn fingerprint(source: &Path) -> Result<String, Error> {
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, &stamp(source)?).map_err(std::io::Error::other)?;
    Ok(hex::encode(hash.finalize()))
}

/// Catch both new and removed files, including a SQLite WAL written during
/// the clone. SQLite's rebuildable read cache does not count as new work.
pub(super) fn stamp(source: &Path) -> Result<BTreeMap<PathBuf, Entry>, Error> {
    let mut tree = BTreeMap::new();
    for entry in entries(&source.join("box"))? {
        let path = entry.path();
        if entry.file_name() == "run" {
            tree.insert(path.clone(), Entry::Directory);
            for run in entries(&path)? {
                let name = run.file_name();
                if !process_record(&name.to_string_lossy()) {
                    walk(&run.path(), &mut tree)?;
                }
            }
        } else {
            walk(&path, &mut tree)?;
        }
    }
    for (relative, _) in BESIDE {
        walk(&source.join(relative), &mut tree)?;
    }
    for file in LEGACY_INPUTS {
        walk(&source.join(file), &mut tree)?;
    }
    walk(&preferences::preference_path(source)?, &mut tree)?;
    Ok(tree)
}

fn walk(path: &Path, tree: &mut BTreeMap<PathBuf, Entry>) -> Result<(), Error> {
    let Some(info) = metadata(path)? else {
        return Ok(());
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if name.ends_with(".db-shm") || (name.ends_with(".db-wal") && info.len() == 0) {
        return Ok(());
    }
    let entry = if info.is_dir() {
        for entry in entries(path)? {
            walk(&entry.path(), tree)?;
        }
        Entry::Directory
    } else if info.is_symlink() {
        Entry::Link(fs::read_link(path)?)
    } else if info.is_file() {
        Entry::File {
            device: info.dev(),
            inode: info.ino(),
            size: info.len(),
            written: info.modified()?,
        }
    } else {
        return Ok(());
    };
    tree.insert(path.into(), entry);
    Ok(())
}

pub(super) fn require_quiet(disk: &Disk) -> Result<(), Error> {
    let active = recorded_workers(disk)?;
    if !active.is_empty() {
        return Err(super::invalid(&format!(
            "An earlier Kuvo Code is still running in the background on this Mac ({}) and didn't stop when asked. Restart this Mac, then try again.",
            active.join(", ")
        )));
    }
    Ok(())
}

pub(super) fn recorded_workers(disk: &Disk) -> Result<Vec<&'static str>, Error> {
    use kuvo_os::process::{ProcessIdentity, ProcessState};

    let mut active = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (file, role, label) in [
        ("engine.json", "opencode", "its engine"),
        ("bridge.json", "kuvo-agents", "its coding agents' bridge"),
        ("serve.json", "kuvo-link", "its server"),
        (
            "box-services.json",
            "kuvo-link.js box-services",
            "its box services",
        ),
        ("link.pid", "kuvo-link", "its link"),
        ("box.pid", "kuvo-start", "its keeper"),
    ] {
        let path = disk.path(Path::new("run").join(file))?;
        let Some(bytes) = files::read(&path)? else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let pid: Option<u32> = if let Ok(pid) = text.trim().parse() {
            Some(pid)
        } else if let Ok(value @ serde_json::Value::Object(_)) = serde_json::from_slice(&bytes) {
            let record =
                format::ObjectFormat::additive("Local process rendezvous").decode(value)?;
            record
                .fields
                .get("pid")
                .and_then(serde_json::Value::as_u64)
                .and_then(|pid| u32::try_from(pid).ok())
        } else {
            None
        };
        let Some(pid) = pid.filter(|pid| *pid > 1 && !seen.contains(pid)) else {
            continue;
        };
        if let Some(owner) = ProcessIdentity::read(pid)?
            && super::processes::command_line(pid)?.contains(role)
            && owner.state() == ProcessState::Live
        {
            active.push(label);
            seen.insert(pid);
        }
    }
    Ok(active)
}
