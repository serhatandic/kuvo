//! A new project made on this box (PROTOCOL.md, "Work": `POST kuvo/projects/new`, and the
//! make_project tool): a folder in the projects folder you picked (or the one you named for it), a
//! new git repository there (or one cloned into it), added to the box's projects, marked with the
//! bot that made it. The box makes projects only there, and never deletes one.

use std::{
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

use kuvo_services::git::safe;
use serde_json::{Map, Value, json};
use tokio::{process::Command, task::spawn_blocking};

use super::{WorkSettings, asks::project_place, settings::normalized};

/// Why one wasn't made: a status and its words. No projects folder picked yet (403, which needs
/// `projectsFolder`), a name or address that won't do (400), a folder that's there with files in it
/// already (409), git failing (502), or this computer failing (500).
pub type Refused = (u16, String);

fn no<T>(status: u16, message: impl Into<String>) -> Result<T, Refused> {
    Err((status, message.into()))
}

fn failed(error: impl ToString) -> Refused {
    (500, error.to_string())
}

/// What a made project is: a new git repository, one cloned, or a plain folder (as asked, or as
/// git isn't there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Git,
    Cloned,
    Plain,
    NoGit,
}

/// A project made: what's kept of it, and what it is.
pub struct Made {
    pub project: Map<String, Value>,
    pub kind: Kind,
}

fn text<'a>(given: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    let text = given.get(key)?.as_str()?.trim();
    (!text.is_empty()).then_some(text)
}

fn base(folder: &Path) -> String {
    let base = folder.file_name().unwrap_or_default();
    base.to_string_lossy().into_owned()
}

/// A repository address git clones without asking anything: https, or ssh (`git@host:path`,
/// `ssh://`). Never a path here, or a command in disguise.
pub fn cloneable(address: &str) -> bool {
    let starts = |part: &str| part.starts_with(|ch: char| ch.is_ascii_alphanumeric());
    let only = |part: &str, more: &str| {
        let fits = |ch: char| ch.is_ascii_alphanumeric() || more.contains(ch);
        starts(part) && part.chars().all(fits)
    };
    if address.starts_with('-') || address.contains(char::is_whitespace) {
        return false;
    }
    let web = address.strip_prefix("https://");
    if let Some(rest) = web.or(address.strip_prefix("ssh://")) {
        let (host, path) = rest.split_once('/').unwrap_or_default();
        return starts(host) && !path.is_empty();
    }
    let Some((user, rest)) = address.split_once('@') else {
        return false;
    };
    let (host, path) = rest.split_once(':').unwrap_or_default();
    only(&user.replace(['_', '.', '-'], "a"), "") && only(host, ".-") && !path.is_empty()
}

/// A path as you'd write it: under your home folder with `~`.
pub fn shown_path(folder: &str, home: &Path) -> String {
    match folder.strip_prefix(home.to_string_lossy().as_ref()) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => folder.to_owned(),
    }
}

fn is_empty_folder(folder: &Path) -> bool {
    let finders = |entry: std::io::Result<std::fs::DirEntry>| {
        entry.is_ok_and(|entry| entry.file_name() == ".DS_Store")
    };
    std::fs::read_dir(folder).is_ok_and(|mut entries| entries.all(finders))
}

/// The folder you named for a project (written with `~` or in full), as a full path, or why it
/// can't be one.
fn named_place(given: Option<&str>, home: &Path) -> Result<PathBuf, Refused> {
    let given = given.map(str::trim).unwrap_or_default();
    let written = match given.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", home.display())
        }
        _ => given.to_owned(),
    };
    if given.is_empty() {
        return no(400, "Give the folder as a full path.");
    }
    if !Path::new(&written).is_absolute() {
        let bare = given.strip_prefix("./");
        let bare = bare.or(given.strip_prefix('/')).unwrap_or(given);
        return no(
            400,
            format!("Give the folder as a full path, like ~/{bare}."),
        );
    }
    let folder = normalized(Path::new(&written));
    if folder.parent().is_none() || folder == home {
        let shown = shown_path(&folder.to_string_lossy(), home);
        return no(
            400,
            format!("{shown} can't be a project's folder: name one inside it."),
        );
    }
    Ok(folder)
}

/// A command run to its end, never asking anything, stopped after `limit`.
async fn run(mut command: Command, limit: u64) -> Result<Output, String> {
    command.stdin(Stdio::null()).kill_on_drop(true);
    let limit = Duration::from_secs(limit);
    let output = tokio::time::timeout(limit, command.output()).await;
    let output = output.map_err(|_| "git took too long".to_owned())?;
    output.map_err(|error| error.to_string())
}

/// Git in `folder` by the box itself (`gitLater`): its trimmed output, or none where it failed.
pub(crate) async fn git(folder: &Path, args: &[&str], env: &[(&str, &str)]) -> Option<String> {
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let at = folder.to_owned();
    let own = spawn_blocking(move || safe::own_args(&args, Some(&at), None));
    let mut command = Command::new("git");
    command.args(own.await.ok()?.ok()?).current_dir(folder);
    command.envs(env.iter().copied()).stderr(Stdio::null());
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env("GIT_TERMINAL_PROMPT", "0");
    let output = run(command, 60).await.ok()?;
    let said = String::from_utf8_lossy(&output.stdout);
    output.status.success().then(|| said.trim().to_owned())
}

