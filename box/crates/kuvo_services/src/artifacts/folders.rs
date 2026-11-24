//! Watching bots' folders for changes made outside tool calls (commands' outputs and your
//! own edits to memory/job/skills), matching `box/src/link/artifacts/folders.ts`.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kuvo_os::watch::{Notice, Options as WatchOptions, WatchHub};
use tokio::task::JoinHandle;

use super::{
    kinds::strip_folder_prefix,
    recorder::{Recorder, TurnEnded},
    store::{ArtifactStore, COPY_LIMIT, Made, MadeWhere, Who, WhoKind},
    turnfiles::{changed_since, is_skipped},
};

/// Whether a path relative to `bots/` is one the artifact record keeps.
pub fn recordable(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split(std::path::MAIN_SEPARATOR).collect();
    if parts.len() < 2 || !parts[0].starts_with("bot_") {
        return false;
    }
    let inside = &parts[1..];
    if inside.len() == 1 {
        return matches!(inside[0], "MEMORY.md" | "job.json");
    }
    if inside[0] == "groups" {
        return inside.len() == 2 && inside[1].ends_with(".md");
    }
    if inside[0] != "workspace" {
        return false;
    }
    let rest = &inside[1..];
    let name = rest.last().copied().unwrap_or("");
    if name.is_empty()
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".tmp")
        || name.ends_with(".crdownload")
        || name.ends_with(".part")
    {
        return false;
    }
    if rest[0] == ".opencode" {
        return rest.get(1) == Some(&"skills")
            && rest.len() > 3
            && !rest.iter().any(|p| is_skipped(p));
    }
    !rest
        .iter()
        .any(|p| is_skipped(p) || p.starts_with('.'))
}

fn wanted(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split(std::path::MAIN_SEPARATOR).collect();
    if parts.iter().any(|p| is_skipped(p)) {
        return false;
    }
    parts.iter().enumerate().all(|(i, part)| {
        !part.starts_with('.')
            || (*part == ".opencode"
                && matches!(parts.get(i + 1), None | Some(&"skills")))
    })
}

