//! One native watcher per runtime, shared across logical folder subscriptions.

use crate::platform::{Platform, is_wsl_windows_drive};
use crate::timer_fires::{self, Timer};
use notify::{Config, Event, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
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

struct Entry {
    id: u64,
    prefer_native: bool,
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
    Event(Backend, notify::Result<Event>),
}

#[derive(Clone)]
pub struct WatchHub {
    commands: mpsc::UnboundedSender<Command>,
}

impl Default for WatchHub {
    fn default() -> Self {
        // One default registry per resident runtime. Services can keep their
        // own subscription handles without opening duplicate native watchers
        // for the same folder (inotify has a per-user instance limit).
        // Weak senders don't extend the lifetime of an exited test/runtime.
        static DEFAULTS: OnceLock<
            Mutex<HashMap<tokio::runtime::Id, mpsc::WeakUnboundedSender<Command>>>,
        > = OnceLock::new();
        let runtime = tokio::runtime::Handle::current().id();
        let mut defaults = DEFAULTS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        defaults.retain(|_, sender| sender.upgrade().is_some_and(|sender| !sender.is_closed()));
        if let Some(commands) = defaults
            .get(&runtime)
            .and_then(mpsc::WeakUnboundedSender::upgrade)
        {
            return Self { commands };
        }
        let hub = Self::new(Duration::from_millis(500), Duration::from_secs(1), false);
        defaults.insert(runtime, hub.commands.downgrade());
        hub
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
            native: None,
            poll: None,
            native_roots: BTreeMap::new(),
            poll_roots: BTreeMap::new(),
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
    native: Option<RecommendedWatcher>,
    poll: Option<PollWatcher>,
    native_roots: BTreeMap<PathBuf, bool>,
    poll_roots: BTreeMap<PathBuf, bool>,
}

impl Worker {
    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Command>) {
        while let Some(command) = inbox.recv().await {
            let configure = match command {
                Command::Open {
                    folder,
                    recursive,
                    reply,
                } => {
                    if !reply.is_closed() {
                        let (worker, result) = tokio::task::spawn_blocking(move || {
                            let result = self.open(folder, recursive);
                            (self, result)
                        })
                        .await
                        .expect("watch worker");
                        self = worker;
                        let _ = reply.send(result);
                    }
                    false
                }
                Command::Close { folder, id, at } => {
                    self.close(folder, id, at);
                    false
                }
                Command::Expire { folder, id, expiry } => {
                    if self.entries.get(&folder).is_some_and(|entry| {
                        entry.id == id && entry.subscribers == 0 && entry.expiry == expiry
                    }) {
                        self.entries.remove(&folder);
                        true
                    } else {
                        false
                    }
                }
                Command::Count(reply) => {
                    let _ = reply.send(self.entries.len());
                    false
                }
                Command::Event(backend, event) => self.report(backend, event),
            };
            if configure {
                self = tokio::task::spawn_blocking(move || {
                    self.configure();
                    self
                })
                .await
                .expect("watch worker");
            }
        }
        let _ = tokio::task::spawn_blocking(move || drop(self)).await;
    }

    fn open(&mut self, folder: PathBuf, recursive: bool) -> io::Result<Subscription> {
        let needs_setup = self
            .entries
            .get(&folder)
            .is_none_or(|entry| recursive && !entry.recursive);
        if needs_setup {
            if let Some(entry) = self.entries.get_mut(&folder) {
                entry.recursive = true;
            } else {
                self.next_id += 1;
                let (notices, _) = broadcast::channel(128);
                let (backend, _) = state_watch::channel(Backend::Native);
                self.entries.insert(
                    folder.clone(),
                    Entry {
                        id: self.next_id,
                        notices,
                        backend,
                        prefer_native: !self.force_poll && !requires_polling(&folder),
                        recursive,
                        subscribers: 0,
                        expiry: 0,
                    },
                );
            }
            self.configure();
        }
        let entry = self
            .entries
            .get_mut(&folder)
            .ok_or_else(|| io::Error::other("could not watch folder"))?;
        entry.subscribers += 1;
        Ok(Subscription {
            commands: self.commands.upgrade().ok_or_else(worker_stopped)?,
            folder,
            id: entry.id,
            rx: entry.notices.subscribe(),
            backend: entry.backend.subscribe(),
            recursive,
            initial: true,
        })
    }

    fn callback(&self, backend: Backend) -> impl FnMut(notify::Result<Event>) + Send + 'static {
        let commands = self.commands.clone();
        move |event| {
            // A reader can see a newly created or truncated record before the
            // writer finishes. Linux reports the completed write as an access
            // event; keep that notification, but ignore ordinary reads/opens.
            if event.as_ref().is_ok_and(|event| {
                event.kind.is_access()
                    && !matches!(event.kind,
                        notify::EventKind::Access(notify::event::AccessKind::Close(
                            notify::event::AccessMode::Write)))
            }) {
                return;
            }
            if let Some(commands) = commands.upgrade() {
                let _ = commands.send(Command::Event(backend, event));
            }
        }
    }

    fn configure(&mut self) {
        // notify's inotify backend removes all descendants when unwatching
        // a recursive root. Keep physical recursive watches disjoint; a
        // logical child shares its closest recursive ancestor's coverage.
        let mut native = BTreeMap::new();
        let mut candidates: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.prefer_native)
            .map(|(path, entry)| (path.clone(), entry.recursive))
            .collect();
        candidates.sort_by_key(|(path, _)| path.components().count());
        for (path, recursive) in candidates {
            if !covered(&native, &path, recursive) {
                native.insert(path, recursive);
            }
        }

        let previous = self.native_roots.clone();
        if self.native.is_none() && !native.is_empty() {
            self.native = notify::recommended_watcher(self.callback(Backend::Native)).ok();
        }
        if let Some(watcher) = &mut self.native
            && native != self.native_roots
        {
            // Batch updates: FSEvents otherwise stops and restarts its stream
            // separately for every root. Remove obsolete coverage first;
            // inotify's recursive unwatch also drops descendant descriptors.
            let mut paths = watcher.paths_mut();
            self.native_roots.retain(|path, recursive| {
                if native.get(path) == Some(recursive) {
                    true
                } else {
                    let _ = paths.remove(path);
                    false
                }
            });
            for (path, recursive) in native {
                if self.native_roots.get(&path) == Some(&recursive) {
                    continue;
                }
                if paths.add(&path, mode(recursive)).is_ok() {
                    self.native_roots.insert(path, recursive);
                } else {
                    // Recursive registration can fail partway through.
                    let _ = paths.remove(&path);
                }
            }
            if paths.commit().is_err() {
                self.native_roots.clear();
                self.native = None;
            }
        }
        if self.native_roots.is_empty() {
            self.native = None;
        }
        let wanted_poll: BTreeMap<_, _> = self
            .entries
            .iter()
            .filter(|(path, entry)| {
                !entry.prefer_native || !covered(&self.native_roots, path, entry.recursive)
            })
            .map(|(path, entry)| (path.clone(), entry.recursive))
            .collect();
        if self.poll.is_none() && !wanted_poll.is_empty() {
            let config = Config::default()
                .with_poll_interval(self.poll_interval)
                .with_compare_contents(true);
            self.poll = PollWatcher::new(self.callback(Backend::Poll), config).ok();
        }
        if let Some(watcher) = &mut self.poll {
            self.poll_roots.retain(|path, recursive| {
                if wanted_poll.get(path) == Some(recursive) {
                    true
                } else {
                    let _ = watcher.unwatch(path);
                    false
                }
            });
            for (path, recursive) in wanted_poll {
                if self.poll_roots.get(&path) != Some(&recursive)
                    && watcher.watch(&path, mode(recursive)).is_ok()
                {
                    self.poll_roots.insert(path, recursive);
                }
            }
        }
        if self.poll_roots.is_empty() {
            self.poll = None;
        }
        self.entries.retain(|path, entry| {
            let backend =
                if entry.prefer_native && covered(&self.native_roots, path, entry.recursive) {
                    Backend::Native
                } else if self.poll_roots.contains_key(path) {
                    Backend::Poll
                } else {
                    let _ = entry
                        .notices
                        .send(Notice::Failed("could not register folder watch".into()));
                    return false;
                };
            let changed = entry.backend.send_if_modified(|current| {
                if *current == backend {
                    false
                } else {
                    *current = backend;
                    true
                }
            });
            // Reconfiguration can lose notifications. On macOS changing any
            // root restarts the FSEvents stream; elsewhere only overlapping
            // roots lose coverage.
            if changed
                || (backend == Backend::Native
                    && previous != self.native_roots
                    && (cfg!(target_os = "macos")
                        || previous.iter().chain(self.native_roots.iter()).any(
                            |(root, recursive)| {
                                (previous.get(root) != self.native_roots.get(root))
                                    && (root == path || (*recursive && path.starts_with(root)))
                            },
                        )))
            {
                let _ = entry.notices.send(Notice::Rescan);
            }
            true
        });
    }

    fn report(&mut self, backend: Backend, event: notify::Result<Event>) -> bool {
        let mut lost = Vec::new();
        for (folder, entry) in &self.entries {
            if *entry.backend.borrow() != backend {
                continue;
            }
            match &event {
                Ok(event) if event.need_rescan() || event.paths.is_empty() => {
                    let _ = entry.notices.send(Notice::Rescan);
                }
                Ok(event) => {
                    // A root or ancestor can disappear or be atomically
                    // replaced. Discard the stale registration so a retry
                    // really watches the new directory, not the old inode.
                    if (event.kind.is_remove()
                        || event.kind.is_modify()
                            && matches!(
                                event.kind,
                                notify::EventKind::Modify(notify::event::ModifyKind::Name(
                                    notify::event::RenameMode::From
                                        | notify::event::RenameMode::Both
                                        | notify::event::RenameMode::Any
                                        | notify::event::RenameMode::Other
                                ))
                            ))
                        && event.paths.iter().any(|path| folder.starts_with(path))
                    {
                        lost.push(folder.clone());
                        let _ = entry
                            .notices
                            .send(Notice::Failed("watched folder moved or removed".into()));
                        continue;
                    }
                    let paths: Vec<_> = event
                        .paths
                        .iter()
                        .filter_map(|path| {
                            let relative = path.strip_prefix(folder).ok()?;
                            // ReadDirectoryChangesW watches a directory's contents,
                            // not its own metadata. A recursively watched ancestor
                            // also reports metadata changes to the logical root:
                            // writing a child can change that directory's stamp.
                            // Do not turn those into a change of every named file.
                            if cfg!(windows)
                                && relative.as_os_str().is_empty()
                                && event.kind.is_modify()
                                && !matches!(
                                    event.kind,
                                    notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                                )
                            {
                                return None;
                            }
                            Some(relative.to_owned())
                        })
                        .collect();
                    if !paths.is_empty() {
                        let _ = entry.notices.send(Notice::Changed { paths });
                    }
                }
                Err(error)
                    if error.paths.is_empty()
                        || error
                            .paths
                            .iter()
                            .any(|path| path.starts_with(folder) || folder.starts_with(path)) =>
                {
                    let _ = entry.notices.send(Notice::Failed(error.to_string()));
                    lost.push(folder.clone());
                }
                Err(_) => {}
            }
        }
        for path in &lost {
            self.entries.remove(path);
        }
        !lost.is_empty()
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
            timer_fires::sleep_until(Timer::FileWatchGrace, deadline).await;
            if let Some(commands) = commands.upgrade() {
                let _ = commands.send(Command::Expire { folder, id, expiry });
            }
        });
    }
}

