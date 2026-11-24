use super::store::{Item, Store, file_name};
use kuvo_store::files::{self, FileStamp};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
};

pub(super) mod staging;

pub(super) struct Download {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Read the named regular file only while its directory is still the one the queue owns.
pub(super) fn read(store: &Store, item: &Item, name: &str) -> Option<Download> {
    let kept = item
        .files
        .iter()
        .find(|entry| entry["name"].as_str() == Some(name))?;
    let media_type = kept["type"].as_str()?.to_owned();
    let mut file = open_saved(store, &item.id, name)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(Download { media_type, bytes })
}

pub(super) fn open_saved(store: &Store, item: &str, name: &str) -> Option<File> {
    if !super::store::item_id(item) || !file_name(name) {
        return None;
    }
    let run = store.root.parent()?.canonicalize().ok()?;
    let queue = run.join("work-queue");
    let files = queue.join("files");
    let folder = files.join(item);
    let parents = [run, queue, files, folder.clone()];
    let before = folders(&parents)?;
    let path = folder.join(name);
    if path.parent() != Some(folder.as_path()) {
        return None;
    }
    let file = open(&path, false).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || is_link(&metadata) {
        return None;
    }
    let opened = files::opened_file_stamp(&file).ok()?;
    let saved = open(&path, false).ok()?;
    let saved_metadata = saved.metadata().ok()?;
    if !saved_metadata.is_file()
        || is_link(&saved_metadata)
        || files::opened_file_stamp(&saved).ok()? != opened
        || folders(&parents)? != before
    {
        return None;
    }
    Some(file)
}

pub(super) fn place_handoff(
    store: &Store,
    item: &str,
    carried: &std::collections::BTreeMap<String, Vec<u8>>,
) -> io::Result<()> {
    let run = store
        .root
        .parent()
        .expect("Work Queue under run")
        .canonicalize()?;
    let queue = run.join("work-queue");
    let parent = queue.join("files");
    for path in [&queue, &parent] {
        if path == &parent {
            files::create_private_dirs(path)?;
        }
        let info = open(path, true)?.metadata()?;
        if !info.is_dir() || is_link(&info) {
            return Err(io::Error::other("Invalid Work Queue attachment directory."));
        }
    }
    let folder = parent.join(item);
    files::remove_tree_synced(&folder)?;
    for (name, bytes) in carried {
        files::write_atomic_through(&folder.join(name), bytes, &run)?;
    }
    Ok(())
}

pub(super) fn remove(store: &Store, id: &str, names: &[String]) -> io::Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let run = store
        .root
        .parent()
        .expect("Work Queue under run")
        .canonicalize()?;
    let queue = run.join("work-queue");
    let files_dir = queue.join("files");
    let item = files_dir.join(id);
    let paths = [run, queue, files_dir, item.clone()];
    if super::store::item_id(id) && folders(&paths).is_some() {
        for name in names.iter().filter(|name| file_name(name)) {
            let path = item.join(name);
            if path.parent() == Some(item.as_path()) {
                files::remove_file_synced(&path)?;
            }
        }
    }
    Ok(())
}

pub(super) fn move_files_root(store: &Store, create: bool) -> io::Result<Option<PathBuf>> {
    let run = store
        .root
        .parent()
        .expect("Work Queue under run")
        .canonicalize()?;
    let queue = run.join("work-queue");
    let parent = queue.join("files");
    let queue_info = open(&queue, true)?.metadata()?;
    if !queue_info.is_dir() || is_link(&queue_info) {
        return Err(io::Error::other("Invalid Work Queue attachment directory."));
    }
    if create {
        files::create_private_dirs(&parent)?;
    }
    let folder = match open(&parent, true) {
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(None),
        other => other?,
    };
    let info = folder.metadata()?;
    if !info.is_dir() || is_link(&info) {
        return Err(io::Error::other("Invalid Work Queue attachment directory."));
    }
    Ok(Some(parent))
}

pub(super) fn verify_handoff_file(
    store: &Store,
    item: &str,
    name: &str,
    size: u64,
    digest: &str,
) -> io::Result<()> {
    use sha2::{Digest, Sha256};
    let run = store
        .root
        .parent()
        .expect("Work Queue under run")
        .canonicalize()?;
    let queue = run.join("work-queue");
    let files_root = queue.join("files");
    let folder = files_root.join(item);
    let root = [run, queue, files_root, folder.clone()];
    let checked =
        folders(&root).ok_or_else(|| io::Error::other("Invalid Work Queue attachment folder."))?;
    let file = folder.join(name);
    let reader = files::open_payload(&file)?;
    let before = files::opened_file_stamp(&reader)?;
    let mut hash = Sha256::new();
    let read = io::copy(&mut reader.take(size.saturating_add(1)), &mut hash)?;
    if read != size
        || before.size != size
        || files::file_stamp(&file)?.as_ref() != Some(&before)
        || folders(&root).as_ref() != Some(&checked)
        || kuvo_wire::keys::encode(&hash.finalize()) != digest
    {
        return Err(io::Error::other(format!(
            "A Work Queue attachment no longer matches its recorded bytes: {name}."
        )));
    }
    Ok(())
}

fn folders(paths: &[PathBuf; 4]) -> Option<Vec<FileStamp>> {
    paths
        .iter()
        .map(|path| {
            let folder = open(path, true).ok()?;
            let info = folder.metadata().ok()?;
            if !info.is_dir() || is_link(&info) {
                return None;
            }
            files::opened_file_stamp(&folder).ok()
        })
        .collect()
}

#[cfg(unix)]
fn is_link(metadata: &Metadata) -> bool {
    metadata.is_symlink()
}

#[cfg(windows)]
fn is_link(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(unix)]
fn open(path: &Path, directory: bool) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let flags = libc::O_NONBLOCK | libc::O_NOFOLLOW;
    let flags = if directory {
        flags | libc::O_DIRECTORY
    } else {
        flags
    };
    OpenOptions::new().read(true).custom_flags(flags).open(path)
}

#[cfg(windows)]
fn open(path: &Path, directory: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    };
    let mut options = OpenOptions::new();
    if directory {
        options
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    } else {
        options
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}
