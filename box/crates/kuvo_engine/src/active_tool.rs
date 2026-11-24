//! Verify an agent tool invocation against the current engine transcript.

use crate::{
    Engine, api,
    lifetime::{Demand, Lease},
};
use http::Method;
use serde_json::Value;
use std::{io, path::PathBuf};

pub struct ActiveTool {
    pub root: String,
    pub name: Option<String>,
    pub group: Option<String>,
    pub seat: bool,
    pub directory: Option<PathBuf>,
}

impl Engine {
    /// Look up the caller after matching the tool's own running or pending transcript block.
    pub async fn active_tool(
        &self,
        session: &str,
        name: &str,
        input: &Value,
    ) -> Option<ActiveTool> {
        if !session_id(session) {
            return None;
        }
        let lease = self.acquire(Demand::Passive).await?;
        let page = format!("{}/message?limit=6&order=desc", session_url(session));
        let found = self
            .call::<Vec<Value>>(&lease, Method::GET, &page, None)
            .await
            .ok()?;
        let empty = serde_json::json!({});
        let expected = kuvo_wire::json::canonical(input);
        let matches = |block: &Value| {
            block["type"] == "tool"
                && block["name"].as_str().is_some_and(|given| {
                    given == name
                        || given
                            .strip_suffix(name)
                            .is_some_and(|outer| outer.ends_with('_'))
                })
                && matches!(
                    block["state"]["status"].as_str(),
                    Some("running" | "pending")
                )
                && kuvo_wire::json::canonical(block["state"].get("input").unwrap_or(&empty))
                    == expected
        };
        if !found
            .iter()
            .filter_map(|msg| msg["content"].as_array())
            .flatten()
            .any(matches)
        {
            return None;
        }
        let current = self.root_record(&lease, session).await.ok()?;
        let seat = current.pointer("/metadata/kuvo/seat");
        let group = seat
            .and_then(|seat| seat["group"].as_str())
            .filter(|group| !group.is_empty())
            .map(str::to_owned);
        let root = current["id"]
            .as_str()
            .filter(|id| session_id(id))
            .unwrap_or(session)
            .into();
        let name = current["title"].as_str().map(str::to_owned);
        let directory = current["location"]["directory"]
            .as_str()
            .or_else(|| current["directory"].as_str())
            .map(PathBuf::from);
        Some(ActiveTool {
            root,
            name,
            group,
            seat: seat.is_some() || above(&current).is_some(),
            directory,
        })
    }

    /// Resolve a handoff recipient in the local session catalog.
    pub async fn tool_session_owner(&self, session: &str) -> io::Result<Option<(String, String)>> {
        if !session_id(session) {
            return Ok(None);
        }
        let lease = self
            .acquire(Demand::Passive)
            .await
            .ok_or_else(|| io::Error::other("The engine isn't running."))?;
        let root = match self.root_record(&lease, session).await {
            Ok(found) => found,
            Err(error) if error.missing() => return Ok(None),
            Err(error) => return Err(io::Error::other(error)),
        };
        let id = root["id"]
            .as_str()
            .filter(|id| session_id(id))
            .unwrap_or(session)
            .to_owned();
        let title = root["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .unwrap_or("that session")
            .to_owned();
        Ok(Some((id, title)))
    }

    async fn root_record(&self, lease: &Lease, session: &str) -> Result<Value, api::Error> {
        let mut current = self
            .call::<Value>(lease, Method::GET, &session_url(session), None)
            .await?;
        for _ in 0..5 {
            let Some(parent) = above(&current) else { break };
            current = self
                .call::<Value>(lease, Method::GET, &session_url(parent), None)
                .await?;
        }
        Ok(current)
    }
}

fn session_url(session: &str) -> String {
    format!("api/session/{}", api::segment(session))
}

fn session_id(id: &str) -> bool {
    (1..=100).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn above(record: &Value) -> Option<&str> {
    record["parentID"]
        .as_str()
        .or_else(|| record["metadata"]["kuvo"]["engineFor"].as_str())
        .filter(|value| session_id(value))
}
