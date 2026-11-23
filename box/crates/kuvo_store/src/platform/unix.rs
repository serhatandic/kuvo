use std::{
    fs::{self, DirBuilder, File, Permissions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::Path,
};

pub fn private_dirs(folder: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(folder)
}

pub fn private_file(file: &File) -> io::Result<()> {
    file.set_permissions(Permissions::from_mode(0o600))
}

pub fn restrict_folder(folder: &Path) -> io::Result<()> {
    let metadata = fs::metadata(folder)?;
    // SAFETY: geteuid takes no arguments and has no memory preconditions.
    let uid = unsafe { libc::geteuid() };
    if uid != 0 && metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the folder belongs to another account",
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        fs::set_permissions(folder, Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

pub fn publish_new(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)
}

pub fn sync_dir(folder: &Path) -> io::Result<()> {
    File::open(folder)?.sync_all()
}

/// Kept for the old shell launcher protocol, which identifies owners by command line.
pub fn launcher_runs(pid: u32) -> io::Result<bool> {
    let line = if cfg!(target_os = "linux") {
        match fs::read(format!("/proc/{pid}/cmdline")) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            result => result?,
        }
    } else {
        let output = std::process::Command::new("/bin/ps")
            .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
            .output()?;
        output.stdout
    };
    let line = String::from_utf8_lossy(&line);
    Ok(["kuvo-start", "kuvo-launch", "kuvo_script", "kuvo-link"]
        .iter()
        .any(|role| line.contains(role)))
}

pub fn directory_conflict(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EEXIST | libc::ENOTEMPTY))
}

pub fn open_for_sync(path: &Path) -> io::Result<File> {
    File::open(path)
}

pub fn directory_id(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|meta| meta.ino())
}

pub fn file_stamp(path: &Path) -> io::Result<crate::files::FileStamp> {
    use std::time::UNIX_EPOCH;

    let meta = fs::metadata(path)?;
    let created_ns = meta
        .created()
        .ok()
        .map(|time| match time.duration_since(UNIX_EPOCH) {
            Ok(elapsed) => elapsed.as_nanos() as i128,
            Err(before) => -(before.duration().as_nanos() as i128),
        });
    let nanos = |secs, nanos| i128::from(secs) * 1_000_000_000 + i128::from(nanos);
    Ok(crate::files::FileStamp {
        device: meta.dev(),
        file: u128::from(meta.ino()),
        created_ns,
        modified_ns: nanos(meta.mtime(), meta.mtime_nsec()),
        changed_ns: nanos(meta.ctime(), meta.ctime_nsec()),
        size: meta.len(),
    })
}

pub fn remove_tree(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
