//! The checkout records and their Settings routes. All mutations read the record again under its lock.
use super::{
    projects::WORKTREE_RECORDS,
    safe::{self, Intent},
};
use http::StatusCode;
use kuvo_engine::{Engine, WorktreeConversation};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Deserialize, Serialize)]
struct Tree {
    project: PathBuf,
    root: PathBuf,
    branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    made: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    used: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored: Option<bool>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl Tree {
    fn folder(&self) -> &Path {
        self.folder.as_deref().unwrap_or(&self.root)
    }
    fn contains(&self, row: &WorktreeConversation) -> bool {
        row.folder.starts_with(&self.root)
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Saved {
    #[serde(flatten)]
    tree: Tree,
    head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct Records {
    #[serde(default)]
    worktrees: Vec<Tree>,
    #[serde(default)]
    removed: Map<String, Value>,
    #[serde(default)]
    restorable: Vec<Saved>,
    #[serde(default, rename = "handedOver", skip_serializing_if = "Map::is_empty")]
    handed_over: Map<String, Value>,
}

fn refusal(status: StatusCode, message: impl Into<String>) -> Refusal {
    Refusal::new(status, message)
}
fn bad(text: &str) -> Refusal {
    refusal(StatusCode::BAD_REQUEST, text)
}
fn conflict(text: impl Into<String>) -> Refusal {
    refusal(StatusCode::CONFLICT, text)
}
fn missing() -> Refusal {
    refusal(
        StatusCode::NOT_FOUND,
        "That worktree isn't on this machine any more.",
    )
}
fn not_restorable() -> Refusal {
    refusal(StatusCode::NOT_FOUND, "It can’t come back any more.")
}

fn storage(error: impl std::fmt::Display) -> Refusal {
    refusal(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn rows<T: serde::de::DeserializeOwned>(fields: &Map<String, Value>, key: &str) -> Vec<T> {
    fields
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect()
}
fn read(home: &Path) -> Result<Records, Refusal> {
    let file = home.join("box/worktrees.json");
    let Some(value) = std::fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return Ok(Records::default());
    };
    if !value.is_object() {
        return Ok(Records::default());
    }
    let fields = WORKTREE_RECORDS
        .decode(value)
        .map_err(super::unsupported)?
        .fields;
    Ok(Records {
        worktrees: rows(&fields, "worktrees"),
        restorable: rows(&fields, "restorable"),
        removed: fields
            .get("removed")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        handed_over: fields
            .get("handedOver")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    })
}

async fn change(home: &Path, f: impl FnOnce(&mut Records) + Send) -> Result<(), Refusal> {
    let lock = home.join("box/worktrees.json.lock");
    let _lock = tokio::task::spawn_blocking(move || {
        kuvo_store::locks::StaleLock::acquire(
            &lock,
            Duration::from_secs(10),
            Duration::from_secs(3),
            kuvo_store::locks::OnBusy::Refuse,
        )
    })
    .await
    .map_err(storage)?
    .map_err(storage)?
    .ok_or_else(|| storage("Could not lock worktree records"))?;
    let mut records = read(home)?;
    f(&mut records);
    let value = serde_json::to_value(records).map_err(storage)?;
    let fields = value.as_object().cloned().unwrap_or_default();
    kuvo_store::files::write_secret(
        &home.join("box/worktrees.json"),
        &serde_json::to_vec_pretty(&WORKTREE_RECORDS.encode(fields)).map_err(storage)?,
    )
    .map_err(storage)
}

async fn git_with(
    folder: &Path,
    args: &[&str],
    intent: Intent,
    env: &[(&str, &str)],
) -> Option<String> {
    let out = safe::run(folder, args, intent, env, Duration::from_secs(60))
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

async fn git(folder: &Path, args: &[&str]) -> Option<String> {
    git_with(folder, args, Intent::Own, &[]).await
}
async fn asked(folder: &Path, args: &[&str]) -> Option<String> {
    git_with(folder, args, Intent::Asked, &[]).await
}
const BY_KUVO: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Kuvo Code"),
    ("GIT_AUTHOR_EMAIL", "kuvo-code@localhost"),
    ("GIT_COMMITTER_NAME", "Kuvo Code"),
    ("GIT_COMMITTER_EMAIL", "kuvo-code@localhost"),
];

/// An empty first commit for a new repository, leaving the staged index alone.
pub async fn first_commit(project: &Path) -> bool {
    let verify = ["rev-parse", "--verify", "--quiet", "HEAD"];
    if git(project, &verify).await.is_some() {
        return true;
    }
    if git(project, &["for-each-ref", "--count=1", "--format=x"])
        .await
        .as_deref()
        != Some("")
    {
        return false;
    }
    let Some(empty) = git(project, &["mktree"]).await else {
        return false;
    };
    let args = [
        "-c",
        "user.useConfigOnly=true",
        "commit-tree",
        &empty,
        "-m",
        "Initial commit",
    ];
    let commit = match git(project, &args).await {
        Some(commit) => Some(commit),
        None => git_with(project, &args, Intent::Own, BY_KUVO).await,
    };
    if let Some(commit) = commit {
        let _ = git(
            project,
            &[
                "update-ref",
                "-m",
                "commit (initial): Initial commit",
                "HEAD",
                &commit,
                "",
            ],
        )
        .await;
    }
    git(project, &verify).await.is_some()
}

pub struct Git;
impl kuvo_engine::WorktreeGit for Git {
    fn run<'a>(
        &'a self,
        folder: &'a Path,
        args: &'a [&'a str],
        requested: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            if requested {
                asked(folder, args).await
            } else {
                git(folder, args).await
            }
        })
    }

    fn first_commit<'a>(
        &'a self,
        project: &'a Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(first_commit(project))
    }
}

