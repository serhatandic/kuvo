//! Guards for the on-disk lock protocols.

use crate::{files, platform};
use kuvo_os::process::{ProcessIdentity, ProcessState};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

/// Publishes a unique PID marker with the directory. Only a dead owner can
/// be reclaimed. Dropping the guard releases only the marker it published.
#[derive(Debug)]
pub struct OwnedLock {
    path: PathBuf,
    marker: String,
    rehome_pid: bool,
}

impl OwnedLock {
    pub fn acquire(path: &Path, wait: Duration) -> io::Result<Self> {
        Self::take(path, wait, false)
    }

    /// The signed Mac key handover tool authenticates its parent with this
    /// PID file. Publish it together with the unique owner marker.
    #[cfg(target_os = "macos")]
    pub fn acquire_rehome(path: &Path, wait: Duration) -> io::Result<Self> {
        Self::take(path, wait, true)
    }

    fn take(path: &Path, wait: Duration, rehome_pid: bool) -> io::Result<Self> {
        let parent = files::parent(path)?;
        files::create_private_dirs(parent)?;
        let mut token = [0; 8];
        getrandom::fill(&mut token).map_err(io::Error::other)?;
        let marker = format!("owner-{}-{}", std::process::id(), hex::encode(token));
        let mut prefix = path
            .file_name()
            .ok_or(io::ErrorKind::InvalidInput)?
            .to_os_string();
        prefix.push(".pending-");
        let pending = tempfile::Builder::new()
            .prefix(&prefix)
            .tempdir_in(parent)?;
        platform::restrict_folder(pending.path())?;
        if rehome_pid {
            let identity = ProcessIdentity::read(std::process::id())?.ok_or_else(|| {
                io::Error::other("couldn't identify the process taking the rehome lock")
            })?;
            let bytes = serde_json::to_vec(&identity).map_err(io::Error::other)?;
            files::write_new(&pending.path().join(&marker), &bytes)?;
            files::write_new(
                &pending.path().join("pid"),
                std::process::id().to_string().as_bytes(),
            )?;
        } else {
            let file = File::create(pending.path().join(&marker))?;
            platform::private_file(&file)?;
        }
        with_deadline(wait, Duration::from_millis(10), || {
            // On Windows a lock its owner is removing refuses to be opened
            // until it's gone: busy, as one held.
            match fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() => match reap(path, rehome_pid) {
                    Ok(true) => {}
                    Ok(false) => return Ok(None),
                    Err(err) if platform::directory_conflict(&err) => return Ok(None),
                    Err(err) => return Err(err),
                },
                Ok(_) => return Ok(None),
                Err(err) if err.kind() == io::ErrorKind::NotFound => (),
                Err(err) if platform::directory_conflict(&err) => return Ok(None),
                Err(err) => return Err(err),
            }
            match fs::rename(pending.path(), path) {
                Ok(()) => Ok(Some(Self {
                    path: path.into(),
                    marker: marker.clone(),
                    rehome_pid,
                })),
                Err(error) if platform::directory_conflict(&error) => Ok(None),
                Err(error) => Err(error),
            }
        })
        .map_err(|err| files::at("claiming", path, err))
    }
}

fn reap(path: &Path, rehome_pid: bool) -> io::Result<bool> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    let mut empty = true;
    for entry in entries {
        empty = false;
        let entry = entry?;
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(owner_pid) else {
            continue;
        };
        let saved = rehome_pid
            .then(|| files::read(&entry.path()).ok().flatten())
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<ProcessIdentity>(&bytes).ok())
            .filter(|identity| identity.pid == pid);
        let gone = saved.map_or_else(
            || matches!(ProcessIdentity::read(pid), Ok(None)),
            |identity| identity.state() == ProcessState::Stale,
        );
        if !gone {
            return Ok(false);
        }
        // Only the waiter that removes this particular marker can remove the directory.
        // The layout lock serializes rehome reclaimers. Keep the owner marker
        // until the last deletion so a kill during reclamation can retry it.
        if rehome_pid {
            let _ = fs::remove_file(path.join("pid"));
        }
        if fs::remove_file(entry.path()).is_ok() {
            return Ok(fs::remove_dir(path).is_ok());
        }
        return Ok(false);
    }
    Ok(empty && fs::remove_dir(path).is_ok())
}

