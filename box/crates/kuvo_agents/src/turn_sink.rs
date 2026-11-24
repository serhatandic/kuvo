//! Connect an adapter's running turn to its resident-owned dispatcher.

use crate::{
    live_asks::TurnAsks,
    runtime::AdapterCall,
    session::{
        Answer, Decision, EventEnvelope, PermissionAsk, Preflight, QuestionAsk, TurnSink,
        VendorState,
    },
    session_store::SessionStore,
};
use std::{io, sync::Arc};
use tokio::sync::{mpsc, oneshot};

pub trait TurnPolicy: Send + Sync {
    fn preflight(&self, ask: PermissionAsk, native_plan: bool) -> AdapterCall<'_, Preflight>;

    /// Leave the decision to the person when the current policy calls for
    /// interaction. A caller can also apply a previous decision here.
    fn permission(&self, ask: &PermissionAsk) -> AdapterCall<'_, Option<Decision>>;
}

pub struct EventWrite {
    pub event: EventEnvelope,
    committed: oneshot::Sender<io::Result<()>>,
}

impl EventWrite {
    /// Acknowledge only after any durable transcript update is complete.
    pub fn finish(self, result: io::Result<()>) {
        let _ = self.committed.send(result);
    }
}

pub struct ResidentSink {
    session: String,
    store: SessionStore,
    asks: TurnAsks,
    policy: Arc<dyn TurnPolicy>,
    events: mpsc::Sender<EventWrite>,
}

impl ResidentSink {
    pub fn channel(
        session: String,
        store: SessionStore,
        asks: TurnAsks,
        policy: Arc<dyn TurnPolicy>,
    ) -> (Arc<Self>, mpsc::Receiver<EventWrite>) {
        let (events, writes) = mpsc::channel(32);
        let sink = Self {
            session,
            store,
            asks,
            policy,
            events,
        };
        (Arc::new(sink), writes)
    }

    pub async fn retire(&self) -> io::Result<()> {
        self.asks.end().await
    }

    pub(crate) fn retirement(&self) -> TurnAsks {
        self.asks.clone()
    }
}

impl TurnSink for ResidentSink {
    fn event(&self, event: EventEnvelope) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            if self.asks.is_ended() {
                return Err(closed());
            }
            let (committed, confirmed) = oneshot::channel();
            self.events
                .send(EventWrite { event, committed })
                .await
                .map_err(|_| closed())?;
            confirmed.await.map_err(|_| closed())?
        })
    }

    fn save_state(&self, state: VendorState) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            if !self.asks.is_ended() && self.store.save_state(&self.session, state).await? {
                Ok(())
            } else {
                Err(closed())
            }
        })
    }

    fn preflight(&self, ask: PermissionAsk, native_plan: bool) -> AdapterCall<'_, Preflight> {
        if self.asks.is_ended() {
            Box::pin(async { Ok(Preflight::Deny) })
        } else {
            self.policy.preflight(ask, native_plan)
        }
    }

    fn permission(&self, ask: PermissionAsk) -> AdapterCall<'_, Decision> {
        Box::pin(async move {
            if self.asks.is_ended() {
                return Ok(Decision::Reject);
            }
            if let Some(decision) = self.policy.permission(&ask).await? {
                return Ok(if self.asks.is_ended() {
                    Decision::Reject
                } else {
                    decision
                });
            }
            self.asks.permission(ask).await
        })
    }

    fn question(&self, ask: QuestionAsk) -> AdapterCall<'_, Answer> {
        Box::pin(self.asks.question(ask))
    }

    fn cancel_ask<'a>(&'a self, id: &'a str) -> AdapterCall<'a, ()> {
        Box::pin(self.asks.cancel(id))
    }
}

fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "The coding-agent turn is no longer open.",
    )
}
