//! The status rows, including what a file looked like when it was listed.
use super::changes::{self, Comparison, Scope, modified_ns, read};
use http::StatusCode;
use kuvo_server::Refusal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Status {
    Added,
    Deleted,
    Modified,
    Renamed,
}
impl Status {
    fn from_letter(letter: char) -> Self {
        match letter {
            'A' | 'C' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            _ => Self::Modified,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
enum Group {
    Conflicted,
    Staged,
    Unstaged,
    Untracked,
}

struct Entry<'a> {
    path: &'a str,
    old: Option<&'a str>,
    index: Option<&'a str>,
    x: char,
    y: char,
    kind: char,
}
fn entries(status: &str) -> Vec<Entry<'_>> {
    let mut records = status.split('\0');
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        let Some(kind) = record.chars().next() else {
            continue;
        };
        if kind == '?' {
            entries.push(Entry {
                path: &record[2..],
                old: None,
                index: None,
                x: '.',
                y: '?',
                kind,
            });
            continue;
        }
        let count = match kind {
            '1' => 9,
            '2' => 10,
            'u' => 11,
            _ => continue,
        };
        let fields: Vec<_> = record.splitn(count, ' ').collect();
        if fields.len() != count {
            continue;
        }
        let mut xy = fields[1].chars();
        let index = (kind != 'u' && fields[7].chars().any(|c| c != '0')).then_some(fields[7]);
        entries.push(Entry {
            path: fields[count - 1],
            old: if kind == '2' { records.next() } else { None },
            index,
            x: xy.next().unwrap_or('.'),
            y: xy.next().unwrap_or('.'),
            kind,
        });
    }
    entries
}
fn disk_stamp(path: &Path) -> (String, u128) {
    fs::symlink_metadata(path)
        .ok()
        .map(|s| {
            let time = modified_ns(&s).unwrap_or(0);
            (format!("{}-{time}", s.len()), time)
        })
        .unwrap_or_else(|| ("gone".into(), 0))
}
async fn inside(folder: &Path) -> Option<(String, u128)> {
    let root = changes::root_of(folder).await?;
    if crate::files::real(&root) != crate::files::real(folder) {
        return None;
    }
    let status = read(
        folder,
        &[
            "-c",
            "core.quotepath=off",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
    )
    .await?;
    let head = read(folder, &["rev-parse", "--verify", "-q", "HEAD"])
        .await
        .unwrap_or_else(|| "none".into());
    let mut hash = Sha256::new();
    hash.update(&status);
    let mut newest = 0;
    for record in status.split('\0') {
        let Some(name) = record.get(3..).filter(|s| !s.is_empty()) else {
            continue;
        };
        let (stamp, time) = disk_stamp(&folder.join(name));
        newest = newest.max(time);
        hash.update(format!("\0{name}\0{stamp}"));
    }
    Some((
        format!("{}-{}", head.trim(), hex::encode(&hash.finalize()[..16])),
        newest,
    ))
}
async fn stamp(root: &Path, name: &str, index: Option<&str>, since: Option<u128>) -> String {
    let file = root.join(name.trim_end_matches('/'));
    let (value, time) = if fs::symlink_metadata(&file).is_ok_and(|stat| stat.is_dir())
        && fs::symlink_metadata(file.join(".git")).is_ok()
    {
        match inside(&file).await {
            Some((stamp, time)) => (format!("repository-{stamp}"), time),
            None => {
                let mut random = [0; 16];
                let _ = getrandom::fill(&mut random);
                (format!("repository-unread-{}", hex::encode(random)), 0)
            }
        }
    } else {
        disk_stamp(&file)
    };
    format!(
        "{}:{value}{}",
        index.unwrap_or("-"),
        if since.is_some_and(|s| time >= s) {
            ":since"
        } else {
            ""
        }
    )
}

pub(super) async fn stale(
    root: &Path,
    name: &str,
    old: Option<&str>,
    status: Status,
    was: &str,
) -> Option<String> {
    if status == Status::Added
        && read(
            root,
            &[
                "--literal-pathspecs",
                "ls-tree",
                "--name-only",
                "HEAD",
                "--",
                name,
            ],
        )
        .await
        .is_some_and(|s| !s.trim().is_empty())
        && read(
            root,
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
    {
        return Some(format!(
            "{name} is in the last commit now, so it isn’t a new file any more. Look at it again before you discard it."
        ));
    }
    let status = read(
        root,
        &[
            "--literal-pathspecs",
            "-c",
            "core.quotepath=off",
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--",
            name,
        ],
    )
    .await
    .unwrap_or_default();
    let index = entries(&status)
        .into_iter()
        .find(|e| e.path == name)
        .and_then(|e| e.index);
    let mut now = stamp(root, name, index, None).await;
    if let Some(old) = old {
        now.push('|');
        now.push_str(&stamp(root, old, None, None).await);
    }
    if now.contains(":repository-unread-") {
        return Some(format!(
            "git couldn’t read the repository inside {name}, so it wasn’t discarded."
        ));
    }
    (now != was).then(|| {
        format!("{name} changed since it was listed. Look at it again before you discard it.")
    })
}

async fn lines(root: &Path, compared: &[&str], limit: &str) -> BTreeMap<String, (u64, u64)> {
    let mut args = vec![
        "-c",
        "core.quotepath=off",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--numstat",
        "-z",
    ];
    args.extend(compared);
    args.extend(["--", limit]);
    let out = read(root, &args).await.unwrap_or_default();
    let mut records = out.split('\0');
    let mut lines = BTreeMap::new();
    while let Some(record) = records.next() {
        let fields: Vec<_> = record.splitn(3, '\t').collect();
        if fields.len() != 3 {
            continue;
        }
        let name = if fields[2].is_empty() {
            records.next();
            records.next().unwrap_or("")
        } else {
            fields[2]
        };
        if let (Ok(add), Ok(del)) = (fields[0].parse(), fields[1].parse()) {
            lines.insert(name.to_owned(), (add, del));
        }
    }
    lines
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row<'a> {
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<&'a str>,
    group: Group,
    status: Status,
    scope: &'static str,
    stage: &'static str,
    #[serde(flatten)]
    detail: BTreeMap<&'static str, Value>,
}
async fn row<'a>(
    root: &Path,
    entry: &Entry<'a>,
    group: Group,
    status: Status,
    old: Option<&'a str>,
    changed: bool,
    since: u128,
    counts: &BTreeMap<String, (u64, u64)>,
) -> Row<'a> {
    let path = entry.path;
    let mut detail = BTreeMap::new();
    let (scope, stage) = match group {
        Group::Staged => ("staged", "unstage"),
        Group::Conflicted => ("uncommitted", "resolve"),
        _ => ("unstaged", "stage"),
    };
    if let Some((add, del)) = counts.get(path) {
        detail.insert("additions", json!(add));
        detail.insert("deletions", json!(del));
    }
    if group != Group::Conflicted {
        let discard = if status == Status::Added {
            "trash"
        } else if scope == "unstaged" {
            "unstaged"
        } else {
            "head"
        };
        let mut stamped = stamp(root, path, entry.index, Some(since)).await;
        if let Some(old) = old {
            stamped.push('|');
            stamped.push_str(&stamp(root, old, None, Some(since)).await);
        }
        let kind = match status {
            Status::Added => "trash",
            Status::Deleted => "restore",
            Status::Renamed if old.is_some() => "undoMove",
            _ => "changes",
        };
        let loses = matches!(kind, "trash" | "undoMove")
            && (changed || fs::symlink_metadata(root.join(path.trim_end_matches('/'))).is_err());
        let discards = if loses {
            json!({"kind":kind,"undoable":false,"loses":"What you staged of it isn’t kept anywhere, so this can’t be undone."})
        } else {
            json!({"kind":kind,"undoable":kind != "changes"})
        };
        detail.insert("discard", json!(discard));
        detail.insert("stamp", json!(stamped));
        detail.insert("discards", discards);
    }
    if path.ends_with('/') {
        detail.insert("repository", json!(true));
    }
    Row {
        path,
        old_path: old,
        group,
        status,
        scope,
        stage,
        detail,
    }
}

