//! Move a discarded entry to the operating system's Trash where available, otherwise Kuvo's.
use std::{fs, io, path::Path};

pub fn move_entry(home: &Path, entry: &Path) -> io::Result<()> {
    let alternate = std::env::var_os("KUVO_CHANGES_TRASH");
    let name = entry.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    #[cfg(target_os = "macos")]
    if alternate.is_none() {
        if Path::new("/usr/bin/trash").is_file() {
            let status = std::process::Command::new("/usr/bin/trash")
                .arg(entry)
                .status()?;
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other("trash failed"))
            };
        }
        let account = super::platform::Account::current()?;
        let env = std::env::vars_os().collect();
        let bin = account
            .home_folder(super::platform::Platform::Mac, &env)?
            .join(".Trash");
        fs::create_dir_all(&bin)?;
        let mut target = bin.join(name);
        let mut n = 2;
        while fs::symlink_metadata(&target).is_ok() {
            target = bin.join(format!("{} {n}", name.to_string_lossy()));
            n += 1;
        }
        return transfer(entry, &target);
    }
    let bin = alternate
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join("trash"));
    fs::create_dir_all(&bin)?;
    let at = tempfile::tempdir_in(bin)?;
    let target = at.path().join(name);
    transfer(entry, &target)?;
    let _ = at.keep();
    Ok(())
}

fn transfer(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            // Do not remove the original until its whole copy is present.
            copy_entry(from, to)?;
            let stat = fs::symlink_metadata(from)?;
            if stat.is_dir() {
                fs::remove_dir_all(from)
            } else {
                remove_entry(from, &stat)
            }
        }
        Err(error) => Err(error),
    }
}
fn copy_entry(from: &Path, to: &Path) -> io::Result<()> {
    let stat = fs::symlink_metadata(from)?;
    if stat.file_type().is_symlink() {
        return copy_link(from, to, &stat);
    }
    if stat.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_entry(&entry.path(), &to.join(entry.file_name()))?;
        }
        fs::set_permissions(to, stat.permissions())
    } else if stat.is_file() {
        fs::copy(from, to).map(|_| ())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular entry",
        ))
    }
}
#[cfg(unix)]
fn copy_link(from: &Path, to: &Path, _stat: &fs::Metadata) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}
#[cfg(windows)]
fn copy_link(from: &Path, to: &Path, stat: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    let target = fs::read_link(from)?;
    if stat.file_type().is_symlink_dir() {
        std::os::windows::fs::symlink_dir(target, to)
    } else {
        std::os::windows::fs::symlink_file(target, to)
    }
}
#[cfg(unix)]
fn remove_entry(from: &Path, _stat: &fs::Metadata) -> io::Result<()> {
    fs::remove_file(from)
}
#[cfg(windows)]
fn remove_entry(from: &Path, stat: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    if stat.file_type().is_symlink_dir() {
        fs::remove_dir(from)
    } else {
        fs::remove_file(from)
    }
}
