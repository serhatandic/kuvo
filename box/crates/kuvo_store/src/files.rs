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
    publish_prepared(path, through, operation, platform::private_file, |file| {
        file.write_all(bytes)
    })
}

/// Generate a private file without buffering it in memory. Incomplete output
/// stays in the temporary file and never replaces the destination.
pub fn write_generated(
    path: &Path,
    generate: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    publish_prepared(
        path,
        None,
        platform::replace,
        platform::private_file,
        generate,
    )
    .map_err(|error| at("generating", path, error))
}

/// Stream a private file into a new name, leaving any existing entry in place.
pub fn write_generated_new(
    path: &Path,
    generate: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    publish_prepared(
        path,
        None,
        platform::publish_new,
        platform::private_file,
        generate,
    )
    .map_err(|error| at("generating new file", path, error))
}

fn publish_prepared(
    path: &Path,
    through: Option<&Path>,
    operation: fn(&Path, &Path) -> io::Result<()>,
    prepare: impl FnOnce(&fs::File) -> io::Result<()>,
    generate: impl FnOnce(&mut fs::File) -> io::Result<()>,
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
    prepare(temp.as_file())?;
    generate(temp.as_file_mut())?;
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

/// Publish a staged Mac entry without replacing an entry that appeared at its destination.
#[cfg(target_os = "macos")]
pub fn place_mac_entry(from: &Path, to: &Path) -> io::Result<()> {
    platform::place_exclusive(from, to).map_err(|error| at("placing", to, error))?;
    sync_parent(to)?;
    if parent(from)? != parent(to)? {
        sync_parent(from)?;
    }
    Ok(())
}

/// Retire a file or link without following it. Missing entries are already retired.
pub fn remove_file_synced(path: &Path) -> io::Result<()> {
    remove_synced(path, |path| fs::remove_file(path))
}

/// Retire a directory tree without following its links.
pub fn remove_tree_synced(path: &Path) -> io::Result<()> {
    remove_synced(path, platform::remove_tree)
}

pub fn remove_empty_dir_synced(path: &Path) -> io::Result<()> {
    remove_synced(path, |path| fs::remove_dir(path))
}

fn remove_synced(path: &Path, remove: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    match remove(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(at("retiring", path, error)),
    }
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

/// Flush a directory and its ancestors using the platform's directory policy.
pub fn sync_folder_through(folder: &Path, through: &Path) -> io::Result<()> {
    check_ancestor(folder, through)?;
    sync_directories(folder, through)
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

/// Open a managed payload without following its leaf or reading a file shared
/// with another path through a hard link.
pub fn open_payload(path: &Path) -> io::Result<fs::File> {
    platform::open_payload(path).map_err(|error| at("opening managed file", path, error))
}

pub fn opened_file_stamp(file: &fs::File) -> io::Result<FileStamp> {
    platform::opened_file_stamp(file)
}

/// Copy a legacy Mac entry into private staging, preserving modes and links.
/// The source is left in place. The caller owns the unpublished staging tree.
#[cfg(target_os = "macos")]
pub fn clone_mac_entry(from: &Path, to: &Path, allow_copy: bool) -> io::Result<()> {
    use std::process::{Command, Stdio};

    create_private_dirs(parent(to)?)?;
    let copy = |options| {
        Command::new("/bin/cp")
            .env_clear()
            .arg(options)
            .arg(from)
            .arg(to)
            .stdin(Stdio::null())
            .output()
    };
    let mut output = copy("-cRPp")?;
    if !output.status.success() && allow_copy {
        remove_tree_synced(to)?;
        output = copy("-RPp")?;
    }
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "staging {}: {}",
            from.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Rewrite a regular file in a private Mac staging tree with its existing mode.
#[cfg(target_os = "macos")]
pub fn rewrite_mac_entry(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let entry = fs::symlink_metadata(path)?;
    if !entry.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular staged file",
        ));
    }
    let mode = entry.permissions().mode() & 0o777;
    publish_prepared(
        path,
        None,
        platform::replace,
        |file| file.set_permissions(fs::Permissions::from_mode(mode)),
        |file| file.write_all(bytes),
    )
    .map_err(|error| at("rewriting", path, error))
}

/// Replace the target of a symlink in a private Mac staging tree.
#[cfg(target_os = "macos")]
pub fn repoint_mac_link(path: &Path, target: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a staged symlink",
        ));
    }
    let draft = tempfile::Builder::new()
        .prefix(DRAFT_PREFIX)
        .tempdir_in(parent(path)?)?;
    let link = draft.path().join("link");
    std::os::unix::fs::symlink(target, &link).map_err(|error| at("repointing", path, error))?;
    replace_synced(&link, path)?;
    draft.close()?;
    sync_parent(path)
}

/// Seal a byte range in a private staging file. The caller serializes access
/// and publishes progress separately, after this write has reached disk.
pub fn write_staged_range(path: &Path, offset: u64, bytes: &[u8]) -> io::Result<()> {
    use std::io::{Seek, SeekFrom};

    let write = || {
        let mut file = open_staged(path)?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    write().map_err(|error| at("staging bytes in", path, error))
}

/// Seal a line in a private append-only staging journal. An interrupted
/// write can leave at most a partial last line, which the next append drops.
pub fn append_staged_line(path: &Path, complete_length: u64, line: &[u8]) -> io::Result<()> {
    use std::io::{Seek, SeekFrom};

    let write = || {
        let mut file = open_staged(path)?;
        let length = file.metadata()?.len();
        if length < complete_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staging journal was shortened",
            ));
        }
        if length > complete_length {
            file.set_len(complete_length)?;
        }
        file.seek(SeekFrom::Start(complete_length))?;
        file.write_all(line)?;
        file.sync_all()
    };
    write().map_err(|error| at("adding a staging journal entry to", path, error))
}

fn open_staged(path: &Path) -> io::Result<fs::File> {
    match fs::symlink_metadata(path) {
        Ok(entry) if entry.is_file() => fs::OpenOptions::new().write(true).open(path),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular staging file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_new(path, &[])?;
            fs::OpenOptions::new().write(true).open(path)
        }
        Err(error) => Err(error),
    }
}
