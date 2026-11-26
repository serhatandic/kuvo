//! Repository file history for Undo, with a separate object store and index.
//! An operation on a snapshot never changes the working repository's index,
//! commits or refs. A repository is discovered only when a message needs it.

use std::{
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    io::{self, ErrorKind},
    path::{Component, Path, PathBuf},
    process::{Output, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use serde_json::Value;
use sha1::{Digest, Sha1};
use tokio::{io::AsyncWriteExt, sync::Mutex as AsyncMutex};

const LIMIT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub(crate) struct Snapshots {
    root: PathBuf,
    stores: Arc<Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>>,
    taken: Arc<AtomicU64>,
    collecting: Arc<Mutex<BTreeSet<PathBuf>>>,
}

struct Repository {
    top: PathBuf,
    prefix: PathBuf,
    store: PathBuf,
    exclude: Option<PathBuf>,
}

impl Snapshots {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            stores: Default::default(),
            taken: Arc::default(),
            collecting: Arc::default(),
        }
    }

    async fn repository(&self, directory: &Path) -> io::Result<Option<Repository>> {
        let directory = match canonical(directory) {
            Ok(directory) => directory,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        // Normal repositories and linked worktrees both have a .git marker.
        // In a plain chat folder there is no Git process to start at all.
        if !directory
            .ancestors()
            .any(|folder| folder.join(".git").symlink_metadata().is_ok())
        {
            return Ok(None);
        }
        let top = git(&directory, ["rev-parse", "--show-toplevel"], None, None).await;
        let Ok(top) = top else { return Ok(None) };
        if !top.status.success() {
            return Ok(None);
        }
        let top = canonical(Path::new(&text(&top.stdout)))?;
        let prefix = directory
            .strip_prefix(&top)
            .map_err(io::Error::other)?
            .to_owned();
        let exclude = git(
            &directory,
            [
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "info/exclude",
            ],
            None,
            None,
        )
        .await
        .ok()
        .filter(|out| out.status.success())
        .map(|out| PathBuf::from(text(&out.stdout)));
        let hash = hex::encode(Sha1::digest(top.to_string_lossy().as_bytes()));
        Ok(Some(Repository {
            top,
            prefix,
            store: self.root.join(&hash[..20]),
            exclude,
        }))
    }

    async fn lock(&self, repo: &Repository) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut stores = self.stores.lock().unwrap_or_else(|e| e.into_inner());
            stores.retain(|_, lock| lock.strong_count() != 0);
            if let Some(lock) = stores.get(&repo.store).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                stores.insert(repo.store.clone(), Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }

    pub async fn take(&self, directory: &Path) -> io::Result<Option<String>> {
        let Some(repo) = self.repository(directory).await? else {
            return Ok(None);
        };
        let _guard = self.lock(&repo).await;
        if !repo.store.join("HEAD").is_file() {
            tokio::fs::create_dir_all(&repo.store).await?;
            checked(
                git(
                    &repo.top,
                    [
                        OsStr::new("init"),
                        OsStr::new("--bare"),
                        OsStr::new("-q"),
                        repo.store.as_os_str(),
                    ],
                    None,
                    None,
                )
                .await?,
                "make a place to keep this project's files",
            )?;
        }
        checked(
            repo.run(
                [
                    OsStr::new("add"),
                    OsStr::new("-A"),
                    OsStr::new("--"),
                    repo.scope(),
                ],
                None,
            )
            .await?,
            "keep this project's files",
        )?;
        let tree = checked(
            repo.run(["write-tree"], None).await?,
            "keep this project's files",
        )?;
        let tree = text(&tree.stdout);
        let ignored = repo
            .run(
                [
                    OsStr::new("ls-files"),
                    OsStr::new("-z"),
                    OsStr::new("-o"),
                    OsStr::new("-i"),
                    OsStr::new("--exclude-standard"),
                    OsStr::new("--directory"),
                    OsStr::new("--"),
                    repo.scope(),
                ],
                None,
            )
            .await?;
        if ignored.status.success() {
            let folder = repo.store.join("kuvo-ignored");
            tokio::fs::create_dir_all(&folder).await?;
            let file = folder.join(&tree);
            let before = tokio::fs::read(&file).await.unwrap_or_default();
            let paths: BTreeSet<_> = fields(&before).chain(fields(&ignored.stdout)).collect();
            let mut kept = paths.into_iter().collect::<Vec<_>>().join(&0);
            kept.push(0);
            kuvo_store::files::write_atomic(&file, &kept)?;
        }
        // Pack old objects now and then without pruning them or holding the
        // snapshot queue. The tree IDs are kept in conversation records, not refs.
        if (self.taken.fetch_add(1, Ordering::Relaxed) + 1) % 50 == 0
            && self
                .collecting
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(repo.store.clone())
        {
            let collecting = self.collecting.clone();
            tokio::spawn(async move {
                let _ = repo
                    .run(
                        [
                            "-c",
                            "gc.auto=6700",
                            "-c",
                            "gc.autoDetach=false",
                            "gc",
                            "--auto",
                            "--quiet",
                        ],
                        None,
                    )
                    .await;
                collecting
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&repo.store);
            });
        }
        Ok(Some(tree))
    }

    pub async fn changed(&self, directory: &Path, from: &str, to: &str) -> io::Result<Vec<String>> {
        tree_id(from)?;
        tree_id(to)?;
        if from == to {
            return Ok(vec![]);
        }
        let Some(repo) = self.repository(directory).await? else {
            return Ok(vec![]);
        };
        let _guard = self.lock(&repo).await;
        let args: Vec<OsString> = [
            "diff",
            "--raw",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-abbrev",
            from,
            to,
            "--",
        ]
        .into_iter()
        .map(Into::into)
        .chain([repo.scope().into()])
        .collect();
        let out = checked(repo.run(args, None).await?, "tell which files changed")?;
        let mut fields = fields(&out.stdout);
        let mut paths = Vec::new();
        while let Some(header) = fields.next() {
            let Some(path) = fields.next() else { break };
            let header = String::from_utf8_lossy(header);
            if header
                .split_whitespace()
                .take(2)
                .all(|mode| mode.trim_start_matches(':') != "160000")
            {
                paths.push(String::from_utf8_lossy(path).into_owned());
            }
        }
        Ok(paths)
    }

    pub async fn changes(
        &self,
        directory: &Path,
        from: &str,
        to: &str,
        paths: &[String],
    ) -> io::Result<Vec<Value>> {
        tree_id(from)?;
        tree_id(to)?;
        let Some(repo) = self.repository(directory).await? else {
            return Ok(vec![]);
        };
        let _guard = self.lock(&repo).await;
        let mut changes = Vec::new();
        for path in paths
            .iter()
            .take(200)
            .filter(|path| repo.path(path).is_some())
        {
            let out = repo
                .run(
                    [
                        "-c",
                        "diff.suppressBlankEmpty=false",
                        "diff",
                        "--no-color",
                        "--no-renames",
                        "--no-ext-diff",
                        "--no-textconv",
                        from,
                        to,
                        "--",
                        path,
                    ],
                    None,
                )
                .await?;
            if !out.status.success() {
                continue;
            }
            let patch = String::from_utf8_lossy(&out.stdout);
            let patch = if patch.len() > 50_000 {
                &patch[..patch.floor_char_boundary(50_000)]
            } else {
                &patch
            };
            let status = if patch.lines().any(|line| line.starts_with("new file mode")) {
                "added"
            } else if patch
                .lines()
                .any(|line| line.starts_with("deleted file mode"))
            {
                "deleted"
            } else {
                "modified"
            };
            changes.push(crate::file_change::patch_change(
                &repo.top.join(path).to_string_lossy(),
                patch,
                Some(status),
            ));
        }
        Ok(changes)
    }

    pub async fn restore(
        &self,
        directory: &Path,
        tree: &str,
        paths: &[String],
        since: Option<u64>,
    ) -> io::Result<()> {
        tree_id(tree)?;
        let Some(repo) = self.repository(directory).await? else {
            return Ok(());
        };
        let _guard = self.lock(&repo).await;
        let wanted: BTreeSet<_> = paths
            .iter()
            .filter(|path| repo.path(path).is_some())
            .map(String::as_str)
            .collect();
        let output = checked(
            repo.run(["ls-tree", "-r", "-z", "--full-tree", tree], None)
                .await?,
            "read what the files were",
        )?;
        let mut there = BTreeSet::new();
        let mut nested = BTreeSet::new();
        for field in fields(&output.stdout) {
            let entry = String::from_utf8_lossy(field);
            if let Some((kind, path)) = entry.split_once('\t')
                && let Some(path) = wanted.get(path)
            {
                if kind.starts_with("160000 ") {
                    nested.insert(*path);
                } else {
                    there.insert(*path);
                }
            }
        }
        let rules = |path: &str| Path::new(path).file_name() == Some(OsStr::new(".gitignore"));
        for path in wanted
            .iter()
            .filter(|path| rules(path) && !there.contains(**path) && !nested.contains(**path))
        {
            repo.remove(path)?;
        }
        repo.checkout(tree, there.iter().copied().filter(|path| rules(path)))
            .await?;
        let mut gone: Vec<_> = wanted
            .iter()
            .copied()
            .filter(|path| {
                !rules(path)
                    && !there.contains(path)
                    && !nested.contains(path)
                    && repo.path(path).is_some()
            })
            .collect();
        if !gone.is_empty() {
            let input: Vec<_> = gone.iter().map(|path| format!("./{path}")).collect();
            let input = nul(&input);
            let out = repo
                .run_patterns(["check-ignore", "--no-index", "-z", "--stdin"], Some(input))
                .await?;
            if !matches!(out.status.code(), Some(0 | 1)) {
                return Err(io::Error::other(
                    "Kuvo couldn't tell which files the ignore rules left out.",
                ));
            }
            let left: BTreeSet<_> = fields(&out.stdout)
                .map(|path| path.strip_prefix(b"./").unwrap_or(path))
                .collect();
            gone.retain(|path| !left.contains(path.as_bytes()));
        }
        // The same tree may have been taken with different ignored files. The kept
        // union protects either case, and legacy trees use the creation time.
        if tree.bytes().all(|b| b.is_ascii_hexdigit()) && matches!(tree.len(), 40 | 64) {
            match tokio::fs::read(repo.store.join("kuvo-ignored").join(tree)).await {
                Ok(bytes) => {
                    let left: BTreeSet<_> = fields(&bytes).collect();
                    gone.retain(|path| {
                        !left.contains(path.as_bytes())
                            && !Path::new(path).ancestors().skip(1).any(|folder| {
                                left.contains(format!("{}/", folder.to_string_lossy()).as_bytes())
                            })
                    });
                }
                Err(_) => {
                    if let Some(since) = since {
                        gone.retain(|path| {
                            !std::fs::symlink_metadata(repo.top.join(path))
                                .ok()
                                .and_then(|meta| meta.created().ok())
                                .and_then(|born| born.duration_since(UNIX_EPOCH).ok())
                                .is_some_and(|born| (born.as_millis() as u64) < since)
                        });
                    }
                }
            }
        }
        gone.sort_by_key(|path| std::cmp::Reverse(path.len()));
        for path in gone {
            repo.remove(path)?;
        }
        repo.checkout(tree, there.iter().copied().filter(|path| !rules(path)))
            .await
    }
}

