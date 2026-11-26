//! The OpenCode side of a worktree's saved filing and session instructions.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use http::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{Engine, api, disk, lifetime::Demand, sessions};

/// Git operations the session starter shares with the worktree routes.
pub trait WorktreeGit: Send + Sync {
    fn run<'a>(
        &'a self,
        folder: &'a Path,
        args: &'a [&'a str],
        asked: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>;
    fn first_commit<'a>(
        &'a self,
        project: &'a Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>;
}

impl Engine {
    pub fn with_worktree_git(mut self, git: std::sync::Arc<dyn WorktreeGit>) -> Self {
        self.worktree_git = Some(git);
        self
    }

    pub(crate) async fn worktree_git(
        &self,
        folder: &Path,
        args: &[&str],
        asked: bool,
    ) -> Option<String> {
        match &self.worktree_git {
            Some(git) => git.run(folder, args, asked).await,
            None => crate::start_session::git(folder, args).await,
        }
    }
}

pub(crate) const FILE: &str = "worktrees.json";

#[derive(Debug, PartialEq, Eq)]
pub struct WorktreeConversation {
    pub session: String,
    pub title: String,
    pub independent: bool,
    pub folder: PathBuf,
    pub archived: bool,
    pub running: bool,
}

#[derive(Deserialize)]
pub(crate) struct Worktree {
    pub(crate) root: String,
    project: String,
    folder: Option<String>,
    branch: Option<String>,
    pub(crate) snapshot: Option<String>,
}

impl Worktree {
    fn folder(&self) -> &str {
        self.folder.as_deref().unwrap_or(&self.root)
    }
}

#[derive(Default)]
pub(crate) struct Records {
    active: Vec<Worktree>,
    restorable: Vec<Worktree>,
    pub(crate) filed: BTreeMap<String, String>,
}

pub(crate) enum Location {
    Live,
    Removed,
}

impl Records {
    pub(crate) fn at(&self, folder: &str) -> Option<(&Worktree, Location)> {
        if folder.is_empty() {
            return None;
        }
        let inside = |tree: &&Worktree| Path::new(folder).starts_with(&tree.root);
        self.active
            .iter()
            .find(inside)
            .map(|tree| (tree, Location::Live))
            .or_else(|| {
                self.restorable
                    .iter()
                    .find(inside)
                    .map(|tree| (tree, Location::Removed))
            })
    }
}

pub(crate) async fn read(home: &Path) -> io::Result<Records> {
    let value = disk::read::<Value>(&home.join("box").join(FILE))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    kuvo_store::format::check_version("Worktree records", value.get("formatVersion"), 0, 1)
        .map_err(io::Error::other)?;
    let rows = |kind: &str| {
        value
            .get(kind)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| Worktree::deserialize(row).ok())
    };
    let active = rows("worktrees").collect::<Vec<_>>();
    let restorable = rows("restorable").collect::<Vec<_>>();
    let mut filed = BTreeMap::new();
    for field in ["handedOver", "removed"] {
        if let Some(entries) = value[field].as_object() {
            filed.extend(entries.iter().filter_map(|(folder, project)| {
                Some((folder.clone(), project.as_str()?.to_owned()))
            }));
        }
    }
    for saved in &restorable {
        filed.insert(saved.folder().to_owned(), saved.project.clone());
    }
    for saved in &active {
        filed.insert(saved.root.clone(), saved.project.clone());
        filed.insert(saved.folder().to_owned(), saved.project.clone());
    }
    Ok(Records {
        active,
        restorable,
        filed,
    })
}

pub(crate) fn watch(engine: Engine, stop: CancellationToken) -> mpsc::UnboundedSender<String> {
    let (sent, mut pending) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let drain = async {
            while let Some(session) = pending.recv().await {
                let _ = engine.tell_worktree(&session).await;
            }
        };
        stop.run_until_cancelled(drain).await;
    });
    sent
}

impl Engine {
    /// Read every local OpenCode conversation and its current run state.
    /// The worktree owner combines this with the coding-agent inventory before removing files.
    pub async fn worktree_inventory(&self) -> io::Result<Vec<WorktreeConversation>> {
        Ok(self
            .worktree_sessions()
            .await?
            .into_iter()
            .filter(|row| row.independent)
            .collect())
    }