const BY_KUVO: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Kuvo Code"),
    ("GIT_AUTHOR_EMAIL", "kuvo-code@localhost"),
    ("GIT_COMMITTER_NAME", "Kuvo Code"),
    ("GIT_COMMITTER_EMAIL", "kuvo-code@localhost"),
];

/// A repository with no commits yet given an empty first one, so its sessions can each start in a
/// worktree at once: by whoever commits there, else by Kuvo Code. Only one with nothing in it.
async fn first_commit(folder: &Path) -> bool {
    let has = async || git(folder, &["rev-parse", "--verify", "--quiet", "HEAD"], &[]).await;
    if has().await.is_some() {
        return true;
    }
    let none = git(folder, &["for-each-ref", "--count=1", "--format=x"], &[]).await;
    let empty = git(folder, &["mktree"], &[]).await;
    let Some(empty) = empty.filter(|tree| !tree.is_empty() && none.as_deref() == Some("")) else {
        return false;
    };
    let message = ["-m", "Initial commit"];
    let tree = [
        &["-c", "user.useConfigOnly=true", "commit-tree", &empty][..],
        &message,
    ]
    .concat();
    let commit = match git(folder, &tree, &[]).await {
        None => git(folder, &tree, BY_KUVO).await,
        made => made,
    };
    // (Another may have made one meanwhile: then it has one.)
    if let Some(commit) = commit {
        let reason = ["-m", "commit (initial): Initial commit"];
        let update = [&["update-ref"][..], &reason, &["HEAD", &commit, ""]].concat();
        if git(folder, &update, &[]).await.is_some() {
            return true;
        }
    }
    has().await.is_some()
}

