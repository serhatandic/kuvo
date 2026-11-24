//! Keep a native prompt's backup until the inbox or transcript confirms it.

use std::{collections::BTreeMap, time::Duration};

use http::{Method, StatusCode};
use kuvo_os::process::ProcessIdentity;
use kuvo_server::Refusal;
use serde::{Deserialize, Serialize};

use super::{
    Delivery, Inbox, Queue, inbox, queue_failed, reached, save_failed, saved_removal,
    transcript_has,
};
use crate::{Engine, api, lifetime::Lease, sessions};

// The native fields also occur in interrupted sends from older residents.
#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Handoffs {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    handoff: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    handoff_settled: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    handoff_pids: BTreeMap<String, u32>,
}

impl Handoffs {
    pub(super) fn is_empty(&self) -> bool {
        self.handoff.is_empty()
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.handoff.iter().any(|each| each == id)
    }

    fn unresolved(&self, id: &str) -> bool {
        self.contains(id)
            && !self.handoff_settled.iter().any(|each| each == id)
            && !self
                .handoff_pids
                .get(id)
                .is_some_and(|pid| matches!(ProcessIdentity::read(*pid), Ok(None)))
    }

    fn start(&mut self, id: &str, pid: u32) {
        self.clear(id);
        self.handoff.push(id.to_owned());
        if pid > 0 {
            self.handoff_pids.insert(id.to_owned(), pid);
        }
    }

    pub(super) fn clear(&mut self, id: &str) {
        self.handoff.retain(|each| each != id);
        self.handoff_settled.retain(|each| each != id);
        self.handoff_pids.remove(id);
    }

    pub(super) fn check(&self, id: &str) -> Result<(), Refusal> {
        if self.unresolved(id) {
            return Err(busy("The previous send may still be on its way."));
        }
        Ok(())
    }
}

fn busy(message: &str) -> Refusal {
    Refusal::new(StatusCode::BAD_GATEWAY, message)
}

fn settled(error: &api::Error) -> bool {
    matches!(error, api::Error::Status(_, _)) || error.connection_refused()
}

pub(super) async fn send(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    id: &str,
    steer: bool,
) -> Result<bool, Refusal> {
    if saved_removal(engine, session, id)
        .await
        .map_err(save_failed)?
        .is_some_and(|removal| !removal.complete)
    {
        return Ok(false);
    }
    let Some(mut prompt) = engine
        .queues
        .waiting_prompt(id)
        .await
        .map_err(save_failed)?
    else {
        return Ok(false);
    };
    if !reached(engine, lease, session, id)
        .await
        .map_err(queue_failed)?
    {
        engine
            .queues
            .get(session)
            .await
            .map_err(save_failed)?
            .handoffs
            .check(id)?;
        if steer {
            prompt.delivery = Delivery::Steer;
        }
        let prompt = serde_json::to_value(prompt).map_err(|error| save_failed(error.into()))?;
        engine
            .queues
            .put(session, |state| state.handoffs.start(id, lease.record.pid))
            .await
            .map_err(save_failed)?;
        let result = engine
            .call_raw(
                lease,
                Method::POST,
                &format!("{}/prompt", sessions::path(session)),
                Some(&prompt),
                Duration::from_secs(120),
            )
            .await;
        if result.as_ref().is_ok() || result.as_ref().is_err_and(settled) {
            engine
                .queues
                .put(session, |state| {
                    state.handoffs.handoff_settled.push(id.to_owned())
                })
                .await
                .map_err(save_failed)?;
        }
        result.map_err(queue_failed)?;
        if !reached(engine, lease, session, id)
            .await
            .map_err(queue_failed)?
        {
            return Err(busy("A waiting message was not confirmed."));
        }
    }
    engine
        .queues
        .clear(session, id)
        .await
        .map_err(save_failed)?;
    Ok(true)
}

/// Absence can only settle a completed native POST or one whose process died.
pub(super) async fn removable(
    engine: &Engine,
    lease: Option<&Lease>,
    session: &str,
    id: &str,
    saved: &Handoffs,
    items: Option<&[Inbox]>,
) -> Result<bool, Refusal> {
    if !saved.contains(id) {
        return Ok(true);
    }
    saved.check(id)?;
    let lease = lease.ok_or_else(|| busy("The engine isn't answering."))?;
    let read;
    let items = match items {
        Some(items) => items,
        None => {
            read = inbox(engine, lease, session).await.map_err(queue_failed)?;
            &read
        }
    };
    if items.iter().any(|entry| entry.id == id) {
        return Err(busy("The engine still holds this message."));
    }
    Ok(!transcript_has(engine, lease, session, id)
        .await
        .map_err(queue_failed)?)
}

pub(super) async fn pending(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    mut queue: Queue,
    items: &[Inbox],
) -> Result<Queue, Refusal> {
    for id in &queue.handoffs.handoff {
        if !items.iter().any(|entry| entry.id == *id)
            && transcript_has(engine, lease, session, id)
                .await
                .map_err(queue_failed)?
        {
            queue.waiting.retain(|entry| entry.id != *id);
            queue.held.retain(|entry| entry != id);
        }
    }
    Ok(queue)
}
