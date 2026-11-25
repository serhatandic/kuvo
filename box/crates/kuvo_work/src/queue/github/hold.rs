//! The shared GitHub cache lock uses a directory and a token unique to its holder.

use kuvo_store::files;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const HOLD: Duration = Duration::from_secs(180);

pub(super) struct Hold {
    path: PathBuf,
    token: String,
}

impl Hold {
    pub fn take(path: PathBuf) -> io::Result<Option<Self>> {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let prefix = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(io::ErrorKind::InvalidInput)?;
        files::create_private_dirs(parent)?;
        let mut random = [0; 9];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let token = kuvo_wire::keys::encode(&random);
        let make = || match fs::create_dir(&path) {
            Ok(()) => {
                let _ = fs::write(path.join(&token), []);
                Some(Self {
                    path: path.clone(),
                    token: token.clone(),
                })
            }
            Err(_) => None,
        };
        if let Some(hold) = make() {
            return Ok(Some(hold));
        }
        for name in files::list_of(parent) {
            if name.to_str().is_some_and(|name| {
                name.starts_with(prefix) && name[prefix.len()..].starts_with('.')
            }) {
                let aside = parent.join(name);
                if stale(&aside) {
                    let _ = remove(&aside);
                }
            }
        }
        if !stale(&path) {
            return Ok(None);
        }
        let aside = parent.join(format!("{prefix}.{token}"));
        if fs::rename(&path, &aside).is_err() {
            return Ok(None);
        }
        if !stale(&aside) {
            let _ = fs::rename(&aside, &path);
            return Ok(None);
        }
        let _ = remove(&aside);
        Ok(make())
    }

    pub fn touch(&self) {
        let _ = filetime::set_file_mtime(&self.path, filetime::FileTime::now());
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if self.path.join(&self.token).is_file() {
            let _ = remove(&self.path);
        }
    }
}

fn stale(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .and_then(|stat| stat.modified())
        .ok()
        .is_some_and(|changed| {
            SystemTime::now()
                .duration_since(changed)
                .is_ok_and(|age| age >= HOLD)
        })
}

fn remove(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
