//! Git, Changes, worktrees and projects (rust-box.md row 6.3, part B): so far the box's projects
//! list (`kuvo/projects`) and the safe git layer every Changes route runs git through (`safe`).

pub(crate) mod projects;
pub mod safe;
mod trees;

use std::path::PathBuf;

use kuvo_protocol::routes::get_kuvo_projects;
use kuvo_protocol::version::{FormatRefusalCode, UnsupportedFormatVersion};
use kuvo_server::{Access, Refusal, Registry, RouteReply};

use trees::LinkedTrees;

/// The features these routes answer whole (`projects` waits on its changes and events).
pub const FEATURES: &[&str] = &[];

pub fn register(registry: &mut Registry, box_home: PathBuf) {
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
