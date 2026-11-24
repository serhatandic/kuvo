//! Git, Changes, worktrees and projects (rust-box.md row 6.3, part B): so far the box's projects
//! list (`kuvo/projects`).

mod projects;
mod trees;

use std::path::PathBuf;

use http::StatusCode;
use kuvo_protocol::JsonStatus;
use kuvo_protocol::routes::get_kuvo_projects;
use kuvo_protocol::version::{FormatRefusalCode, UnsupportedFormatVersion};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::de::DeserializeOwned;

use trees::LinkedTrees;

/// The features these routes answer whole (`projects` waits on its changes and events).
pub const FEATURES: &[&str] = &[];

pub fn register(registry: &mut Registry, box_home: PathBuf) {
    let linked = LinkedTrees::default();
    registry.register::<get_kuvo_projects::Spec, _, _>(Access::Any, move |_| {
        let (box_home, linked) = (box_home.clone(), linked.clone());
        async move {
            let listed = projects::listed(&box_home, &linked).map_err(unsupported)?;
            typed::<_, 200>(listed)
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

fn typed<R, const STATUS: u16>(value: serde_json::Value) -> Result<RouteReply<R>, Refusal>
where
    R: JsonStatus<STATUS>,
    <R as JsonStatus<STATUS>>::Body: DeserializeOwned,
{
    serde_json::from_value(value)
        .map(RouteReply::json::<STATUS>)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}
