//! Connect the engine's session cards to the vault holder in this resident.

use std::sync::Arc;

use http::StatusCode;
use kuvo_engine::{
    AskResult, ExternalAsk, ExternalAskKind, ExternalAsks, ExternalCard, ExternalDecision,
    ExternalReply, ExternalWaiting,
};
use kuvo_server::{DispatchRequest, Refusal};
use kuvo_services::vault::{Resident, VaultAskReply, VaultDecision, VaultWaiting};
use tokio::{sync::watch, task::JoinHandle};

pub(crate) struct VaultAsks {
    vault: Arc<Resident>,
    waiting: watch::Receiver<Arc<Vec<ExternalWaiting>>>,
    updates: JoinHandle<()>,
}

impl VaultAsks {
    pub fn new(vault: Arc<Resident>) -> Self {
        let mut source = vault.session_waiting();
        let (publish, waiting) = watch::channel(waiting_rows(&source.borrow_and_update()));
        let updates = tokio::spawn(async move {
            while source.changed().await.is_ok() {
                publish.send_replace(waiting_rows(&source.borrow_and_update()));
            }
        });
        Self {
            vault,
            waiting,
            updates,
        }
    }
}

impl Drop for VaultAsks {
    fn drop(&mut self) {
        self.updates.abort();
    }
}

impl ExternalAsks for VaultAsks {
    fn list<'a>(
        &'a self,
        sessions: &'a [String],
        viewer: &'a DispatchRequest,
    ) -> AskResult<'a, Vec<ExternalAsk>> {
        Box::pin(async move {
            self.vault.session_asks(sessions, viewer).map(|asks| {
                asks.into_iter()
                    .map(|ask| ExternalAsk {
                        at: ask.at,
                        card: ExternalCard::Vault(ask.card),
                    })
                    .collect()
            })
        })
    }

    fn reply<'a>(&'a self, answer: ExternalReply<'a>) -> AskResult<'a, ()> {
        Box::pin(async move {
            if answer.kind != ExternalAskKind::Vault {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "It isn’t waiting any more.",
                ));
            }
            let decision = match answer.decision {
                ExternalDecision::Once => VaultDecision::Once,
                ExternalDecision::Always => VaultDecision::Always,
                ExternalDecision::Reject => VaultDecision::Reject,
            };
            self.vault.decide_session_ask(VaultAskReply {
                session: answer.session,
                id: answer.id,
                decision,
                item: answer.item,
                request: answer.request,
                cancel: answer.cancel,
            })
        })
    }

    fn waiting(&self) -> watch::Receiver<Arc<Vec<ExternalWaiting>>> {
        self.waiting.clone()
    }
}

fn waiting_rows(rows: &[VaultWaiting]) -> Arc<Vec<ExternalWaiting>> {
    Arc::new(
        rows.iter()
            .map(|row| ExternalWaiting {
                id: row.id.clone(),
                session: row.session.clone(),
                at: row.at,
                kind: ExternalAskKind::Vault,
            })
            .collect(),
    )
}
