//! Repository reads and the explicit actions in the Changes panel.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use http::StatusCode;
use kuvo_server::Refusal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    projects,
    safe::{self, Intent},
};

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const ONE_FILE_MOST: u64 = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(600);

pub(super) fn no(status: StatusCode, message: impl Into<String>) -> Refusal {
    Refusal::new(status, message)
}

pub(super) fn folder(value: &str) -> Result<PathBuf, Refusal> {
    let path = PathBuf::from(value);
    if !projects::absolute(value) || !path.is_dir() {
        return Err(no(StatusCode::NOT_FOUND, "That folder isn’t there."));
    }
    Ok(path)
}

pub(super) fn name(value: &str) -> Option<&str> {
    let path = value.strip_suffix('/').unwrap_or(value);
    (!projects::absolute(path)
        && !path.contains('\0')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
        && Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_))))
    .then_some(path)
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Scope {
    #[default]
    Uncommitted,
    Unstaged,
    Staged,
    Branch,
}

pub(super) fn scope(body: &Value) -> Result<Scope, Refusal> {
    serde_json::from_value(
        body.get("scope")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or(json!("uncommitted")),
    )
    .map_err(|_| no(StatusCode::BAD_REQUEST, "That isn’t a scope."))
}

pub(super) async fn read(root: &Path, args: &[&str]) -> Option<String> {
    let out = safe::run(root, args, Intent::Own, &[], TIMEOUT)
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(super) async fn asked(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<(), String> {
    let intent = if args.first() == Some(&"--literal-pathspecs") {
        Intent::Asked
    } else {
        Intent::Yours
    };
    let out = safe::run(root, args, intent, env, TIMEOUT).await?;
    if out.status.success() {
        return Ok(());
    }
    let words = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if words.trim().is_empty() {
        Err(format!("git {} failed.", args[0]))
    } else {
        Err(words.trim().to_owned())
    }
}

pub(super) async fn root_of(directory: &Path) -> Option<PathBuf> {
    read(directory, &["rev-parse", "--show-toplevel"])
        .await
        .map(|root| PathBuf::from(root.trim()))
        .filter(|root| !root.as_os_str().is_empty())
}

pub(super) async fn branch(root: &Path) -> Option<String> {
    read(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .map(|head| head.trim().to_owned())
        .filter(|head| !head.is_empty())
}

pub(super) async fn has_staged(root: &Path) -> bool {
    read(root, &["diff", "--cached", "--quiet"]).await.is_none()
}

pub(super) fn relative(root: &Path, folder: &Path) -> String {
    let root = crate::files::real(root).unwrap_or_else(|| root.to_owned());
    let folder = crate::files::real(folder).unwrap_or_else(|| folder.to_owned());
    folder
        .strip_prefix(root)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| {
            p.components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|| ".".into())
}

pub(super) struct Comparison {
    pub(super) root: PathBuf,
    pub(super) against: String,
    pub(super) limit: String,
    scope: Scope,
}
impl Comparison {
    pub(super) async fn new(directory: &Path, scope: Scope, base: Option<&str>) -> Option<Self> {
        let root = root_of(directory).await?;
        let limit = relative(&root, directory);
        let mut against = if read(&root, &["rev-parse", "--verify", "-q", "HEAD"])
            .await
            .is_some()
        {
            "HEAD"
        } else {
            EMPTY_TREE
        }
        .to_owned();
        if scope == Scope::Branch
            && let Some(base) = base.filter(|base| !base.is_empty() && !base.starts_with('-'))
        {
            if let Some(fork) = read(&root, &["merge-base", "HEAD", base]).await {
                against = fork.trim().to_owned();
            }
        }
        Some(Self {
            root,
            against,
            limit,
            scope,
        })
    }
    fn args(&self) -> Vec<&str> {
        match self.scope {
            Scope::Staged => vec!["--cached", &self.against],
            Scope::Unstaged => vec![],
            _ => vec![&self.against],
        }
    }
}

pub(super) fn modified_ns(stat: &fs::Metadata) -> Option<u128> {
    stat.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| time.as_nanos())
}

#[derive(Serialize)]
struct Diff {
    repository: bool,
    root: PathBuf,
    against: String,
    patch: String,
    untracked: Vec<String>,
    hashes: BTreeMap<String, String>,
    uncommitted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    big: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    binary: Option<bool>,
}

pub(super) async fn diff(
    directory: &Path,
    scope: Scope,
    base: Option<&str>,
    only: Option<&str>,
    old: Option<&str>,
) -> Result<Value, Refusal> {
    let Some(c) = Comparison::new(directory, scope, base).await else {
        return Ok(json!({"repository": false}));
    };
    let args = [
        "-c",
        "core.quotepath=off",
        "-c",
        "diff.suppressBlankEmpty=false",
        "-c",
        "diff.submodule=short",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "-U100000",
    ];
    let failed = || {
        no(
            StatusCode::INTERNAL_SERVER_ERROR,
            "git couldn’t read the changes.",
        )
    };
    let mut result = Diff {
        repository: true,
        root: c.root.clone(),
        against: c.against.clone(),
        patch: String::new(),
        untracked: vec![],
        hashes: BTreeMap::new(),
        uncommitted: true,
        big: None,
        binary: None,
    };
    if let Some(file) = only {
        let known = read(
            &c.root,
            &[
                "--literal-pathspecs",
                "ls-files",
                "--error-unmatch",
                "--",
                file,
            ],
        )
        .await
        .is_some();
        let disk = fs::metadata(c.root.join(file)).ok();
        if !known && scope != Scope::Staged && !file.ends_with('/') {
            if let Some(stat) = disk.filter(fs::Metadata::is_file) {
                if stat.len() > ONE_FILE_MOST {
                    result.big = Some(true);
                } else {
                    let mut given = args.to_vec();
                    given.extend(["--no-index", "--", "/dev/null", file]);
                    let out = safe::run(&c.root, &given, Intent::Own, &[], TIMEOUT)
                        .await
                        .map_err(|_| failed())?;
                    result.patch = String::from_utf8_lossy(&out.stdout).into_owned();
                }
            }
        } else {
            let before = old.unwrap_or(file);
            let committed = format!("{}:{before}", c.against);
            let indexed = format!(":{before}");
            let staged = format!(":{file}");
            let sides = match scope {
                Scope::Staged => vec![committed.as_str(), staged.as_str()],
                Scope::Unstaged => vec![indexed.as_str()],
                _ => vec![committed.as_str()],
            };
            let mut big = scope != Scope::Staged && disk.is_some_and(|s| s.len() > ONE_FILE_MOST);
            for side in sides {
                big |= read(&c.root, &["cat-file", "-s", side])
                    .await
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .is_some_and(|n| n > ONE_FILE_MOST);
            }
            if big {
                result.big = Some(true);
            } else {
                let mut given = vec!["--literal-pathspecs"];
                given.extend(args);
                given.extend(c.args());
                given.extend(["--", file]);
                given.extend(old);
                result.patch = read(&c.root, &given).await.ok_or_else(failed)?;
            }
        }
        if result.patch.len() as u64 > ONE_FILE_MOST {
            result.big = Some(true);
            result.patch.clear();
        }
        if result
            .patch
            .split("\n@@ ")
            .next()
            .unwrap_or("")
            .lines()
            .any(|line| line.starts_with("Binary files ") && line.ends_with(" differ"))
        {
            result.binary = Some(true);
        }
    } else {
        let mut given = args.to_vec();
        given.extend(c.args());
        given.extend(["--", &c.limit]);
        result.patch = read(&c.root, &given).await.ok_or_else(failed)?;
        if scope != Scope::Staged {
            let listed = read(
                &c.root,
                &[
                    "-c",
                    "core.quotepath=off",
                    "ls-files",
                    "--others",
                    "--exclude-standard",
                    "-z",
                    "--",
                    &c.limit,
                ],
            )
            .await
            .unwrap_or_default();
            result.untracked = listed
                .split('\0')
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for file in &result.untracked {
                if !file.ends_with('/')
                    && let Ok(stat) = fs::metadata(c.root.join(file))
                    && let Some(ns) = modified_ns(&stat)
                    && now.saturating_sub(ns) > 3_000_000_000
                {
                    result
                        .hashes
                        .insert(file.clone(), format!("{}-{ns}", stat.len()));
                }
            }
        }
        if scope == Scope::Branch {
            result.uncommitted = read(
                &c.root,
                &["status", "--porcelain", "--untracked-files=normal"],
            )
            .await
            .is_none_or(|s| !s.is_empty());
        }
    }
    Ok(json!(result))
}

pub(super) async fn push(root: &Path, env: &[(&str, &str)]) -> Result<(), String> {
    match asked(root, &["push"], env).await {
        Err(error) if error.contains("no upstream branch") => {
            asked(root, &["push", "-u", "origin", "HEAD"], env).await
        }
        result => result,
    }
}

pub(super) async fn commit(directory: &Path, body: &Value) -> Result<Value, Refusal> {
    let text = body["message"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| no(StatusCode::BAD_REQUEST, "Write a commit message."))?;
    let root = root_of(directory)
        .await
        .ok_or_else(|| no(StatusCode::UNPROCESSABLE_ENTITY, "Not a git repository."))?;
    let env = author(body);
    let error = |s| no(StatusCode::UNPROCESSABLE_ENTITY, s);
    if !has_staged(&root).await {
        asked(
            &root,
            &["add", "-A", "--", &relative(&root, directory)],
            &env,
        )
        .await
        .map_err(error)?;
    }
    asked(&root, &["commit", "-m", text], &env)
        .await
        .map_err(error)?;
    if body["push"] == true
        && let Err(failed) = push(&root, &env).await
    {
        return Err(Refusal::json(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"message": format!("Committed, but the push didn’t go through:\n{failed}"), "committed":true,"pushed":false}),
        ));
    }
    Ok(json!({}))
}

pub(super) fn author(body: &Value) -> Vec<(&str, &str)> {
    [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
    ]
    .into_iter()
    .filter_map(|key| body["author"][key].as_str().map(|value| (key, value)))
    .collect()
}
