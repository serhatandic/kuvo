use super::store::{Item, Store, file_name};
use kuvo_store::files::{self, FileStamp};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
};

pub(super) struct Download {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Read the named regular file only while its directory is still the one the queue owns.
pub(super) fn read(store: &Store, item: &Item, name: &str) -> Option<Download> {
    if !file_name(name) {
        return None;
    }
    let kept = item
        .files
        .iter()
        .find(|entry| entry["name"].as_str() == Some(name))?;
    let media_type = kept["type"].as_str()?.to_owned();
    let run = store.root.parent()?.canonicalize().ok()?;
    let queue = run.join("work-queue");
    let files = queue.join("files");
    let folder = files.join(&item.id);
    let parents = [run, queue, files, folder.clone()];
    let before = folders(&parents)?;
    let path = folder.join(name);
    if path.parent() != Some(folder.as_path()) {
        return None;
    }
    let mut file = open(&path, false).ok()?;
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
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(Download { media_type, bytes })
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
