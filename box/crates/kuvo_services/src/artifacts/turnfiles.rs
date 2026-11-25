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
    ".git", "node_modules", ".venv", "venv", "__pycache__", ".cache", ".pytest_cache",
    ".mypy_cache", ".next", ".turbo", ".gradle", "target", "DerivedData",
];

const SCRATCH: &[&str] = &["tmp", "temp", ".tmp"];
const CLOCK_SLACK_MS: i64 = 2000;
const WALK_LIMIT: usize = 20_000;
const FILE_LIMIT: usize = 200;
const COMMIT_LIMIT: usize = 100;

pub fn is_skipped(part: &str) -> bool { SKIPPED.contains(&part) }
pub fn is_scratch_part(part: &str) -> bool { SCRATCH.contains(&part.to_ascii_lowercase().as_str()) }

/// Whether `path` is inside a scratch folder (`tmp`, `temp`, `.tmp`) of `project`.
pub fn in_scratch(path: &str, project: &str) -> bool {
    let Some(inside) = strip_folder_prefix(path, project) else { return false };
    let parts: Vec<&str> = inside.split(std::path::MAIN_SEPARATOR).collect();
    parts.len() > 1 && parts[..parts.len() - 1].iter().any(|p| is_scratch_part(p))
}

fn own_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("KUVO_HOME").filter(|v| !v.is_empty()) { out.push(PathBuf::from(home)); }
    if let Some(parent) = std::env::var_os("KUVO_AGENTS_HOME").filter(|v| !v.is_empty()).and_then(|a| Path::new(&a).parent().map(Path::to_path_buf)) {
        out.push(parent);
    }
    if let Ok(raw) = std::env::var("KUVO_OWN_FOLDERS") && let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
        out.extend(list.into_iter().filter(|e| !e.is_empty()).map(PathBuf::from));
    }
    out
}

async fn repository_of(folder: &Path) -> Option<PathBuf> {
    let mut at = folder.to_path_buf();
    loop {
        if tokio::fs::metadata(at.join(".git")).await.is_ok() { return Some(at); }
        if !at.pop() { return None; }
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
    let own: Vec<String> = own_folders().into_iter().map(|p| p.to_string_lossy().into_owned())
        .filter(|inside| strip_folder_prefix(inside, &folder_str).is_some()).collect();

    let allowed = |path: &Path| -> bool {
        let path_str = path.to_string_lossy();
        let Some(rel) = strip_folder_prefix(&path_str, &folder_str) else { return false };
        !rel.split(std::path::MAIN_SEPARATOR).any(|p| is_skipped(p) || is_scratch_part(p))
            && !own.iter().any(|inside| path_str == inside.as_str() || strip_folder_prefix(&path_str, inside).is_some())
    };

    let (mut found, cutoff) = (Vec::new(), since_ms.saturating_sub(CLOCK_SLACK_MS));
    if let Some(top) = top && let Some(paths) = from_git(&top, folder, cutoff).await {
        let candidates: Vec<PathBuf> = paths.into_iter()
            .filter(|p| allowed(p) && strip_folder_prefix(&p.to_string_lossy(), &folder_str).is_some_and(&keep))
            .collect();
        for chunk in candidates.chunks(32) {
            if found.len() >= FILE_LIMIT { break; }
            check_batch(chunk, cutoff, &mut found).await;
        }
        return found;
    }

    let (mut looked, mut stack) = (0usize, vec![folder.to_path_buf()]);
    while let Some(at) = stack.pop() {
        if looked >= WALK_LIMIT || found.len() >= FILE_LIMIT { break; }
        let Ok(mut entries) = tokio::fs::read_dir(&at).await else { continue };
        let (mut pending, mut children) = (Vec::new(), Vec::new());
        while let Ok(Some(entry)) = entries.next_entry().await {
            looked += 1;
            if looked > WALK_LIMIT || found.len() >= FILE_LIMIT { break; }
            let path = entry.path();
            if !allowed(&path) { continue; }
            let path_str = path.to_string_lossy().into_owned();
            let Some(rel) = strip_folder_prefix(&path_str, &folder_str) else { continue };
            let Ok(ft) = entry.file_type().await else { continue };
            if ft.is_dir() {
                if descend(rel) { children.push(path); }
            } else if (ft.is_file() || ft.is_symlink()) && keep(rel) {
                pending.push(path);
                if pending.len() == 32 { check_batch(&pending, cutoff, &mut found).await; pending.clear(); }
            }
        }
        if !pending.is_empty() && found.len() < FILE_LIMIT { check_batch(&pending, cutoff, &mut found).await; }
        stack.extend(children.into_iter().rev());
    }
    found
}

async fn check_batch(paths: &[PathBuf], cutoff_ms: i64, found: &mut Vec<PathBuf>) {
    for path in paths {
        if found.len() >= FILE_LIMIT { break; }
        if let Ok(meta) = tokio::fs::metadata(path).await && meta.is_file()
            && meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .is_some_and(|d| (d.as_millis() as i64) >= cutoff_ms)
        {
            found.push(path.clone());
        }
    }
}

async fn git_output(folder: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(folder).args(args)
        .env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), cmd.output()).await.ok()?.ok()?;
    output.status.success().then(|| String::from_utf8(output.stdout).ok()).flatten()
}

async fn from_git(top: &Path, folder: &Path, since_ms: i64) -> Option<Vec<PathBuf>> {
    let (since_sec, limit) = (format!("--since=@{}", since_ms.div_euclid(1000)), format!("-n{COMMIT_LIMIT}"));
    let log_args = ["log", &since_sec, &limit, "--no-merges", "--diff-filter=ACMR", "--name-only", "-z", "--format=", "HEAD", "--", "."];
    let (status_out, log_out) = tokio::join!(
        git_output(folder, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--", "."]),
        git_output(folder, &log_args)
    );
    let (status_out, folder_str, mut paths) = (status_out?, folder.to_string_lossy().into_owned(), Vec::new());
    let entries: Vec<&str> = status_out.split('\0').collect();
    let mut i = 0;
    while i < entries.len() {
        let entry = entries[i];
        i += 1;
        if entry.len() < 4 { continue; }
        let b = entry.as_bytes();
        if b[0] == b'R' || b[0] == b'C' { i += 1; }
        if b[0] == b'D' || b[1] == b'D' { continue; }
        let path = top.join(&entry[3..]);
        if strip_folder_prefix(&path.to_string_lossy(), &folder_str).is_some() { paths.push(path); }
    }
    let mut listed: HashSet<PathBuf> = paths.iter().cloned().collect();
    for name in log_out.unwrap_or_default().split('\0').map(|r| r.trim_start_matches('\n')).filter(|s| !s.is_empty()) {
        let path = top.join(name);
        if strip_folder_prefix(&path.to_string_lossy(), &folder_str).is_some() && listed.insert(path.clone()) {
            paths.push(path);
        }
    }
    Some(paths)
}
