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

    /// The card you're shown for an ask the policy leaves to you: the ask as
    /// it is, unless the policy asks it as something else.
    fn card(&self, ask: PermissionAsk) -> AdapterCall<'_, PermissionAsk> {
        Box::pin(async move { Ok(ask) })
    }
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
    /// The agent whose slot its checkpoints write.
    provider: String,
    store: SessionStore,
    roster: Option<Arc<crate::roster::Roster>>,
    asks: TurnAsks,
    feed: Option<crate::feed::Feed>,
    policy: Arc<dyn TurnPolicy>,
    events: mpsc::Sender<EventWrite>,
}

impl ResidentSink {
    pub fn channel(
        session: String,
        provider: String,
        store: SessionStore,
        roster: Option<Arc<crate::roster::Roster>>,
        asks: TurnAsks,
        feed: Option<crate::feed::Feed>,
        policy: Arc<dyn TurnPolicy>,
    ) -> (Arc<Self>, mpsc::Receiver<EventWrite>) {
        let (events, writes) = mpsc::channel(32);
        let sink = Self {
            session,
            provider,
            store,
            roster,
            asks,
            feed,
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
            if !self.asks.is_ended() && self
                    .store
                    .save_state(&self.session, &self.provider, state)
                    .await? {
                Ok(())
            } else {
                Err(closed())
            }
        })
    }

    fn handover<'a>(&'a self, prompt: &'a str, fresh: bool) -> AdapterCall<'a, Option<String>> {
        Box::pin(self.store.handover(&self.session, &self.provider, prompt, fresh, self.roster.as_deref()))
    }

    fn subagent(&self, task: crate::subagent::Task) -> AdapterCall<'_, crate::subagent::ChildTurn> {
        Box::pin(crate::subagent::ChildTurn::start(self.store.clone(), self.feed.clone(), &self.session, self.asks.turn(), task))
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
            let ask = self.policy.card(ask).await?;
            self.asks.permission(ask).await
        })
    }

    fn question(&self, ask: QuestionAsk) -> AdapterCall<'_, Answer> {
        Box::pin(self.asks.question(ask))
    }

    fn cancel_ask<'a>(&'a self, id: &'a str) -> AdapterCall<'a, ()> {
        Box::pin(self.asks.cancel(id))
    }

    fn cancel_asks(&self) -> AdapterCall<'_, ()> {
        Box::pin(self.asks.cancel_all())
    }
}

fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "The coding-agent turn is no longer open.",
    )
}