fn bot_folder(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split(std::path::MAIN_SEPARATOR).collect();
    let Some(&bot) = parts.first() else {
        return false;
    };
    if !bot.starts_with("bot_") {
        return false;
    }
    match parts.get(1).copied() {
        None => true,
        Some("groups") => parts.len() == 2,
        Some("workspace") => wanted(rel),
        _ => false,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[derive(Clone)]
pub struct BotFolders {
    store: ArtifactStore,
    recorder: Recorder,
    bots: PathBuf,
    settle: Duration,
    watching: Arc<AtomicBool>,
    looked_at: Arc<AtomicI64>,
    generation: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<PathBuf, JoinHandle<()>>>>,
    watch_task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl BotFolders {
    pub fn new(store: ArtifactStore, recorder: Recorder, bots: PathBuf, settle: Duration) -> Self {
        Self {
            store,
            recorder,
            bots,
            settle,
            watching: Arc::new(AtomicBool::new(false)),
            looked_at: Arc::new(AtomicI64::new(0)),
            generation: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            watch_task: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn start(&self, watch: Option<&WatchHub>) {
        self.looked_at.store(now_ms(), Ordering::SeqCst);
        for bot in self.list_bots() {
            self.baseline(&bot).await;
        }
        let Some(hub) = watch else {
            return;
        };
        let Ok(mut sub) = hub
            .subscribe(
                self.bots.clone(),
                WatchOptions {
                    recursive: true,
                    create: false,
                },
            )
            .await
        else {
            self.watching.store(false, Ordering::SeqCst);
            return;
        };
        self.watching.store(true, Ordering::SeqCst);
        let this = self.clone();
        let task = tokio::spawn(async move {
            while let Some(notice) = sub.recv().await {
                match notice {
                    Notice::Changed { paths } => {
                        for rel in paths {
                            this.changed(this.bots.join(rel));
                        }
                    }
                    Notice::Rescan => {
                        this.missed().await;
                    }
                    Notice::Failed(_) => {
                        this.watching.store(false, Ordering::SeqCst);
                    }
                }
            }
        });
        *self.watch_task.lock().unwrap() = Some(task);
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.watching.store(false, Ordering::SeqCst);
        if let Some(task) = self.watch_task.lock().unwrap().take() {
            task.abort();
        }
        for (_, handle) in self.pending.lock().unwrap().drain() {
            handle.abort();
        }
    }

    pub async fn missed(&self) {
        let since = self.looked_at.swap(now_ms(), Ordering::SeqCst);
        let generation = self.generation.load(Ordering::SeqCst);
        let paths = changed_since(&self.bots, since, recordable, bot_folder).await;
        if generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        for path in paths {
            let pending = self.pending.lock().unwrap().contains_key(&path);
            if !pending && !self.store.as_seen(&path.to_string_lossy()) {
                self.changed(path);
            }
        }
    }

    pub fn changed(&self, path: PathBuf) {
        let bots_str = self.bots.to_string_lossy();
        let path_str = path.to_string_lossy().into_owned();
        let Some(inside) = strip_folder_prefix(&path_str, &bots_str) else {
            return;
        };
        let bot = inside
            .split(std::path::MAIN_SEPARATOR)
            .next()
            .unwrap_or("")
            .to_owned();
        if inside == bot {
            let this = self.clone();
            tokio::spawn(async move {
                this.baseline(&bot).await;
            });
            return;
        }
        if !recordable(inside) {
            return;
        }
        let this = self.clone();
        let settle = self.settle;
        let path_for_task = path.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(settle).await;
            this.record_path(&path_for_task).await;
        });
        let mut map = self.pending.lock().unwrap();
        if let Some(prev) = map.insert(path, handle) {
            prev.abort();
        }
    }

    pub async fn turn_ended(
        &self,
        bot: &str,
        since_ms: i64,
        made: &(dyn Fn(&str) -> Made + Send + Sync),
    ) {
        let mut settling: HashSet<PathBuf> = self.take_settling(Some(bot)).into_iter().collect();
        let folder = self.bots.join(bot);
        let bot_owned = bot.to_owned();
        let bot_owned2 = bot.to_owned();
        let found = changed_since(
            &folder,
            since_ms,
            move |inside| recordable(&format!("{bot_owned}{}{inside}", std::path::MAIN_SEPARATOR)),
            move |inside| bot_folder(&format!("{bot_owned2}{}{inside}", std::path::MAIN_SEPARATOR)),
        )
        .await;
        for path in self.take_settling(Some(bot)) {
            settling.insert(path);
        }
        let watching = self.watching.load(Ordering::SeqCst);
        for path in found {
            if !watching || !self.store.as_seen(&path.to_string_lossy()) {
                settling.insert(path);
            }
        }
        if !settling.is_empty() {
            let paths: Vec<String> = settling
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            self.store.change_all(&paths, made("shell")).await;
        }
    }

    pub async fn flush(&self, bot: Option<&str>) {
        let paths = self.take_settling(bot);
        for path in paths {
            self.record_path(&path).await;
        }
    }

    fn take_settling(&self, bot: Option<&str>) -> Vec<PathBuf> {
        let bots_str = self.bots.to_string_lossy().into_owned();
        let mut map = self.pending.lock().unwrap();
        let matching: Vec<PathBuf> = map
            .keys()
            .filter(|path| match bot {
                None => true,
                Some(bot) => strip_folder_prefix(&path.to_string_lossy(), &bots_str)
                    .and_then(|rel| rel.split(std::path::MAIN_SEPARATOR).next())
                    == Some(bot),
            })
            .cloned()
            .collect();
        for path in &matching {
            if let Some(handle) = map.remove(path) {
                handle.abort();
            }
        }
        matching
    }

    async fn record_path(&self, path: &Path) {
        if let Some(handle) = self.pending.lock().unwrap().remove(path) {
            handle.abort();
        }
        let path_str = path.to_string_lossy().into_owned();
        let bots_str = self.bots.to_string_lossy();
        let bot = strip_folder_prefix(&path_str, &bots_str)
            .and_then(|rel| rel.split(std::path::MAIN_SEPARATOR).next())
            .unwrap_or("")
            .to_owned();
        let made = if let Some(running) = self.recorder.made_in_folder(&bot) {
            running("shell")
        } else {
            Made {
                by: Who {
                    kind: WhoKind::Outside,
                    session: None,
                    bot: None,
                    agent: None,
                },
                turn: None,
                group: None,
                trigger: None,
                via: "outside".to_owned(),
                where_: MadeWhere {
                    bot: Some(bot),
                    ..Default::default()
                },
                download: false,
                patch: None,
                separate: false,
                keep: false,
                at: None,
            }
        };
        self.store.change(&path_str, made).await;
    }

    fn list_bots(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.bots) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.starts_with("bot_"))
            .collect()
    }

    async fn baseline(&self, bot: &str) {
        let folder = self.bots.join(bot);
        for name in ["MEMORY.md", "job.json"] {
            let file = folder.join(name);
            let file_str = file.to_string_lossy().into_owned();
            if self.store.at(&file_str).is_some() {
                continue;
            }
            if let Ok(meta) = fs::metadata(&file)
                && meta.is_file()
                && meta.len() <= COPY_LIMIT
                && let Ok(bytes) = fs::read(&file)
            {
                self.store
                    .before(&file_str, bytes, Some(bot.to_owned()), None)
                    .await;
            }
        }
        let skills_dir = folder.join("workspace").join(".opencode").join("skills");
        self.baseline_skills(&skills_dir, bot).await;
    }

    async fn baseline_skills(&self, root: &Path, bot: &str) {
        let mut left = 200usize;
        let mut stack = vec![root.to_path_buf()];
        while let Some(at) = stack.pop() {
            if left == 0 {
                break;
            }
            let Ok(mut entries) = tokio::fs::read_dir(&at).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                if left == 0 {
                    break;
                }
                left -= 1;
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_skipped(&name) {
                    continue;
                }
                let path = entry.path();
                let Ok(ft) = entry.file_type().await else {
                    continue;
                };
                if ft.is_dir() {
                    stack.push(path);
                } else if ft.is_file() {
                    let path_str = path.to_string_lossy().into_owned();
                    if self.store.at(&path_str).is_some() {
                        continue;
                    }
                    if let Ok(meta) = tokio::fs::metadata(&path).await
                        && meta.len() <= COPY_LIMIT
                        && let Ok(bytes) = tokio::fs::read(&path).await
                    {
                        self.store
                            .before(&path_str, bytes, Some(bot.to_owned()), None)
                            .await;
                    }
                }
            }
        }
    }
}

