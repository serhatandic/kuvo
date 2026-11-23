//! The session's ask routes also carry cards owned by the sandbox and vault.

use std::{future::Future, pin::Pin, sync::Arc};

use kuvo_protocol::routes::types::{
    GetKuvoPeerSessionComputerSessionAsksStatus200AsksItemSandbox as SandboxCard,
    GetKuvoPeerSessionComputerSessionAsksStatus200AsksItemVault as VaultCard,
};
use kuvo_server::{DispatchRequest, Refusal};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub type AskResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, Refusal>> + Send + 'a>>;

pub enum ExternalCard {
    Sandbox(SandboxCard),
    Vault(VaultCard),
}

impl ExternalCard {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Sandbox(card) => &card.id,
            Self::Vault(card) => &card.id,
        }
    }

    pub(crate) fn session(&self) -> &str {
        match self {
            Self::Sandbox(card) => &card.session,
            Self::Vault(card) => &card.session,
        }
    }

    pub(crate) fn encode(&self) -> serde_json::Value {
        match self {
            Self::Sandbox(card) => serde_json::json!(card),
            Self::Vault(card) => serde_json::json!(card),
        }
    }
}

pub struct ExternalAsk {
    pub at: u64,
    pub card: ExternalCard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExternalAskKind {
    Sandbox,
    Vault,
}

#[derive(Clone)]
pub struct ExternalWaiting {
    pub id: String,
    pub session: String,
    pub at: u64,
    pub kind: ExternalAskKind,
}

#[derive(Clone, Copy)]
pub enum ExternalDecision {
    Once,
    Always,
    Reject,
}

pub struct ExternalReply<'a> {
    pub kind: ExternalAskKind,
    pub session: &'a str,
    pub id: &'a str,
    pub decision: ExternalDecision,
    pub item: Option<&'a str>,
    pub request: &'a DispatchRequest,
    pub cancel: &'a CancellationToken,
}

/// The owning service validates the live grant and consumes the ask together.
/// Vault proofs and sandbox policy stay with the services that hold the asks.
pub trait ExternalAsks: Send + Sync {
    fn list<'a>(
        &'a self,
        sessions: &'a [String],
        viewer: &'a DispatchRequest,
    ) -> AskResult<'a, Vec<ExternalAsk>>;
    fn reply<'a>(&'a self, answer: ExternalReply<'a>) -> AskResult<'a, ()>;
    fn waiting(&self) -> watch::Receiver<Arc<Vec<ExternalWaiting>>>;
}

impl crate::Engine {
    pub fn with_external_asks(mut self, source: Arc<dyn ExternalAsks>) -> Self {
        self.external_asks = Some(source);
        self
    }
}