/// git clone, for as long as a big repository takes (ten minutes at most), never asking anything:
/// ssh too, unless you set how git runs it. Why not, if it failed.
async fn clone_into(address: &str, folder: &Path) -> Option<String> {
    let set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let parent = folder.parent().unwrap_or(folder);
    let config = git(parent, &["config", "--get", "core.sshCommand"], &[]).await;
    let own_ssh = set("GIT_SSH_COMMAND") || set("GIT_SSH") || config.is_some();
    let into = folder.to_string_lossy();
    let args = ["clone", "--quiet", "--", address, &into].map(str::to_owned);
    let asked = spawn_blocking(move || safe::asked_args(&args, None)).await;
    let Ok(Ok(args)) = asked else {
        return Some("Kuvo Code won’t run git there.".into());
    };
    let mut command = Command::new("git");
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    if !own_ssh {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let output = match run(command, 600).await {
        Ok(output) if output.status.success() => return None,
        Ok(output) => output,
        Err(error) => return Some(error),
    };
    let said = String::from_utf8_lossy(&output.stderr);
    let last = said.trim().lines().last();
    Some(last.unwrap_or("git failed").to_owned())
}

/// A new project called `given.name`, in the projects folder (or the folder named for it): what's
/// kept of it, or why not.
pub async fn make_project(
    given: &Map<String, Value>,
    made_by: Option<Value>,
    settings: &WorkSettings,
    box_home: &Path,
) -> Result<Made, Refused> {
    let home = settings.account();
    let Some(name) = text(given, "name") else {
        return no(400, "Say what it's called (name).");
    };
    let named = match given.get("folder") {
        None | Some(Value::Null) => None,
        Some(folder) => Some(named_place(folder.as_str(), home)?),
    };
    let root = settings.projects_folder();
    if named.is_none() && root.is_none() {
        return no(
            403,
            "There's no projects folder here yet: the user picks one.",
        );
    }
    let place = named
        .clone()
        .or_else(|| project_place(name, root.as_deref()));
    let Some(folder) = place else {
        return no(
            400,
            format!("{name} can't be a folder's name: use letters or numbers."),
        );
    };
    let clone = text(given, "clone");
    if let Some(clone) = clone.filter(|clone| !cloneable(clone)) {
        let why = "isn't a repository address git can clone: give an https or ssh one.";
        return no(400, format!("{clone} {why}"));
    }
    let directory = folder.to_string_lossy().into_owned();
    let shown = shown_path(&directory, home);
    let kept = kuvo_services::git::read_projects(box_home).map_err(failed)?;
    let here = |each: &&Map<String, Value>| {
        each["directory"] == directory.as_str() && each.get("removed").is_none()
    };
    if let Some(listed) = kept.iter().find(here) {
        let listed = listed.get("name").and_then(Value::as_str);
        let listed = listed.map_or_else(|| base(&folder), str::to_owned);
        return no(
            409,
            format!("There's a project at {shown} already: {listed}."),
        );
    }
    let there = folder.exists();
    if there && !is_empty_folder(&folder) {
        let next = match named {
            Some(_) => format!(
                "To work on what's there, start sessions in it with start_session (folder: \"{shown}\")."
            ),
            None => "Pick another name.".into(),
        };
        let why = format!("There's a folder at {shown} already, with files in it. {next}");
        return no(409, why);
    }
    let kind = if let Some(clone) = clone {
        // git makes the folder, and takes away what it made of one that failed. (An empty one there
        // goes first: git clones only into a folder it makes, and a Finder note isn't anything.)
        if there {
            let _ = std::fs::remove_file(folder.join(".DS_Store"));
            std::fs::remove_dir(&folder).map_err(failed)?;
        }
        if let Some(why) = clone_into(clone, &folder).await {
            if !there && is_empty_folder(&folder) {
                let _ = std::fs::remove_dir(&folder);
            }
            return no(502, format!("Couldn't clone {clone}: {why}"));
        }
        Kind::Cloned
    } else {
        std::fs::create_dir_all(&folder).map_err(failed)?;
        let init = async |args: &[&str]| git(&folder, args, &[]).await.is_some();
        match given.get("git") {
            Some(Value::Bool(false)) => Kind::Plain,
            _ if init(&["init", "--quiet", "--initial-branch=main"]).await => Kind::Git,
            _ if init(&["init", "--quiet"]).await => Kind::Git,
            _ => Kind::NoGit,
        }
    };
    if matches!(kind, Kind::Git | Kind::Cloned) {
        first_commit(&folder).await;
    }
    let renamed = base(&folder) != name;
    let project = kuvo_services::git::change_projects(box_home, |projects| {
        let at = projects
            .iter()
            .position(|each| each["directory"] == directory.as_str());
        let before = at.map(|at| projects.remove(at));
        // (A new one's folders are a client's to set: until one does, those a Mac from before gives
        // its sessions stay theirs, `addedByBox`.)
        let mut project = before.clone().unwrap_or_default();
        project.insert("directory".into(), json!(directory));
        let folders = before
            .as_ref()
            .and_then(|before| before.get("folders").cloned());
        project.insert("folders".into(), folders.unwrap_or(json!([])));
        let new = before.is_none().then_some(("addedByBox", json!(true)));
        let named = renamed.then(|| ("name", json!(name)));
        let more = [new, named, made_by.map(|by| ("madeBy", by))];
        for (key, value) in more.into_iter().flatten() {
            project.insert(key.into(), value);
        }
        project.shift_remove("removed");
        projects.insert(0, project.clone());
        project
    });
    Ok(Made {
        project: project.map_err(failed)?,
        kind,
    })
}

/// What a made project is, in the words its maker reads.
pub fn made_words(made: &Made, home: &Path, here: &str) -> String {
    let directory = made.project["directory"].as_str().unwrap_or_default();
    let name = made.project.get("name").and_then(Value::as_str);
    let name = name.map_or_else(|| base(Path::new(directory)), str::to_owned);
    let what = match made.kind {
        Kind::Git => "a new git repository".to_owned(),
        Kind::Cloned => "cloned into it".to_owned(),
        Kind::Plain => "a plain folder".to_owned(),
        Kind::NoGit => format!("a plain folder (git isn't on {here})"),
    };
    let shown = shown_path(directory, home);
    let next = format!("start sessions in it with start_session (project: \"{name}\")");
    format!("Made {name} at {shown}, {what}, on {here}. It's in the sidebar; {next}.")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{cloneable, shown_path};

    #[test]
    fn only_addresses_git_clones_without_asking() {
        for good in [
            "https://github.com/a/b",
            "ssh://host/x.git",
            "git@github.com:a/b.git",
        ] {
            assert!(cloneable(good), "{good}");
        }
        let bad = [
            "-u x",
            "/tmp/repo",
            "file:///tmp/repo",
            "https://host",
            "https:///x",
            "git@host",
        ];
        for bad in bad
            .into_iter()
            .chain(["git@-h:x", "a b", "a@b@c:d", "x:y@h:p", "https://-h/x"])
        {
            assert!(!cloneable(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_new_repository_starts_with_an_empty_commit_and_nothing_left_over() {
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path();
        assert!(
            super::git(folder, &["init", "--quiet"], &[])
                .await
                .is_some()
        );
        assert!(super::first_commit(folder).await);
        let log = super::git(folder, &["log", "--format=%s"], &[]).await;
        assert_eq!(log.as_deref(), Some("Initial commit"));
        let status = super::git(folder, &["status", "--porcelain"], &[]).await;
        assert_eq!(status.as_deref(), Some(""));
        assert!(
            super::first_commit(folder).await,
            "one with a commit has one"
        );
    }

    #[test]
    fn a_path_under_your_home_is_shown_with_a_tilde() {
        let home = Path::new("/home/ada");
        assert_eq!(shown_path("/home/ada/Projects/x", home), "~/Projects/x");
        assert_eq!(shown_path("/home/ada", home), "~");
        assert_eq!(shown_path("/home/adam/x", home), "/home/adam/x");
    }
}
