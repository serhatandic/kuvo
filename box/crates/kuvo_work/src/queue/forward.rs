pub(super) mod records;

use kuvo_os::timer_fires::{self, Timer};
use super::{
    routes::{internal, storage_refusal},
    store::Store,
};
use http::StatusCode;
use kuvo_peers::{
    directory::ComputerKey,
    ownership,
    relay::{RelayEvent, RelayHandle},
    work_queue_mails,
};
use kuvo_server::Refusal;
use records::{Act, Method, Reply, Spool};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{sync::broadcast, time::Instant};

pub(super) const AGENT_FEATURE: &str = "work-queue.forward.agent";

struct Outgoing {
    item: String,
    named: Option<String>,
    method: Method,
    action: Option<String>,
    area: Option<String>,
    body: Option<Value>,
    id: Option<String>,
    agent: Option<records::Agent>,
}

pub(super) struct CallAgent {
    pub agent: records::Agent,
    pub undo: bool,
    pub body: Value,
    pub operation: Option<String>,
}

const RETRY: Duration = Duration::from_secs(5);
const WAIT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub(super) struct Forward {
    store: Store,
    owners: ownership::Handle,
    relay: Option<RelayHandle>,
}

impl Forward {
    pub fn new(store: Store, owners: ownership::Handle, relay: Option<RelayHandle>) -> Self {
        Self {
            store,
            owners,
            relay,
        }
    }

    pub fn admits(&self, peer: ComputerKey) -> bool {
        self.owners.work_queue_destination(peer).is_ok()
    }

    pub async fn call(
        &self,
        target: &super::peer_reads::QueueTarget,
        named: String,
        method: Method,
        action: Option<String>,
        body: Option<Value>,
    ) -> Result<Value, Refusal> {
        self.request(
            target.key,
            target.computer.clone(),
            Outgoing {
                item: target.id.clone(),
                named: Some(named),
                method,
                action,
                area: None,
                body,
                id: None,
                agent: None,
            },
        )
        .await
    }

    pub async fn read_edit(
        &self,
        peer: ComputerKey,
        computer: Value,
        body: Value,
    ) -> Result<Value, Refusal> {
        self.request(
            peer,
            computer,
            Outgoing {
                item: "offline-edit".into(),
                named: None,
                method: Method::Get,
                action: None,
                area: Some("ownership".into()),
                body: Some(body),
                id: None,
                agent: None,
            },
        )
        .await
    }

    pub fn legacy_label_main(&self, main: &str) -> Option<(ComputerKey, Value)> {
        let peer = kuvo_wire::keys::decode_exact(main)?;
        let (view, _, _) = self.owners.work_queue_destination(peer).ok()?;
        if !view.advertises(&peer, "data.ownership") {
            return None;
        }
        let computer = view.roster.member(&peer)?;
        Some((
            peer,
            json!({"key":main,"name":computer.name,"kind":computer.kind}),
        ))
    }

    pub async fn owner_write(
        &self,
        peer: ComputerKey,
        computer: Value,
        path: &str,
        value: Value,
    ) -> Result<Value, Refusal> {
        self.request(
            peer,
            computer,
            Outgoing {
                item: path.into(),
                named: None,
                method: Method::Post,
                action: None,
                area: Some("ownership".into()),
                body: Some(json!({"value":value})),
                id: None,
                agent: None,
            },
        )
        .await
    }

    pub async fn agent_change(
        &self,
        peer: ComputerKey,
        computer: Value,
        item: String,
        action: CallAgent,
    ) -> Result<Value, Refusal> {
        let CallAgent {
            agent,
            undo,
            body,
            operation,
        } = action;
        let (method, directive) = match agent.tool {
            records::Tool::Remove if undo => (Method::Post, Some("restore".into())),
            records::Tool::Remove => (Method::Delete, None),
            records::Tool::Update | records::Tool::Hand => (Method::Patch, None),
        };
        let id = operation
            .as_deref()
            .map(|key| format!("wqa_tool_{}", &key[..key.len().min(32)]));
        self.request(
            peer,
            computer,
            Outgoing {
                item,
                named: None,
                method,
                action: directive,
                area: Some("queue-agent".into()),
                body: Some(body),
                id,
                agent: Some(agent),
            },
        )
        .await
    }

    pub fn pairing_state(&self, key: &ComputerKey, pairing: &str) -> ownership::PairingState {
        self.owners.current_pairing(key, pairing)
    }

