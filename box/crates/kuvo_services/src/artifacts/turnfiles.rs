//! Files a turn changed that no tool call named (commands' outputs), matching `artifacts/turnfiles.ts`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, UNIX_EPOCH},
};

use super::kinds::strip_folder_prefix;

/// Folders whose files are never what an agent made for you (tools' own, caches).
pub const SKIPPED: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
    ".pytest_cache",
    ".mypy_cache",
    ".next",
    ".turbo",
    ".gradle",
    "target",
    "DerivedData",
];

const SCRATCH: &[&str] = &["tmp", "temp", ".tmp"];
const CLOCK_SLACK_MS: i64 = 2000;
const WALK_LIMIT: usize = 20_000;
const FILE_LIMIT: usize = 200;
const COMMIT_LIMIT: usize = 100;

pub fn is_skipped(part: &str) -> bool {
    SKIPPED.contains(&part)
}

pub fn is_scratch_part(part: &str) -> bool {
    let lower = part.to_ascii_lowercase();
    SCRATCH.contains(&lower.as_str())
}

/// Whether `path` is inside a scratch folder (`tmp`, `temp`, `.tmp`) of `project`.
pub fn in_scratch(path: &str, project: &str) -> bool {
    let Some(inside) = strip_folder_prefix(path, project) else {
        return false;
    };
    let parts: Vec<&str> = inside.split(std::path::MAIN_SEPARATOR).collect();
    if parts.len() <= 1 {
        return false;
    }
    parts[..parts.len() - 1]
        .iter()
        .any(|part| is_scratch_part(part))
}

fn own_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("KUVO_HOME")
        && !home.is_empty()
    {
        out.push(PathBuf::from(home));
    }
    if let Some(agents) = std::env::var_os("KUVO_AGENTS_HOME")
        && !agents.is_empty()
        && let Some(parent) = Path::new(&agents).parent()
    {
        out.push(parent.to_path_buf());
    }
    if let Ok(raw) = std::env::var("KUVO_OWN_FOLDERS")
        && let Ok(list) = serde_json::from_str::<Vec<String>>(&raw)
    {
        for entry in list {
            if !entry.is_empty() {
                out.push(PathBuf::from(entry));
            }
        }
    }
    out
}

async fn repository_of(folder: &Path) -> Option<PathBuf> {
    let mut at = folder.to_path_buf();
    loop {
        if tokio::fs::metadata(at.join(".git")).await.is_ok() {
            return Some(at);
        }
        if !at.pop() {
            return None;
        }
    }
}

