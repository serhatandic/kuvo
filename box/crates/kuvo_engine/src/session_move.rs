//! OpenCode's part of a move. The peer worker owns admission, the journal and
//! the activation decision. This handle pins the engine for the lifetime of the lease.

use std::{collections::HashSet, fmt, path::Path, time::Duration};

use http::Method;
use serde_json::{Value, json};

use crate::{
    Engine, api,
    lifetime::{Demand, Lease},
};

const READ_WAIT: Duration = Duration::from_secs(120);
const EXPORT_WAIT: Duration = Duration::from_secs(600);
const IMPORTED_BY: &str = "kuvo.moveImport";

#[derive(Debug)]
pub enum MoveEngineError {
    Unavailable,
    InvalidTree,
    MissingOwnership,
    Upstream(Box<dyn std::error::Error + Send + Sync>),
}

impl fmt::Display for MoveEngineError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => out.write_str("The engine isn’t running."),
            Self::InvalidTree => out.write_str("Its conversations aren’t its own."),
            Self::MissingOwnership => {
                out.write_str("The conversation copy has no import ownership receipt")
            }
            Self::Upstream(error) => write!(out, "{error}"),
        }
    }
}

impl std::error::Error for MoveEngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Upstream(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<api::Error> for MoveEngineError {
    fn from(error: api::Error) -> Self {
        Self::Upstream(Box::new(error))
    }
}

pub struct SessionMove {
    engine: Engine,
    lease: Lease,
}

impl Engine {
    pub async fn session_move(&self) -> Result<SessionMove, MoveEngineError> {
        let lease = self
            .acquire(Demand::Existing)
            .await
            .ok_or(MoveEngineError::Unavailable)?;
        Ok(SessionMove {
            engine: self.clone(),
            lease,
        })
    }
}

impl SessionMove {
    /// Export the entire session tree, including paginated child lists. Each
    /// export's own ID and parent must agree with the listing that led to it.
    pub async fn export_tree(&self, root: &str) -> Result<Vec<Value>, MoveEngineError> {
        if !valid_session(root) {
            return Err(MoveEngineError::InvalidTree);
        }
        let mut exports = vec![self.export(root, None).await?];
        let mut seen = HashSet::from([root.to_owned()]);
        let mut parents = vec![root.to_owned()];
        while let Some(parent) = parents.pop() {
            for child in self.children(&parent).await? {
                if !seen.insert(child.clone()) {
                    return Err(MoveEngineError::InvalidTree);
                }
                exports.push(self.export(&child, Some(&parent)).await?);
                parents.push(child);
            }
        }
        Ok(exports)
    }

    /// Import with the move's own receipt in the engine's metadata. The
    /// caller has already journaled the import and reserved the destination.
    pub async fn import(
        &self,
        exported: &Value,
        directory: &Path,
        receipt: &str,
    ) -> Result<(), MoveEngineError> {
        let mut info = exported["info"]
            .as_object()
            .cloned()
            .ok_or(MoveEngineError::InvalidTree)?;
        let id = info
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| valid_session(id))
            .ok_or(MoveEngineError::InvalidTree)?;
        if receipt.is_empty() {
            return Err(MoveEngineError::MissingOwnership);
        }
        let _turn = self.engine.turn(id).await;
        let directory_text = directory.to_str().ok_or(MoveEngineError::InvalidTree)?;
        let mut metadata = info
            .remove("metadata")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let chats = self.engine.home.join("chats");
        if directory == chats
            || directory
                .canonicalize()
                .is_ok_and(|actual| chats.canonicalize().is_ok_and(|chats| chats == actual))
        {
            let mut kuvo = metadata
                .remove("kuvo")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            kuvo.insert("chat".into(), json!(directory_text));
            metadata.insert("kuvo".into(), kuvo.into());
        } else if let Some(kuvo) = metadata.get_mut("kuvo").and_then(Value::as_object_mut) {
            kuvo.remove("chat");
        }
        metadata.insert(IMPORTED_BY.into(), json!(receipt));
        info.insert("metadata".into(), metadata.into());
        let body = json!({"info": info, "messages": exported["messages"], "location": {"directory": directory_text}});
        self.engine
            .call_raw(
                &self.lease,
                Method::POST,
                "api/experimental/session/import",
                Some(&body),
                READ_WAIT,
            )
            .await?;
        Ok(())
    }

    /// Roll back an uncertain import only after OpenCode proves that this
    /// move created the copy. Returns whether this call removed the import.
    pub async fn discard_import(
        &self,
        session: &str,
        receipt: &str,
    ) -> Result<bool, MoveEngineError> {
        if !valid_session(session) {
            return Err(MoveEngineError::InvalidTree);
        }
        if receipt.is_empty() {
            return Err(MoveEngineError::MissingOwnership);
        }
        let _turn = self.engine.turn(session).await;
        let route = format!("api/session/{session}");
        let page = match self
            .engine
            .call_raw(&self.lease, Method::GET, &route, None, READ_WAIT)
            .await
        {
            Ok(found) => found,
            Err(error) if error.missing() => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let found = page.get("data").unwrap_or(&page);
        if found["id"].as_str() != Some(session)
            || found["metadata"][IMPORTED_BY].as_str() != Some(receipt)
        {
            return Ok(false);
        }
        match self
            .engine
            .call_raw(&self.lease, Method::DELETE, &route, None, READ_WAIT)
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if error.missing() => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    async fn export(&self, session: &str, parent: Option<&str>) -> Result<Value, MoveEngineError> {
        let route = format!("api/experimental/session/{session}/export");
        let page = self
            .engine
            .call_raw(&self.lease, Method::GET, &route, None, EXPORT_WAIT)
            .await?;
        let export = page.get("data").unwrap_or(&page);
        let info = &export["info"];
        let ancestry = &info["parentID"];
        if info["id"].as_str() != Some(session)
            || !match parent {
                Some(parent) => ancestry.as_str() == Some(parent),
                None => ancestry.is_null(),
            }
        {
            return Err(MoveEngineError::InvalidTree);
        }
        Ok(match page {
            Value::Object(mut fields) => fields.remove("data").unwrap_or_else(|| fields.into()),
            _ => return Err(MoveEngineError::InvalidTree),
        })
    }

    async fn children(&self, parent: &str) -> Result<Vec<String>, MoveEngineError> {
        let mut route = format!("api/session?parentID={parent}&limit=100&order=desc");
        let mut children = Vec::new();
        let mut cursors = HashSet::new();
        loop {
            let page = self
                .engine
                .call_raw(&self.lease, Method::GET, &route, None, READ_WAIT)
                .await?;
            let rows = page["data"]
                .as_array()
                .ok_or(MoveEngineError::InvalidTree)?;
            for row in rows {
                let id = row["id"]
                    .as_str()
                    .filter(|id| valid_session(id))
                    .ok_or(MoveEngineError::InvalidTree)?;
                children.push(id.to_owned());
            }
            let Some(next) = page["cursor"]["next"]
                .as_str()
                .filter(|value| !value.is_empty())
            else {
                return Ok(children);
            };
            if rows.is_empty() || !cursors.insert(next.to_owned()) {
                return Err(MoveEngineError::InvalidTree);
            }
            route = format!(
                "api/session?parentID={parent}&limit=100&cursor={}",
                api::segment(next)
            );
        }
    }
}

fn valid_session(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|suffix| {
        (1..=60).contains(&suffix.len()) && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}
