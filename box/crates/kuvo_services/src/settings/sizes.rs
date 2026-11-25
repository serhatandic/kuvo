//! Disk measurements are shared by the page's readers and cached in the TS format.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Folder {
    pub title: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub title: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<Folder>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outside {
    pub path: PathBuf,
    pub bytes: u64,
    pub trees: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sizes {
    pub at: i64,
    pub total: u64,
    pub home: u64,
    pub parts: Vec<Part>,
    pub outside: Vec<Outside>,
}
#[derive(Default)]
pub struct Measurement {
    state: Mutex<Standing>,
}
#[derive(Default)]
struct Standing {
    busy: bool,
    failed_at: i64,
    problem: Option<String>,
}
fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
fn kept(home: &Path) -> Option<Sizes> {
    std::fs::read(home.join("run/storage-sizes.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
}
impl Measurement {
    pub fn read(
        self: &Arc<Self>,
        home: &Path,
        refresh: bool,
    ) -> (Option<Sizes>, bool, Option<String>) {
        let sizes = kept(home);
        let mut state = self.state.lock().expect("measurement");
        let fresh = |at| now() - at < 3_600_000;
        if !state.busy
            && (refresh || !(sizes.as_ref().is_some_and(|s| fresh(s.at)) || fresh(state.failed_at)))
        {
            state.busy = true;
            let measure = self.clone();
            let home = home.to_owned();
            tokio::spawn(async move {
                let result = measure_sizes(&home).await.and_then(|sizes| {
                    kuvo_store::files::write_secret(
                        &home.join("run/storage-sizes.json"),
                        &serde_json::to_vec(&sizes)?,
                    )
                });
                let mut state = measure.state.lock().expect("measurement");
                state.busy = false;
                match result {
                    Ok(()) => {
                        state.problem = None;
                        state.failed_at = 0;
                    }
                    Err(e) => {
                        state.problem = Some(e.to_string());
                        state.failed_at = now();
                    }
                }
            });
        }
        (sizes, state.busy, state.problem.clone())
    }
}
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}
fn part(id: &str, title: &str, bytes: u64, folders: Vec<Folder>) -> Part {
    Part {
        id: id.into(),
        title: title.into(),
        bytes,
        folders,
    }
}
async fn measure_sizes(home: &Path) -> io::Result<Sizes> {
    let root = real(home);
    let sizes = kuvo_os::disk_usage::measure(vec![root.clone()], 3).await?;
    let of = |path: &Path| sizes.get(path).copied().unwrap_or_default();
    let home_bytes = of(&root);
    let fallback = real(&root.join("data/opencode/worktree"));
    let records: Value = std::fs::read(root.join("box/worktrees.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    let roots = records["worktrees"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["root"].as_str())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| real(&p))
        .filter(|p| !p.starts_with(&fallback) && p.exists())
        .collect::<BTreeSet<_>>();
    let root_sizes = kuvo_os::disk_usage::measure(roots.iter().cloned().collect(), 0).await?;
    let sized = |p: &Path| {
        sizes
            .get(p)
            .or_else(|| root_sizes.get(p))
            .copied()
            .unwrap_or_default()
    };
    let mut inside = roots
        .iter()
        .filter(|p| p.starts_with(&root))
        .cloned()
        .collect::<Vec<_>>();
    if fallback.exists() && fallback.starts_with(&root) {
        inside.push(fallback);
    }
    let mut by_folder = BTreeMap::<PathBuf, Outside>::new();
    for path in roots.iter().filter(|p| !p.starts_with(&root)) {
        if let Some(folder) = path.parent() {
            let each = by_folder.entry(folder.to_owned()).or_insert(Outside {
                path: folder.to_owned(),
                bytes: 0,
                trees: 0,
            });
            each.bytes += sized(path);
            each.trees += 1;
        }
    }
    let outside = by_folder.into_values().collect::<Vec<_>>();
    let outside_bytes = outside.iter().map(|p| p.bytes).sum::<u64>();
    let sessions = [
        root.join("data/opencode"),
        root.join("chats"),
        root.join("attachments"),
    ]
    .into_iter()
    .map(|p| real(&p))
    .filter(|p| p.starts_with(&root))
    .collect::<Vec<_>>();
    let sessions_bytes = sessions.iter().map(|p| of(p)).sum::<u64>().saturating_sub(
        inside
            .iter()
            .filter(|p| sessions.iter().any(|s| p.starts_with(s)))
            .map(|p| sized(p))
            .sum::<u64>(),
    );
    let trees = inside.iter().map(|p| sized(p)).sum::<u64>();
    let made = of(&root.join("artifacts"));
    let browser = of(&root.join("browser"));
    let other = home_bytes.saturating_sub(sessions_bytes + trees + made + browser);
    let counted = sessions
        .into_iter()
        .chain(inside)
        .chain([root.join("artifacts"), root.join("browser")])
        .collect::<Vec<_>>();
    let mut folders = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let path = entry.path();
            if name.starts_with('.') || counted.iter().any(|p| p.starts_with(&path)) {
                return None;
            }
            let bytes = of(&path);
            if bytes == 0 {
                return None;
            }
            let title = match name.as_str() {
                "agents" => "Coding agents",
                "engine" => "The engine",
                "tools" => "Tools",
                "cache" => "Caches",
                "trash" => "Deleted, kept a while",
                "bin" => "Programs",
                "box" => "Settings and keys",
                "run" => "Running state",
                "updates" => "Updates",
                "search" => "Search index",
                "integrations" => "Integrations",
                "bots" => "Bots",
                "groups" => "Group chats",
                "wakes" => "Wake-ups",
                "workspace" => "Workspace",
                "state" => "State",
                "config" => "Engine settings",
                "t" | "tmp" => "Temporary files",
                "keys" => "Keys",
                "attachments" => "Attachments",
                "chats" => "Chats",
                "data" => "Sessions",
                _ => &name,
            };
            Some(Folder {
                title: title.into(),
                bytes,
            })
        })
        .collect::<Vec<_>>();
    folders.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let remainder = other.saturating_sub(folders.iter().map(|f| f.bytes).sum::<u64>());
    if remainder > 0 {
        folders.push(Folder {
            title: "Other files".into(),
            bytes: remainder,
        });
    }
    let parts = vec![
        part("sessions", "Sessions", sessions_bytes, vec![]),
        part("worktrees", "Worktrees", trees + outside_bytes, vec![]),
        part("made", "What agents made", made, vec![]),
        part("browser", "Browser", browser, vec![]),
        part("other", "Other", other, folders),
    ];
    Ok(Sizes {
        at: now(),
        total: home_bytes + outside_bytes,
        home: home_bytes,
        parts,
        outside,
    })
}
