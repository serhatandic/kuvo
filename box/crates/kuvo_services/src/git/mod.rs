//! Git, Changes, worktrees and projects (rust-box.md row 6.3).

mod actions;
mod branches;
mod changes;
mod follow;
mod list;
pub(crate) mod projects;
mod pulls;
mod routes;
pub mod safe;
mod trees;

use std::path::PathBuf;

use http::StatusCode;
use kuvo_protocol::routes::{get_kuvo_projects, put_kuvo_projects};
use kuvo_protocol::version::{FormatRefusalCode, UnsupportedFormatVersion};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Value, json};

pub use projects::{ChangeFailed, ProjectNames, Unsupported, change_projects, read_projects};
use trees::LinkedTrees;

/// The features these routes answer whole (`projects` waits on its changes and events).
pub const FEATURES: &[&str] = &[
    "git.branches",
    "git.branches.pullRequest",
    "git.branches.worktree",
    "git.changes.binary",
    "git.changes.path",
    "git.files",
    "git.files.does",
    "git.merge",
    "git.pull-request",
    "git.pull-request.draft",
    "git.push",
];

pub fn register(registry: &mut Registry, box_home: PathBuf, is_mac: bool) {
    let home = box_home.clone();
    registry.register::<put_kuvo_projects::Spec, _, _>(Access::Any, move |call| {
        let home = home.clone();
        async move {
            let folder = form_urlencoded::parse(
                call.request
                    .path
                    .split_once('?')
                    .map_or("", |(_, query)| query)
                    .as_bytes(),
            )
            .find(|(key, _)| key == "directory")
            .map(|(_, value)| value.into_owned())
            .filter(|folder| projects::absolute(folder))
            .ok_or_else(|| {
                Refusal::new(StatusCode::BAD_REQUEST, "Say which folder (directory).")
            })?;
            let body = serde_json::from_slice::<Value>(&call.request.body).unwrap_or_default();
            let body = if body.is_null() { json!({}) } else { body };
            let given = body
                .as_object()
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "That isn’t a project."))?;
            let project = projects::put(&home, &folder, given).map_err(|error| match error {
                ChangeFailed::Unsupported(error) => unsupported(error),
                other => Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
            })?;
            Ok(RouteReply::json_as_kept::<200>(json!({"project": project})))
        }
    });
    routes::register(registry, box_home.clone(), is_mac);
    let linked = LinkedTrees::default();
    registry.register::<get_kuvo_projects::Spec, _, _>(Access::Any, move |_| {
        let (box_home, linked) = (box_home.clone(), linked.clone());
        async move {
            let listed = projects::listed(&box_home, &linked).map_err(unsupported)?;
            // (Each project as it's kept: fields the route doesn't name are answered too, as TS does.)
            Ok(RouteReply::<get_kuvo_projects::Spec>::json_as_kept::<200>(
                listed,
            ))
        }
    });
}

/// A record written by a newer box, refused as the TS box refuses it.
fn unsupported(error: projects::Unsupported) -> Refusal {
    Refusal::unsupported_record(UnsupportedFormatVersion {
        code: FormatRefusalCode::UnsupportedFormatVersion,
        message: error.to_string(),
    })
}
