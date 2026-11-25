//! One watcher per folder, with a short grace period for reconnecting subscribers.

use crate::platform::{Platform, is_wsl_windows_drive};
use notify::{Config, Event, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch as state_watch},
    task::JoinHandle,
    time::Instant,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub recursive: bool,
    pub create: bool,
}

#[derive(Clone, Debug)]
pub enum Notice {
    /// Paths are relative to the watched folder. An unknown change is a Rescan.
    Changed {
        paths: Vec<PathBuf>,
    },
    /// Look again after starting, after raising coverage, or when events may have been lost.
    Rescan,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Native,
    Poll,
}

enum WatcherKind {
    Native(RecommendedWatcher),
    Poll(PollWatcher),
}

impl WatcherKind {
    fn backend(&self) -> Backend {
        match self {
            Self::Native(_) => Backend::Native,
            Self::Poll(_) => Backend::Poll,
        }
    }

    fn as_mut(&mut self) -> &mut dyn Watcher {
        match self {
            Self::Native(watcher) => watcher,
            Self::Poll(watcher) => watcher,
        }
    }
}

struct Entry {
    id: u64,
    watcher: WatcherKind,
    notices: broadcast::Sender<Notice>,
    backend: state_watch::Sender<Backend>,
    recursive: bool,
    subscribers: usize,
    expiry: u64,
}

enum Command {
    Open {
        folder: PathBuf,
        recursive: bool,
        reply: oneshot::Sender<io::Result<Subscription>>,
    },
    Close {
        folder: PathBuf,
        id: u64,
        at: Instant,
    },
    Expire {
        folder: PathBuf,
        id: u64,
        expiry: u64,
    },
    Count(oneshot::Sender<usize>),
}

#[derive(Clone)]
pub struct WatchHub {
    commands: mpsc::UnboundedSender<Command>,
}

impl Default for WatchHub {
    fn default() -> Self {
        Self::new(Duration::from_millis(500), Duration::from_secs(1), false)
    }
}

impl WatchHub {
    /// Keep one registry in the resident and pass its handle to every folder-watch caller.
    /// A Tokio runtime with time enabled owns the registry's task.
    pub fn new(grace: Duration, poll_interval: Duration, force_poll: bool) -> Self {
        assert!(!poll_interval.is_zero(), "poll interval must be positive");
        let (commands, inbox) = mpsc::unbounded_channel();
        let worker = Worker {
            entries: HashMap::new(),
            next_id: 0,
            commands: commands.downgrade(),
            grace,
            poll_interval,
            force_poll,
        };
        tokio::spawn(worker.run(inbox));
        Self { commands }
    }

    /// Includes watchers retained during their reconnection grace period.
    pub async fn watched_folders(&self) -> io::Result<usize> {
        let (reply, response) = oneshot::channel();
        self.send(Command::Count(reply))?;
        response.await.map_err(|_| worker_stopped())
    }

    pub async fn subscribe(
        &self,
        folder: impl AsRef<Path>,
        options: Options,
    ) -> io::Result<Subscription> {
        let folder = folder.as_ref().to_owned();
        let folder = tokio::task::spawn_blocking(move || {
            if options.create {
                std::fs::create_dir_all(&folder)?;
            }
            let folder = folder.canonicalize()?;
            if !folder.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "watch target must be a folder",
                ));
            }
            Ok(folder)
        })
        .await
        .map_err(io::Error::other)??;
        let (reply, response) = oneshot::channel();
        self.send(Command::Open {
            folder,
            recursive: options.recursive,
            reply,
        })?;
        response.await.map_err(|_| worker_stopped())?
    }

    fn send(&self, command: Command) -> io::Result<()> {
        self.commands.send(command).map_err(|_| worker_stopped())
    }
}

fn worker_stopped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "folder watch registry stopped")
}

struct Worker {
    entries: HashMap<PathBuf, Entry>,
    next_id: u64,
    commands: mpsc::WeakUnboundedSender<Command>,
    grace: Duration,
    poll_interval: Duration,
    force_poll: bool,
}

