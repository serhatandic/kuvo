//! Thumbnails for Library items (`library/thumbnails/<id>-<stamp>.png`),
//! matching `box/src/link/library/thumbnails.ts` with one platform seam.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, watch};

use super::store::{Item, LibraryKind};
use crate::artifacts::kinds::basename;

pub const THUMBNAIL_SIDE: u32 = 480;
const AS_IT_IS_LIMIT: u64 = 25 * 1024 * 1024;
const MAKING_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct Thumbnail { pub bytes: Vec<u8>, pub mime: String }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailPlatform { MacOs, Other }

impl ThumbnailPlatform {
    pub fn current() -> Self { if cfg!(target_os = "macos") { Self::MacOs } else { Self::Other } }
}

type InFlight = Arc<Mutex<HashMap<String, watch::Receiver<Option<Option<Vec<u8>>>>>>>;

#[derive(Clone)]
pub struct Thumbnails {
    folder: PathBuf,
    platform: ThumbnailPlatform,
    slots: Arc<Semaphore>,
    in_flight: InFlight,
}

impl Thumbnails {
    pub fn new(folder: PathBuf, platform: ThumbnailPlatform) -> Self {
        Self { folder, platform, slots: Arc::new(Semaphore::new(2)), in_flight: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn can(&self, item: &Item) -> bool {
        item.kind == LibraryKind::Image
            || (self.platform == ThumbnailPlatform::MacOs && matches!(item.kind, LibraryKind::Document | LibraryKind::Page))
    }

    pub async fn of(&self, item: &Item, copy: impl FnOnce() -> Option<Vec<u8>>) -> Option<Thumbnail> {
        let meta = match fs::metadata(&item.path) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return None,
            Err(_) => {
                return (item.kind == LibraryKind::Image).then(copy).flatten()
                    .map(|bytes| Thumbnail { bytes, mime: item.mime.clone() });
            }
        };
        if self.platform != ThumbnailPlatform::MacOs {
            if item.kind != LibraryKind::Image || meta.len() > AS_IT_IS_LIMIT { return None; }
            return fs::read(&item.path).ok().map(|bytes| Thumbnail { bytes, mime: item.mime.clone() });
        }
        let key = cache_key(&item.id, &meta);
        let file = self.folder.join(format!("{key}.png"));
        let png = |bytes| Thumbnail { bytes, mime: "image/png".to_owned() };
        if let Ok(bytes) = fs::read(&file) { return Some(png(bytes)); }

        let existing = self.in_flight.lock().unwrap().get(&key).cloned();
        if let Some(mut rx) = existing {
            while rx.borrow().is_none() { rx.changed().await.ok()?; }
            return rx.borrow().as_ref().and_then(Clone::clone).map(png);
        }

        let (tx, rx) = watch::channel(None);
        self.in_flight.lock().unwrap().insert(key.clone(), rx);
        let permit = self.slots.acquire().await.ok();
        let bytes = if permit.is_some() { self.make(item, &file).await } else { None };
        drop(permit);
        let _ = tx.send(Some(bytes.clone()));
        self.in_flight.lock().unwrap().remove(&key);
        bytes.map(png)
    }

    async fn make(&self, item: &Item, file: &Path) -> Option<Vec<u8>> {
        fs::create_dir_all(&self.folder).ok()?;
        let scratch = tempfile::Builder::new().prefix(".making-").tempdir_in(&self.folder).ok()?;
        let side = THUMBNAIL_SIDE.to_string();
        let made = if item.kind == LibraryKind::Image {
            let out = scratch.path().join("thumbnail.png");
            run_tool("/usr/bin/sips", &["-Z", &side, "-s", "format", "png", &item.path, "--out", &out.to_string_lossy()]).await.then_some(out)?
        } else {
            let out = scratch.path().join(format!("{}.png", basename(&item.path)));
            run_tool("/usr/bin/qlmanage", &["-t", "-s", &side, "-o", &scratch.path().to_string_lossy(), &item.path]).await.then_some(out)?
        };
        let bytes = fs::read(&made).ok()?;
        let prefix = format!("{}-", item.id);
        for entry in fs::read_dir(&self.folder).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) { let _ = fs::remove_file(entry.path()); }
        }
        fs::rename(&made, file).ok()?;
        Some(bytes)
    }
}

fn cache_key(id: &str, meta: &fs::Metadata) -> String {
    let mtime_ms = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
    #[cfg(unix)]
    let ino = { use std::os::unix::fs::MetadataExt; meta.ino() };
    #[cfg(not(unix))]
    let ino = 0u64;
    let digest = hex::encode(Sha256::digest(format!("{}:{mtime_ms}:{ino}", meta.len()).as_bytes()));
    format!("{id}-{}", &digest[..16])
}

async fn run_tool(program: &str, args: &[&str]) -> bool {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(MAKING_TIMEOUT, cmd.status()).await.ok().and_then(Result::ok).is_some_and(|s| s.success())
}
