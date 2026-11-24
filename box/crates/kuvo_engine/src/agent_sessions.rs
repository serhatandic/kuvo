//! The coding-agent sessions supplied by the resident's agent runtime: the
//! inventory the lists merge, and the actions the shared session routes take
//! on a conversation the coding agents keep.

use std::{collections::BTreeSet, future::Future, io, pin::Pin, sync::Arc};

use serde_json::{Map, Value};
use tokio::sync::watch;

pub struct AgentSessionRow {
    pub header: Map<String, Value>,
    pub running: bool,
}

pub type AgentSessionRead<'a> =
    Pin<Box<dyn Future<Output = io::Result<Vec<AgentSessionRow>>> + Send + 'a>>;

pub type AgentCall<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// A client's change to a conversation's header.
#[derive(Debug, Default)]
pub struct AgentSessionPatch {
    pub title: Option<String>,
    pub permissions: Option<Value>,
    pub metadata: Option<Map<String, Value>>,
}

/// A new conversation for a coding agent, as a client's start asks for it.
#[derive(Debug)]
pub struct NewAgentSession {
    pub id: String,
    pub directory: String,
    pub agent: Option<String>,
    pub model: Value,
    pub title: Option<String>,
    pub permissions: Option<Value>,
    pub metadata: Map<String, Value>,
}

/// A message for a coding agent's conversation.
#[derive(Debug)]
pub struct AgentPrompt {
    pub id: String,
    pub text: String,
    pub metadata: Option<Map<String, Value>>,
}

pub trait AgentSessionSource: Send + Sync {
    fn inventory(&self) -> AgentSessionRead<'_>;

    fn running_changes(&self) -> Option<watch::Receiver<Arc<BTreeSet<String>>>> {
        None
    }

    /// The conversation as clients read it, or `None` when the coding agents
    /// don't keep it.
    fn session<'a>(&'a self, _id: &'a str) -> AgentCall<'a, Option<Map<String, Value>>> {
        Box::pin(async { Ok(None) })
    }

    /// Change its header. Answers the header as saved, or `None` when it's gone.
    fn update<'a>(
        &'a self,
        _id: &'a str,
        _patch: AgentSessionPatch,
    ) -> AgentCall<'a, Option<Map<String, Value>>> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Stop its turn and remove it. False when it wasn't here.
    fn delete<'a>(&'a self, _id: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Make a conversation. `InvalidInput` names a model no agent here runs.
    fn create(&self, _new: NewAgentSession) -> AgentCall<'_, Map<String, Value>> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Give the conversation a message, starting its turn.
    fn prompt<'a>(&'a self, _id: &'a str, _prompt: AgentPrompt) -> AgentCall<'a, ()> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Stop a turn that waits on an approval or a question. True when one did.
    fn stop_if_waiting<'a>(&'a self, _id: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async { Ok(false) })
    }
}

impl crate::Engine {
    pub fn with_agent_sessions(mut self, source: Arc<dyn AgentSessionSource>) -> Self {
        self.agent_sessions = Some(source);
        self
    }

    /// The coding agents' conversation `id`, or `None` for the engine's.
    pub(crate) async fn agent_session(
        &self,
        id: &str,
    ) -> Result<Option<(Arc<dyn AgentSessionSource>, Map<String, Value>)>, kuvo_server::Refusal>
    {
        let Some(source) = self.agent_sessions.clone() else {
            return Ok(None);
        };
        match source.session(id).await {
            Ok(Some(header)) => Ok(Some((source, header))),
            Ok(None) => Ok(None),
            Err(error) => {
                eprintln!("Kuvo could not read the coding-agent session {id}: {error}");
                Err(kuvo_server::Refusal::new(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "The coding agents on this computer aren't answering. Try again in a moment.",
                ))
            }
        }
    }
}