    async fn request(
        &self,
        peer: ComputerKey,
        computer: Value,
        outgoing: Outgoing,
    ) -> Result<Value, Refusal> {
        let Outgoing {
            item,
            named,
            method,
            action,
            area,
            body,
            id,
            agent,
        } = outgoing;
        let display = computer["name"].as_str().unwrap_or_default();
        let named = named.as_deref().unwrap_or(&item);
        let failed = |status, reason: String| {
            Refusal::json(status, json!({"message":reason,"computer":computer}))
        };
        let (view, setup, host) = self.owners.work_queue_destination(peer).map_err(|_| {
            failed(
                StatusCode::CONFLICT,
                format!("{named} is on {display}: change it there."),
            )
        })?;
        if !view.advertises(&peer, "work-queue.forward") {
            return Err(failed(
                StatusCode::CONFLICT,
                format!("{named} is on {display}: change it there."),
            ));
        }
        let away = || {
            failed(
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "{display} isn't answering now (it may be asleep, off or offline), so nothing was changed there. Try again once it's back."
                ),
            )
        };
        if view
            .standing(peer, jiff::Timestamp::now().as_millisecond())
            .away()
        {
            return Err(away());
        }
        let relay = self.relay.as_ref().ok_or_else(away)?;
        let mut events = relay.subscribe().ok_or_else(away)?;
        if !view.relay_connected {
            let here = &view.roster.name;
            return Err(failed(
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "{here} isn't connected to your other computers now, so nothing was changed on {display}. Try again in a moment."
                ),
            ));
        }
        let id = match id {
            Some(id) => id,
            None => {
                let mut random = [0; 16];
                getrandom::fill(&mut random).map_err(internal)?;
                format!("wqa_{}", kuvo_wire::keys::encode(&random))
            }
        };
        let at = jiff::Timestamp::now().as_millisecond();
        let spool = Spool {
            to: kuvo_wire::keys::encode(&peer),
            act: Act {
                id: id.clone(),
                at,
                item,
                method,
                action,
                name: None,
                area,
                body,
                agent,
            },
        };
        let payload = json!({"act":spool.act});
        let mails = work_queue_mails(payload, at).ok_or_else(|| {
            failed(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("That change is too big to send to {display}."),
            )
        })?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || spool.write(&store))
            .await
            .map_err(internal)?
            .map_err(storage_refusal)?;
        let _kept = Sending {
            store: self.store.clone(),
            id: id.clone(),
        };
        let deadline = Instant::now() + WAIT;
        let mut retry = timer_fires::interval(Timer::NumberForwardWait, RETRY);
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    let here = &view.roster.name;
                    return Err(failed(StatusCode::SERVICE_UNAVAILABLE, format!("{display} didn't answer in time, so {here} can't tell whether it was done there. Look again in a moment.")));
                }
                _ = retry.tick() => {
                    if self.owners.work_queue_destination(peer).is_err() { return Err(away()); }
                    for mail in &mails { if relay.sender.mail(host.clone(), mail.clone()).await.is_err() { break; } }
                }
                event = events.recv() => match event {
                    Ok(event) => match event.as_ref() {
                        RelayEvent::Unreachable(address) if *address == host.address => {
                            let here = &view.roster.name;
                            return Err(failed(StatusCode::SERVICE_UNAVAILABLE, format!("{display} stopped answering, so {here} can't tell whether it was done there. Look again once it's back.")));
                        }
                        RelayEvent::Mail { key, envelope, .. } if key == &peer => {
                            let Some(value) = envelope.0.get("workQueue").and_then(|value| value.get("acted")) else { continue; };
                            if value["id"] != id { continue; }
                            if self.owners.work_queue_destination(peer).ok().is_none_or(|(_, current, _)| current != setup) { return Err(away()); }
                            let Ok(reply) = serde_json::from_value::<Reply>(value.clone()) else { continue; };
                            let store = self.store.clone(); let sender = kuvo_wire::keys::encode(&peer); let value = value.clone();
                            tokio::task::spawn_blocking(move || records::keep_answer(&store, &sender, &value)).await.map_err(internal)?.map_err(storage_refusal)?;
                            if reply.status != 200 {
                                let mut value = if reply.value.is_object() { reply.value } else { json!({}) };
                                value["computer"] = computer;
                                return Err(Refusal::json(StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY), value));
                            }
                            return Ok(reply.value);
                        }
                        _ => {}
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Err(away()),
                }
            }
        }
    }
}

struct Sending {
    store: Store,
    id: String,
}

impl Drop for Sending {
    fn drop(&mut self) {
        records::forget(&self.store, &self.id);
    }
}

impl kuvo_services::settings::ArchiveForward for Forward {
    fn write<'a>(
        &'a self,
        peer: [u8; 32],
        computer: Value,
        session: &'a str,
        action: &'a str,
        revision: &'a str,
        caller: &'a kuvo_server::Caller,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Refusal>> + Send + 'a>> {
        Box::pin(async move {
            if !matches!(caller, kuvo_server::Caller::Local(_)) {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Open this record on {} to change it.",
                        computer["name"].as_str().unwrap_or_default()
                    ),
                ));
            }
            self.owner_write(
                peer,
                computer,
                &format!("kuvo/peer-archive/{session}/{action}"),
                json!({"was":revision}),
            )
            .await?;
            Ok(())
        })
    }
}