pub(super) async fn files(directory: &Path) -> Result<Value, Refusal> {
    let Some(c) = Comparison::new(directory, Scope::Uncommitted, None).await else {
        return Ok(json!({"repository":false}));
    };
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let status = read(
        &c.root,
        &[
            "-c",
            "core.quotepath=off",
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--find-renames",
            "--",
            &c.limit,
        ],
    )
    .await
    .ok_or_else(|| {
        changes::no(
            StatusCode::INTERNAL_SERVER_ERROR,
            "git couldn’t read the changes.",
        )
    })?;
    let staged_args = ["--cached", &c.against];
    let (staged, unstaged) = tokio::join!(
        lines(&c.root, &staged_args, &c.limit),
        lines(&c.root, &[], &c.limit)
    );
    let none = BTreeMap::new();
    let mut rows = Vec::new();
    let mut new = 0u64;
    for entry in entries(&status) {
        match entry.kind {
            '?' => {
                new += 1;
                if new <= 1000 {
                    rows.push(
                        row(
                            &c.root,
                            &entry,
                            Group::Untracked,
                            Status::Added,
                            None,
                            false,
                            since,
                            &none,
                        )
                        .await,
                    );
                }
            }
            'u' => rows.push(
                row(
                    &c.root,
                    &entry,
                    Group::Conflicted,
                    Status::Modified,
                    None,
                    false,
                    since,
                    &none,
                )
                .await,
            ),
            _ => {
                if entry.x != '.' {
                    rows.push(
                        row(
                            &c.root,
                            &entry,
                            Group::Staged,
                            Status::from_letter(entry.x),
                            entry.old.filter(|_| entry.x == 'R'),
                            entry.y != '.',
                            since,
                            &staged,
                        )
                        .await,
                    );
                }
                if entry.y != '.' {
                    rows.push(
                        row(
                            &c.root,
                            &entry,
                            Group::Unstaged,
                            Status::from_letter(entry.y),
                            None,
                            false,
                            since,
                            &unstaged,
                        )
                        .await,
                    );
                }
            }
        }
    }
    let order = crate::files::order::FinderOrder::new();
    rows.sort_by(|a, b| {
        a.group
            .cmp(&b.group)
            .then_with(|| order.compare(a.path, b.path))
    });
    let more = new.saturating_sub(1000);
    let has = changes::has_staged(&c.root).await;
    let names = read(
        &c.root,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            &c.against,
        ],
    )
    .await
    .unwrap_or_default();
    let elsewhere = names
        .split('\0')
        .filter(|n| {
            !n.is_empty()
                && c.limit != "."
                && *n != c.limit
                && !n.strip_prefix(&c.limit).is_some_and(|s| s.starts_with('/'))
        })
        .count();
    let takes = if has {
        read(
            &c.root,
            &[
                "-c",
                "core.quotepath=off",
                "diff",
                "--cached",
                "--name-only",
                "-z",
                "--find-renames",
                "--no-ext-diff",
                &c.against,
            ],
        )
        .await
        .unwrap_or_default()
        .split('\0')
        .filter(|s| !s.is_empty())
        .count()
    } else {
        rows.iter()
            .map(|row| row.path)
            .collect::<HashSet<_>>()
            .len()
            + more as usize
    };
    let mut answer = json!({"repository":true,"root":c.root,"branch":changes::branch(&c.root).await,"files":rows,"uncommitted":!rows.is_empty() || more > 0,"commits":if has {"staged"} else {"all"},"stagedElsewhere":elsewhere,"takes":takes});
    if more > 0 {
        answer["more"] = json!(more);
    }
    if let Some(dir) = read(&c.root, &["rev-parse", "--absolute-git-dir"]).await {
        let dir = Path::new(dir.trim());
        for (markers, state) in [
            (&["rebase-merge", "rebase-apply"][..], "rebasing"),
            (&["MERGE_HEAD"][..], "merging"),
            (&["CHERRY_PICK_HEAD"][..], "cherry-picking"),
            (&["REVERT_HEAD"][..], "reverting"),
        ] {
            if markers.iter().any(|marker| dir.join(marker).exists()) {
                answer["underway"] = json!(state);
                break;
            }
        }
    }
    Ok(answer)
}
