//! The box's record of what its agents made (`artifacts/`), matching `artifacts/store.ts` 1:1.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, Mac};
use jiff::Timestamp;
use kuvo_store::format::ObjectFormat;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use super::{
    budget::budget_confirmation,
    diff::{from_patch, unified},
    index::ArtifactIndex,
    inventory::ARTIFACT_INDEX_FORMAT,
    kinds::{Kind, basename, is_binary, is_text, kind_of, strip_folder_prefix, type_of},
    paths::real_of,
    policy::{CopyPolicy, DiskProbe, NoCopy},
    turnfiles::in_scratch,
};

type HmacSha256 = Hmac<Sha256>;

pub const ARTIFACT_SETTINGS_FORMAT: ObjectFormat =
    ObjectFormat::additive("Artifact storage settings");

/// The biggest file a copy is kept of (25 MiB).
pub const COPY_LIMIT: u64 = 25 * 1024 * 1024;
/// Default budget for older versions' copies (1 GiB).
pub const DEFAULT_BUDGET: u64 = 1024 * 1024 * 1024;
const LINE_LIMIT: usize = 1024 * 1024;
const PATCH_LIMIT: usize = 100_000;
const OUTSIDE_WINDOW_MS: i64 = 5 * 60_000;

pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

pub fn system_clock() -> Timestamp {
    Timestamp::now()
}

/// Format a `Timestamp` identically to JS `Date.prototype.toISOString()` (`YYYY-MM-DDTHH:MM:SS.sssZ`).
pub fn stamp(ts: Timestamp) -> String {
    let ms = ts.as_millisecond();
    let truncated = Timestamp::from_millisecond(ms).unwrap_or(ts);
    let dt = truncated.to_zoned(jiff::tz::TimeZone::UTC);
    let subsec_ms = (ms.rem_euclid(1000)) as u32;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        subsec_ms
    )
}

pub fn parse_millis(iso: &str) -> Option<i64> {
    iso.parse::<Timestamp>().ok().map(Timestamp::as_millisecond)
}

