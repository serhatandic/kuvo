use crate::platform;
use std::{
    fs::{self, Metadata},
    io::{self, Write},
    path::Path,
};
pub(crate) const DRAFT_PREFIX: &str = ".kuvo-write-";

pub(crate) fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .map(|parent| {
            if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            }
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))
}

pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(at("reading", path, error)),
    }
}

pub fn text_of(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
}

pub fn list_of(folder: &Path) -> Vec<std::ffi::OsString> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .collect::<io::Result<Vec<_>>>()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.file_name())
        .collect()
}

pub fn create_private_dirs(folder: &Path) -> io::Result<()> {
    platform::private_dirs(folder).map_err(|error| at("making private folders at", folder, error))
}

pub fn private_folder(folder: &Path) -> io::Result<()> {
    create_private_dirs(folder)?;
    let real = folder.canonicalize()?;
    if !real.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a directory", folder.display()),
        ));
    }
    platform::restrict_folder(&real).map_err(|error| at("restricting", folder, error))
}

/// Every published write seals its contents, its directory entry, and any parent
/// directories it had to create. Call this on a blocking thread for async work.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    publish(path, bytes, None, platform::replace).map_err(|error| at("replacing", path, error))
}

pub fn write_secret(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let folder = parent(path)?;
    let through = existing_ancestor(folder)?.to_owned();
    private_folder(folder)?;
    write_atomic_through(path, bytes, &through)
}

/// Atomically install a record only if no entry is already there.
pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    publish(path, bytes, None, platform::publish_new).map_err(|error| at("publishing", path, error))
}

fn publish(
    path: &Path,
    bytes: &[u8],
    through: Option<&Path>,
    operation: fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let folder = parent(path)?;
    let through = match through {
        Some(through) => {
            check_ancestor(folder, through)?;
            through
        }
        None => existing_ancestor(folder)?,
    };
    platform::private_dirs(folder)?;
    let mut temp = tempfile::Builder::new()
        .prefix(DRAFT_PREFIX)
        .tempfile_in(folder)?;
    platform::private_file(temp.as_file())?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    // Close the writable handle before replacement. TempPath cleans up the
    // draft if publishing fails, or unlinks the source of a new hard link.
    let temp = temp.into_temp_path();
    operation(&temp, path)?;
    drop(temp);
    sync_directories(folder, through)
}

fn existing_ancestor(mut folder: &Path) -> io::Result<&Path> {
    loop {
        match fs::metadata(folder) {
            Ok(meta) if meta.is_dir() => return Ok(folder),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    folder.display().to_string(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => folder = parent(folder)?,
            Err(error) => return Err(error),
        }
    }
}

pub fn replace_synced(from: &Path, to: &Path) -> io::Result<()> {
    platform::replace(from, to).map_err(|error| at("moving", from, error))?;
    sync_parent(to)?;
    if parent(from)? != parent(to)? {
        sync_parent(from)?;
    }
    Ok(())
}

pub fn sync_parent(file: &Path) -> io::Result<()> {
    let folder = parent(file)?;
    platform::sync_dir(folder).map_err(|error| at("syncing", folder, error))
}

/// Seal a staged import from its leaves to a chosen ancestor. Links are
/// preserved as directory entries and their targets are never opened.
pub fn sync_tree(root: &Path, through: &Path) -> io::Result<()> {
    let folder = parent(root)?;
    check_ancestor(folder, through)?;
    sync_entry(root, fs::symlink_metadata(root)?)?;
    sync_directories(folder, through)
}

fn sync_entry(path: &Path, meta: Metadata) -> io::Result<()> {
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            sync_entry(&entry.path(), fs::symlink_metadata(entry.path())?)?;
        }
        platform::sync_dir(path)
    } else if meta.is_file() {
        sync_file(path)
    } else if meta.file_type().is_symlink() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a file, directory or link", path.display()),
        ))
    }
}

pub fn sync_file_through(file: &Path, through: &Path) -> io::Result<()> {
    let folder = parent(file)?;
    check_ancestor(folder, through)?;
    sync_file(file)?;
    sync_directories(folder, through)
}

pub fn write_atomic_through(path: &Path, bytes: &[u8], through: &Path) -> io::Result<()> {
    publish(path, bytes, Some(through), platform::replace)
        .map_err(|error| at("replacing", path, error))
}

fn sync_file(path: &Path) -> io::Result<()> {
    platform::open_for_sync(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| at("syncing", path, error))
}

fn check_ancestor(folder: &Path, through: &Path) -> io::Result<()> {
    if !through.as_os_str().is_empty()
        && (folder.ancestors().any(|ancestor| ancestor == through)
            || (through == Path::new(".") && folder.is_relative()))
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the sync must stop at a parent of the file",
        ))
    }
}

fn sync_directories(mut folder: &Path, through: &Path) -> io::Result<()> {
    loop {
        platform::sync_dir(folder).map_err(|error| at("syncing", folder, error))?;
        if folder == through {
            return Ok(());
        }
        folder = parent(folder)?;
    }
}

pub(crate) fn at(action: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{action} {}: {error}", path.display()),
    )
}

/// Native identity and revision of an input to a resumable local index.
/// Replacing a file changes identity even when its size and mtime are kept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStamp {
    pub device: u64,
    pub file: u128,
    pub created_ns: Option<i128>,
    pub modified_ns: i128,
    pub changed_ns: i128,
    pub size: u64,
}

pub fn file_stamp(path: &Path) -> io::Result<Option<FileStamp>> {
    match platform::file_stamp(path) {
        Ok(stamp) => Ok(Some(stamp)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(at("reading the file stamp of", path, error)),
    }
}