fn snapshot_ref(tree: &Tree, commit: &str) -> String {
    let name = tree.root.file_name().unwrap_or_default().to_string_lossy();
    let name = regex::Regex::new("[^A-Za-z0-9_-]+")
        .expect("constant expression")
        .replace_all(&name, "-");
    format!(
        "refs/kuvo/worktrees/{name}-{}",
        commit.chars().take(12).collect::<String>()
    )
}

async fn drop_snapshot(tree: &Tree, commit: &str) {
    let _ = git(
        &tree.project,
        &["update-ref", "-d", &snapshot_ref(tree, commit), commit],
    )
    .await;
}

// Keep the combined staged and unstaged contents in a separate index, without
// moving HEAD or changing what is staged in the checkout.
async fn snapshot(home: &Path, tree: &Tree) -> Result<Option<String>, ()> {
    let status = git(
        &tree.root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await
    .ok_or(())?;
    if status.is_empty() {
        return Ok(None);
    }
    let dir = tempfile::tempdir_in(home.join("run")).map_err(|_| ())?;
    let index = dir.path().join("index");
    let index = index.to_str().ok_or(())?;
    let env = [("GIT_INDEX_FILE", index)];
    git_with(&tree.root, &["read-tree", "HEAD"], Intent::Asked, &env)
        .await
        .ok_or(())?;
    git_with(&tree.root, &["add", "-A"], Intent::Asked, &env)
        .await
        .ok_or(())?;
    let contents = git_with(&tree.root, &["write-tree"], Intent::Own, &env)
        .await
        .ok_or(())?;
    let commit = git_with(
        &tree.root,
        &[
            "commit-tree",
            &contents,
            "-p",
            "HEAD",
            "-m",
            &format!(
                "What wasn't committed in {} when its worktree was removed",
                tree.branch
            ),
        ],
        Intent::Own,
        BY_KUVO,
    )
    .await
    .ok_or(())?;
    git(
        &tree.root,
        &["update-ref", &snapshot_ref(tree, &commit), &commit],
    )
    .await
    .ok_or(())?;
    Ok(Some(commit))
}

fn slug(title: &str) -> String {
    use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
    const CYRILLIC: &str = "абвгдеёжзийклмнопрстуфхцчшщъыьэюяіїєґ";
    const CYRILLIC_LATIN: &[&str] = &[
        "a", "b", "v", "g", "d", "e", "e", "z", "z", "i", "j", "k", "l", "m", "n", "o", "p", "r",
        "s", "t", "u", "f", "h", "c", "c", "s", "s", "", "y", "", "e", "u", "a", "i", "i", "e",
        "g",
    ];
    const GREEK: &str = "αβγδεζηθικλμνξοπρσςτυφχψω";
    const GREEK_LATIN: &[&str] = &[
        "a", "b", "g", "d", "e", "z", "e", "th", "i", "k", "l", "m", "n", "x", "o", "p", "r", "s",
        "s", "t", "y", "ph", "ch", "ps", "o",
    ];
    fn latin(text: impl Iterator<Item = char>) -> String {
        text.map(|ch| {
            if let Some(at) = CYRILLIC.chars().position(|x| x == ch) {
                CYRILLIC_LATIN[at].to_owned()
            } else if let Some(at) = GREEK.chars().position(|x| x == ch) {
                GREEK_LATIN[at].to_owned()
            } else if ch == 'ı' {
                "i".into()
            } else {
                ch.to_string()
            }
        })
        .collect()
    }
    let text = latin(
        latin(title.to_lowercase().chars())
            .nfkd()
            .filter(|ch| !is_combining_mark(*ch)),
    );
    let mut slug = String::new();
    for word in text.split(|ch: char| !ch.is_alphanumeric()) {
        let word: String = word.chars().filter(char::is_ascii).collect();
        if word.is_empty() {
            continue;
        }
        if slug.len() + word.len() + usize::from(!slug.is_empty()) > 40 {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    slug
}

async fn on_remote(folder: &Path, branch: &str) -> bool {
    git(
        folder,
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            &format!("refs/remotes/*/{branch}"),
        ],
    )
    .await
    .is_some_and(|s| !s.is_empty())
}

async fn free_branch(folder: &Path, name: &str) -> Option<String> {
    for suffix in 1..=20 {
        let branch = if suffix == 1 {
            name.to_owned()
        } else {
            format!("{name}-{suffix}")
        };
        if git(
            folder,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .await
        .is_none()
            && !on_remote(folder, &branch).await
        {
            return Some(branch);
        }
    }
    None
}

async fn branch_exists(tree: &Tree) -> bool {
    git(
        &tree.project,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", tree.branch),
        ],
    )
    .await
    .is_some()
}

async fn dirty(tree: &Tree) -> Option<bool> {
    git(
        &tree.root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ],
    )
    .await
    .map(|s| !s.is_empty())
}
async fn ahead(tree: &Tree) -> Option<u64> {
    let base = tree.base.as_ref()?;
    git(
        &tree.root,
        &["rev-list", "--count", &format!("{base}..HEAD")],
    )
    .await?
    .parse()
    .ok()
}
async fn listed_saved(saved: &Saved) -> Value {
    let tree = &saved.tree;
    let branch = branch_exists(tree).await;
    let back = branch
        || saved.snapshot.is_some()
        || git(
            &tree.project,
            &["cat-file", "-e", &format!("{}^{{commit}}", saved.head)],
        )
        .await
        .is_some();
    let mut row = json!({"project":tree.project,"root":tree.root,"folder":tree.folder(),"branch":tree.branch,"saved":saved.snapshot.is_some(),"restorable":back});
    if let Some(base) = &tree.base {
        row["base"] = json!(base);
        if branch
            && let Some(n) = git(
                &tree.project,
                &[
                    "rev-list",
                    "--count",
                    &format!("{base}..refs/heads/{}", tree.branch),
                ],
            )
            .await
            .and_then(|n| n.parse::<u64>().ok())
        {
            row["ahead"] = json!(n);
        }
    }
    row
}

#[derive(Clone)]
struct Owner {
    engine: Engine,
    home: PathBuf,
    turn: Arc<tokio::sync::Mutex<()>>,
}

impl Owner {
    async fn list(&self, raw: bool) -> Result<Value, Refusal> {
        let records = read(&self.home)?;
        let rows = if raw || records.worktrees.is_empty() {
            Vec::new()
        } else {
            self.engine.worktree_sessions().await.unwrap_or_default()
        };
        let mut worktrees = Vec::new();
        for tree in records.worktrees {
            if raw {
                worktrees.push(json!(tree));
                continue;
            }
            let here: Vec<_> = rows.iter().filter(|row| tree.contains(row)).collect();
            let mut view = json!({"path":tree.root,"folder":tree.folder(),"project":tree.project,"projectName":tree.project.file_name().unwrap_or_default().to_string_lossy(),
                "branch":tree.branch,"sessions":here.len(),"working":here.iter().any(|row| row.running)});
            if let Some(base) = &tree.base {
                view["base"] = json!(base);
            }
            if tree.root.exists() {
                if let Some(value) = dirty(&tree).await {
                    view["uncommitted"] = json!(value);
                }
                if let Some(value) = ahead(&tree).await {
                    view["ahead"] = json!(value);
                }
                if let Ok(Ok(sizes)) = tokio::time::timeout(
                    Duration::from_secs(60),
                    kuvo_os::disk_usage::measure(vec![tree.root.clone()], 0),
                )
                .await
                    && let Some(size) = sizes.get(&tree.root)
                {
                    view["size"] = json!(size);
                }
            }
            worktrees.push(view);
        }
        let mut restorable = Vec::new();
        for saved in &records.restorable {
            restorable.push(listed_saved(saved).await);
        }
        Ok(json!({"worktrees":worktrees,"restorable":restorable}))
    }

    async fn create(&self, body: &Value) -> Result<Value, Refusal> {
        let project = body["project"]
            .as_str()
            .map(Path::new)
            .filter(|p| p.is_absolute() && p.is_dir())
            .ok_or_else(|| bad("Send the project's folder on this machine as {\"project\"}."))?;
        let _turn = self.turn.lock().await;
        read(&self.home)?;
        self.engine.create_worktree(project).await
    }

    async fn record(&self, body: &Value) -> Result<Value, Refusal> {
        let _turn = self.turn.lock().await;
        let absolute = |key: &str| {
            body[key]
                .as_str()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        if body["removed"] == true {
            let (Some(folder), Some(project)) = (absolute("folder"), absolute("project")) else {
                return Err(bad("Send {removed: true, project, folder}."));
            };
            if folder == project {
                return Err(bad("Send {removed: true, project, folder}."));
            }
            change(&self.home, |records| {
                let key = folder.to_string_lossy().into_owned();
                if !records
                    .worktrees
                    .iter()
                    .any(|tree| tree.root == folder || tree.folder() == folder)
                    && !records.removed.contains_key(&key)
                    && !records.handed_over.contains_key(&key)
                {
                    records.handed_over.insert(key, json!(project));
                }
            })
            .await?;
        } else {
            let invalid = || bad("Send {project, root, folder?, branch, base?}.");
            let project = absolute("project").ok_or_else(invalid)?;
            let root = absolute("root").ok_or_else(invalid)?;
            let folder = if body["folder"].is_string() {
                absolute("folder").ok_or_else(invalid)?
            } else {
                root.clone()
            };
            let branch = body["branch"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(invalid)?
                .to_owned();
            if !root.exists() {
                return Err(missing());
            }
            let tree = Tree {
                project,
                root,
                folder: Some(folder),
                branch,
                base: body["base"].as_str().map(str::to_owned),
                made: None,
                prefix: None,
                used: None,
                restored: None,
                extra: Map::new(),
            };
            change(&self.home, |records| {
                if !records.worktrees.iter().any(|old| old.root == tree.root) {
                    records.worktrees.push(tree);
                }
            })
            .await?;
        }
        Ok(json!({}))
    }

    async fn remove(&self, body: &Value) -> Result<Value, Refusal> {
        let _turn = self.turn.lock().await;
        let tree = read(&self.home)?
            .worktrees
            .into_iter()
            .find(|tree| {
                body["path"]
                    .as_str()
                    .is_some_and(|p| Path::new(p) == tree.root)
            })
            .ok_or_else(missing)?;
        let rows = self.engine.worktree_sessions().await.map_err(|error| {
            let why = if error.kind() == std::io::ErrorKind::NotConnected {
                format!("The engine isn't running, so the worktree of {} stays. Try again once it's started.",tree.branch)
            } else { format!("It couldn't be told whether an agent is working in the worktree of {}, so it stays. Try again in a moment.", tree.branch) };
            refusal(StatusCode::SERVICE_UNAVAILABLE, why)
        })?;
        if let Some(was) = body.get("was") {
            let mut now = json!({"record":tree});
            if let Some(value) = dirty(&tree).await {
                now["uncommitted"] = json!(value);
            }
            if let Some(value) = ahead(&tree).await {
                now["ahead"] = json!(value);
            }
            if &now != was {
                return Err(conflict(crate::config::CHANGED_SINCE));
            }
        }
        let discard = body["discard"] != false;
        let forget = body["forget"] == true;
        self.remove_tree(tree, discard, forget, false, &rows)
            .await?;
        Ok(json!({}))
    }

    async fn remove_tree(
        &self,
        tree: Tree,
        discard: bool,
        forget: bool,
        keep: bool,
        rows: &[WorktreeConversation],
    ) -> Result<(), Refusal> {
        if rows.iter().any(|row| tree.contains(row) && row.running) {
            return Err(conflict(format!(
                "An agent is still working in the worktree of {}, so it stays. Stop it first, then remove the worktree.",
                tree.branch
            )));
        }
        if forget && rows.iter().any(|row| tree.contains(row)) {
            return Err(conflict(format!(
                "A session works in the worktree of {}, so it stays. Remove it with the session, or in Settings › Worktrees.",
                tree.branch
            )));
        }
        let gone = !tree.root.exists();
        if !discard && !keep && !gone && dirty(&tree).await != Some(false) {
            return Err(conflict(format!(
                "The worktree of {} has changes that aren’t committed, so it stays. Commit them, or remove it in Settings › Worktrees, which throws them away.",
                tree.branch
            )));
        }
        let head = if gone {
            git(
                &tree.project,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{}", tree.branch),
                ],
            )
            .await
        } else {
            git(&tree.root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await
        };
        // Do not remove a checkout with commits on no branch until those commits have a branch.
        let mut saved_tree = tree.clone();
        if !gone
            && git(&tree.root, &["symbolic-ref", "--quiet", "HEAD"])
                .await
                .is_none()
            && let Some(head) = &head
            && git(
                &tree.root,
                &[
                    "for-each-ref",
                    "--contains",
                    head,
                    "--format=%(refname)",
                    "refs/heads",
                ],
            )
            .await
            .is_some_and(|s| s.is_empty())
        {
            let kept = free_branch(&tree.root, &format!("{}-kept",tree.branch)).await.ok_or_else(|| conflict(format!("The worktree of {} has commits on no branch, and they couldn’t be kept on one, so it stays.",tree.branch)))?;
            if git(&tree.root, &["branch", &kept, head]).await.is_none() {
                return Err(conflict(format!(
                    "The worktree of {} has commits on no branch, and they couldn’t be kept on one, so it stays.",
                    tree.branch
                )));
            }
            saved_tree.branch = kept;
        }
        let saved = if keep && !gone {
            snapshot(&self.home, &tree).await.map_err(|_| {
                conflict(format!(
                    "What isn’t committed in the worktree of {} couldn’t be kept, so it stays.",
                    tree.branch
                ))
            })?
        } else {
            None
        };
        let removed: Result<(), Refusal> = async {
            if gone {
                let _ = git(&tree.project, &["worktree", "prune"]).await;
            } else if tree.restored == Some(true) {
                let root = tree.root.to_string_lossy();
                let mut args = vec!["worktree", "remove"];
                if discard || keep {
                    args.push("--force");
                }
                args.push(&root);
                asked(&tree.project, &args).await.ok_or_else(|| {
                    refusal(StatusCode::BAD_GATEWAY, "Its folder couldn't be removed.")
                })?;
            } else {
                self.engine
                    .remove_worktree(&tree.project, &tree.root, discard || keep)
                    .await
                    .map_err(|why| refusal(StatusCode::BAD_GATEWAY, why))?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = removed {
            if let Some(commit) = &saved {
                drop_snapshot(&tree, commit).await;
            }
            return Err(error);
        }
        let _ = git(
            &tree.project,
            &[
                "branch",
                if forget { "-D" } else { "-d" },
                "--",
                &tree.branch,
            ],
        )
        .await;
        change(&self.home, |records| {
            records.worktrees.retain(|old| old.root != tree.root);
            let folder = tree.folder().to_string_lossy().into_owned();
            records.removed.shift_remove(&folder);
            records.removed.insert(folder, json!(tree.project));
            while records.removed.len() > 200 {
                if let Some(first) = records.removed.keys().next().cloned() {
                    records.removed.shift_remove(&first);
                }
            }
            records.restorable.retain(|old| old.tree.root != tree.root);
            if !forget && let Some(head) = head {
                records.restorable.push(Saved {
                    tree: saved_tree,
                    head,
                    snapshot: saved,
                });
            }
        })
        .await
    }

    async fn restore(&self, body: &Value) -> Result<Value, Refusal> {
        let _turn = self.turn.lock().await;
        let saved = read(&self.home)?
            .restorable
            .into_iter()
            .find(|saved| {
                body["folder"].as_str().is_some_and(|p| {
                    Path::new(p) == saved.tree.root || Path::new(p) == saved.tree.folder()
                })
            })
            .ok_or_else(not_restorable)?;
        let mut tree = saved.tree;
        if !tree.project.exists() {
            return Err(conflict("Its project folder is gone."));
        }
        if tree.root.exists()
            && std::fs::read_dir(&tree.root).map_or(true, |mut entries| entries.next().is_some())
        {
            return Err(conflict("Something else is in its folder now."));
        }
        let snapshot = if let Some(commit) = saved.snapshot.as_deref() {
            git(
                &tree.project,
                &["cat-file", "-e", &format!("{commit}^{{commit}}")],
            )
            .await
            .map(|_| commit)
        } else {
            None
        };
        let branch = branch_exists(&tree).await;
        if !branch
            && git(
                &tree.project,
                &["cat-file", "-e", &format!("{}^{{commit}}", saved.head)],
            )
            .await
            .is_none()
        {
            return Err(conflict("Its work is gone, so it can’t come back."));
        }
        let _ = git(&tree.project, &["worktree", "prune"]).await;
        let root = tree.root.to_string_lossy();
        let args = if branch {
            vec!["worktree", "add", &root, &tree.branch]
        } else {
            vec!["worktree", "add", "-b", &tree.branch, &root, &saved.head]
        };
        asked(&tree.project, &args)
            .await
            .ok_or_else(|| conflict("Its folder couldn’t be made again."))?;
        if let Some(commit) = snapshot {
            let onto = git(&tree.root, &["rev-parse", "HEAD"]).await;
            let parent = git(&tree.root, &["rev-parse", &format!("{commit}^")]).await;
            let back = if onto.is_some() && onto == parent {
                asked(&tree.root, &["read-tree", "-m", "-u", "HEAD", commit]).await
            } else {
                git_with(
                    &tree.root,
                    &["cherry-pick", "--no-commit", commit],
                    Intent::Asked,
                    BY_KUVO,
                )
                .await
            };
            if back.is_none() {
                let _ = asked(&tree.root, &["cherry-pick", "--abort"]).await;
                let _ = asked(&tree.project, &["worktree", "remove", "--force", &root]).await;
                if !branch {
                    let _ = git(&tree.project, &["branch", "-D", "--", &tree.branch]).await;
                }
                return Err(conflict(format!(
                    "Its saved changes clash with newer work on {}.",
                    tree.branch
                )));
            }
            let _ = asked(&tree.root, &["reset", "--quiet"]).await;
        }
        tree.made = None;
        tree.prefix = None;
        tree.used = Some(false);
        tree.restored = Some(true);
        change(&self.home, |records| {
            records.restorable.retain(|old| old.tree.root != tree.root);
            records.worktrees.retain(|old| old.root != tree.root);
            records.worktrees.push(tree.clone());
        })
        .await?;
        if let Some(commit) = snapshot {
            drop_snapshot(&tree, commit).await;
        }
        if let Ok(rows) = self.engine.worktree_sessions().await {
            for row in rows.iter().filter(|row| tree.contains(row)) {
                let _ = self.engine.tell_worktree(&row.session).await;
            }
        }
        Ok(json!(tree))
    }

    async fn name_due(
        &self,
        records: &Records,
        rows: &[WorktreeConversation],
    ) -> Result<(), Refusal> {
        for tree in &records.worktrees {
            if tree.made.as_deref() != Some(tree.branch.as_str()) {
                continue;
            }
            let Some(title) = rows
                .iter()
                .filter(|row| row.independent && row.folder == tree.folder())
                .map(|row| slug(&row.title))
                .find(|title| !title.is_empty())
            else {
                continue;
            };
            if git(
                &tree.root,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    &format!("{}@{{upstream}}", tree.branch),
                ],
            )
            .await
            .is_some()
                || on_remote(&tree.root, &tree.branch).await
            {
                continue;
            }
            let name = format!("{}{}", tree.prefix.as_deref().unwrap_or_default(), title);
            let Some(branch) = free_branch(&tree.root, &name).await else {
                continue;
            };
            let renamed = git(&tree.root, &["branch", "-m", &tree.branch, &branch])
                .await
                .is_some();
            let branch = if renamed {
                branch
            } else {
                let Some(now) =
                    git(&tree.root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await
                else {
                    continue;
                };
                if now == tree.branch {
                    continue;
                }
                now
            };
            change(&self.home, |records| {
                if let Some(current) = records
                    .worktrees
                    .iter_mut()
                    .find(|current| current.root == tree.root && current.branch == tree.branch)
                {
                    current.branch = branch;
                }
            })
            .await?;
            if renamed {
                for row in rows.iter().filter(|row| tree.contains(row)) {
                    let _ = self.engine.tell_worktree(&row.session).await;
                }
            }
        }
        Ok(())
    }

    async fn sweep(&self) -> Result<(), Refusal> {
        let _turn = self.turn.lock().await;
        let records = read(&self.home)?;
        if records.worktrees.is_empty() && records.restorable.is_empty() {
            return Ok(());
        }
        let Ok(rows) = self.engine.worktree_sessions().await else {
            return Ok(());
        };
        self.name_due(&records, &rows).await?;
        let mut used = Vec::new();
        for tree in &records.worktrees {
            if match tree.used {
                None => rows.iter().any(|row| row.independent && tree.contains(row)),
                Some(false) => rows
                    .iter()
                    .any(|row| row.independent && tree.contains(row) && !row.archived),
                Some(true) => false,
            } {
                used.push(tree.root.clone());
            }
        }
        if !used.is_empty() {
            change(&self.home, |records| {
                for tree in &mut records.worktrees {
                    if used.contains(&tree.root) {
                        tree.used = Some(true);
                    }
                }
            })
            .await?;
        }
        let goes = |tree: &Tree, rows: &[WorktreeConversation]| {
            let has = rows.iter().any(|row| row.independent && tree.contains(row));
            (tree.used == Some(true) || (tree.used == Some(false) && !has))
                && !rows.iter().any(|row| {
                    tree.contains(row) && (row.running || row.independent && !row.archived)
                })
        };
        for tree in read(&self.home)?.worktrees {
            if !goes(&tree, &rows) {
                continue;
            }
            let Ok(now) = self.engine.worktree_sessions().await else {
                continue;
            };
            if !goes(&tree, &now) {
                continue;
            }
            let keep = now.iter().any(|row| row.independent && tree.contains(row));
            let _ = self.remove_tree(tree, !keep, false, keep, &now).await;
        }
        // Only drop a saved checkout after a fresh complete inventory confirms its last session is gone.
        if read(&self.home)?.restorable.is_empty() {
            return Ok(());
        }
        let Ok(now) = self.engine.worktree_sessions().await else {
            return Ok(());
        };
        let orphans: Vec<_> = read(&self.home)?
            .restorable
            .into_iter()
            .filter(|saved| {
                !now.iter()
                    .any(|row| row.independent && saved.tree.contains(row))
            })
            .collect();
        for saved in &orphans {
            if let Some(commit) = &saved.snapshot {
                drop_snapshot(&saved.tree, commit).await;
            }
        }
        if !orphans.is_empty() {
            change(&self.home, |records| {
                records
                    .restorable
                    .retain(|saved| !orphans.iter().any(|gone| gone.tree.root == saved.tree.root))
            })
            .await?;
        }
        Ok(())
    }
}

fn watch(owner: Owner) {
    let mut events = owner.engine.events();
    let mut states = owner.engine.states();
    let mut agents = owner.engine.agent_events();
    tokio::spawn(async move {
        loop {
            let _ = owner.sweep().await;
            loop {
                let kind = tokio::select! {
                    event = events.recv() => match event {
                        Ok(event) => event.kind.clone(), Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break, Err(_) => return,
                    },
                    change = states.changed() => { if change.is_err() { return; } break; },
                    event = async {
                        match &mut agents { Some(events) => events.recv().await, None => std::future::pending().await }
                    } => match event {
                        Ok(event) => {
                            if event.kind == "session.created" { let _ = owner.engine.tell_worktree(&event.session).await; }
                            event.kind.to_owned()
                        }, Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break, Err(_) => { agents = None; continue; }
                    },
                };
                if matches!(
                    kind.as_str(),
                    "server.connected"
                        | "session.created"
                        | "session.deleted"
                        | "session.updated"
                        | "session.metadata.updated"
                        | "session.renamed"
                        | "session.execution.succeeded"
                        | "session.execution.failed"
                        | "session.execution.cancelled"
                        | "session.execution.interrupted"
                ) {
                    break;
                }
            }
        }
    });
}

pub(super) fn register(registry: &mut Registry, home: PathBuf, engine: Engine) {
    use kuvo_protocol::routes::*;
    let owner = Owner {
        engine,
        home,
        turn: Arc::default(),
    };
    watch(owner.clone());
    let listed = owner.clone();
    registry.register::<get_kuvo_worktrees::Spec, _, _>(Access::Any, move |call| {
        let owner = listed.clone();
        async move {
            let records = call.request.path.split_once('?').is_some_and(|(_, query)| {
                form_urlencoded::parse(query.as_bytes()).any(|(k, v)| k == "records" && v == "1")
            });
            Ok(RouteReply::json_as_kept::<200>(owner.list(records).await?))
        }
    });
    let followed = owner.clone();
    registry.register::<get_kuvo_worktrees_events::Spec, _, _>(Access::Any, move |call| {
        let owner = followed.clone();
        async move {
            let mut notices = kuvo_os::watch::WatchHub::default().subscribe(owner.home.join("box"), kuvo_os::watch::Options::default()).await.map_err(storage)?;
            let stream = async_stream::try_stream! {
                let mut last = None;
                loop {
                    let row = match owner.list(true).await { Ok(row) => row, Err(_) => break };
                    if last.as_ref() != Some(&row) {
                        last = Some(row.clone());
                        let body = serde_json::from_value(row).map_err(std::io::Error::other)?;
                        yield kuvo_server::JsonLine::<get_kuvo_worktrees_events::Spec,200>::new(body)?;
                    }
                    tokio::select! {
                        _ = call.cancel.cancelled() => break,
                        notice = async {
                           loop {
                               match notices.recv().await {
                                   Some(kuvo_os::watch::Notice::Changed { paths }) if !paths.iter().any(|p| p == Path::new("worktrees.json")) => {},
                                   other => break other,
                               }
                           }
                        } => if notice.is_none() { break; },
                    }
                }
            };
            Ok(RouteReply::ndjson::<200>(stream))
        }
    });
    macro_rules! route {
        ($route:ident, $method:ident) => {
            let owner = owner.clone();
            registry.register::<$route::Spec, _, _>(Access::Any, move |call| {
                let owner = owner.clone();
                async move {
                    let body =
                        serde_json::from_slice::<Value>(&call.request.body).unwrap_or_default();
                    Ok(RouteReply::json_as_kept::<200>(owner.$method(&body).await?))
                }
            });
        };
    }
    route!(post_kuvo_worktrees, create);
    route!(post_kuvo_worktrees_record, record);
    route!(post_kuvo_worktrees_remove, remove);
    route!(post_kuvo_worktrees_restore, restore);
}