impl Repository {
    fn scope(&self) -> &OsStr {
        if self.prefix.as_os_str().is_empty() {
            OsStr::new(".")
        } else {
            self.prefix.as_os_str()
        }
    }

    async fn run(
        &self,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        input: Option<Vec<u8>>,
    ) -> io::Result<Output> {
        git(&self.top, args, Some((self, false)), input).await
    }

    async fn run_patterns(
        &self,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        input: Option<Vec<u8>>,
    ) -> io::Result<Output> {
        git(&self.top, args, Some((self, true)), input).await
    }

    /// The path stays in the session's part of the repository. Never traverse
    /// links or a repository nested there (including a linked worktree).
    fn path(&self, path: &str) -> Option<PathBuf> {
        let path = Path::new(path);
        if !path.starts_with(&self.prefix) || !path.components().all(|part| matches!(part, Component::Normal(name) if !name.eq_ignore_ascii_case(".git"))) {
            return None;
        }
        let target = self.top.join(path);
        if target == self.top {
            return None;
        }
        for parent in target
            .ancestors()
            .skip(1)
            .take_while(|parent| *parent != self.top)
        {
            if std::fs::symlink_metadata(parent).is_ok_and(|meta| meta.file_type().is_symlink())
                || parent.join(".git").symlink_metadata().is_ok()
            {
                return None;
            }
        }
        Some(target)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        let Some(target) = self.path(path) else {
            return Ok(());
        };
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_dir() => {
                // A directory containing another repository or other unrelated
                // files is not part of the individual file change.
                if target.join(".git").symlink_metadata().is_ok() {
                    return Ok(());
                }
                std::fs::remove_dir(&target)?;
            }
            Ok(_) => std::fs::remove_file(&target)?,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        }
        for parent in target
            .ancestors()
            .skip(1)
            .take_while(|parent| *parent != self.top)
        {
            if std::fs::remove_dir(parent).is_err() {
                break;
            }
        }
        Ok(())
    }

    async fn checkout<'a>(
        &self,
        tree: &str,
        paths: impl Iterator<Item = &'a str>,
    ) -> io::Result<()> {
        let mut kept = Vec::new();
        for path in paths {
            let Some(target) = self.path(path) else {
                continue;
            };
            if target.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
                if target.join(".git").symlink_metadata().is_ok() {
                    continue;
                }
                std::fs::remove_dir(&target)?;
            }
            kept.push(path);
        }
        if !kept.is_empty() {
            checked(
                self.run(
                    [
                        "checkout",
                        tree,
                        "--pathspec-from-file=-",
                        "--pathspec-file-nul",
                    ],
                    Some(nul(&kept)),
                )
                .await?,
                "put the files back",
            )?;
        }
        Ok(())
    }
}

