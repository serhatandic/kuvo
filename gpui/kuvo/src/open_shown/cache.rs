use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use kuvo_box::shown_files::ShownFile;
use sha2::{Digest, Sha256};

use super::write_received;

pub(super) static LOCK: Mutex<()> = Mutex::new(());
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CAPACITY: u64 = 250 * 1024 * 1024;

pub(super) fn put(root: &Path, shown: &ShownFile, bytes: &[u8]) -> io::Result<PathBuf> {
    let session = shown
        .session
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing session ID"))?;
    let name = Path::new(&shown.path)
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing file name"))?;
    let mut key = Sha256::new();
    key.update(shown.computer.as_deref().unwrap_or("").as_bytes());
    key.update([0]);
    key.update(shown.uri.as_bytes());
    key.update([0]);
    key.update(bytes);
    let folder = root
        .join(format!("session-{}", kuvo_box::paths::segment(session)))
        .join(format!("v1-{:x}", key.finalize()));
    let mut directories = fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directories.mode(0o700);
    }
    directories.create(&folder)?;
    let path = folder.join(name);
    // An app may have edited its copy. Reuse only the bytes this request actually received.
    if !path
        .symlink_metadata()
        .is_ok_and(|meta| meta.is_file() && meta.len() == bytes.len() as u64)
        || !fs::read(&path).is_ok_and(|on_disk| on_disk == bytes)
    {
        write_received(bytes, &path)?;
    }
    #[cfg(target_os = "macos")]
    {
        let file = File::open(&path)?;
        if super::read_quarantine(&file)?.is_none() {
            super::quarantine(&file)?;
        }
    }
    // Track use on the directory so a cache hit leaves the document's timestamp alone.
    File::open(folder)?.set_modified(SystemTime::now())?;
    Ok(path)
}

pub(super) fn prune(root: &Path, keep: Option<&Path>) {
    prune_at(root, keep, SystemTime::now(), CAPACITY);
}

fn entry_usage(path: &Path) -> u64 {
    let mut size = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        // Count allocation overhead even for empty entries. Apps can leave files or folders
        // beside the cached original (Archive Utility, for example).
        size = size.saturating_add(4096);
        if meta.is_file() {
            size = size.saturating_add(meta.len());
        } else if meta.is_dir()
            && let Ok(children) = fs::read_dir(path)
        {
            pending.extend(children.flatten().map(|child| child.path()));
        }
    }
    size
}

fn prune_at(root: &Path, keep: Option<&Path>, now: SystemTime, capacity: u64) {
    let Ok(sessions) = fs::read_dir(root) else {
        return;
    };
    let mut entries = Vec::new();
    let mut total = 0u64;
    for session in sessions.flatten() {
        if !session
            .file_name()
            .to_string_lossy()
            .starts_with("session-")
            || !session.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        let Ok(versions) = fs::read_dir(session.path()) else {
            continue;
        };
        for version in versions.flatten() {
            let version_name = version.file_name();
            let version_name = version_name.to_string_lossy();
            // The early implementation used tempfile's .tmp directories.
            if !(version_name.starts_with("v1-") || version_name.starts_with(".tmp"))
                || !version.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let path = version.path();
            let size = entry_usage(&path);
            let last_used = version
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .unwrap_or(now);
            if keep != Some(path.as_path())
                && now.duration_since(last_used).is_ok_and(|age| age > MAX_AGE)
                && fs::remove_dir_all(&path).is_ok()
            {
                continue;
            }
            total = total.saturating_add(size);
            if keep != Some(path.as_path()) {
                entries.push((last_used, size, path));
            }
        }
    }
    entries.sort_unstable();
    for (_, size, path) in entries {
        if total <= capacity {
            break;
        }
        if fs::remove_dir_all(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    // Remove empty session folders left by eviction. remove_dir only takes empty folders.
    if let Ok(sessions) = fs::read_dir(root) {
        for session in sessions
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("session-"))
        {
            let _ = fs::remove_dir(session.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuvo_box::shown_files::FileKind;

    #[test]
    fn reuses_one_copy_per_source_version_and_prunes_old_or_excess_copies() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let shown = ShownFile {
            id: "file".into(),
            uri: "file:///report.txt".into(),
            path: "/report.txt".into(),
            name: "report.txt".into(),
            caption: None,
            kind: FileKind::Text,
            computer: Some("remote".into()),
            session: Some("ses_1".into()),
        };
        let old = put(root, &shown, b"old").unwrap();
        let current = put(root, &shown, b"new").unwrap();
        assert_eq!(put(root, &shown, b"new").unwrap(), current);
        assert_ne!(old, current);
        fs::write(&current, b"edited").unwrap();
        assert_eq!(put(root, &shown, b"new").unwrap(), current);
        assert_eq!(fs::read(&current).unwrap(), b"new");
        let overflow = put(root, &shown, b"overflow").unwrap();
        let now = SystemTime::now();
        File::open(old.parent().unwrap())
            .unwrap()
            .set_modified(now - MAX_AGE - Duration::from_secs(1))
            .unwrap();
        File::open(overflow.parent().unwrap())
            .unwrap()
            .set_modified(now - Duration::from_secs(60))
            .unwrap();
        prune_at(root, None, now, u64::MAX);
        assert!(!old.exists());
        assert!(overflow.exists());
        let extracted = overflow.parent().unwrap().join("extracted");
        fs::create_dir(&extracted).unwrap();
        fs::write(extracted.join("payload"), vec![0; 5000]).unwrap();
        prune_at(root, current.parent(), now, 17_000);
        assert!(!overflow.exists());
        assert!(current.is_file());
    }
}
