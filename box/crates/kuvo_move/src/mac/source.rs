use super::{BESIDE, preferences, process_record};
use crate::{
    Disk, Error, ValueWithPid, check_writers, entries, json_at, metadata, require_stopped,
};
use kuvo_store::files;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(PartialEq, Eq)]
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
    for file in [
        "agent-letters.json",
        "agent-messages.json",
        "agent-letters-seen.json",
    ] {
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