    /// All conversations, including dependent runs; a running child also keeps a checkout busy.
    pub async fn worktree_sessions(&self) -> io::Result<Vec<WorktreeConversation>> {
        let lease = self.acquire(Demand::Passive).await.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "The engine isn't running.")
        })?;
        let mut inventory = crate::session_list::read_inventory(self, &lease)
            .await
            .map_err(io::Error::other)?;
        if let Some(source) = &self.agent_sessions {
            for row in source.worktree_inventory().await? {
                let session =
                    serde_json::from_value(Value::Object(row.header)).map_err(io::Error::other)?;
                inventory.push(crate::session_list::Observed {
                    session,
                    running: row.running,
                });
            }
        } else {
            // A native runtime that failed to start cannot tell which retained conversations
            // still use their checkouts. An absent runtime with no native records is known empty.
            match std::fs::read_dir(self.home.join("agents/sessions")) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry?;
                        if entry.file_name().to_string_lossy().starts_with("ses_") {
                            return Err(io::Error::other(
                                "The coding agents' sessions couldn't be read.",
                            ));
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let mut by_place = BTreeMap::<(String, PathBuf), WorktreeConversation>::new();
        for row in inventory {
            let session = row.session;
            let independent = session.check_independent().is_ok();
            let folder = PathBuf::from(session.directory());
            if folder.as_os_str().is_empty() {
                continue;
            }
            let archived = session.archived();
            let key = (session.id.clone(), folder.clone());
            by_place
                .entry(key)
                .and_modify(|old| {
                    // A saved OpenCode history can also describe a live engine session.
                    // Never let a historical idle or archived row hide a live one.
                    old.running |= row.running;
                    old.archived &= archived;
                    old.independent |= independent;
                })
                .or_insert(WorktreeConversation {
                    session: session.id,
                    title: session.title,
                    independent,
                    folder,
                    archived,
                    running: row.running,
                });
        }
        Ok(by_place.into_values().collect())
    }

    /// Refresh OpenCode's instructions after the worktree owner records its branch.
    pub async fn tell_worktree(&self, session: &str) -> io::Result<bool> {
        if !sessions::valid_id(session) {
            return Ok(false);
        }
        let saved = read(&self.home).await?;
        if saved.active.is_empty() {
            return Ok(false);
        }
        let Ok(_admitted) = crate::session_move::admit(self, session, None).await else {
            return Ok(false);
        };
        // Prefer the live engine's header to a copy of its history in the native
        // catalog. A native conversation may exist even with the engine parked.
        let live = if let Some(lease) = self.acquire(Demand::Passive).await {
            self.call::<sessions::Session>(&lease, Method::GET, &sessions::path(session), None)
                .await
                .ok()
                .map(|found| (found, Some(lease)))
        } else {
            None
        };
        let (found, lease) = if let Some(live) = live {
            live
        } else {
            let Some(source) = &self.agent_sessions else {
                return Ok(false);
            };
            let Some(native) = source.session(session).await? else {
                return Ok(false);
            };
            (
                serde_json::from_value(Value::Object(native)).map_err(io::Error::other)?,
                None,
            )
        };
        let Some((tree, branch)) = saved.active.iter().find_map(|tree| {
            (tree.folder() == found.directory()).then_some((tree, tree.branch.as_deref()?))
        }) else {
            return Ok(false);
        };
        let note = format!(
            "You work in a git worktree of this project, on the branch {branch}, so your changes stay apart from the user's own checkout at {} until they merge them. Make your changes here, not there.",
            tree.project
        );
        let Some(lease) = lease else {
            return self
                .agent_sessions
                .as_ref()
                .expect("native session source")
                .set_instruction(session, "kuvo.worktree", Some(note))
                .await;
        };
        let address = api::segment(session);
        let route =
            format!("api/experimental/session/{address}/instructions/entries/kuvo.worktree");
        self.call::<Value>(&lease, Method::PUT, &route, Some(&json!({"value": note})))
            .await
            .map_err(io::Error::other)?;
        Ok(true)
    }
}