impl Worker {
    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Command>) {
        while let Some(command) = inbox.recv().await {
            match command {
                Command::Open {
                    folder,
                    recursive,
                    reply,
                } => {
                    if !reply.is_closed() {
                        // If the caller cancels while the watch is opening, the returned
                        // subscription is dropped and releases its lease through Close.
                        let _ = reply.send(self.open(folder, recursive).await);
                    }
                }
                Command::Close { folder, id, at } => self.close(folder, id, at),
                Command::Expire { folder, id, expiry } => {
                    if self.entries.get(&folder).is_some_and(|entry| {
                        entry.id == id && entry.subscribers == 0 && entry.expiry == expiry
                    }) {
                        let entry = self.entries.remove(&folder);
                        let _ = tokio::task::spawn_blocking(move || drop(entry)).await;
                    }
                }
                Command::Count(reply) => {
                    let _ = reply.send(self.entries.len());
                }
            }
        }
        let _ = tokio::task::spawn_blocking(move || drop(self.entries)).await;
    }

    async fn open(&mut self, folder: PathBuf, recursive: bool) -> io::Result<Subscription> {
        let needs_setup = self
            .entries
            .get(&folder)
            .is_none_or(|entry| recursive && !entry.recursive);
        if needs_setup {
            let prior = self.entries.remove(&folder);
            self.next_id += 1;
            let id = self.next_id;
            let path = folder.clone();
            let poll_interval = self.poll_interval;
            let force_poll = self.force_poll;
            let entry = tokio::task::spawn_blocking(move || -> io::Result<Entry> {
                if let Some(mut entry) = prior {
                    let watcher = entry.watcher.as_mut();
                    if watcher
                        .unwatch(&path)
                        .and_then(|()| watcher.watch(&path, RecursiveMode::Recursive))
                        .is_err()
                    {
                        drop(entry.watcher);
                        entry.watcher =
                            create_watcher(&path, &entry.notices, true, poll_interval, force_poll)
                                .inspect_err(|error| {
                                    let _ = entry.notices.send(Notice::Failed(error.to_string()));
                                    let _ = entry.notices.send(Notice::Rescan);
                                })?;
                    }
                    entry.backend.send_replace(entry.watcher.backend());
                    entry.recursive = true;
                    let _ = entry.notices.send(Notice::Rescan);
                    Ok(entry)
                } else {
                    let (notices, _) = broadcast::channel(128);
                    let watcher =
                        create_watcher(&path, &notices, recursive, poll_interval, force_poll)?;
                    let (backend, _) = state_watch::channel(watcher.backend());
                    Ok(Entry {
                        id,
                        watcher,
                        notices,
                        backend,
                        recursive,
                        subscribers: 0,
                        expiry: 0,
                    })
                }
            })
            .await
            .map_err(io::Error::other)??;
            self.entries.insert(folder.clone(), entry);
        }
        let entry = self.entries.get_mut(&folder).expect("watch was installed");
        entry.subscribers += 1;
        Ok(Subscription {
            commands: self.commands.clone(),
            folder,
            id: entry.id,
            rx: entry.notices.subscribe(),
            backend: entry.backend.subscribe(),
            recursive,
            initial: true,
        })
    }

    fn close(&mut self, folder: PathBuf, id: u64, at: Instant) {
        let Some(entry) = self.entries.get_mut(&folder).filter(|entry| entry.id == id) else {
            return;
        };
        entry.subscribers -= 1;
        if entry.subscribers != 0 {
            return;
        }
        entry.expiry += 1;
        let expiry = entry.expiry;
        let deadline = at + self.grace;
        let commands = self.commands.clone();
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            if let Some(commands) = commands.upgrade() {
                let _ = commands.send(Command::Expire { folder, id, expiry });
            }
        });
    }
}

fn create_watcher(
    folder: &Path,
    notices: &broadcast::Sender<Notice>,
    recursive: bool,
    poll_interval: Duration,
    force_poll: bool,
) -> io::Result<WatcherKind> {
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    let callback = || {
        let folder = folder.to_owned();
        let notices = notices.clone();
        move |event| report(&folder, &notices, event)
    };
    if !force_poll
        && !requires_polling(folder)
        && let Ok(mut native) = notify::recommended_watcher(callback())
        && native.watch(folder, mode).is_ok()
    {
        return Ok(WatcherKind::Native(native));
    }
    let config = Config::default()
        .with_poll_interval(poll_interval)
        .with_compare_contents(true);
    let mut watcher = PollWatcher::new(callback(), config).map_err(io::Error::other)?;
    watcher.watch(folder, mode).map_err(io::Error::other)?;
    Ok(WatcherKind::Poll(watcher))
}

