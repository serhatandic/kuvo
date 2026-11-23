//! git as the box runs it by itself in your folders, for the names it lists: never asking
//! anything, taking no locks it needn't, and starting nothing the repository's settings name.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Options before every command the box runs by itself (`OWN` in `src/shared/git.ts`).
const OWN: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "log.showSignature=false",
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "commit.gpgSign=false",
    "-c",
    "tag.gpgSign=false",
    "-c",
    "credential.helper=",
];
/// How long a listing may take (stuck on a lock or a hung disk): what it gave by then.
const TIMEOUT: Duration = Duration::from_secs(600);

/// The names git lists, each once, and whether that's all of them.
pub(crate) struct Listed {
    pub(crate) names: Vec<String>,
    pub(crate) complete: bool,
}

/// The files git has in `folder` (tracked and new, without what it ignores, names as they are), as
/// many as `limit`, git stopped once there are enough. None where git failed (outside a repository).
pub(crate) async fn names(folder: &Path, limit: usize) -> Option<Listed> {
    let mut git = Command::new("git");
    #[cfg(windows)]
    git.args(["-c", "core.longpaths=true"]);
    let mut child = git
        .args(["-c", "core.quotepath=off"])
        .args(OWN)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(folder)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    let mut rest = Vec::new();
    let mut buffer = vec![0; 64 * 1024];
    let read = async {
        loop {
            let Ok(count) = out.read(&mut buffer).await else {
                return false;
            };
            if count == 0 {
                return false;
            }
            rest.extend_from_slice(&buffer[..count]);
            while let Some(end) = rest.iter().position(|byte| *byte == 0) {
                let name: Vec<u8> = rest.drain(..=end).collect();
                let name = String::from_utf8_lossy(&name[..end]).into_owned();
                if !name.is_empty() && seen.insert(name.clone()) {
                    names.push(name);
                }
                if names.len() >= limit {
                    return true;
                }
            }
        }
    };
    let enough = match tokio::time::timeout(TIMEOUT, read).await {
        Ok(enough) => enough,
        Err(_) => {
            let _ = child.kill().await;
            return Some(Listed {
                names,
                complete: false,
            });
        }
    };
    if enough {
        let _ = child.kill().await;
        return Some(Listed {
            names,
            complete: false,
        });
    }
    let status = child.wait().await.ok()?;
    status.success().then_some(Listed {
        names,
        complete: true,
    })
}
