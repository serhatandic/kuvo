use super::{BESIDE, preferences, process_record};
use crate::{
    Disk, Error, ValueWithPid, check_writers, entries, json_at, metadata, require_stopped,
};
use kuvo_store::{files, format};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
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

pub(super) fn no_open_database(source: &Path) -> Result<(), Error> {
    let folder = source.join("opencode/data/opencode");
    let mut databases = Vec::new();
    for entry in entries(&folder)? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_file()
            && name.starts_with("opencode")
            && (name.ends_with(".db") || name.ends_with(".db-wal") || name.ends_with(".db-shm"))
        {
            databases.push(entry.path());
        }
    }
    if databases.is_empty() {
        return Ok(());
    }
    let result = Command::new("/usr/sbin/lsof")
        .env_clear()
        .args(["-n", "-P", "-F", "fa", "--"])
        .args(databases)
        .output()?;
    if String::from_utf8_lossy(&result.stdout)
        .lines()
        .any(|line| matches!(line, "aw" | "au"))
    {
        return Err(super::invalid(
            "Your sessions are still open in another program on this Mac. Quit Kuvo Code wherever it's open, then try again.",
        ));
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
    check_writers(disk)?;
    for record in ["run/engine.json", "run/box-services.json"] {
        let record = disk.path(record)?;
        let pid = json_at::<ValueWithPid>(&record)?.and_then(|value| value.pid);
        require_stopped(record, pid)?;
    }
    let keeper = disk.path("run/box.pid")?;
    let pid =
        files::read(&keeper)?.and_then(|bytes| String::from_utf8_lossy(&bytes).trim().parse().ok());
    require_stopped(keeper, pid)
}
