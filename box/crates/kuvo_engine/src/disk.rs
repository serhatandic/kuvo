//! Small engine records, written on the blocking pool.

use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn write(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let parent = path.parent().expect("engine record has a directory");
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, value)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) async fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) async fn durable(
    path: std::path::PathBuf,
    value: impl serde::Serialize + Send + 'static,
) -> io::Result<()> {
    tokio::task::spawn_blocking(move || write(&path, &value)).await?
}

pub(crate) fn key(session: &str, message: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(
        serde_json::json!([session, message]).to_string(),
    ))
}
