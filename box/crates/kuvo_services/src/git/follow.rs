//! One followed read per folder, with index checks and filesystem notifications where available.
use kuvo_os::timer_fires::{self, Timer};
use super::{changes, list};
use kuvo_os::{
    watch::{FileSet, Notice, Options, WatchHub},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{
    sync::{Notify, watch},
    time::Instant,
};

#[derive(Clone)]
pub(super) struct Snapshot {
    pub started: Instant,
    pub value: Value,
}
struct Sent {
    tx: watch::Sender<Option<Snapshot>>,
    refresh: Notify,
}
#[derive(Clone, Default)]
pub(super) struct Followers {
    folders: Arc<Mutex<HashMap<PathBuf, Weak<Sent>>>>,
    watches: WatchHub,
}
impl Followers {
    pub(super) fn subscribe(&self, folder: PathBuf) -> watch::Receiver<Option<Snapshot>> {
        let mut folders = self.folders.lock().expect("followers");
        folders.retain(|_, v| v.strong_count() > 0);
        if let Some(sent) = folders.get(&folder).and_then(Weak::upgrade) {
            let rx = sent.tx.subscribe();
            sent.refresh.notify_one();
            return rx;
        }
        let (tx, rx) = watch::channel(None);
        let tx = Arc::new(Sent {
            tx,
            refresh: Notify::new(),
        });
        folders.insert(folder.clone(), Arc::downgrade(&tx));
        let watches = self.watches.clone();
        tokio::spawn(async move {
            follow(folder, tx, watches).await;
        });
        rx
    }
}
async fn git_files(folder: &Path, files: &mut FileSet) {
    let gitdir = changes::read(folder, &["rev-parse", "--absolute-git-dir"])
        .await.map(|raw| PathBuf::from(raw.trim())).filter(|path| !path.as_os_str().is_empty());
    let common = changes::read(folder, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .await.map(|raw| PathBuf::from(raw.trim())).filter(|path| !path.as_os_str().is_empty());
    let mut paths = vec![folder.join(".git")];
    let mut folders = Vec::new();
    if let Some(dir) = gitdir {
        paths.extend(["index", "HEAD", "logs/HEAD", "MERGE_HEAD", "CHERRY_PICK_HEAD"].map(|name| dir.join(name)));
        folders.push((dir.join("refs"), true));
    }
    if let Some(dir) = common {
        paths.extend(["packed-refs", "config"].map(|name| dir.join(name)));
        folders.push((dir.join("refs"), true));
    }
    files.set_targets(paths, folders);
}

async fn follow(folder: PathBuf, tx: Arc<Sent>, watches: WatchHub) {
    let mut git = FileSet::new(watches.clone());
    git_files(&folder, &mut git).await;
    let mut watch = watches.subscribe(&folder, Options { recursive: true, create: false }).await.ok();
    let mut marker = std::fs::symlink_metadata(folder.join(".git")).and_then(|m| m.modified()).ok();
    loop {
        let started = Instant::now();
        if let Ok(value) = list::files(&folder).await { tx.tx.send_replace(Some(Snapshot { started, value })); }
        let earliest = Instant::now() + Duration::from_secs(1).max(started.elapsed() * 2);
        loop {
            tokio::select! {
                _ = tx.tx.closed() => return,
                _ = tx.refresh.notified() => break,
                _ = git.changed() => {
                    let current = std::fs::symlink_metadata(folder.join(".git")).and_then(|m| m.modified()).ok();
                    if marker != current { marker = current; git_files(&folder, &mut git).await; }
                    break;
                }
                // Only on a real watch failure, not a periodic check on native filesystems.
                _ = timer_fires::sleep(Timer::GitFollow, Duration::from_secs(10)), if watch.is_none() => {
                    watch = watches.subscribe(&folder, Options { recursive: true, create: false }).await.ok();
                    break;
                }
                event = async { match watch.as_mut() { Some(w) => w.recv().await, None => std::future::pending().await } } => {
                    match event {
                        Some(Notice::Changed { paths }) if paths.iter().all(|path| path.components().any(|c| c.as_os_str()==".git")) => continue,
                        Some(Notice::Failed(_)) | None => { watch = None; break; }
                        Some(_) => break,
                    }
                }
            }
        }
        // Debounce actual changes; no independent refresh or index stamp timer.
        tokio::select! {
            _ = tx.tx.closed() => return,
            _ = timer_fires::sleep_until(Timer::GitFollow, earliest) => {},
        }
    }
}