fn mode(recursive: bool) -> RecursiveMode {
    if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    }
}

fn covered(roots: &BTreeMap<PathBuf, bool>, folder: &Path, recursive: bool) -> bool {
    roots.get(folder).is_some_and(|deep| *deep || !recursive)
        || roots
            .iter()
            .any(|(root, deep)| *deep && folder.starts_with(root))
}

pub struct Subscription {
    commands: mpsc::UnboundedSender<Command>,
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
        let _ = self.commands.send(Command::Close {
            folder: self.folder.clone(),
            id: self.id,
            at: Instant::now(),
        });
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
    stable_roots: Vec<PathBuf>,
    child_files: Option<(PathBuf, BTreeSet<std::ffi::OsString>)>,
    watches: BTreeMap<PathBuf, (Targets, JoinHandle<()>)>,
    tx: mpsc::UnboundedSender<()>,
    rx: mpsc::UnboundedReceiver<()>,
}
#[derive(Default, PartialEq, Eq, Clone)]
struct Targets {
    names: BTreeSet<PathBuf>,
    all: bool,
    recursive: bool,
    child_names: BTreeSet<std::ffi::OsString>,
}

impl Targets {
    fn matches(&self, path: &Path) -> bool {
        self.all
            || path.as_os_str().is_empty()
            || self.names.iter().any(|name| name.starts_with(path))
            || (!self.child_names.is_empty()
                && (path.components().count() <= 1
                    || path
                        .components()
                        .nth(1)
                        .is_some_and(|name| self.child_names.contains(name.as_os_str()))))
    }
}

