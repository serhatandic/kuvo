//! Describe the effect of removing sessions before a client proceeds.

use std::{collections::BTreeSet, path::Path};

use http::StatusCode;
use kuvo_server::Refusal;
use serde::Serialize;

use super::Session;
use crate::{
    Engine,
    lifetime::{Demand, Lease},
    session_list::{Observed, agent_rows, read_inventory},
    worktree_notes::{self, Location, Records},
};

const UNDO: &str = "This can’t be undone.";

#[derive(Serialize)]
pub(super) struct Confirmation {
    title: String,
    message: String,
    label: &'static str,
}

#[derive(Default)]
struct Worktrees {
    live: usize,
    uncommitted: bool,
    kept: usize,
}

fn title(session: &Session) -> String {
    let name = if session.title.is_empty() {
        "New Session"
    } else {
        &session.title
    };
    format!("Delete “{name}”?")
}

/// What deleting `session` takes with it. Without a lease (a coding agent's
/// conversation) the engine's rows are read only if it already runs.
pub(super) async fn one(
    engine: &Engine,
    lease: Option<&Lease>,
    session: &Session,
) -> Result<Confirmation, Refusal> {
    let records = records(engine).await?;
    let rows = if records.at(session.directory()).is_some() {
        let mut rows = match lease {
            Some(lease) => read_inventory(engine, lease).await.unwrap_or_default(),
            None => match engine.acquire(Demand::Passive).await {
                Some(lease) => read_inventory(engine, &lease).await.unwrap_or_default(),
                None => Vec::new(),
            },
        };
        rows.extend(agent_rows(engine).await.unwrap_or_default());
        rows
    } else {
        Vec::new()
    };
    Ok(describe(&[session], 1, &rows, &records).await)
}

pub(super) async fn many(
    engine: &Engine,
    lease: &Lease,
    mut ids: BTreeSet<&str>,
) -> Result<Confirmation, Refusal> {
    let mut rows = read_inventory(engine, lease)
        .await
        .map_err(|_| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running."))?;
    rows.extend(agent_rows(engine).await.unwrap_or_default());
    let mut count = ids.len();
    let mut deleting = Vec::new();
    for Observed { session, .. } in &rows {
        if ids.remove(session.id.as_str()) {
            if session.check_independent().is_ok() {
                deleting.push(session);
            } else {
                count -= 1;
            }
        }
    }
    if deleting.is_empty() {
        return Err(Refusal::new(StatusCode::NOT_FOUND, "None of them is here."));
    }
    let records = records(engine).await?;
    Ok(describe(&deleting, count, &rows, &records).await)
}

async fn records(engine: &Engine) -> Result<Records, Refusal> {
    worktree_notes::read(&engine.home)
        .await
        .map_err(|error| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))
}

async fn describe(
    deleting: &[&Session],
    count: usize,
    rows: &[Observed],
    records: &Records,
) -> Confirmation {
    let asked = deleting
        .iter()
        .map(|session| session.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut roots = BTreeSet::new();
    let mut worktrees = Worktrees::default();
    for session in deleting {
        let Some((tree, location)) = records.at(session.directory()) else {
            continue;
        };
        if !roots.insert(&tree.root) {
            continue;
        }
        let there = rows.iter().filter(|row| {
            row.session.check_independent().is_ok()
                && Path::new(row.session.directory()).starts_with(&tree.root)
        });
        let others = there
            .clone()
            .filter(|row| !asked.contains(row.session.id.as_str()));
        match location {
            Location::Removed => {
                if tree
                    .snapshot
                    .as_ref()
                    .is_some_and(|commit| !commit.is_empty())
                    && others.count() == 0
                {
                    worktrees.kept += 1;
                }
            }
            Location::Live => {
                if others.clone().any(|row| !row.session.archived()) {
                    continue;
                }
                worktrees.live += 1;
                if others.count() == 0 && !worktrees.uncommitted {
                    let clean = crate::start_session::git(
                        Path::new(&tree.root),
                        &["status", "--porcelain", "--untracked-files=normal"],
                    )
                    .await
                    .is_some_and(|status| status.is_empty());
                    worktrees.uncommitted = !clean || there.clone().any(|row| row.running);
                }
            }
        }
    }

    let (title, message) = if count == 1
        && let [session] = deleting
    {
        let message = if worktrees.uncommitted {
            "Its worktree and changes that weren’t committed are deleted too."
        } else if worktrees.live != 0 {
            "Its worktree is deleted too."
        } else if worktrees.kept != 0 {
            "The changes kept to restore its worktree are deleted too."
        } else {
            UNDO
        };
        (title(session), message.to_owned())
    } else {
        let mut message = String::from("Their messages and undo snapshots go too.");
        if worktrees.live > 0 {
            message.push_str(if worktrees.live == 1 {
                " The worktree they were in is deleted too"
            } else {
                " The worktrees they were in are deleted too"
            });
            message.push_str(if worktrees.uncommitted {
                ", with changes that weren’t committed."
            } else {
                "."
            });
        }
        if worktrees.kept > 0 {
            message.push_str(if worktrees.kept == 1 {
                " The changes kept to restore their worktree go too."
            } else {
                " The changes kept to restore their worktrees go too."
            });
        }
        message.push(' ');
        message.push_str(UNDO);
        (format!("Delete {count} sessions?"), message)
    };
    Confirmation {
        title,
        message,
        label: "Delete",
    }
}
