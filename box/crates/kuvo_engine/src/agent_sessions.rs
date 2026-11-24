//! The coding-session inventory supplied by the resident's agent runtime.

use std::{collections::BTreeSet, future::Future, io, pin::Pin, sync::Arc};

use serde_json::{Map, Value};
use tokio::sync::watch;

pub struct AgentSessionRow {
    pub header: Map<String, Value>,
    pub running: bool,
}

pub type AgentSessionRead<'a> =
    Pin<Box<dyn Future<Output = io::Result<Vec<AgentSessionRow>>> + Send + 'a>>;

pub trait AgentSessionSource: Send + Sync {
    fn inventory(&self) -> AgentSessionRead<'_>;

    fn running_changes(&self) -> Option<watch::Receiver<Arc<BTreeSet<String>>>> {
        None
    }
}

impl crate::Engine {
    pub fn with_agent_sessions(mut self, source: Arc<dyn AgentSessionSource>) -> Self {
        self.agent_sessions = Some(source);
        self
    }
}
