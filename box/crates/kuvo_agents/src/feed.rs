//! What the coding agents tell their clients (`kuvo/agents/api/event`): their accounts, programs
//! and updates changed. One broadcast for every agent; their conversations' turns join it where
//! the feed is served.

use serde_json::Value;
use tokio::sync::broadcast;

/// An event in the engine's words: `integration.updated`, `model.updated`, …
#[derive(Clone, Debug, PartialEq)]
pub struct FeedEvent {
    pub kind: String,
    pub data: Value,
    /// The folder of the conversation it's about, where there is one.
    pub directory: Option<String>,
}

#[derive(Clone)]
pub struct Feed(broadcast::Sender<FeedEvent>);

impl Default for Feed {
    fn default() -> Self {
        Self(broadcast::channel(256).0)
    }
}

impl Feed {
    pub fn emit(&self, kind: &str, data: Value) {
        let _ = self.0.send(FeedEvent {
            kind: kind.to_owned(),
            data,
            directory: None,
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<FeedEvent> {
        self.0.subscribe()
    }
}