pub fn parse_seconds(iso: &str) -> Option<f64> {
    parse_millis(iso).map(|ms| ms as f64 / 1000.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    You,
    Wake,
    Group,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhoKind {
    Agent,
    You,
    Outside,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Who {
    pub kind: WhoKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionStatus {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineDelta {
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub from: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub n: u64,
    pub at: String,
    pub by: Who,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<Turn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    pub via: Vec<String>,
    pub status: VersionStatus,
    pub edits: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<LineDelta>,
    pub size: u64,
    pub hash: Option<String>,
    pub kept: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<NoCopy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<Handoff>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Where {
    #[serde(default)]
    pub bots: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default)]
    pub sessions: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinBy {
    Bot,
    You,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pinned {
    pub by: PinBy,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactHeader {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: Kind,
    #[serde(rename = "type")]
    pub mime: String,
    #[serde(rename = "where")]
    pub where_: Where,
    pub pinned: Option<Pinned>,
    pub gone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub living: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<bool>,
    pub updated: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: Kind,
    #[serde(rename = "type")]
    pub mime: String,
    #[serde(rename = "where")]
    pub where_: Where,
    pub pinned: Option<Pinned>,
    pub gone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub living: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<bool>,
    pub updated: String,
    pub versions: Vec<Version>,
}

impl Artifact {
    pub fn header(&self) -> ArtifactHeader {
        ArtifactHeader {
            id: self.id.clone(),
            path: self.path.clone(),
            name: self.name.clone(),
            kind: self.kind,
            mime: self.mime.clone(),
            where_: self.where_.clone(),
            pinned: self.pinned.clone(),
            gone: self.gone.clone(),
            shown: self.shown.clone(),
            living: self.living,
            git: self.git,
            updated: self.updated.clone(),
        }
    }

    pub fn from_header(header: ArtifactHeader, versions: Vec<Version>) -> Self {
        Self {
            id: header.id,
            path: header.path,
            name: header.name,
            kind: header.kind,
            mime: header.mime,
            where_: header.where_,
            pinned: header.pinned,
            gone: header.gone,
            shown: header.shown,
            living: header.living,
            git: header.git,
            updated: header.updated,
            versions,
        }
    }

    fn apply_header(&mut self, header: ArtifactHeader) {
        self.id = header.id;
        self.path = header.path;
        self.name = header.name;
        self.kind = header.kind;
        self.mime = header.mime;
        self.where_ = header.where_;
        self.pinned = header.pinned;
        self.gone = header.gone;
        self.shown = header.shown;
        self.living = header.living;
        self.git = header.git;
        self.updated = header.updated;
    }
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub header: ArtifactHeader,
    pub versions: usize,
    pub latest: Version,
}

#[derive(Clone, Debug, Default)]
pub struct MadeWhere {
    pub bot: Option<String>,
    pub project: Option<String>,
    pub session: Option<String>,
    pub group: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Made {
    pub by: Who,
    pub turn: Option<Turn>,
    pub group: Option<GroupRef>,
    pub trigger: Option<Trigger>,
    pub via: String,
    pub where_: MadeWhere,
    pub download: bool,
    pub patch: Option<String>,
    pub separate: bool,
    pub keep: bool,
    pub at: Option<Timestamp>,
}

impl Made {
    pub fn outside(via: impl Into<String>) -> Self {
        Self {
            by: Who {
                kind: WhoKind::Outside,
                session: None,
                bot: None,
                agent: None,
            },
            turn: None,
            group: None,
            trigger: None,
            via: via.into(),
            where_: MadeWhere::default(),
            download: false,
            patch: None,
            separate: false,
            keep: false,
            at: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub session: Option<String>,
    pub bot: Option<String>,
    pub group: Option<String>,
    pub project: Option<String>,
    pub kind: Option<Kind>,
    pub pinned: Option<bool>,
    pub by: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StorageInfo {
    pub copies: u64,
    pub older: u64,
    pub budget: u64,
    pub free: u64,
    pub floor: u64,
    pub low: bool,
}

#[derive(Clone, Debug)]
pub struct ShownCitation {
    pub path: String,
    pub at_ms: i64,
}

#[derive(Clone, Default)]
pub struct StoreOptions {
    pub bots: Option<PathBuf>,
    pub budget: Option<u64>,
    pub clock: Option<Clock>,
    pub disk: Option<DiskProbe>,
    pub shown: Vec<ShownCitation>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    UnsupportedFormat(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<kuvo_store::format::Error> for StoreError {
    fn from(err: kuvo_store::format::Error) -> Self {
        match err {
            kuvo_store::format::Error::Unsupported { .. } => {
                Self::UnsupportedFormat(err.to_string())
            }
            other => Self::Io(io::Error::other(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    size: u64,
    mtime_ms: i64,
    ctime_ms: i64,
    ino: u64,
}

impl FileStamp {
    fn of(meta: &fs::Metadata) -> Self {
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
        #[cfg(unix)]
        let (ctime_ms, ino) = {
            use std::os::unix::fs::MetadataExt;
            let ctime_ms = meta.ctime() * 1000 + meta.ctime_nsec() / 1_000_000;
            (ctime_ms, meta.ino())
        };
        #[cfg(not(unix))]
        let (ctime_ms, ino) = (mtime_ms, 0u64);
        Self {
            size: meta.len(),
            mtime_ms,
            ctime_ms,
            ino,
        }
    }

    fn could_be_rewritten(&self, at_ms: i64) -> bool {
        let latest = self.mtime_ms.max(self.ctime_ms);
        let coarse = self.mtime_ms % 1000 == 0 || self.ctime_ms % 1000 == 0;
        at_ms - latest < if coarse { 2000 } else { 100 }
    }
}

#[derive(Clone)]
struct LookedFile {
    stamp: FileStamp,
    bytes: Option<Vec<u8>>,
}

#[derive(Clone)]
enum LookOutcome {
    Missing,
    Unreadable,
    File(LookedFile),
}

#[derive(Clone)]
enum Waiting {
    Read,
    Seen { seen: LookOutcome, at_ms: i64 },
    Before(Vec<u8>),
}

#[derive(Clone, Copy)]
struct CopyEntry {
    uses: u64,
    size: u64,
}

#[derive(Clone, Copy)]
struct RecordedStamp {
    stamp: FileStamp,
    at_ms: i64,
}

struct ReadFile {
    hash: String,
    size: u64,
    bytes: Option<Vec<u8>>,
    private: Option<NoCopy>,
}

enum ReadOutcome {
    Missing,
    Unreadable,
    File(ReadFile),
}

#[derive(Serialize, Deserialize)]
struct JournalLine {
    a: ArtifactHeader,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v: Option<Version>,
}

struct StoreInner {
    root: PathBuf,
    bots: Option<PathBuf>,
    default_budget: Option<u64>,
    chosen_budget: Option<u64>,
    clock: Clock,
    policy: CopyPolicy,
    key: [u8; 32],
    artifacts: ArtifactIndex,
    copies: HashMap<String, CopyEntry>,
    copy_bytes: u64,
    waiting: HashMap<String, VecDeque<Waiting>>,
    recorded: HashMap<String, RecordedStamp>,
    files: HashMap<String, String>,
    changes: broadcast::Sender<Artifact>,
}

#[derive(Clone)]
pub struct ArtifactStore {
    inner: Arc<Mutex<StoreInner>>,
    policy: CopyPolicy,
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

struct WaitingGuard {
    inner: Arc<Mutex<StoreInner>>,
    paths: Vec<String>,
}

impl Drop for WaitingGuard {
    fn drop(&mut self) {
        let mut guard = self.inner.lock().unwrap();
        for path in &self.paths {
            if let Some(queue) = guard.waiting.get_mut(path) {
                queue.pop_front();
                if queue.is_empty() {
                    guard.waiting.remove(path);
                }
            }
        }
    }
}

fn kinds_kept_whole(kind: Kind) -> bool {
    matches!(kind, Kind::Memory | Kind::Skill)
}

fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl ArtifactStore {
    pub fn open(root: impl AsRef<Path>, options: StoreOptions) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("log"))?;
        fs::create_dir_all(root.join("copies"))?;
        let policy = match options.disk {
            Some(disk) => CopyPolicy::new(disk),
            None => CopyPolicy::for_folder(root.clone()),
        };
        let chosen_budget = read_settings(&root)?
            .get("budget")
            .and_then(Value::as_u64);
        let key = key_in(&root)?;
        let artifacts = ArtifactIndex::open(root.join("log"), &root.join("index-v1.sqlite"))?;
        let (changes, _) = broadcast::channel(256);
        let clock = options.clock.unwrap_or_else(|| Arc::new(system_clock));
        let mut inner = StoreInner {
            root,
            bots: options.bots,
            default_budget: options.budget,
            chosen_budget,
            clock,
            policy: policy.clone(),
            key,
            artifacts,
            copies: HashMap::new(),
            copy_bytes: 0,
            waiting: HashMap::new(),
            recorded: HashMap::new(),
            files: HashMap::new(),
            changes,
        };
        inner.load(&options.shown)?;
        let store = Self {
            inner: Arc::new(Mutex::new(inner)),
            policy,
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        store.resolve_files();
        Ok(store)
    }

    pub fn id_of(path: &str) -> String {
        let digest = hex::encode(Sha256::digest(path.as_bytes()));
        format!("art_{}", &digest[..24])
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Artifact> {
        self.inner.lock().unwrap().changes.subscribe()
    }

    pub fn get(&self, id: &str) -> Option<Artifact> {
        self.inner.lock().unwrap().artifacts.get(id)
    }

    pub fn at(&self, path: &str) -> Option<Artifact> {
        self.get(&Self::id_of(path))
    }

    pub fn can_restore(&self, header: &ArtifactHeader) -> bool {
        self.inner.lock().unwrap().can_restore(header)
    }

    pub fn resolve_files(&self) {
        let mut inner = self.inner.lock().unwrap();
        let mut after = String::new();
        loop {
            let batch = inner.artifacts.paths(&after);
            if batch.is_empty() {
                break;
            }
            after = batch.last().unwrap().0.clone();
            for (id, path) in batch {
                if inner.files.contains_key(&id) {
                    continue;
                }
                if let Some(real) = real_of(Path::new(&path)) {
                    let real_str = real.to_string_lossy().into_owned();
                    if real_str != path {
                        inner.files.insert(id, real_str);
                    }
                }
            }
        }
    }

    /// Whether `path` matches what the record already has or will record from queued work.
    pub fn as_seen(&self, path: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        if let Some(last) = inner.waiting.get(path).and_then(VecDeque::back) {
            if matches!(last, Waiting::Read) {
                return true;
            }
            let now = stat_file_stamp(Path::new(path));
            match last {
                Waiting::Before(before) => {
                    let StatStamp::File(st) = now else {
                        return false;
                    };
                    if st.size != before.len() as u64 {
                        return false;
                    }
                    matches!(look(Path::new(path)), LookOutcome::File(f) if f.bytes.as_deref() == Some(before.as_slice()))
                }
                Waiting::Seen { seen, at_ms } => match (seen, now) {
                    (LookOutcome::Missing, StatStamp::Missing) => true,
                    (LookOutcome::File(seen_f), StatStamp::File(now_st)) => {
                        if seen_f.stamp != now_st {
                            return false;
                        }
                        if !seen_f.stamp.could_be_rewritten(*at_ms) {
                            return true;
                        }
                        match look(Path::new(path)) {
                            LookOutcome::File(read_f) => match (&read_f.bytes, &seen_f.bytes) {
                                (Some(a), Some(b)) => a == b,
                                _ => read_f.stamp == seen_f.stamp,
                            },
                            _ => false,
                        }
                    }
                    _ => false,
                },
                Waiting::Read => true,
            }
        } else {
            let now = stat_file_stamp(Path::new(path));
            let artifact = inner.artifacts.get(&Self::id_of(path));
            match now {
                StatStamp::Unreadable => true,
                StatStamp::Missing => artifact.is_none_or(|a| a.gone.is_some()),
                StatStamp::File(now_st) => {
                    let Some(artifact) = artifact else {
                        return false;
                    };
                    let Some(latest) = artifact.versions.last() else {
                        return false;
                    };
                    if artifact.gone.is_some() || latest.size != now_st.size {
                        return false;
                    }
                    if let Some(recorded) = inner.recorded.get(path) {
                        if recorded.stamp != now_st {
                            return false;
                        }
                        if !recorded.stamp.could_be_rewritten(recorded.at_ms) {
                            return true;
                        }
                    }
                    match inner.read_file(Path::new(path), None) {
                        ReadOutcome::Unreadable => true,
                        ReadOutcome::File(read) => latest.hash.as_deref() == Some(&read.hash),
                        ReadOutcome::Missing => false,
                    }
                }
            }
        }
    }

    pub async fn settled(&self) {
        let _guard = self.write_lock.lock().await;
    }

    fn enqueue_waiting(&self, paths: &[String], waits: Option<Waiting>) -> WaitingGuard {
        if let Some(waits) = waits {
            let mut inner = self.inner.lock().unwrap();
            for path in paths {
                inner
                    .waiting
                    .entry(path.clone())
                    .or_default()
                    .push_back(waits.clone());
            }
            WaitingGuard {
                inner: self.inner.clone(),
                paths: paths.to_vec(),
            }
        } else {
            WaitingGuard {
                inner: self.inner.clone(),
                paths: Vec::new(),
            }
        }
    }

    pub fn change(
        &self,
        path: &str,
        made: Made,
    ) -> impl std::future::Future<Output = Option<Artifact>> + Send + 'static {
        let at_ms = wall_ms();
        let seen = look(Path::new(path));
        let path = path.to_owned();
        let wait = self.enqueue_waiting(
            std::slice::from_ref(&path),
            Some(Waiting::Seen {
                seen: seen.clone(),
                at_ms,
            }),
        );
        let this = self.clone();
        async move {
            let _wait = wait;
            let _lock = this.write_lock.lock().await;
            this.policy.learn(&[PathBuf::from(&path)]).await;
            let mut inner = this.inner.lock().unwrap();
            inner.change_now(&path, &made, Some((seen, at_ms)))
        }
    }

    pub fn change_all(
        &self,
        paths: &[String],
        made: Made,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        let paths = paths.to_vec();
        let wait = if paths.is_empty() {
            None
        } else {
            Some(self.enqueue_waiting(&paths, Some(Waiting::Read)))
        };
        let this = self.clone();
        async move {
            let Some(_wait) = wait else {
                return;
            };
            let _lock = this.write_lock.lock().await;
            let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            this.policy.learn(&pb).await;
            for path in &paths {
                {
                    let mut inner = this.inner.lock().unwrap();
                    let _ = inner.change_now(path, &made, None);
                }
                if paths.len() > 1 {
                    tokio::task::yield_now().await;
                }
            }
        }
    }

    pub fn before(
        &self,
        path: &str,
        bytes: Vec<u8>,
        bot: Option<String>,
        at: Option<Timestamp>,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        let path = path.to_owned();
        let should_wait = {
            let inner = self.inner.lock().unwrap();
            inner.artifacts.get(&Self::id_of(&path)).is_none() && !inner.waiting.contains_key(&path)
        };
        let wait = self.enqueue_waiting(
            std::slice::from_ref(&path),
            should_wait.then(|| Waiting::Before(bytes.clone())),
        );
        let this = self.clone();
        async move {
            let _wait = wait;
            let _lock = this.write_lock.lock().await;
            this.policy.learn(&[PathBuf::from(&path)]).await;
            let mut inner = this.inner.lock().unwrap();
            let when = at.unwrap_or_else(|| (inner.clock)());
            inner.before_now(&path, &bytes, bot.as_deref(), when);
        }
    }

    pub async fn pin(&self, id: &str, pinned: Option<(PinBy, Option<String>)>) -> Option<Artifact> {
        let path = self.get(id).map(|a| PathBuf::from(a.path));
        let _lock = self.write_lock.lock().await;
        if let Some(path) = path {
            self.policy.learn(&[path]).await;
        }
        let mut inner = self.inner.lock().unwrap();
        inner.pin_now(id, pinned)
    }

    pub async fn restore(&self, id: &str, n: u64, at: Option<Timestamp>) -> Result<Artifact, String> {
        let path = self.get(id).map(|a| PathBuf::from(a.path));
        let _lock = self.write_lock.lock().await;
        if let Some(path) = path {
            self.policy.learn(&[path]).await;
        }
        let mut inner = self.inner.lock().unwrap();
        let when = at.unwrap_or_else(|| (inner.clock)());
        inner.restore_now(id, n, when)
    }

    pub fn shown(
        &self,
        path: &str,
        made: Made,
    ) -> impl std::future::Future<Output = Option<Artifact>> + Send + 'static {
        let path = path.to_owned();
        let wait = self.enqueue_waiting(std::slice::from_ref(&path), Some(Waiting::Read));
        let this = self.clone();
        async move {
            let _wait = wait;
            let _lock = this.write_lock.lock().await;
            this.policy.learn(&[PathBuf::from(&path)]).await;
            let mut inner = this.inner.lock().unwrap();
            inner.shown_now(&path, &made)
        }
    }

    pub async fn check_gone(&self, id: &str) -> Option<Artifact> {
        let _lock = self.write_lock.lock().await;
        let mut inner = self.inner.lock().unwrap();
        inner.check_gone_now(id)
    }

    /// `None` = no such version; `Some(None)` = version exists but not kept; `Some(Some(bytes))` = copy bytes.
    pub fn bytes(&self, id: &str, n: u64) -> Option<Option<Vec<u8>>> {
        self.inner.lock().unwrap().bytes(id, n)
    }

    pub async fn image_preview(&self, id: &str, n: u64) -> Option<Option<Vec<u8>>> {
        let copy = self.bytes(id, n)?;
        if copy.is_some() {
            return Some(copy);
        }
        let candidate = {
            let inner = self.inner.lock().unwrap();
            let artifact = inner.artifacts.get(id)?;
            let version = artifact.versions.last()?;
            if artifact.kind != Kind::Image
                || version.n != n
                || !version
                    .hash
                    .as_deref()
                    .is_some_and(|h| h.starts_with("sha256:"))
                || version.reason.is_some_and(NoCopy::is_private)
            {
                return Some(None);
            }
            (PathBuf::from(artifact.path), version.hash.clone().unwrap())
        };
        self.policy.learn(std::slice::from_ref(&candidate.0)).await;
        let inner = self.inner.lock().unwrap();
        match inner.read_file(&candidate.0, None) {
            ReadOutcome::File(file) if file.hash == candidate.1 => Some(file.bytes),
            _ => Some(None),
        }
    }

    pub fn list(&self, scope: &Scope, limit: usize, before: Option<&str>) -> (Vec<Summary>, Option<String>) {
        self.inner.lock().unwrap().list(scope, limit, before)
    }

    pub fn session_outputs(&self, sessions: Option<&[String]>) -> Vec<Summary> {
        self.inner.lock().unwrap().session_outputs(sessions)
    }

    pub fn timeline(
        &self,
        scope: &Scope,
        limit: usize,
        before: Option<&str>,
    ) -> (Vec<(ArtifactHeader, Version)>, Option<String>) {
        self.inner.lock().unwrap().timeline(scope, limit, before)
    }

    pub fn storage(&self) -> StorageInfo {
        self.inner.lock().unwrap().storage()
    }

    pub fn set_budget(&self, bytes: u64, confirmed: bool) -> Result<Option<String>, StoreError> {
        self.inner.lock().unwrap().set_budget(bytes, confirmed)
    }

    pub fn clear_older(&self, before: Option<&str>) -> usize {
        self.inner.lock().unwrap().clear_older(before)
    }
}

enum StatStamp {
    Missing,
    Unreadable,
    File(FileStamp),
}

fn stat_file_stamp(path: &Path) -> StatStamp {
    match fs::metadata(path) {
        Ok(meta) => {
            if meta.is_file() {
                StatStamp::File(FileStamp::of(&meta))
            } else {
                StatStamp::Missing
            }
        }
        Err(err) if is_not_found(&err) => StatStamp::Missing,
        Err(_) => StatStamp::Unreadable,
    }
}

fn is_not_found(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound || err.kind() == io::ErrorKind::NotADirectory
}

fn look(path: &Path) -> LookOutcome {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if is_not_found(&err) => return LookOutcome::Missing,
        Err(_) => return LookOutcome::Unreadable,
    };
    if !meta.is_file() {
        return LookOutcome::Missing;
    }
    let mut stamp = FileStamp::of(&meta);
    if stamp.size > COPY_LIMIT {
        return LookOutcome::File(LookedFile { stamp, bytes: None });
    }
    match fs::read(path) {
        Ok(bytes) => {
            stamp.size = bytes.len() as u64;
            LookOutcome::File(LookedFile {
                stamp,
                bytes: Some(bytes),
            })
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => LookOutcome::Missing,
        Err(_) => LookOutcome::Unreadable,
    }
}

fn key_in(root: &Path) -> io::Result<[u8; 32]> {
    let file = root.join("key");
    if let Ok(bytes) = fs::read(&file)
        && bytes.len() == 32
    {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(io::Error::other)?;
    kuvo_store::files::write_secret(&file, &key)?;
    Ok(key)
}

fn read_settings(root: &Path) -> Result<Map<String, Value>, StoreError> {
    let path = root.join("settings.json");
    let raw = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(Map::new()),
    };
    let Ok(val) = serde_json::from_str::<Value>(&raw) else {
        return Ok(Map::new());
    };
    if !val.is_object() {
        return Ok(Map::new());
    }
    Ok(ARTIFACT_SETTINGS_FORMAT.decode(val)?.fields)
}

impl StoreInner {
    fn now(&self) -> Timestamp {
        (self.clock)()
    }

    fn budget(&self) -> u64 {
        self.chosen_budget
            .or(self.default_budget)
            .unwrap_or(DEFAULT_BUDGET)
    }

    fn copy_path(&self, hash: &str) -> PathBuf {
        let hex = hash.strip_prefix("sha256:").unwrap_or(hash);
        let prefix = if hex.len() >= 2 { &hex[..2] } else { hex };
        self.root.join("copies").join(prefix).join(hex)
    }

    fn keep_copy(&self, hash: &str, bytes: &[u8]) -> io::Result<()> {
        let file = self.copy_path(hash);
        if file.exists() {
            return Ok(());
        }
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(format!("{}.new", file.display()));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &file)?;
        Ok(())
    }

    fn use_copy(&mut self, hash: &str, size: u64, delta: i64) {
        if !hash.starts_with("sha256:") {
            return;
        }
        let entry = self
            .copies
            .entry(hash.to_owned())
            .or_insert(CopyEntry { uses: 0, size });
        if entry.uses == 0 && delta > 0 {
            self.copy_bytes = self.copy_bytes.saturating_add(entry.size);
        }
        if delta > 0 {
            entry.uses = entry.uses.saturating_add(delta as u64);
        } else {
            entry.uses = entry.uses.saturating_sub((-delta) as u64);
        }
        if entry.uses > 0 {
            return;
        }
        let removed_size = entry.size;
        self.copies.remove(hash);
        self.copy_bytes = self.copy_bytes.saturating_sub(removed_size);
        let _ = fs::remove_file(self.copy_path(hash));
    }

    fn told(&self, artifact: Artifact) -> Artifact {
        let _ = self.changes.send(artifact.clone());
        artifact
    }

    fn write_entry(&mut self, artifact: &Artifact, version: Option<&Version>) -> io::Result<()> {
        let line = JournalLine {
            a: artifact.header(),
            v: version.cloned(),
        };
        let Value::Object(fields) = serde_json::to_value(line).map_err(io::Error::other)? else {
            unreachable!();
        };
        let encoded = serde_json::to_string(&ARTIFACT_INDEX_FORMAT.encode(fields))
            .map_err(io::Error::other)?;
        let month = &stamp(self.now())[..7];
        let log_file = self.root.join("log").join(format!("{month}.jsonl"));
        let mut file = OpenOptions::new().create(true).append(true).open(&log_file)?;
        file.write_all(encoded.as_bytes())?;
        file.write_all(b"\n")?;
        drop(file);
        self.artifacts.appended(artifact)?;
        if !self.files.contains_key(&artifact.id)
            && let Some(real) = real_of(Path::new(&artifact.path))
        {
            let real_str = real.to_string_lossy().into_owned();
            if real_str != artifact.path {
                self.files.insert(artifact.id.clone(), real_str);
            }
        }
        Ok(())
    }

    fn can_restore(&self, header: &ArtifactHeader) -> bool {
        let in_bots = self
            .bots
            .as_ref()
            .is_some_and(|b| strip_folder_prefix(&header.path, &b.to_string_lossy()).is_some());
        let in_project = header
            .where_
            .project
            .as_deref()
            .is_some_and(|p| strip_folder_prefix(&header.path, p).is_some());
        in_bots || in_project
    }

    fn read_file(&self, path: &Path, seen: Option<LookOutcome>) -> ReadOutcome {
        let seen = seen.unwrap_or_else(|| look(path));
        let looked = match seen {
            LookOutcome::Missing => return ReadOutcome::Missing,
            LookOutcome::Unreadable => return ReadOutcome::Unreadable,
            LookOutcome::File(file) => file,
        };
        let privacy = self.policy.privacy(path);
        let Some(bytes) = looked.bytes else {
            return ReadOutcome::File(ReadFile {
                hash: format!("stat:{}:{}", looked.stamp.size, looked.stamp.mtime_ms),
                size: looked.stamp.size,
                bytes: None,
                private: privacy,
            });
        };
        if let Some(privacy) = privacy {
            let mut mac = HmacSha256::new_from_slice(&self.key).expect("32-byte key");
            mac.update(&bytes);
            let digest = hex::encode(mac.finalize().into_bytes());
            return ReadOutcome::File(ReadFile {
                hash: format!("keyed:{digest}"),
                size: looked.stamp.size,
                bytes: None,
                private: Some(privacy),
            });
        }
        let digest = hex::encode(Sha256::digest(&bytes));
        ReadOutcome::File(ReadFile {
            hash: format!("sha256:{digest}"),
            size: looked.stamp.size,
            bytes: Some(bytes),
            private: None,
        })
    }

    fn no_copy(&self, artifact: &Artifact, readable: bool) -> Option<NoCopy> {
        if !readable {
            return Some(NoCopy::TooBig);
        }
        let always = artifact.pinned.is_some() || kinds_kept_whole(artifact.kind);
        if !always && artifact.kind == Kind::Code && self.policy.in_git(Path::new(&artifact.path)) {
            return Some(NoCopy::Git);
        }
        None
    }

    fn refine(kind: Kind, bytes: Option<&[u8]>) -> Kind {
        if kind != Kind::Code {
            return kind;
        }
        match bytes {
            Some(b) if is_binary(b) => Kind::File,
            _ => kind,
        }
    }

    fn note(artifact: &mut Artifact, made: &Made) -> bool {
        let mut changed = false;
        if let Some(bot) = &made.where_.bot
            && !artifact.where_.bots.contains(bot)
        {
            artifact.where_.bots.push(bot.clone());
            changed = true;
        }
        if let Some(project) = &made.where_.project
            && artifact.where_.project.is_none()
        {
            artifact.where_.project = Some(project.clone());
            changed = true;
        }
        if let Some(session) = &made.where_.session
            && !artifact.where_.sessions.contains(session)
        {
            artifact.where_.sessions.push(session.clone());
            changed = true;
        }
        if let Some(group) = &made.where_.group
            && !artifact.where_.groups.contains(group)
        {
            artifact.where_.groups.push(group.clone());
            changed = true;
        }
        changed
    }

    fn same_turn(latest: &Version, made: &Made, now: Timestamp) -> bool {
        if latest.before == Some(true)
            || made.separate
            || latest.via.iter().any(|v| v == "restore" || v == "replaced")
        {
            return false;
        }
        if let (Some(m_turn), Some(l_turn)) = (&made.turn, &latest.turn) {
            return m_turn.session == l_turn.session && m_turn.message == l_turn.message;
        }
        if made.turn.is_none()
            && latest.turn.is_none()
            && made.by.kind == latest.by.kind
            && made.by.kind != WhoKind::Agent
        {
            return parse_millis(&latest.at)
                .is_some_and(|prev| now.as_millisecond() - prev < OUTSIDE_WINDOW_MS);
        }
        false
    }

    fn change_now(
        &mut self,
        path: &str,
        made: &Made,
        looked: Option<(LookOutcome, i64)>,
    ) -> Option<Artifact> {
        let now = made.at.unwrap_or_else(|| self.now());
        let (seen, at_ms) = looked.unwrap_or_else(|| (look(Path::new(path)), wall_ms()));
        let seen_stamp = match &seen {
            LookOutcome::File(f) => Some(f.stamp),
            _ => None,
        };
        let read = match self.read_file(Path::new(path), Some(seen)) {
            ReadOutcome::Unreadable => return None,
            ReadOutcome::Missing => {
                self.recorded.remove(path);
                let mut artifact = self.artifacts.get(&ArtifactStore::id_of(path))?;
                if artifact.gone.is_some() {
                    return None;
                }
                artifact.gone = Some(stamp(now));
                Self::note(&mut artifact, made);
                return self.add_version(
                    artifact,
                    PendingVersion {
                        status: VersionStatus::Deleted,
                        hash: None,
                        size: 0,
                        kept: false,
                        reason: None,
                        patch: None,
                    },
                    made,
                    now,
                    None,
                );
            }
            ReadOutcome::File(read) => read,
        };

        let mut artifact = match self.artifacts.get(&ArtifactStore::id_of(path)) {
            Some(existing) => existing,
            None => {
                let kind = Self::refine(
                    kind_of(path, self.bots.as_deref(), made.download),
                    read.bytes.as_deref(),
                );
                let binary = read.bytes.as_deref().is_some_and(is_binary);
                Artifact {
                    id: ArtifactStore::id_of(path),
                    path: path.to_owned(),
                    name: basename(path).to_owned(),
                    kind,
                    mime: type_of(path, binary).to_owned(),
                    where_: Where::default(),
                    pinned: None,
                    gone: None,
                    shown: None,
                    living: None,
                    git: self.policy.in_git(Path::new(path)).then_some(true),
                    updated: stamp(now),
                    versions: Vec::new(),
                }
            }
        };

        let unchanged = artifact.versions.last().is_some_and(|latest| {
            latest.hash.as_deref() == Some(read.hash.as_str())
                && (artifact.gone.is_none() || latest.status != VersionStatus::Deleted)
        });
        if unchanged {
            if let Some(st) = seen_stamp {
                self.recorded
                    .insert(path.to_owned(), RecordedStamp { stamp: st, at_ms });
            }
            let back = artifact.gone.is_some();
            artifact.gone = None;
            if made.keep
                && let Some(latest) = artifact.versions.last_mut()
                && !latest.kept
                && let Some(bytes) = &read.bytes
                && !latest.reason.is_some_and(NoCopy::is_private)
            {
                if self.keep_copy(&read.hash, bytes).is_err() {
                    return None;
                }
                self.use_copy(&read.hash, read.size, 1);
                latest.kept = true;
                latest.reason = None;
                let latest_clone = latest.clone();
                let _ = self.write_entry(&artifact, Some(&latest_clone));
            }
            let noted = Self::note(&mut artifact, made);
            let same_turn = artifact
                .versions
                .last()
                .is_some_and(|latest| Self::same_turn(latest, made, now));
            if same_turn
                && made.via != "shell"
                && made.via != "outside"
                && let Some(latest) = artifact.versions.last_mut()
                && !latest.via.contains(&made.via)
            {
                latest.via.push(made.via.clone());
                let latest_clone = latest.clone();
                let _ = self.write_entry(&artifact, Some(&latest_clone));
                return Some(self.told(artifact));
            }
            if noted || back {
                let _ = self.write_entry(&artifact, None);
                return Some(self.told(artifact));
            }
            return None;
        }

        artifact.gone = None;
        Self::note(&mut artifact, made);
        let reason = read.private.or_else(|| {
            if made.keep {
                read.bytes.is_none().then_some(NoCopy::TooBig)
            } else {
                self.no_copy(&artifact, read.bytes.is_some())
            }
        });
        if reason.is_none()
            && let Some(bytes) = &read.bytes
            && self.keep_copy(&read.hash, bytes).is_err()
        {
            return None;
        }
        let patch = if reason == Some(NoCopy::Git) {
            made.patch.as_deref().map(|p| truncate_chars(p, PATCH_LIMIT))
        } else {
            None
        };
        let status = match artifact.versions.last() {
            Some(latest) if latest.status != VersionStatus::Deleted => VersionStatus::Modified,
            _ => VersionStatus::Added,
        };
        let added = self.add_version(
            artifact,
            PendingVersion {
                status,
                hash: Some(read.hash),
                size: read.size,
                kept: reason.is_none(),
                reason,
                patch,
            },
            made,
            now,
            if reason.is_none() {
                read.bytes.as_deref()
            } else {
                None
            },
        );
        if added.is_some()
            && let Some(st) = seen_stamp
        {
            self.recorded
                .insert(path.to_owned(), RecordedStamp { stamp: st, at_ms });
        }
        added
    }

    fn before_now(&mut self, path: &str, bytes: &[u8], bot: Option<&str>, at: Timestamp) {
        if self.artifacts.get(&ArtifactStore::id_of(path)).is_some()
            || bytes.len() as u64 > COPY_LIMIT
            || self.policy.privacy(Path::new(path)).is_some()
        {
            return;
        }
        let kind = Self::refine(kind_of(path, self.bots.as_deref(), false), Some(bytes));
        if !kinds_kept_whole(kind)
            && ((kind == Kind::Code && self.policy.in_git(Path::new(path)))
                || self.policy.low_on(bytes.len() as u64))
        {
            return;
        }
        let hash = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        if self.keep_copy(&hash, bytes).is_err() {
            return;
        }
        let version = Version {
            n: 0,
            at: stamp(at),
            by: Who {
                kind: WhoKind::Outside,
                session: None,
                bot: None,
                agent: None,
            },
            turn: None,
            group: None,
            trigger: None,
            via: Vec::new(),
            status: VersionStatus::Added,
            edits: 0,
            lines: None,
            size: bytes.len() as u64,
            hash: Some(hash.clone()),
            kept: true,
            reason: None,
            patch: None,
            restored: None,
            before: Some(true),
            handoff: None,
        };
        let artifact = Artifact {
            id: ArtifactStore::id_of(path),
            path: path.to_owned(),
            name: basename(path).to_owned(),
            kind,
            mime: type_of(path, is_binary(bytes)).to_owned(),
            where_: Where {
                bots: bot.into_iter().map(str::to_owned).collect(),
                project: None,
                sessions: Vec::new(),
                groups: Vec::new(),
            },
            pinned: None,
            gone: None,
            shown: None,
            living: None,
            git: None,
            updated: stamp(at),
            versions: vec![version.clone()],
        };
        self.recorded.remove(path);
        self.use_copy(&hash, bytes.len() as u64, 1);
        let _ = self.write_entry(&artifact, Some(&version));
    }

    fn pin_now(&mut self, id: &str, pinned: Option<(PinBy, Option<String>)>) -> Option<Artifact> {
        let mut artifact = self.artifacts.get(id)?;
        artifact.pinned = pinned.as_ref().map(|(by, note)| Pinned {
            by: *by,
            at: stamp(self.now()),
            note: note.clone(),
        });
        let _ = self.write_entry(&artifact, None);
        if pinned.is_some()
            && let Some(latest) = artifact.versions.last()
            && !latest.kept
            && latest.reason == Some(NoCopy::Git)
            && let Some(expected_hash) = latest.hash.clone()
            && let ReadOutcome::File(read) = self.read_file(Path::new(&artifact.path), None)
            && let Some(bytes) = read.bytes
            && read.hash == expected_hash
            && self.keep_copy(&read.hash, &bytes).is_ok()
        {
            self.use_copy(&read.hash, read.size, 1);
            if let Some(latest_mut) = artifact.versions.last_mut() {
                latest_mut.kept = true;
                latest_mut.reason = None;
                let latest_clone = latest_mut.clone();
                let _ = self.write_entry(&artifact, Some(&latest_clone));
            }
        }
        Some(self.told(artifact))
    }

    fn restore_now(&mut self, id: &str, n: u64, at: Timestamp) -> Result<Artifact, String> {
        let artifact = self
            .artifacts
            .get(id)
            .ok_or_else(|| "There's no such version.".to_owned())?;
        let version = artifact
            .versions
            .iter()
            .find(|v| v.n == n)
            .cloned()
            .ok_or_else(|| "There's no such version.".to_owned())?;
        if !self.can_restore(&artifact.header()) {
            return Err(
                "It's outside its bot's folder and its project, so it isn't put back from here."
                    .to_owned(),
            );
        }
        let bytes = match self.bytes(id, n) {
            Some(Some(bytes)) => bytes,
            _ => {
                return Err(if version.status == VersionStatus::Deleted {
                    "That version is a deletion: there's nothing to put back.".to_owned()
                } else {
                    "That version's copy isn't kept, so it can't be put back.".to_owned()
                });
            }
        };
        let replaced_made = Made {
            by: Who {
                kind: WhoKind::You,
                session: None,
                bot: None,
                agent: None,
            },
            turn: None,
            group: None,
            trigger: None,
            via: "replaced".to_owned(),
            where_: MadeWhere::default(),
            download: false,
            patch: None,
            separate: true,
            keep: true,
            at: Some(at),
        };
        self.change_now(&artifact.path, &replaced_made, None);
        let latest_before = self
            .artifacts
            .get(id)
            .and_then(|a| a.versions.last().cloned());
        if let Some(latest) = &latest_before
            && latest.status != VersionStatus::Deleted
            && !latest.kept
        {
            return Err(
                "What's there now can't be kept, so putting this back couldn't be undone."
                    .to_owned(),
            );
        }

        let target = Path::new(&artifact.path);
        let tmp = PathBuf::from(format!("{}.kuvo-restore", artifact.path));
        if let Err(err) = write_restore_file(target, &tmp, &bytes) {
            let _ = fs::remove_file(&tmp);
            return Err(format!("It couldn't be written: {err}"));
        }

        let restore_made = Made {
            by: Who {
                kind: WhoKind::You,
                session: None,
                bot: None,
                agent: None,
            },
            turn: None,
            group: None,
            trigger: None,
            via: "restore".to_owned(),
            where_: MadeWhere::default(),
            download: false,
            patch: None,
            separate: true,
            keep: true,
            at: Some(at),
        };
        let mut updated = self
            .change_now(&artifact.path, &restore_made, None)
            .or_else(|| self.artifacts.get(id))
            .ok_or_else(|| "It couldn't be put back.".to_owned())?;
        if let Some(restored) = updated.versions.last_mut()
            && Some(restored.n) != latest_before.as_ref().map(|v| v.n)
        {
            restored.restored = Some(n);
            let restored_clone = restored.clone();
            let _ = self.write_entry(&updated, Some(&restored_clone));
        }
        Ok(updated)
    }

    fn shown_now(&mut self, path: &str, made: &Made) -> Option<Artifact> {
        let mut artifact = self
            .change_now(path, made, None)
            .or_else(|| self.artifacts.get(&ArtifactStore::id_of(path)))?;
        artifact.shown = Some(stamp(made.at.unwrap_or_else(|| self.now())));
        let _ = self.write_entry(&artifact, None);
        Some(self.told(artifact))
    }

    fn check_gone_now(&mut self, id: &str) -> Option<Artifact> {
        let mut artifact = self.artifacts.get(id)?;
        if artifact
            .versions
            .last()
            .is_some_and(|v| v.status == VersionStatus::Deleted)
        {
            return Some(artifact);
        }
        let there = match fs::metadata(&artifact.path) {
            Ok(_) => true,
            Err(err) if is_not_found(&err) => false,
            Err(_) => true,
        };
        if there == artifact.gone.is_none() {
            return Some(artifact);
        }
        artifact.gone = if there { None } else { Some(stamp(self.now())) };
        let _ = self.write_entry(&artifact, None);
        Some(self.told(artifact))
    }

    fn bytes(&self, id: &str, n: u64) -> Option<Option<Vec<u8>>> {
        let version = self.artifacts.get(id)?.versions.into_iter().find(|v| v.n == n)?;
        if !version.kept
            || version.reason.is_some_and(NoCopy::is_private)
            || !version
                .hash
                .as_deref()
                .is_some_and(|h| h.starts_with("sha256:"))
        {
            return Some(None);
        }
        let hash = version.hash.unwrap();
        let Ok(bytes) = fs::read(self.copy_path(&hash)) else {
            return Some(None);
        };
        if bytes.len() as u64 == version.size
            && format!("sha256:{}", hex::encode(Sha256::digest(&bytes))) == hash
        {
            Some(Some(bytes))
        } else {
            Some(None)
        }
    }

    fn file_of<'a>(&'a self, artifact: &'a Artifact) -> &'a str {
        self.files
            .get(&artifact.id)
            .map(String::as_str)
            .unwrap_or(&artifact.path)
    }

    fn matches(artifact: &Artifact, scope: &Scope) -> bool {
        if let Some(kind) = scope.kind
            && artifact.kind != kind
        {
            return false;
        }
        if scope.pinned == Some(true) && artifact.pinned.is_none() {
            return false;
        }
        if let Some(session) = &scope.session
            && !artifact.where_.sessions.contains(session)
        {
            return false;
        }
        if let Some(bot) = &scope.bot
            && !artifact.where_.bots.contains(bot)
        {
            return false;
        }
        if let Some(group) = &scope.group
            && !artifact.where_.groups.contains(group)
        {
            return false;
        }
        if let Some(project) = &scope.project
            && artifact.where_.project.as_deref() != Some(project.as_str())
            && strip_folder_prefix(&artifact.path, project).is_none()
        {
            return false;
        }
        true
    }

    fn list(&self, scope: &Scope, limit: usize, before: Option<&str>) -> (Vec<Summary>, Option<String>) {
        let mut seen = HashSet::new();
        let mut artifacts: Vec<Summary> = Vec::new();
        let after: Option<Vec<&str>> = before.map(|b| b.split('|').collect());
        for artifact in self.artifacts.values(scope, true) {
            if !Self::matches(&artifact, scope)
                || !artifact.versions.iter().any(|v| v.before != Some(true))
            {
                continue;
            }
            let file = self.file_of(&artifact).to_owned();
            if !seen.insert(file) {
                continue;
            }
            if artifact.pinned.is_some() {
                if before.is_none() {
                    artifacts.push(make_summary(artifact));
                }
                continue;
            }
            if artifacts.len() >= limit && !artifacts.iter().any(|item| item.header.pinned.is_none())
            {
                return (artifacts, None);
            }
            let key = [artifact.updated.as_str(), artifact.id.as_str()];
            if let Some(after) = &after
                && later(&key, after) <= 0
            {
                continue;
            }
            if artifacts.len() >= limit {
                let cursor = {
                    let last = artifacts.last().unwrap();
                    format!("{}|{}", last.header.updated, last.header.id)
                };
                return (artifacts, Some(cursor));
            }
            artifacts.push(make_summary(artifact));
        }
        (artifacts, None)
    }

    fn session_outputs(&self, sessions: Option<&[String]>) -> Vec<Summary> {
        let scopes: Vec<Scope> = match sessions {
            Some(list) => list
                .iter()
                .map(|s| Scope {
                    session: Some(s.clone()),
                    ..Default::default()
                })
                .collect(),
            None => vec![Scope::default()],
        };
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for scope in scopes {
            for artifact in self.artifacts.values(&scope, false) {
                if !seen.insert(artifact.id.clone())
                    || artifact.where_.sessions.is_empty()
                    || !artifact.versions.iter().any(|v| v.before != Some(true))
                {
                    continue;
                }
                out.push(make_summary(artifact));
            }
        }
        out
    }

    fn timeline(
        &self,
        scope: &Scope,
        limit: usize,
        before: Option<&str>,
    ) -> (Vec<(ArtifactHeader, Version)>, Option<String>) {
        let after: Option<Vec<&str>> = before.map(|b| b.split('|').collect());
        let mut entries = Vec::new();
        for artifact in self.artifacts.values(scope, false) {
            if !Self::matches(&artifact, scope) {
                continue;
            }
            let header = artifact.header();
            for version in artifact.versions {
                if version.before == Some(true) {
                    continue;
                }
                let padded = format!("{:09}", version.n);
                if let Some(after) = &after {
                    let key = [version.at.as_str(), header.id.as_str(), padded.as_str()];
                    if later(&key, after) <= 0 {
                        continue;
                    }
                }
                if let Some(group) = &scope.group
                    && version.group.as_ref().map(|g| g.id.as_str()) != Some(group.as_str())
                {
                    continue;
                }
                if let Some(bot) = &scope.bot
                    && version.by.bot.as_deref() != Some(bot.as_str())
                    && !(version.by.kind != WhoKind::Agent && header.where_.bots.contains(bot))
                {
                    continue;
                }
                if let Some(session) = &scope.session
                    && version.turn.as_ref().map(|t| t.session.as_str()) != Some(session.as_str())
                {
                    continue;
                }
                if let Some(by) = &scope.by
                    && version.by.bot.as_deref() != Some(by.as_str())
                {
                    continue;
                }
                entries.push((header.clone(), version));
            }
        }
        entries.sort_by(|(a_h, a_v), (b_h, b_v)| {
            let a_n = format!("{:09}", a_v.n);
            let b_n = format!("{:09}", b_v.n);
            let cmp = later(
                &[a_v.at.as_str(), a_h.id.as_str(), a_n.as_str()],
                &[b_v.at.as_str(), b_h.id.as_str(), b_n.as_str()],
            );
            cmp.cmp(&0)
        });
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let next = if has_more {
            entries
                .last()
                .map(|(h, v)| format!("{}|{}|{:09}", v.at, h.id, v.n))
        } else {
            None
        };
        (entries, next)
    }

    fn prunable(&self) -> Vec<(String, u64, String, u64)> {
        let mut found = Vec::new();
        for artifact in self.artifacts.values(&Scope::default(), false) {
            if artifact.pinned.is_some() || kinds_kept_whole(artifact.kind) {
                continue;
            }
            let deleted = artifact
                .versions
                .last()
                .is_some_and(|v| v.status == VersionStatus::Deleted);
            let newest_n = if deleted {
                None
            } else {
                artifact.versions.iter().rev().find(|v| v.kept).map(|v| v.n)
            };
            for version in &artifact.versions {
                if version.kept && Some(version.n) != newest_n {
                    found.push((artifact.id.clone(), version.n, version.at.clone(), version.size));
                }
            }
        }
        found.sort_by(|a, b| a.2.cmp(&b.2));
        found
    }

    fn storage(&self) -> StorageInfo {
        let older: u64 = self.prunable().iter().map(|(_, _, _, size)| *size).sum();
        let (free, floor) = self.policy.space();
        StorageInfo {
            copies: self.copy_bytes,
            older,
            budget: self.budget(),
            free,
            floor,
            low: free < floor,
        }
    }

    fn set_budget(&mut self, bytes: u64, confirmed: bool) -> Result<Option<String>, StoreError> {
        let older = self.storage().older;
        if bytes < older && !confirmed {
            return Ok(Some(budget_confirmation(older).body));
        }
        let mut settings = read_settings(&self.root)?;
        settings.insert("budget".to_owned(), Value::from(bytes));
        let encoded = serde_json::to_string(&ARTIFACT_SETTINGS_FORMAT.encode(settings))
            .map_err(io::Error::other)?;
        let target = self.root.join("settings.json");
        let tmp = self.root.join("settings.json.new");
        fs::write(&tmp, encoded)?;
        fs::rename(&tmp, &target)?;
        self.chosen_budget = Some(bytes);
        self.settle();
        Ok(None)
    }

    fn clear_older(&mut self, before: Option<&str>) -> usize {
        let mut cleared = 0;
        for (id, n, at, _) in self.prunable() {
            if let Some(before) = before
                && at.as_str() >= before
            {
                continue;
            }
            if self.clear_version(&id, n, NoCopy::Cleared) {
                cleared += 1;
            }
        }
        cleared
    }

    fn clear_version(&mut self, id: &str, n: u64, reason: NoCopy) -> bool {
        let Some(mut artifact) = self.artifacts.get(id) else {
            return false;
        };
        let Some(version) = artifact.versions.iter_mut().find(|v| v.n == n) else {
            return false;
        };
        version.kept = false;
        version.reason = Some(reason);
        let hash = version.hash.clone();
        let size = version.size;
        let version_clone = version.clone();
        if let Some(hash) = hash {
            self.use_copy(&hash, size, -1);
        }
        let _ = self.write_entry(&artifact, Some(&version_clone));
        self.told(artifact);
        true
    }

    fn settle(&mut self) {
        if self.copy_bytes <= self.budget() {
            return;
        }
        let prunable = self.prunable();
        let mut older: u64 = prunable.iter().map(|(_, _, _, size)| *size).sum();
        let budget = self.budget();
        for (id, n, _, size) in prunable {
            if older <= budget {
                break;
            }
            self.clear_version(&id, n, NoCopy::Cleared);
            older = older.saturating_sub(size);
        }
    }

    fn add_version(
        &mut self,
        mut artifact: Artifact,
        change: PendingVersion,
        made: &Made,
        now: Timestamp,
        bytes: Option<&[u8]>,
    ) -> Option<Artifact> {
        let latest = artifact.versions.last().cloned();
        let replacing = latest.as_ref().and_then(|l| {
            if change.status != VersionStatus::Deleted && Self::same_turn(l, made, now) {
                Some(l.clone())
            } else {
                None
            }
        });
        let previous = if replacing.is_some() {
            artifact
                .versions
                .len()
                .checked_sub(2)
                .and_then(|idx| artifact.versions.get(idx))
                .cloned()
        } else {
            latest.clone()
        };

        let mut via = if let Some(rep) = &replacing {
            let mut v = rep.via.clone();
            if !v.contains(&made.via) {
                v.push(made.via.clone());
            }
            v
        } else {
            vec![made.via.clone()]
        };
        if via.is_empty() {
            via.push(made.via.clone());
        }

        let status = if replacing
            .as_ref()
            .is_some_and(|r| r.status == VersionStatus::Added)
            && change.status != VersionStatus::Deleted
        {
            VersionStatus::Added
        } else {
            change.status
        };

        let patch = match (
            replacing.as_ref().and_then(|r| r.patch.as_deref()),
            change.patch.as_deref(),
        ) {
            (Some(old), Some(new)) => {
                let combined = format!("{old}\n{}", hunks_of(new));
                Some(truncate_chars(&combined, PATCH_LIMIT))
            }
            (_, other) => other.map(str::to_owned),
        };

        let lines = if let Some(patch) = &patch {
            let counts = from_patch(patch);
            Some(LineDelta {
                added: counts.additions,
                removed: counts.deletions,
            })
        } else if let Some(bytes) = bytes {
            if let Some(prev) = &previous {
                self.lines_delta(&artifact.mime, prev, bytes)
            } else if is_text(&artifact.mime) && bytes.len() <= LINE_LIMIT {
                let after_text = String::from_utf8_lossy(bytes);
                let diff = unified("", &after_text, 3);
                Some(LineDelta {
                    added: diff.additions,
                    removed: diff.deletions,
                })
            } else {
                None
            }
        } else {
            None
        };

        let handoff = if let (Some(mg), Some(prev)) = (&made.group, &previous)
            && prev.group.as_ref().map(|g| g.id.as_str()) == Some(mg.id.as_str())
            && let (Some(prev_bot), Some(cur_bot)) = (&prev.by.bot, &made.by.bot)
            && prev_bot != cur_bot
        {
            Some(Handoff {
                from: prev_bot.clone(),
            })
        } else {
            None
        };

        let version = Version {
            n: replacing
                .as_ref()
                .map_or_else(|| latest.as_ref().map_or(1, |l| l.n + 1), |r| r.n),
            at: stamp(now),
            by: made.by.clone(),
            turn: made.turn.clone(),
            group: made.group.clone(),
            trigger: made.trigger,
            via,
            status,
            edits: replacing.as_ref().map_or(0, |r| r.edits) + 1,
            lines,
            size: change.size,
            hash: change.hash,
            kept: change.kept,
            reason: change.reason,
            patch,
            restored: None,
            before: None,
            handoff,
        };

        if let Some(rep) = &replacing {
            if rep.kept
                && let Some(hash) = &rep.hash
            {
                self.use_copy(hash, rep.size, -1);
            }
            if let Some(last_slot) = artifact.versions.last_mut() {
                *last_slot = version.clone();
            }
        } else {
            artifact.versions.push(version.clone());
        }
        if version.kept
            && let Some(hash) = &version.hash
        {
            self.use_copy(hash, version.size, 1);
        }

        if let Some(prev) = &previous
            && prev.kept
            && replacing.is_none()
            && artifact.pinned.is_none()
            && !kinds_kept_whole(artifact.kind)
            && self.policy.low_on(0)
            && let Some(prev_mut) = artifact.versions.iter_mut().find(|v| v.n == prev.n)
        {
            prev_mut.kept = false;
            prev_mut.reason = Some(NoCopy::LowDisk);
            let prev_hash = prev_mut.hash.clone();
            let prev_size = prev_mut.size;
            let prev_clone = prev_mut.clone();
            if let Some(hash) = prev_hash {
                self.use_copy(&hash, prev_size, -1);
            }
            let _ = self.write_entry(&artifact, Some(&prev_clone));
        }

        artifact.updated = version.at.clone();
        if artifact.kind == Kind::Document
            && artifact.living != Some(true)
            && artifact
                .versions
                .iter()
                .filter(|v| v.trigger == Some(Trigger::Wake))
                .count()
                >= 2
        {
            artifact.living = Some(true);
        }
        if self.write_entry(&artifact, Some(&version)).is_err() {
            return None;
        }
        if self.copy_bytes <= self.budget() {
            return Some(self.told(artifact));
        }
        self.settle();
        let fresh = self.artifacts.get(&artifact.id).unwrap_or(artifact);
        Some(self.told(fresh))
    }

    fn lines_delta(&self, mime: &str, previous: &Version, bytes: &[u8]) -> Option<LineDelta> {
        if !is_text(mime)
            || !previous.kept
            || !previous
                .hash
                .as_deref()
                .is_some_and(|h| h.starts_with("sha256:"))
            || bytes.len() > LINE_LIMIT
            || previous.size > LINE_LIMIT as u64
        {
            return None;
        }
        let prev_bytes = fs::read(self.copy_path(previous.hash.as_deref()?)).ok()?;
        let before = String::from_utf8_lossy(&prev_bytes);
        let after = String::from_utf8_lossy(bytes);
        let diff = unified(&before, &after, 3);
        Some(LineDelta {
            added: diff.additions,
            removed: diff.deletions,
        })
    }

    fn compact(&mut self, months: &[String]) {
        let file = self.root.join("log").join("0000-00.jsonl");
        let tmp = self.root.join("log").join("0000-00.jsonl.new");
        let res = (|| -> io::Result<usize> {
            let mut out = File::create(&tmp)?;
            let mut lines = 0usize;
            for artifact in self.artifacts.values(&Scope::default(), false) {
                let header = artifact.header();
                if artifact.versions.is_empty() {
                    write_compact_line(&mut out, &header, None)?;
                    lines += 1;
                } else {
                    for version in &artifact.versions {
                        write_compact_line(&mut out, &header, Some(version))?;
                        lines += 1;
                    }
                }
            }
            out.flush()?;
            drop(out);
            fs::rename(&tmp, &file)?;
            for month in months {
                if month != "0000-00.jsonl" {
                    let _ = fs::remove_file(self.root.join("log").join(month));
                }
            }
            self.artifacts.compacted(lines)?;
            Ok(lines)
        })();
        if res.is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    fn load(&mut self, shown: &[ShownCitation]) -> Result<(), StoreError> {
        let months = self.artifacts.months();
        let root = self.root.clone();
        let lines = self.artifacts.load(|index| -> Result<usize, StoreError> {
            let mut total_lines = 0usize;
            for month in &months {
                let file = File::open(root.join("log").join(month))?;
                let reader = BufReader::new(file);
                for each in reader.lines() {
                    let each = each?;
                    if each.is_empty() {
                        continue;
                    }
                    total_lines += 1;
                    let Ok(saved) = serde_json::from_str::<Value>(&each) else {
                        continue;
                    };
                    if !saved.is_object() {
                        continue;
                    }
                    let decoded = ARTIFACT_INDEX_FORMAT.decode(saved)?;
                    let Ok(line) = serde_json::from_value::<JournalLine>(Value::Object(decoded.fields)) else {
                        continue;
                    };
                    if line.a.id.is_empty() {
                        continue;
                    }
                    let mut artifact = match index.get(&line.a.id) {
                        Some(mut existing) => {
                            existing.apply_header(line.a);
                            existing
                        }
                        None => Artifact::from_header(line.a, Vec::new()),
                    };
                    if let Some(version) = line.v {
                        if let Some(pos) = artifact.versions.iter().position(|v| v.n >= version.n) {
                            if artifact.versions[pos].n == version.n {
                                artifact.versions[pos] = version;
                            } else {
                                artifact.versions.insert(pos, version);
                            }
                        } else {
                            artifact.versions.push(version);
                        }
                    }
                    index.put(&artifact)?;
                }
            }
            Ok(total_lines)
        })?;

        let mut marked = HashSet::new();
        for citation in shown {
            let id = ArtifactStore::id_of(&citation.path);
            let Some(mut artifact) = self.artifacts.get(&id) else {
                continue;
            };
            let when = Timestamp::from_millisecond(citation.at_ms)
                .map(stamp)
                .unwrap_or_else(|_| stamp(self.now()));
            if artifact
                .shown
                .as_deref()
                .is_some_and(|existing| existing >= when.as_str())
            {
                continue;
            }
            artifact.shown = Some(when);
            marked.insert(artifact.id.clone());
            self.artifacts.put(&artifact)?;
        }

        let mut dropped = HashSet::new();
        let mut any = false;
        let mut after = String::new();
        let mut versions_count = 0usize;
        let mut copy_uses: Vec<(String, u64)> = Vec::new();
        loop {
            let batch = self.artifacts.page(&after);
            if batch.is_empty() {
                break;
            }
            after = batch.last().unwrap().0.clone();
            let root_ref = &self.root;
            self.artifacts.update::<(), io::Error>(|index| {
                for (id, body) in &batch {
                    let Ok(mut artifact) = serde_json::from_str::<Artifact>(body) else {
                        continue;
                    };
                    if let Some(project) = artifact.where_.project.as_deref()
                        && artifact.where_.bots.is_empty()
                        && artifact.pinned.is_none()
                        && artifact.shown.is_none()
                        && in_scratch(&artifact.path, project)
                        && artifact.versions.iter().all(|v| {
                            !v.via.is_empty() && v.via.iter().all(|via| via == "shell")
                        })
                    {
                        index.drop_id(id)?;
                        any = true;
                        for v in &artifact.versions {
                            if let Some(hash) = &v.hash
                                && hash.starts_with("sha256:")
                            {
                                dropped.insert(hash.clone());
                            }
                        }
                        continue;
                    }
                    versions_count += artifact.versions.len() + 1;
                    let mime = type_of(
                        &artifact.path,
                        artifact.kind == Kind::File || artifact.mime == "application/octet-stream",
                    );
                    let mut modified = artifact.mime != mime;
                    artifact.mime = mime.to_owned();
                    for version in &mut artifact.versions {
                        if !version.kept {
                            continue;
                        }
                        let Some(hash) = &version.hash else {
                            continue;
                        };
                        let hex = hash.strip_prefix("sha256:").unwrap_or(hash);
                        let copy_file = root_ref
                            .join("copies")
                            .join(&hex[..hex.len().min(2)])
                            .join(hex);
                        if copy_file.exists() {
                            copy_uses.push((hash.clone(), version.size));
                        } else {
                            version.kept = false;
                            modified = true;
                        }
                    }
                    if modified {
                        index.put(&artifact)?;
                    }
                }
                Ok(())
            })?;
        }

        for (hash, size) in copy_uses {
            self.use_copy(&hash, size, 1);
        }

        if any || (lines > 1000 && lines > versions_count * 2) {
            self.compact(&months);
        } else {
            for id in marked {
                if let Some(artifact) = self.artifacts.get(&id) {
                    let _ = self.write_entry(&artifact, None);
                }
            }
        }
        for hash in dropped {
            if !self.copies.contains_key(&hash) {
                let _ = fs::remove_file(self.copy_path(&hash));
            }
        }
        Ok(())
    }
}

struct PendingVersion {
    status: VersionStatus,
    hash: Option<String>,
    size: u64,
    kept: bool,
    reason: Option<NoCopy>,
    patch: Option<String>,
}

fn write_compact_line(
    out: &mut File,
    header: &ArtifactHeader,
    version: Option<&Version>,
) -> io::Result<()> {
    let line = JournalLine {
        a: header.clone(),
        v: version.cloned(),
    };
    let Value::Object(fields) = serde_json::to_value(line).map_err(io::Error::other)? else {
        unreachable!();
    };
    let encoded = serde_json::to_string(&ARTIFACT_INDEX_FORMAT.encode(fields))
        .map_err(io::Error::other)?;
    out.write_all(encoded.as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

fn write_restore_file(target: &Path, tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(target).ok().map(|m| m.permissions().mode() & 0o7777)
    };
    let _ = fs::remove_file(tmp);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(tmp)?;
    file.write_all(bytes)?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    drop(file);
    fs::rename(tmp, target)?;
    Ok(())
}

fn hunks_of(patch: &str) -> &str {
    match patch.find("@@") {
        Some(idx) => &patch[idx..],
        None => patch,
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn later(a: &[&str], b: &[&str]) -> i32 {
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or("");
        let y = b.get(i).copied().unwrap_or("");
        if x != y {
            return if x < y { 1 } else { -1 };
        }
    }
    0
}

fn make_summary(artifact: Artifact) -> Summary {
    let header = artifact.header();
    let versions = artifact
        .versions
        .iter()
        .filter(|v| v.before != Some(true))
        .count();
    let latest = artifact.versions.into_iter().last().unwrap();
    Summary {
        header,
        versions,
        latest,
    }
}
