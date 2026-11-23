//! Disposable peer copies. Each file has one resident writer.

use std::{fs, io, io::Write, path::Path};

use serde::{Serialize, de::DeserializeOwned};

pub fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    replace(path, value, false)
}

pub fn write_durable(path: &Path, value: &impl Serialize) -> io::Result<()> {
    replace(path, value, true)
}

fn replace(path: &Path, value: &impl Serialize, durable: bool) -> io::Result<()> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    if durable {
        file.as_file().sync_all()?;
    }
    file.persist(path)?;
    Ok(())
}