fn owner_pid(marker: &str) -> Option<u32> {
    let (pid, token) = marker.strip_prefix("owner-")?.split_once('-')?;
    if pid.starts_with('0')
        || !pid.bytes().all(|b| b.is_ascii_digit())
        || token.len() != 16
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    pid.parse().ok()
}

impl Drop for OwnedLock {
    fn drop(&mut self) {
        if self.rehome_pid && self.path.join(&self.marker).is_file() {
            let _ = fs::remove_file(self.path.join("pid"));
        }
        if fs::remove_file(self.path.join(&self.marker)).is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

/// Kernel-held lock for Rust-only writers. The lock file remains after release.
pub fn try_file_lock(path: &Path) -> io::Result<File> {
    let file = open_lock(path)?;
    file.try_lock().map_err(io::Error::from)?;
    Ok(file)
}

/// Hold a published resource open while readers are still using it.
pub fn try_shared_file_lock(path: &Path) -> io::Result<File> {
    let file = open_lock(path)?;
    file.try_lock_shared().map_err(io::Error::from)?;
    Ok(file)
}

fn open_lock(path: &Path) -> io::Result<File> {
    File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}

#[derive(Clone, Copy, Debug)]
pub enum OnBusy {
    Proceed,
    Refuse,
}

/// The stale directory protocol used by legacy best-effort readers/writers.
#[derive(Debug)]
pub struct StaleLock {
    path: PathBuf,
    identity: Option<u64>,
}

impl StaleLock {
    pub fn acquire(
        path: &Path,
        stale: Duration,
        wait: Duration,
        on_busy: OnBusy,
    ) -> io::Result<Option<Self>> {
        files::create_private_dirs(files::parent(path)?)?;
        match with_deadline(wait, Duration::from_millis(10), || {
            match fs::create_dir(path) {
                Ok(()) => Ok(Some(Self {
                    path: path.into(),
                    identity: platform::directory_id(path),
                })),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if older_than(path, stale) {
                        let _ = fs::remove_dir(path);
                    }
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        }) {
            Ok(guard) => Ok(Some(guard)),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && matches!(on_busy, OnBusy::Proceed) =>
            {
                Ok(None)
            }
            Err(error) => Err(files::at("claiming", path, error)),
        }
    }
}

impl Drop for StaleLock {
    fn drop(&mut self) {
        if self.identity.is_some() && self.identity == platform::directory_id(&self.path) {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

/// The old engine launcher and adoption command share this PID marker protocol.
#[derive(Debug)]
pub struct LaunchLock {
    lock: PathBuf,
    holder: PathBuf,
    pid: String,
}

impl LaunchLock {
    pub fn acquire(run: &Path, stale: Duration, wait: Duration) -> io::Result<Self> {
        files::create_private_dirs(run)?;
        let lock = run.join(".launching");
        let holder = run.join(".launching.pid");
        let pid = std::process::id().to_string();
        with_deadline(wait, Duration::from_millis(100), || {
            match fs::create_dir(&lock) {
                Ok(()) => {
                    if let Err(error) = files::write_atomic(&holder, format!("{pid}\n").as_bytes())
                    {
                        let _ = fs::remove_dir(&lock);
                        return Err(error);
                    }
                    Ok(Some(Self {
                        lock: lock.clone(),
                        holder: holder.clone(),
                        pid: pid.clone(),
                    }))
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    break_launch_lock(run, &lock, &holder, &pid, stale);
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        })
        .map_err(|error| files::at("claiming the engine launch lock at", &lock, error))
    }
}

impl Drop for LaunchLock {
    fn drop(&mut self) {
        if said(&self.holder).as_deref() == Some(&self.pid) && fresh_claim(&self.lock, &self.holder)
        {
            let _ = fs::remove_file(&self.holder);
            let _ = fs::remove_dir(&self.lock);
        }
    }
}

/// A released machine's launcher only removes an empty, five-minute-old
/// directory. Keep a recorded owner inside it while its data is moved.
pub struct ReleasedLaunch {
    path: PathBuf,
    owner: ProcessIdentity,
}

impl ReleasedLaunch {
    const MARKER: &str = "released-install.json";

    #[cfg(unix)]
    pub fn acquire(run: &Path) -> io::Result<Self> {
        files::create_private_dirs(run)?;
        let path = run.join(".launching");
        let owner = ProcessIdentity::read(std::process::id())?
            .ok_or_else(|| io::Error::other("The installer cannot identify itself."))?;
        let pending = tempfile::Builder::new()
            .prefix(".launching.pending-")
            .tempdir_in(run)?;
        files::write_new(
            &pending.path().join(Self::MARKER),
            &serde_json::to_vec(&owner)?,
        )?;
        // Do not replace an empty legacy launch lock, or leave an unowned one
        // if the installer dies between creating the directory and its marker.
        if let Err(error) = platform::place_exclusive(pending.path(), &path) {
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
            if !Self::read_owner(&path)?.is_some_and(|old| old.state() == ProcessState::Stale) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Another installation change is under way.",
                ));
            }
            Self::retire(&path)?;
            platform::place_exclusive(pending.path(), &path).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Another installation change is under way.",
                    )
                } else {
                    error
                }
            })?;
        }
        Ok(Self { path, owner })
    }

    fn read_owner(path: &Path) -> io::Result<Option<ProcessIdentity>> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let marker = path.join(Self::MARKER);
        if !fs::symlink_metadata(&marker).is_ok_and(|meta| meta.is_file()) {
            return Ok(None);
        }
        Ok(files::read(&marker)?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()?)
    }

