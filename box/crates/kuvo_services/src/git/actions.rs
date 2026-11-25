//! Stage, unstage and discard the file the client named, never a path pattern.
use super::{
    changes::{self, Scope, asked, read},
    list::{self, Status},
};
use http::StatusCode;
use kuvo_server::Refusal;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Stage,
    Unstage,
    Discard,
}

struct File<'a> {
    root: PathBuf,
    name: &'a str,
    old: Option<&'a str>,
    status: Status,
    scope: Scope,
}

impl File<'_> {
    async fn run(&self, args: &[&str]) -> Result<(), String> {
        let mut literal = vec!["--literal-pathspecs"];
        literal.extend(args);
        asked(&self.root, &literal, &[])
            .await
            .map_err(|_| format!("git couldn’t {} {}.", args[0], self.name))
    }
    async fn indexed(&self, name: &str) -> bool {
        read(
            &self.root,
            &[
                "--literal-pathspecs",
                "ls-files",
                "--error-unmatch",
                "--",
                name,
            ],
        )
        .await
        .is_some()
    }
    async fn untrack(&self, name: &str) -> Result<(), String> {
        if self.indexed(name).await {
            self.run(&["rm", "--cached", "-f", "-q", "--", name])
                .await?;
        }
        Ok(())
    }
    fn blocking(&self, name: &str) -> Option<PathBuf> {
        let mut at = self.root.clone();
        let parent = Path::new(name).parent()?;
        for part in parent.components() {
            at.push(part.as_os_str());
            let stat = fs::symlink_metadata(&at).ok()?;
            if !stat.is_dir() {
                return Some(at);
            }
        }
        None
    }
    async fn trash(&self, home: &Path, target: &Path) -> Result<(), String> {
        let target = target.to_owned();
        let shown = target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let home = home.to_owned();
        tokio::task::spawn_blocking(move || kuvo_os::trash::move_entry(&home, &target))
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| format!("Couldn’t move {shown} out of the way."))
    }
    async fn gone(&self, home: &Path, name: &str) -> Result<(), String> {
        if self.blocking(name).is_some() {
            return Err(format!("Couldn’t move {name} out of the way."));
        }
        let target = self.root.join(name);
        if fs::symlink_metadata(&target).is_ok() {
            self.trash(home, &target).await?;
        }
        Ok(())
    }
    async fn same_as_committed(&self, name: &str) -> bool {
        let committed = read(
            &self.root,
            &["--literal-pathspecs", "ls-tree", "HEAD", "--", name],
        )
        .await;
        let committed = committed
            .as_deref()
            .and_then(|s| s.split('\t').next())
            .and_then(|s| s.split(' ').nth(2));
        let disk = read(&self.root, &["hash-object", "--no-filters", "--", name]).await;
        committed.is_some() && committed == disk.as_deref().map(str::trim)
    }
    async fn clear(&self, home: &Path, name: &str) -> Result<(), String> {
        if let Some(blocking) = self.blocking(name) {
            return self.trash(home, &blocking).await;
        }
        let target = self.root.join(name);
        if !fs::symlink_metadata(&target).is_ok_and(|s| s.is_dir()) {
            return Ok(());
        }
        let command = if self.scope == Scope::Unstaged {
            ["ls-files", "-s"]
        } else {
            ["ls-tree", "HEAD"]
        };
        let kept = read(
            &self.root,
            &["--literal-pathspecs", command[0], command[1], "--", name],
        )
        .await
        .unwrap_or_default();
        if kept.lines().any(|line| {
            line.split_once('\t').is_some_and(|(entry, path)| {
                path == name && (entry.starts_with("100") || entry.starts_with("120"))
            })
        }) {
            self.trash(home, &target).await?;
        }
        Ok(())
    }
    async fn restore(&self, names: &[&str]) -> Result<(), String> {
        let mut args = if self.scope == Scope::Unstaged {
            vec!["restore", "--worktree", "--"]
        } else {
            vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"]
        };
        args.extend(names);
        self.run(&args).await
    }
    async fn perform(&self, action: Action, home: &Path) -> Result<(), String> {
        let paths = [Some(self.name), self.old]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if action == Action::Stage {
            let mut args = vec!["add", "-A", "--", self.name];
            if let Some(old) = self.old
                && self.indexed(old).await
            {
                args.push(old);
            }
            return self.run(&args).await;
        }
        if action == Action::Unstage {
            let mut args = vec!["restore", "--staged", "--"];
            args.extend(&paths);
            if self.run(&args).await.is_ok() {
                return Ok(());
            }
            if read(&self.root, &["rev-parse", "--git-dir"])
                .await
                .is_none()
                || read(&self.root, &["rev-parse", "--verify", "-q", "HEAD"])
                    .await
                    .is_some()
            {
                return Err(format!("git couldn’t restore {}.", self.name));
            }
            let mut args = vec!["rm", "--cached", "-f", "-q", "--"];
            args.extend(paths);
            return self.run(&args).await;
        }
        if self.status == Status::Added {
            self.untrack(self.name).await?;
            return self.gone(home, self.name).await;
        }
        if self.status == Status::Renamed
            && let Some(old) = self.old
        {
            let written = self.root.join(old);
            if self.blocking(old).is_none()
                && fs::symlink_metadata(&written).is_ok_and(|s| !s.is_dir())
                && !self.same_as_committed(old).await
            {
                self.trash(home, &written).await?;
            }
            self.untrack(self.name).await?;
            self.clear(home, old).await?;
            self.restore(&[old]).await?;
            return self.gone(home, self.name).await;
        }
        let target = self.root.join(self.name);
        if self.status == Status::Modified
            && read(
                &self.root,
                &["--literal-pathspecs", "ls-files", "-s", "--", self.name],
            )
            .await
            .is_some_and(|s| s.starts_with("160000"))
            && read(&target, &["status", "--porcelain"])
                .await
                .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(format!(
                "{} is a submodule: its changes are in its own repository, so discard them there.",
                self.name
            ));
        }
        if self.status == Status::Deleted
            && self.scope != Scope::Unstaged
            && self.blocking(self.name).is_none()
            && fs::symlink_metadata(&target).is_ok_and(|s| s.is_file())
            && !self.same_as_committed(self.name).await
        {
            self.trash(home, &target).await?;
        }
        let names = if self.scope == Scope::Unstaged {
            paths
        } else {
            vec![self.name]
        };
        for name in &names {
            self.clear(home, name).await?;
        }
        self.restore(&names).await
    }
}

