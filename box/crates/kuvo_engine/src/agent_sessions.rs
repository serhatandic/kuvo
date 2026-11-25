//! The coding-agent sessions supplied by the resident's agent runtime: the
//! inventory the lists merge, and the actions the shared session routes take
//! on a conversation the coding agents keep.

use std::{collections::BTreeSet, future::Future, io, pin::Pin, sync::Arc};

use serde_json::{Map, Value};
use tokio::sync::{broadcast, watch};

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

/// A message for a coding agent's line, as the box's queue hands it over.
#[derive(Debug)]
pub struct AgentLinePrompt {
    pub id: String,
    pub text: String,
    pub files: Vec<Value>,
    /// After the turn under way, rather than into it.
    pub queue: bool,
    pub metadata: Option<Map<String, Value>>,
}

/// What became of a message taken back from a coding agent's line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentTakeBack {
    Removed,
    Missing,
    /// Being handed to the agent, or Kuvo is checking whether it took it.
    Busy,
}

/// An event of a coding agent's conversation in the engine's words
/// (`session.inbox.enqueued`, `session.execution.succeeded`, ...), for the
/// box's queue to follow as it follows the engine's.
#[derive(Clone, Debug)]
pub struct AgentEvent {
    pub session: String,
    pub kind: &'static str,
    pub data: Value,
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

    /// What waits in the conversation's line, as the engine lists its inbox,
    /// or `None` when the coding agents don't keep it.
    fn line<'a>(&'a self, _id: &'a str) -> AgentCall<'a, Option<Vec<Value>>> {
        Box::pin(async { Ok(None) })
    }

    /// Put a message in the conversation's line.
    fn put_in_line<'a>(&'a self, _id: &'a str, _prompt: AgentLinePrompt) -> AgentCall<'a, ()> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Steer or queue a waiting message.
    fn set_delivery<'a>(&'a self, _id: &'a str, _item: &'a str, _queue: bool) -> AgentCall<'a, ()> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Take a waiting message back.
    fn take_back<'a>(&'a self, _id: &'a str, _item: &'a str) -> AgentCall<'a, AgentTakeBack> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Stop the turn under way; `resume` starts what you steered next. True
    /// when a turn was under way.
    fn interrupt<'a>(&'a self, _id: &'a str, _resume: bool) -> AgentCall<'a, bool> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    /// Whether the conversation's transcript has this message.
    fn has_message<'a>(&'a self, _id: &'a str, _message: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    /// The conversation's last `limit` transcript rows, newest first.
    fn recent<'a>(&'a self, _id: &'a str, _limit: u32) -> AgentCall<'a, Vec<Value>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// The conversations' line and turn events, in the engine's words.
    fn events(&self) -> Option<broadcast::Receiver<AgentEvent>> {
        None
    }

    /// Stop what the conversation left running besides its turn: the commands its agent left
    /// running. False for one not here.
    fn stop_background<'a>(&'a self, _id: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    /// Stop a turn that waits on an approval or a question. True when one did.
    fn stop_if_waiting<'a>(&'a self, _id: &'a str) -> AgentCall<'a, bool> {
        Box::pin(async { Ok(false) })
    }
}

impl crate::Engine {
    pub fn with_agent_sessions(mut self, source: Arc<dyn AgentSessionSource>) -> Self {
        let events = source.events();
        self.agent_sessions = Some(source);
        if let Some(events) = events {
            crate::queue::follow_agents(self.clone(), events);
        }
        self
    }

    /// Whether the coding agents' conversation is out of its turn, waiting `wait` at most for it
    /// to be: true for one that isn't theirs.
    pub async fn agent_turn_ended(&self, id: &str, wait: std::time::Duration) -> bool {
        let Some(mut running) = self
            .agent_sessions
            .as_ref()
            .and_then(|source| source.running_changes())
        else {
            return true;
        };
        let ended = running.wait_for(|running| !running.contains(id));
        tokio::time::timeout(wait, ended)
            .await
            .is_ok_and(|ended| ended.is_ok())
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
