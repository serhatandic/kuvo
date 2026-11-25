//! One followed read per folder, with index checks and filesystem notifications where available.
use kuvo_os::timer_fires::{self, Timer};
use super::{changes, list};
use kuvo_os::{
    platform::{Platform, is_wsl_windows_drive},
    watch::{Notice, Options, WatchHub},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, SystemTime},
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
fn index_stamp(dir: &Path) -> [Option<SystemTime>; 3] {
    ["index", "HEAD", "logs/HEAD"].map(|file| {
        std::fs::metadata(dir.join(file))
            .and_then(|s| s.modified())
            .ok()
    })
}
async fn follow(folder: PathBuf, tx: Arc<Sent>, watches: WatchHub) {
    let gitdir = changes::read(&folder, &["rev-parse", "--absolute-git-dir"])
        .await
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty());
    let watch_files = gitdir.is_some()
        && (Platform::current() == Platform::Mac
            || (Platform::current() == Platform::Wsl && !is_wsl_windows_drive(&folder)));
    let mut watch = if watch_files {
        watches
            .subscribe(
                &folder,
                Options {
                    recursive: true,
                    create: false,
                },
            )
            .await
            .ok()
    } else {
        None
    };
    let every = Duration::from_secs(if watch.is_some() { 30 } else { 10 });
    let mut index = gitdir.as_deref().map(index_stamp);
    loop {
        let started = Instant::now();
        if let Ok(value) = list::files(&folder).await {
            tx.tx.send_replace(Some(Snapshot { started, value }));
        }
        let earliest = Instant::now() + Duration::from_secs(1).max(started.elapsed() * 2);
        let deadline = Instant::now() + every;
        let mut check = timer_fires::interval(Timer::GitFollow, Duration::from_secs(1));
        check.tick().await;
        loop {
            tokio::select! {
                _=tx.tx.closed()=>return,
                _=tokio::time::sleep_until(deadline)=>break,
                _=tx.refresh.notified()=>break,
                _=check.tick()=>{
                    let next=gitdir.as_deref().map(index_stamp);
                    if index != next {index=next;break;}
                }
                event=async {
                    match watch.as_mut() {Some(w)=>w.recv().await,None=>std::future::pending().await}
                }=>{
                    match event {
                        Some(Notice::Changed{paths}) if paths.iter().all(|path|path.components().any(|c|c.as_os_str()==".git"))=>continue,
                        Some(_)=>break,
                        None=>{watch=None;}
                    }
                }
            }
        }
        tokio::select! {
            _=tx.tx.closed()=>return,
            _=tokio::time::sleep_until(earliest)=>{}
        }
    }
}
