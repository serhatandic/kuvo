//! Keep releases available to running programs and peer transfers until they let go.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use kuvo_store::{files, home::Home, locks};

use crate::{Release, fleet};

const RETIRED: &str = ".retired-";

fn lease_path(home: &Path, release: &Release) -> io::Result<PathBuf> {
    let dir = home.join("run/release-leases");
    files::create_private_dirs(&dir)?;
    let release_dir = release.folder(home);
    let name = release_dir
        .file_name()
        .expect("a release has a directory name");
    Ok(dir.join(name))
}

pub(crate) fn share(home: &Path, release: &Release) -> io::Result<File> {
    locks::try_shared_file_lock(&lease_path(home, release)?)
}

pub(crate) fn exclusive(home: &Path, release: &Release) -> io::Result<File> {
    locks::try_file_lock(&lease_path(home, release)?)
}

/// Protect this program's published release until it exits.
pub fn lease_running_release(home: &Path) -> io::Result<Option<File>> {
    let versions = match home.join("versions").canonicalize() {
        Ok(versions) => versions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let program = std::env::current_exe()?.canonicalize()?;
    let Ok(relative) = program.strip_prefix(&versions) else {
        return Ok(None);
    };
    let Some(name) = relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
    else {
        return Ok(None);
    };
    let expired = || io::Error::new(io::ErrorKind::NotFound, "This Kuvo release was retired.");
    if name.starts_with(RETIRED) {
        return Err(expired());
    }
    let Ok(release) = format!("versions/{name}/bin").parse::<Release>() else {
        return Ok(None);
    };
    let guard = share(home, &release)?;
    if !release.executable(home).is_file() {
        return Err(expired());
    }
    Ok(Some(guard))
}

pub(crate) fn sweep(home: &Home, staged: &Release) -> io::Result<()> {
    let versions = home.root().join("versions");
    let mut retired = Vec::new();
    {
        let _selection = locks::try_file_lock(&home.run().join(".keeper-selection.lock"))?;
        if home.run().join("previous").try_exists()? {
            return Ok(());
        }
        let (_, current) = fleet::installed(home)?;
        if current.as_ref() == Some(staged) {
            return Ok(());
        }
        for entry in fs::read_dir(&versions)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with(RETIRED) {
                retired.push(entry.path());
                continue;
            }
            let Ok(release) = format!("versions/{name}/bin").parse::<Release>() else {
                continue;
            };
            if &release == staged || current.as_ref() == Some(&release) {
                continue;
            }
            match retire(home.root(), &versions, &release) {
                Ok(path) => retired.push(path),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => eprintln!("Retiring Kuvo release {name}: {error}"),
            }
        }
    }

    for path in retired {
        if let Err(error) = files::remove_tree_synced(&path) {
            eprintln!("Removing retired Kuvo release: {error}");
        }
    }
    Ok(())
}

fn retire(home: &Path, versions: &Path, release: &Release) -> io::Result<PathBuf> {
    let _exclusive = exclusive(home, release)?;
    let retired = tempfile::Builder::new()
        .prefix(RETIRED)
        .tempdir_in(versions)?
        .keep();
    files::replace_synced(&release.folder(home), &retired.join("release"))?;
    Ok(retired)
}
