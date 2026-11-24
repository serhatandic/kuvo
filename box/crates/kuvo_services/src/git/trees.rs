//! A project's git worktrees (`work/project-trees.ts`): only the ones its repository lists that name
//! it back, each `.git/worktrees/<name>/gitdir` leading to a folder whose own `.git` file leads back
//! there, `.git/worktrees` and each entry where they're named (no link to another repository's).
//! What git keeps is read only as a small regular file, never by running git.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::files::{real, resolved};

/// The most of a file git keeps that's read.
const MOST_READ: u64 = 4096;
/// How long the linked worktrees found stand before they're looked for again.
const LINKED_FOR: Duration = Duration::from_secs(15);
/// How long one look may take (macOS asking you about a project's folder, a slow disk).
const LOOK_FOR: Duration = Duration::from_secs(30);

/// The deepest folder of `path` that's there, its links followed, and the rest as written.
fn realish(path: &Path) -> PathBuf {
    let full = resolved(path);
    let mut rest = Vec::new();
    let mut at = full.as_path();
    loop {
        if let Some(found) = real(at) {
            return rest
                .iter()
                .rev()
                .fold(found, |path, part: &&std::ffi::OsStr| path.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                at = parent;
            }
            _ => return full,
        }
    }
}

/// A small regular file's text: never a FIFO, a device or a link (opened without waiting and
/// without following one, and what's opened is what's looked at); none for anything else.
fn small_file(file: &Path) -> Option<String> {
    let mut opened = platform::open_plain(file).ok()?;
    let kind = opened.metadata().ok()?;
    if !kind.is_file() || kind.len() > MOST_READ {
        return None;
    }
    let mut bytes = Vec::new();
    (&mut opened).take(MOST_READ).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Whether `git` is a repository's own folder: not a link, its `HEAD` a file, `objects` and
/// `refs` folders.
fn is_repository(git: &Path) -> bool {
    let plain = fs::symlink_metadata(git).is_ok_and(|meta| !meta.file_type().is_symlink());
    plain
        && fs::symlink_metadata(git.join("HEAD")).is_ok_and(|meta| meta.is_file())
        && fs::metadata(git.join("objects")).is_ok_and(|meta| meta.is_dir())
        && fs::metadata(git.join("refs")).is_ok_and(|meta| meta.is_dir())
}

/// Where the `gitdir:` line of a worktree's `.git` file leads, by its real path.
fn named_back(top: &Path) -> Option<PathBuf> {
    let text = small_file(&top.join(".git"))?;
    let named = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    (!named.is_empty())
        .then(|| real(&resolved(&top.join(named))))
        .flatten()
}

/// The repository a folder is in, from it upwards: its working folder's top, and the main working
/// folder of its repository (the same for the main one; for a linked worktree, the folder whose
/// `.git` holds it, where it names it back).
struct Repository {
    top: PathBuf,
    main: PathBuf,
}

fn repository_of(folder: &Path) -> Option<Repository> {
    let start = realish(folder);
    for at in start.ancestors() {
        let git = at.join(".git");
        let kind = fs::symlink_metadata(&git).ok();
        if kind.as_ref().is_some_and(fs::Metadata::is_dir) {
            return is_repository(&git).then(|| Repository {
                top: at.to_owned(),
                main: at.to_owned(),
            });
        }
        if kind.as_ref().is_some_and(fs::Metadata::is_file) {
            let own = named_back(at)?;
            let trees = own.parent()?;
            if trees.file_name()? != "worktrees" {
                return None;
            }
            let common = trees.parent()?;
            if common.file_name()? != ".git" || !is_repository(common) {
                return None;
            }
            if real(trees).as_deref() != Some(trees)
                || !fs::symlink_metadata(&own).is_ok_and(|meta| meta.is_dir())
            {
                return None;
            }
            let back = small_file(&own.join("gitdir"))?;
            let back = back.trim();
            let leads_here = !back.is_empty()
                && real(&resolved(&own.join(back))).as_deref() == Some(git.as_path());
            if !leads_here {
                return None;
            }
            return Some(Repository {
                top: at.to_owned(),
                main: common.parent()?.to_owned(),
            });
        }
        if kind.is_some() {
            return None;
        }
    }
    None
}

/// The linked worktrees of the repository whose main working folder is `main`, by their real
/// paths: each its `.git/worktrees` lists whose folder names it back.
fn linked_worktrees(main: &Path) -> Vec<PathBuf> {
    let Some(top) = real(main) else {
        return Vec::new();
    };
    let git = top.join(".git");
    let trees = git.join("worktrees");
    if !is_repository(&git) || real(&trees).as_deref() != Some(trees.as_path()) {
        return Vec::new();
    }
    let Ok(names) = fs::read_dir(&trees) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for name in names.flatten() {
        let own = trees.join(name.file_name());
        if !fs::symlink_metadata(&own).is_ok_and(|meta| meta.is_dir())
            || real(&own).as_deref() != Some(own.as_path())
        {
            continue;
        }
        let Some(gitdir) = small_file(&own.join("gitdir")) else {
            continue;
        };
        let gitdir = gitdir.trim();
        if gitdir.is_empty() {
            continue;
        }
        let Some(tree) = resolved(&own.join(gitdir)).parent().and_then(real) else {
            continue;
        };
        if tree != top && named_back(&tree).as_deref() == Some(own.as_path()) {
            found.push(tree);
        }
    }
    found
}

/// The folders of a project in its repository's linked worktrees, where it's in the repository's
/// main working folder (its place in each).
fn project_worktrees(folder: &Path) -> Vec<PathBuf> {
    let Some(repository) = repository_of(folder) else {
        return Vec::new();
    };
    if repository.top != repository.main {
        return Vec::new();
    }
    let real_folder = realish(folder);
    let within = real_folder
        .strip_prefix(&repository.top)
        .unwrap_or(Path::new(""));
    linked_worktrees(&repository.main)
        .into_iter()
        .map(|tree| {
            within
                .components()
                .filter(|part| matches!(part, Component::Normal(_)))
                .fold(tree, |path, part| path.join(part))
        })
        .collect()
}

/// Each project's linked worktrees, by the folder its sessions run in there, and its project; not
/// one that's a project of its own.
fn look(projects: &[String]) -> BTreeMap<String, String> {
    let own: HashSet<PathBuf> = projects
        .iter()
        .map(|project| realish(Path::new(project)))
        .collect();
    let mut found = BTreeMap::new();
    for project in projects {
        for tree in project_worktrees(Path::new(project)) {
            if !own.contains(&tree) {
                found
                    .entry(tree.to_string_lossy().into_owned())
                    .or_insert_with(|| project.clone());
            }
        }
    }
    found
}

#[derive(Default)]
struct Found {
    trees: BTreeMap<String, String>,
    at: Option<Instant>,
    looking: bool,
}

/// The projects' linked worktrees as last found, looked for again off the box's thread where that
/// was a while ago (`linkedTrees`): what's answered never waits on a look.
#[derive(Clone, Default)]
pub(crate) struct LinkedTrees(Arc<Mutex<Found>>);

impl LinkedTrees {
    pub(crate) fn as_last_found(
        &self,
        projects: impl FnOnce() -> Vec<String>,
    ) -> BTreeMap<String, String> {
        let mut found = self.0.lock().expect("linked worktrees");
        let stale = found.at.is_none_or(|at| at.elapsed() > LINKED_FOR);
        if stale && !found.looking {
            found.looking = true;
            let projects = projects();
            let shared = self.0.clone();
            tokio::spawn(async move {
                let looked = tokio::time::timeout(
                    LOOK_FOR,
                    tokio::task::spawn_blocking(move || look(&projects)),
                )
                .await;
                let mut found = shared.lock().expect("linked worktrees");
                if let Ok(Ok(trees)) = looked {
                    found.trees = trees;
                }
                found.at = Some(Instant::now());
                found.looking = false;
            });
        }
        found.trees.clone()
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::os::unix::fs::OpenOptionsExt;

    pub(super) fn open_plain(file: &std::path::Path) -> std::io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(file)
    }
}

#[cfg(not(unix))]
mod platform {
    use std::fs::File;

    /// Windows has no FIFOs to hold a reader; a link is refused by looking first.
    pub(super) fn open_plain(file: &std::path::Path) -> std::io::Result<File> {
        if std::fs::symlink_metadata(file)?.file_type().is_symlink() {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        File::open(file)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(folder: &Path, args: &[&str]) {
        let ran = Command::new("git")
            .args(args)
            .current_dir(folder)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
    }

    #[test]
    fn a_projects_linked_worktrees_name_it_back() {
        let scratch = tempfile::tempdir().unwrap();
        let root = real(scratch.path()).unwrap();
        let main = root.join("app");
        fs::create_dir_all(main.join("web")).unwrap();
        git(&main, &["-c", "init.defaultBranch=main", "init", "-q"]);
        fs::write(main.join("web/a"), "a").unwrap();
        git(&main, &["add", "."]);
        git(
            &main,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "x",
            ],
        );
        git(
            &main,
            &["worktree", "add", "-q", root.join("try").to_str().unwrap()],
        );
        let projects = [
            main.to_string_lossy().into_owned(),
            main.join("web").to_string_lossy().into_owned(),
        ];
        let found = look(&projects);
        assert_eq!(
            found.get(&root.join("try").to_string_lossy().into_owned()),
            Some(&projects[0])
        );
        assert_eq!(
            found.get(&root.join("try/web").to_string_lossy().into_owned()),
            Some(&projects[1])
        );
        // A worktree whose `.git` doesn't name it back isn't one.
        fs::write(root.join("try/.git"), "gitdir: /elsewhere\n").unwrap();
        assert!(look(&projects).is_empty());
    }
}