/// Files under `folder` changed since `since_ms` (Unix ms), bounded to `FILE_LIMIT`.
pub async fn changed_since(
    folder: &Path,
    since_ms: i64,
    keep: impl Fn(&str) -> bool,
    descend: impl Fn(&str) -> bool,
) -> Vec<PathBuf> {
    let top = repository_of(folder).await;
    let folder_str = folder.to_string_lossy().into_owned();
    let own: Vec<String> = own_folders()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|inside| strip_folder_prefix(inside, &folder_str).is_some())
        .collect();

    let allowed = |path: &Path| -> bool {
        let path_str = path.to_string_lossy();
        let Some(rel) = strip_folder_prefix(&path_str, &folder_str) else {
            return false;
        };
        if rel
            .split(std::path::MAIN_SEPARATOR)
            .any(|part| is_skipped(part) || is_scratch_part(part))
        {
            return false;
        }
        !own.iter().any(|inside| {
            path_str == inside.as_str() || strip_folder_prefix(&path_str, inside).is_some()
        })
    };

    let mut found = Vec::new();
    let cutoff = since_ms.saturating_sub(CLOCK_SLACK_MS);

    if let Some(top) = top
        && let Some(paths) = from_git(&top, folder, cutoff).await
    {
        let candidates: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| {
                allowed(path)
                    && strip_folder_prefix(&path.to_string_lossy(), &folder_str)
                        .is_some_and(&keep)
            })
            .collect();
        for chunk in candidates.chunks(32) {
            if found.len() >= FILE_LIMIT {
                break;
            }
            check_batch(chunk, cutoff, &mut found).await;
        }
        return found;
    }

    let mut looked = 0usize;
    let mut stack = vec![folder.to_path_buf()];
    while let Some(at) = stack.pop() {
        if looked >= WALK_LIMIT || found.len() >= FILE_LIMIT {
            break;
        }
        let Ok(mut entries) = tokio::fs::read_dir(&at).await else {
            continue;
        };
        let mut pending = Vec::new();
        let mut children = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            looked += 1;
            if looked > WALK_LIMIT || found.len() >= FILE_LIMIT {
                break;
            }
            let path = entry.path();
            if !allowed(&path) {
                continue;
            }
            let path_str = path.to_string_lossy().into_owned();
            let Some(rel) = strip_folder_prefix(&path_str, &folder_str) else {
                continue;
            };
            let Ok(ft) = entry.file_type().await else {
                continue;
            };
            if ft.is_dir() {
                if descend(rel) {
                    children.push(path);
                }
            } else if (ft.is_file() || ft.is_symlink()) && keep(rel) {
                pending.push(path);
                if pending.len() == 32 {
                    check_batch(&pending, cutoff, &mut found).await;
                    pending.clear();
                }
            }
        }
        if !pending.is_empty() && found.len() < FILE_LIMIT {
            check_batch(&pending, cutoff, &mut found).await;
        }
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    found
}

async fn check_batch(paths: &[PathBuf], cutoff_ms: i64, found: &mut Vec<PathBuf>) {
    for path in paths {
        if found.len() >= FILE_LIMIT {
            break;
        }
        if let Ok(meta) = tokio::fs::metadata(path).await
            && meta.is_file()
            && let Ok(modified) = meta.modified()
            && let Ok(dur) = modified.duration_since(UNIX_EPOCH)
            && (dur.as_millis() as i64) >= cutoff_ms
        {
            found.push(path.clone());
        }
    }
}

async fn git_output(folder: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(folder)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

async fn from_git(top: &Path, folder: &Path, since_ms: i64) -> Option<Vec<PathBuf>> {
    let since_sec = format!("--since=@{}", since_ms.div_euclid(1000));
    let limit = format!("-n{COMMIT_LIMIT}");
    let log_args = [
        "log",
        &since_sec,
        &limit,
        "--no-merges",
        "--diff-filter=ACMR",
        "--name-only",
        "-z",
        "--format=",
        "HEAD",
        "--",
        ".",
    ];
    let (status_out, log_out) = tokio::join!(
        git_output(
            folder,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--", "."]
        ),
        git_output(folder, &log_args)
    );
    let status_out = status_out?;
    let folder_str = folder.to_string_lossy().into_owned();
    let mut paths = Vec::new();
    let entries: Vec<&str> = status_out.split('\0').collect();
    let mut i = 0;
    while i < entries.len() {
        let entry = entries[i];
        i += 1;
        if entry.len() < 4 {
            continue;
        }
        let bytes = entry.as_bytes();
        if bytes[0] == b'R' || bytes[0] == b'C' {
            i += 1;
        }
        if bytes[0] == b'D' || bytes[1] == b'D' {
            continue;
        }
        let path = top.join(&entry[3..]);
        if strip_folder_prefix(&path.to_string_lossy(), &folder_str).is_some() {
            paths.push(path);
        }
    }
    let mut listed: HashSet<PathBuf> = paths.iter().cloned().collect();
    for raw in log_out.unwrap_or_default().split('\0') {
        let name = raw.trim_start_matches('\n');
        if name.is_empty() {
            continue;
        }
        let path = top.join(name);
        if strip_folder_prefix(&path.to_string_lossy(), &folder_str).is_some()
            && listed.insert(path.clone())
        {
            paths.push(path);
        }
    }
    Some(paths)
}