pub(super) async fn file_action(
    action: Action,
    home: &Path,
    body: &Value,
) -> Result<Value, Refusal> {
    let invalid = || {
        changes::no(
            StatusCode::BAD_REQUEST,
            "Name a repository and a file in it.",
        )
    };
    let scope = changes::scope(body)?;
    let root = changes::folder(body["root"].as_str().unwrap_or("")).map_err(|_| invalid())?;
    let name = body["path"]
        .as_str()
        .and_then(changes::name)
        .ok_or_else(invalid)?;
    let old = if body["oldPath"].is_null() {
        None
    } else {
        Some(
            body["oldPath"]
                .as_str()
                .and_then(changes::name)
                .ok_or_else(invalid)?,
        )
    };
    let status = serde_json::from_value(
        body.get("status")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or(json!("modified")),
    )
    .map_err(|_| invalid())?;
    let file = File {
        root,
        name,
        old,
        status,
        scope,
    };
    if action == Action::Discard {
        let was = body["stamp"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| changes::no(StatusCode::CONFLICT,format!("{name} wasn’t discarded: this app didn’t say what the file was when it listed it, so changes made since could be lost. Update the app, then try again.")))?;
        // A discard acts on the repository it was listed in, never on an enclosing repository.
        if changes::root_of(&file.root)
            .await
            .as_ref()
            .and_then(|p| crate::files::real(p))
            != crate::files::real(&file.root)
        {
            return Err(changes::no(
                StatusCode::CONFLICT,
                format!(
                    "{name} changed since it was listed. Look at it again before you discard it."
                ),
            ));
        }
        if let Some(failed) = list::stale(&file.root, name, old, status, was).await {
            return Err(changes::no(StatusCode::CONFLICT, failed));
        }
    }
    file.perform(action, home)
        .await
        .map_err(|failed| changes::no(StatusCode::UNPROCESSABLE_ENTITY, failed))?;
    Ok(json!({}))
}