    pub fn owns(&self, path: &Path) -> bool {
        self.path == path && Self::read_owner(path).ok().flatten().as_ref() == Some(&self.owner)
    }

    fn retire(path: &Path) -> io::Result<()> {
        let aside = tempfile::Builder::new()
            .prefix(".launching.retired-")
            .tempdir_in(files::parent(path)?)?
            .keep();
        let retired = aside.join("lock");
        if let Err(error) = fs::rename(path, &retired) {
            let _ = fs::remove_dir(&aside);
            return Err(error);
        }
        fs::remove_file(retired.join(Self::MARKER))?;
        fs::remove_dir(retired)?;
        fs::remove_dir(aside)
    }
}

impl Drop for ReleasedLaunch {
    fn drop(&mut self) {
        if self.owns(&self.path) {
            let _ = Self::retire(&self.path);
        }
    }
}

fn said(file: &Path) -> Option<String> {
    files::text_of(file)
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn changed(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

fn fresh_claim(dir: &Path, marker: &Path) -> bool {
    changed(dir)
        .zip(changed(marker))
        .is_some_and(|(dir, marker)| dir <= marker)
}

fn older_than(path: &Path, age: Duration) -> bool {
    changed(path)
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

fn launching(pid: &str) -> bool {
    pid.parse::<u32>()
        .ok()
        .filter(|&pid| pid > 0)
        .is_some_and(|pid| {
            pid == std::process::id() || platform::launcher_runs(pid).unwrap_or(true)
        })
}

fn gone_holder(lock: &Path, holder: &Path) -> Option<String> {
    said(holder).filter(|pid| fresh_claim(lock, holder) && !launching(pid))
}

fn break_launch_lock(run: &Path, lock: &Path, holder: &Path, pid: &str, stale: Duration) {
    if gone_holder(lock, holder).is_none() && !older_than(lock, stale) {
        return;
    }
    let breaking = run.join(".launching.breaking");
    let taker_path = breaking.join("pid");
    let taker = said(&taker_path);
    let expiry = Duration::from_secs(60);
    let abandoned = match &taker {
        Some(owner) => !launching(owner),
        None => older_than(&breaking, expiry),
    };
    if abandoned && !retire_breaker(run, &breaking, taker, pid, expiry) {
        return;
    }
    if fs::create_dir(&breaking).is_err() {
        return;
    }
    if files::write_atomic(&taker_path, format!("{pid}\n").as_bytes()).is_err() {
        let _ = fs::remove_dir(&breaking);
        return;
    }
    if let Some(gone) = gone_holder(lock, holder) {
        let aside = run.join(format!(".launching.pid.{pid}"));
        if fs::rename(holder, &aside).is_ok() {
            if said(&aside).as_deref() == Some(&gone) {
                if fresh_claim(lock, &aside) {
                    let _ = fs::remove_dir(lock);
                }
            } else {
                let _ = fs::hard_link(&aside, holder);
            }
            let _ = fs::remove_file(aside);
        }
    } else if older_than(lock, stale) {
        let _ = fs::remove_file(holder);
        let _ = fs::remove_dir(lock);
    }
    if said(&taker_path).as_deref() == Some(pid) {
        let _ = fs::remove_file(taker_path);
        let _ = fs::remove_dir(breaking);
    }
}

fn retire_breaker(
    run: &Path,
    breaking: &Path,
    taker: Option<String>,
    pid: &str,
    expiry: Duration,
) -> bool {
    let name = match &taker {
        Some(owner) if owner.bytes().all(|b| b.is_ascii_digit()) => owner.clone(),
        _ => platform::directory_id(breaking).map_or_else(|| "n".into(), |id| format!("n{id}")),
    };
    let prefix = ".launching.breaking.gone-";
    for marker in files::list_of(run)
        .into_iter()
        .filter(|name| name.to_string_lossy().starts_with(prefix))
    {
        let marker = run.join(marker);
        if older_than(&marker, expiry) {
            let _ = fs::remove_dir(marker);
        }
    }
    if fs::create_dir(run.join(format!("{prefix}{name}"))).is_err() {
        return false;
    }
    let aside = run.join(format!(".launching.breaking.{pid}"));
    if fs::rename(breaking, &aside).is_err() {
        return false;
    }
    if said(&aside.join("pid")) != taker {
        if !breaking.exists() {
            let _ = fs::rename(aside, breaking);
        }
        return false;
    }
    let _ = fs::remove_file(aside.join("pid"));
    let _ = fs::remove_dir(aside);
    true
}

fn with_deadline<T>(
    wait: Duration,
    interval: Duration,
    mut attempt: impl FnMut() -> io::Result<Option<T>>,
) -> io::Result<T> {
    let start = Instant::now();
    loop {
        if let Some(guard) = attempt()? {
            return Ok(guard);
        }
        let remaining = wait.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        thread::sleep(interval.min(remaining));
    }
}

/// One async writer's lock per path. Share this value among that writer's tasks
/// so same-process work waits its turn before applying the cross-process deadline.
#[derive(Clone, Debug)]
pub struct SerialLock {
    path: PathBuf,
    turns: std::sync::Arc<tokio::sync::Semaphore>,
}

impl SerialLock {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            turns: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    pub async fn acquire(
        &self,
        stale: Duration,
        wait: Duration,
        on_busy: OnBusy,
    ) -> io::Result<SerialGuard> {
        let turn = self
            .turns
            .clone()
            .acquire_owned()
            .await
            .map_err(io::Error::other)?;
        let path = self.path.clone();
        // The blocking job keeps the turn if its waiting task is cancelled.
        tokio::task::spawn_blocking(move || {
            let file = StaleLock::acquire(&path, stale, wait, on_busy)?;
            Ok(SerialGuard {
                _file: file,
                _turn: turn,
            })
        })
        .await
        .map_err(io::Error::other)?
    }
}

#[derive(Debug)]
pub struct SerialGuard {
    // Release the directory before starting the next local operation.
    _file: Option<StaleLock>,
    _turn: tokio::sync::OwnedSemaphorePermit,
}