async fn git(
    directory: &Path,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    snapshot: Option<(&Repository, bool)>,
    input: Option<Vec<u8>>,
) -> io::Result<Output> {
    let mut command = tokio::process::Command::new("git");
    // Inherited Git paths (a hook or another git command's environment) don't
    // identify the snapshot repository.
    for (key, _) in std::env::vars_os().filter(|(key, _)| key.to_string_lossy().starts_with("GIT_"))
    {
        command.env_remove(key);
    }
    command
        .current_dir(directory)
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some((repo, patterns)) = snapshot {
        command
            .arg("--git-dir")
            .arg(&repo.store)
            .arg("--work-tree")
            .arg(&repo.top)
            .args([
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.quotepath=false",
                "-c",
                "core.symlinks=true",
                "-c",
                "core.longpaths=true",
                "-c",
                "gc.auto=0",
                "-c",
                "gc.pruneExpire=never",
                "-c",
                "gc.worktreePruneExpire=never",
            ])
            .env("GIT_LITERAL_PATHSPECS", if patterns { "0" } else { "1" });
        if let Some(exclude) = &repo.exclude {
            let mut setting = OsString::from("core.excludesFile=");
            setting.push(exclude);
            command.arg("-c").arg(setting);
        }
    }
    let mut child = command.args(args).spawn()?;
    let stdin = child.stdin.take();
    let write = async move {
        if let Some(input) = input
            && let Some(mut stdin) = stdin
        {
            stdin.write_all(&input).await?;
        }
        Ok::<_, io::Error>(())
    };
    tokio::time::timeout(LIMIT, async {
        let (_, output) = tokio::try_join!(write, child.wait_with_output())?;
        Ok(output)
    })
    .await
    .map_err(|_| io::Error::new(ErrorKind::TimedOut, "Git took too long."))?
}

fn checked(out: Output, what: &str) -> io::Result<Output> {
    if out.status.success() {
        Ok(out)
    } else {
        Err(io::Error::other(format!(
            "Kuvo couldn't {what}: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("git failed")
        )))
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}
fn fields(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|&byte| byte == 0)
        .filter(|field| !field.is_empty())
}
fn nul<T: AsRef<str>>(paths: &[T]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for path in paths {
        bytes.extend_from_slice(path.as_ref().as_bytes());
        bytes.push(0);
    }
    bytes
}

fn canonical(path: &Path) -> io::Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    Ok(kuvo_os::platform::windows_conventional_path(&path).unwrap_or(path))
}

fn tree_id(id: &str) -> io::Result<()> {
    if matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(io::Error::new(
            ErrorKind::InvalidData,
            "The saved repository snapshot is unreadable.",
        ))
    }
}