fn report(folder: &Path, tx: &broadcast::Sender<Notice>, event: notify::Result<Event>) {
    match event {
        Ok(event) if event.need_rescan() || event.paths.is_empty() => {
            let _ = tx.send(Notice::Rescan);
        }
        Ok(event) if event.kind.is_access() => {}
        Ok(event) => {
            let paths: Vec<_> = event
                .paths
                .into_iter()
                .filter_map(|path| path.strip_prefix(folder).ok().map(Path::to_owned))
                .collect();
            let _ = tx.send(if paths.is_empty() {
                Notice::Rescan
            } else {
                Notice::Changed { paths }
            });
        }
        Err(error) => {
            let _ = tx.send(Notice::Failed(error.to_string()));
            let _ = tx.send(Notice::Rescan);
        }
    }
}

pub struct Subscription {
    commands: mpsc::WeakUnboundedSender<Command>,
    folder: PathBuf,
    id: u64,
    rx: broadcast::Receiver<Notice>,
    backend: state_watch::Receiver<Backend>,
    recursive: bool,
    initial: bool,
}

impl Subscription {
    pub fn backend(&self) -> Backend {
        *self.backend.borrow()
    }

    pub async fn recv(&mut self) -> Option<Notice> {
        if std::mem::take(&mut self.initial) {
            return Some(Notice::Rescan);
        }
        loop {
            match self.rx.recv().await {
                Ok(Notice::Changed { mut paths }) if !self.recursive => {
                    paths.retain(|path| path.components().count() <= 1);
                    if !paths.is_empty() {
                        return Some(Notice::Changed { paths });
                    }
                }
                Ok(notice) => return Some(notice),
                Err(broadcast::error::RecvError::Lagged(_)) => return Some(Notice::Rescan),
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.upgrade() {
            let _ = commands.send(Command::Close {
                folder: self.folder.clone(),
                id: self.id,
                at: Instant::now(),
            });
        }
    }
}

fn requires_polling(path: &Path) -> bool {
    (Platform::current() == Platform::Wsl && is_wsl_windows_drive(path)) || network_filesystem(path)
}

#[cfg(target_os = "linux")]
fn network_filesystem(path: &Path) -> bool {
    // NFS, SMB/CIFS, 9p (including DrvFS), Ceph, AFS, Coda, NCP, and FUSE mounts.
    rustix::fs::statfs(path).is_ok_and(|stat| {
        matches!(
            stat.f_type as u64 & 0xffff_ffff,
            0x6969
                | 0x517b
                | 0xff53_4d42
                | 0xfe53_4d42
                | 0x0102_1997
                | 0x00c3_6400
                | 0x5346_414f
                | 0x7375_7245
                | 0x564c
                | 0x6573_5546
        )
    })
}

#[cfg(target_os = "macos")]
fn network_filesystem(path: &Path) -> bool {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut result = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is NUL-terminated and result holds writable statfs storage.
    let status = unsafe { libc::statfs(path.as_ptr(), result.as_mut_ptr()) };
    // SAFETY: statfs initialized the result if it succeeded.
    status == 0 && unsafe { result.assume_init() }.f_flags & (libc::MNT_LOCAL as u32) == 0
}

#[cfg(windows)]
fn network_filesystem(path: &Path) -> bool {
    use std::{
        os::windows::ffi::OsStrExt,
        path::{Component, Prefix},
    };
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    if matches!(prefix.kind(), Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)) {
        return true;
    }
    let root: Vec<_> = prefix
        .as_os_str()
        .encode_wide()
        .chain([b'\\' as u16, 0])
        .collect();
    // SAFETY: root is a NUL-terminated drive root. DRIVE_REMOTE is 4.
    unsafe { GetDriveTypeW(root.as_ptr()) == 4 }
}

/// Watch the parents of specific files, not the files' inodes (an atomic replacement is a change).
/// For a missing directory watch the first existing ancestor, then rebind when it appears. This
/// has no timer apart from the watcher backend's platform fallback. Dropping the set releases
/// every subscription.
pub struct FileSet {
    hub: WatchHub,
    paths: Vec<PathBuf>,
    folders: Vec<(PathBuf, bool)>,
    watches: BTreeMap<PathBuf, (Targets, JoinHandle<()>)>,
    tx: mpsc::UnboundedSender<()>,
    rx: mpsc::UnboundedReceiver<()>,
}
#[derive(Default, PartialEq, Eq, Clone)]
struct Targets {
    names: BTreeSet<PathBuf>,
    all: bool,
    recursive: bool,
}

impl FileSet {
    pub fn new(hub: WatchHub) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            hub,
            paths: Vec::new(),
            folders: Vec::new(),
            watches: BTreeMap::new(),
            tx,
            rx,
        }
    }
    pub fn clear(&mut self) {
        self.paths.clear();
        self.folders.clear();
        for (_, (_, task)) in std::mem::take(&mut self.watches) {
            task.abort();
        }
    }
    pub async fn changed(&mut self) {
        let _ = self.rx.recv().await;
        while self.rx.try_recv().is_ok() {}
        self.bind();
    }
    pub fn set_files(&mut self, paths: Vec<PathBuf>) {
        self.set_targets(paths, Vec::new());
    }
    /// Watch named files and directory contents. A folder receives its own shallow or recursive
    /// watch when present; until then, it is watched as a name in its nearest existing ancestor.
    pub fn set_targets(&mut self, paths: Vec<PathBuf>, folders: Vec<(PathBuf, bool)>) {
        self.paths = paths;
        self.folders = folders;
        self.bind();
    }
    fn bind(&mut self) {
        let mut wanted = BTreeMap::<PathBuf, Targets>::new();
        fn named(wanted: &mut BTreeMap<PathBuf, Targets>, mut path: PathBuf) {
            while let Some(parent) = path.parent() {
                if parent.is_dir() {
                    if let Some(name) = path.file_name() {
                        wanted
                            .entry(parent.to_owned())
                            .or_default()
                            .names
                            .insert(name.into());
                    }
                    break;
                }
                path = parent.to_owned();
            }
        }
        for path in &self.paths {
            named(&mut wanted, path.clone());
            if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
                if let Ok(target) = path.canonicalize() {
                    named(&mut wanted, target);
                }
            }
        }
        for (folder, recursive) in &self.folders {
            // Watch both its name (for replacement/removal) and its contents while it exists.
            named(&mut wanted, folder.clone());
            if folder.is_dir() {
                let target = wanted.entry(folder.clone()).or_default();
                target.all = true;
                target.recursive |= *recursive;
            }
        }
        self.watches.retain(|folder, (names, task)| {
            let same = wanted.get(folder) == Some(names) && !task.is_finished();
            if !same {
                task.abort();
            }
            same
        });
        for (folder, names) in wanted {
            if self.watches.contains_key(&folder) {
                continue;
            }
            let (hub, tx, target, names_for_task) = (
                self.hub.clone(),
                self.tx.clone(),
                folder.clone(),
                names.clone(),
            );
            let task = tokio::spawn(async move {
                let Ok(mut sub) = hub
                    .subscribe(
                        target,
                        Options {
                            recursive: names_for_task.recursive,
                            create: false,
                        },
                    )
                    .await
                else {
                    return;
                };
                // Re-read once the actual watch is armed, closing the registration/read race.
                let _ = tx.send(());
                while let Some(notice) = sub.recv().await {
                    match notice {
                        Notice::Changed { paths }
                            if !paths.iter().any(|path| {
                                names_for_task.all
                                    || path.as_os_str().is_empty()
                                    || names_for_task.names.contains(path)
                            }) =>
                        {
                            continue;
                        }
                        _ => {
                            let _ = tx.send(());
                        }
                    }
                }
            });
            self.watches.insert(folder, (names, task));
        }
    }
}
impl Drop for FileSet {
    fn drop(&mut self) {
        self.clear();
    }
}
