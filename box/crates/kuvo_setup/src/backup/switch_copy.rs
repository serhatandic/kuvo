//! One-time rollback copy for the TypeScript beta switch. The updater owns
//! stopping writers before calling this hook and must wait for it to succeed.
#[cfg(not(target_os = "macos"))]
use std::io;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_store::{files, locks::OwnedLock};

use super::{Backups, Error, Result, inventory::stat};

const COPY: &str = "before-rust";
const STAGE: &str = ".before-rust-stage-";
const LOCK: &str = ".before-rust.lock";

impl Backups {
    /// Copy the quiesced home for manual rollback, including beta archives and
    /// configuration. A published copy is kept intact on every retry.
    pub fn preserve_before_rust(&self) -> Result<PathBuf> {
        let home = self.root().canonicalize()?;
        let backups = home.join("backups");
        if stat(&backups)?.is_some_and(|meta| !meta.is_dir()) {
            return Err(Error::Message(
                "The pre-switch backup folder contains a link.",
            ));
        }
        files::private_folder(&backups)?;
        let _guard = OwnedLock::acquire(&backups.join(LOCK), Duration::from_millis(100))?;
        let saved = backups.join(COPY);
        if let Some(meta) = stat(&saved)? {
            return if meta.is_dir() {
                files::sync_folder_through(&backups, &home)?;
                Ok(saved)
            } else {
                Err(Error::Message("The pre-switch copy path is occupied."))
            };
        }
        for entry in fs::read_dir(&backups)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(STAGE) && entry.file_type()?.is_dir()
            {
                files::remove_tree_synced(&entry.path())?;
            }
        }
        let stage = tempfile::Builder::new()
            .prefix(STAGE)
            .tempdir_in(&backups)?;
        let draft = stage.path().join(COPY);
        copy_directory(&home, &draft, Some(&backups))?;
        files::sync_tree(&draft, &home)?;
        files::replace_synced(&draft, &saved)?;
        files::sync_folder_through(&backups, &home)?;
        Ok(saved)
    }
}

fn copy_directory(from: &Path, to: &Path, backups: Option<&Path>) -> Result<()> {
    files::private_folder(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let source = entry.path();
        let target = to.join(&name);
        if backups == Some(from) {
            let leaf = name.to_string_lossy();
            if leaf == COPY || leaf.starts_with(STAGE) || leaf.starts_with(LOCK) {
                continue;
            }
        }
        if backups == Some(source.as_path()) {
            copy_directory(&source, &target, backups)?;
        } else {
            copy_entry(&source, &target)?;
        }
    }
    fs::set_permissions(to, fs::symlink_metadata(from)?.permissions())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn copy_entry(from: &Path, to: &Path) -> Result<()> {
    files::clone_mac_entry(from, to, true)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn copy_entry(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        // Sockets name live IPC endpoints and carry no files to restore.
        if meta.file_type().is_socket() {
            return Ok(());
        }
    }
    if meta.is_symlink() {
        copy_link(from, to, &meta)?;
    } else if meta.is_dir() {
        copy_directory(from, to, None)?;
    } else if meta.is_file() {
        let mut source = super::archive::regular(from)?;
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        io::copy(&mut source, &mut target)?;
        target.set_permissions(meta.permissions())?;
        target.set_times(fs::FileTimes::new().set_modified(meta.modified()?))?;
    } else {
        return Err(Error::Detail(format!(
            "Cannot preserve the filesystem entry {}.",
            from.display()
        )));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn copy_link(from: &Path, to: &Path, _: &fs::Metadata) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(windows)]
fn copy_link(from: &Path, to: &Path, meta: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
    let link = fs::read_link(from)?;
    if meta.file_type().is_symlink_dir() {
        symlink_dir(link, to)
    } else {
        symlink_file(link, to)
    }
}
