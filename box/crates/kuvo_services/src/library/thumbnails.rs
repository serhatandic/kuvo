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
pub struct Thumbnail {
    pub bytes: Vec<u8>,
    pub mime: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailPlatform {
    MacOs,
    Other,
}

impl ThumbnailPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

#[derive(Clone)]
pub struct Thumbnails {
    folder: PathBuf,
    platform: ThumbnailPlatform,
    slots: Arc<Semaphore>,
    in_flight: Arc<Mutex<HashMap<String, watch::Receiver<Option<Option<Vec<u8>>>>>>>,
}

impl Thumbnails {
    pub fn new(folder: PathBuf, platform: ThumbnailPlatform) -> Self {
        Self {
            folder,
            platform,
            slots: Arc::new(Semaphore::new(2)),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn can(&self, item: &Item) -> bool {
        if item.kind == LibraryKind::Image {
            return true;
        }
        self.platform == ThumbnailPlatform::MacOs
            && matches!(item.kind, LibraryKind::Document | LibraryKind::Page)
    }

    pub async fn of(
        &self,
        item: &Item,
        copy: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Option<Thumbnail> {
        let meta = match fs::metadata(&item.path) {
            Ok(meta) => meta,
            Err(_) => {
                if item.kind == LibraryKind::Image {
                    return copy().map(|bytes| Thumbnail {
                        bytes,
                        mime: item.mime.clone(),
                    });
                }
                return None;
            }
        };
        if !meta.is_file() {
            return None;
        }
        if self.platform != ThumbnailPlatform::MacOs {
            if item.kind != LibraryKind::Image || meta.len() > AS_IT_IS_LIMIT {
                return None;
            }
            return fs::read(&item.path).ok().map(|bytes| Thumbnail {
                bytes,
                mime: item.mime.clone(),
            });
        }
        let key = cache_key(&item.id, &meta);
        let file = self.folder.join(format!("{key}.png"));
        if let Ok(bytes) = fs::read(&file) {
            return Some(Thumbnail {
                bytes,
                mime: "image/png".to_owned(),
            });
        }

        let existing = self.in_flight.lock().unwrap().get(&key).cloned();
        if let Some(mut rx) = existing {
            while rx.borrow().is_none() {
                if rx.changed().await.is_err() {
                    return None;
                }
            }
            return rx.borrow().as_ref().and_then(Clone::clone).map(|bytes| Thumbnail {
                bytes,
                mime: "image/png".to_owned(),
            });
        }

        let (tx, rx) = watch::channel(None);
        self.in_flight.lock().unwrap().insert(key.clone(), rx);
        let permit = self.slots.acquire().await.ok();
        let bytes = if permit.is_some() {
            self.make(item, &file).await
        } else {
            None
        };
        drop(permit);
        let _ = tx.send(Some(bytes.clone()));
        self.in_flight.lock().unwrap().remove(&key);
        bytes.map(|bytes| Thumbnail {
            bytes,
            mime: "image/png".to_owned(),
        })
    }

    async fn make(&self, item: &Item, file: &Path) -> Option<Vec<u8>> {
        fs::create_dir_all(&self.folder).ok()?;
        let scratch = tempfile::Builder::new()
            .prefix(".making-")
            .tempdir_in(&self.folder)
            .ok()?;
        let side = THUMBNAIL_SIDE.to_string();
        let made = if item.kind == LibraryKind::Image {
            let out = scratch.path().join("thumbnail.png");
            let ok = run_tool(
                "/usr/bin/sips",
                &["-Z", &side, "-s", "format", "png", &item.path, "--out", &out.to_string_lossy()],
            )
            .await;
            if !ok {
                return None;
            }
            out
        } else {
            let ok = run_tool(
                "/usr/bin/qlmanage",
                &["-t", "-s", &side, "-o", &scratch.path().to_string_lossy(), &item.path],
            )
            .await;
            if !ok {
                return None;
            }
            scratch.path().join(format!("{}.png", basename(&item.path)))
        };
        let bytes = fs::read(&made).ok()?;
        let prefix = format!("{}-", item.id);
        if let Ok(entries) = fs::read_dir(&self.folder) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(&prefix) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        fs::rename(&made, file).ok()?;
        Some(bytes)
    }
}

fn cache_key(id: &str, meta: &fs::Metadata) -> String {
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0u64;
    let stamp = format!("{}:{mtime_ms}:{ino}", meta.len());
    let digest = hex::encode(Sha256::digest(stamp.as_bytes()));
    format!("{id}-{}", &digest[..16])
}

async fn run_tool(program: &str, args: &[&str]) -> bool {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let Ok(output) = tokio::time::timeout(MAKING_TIMEOUT, cmd.status()).await else {
        return false;
    };
    output.is_ok_and(|status| status.success())
}
