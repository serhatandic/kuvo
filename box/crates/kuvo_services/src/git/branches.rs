//! Branch menus and the explicit branch and worktree actions.
use super::changes::{self, asked, branch, read, root_of};
use http::StatusCode;
use kuvo_server::Refusal;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub(super) struct Tree {
    pub root: PathBuf,
    pub project: PathBuf,
    pub base: Option<String>,
}

fn recorded(home: &Path, directory: &Path) -> Result<Option<Tree>, Refusal> {
    let kept = match super::projects::WORKTREE_RECORDS.read(&home.join("box/worktrees.json")) {
        Ok(record) => record.map(|record| record.fields).unwrap_or_default(),
        Err(error @ kuvo_store::format::Error::Unsupported { .. }) => {
            return Err(super::unsupported(error));
        }
        Err(_) => Default::default(),
    };
    let directory = crate::files::real(directory).unwrap_or_else(|| directory.to_owned());
    Ok(kept
        .get("worktrees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value::<Tree>(value.clone()).ok())
        .find(|tree| {
            directory
                .starts_with(crate::files::real(&tree.root).unwrap_or_else(|| tree.root.clone()))
        }))
}
async fn merged_from(home: &Path, directory: &Path) -> Result<Option<Tree>, Refusal> {
    if let Some(tree) = recorded(home, directory)? {
        return Ok(Some(tree));
    }
    let Some(found) = read(
        directory,
        &["rev-parse", "--absolute-git-dir", "--git-common-dir"],
    )
    .await
    else {
        return Ok(None);
    };
    Ok((|| {
        let (own, common) = found.trim().split_once('\n')?;
        let shared = directory.join(common);
        let shared = crate::files::real(&shared).unwrap_or(shared);
        if crate::files::real(Path::new(own)).as_ref() == Some(&shared)
            || shared.file_name()?.to_str()? != ".git"
        {
            return None;
        }
        Some(Tree {
            root: directory.to_owned(),
            project: shared.parent()?.to_owned(),
            base: None,
        })
    })())
}
pub(super) async fn origin(root: &Path) -> Option<String> {
    read(root, &["remote", "get-url", "origin"])
        .await
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
pub(super) async fn base(
    home: &Path,
    directory: &Path,
    root: &Path,
) -> Result<Option<String>, Refusal> {
    if let Some(base) = recorded(home, directory)?.and_then(|t| t.base) {
        return Ok(Some(base));
    }
    if let Some(main) = read(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .await
    {
        let main = main.trim();
        if !main.is_empty() {
            return Ok(Some(main.strip_prefix("origin/").unwrap_or(main).into()));
        }
    }
    for name in ["main", "master"] {
        if exists(root, name).await || exists(root, &format!("origin/{name}")).await {
            return Ok(Some(name.into()));
        }
    }
    Ok(None)
}
pub(super) async fn exists(root: &Path, name: &str) -> bool {
    read(root, &["rev-parse", "--verify", "-q", name])
        .await
        .is_some()
}
pub(super) fn is_base(branch: &str, base: Option<&str>) -> bool {
    base.map_or(matches!(branch, "main" | "master"), |base| branch == base)
}
pub(super) async fn branches(home: &Path, directory: &Path) -> Result<Value, Refusal> {
    let root = root_of(directory).await;
    let at = root.as_deref().unwrap_or(directory);
    if root.is_none() && read(directory, &["rev-parse", "--git-dir"]).await.is_none() {
        return Ok(json!({"repository":false}));
    }
    let listed = read(
        at,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            "refs/heads",
        ],
    )
    .await
    .ok_or_else(|| changes::no(StatusCode::INTERNAL_SERVER_ERROR, "git couldn’t say."))?;
    let head = read(at, &["symbolic-ref", "--quiet", "HEAD"])
        .await
        .unwrap_or_default();
    let current = head.trim().strip_prefix("refs/heads/");
    let branches: Vec<_> = listed
        .lines()
        .filter_map(|line| line.strip_prefix("refs/heads/"))
        .collect();
    let mut pull = false;
    if let (Some(root), Some(branch)) = (&root, current) {
        pull = origin(root).await.is_some()
            && !is_base(branch, base(home, root, root).await?.as_deref());
    }
    let mut answer = json!({"repository":true,"root":root,"current":current,"branches":branches,"pullRequest":pull});
    if let Some(root) = root
        && let Some(tree) = merged_from(home, &root).await?
        && tree.project.exists()
    {
        let into = match tree.base {
            Some(base) => Some(base),
            None => branch(&tree.project).await,
        };
        answer["worktree"] = json!({"into":into});
    }
    Ok(answer)
}
pub(super) async fn switch(directory: &Path, body: &Value) -> Result<Value, Refusal> {
    let name = body["branch"].as_str().unwrap_or("");
    let bad = |msg| changes::no(StatusCode::BAD_REQUEST, msg);
    if name.trim().is_empty() {
        return Err(bad("Name the branch."));
    }
    if name.starts_with('-') {
        return Err(bad("A branch’s name can’t start with “-”."));
    }
    if name.contains(['\0', '\n', '\r']) {
        return Err(bad("A branch’s name is one line."));
    }
    let failed = |msg| changes::no(StatusCode::UNPROCESSABLE_ENTITY, msg);
    let root = match root_of(directory).await {
        Some(root) => root,
        None if read(directory, &["rev-parse", "--git-dir"]).await.is_some() => {
            return Err(failed(
                "This folder has no checkout of its own to switch. Switch branches in one of its worktrees.",
            ));
        }
        None => return Err(failed("Not a git repository.")),
    };
    let args = if body["create"] == true {
        vec!["switch", "-c", name]
    } else {
        vec!["switch", name]
    };
    asked(&root, &args, &[])
        .await
        .map_err(|error| changes::no(StatusCode::UNPROCESSABLE_ENTITY, error))?;
    Ok(json!({}))
}
pub(super) async fn unready(directory: &Path, branch: &str, target: &str) -> Option<String> {
    if read(
        directory,
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .await
    .is_none_or(|status| !status.trim().is_empty())
    {
        return Some("Commit the changes first. Only what’s committed goes.".into());
    }
    match read(
        directory,
        &["rev-list", "--count", &format!("{target}..HEAD")],
    )
    .await
    {
        None => Some(format!("There’s no branch {target} to compare with.")),
        Some(ahead) if ahead.trim() == "0" => {
            Some(format!("{branch} has nothing {target} doesn’t have yet."))
        }
        _ => None,
    }
}
pub(super) async fn merge(home: &Path, directory: &Path, body: &Value) -> Result<Value, Refusal> {
    let given = if body["into"].is_null() {
        None
    } else {
        Some(
            body["into"]
                .as_str()
                .filter(|s| !s.is_empty() && !s.starts_with('-'))
                .ok_or_else(|| {
                    changes::no(StatusCode::BAD_REQUEST, "Name the branch to merge into.")
                })?,
        )
    };
    let conflict = |s: String| changes::no(StatusCode::CONFLICT, s);
    let tree = merged_from(home, directory)
        .await?
        .filter(|t| t.project.exists())
        .ok_or_else(|| {
            conflict(
                "Only a worktree’s branch is merged here. Create a pull request instead.".into(),
            )
        })?;
    let from = branch(directory)
        .await
        .ok_or_else(|| conflict("This folder isn’t on a branch.".into()))?;
    let target = tree.project;
    let checked_out = branch(&target)
        .await
        .ok_or_else(|| conflict("The project’s folder isn’t on a branch.".into()))?;
    let into = given.or(tree.base.as_deref()).unwrap_or(&checked_out);
    if into != checked_out {
        return Err(conflict(format!(
            "The project’s folder is on {checked_out} now. Switch it back to {into} to merge, or create a pull request."
        )));
    }
    if exists(&target, "MERGE_HEAD").await {
        return Err(conflict(
            "The project’s folder is in the middle of a merge. Finish it there first.".into(),
        ));
    }
    if let Some(why) = unready(directory, &from, into).await {
        return Err(conflict(why));
    }
    let env = changes::author(body);
    let failure = match asked(&target, &["merge", "--no-edit", "--", &from], &env).await {
        Ok(()) => return Ok(json!({"into":into})),
        Err(failed) => failed,
    };
    let names = read(
        &target,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "--name-only",
            "-z",
            "--diff-filter=U",
        ],
    )
    .await
    .unwrap_or_default();
    if exists(&target, "MERGE_HEAD").await {
        let _ = asked(&target, &["merge", "--abort"], &env).await;
    }
    let conflicts: Vec<_> = names.split('\0').filter(|s| !s.is_empty()).collect();
    if conflicts.is_empty() {
        return Err(changes::no(StatusCode::UNPROCESSABLE_ENTITY, failure));
    }
    let mut named = conflicts
        .iter()
        .take(5)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if conflicts.len() > 5 {
        named.push_str(&format!(" and {} more", conflicts.len() - 5));
    }
    Err(Refusal::json(
        StatusCode::CONFLICT,
        json!({"message":format!("{into} changed some of the same lines: {named}. Nothing was merged."),"conflicts":conflicts,"into":into}),
    ))
}
