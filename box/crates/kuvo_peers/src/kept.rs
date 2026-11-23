//! Disposable peer copies. Each file has one resident writer.

use std::{fs, io, path::Path};

use serde::{Serialize, de::DeserializeOwned};

pub fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    kuvo_store::files::write_atomic(path, &serde_json::to_vec(value)?)
}

pub fn write_durable(path: &Path, value: &impl Serialize) -> io::Result<()> {
    write(path, value)
}