/// Wire a `Recorder` + optional `BotFolders` together with turn-end project & bot scans,
/// matching `box/src/link/artifacts/record.ts`.
pub fn wire_record(
    store: ArtifactStore,
    bots: Option<PathBuf>,
    settle: Duration,
    options: super::recorder::RecorderOptions,
) -> (Recorder, Option<BotFolders>) {
    let recorder = Recorder::new(
        store.clone(),
        super::recorder::RecorderOptions {
            bots: bots.clone(),
            ..options
        },
    );
    let folders = bots
        .as_ref()
        .map(|b| BotFolders::new(store.clone(), recorder.clone(), b.clone(), settle));
    let folders_hook = folders.clone();
    let store_hook = store;
    let bots_hook = bots;
    recorder.set_turn_ended(Arc::new(move |turn: TurnEnded| {
        let folders = folders_hook.clone();
        let store = store_hook.clone();
        let bots = bots_hook.clone();
        Box::pin(async move {
            let bot_fut = async {
                if let (Some(bot), Some(folders)) = (turn.bot.as_deref(), folders.as_ref()) {
                    folders.turn_ended(bot, turn.since_ms, turn.made.as_ref()).await;
                }
            };
            let project_fut = async {
                if let Some(folder) = turn.folder.as_deref() {
                    let in_bots = bots.as_ref().is_some_and(|b| {
                        let b_str = b.to_string_lossy();
                        folder == b_str.as_ref() || strip_folder_prefix(folder, &b_str).is_some()
                    });
                    if !in_bots {
                        let paths = changed_since(
                            Path::new(folder),
                            turn.since_ms,
                            |_| true,
                            |_| true,
                        )
                        .await;
                        if !paths.is_empty() {
                            let strings: Vec<String> = paths
                                .into_iter()
                                .map(|p| p.to_string_lossy().into_owned())
                                .collect();
                            store.change_all(&strings, (turn.made)("shell")).await;
                        }
                    }
                }
            };
            tokio::join!(bot_fut, project_fut);
        })
    }));
    (recorder, folders)
}
