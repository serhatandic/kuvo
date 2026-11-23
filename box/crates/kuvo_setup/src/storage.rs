use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

use crate::Error;

fn file_error(path: &Path, action: &'static str, source: io::Error) -> Error {
    Error::File {
        action,
        path: path.into(),
        source,
    }
}

#[derive(Clone)]
pub(crate) struct Files(PathBuf);

impl Files {
    pub fn new(home: PathBuf) -> Self {
        Self(home)
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    pub fn read<T: DeserializeOwned>(&self, relative: &str) -> Result<Option<T>, Error> {
        let path = self.path(relative);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|source| Error::Json { path, source }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(file_error(&path, "read", error)),
        }
    }

    pub fn names(&self, relative: &str) -> Result<Vec<String>, Error> {
        let path = self.path(relative);
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(file_error(&path, "list", error)),
        };
        entries
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .map_err(|source| file_error(&path, "list", source))
            })
            .collect()
    }

    pub fn remove(&self, relative: &str) -> Result<(), Error> {
        let path = self.path(relative);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(file_error(&path, "remove", error)),
        }
    }

    pub fn write(&self, relative: &str, value: &impl Serialize) -> Result<(), Error> {
        let path = self.path(relative);
        let bytes = serde_json::to_vec(value).map_err(|source| Error::Json { path, source })?;
        self.write_bytes(relative, &bytes)
    }

    pub fn write_bytes(&self, relative: &str, bytes: &[u8]) -> Result<(), Error> {
        let path = self.path(relative);
        let parent = path.parent().expect("a home-relative file has a parent");
        let publish = || -> io::Result<()> {
            fs::create_dir_all(parent)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(bytes)?;
            temporary.as_file().sync_all()?;
            temporary.persist(&path)?;
            sync_directory(parent)?;
            if parent == self.path("run/setup-checkpoints") {
                sync_directory(&self.path("run"))?;
            }
            Ok(())
        };
        publish().map_err(|source| file_error(&path, "publish", source))
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}
