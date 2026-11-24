//! The OpenCode side of a worktree's saved filing and session instructions.

use std::{collections::BTreeMap, io, path::Path};

use http::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{Engine, api, disk, lifetime::Demand, sessions};

pub(crate) const FILE: &str = "worktrees.json";

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
    /// Refresh OpenCode's instructions after the worktree owner records its branch.
    pub async fn tell_worktree(&self, session: &str) -> io::Result<bool> {
        if !sessions::valid_id(session) {
            return Ok(false);
        }
        let saved = read(&self.home).await?;
        if saved.active.is_empty() {
            return Ok(false);
        }
        let Some(lease) = self.acquire(Demand::Passive).await else {
            return Ok(false);
        };
        let Ok(_admitted) = crate::session_move::admit(self, session, None).await else {
            return Ok(false);
        };
        let found: sessions::Session = self
            .call(&lease, Method::GET, &sessions::path(session), None)
            .await
            .map_err(io::Error::other)?;
        let Some((tree, branch)) = saved.active.iter().find_map(|tree| {
            (tree.folder() == found.directory()).then_some((tree, tree.branch.as_deref()?))
        }) else {
            return Ok(false);
        };
        let note = format!(
            "You work in a git worktree of this project, on the branch {branch}, so your changes stay apart from the user's own checkout at {} until they merge them. Make your changes here, not there.",
            tree.project
        );
        let address = api::segment(session);
        let route =
            format!("api/experimental/session/{address}/instructions/entries/kuvo.worktree");
        self.call::<Value>(&lease, Method::PUT, &route, Some(&json!({"value": note})))
            .await
            .map_err(io::Error::other)?;
        Ok(true)
    }
}