impl FileSet {
    pub fn new(hub: WatchHub) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            hub,
            paths: Vec::new(),
            folders: Vec::new(),
            stable_roots: Vec::new(),
            child_files: None,
            watches: BTreeMap::new(),
            tx,
            rx,
        }
    }
    pub fn clear(&mut self) {
        self.paths.clear();
        self.folders.clear();
        self.stable_roots.clear();
        self.child_files = None;
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
        self.stable_roots.clear();
        self.child_files = None;
        self.bind();
    }
    /// Keep a removable folder's named files on its stable parent, rather than
    /// holding a native watch handle inside the folder itself.
    pub fn set_files_at_roots(&mut self, paths: Vec<PathBuf>, roots: Vec<PathBuf>) {
        self.paths = paths;
        self.folders.clear();
        self.stable_roots = roots;
        self.child_files = None;
        self.bind();
    }
    /// Watch a stable parent recursively for direct child folders appearing
    /// or disappearing, and named files or subtrees within those children. Never keep
    /// handles inside a removable child (on Windows that prevents renames).
    pub fn set_child_files(
        &mut self,
        folder: PathBuf,
        names: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
    ) {
        self.paths.clear();
        self.folders.clear();
        self.stable_roots.clear();
        self.child_files = Some((folder, names.into_iter().map(Into::into).collect()));
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
        fn rooted(wanted: &mut BTreeMap<PathBuf, Targets>, path: PathBuf, roots: &[PathBuf]) {
            if let Some(root) = roots
                .iter()
                .filter(|root| path.starts_with(root))
                .max_by_key(|root| root.components().count())
            {
                named(wanted, root.clone());
                if root.is_dir() {
                    let target = wanted.entry(root.clone()).or_default();
                    target.recursive = true;
                    target
                        .names
                        .insert(path.strip_prefix(root).expect("selected root").to_owned());
                }
            } else {
                named(wanted, path);
            }
        }
        for path in &self.paths {
            rooted(&mut wanted, path.clone(), &self.stable_roots);
            if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
                && let Ok(target) = path.canonicalize()
            {
                rooted(&mut wanted, target, &self.stable_roots);
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
        if let Some((folder, names)) = &self.child_files {
            named(&mut wanted, folder.clone());
            if folder.is_dir() {
                let target = wanted.entry(folder.clone()).or_default();
                target.recursive = true;
                target.child_names.extend(names.iter().cloned());
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
                let mut retry = Duration::from_secs(1);
                loop {
                    match hub
                        .subscribe(
                            &target,
                            Options {
                                recursive: names_for_task.recursive,
                                create: false,
                            },
                        )
                        .await
                    {
                        Ok(mut sub) => {
                            // Re-read once the actual watch is armed, closing the registration/read race.
                            let _ = tx.send(());
                            retry = Duration::from_secs(1);
                            while let Some(notice) = sub.recv().await {
                                match notice {
                                    Notice::Changed { paths }
                                        if !paths
                                            .iter()
                                            .any(|path| names_for_task.matches(path)) =>
                                    {
                                        continue;
                                    }
                                    Notice::Failed(_) => {
                                        let _ = tx.send(());
                                        break;
                                    }
                                    _ => {
                                        let _ = tx.send(());
                                    }
                                }
                            }
                            // The failed subscription is dropped before rearming. An ordinary
                            // change or rescan keeps its original native watch running.
                        }
                        Err(_) => {
                            let _ = tx.send(());
                        }
                    }
                    timer_fires::sleep(Timer::FileWatchRecovery, retry).await;
                    retry = (retry * 2).min(Duration::from_secs(60));
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

#[cfg(test)]
mod file_set_tests {
    use super::*;

    #[test]
    fn child_file_selection_does_not_report_a_bots_unrelated_work() {
        let target = Targets {
            recursive: true,
            child_names: ["bot.json".into(), "session".into()].into(),
            ..Targets::default()
        };
        assert!(target.matches(Path::new("bot_ada")));
        assert!(target.matches(Path::new("bot_ada/bot.json")));
        assert!(target.matches(Path::new("bot_ada/session")));
        assert!(!target.matches(Path::new("bot_ada/memory.json")));
        assert!(!target.matches(Path::new("bot_ada/files/bot.json")));
    }

    #[test]
    fn rooted_file_selection_receives_an_ancestor_removal_without_other_files() {
        let target = Targets {
            recursive: true,
            names: [PathBuf::from("bot_ada/workspace/brief.pdf")].into(),
            ..Targets::default()
        };
        assert!(target.matches(Path::new("bot_ada")));
        assert!(target.matches(Path::new("bot_ada/workspace")));
        assert!(target.matches(Path::new("bot_ada/workspace/brief.pdf")));
        assert!(!target.matches(Path::new("bot_ada/workspace/other.pdf")));
        assert!(!target.matches(Path::new("bot_ben")));
    }
}
