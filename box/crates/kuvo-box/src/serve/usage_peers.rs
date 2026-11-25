//! Plan snapshots on the authenticated peer-mail connection. Unlike log readers,
//! this only runs while a resident is linked, and never asks a provider on a timer.
use kuvo_peers::{
    relay::{RelayEvent, RelayHandle},
    setup_lists::Reader,
};
use kuvo_services::usage::{
    limits::{self, Service},
    limits_peers::Reading,
    resets::{PeerTransport, ResetMail, Resets},
};
use kuvo_setup::SetupHandle;
use kuvo_wire::{keys::encode, mail::Envelope};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, time::Duration};

struct Told {
    at: u64,
    contents: Value,
    checked: Vec<Option<u64>>,
    scope: Option<String>,
}

impl Told {
    fn wait_until(
        &self,
        contents: &Value,
        checked: &[Option<u64>],
        scope: Option<&str>,
        now: u64,
    ) -> Option<u64> {
        let refresh = self.at.saturating_add(600_000);
        if now >= refresh {
            return None;
        }
        if &self.contents == contents {
            return Some(refresh);
        }
        let throttle = self.at.saturating_add(60_000);
        (self.checked == checked && self.scope.as_deref() == scope && now < throttle)
            .then_some(throttle)
    }
}
pub(super) async fn at_deadline(at: Option<u64>) {
    if let Some(at) = at {
        tokio::time::sleep(Duration::from_millis(
            at.saturating_sub(limits::now()).max(1),
        ))
        .await;
    } else {
        std::future::pending::<()>().await;
    }
}

pub async fn follow_limits(
    service: Service,
    mut directory: Reader,
    relay: RelayHandle,
    setup: SetupHandle,
    resets: Resets,
) {
    let Some(mut mail) = relay.subscribe() else {
        return;
    };
    let mut changes = service.subscribe();
    let mut refreshes = service.remote_refreshes();
    let mut told = BTreeMap::<String, Told>::new();
    let mut answered = BTreeMap::<String, u64>::new();
    let mut reset_told = BTreeMap::<(String, String), (u64, u64)>::new();
    let mut connected = false;
    loop {
        let view = directory.view();
        let scope = setup.config_receipt_scope().map(|(id, _)| id);
        let names = view
            .roster
            .members
            .iter()
            .map(|m| (encode(&m.key), m.name.clone()))
            .collect::<BTreeMap<_, _>>();
        service.peer_roster(&names, scope.as_deref(), limits::now());
        told.retain(|id, _| names.contains_key(id));
        answered.retain(|id, _| names.contains_key(id));
        if view.relay_connected && !connected {
            told.clear();
            reset_told.clear();
        }
        reset_told.retain(|(computer, _), _| names.contains_key(computer));
        connected = view.relay_connected;
        let mut deadline = None;
        let mut wake = |at: u64| deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
        if connected {
            if let Some(at) = service.peer_expiry(limits::now()) {
                wake(at);
            }
            let decisions = resets.decisions().await;
            for member in &view.roster.members {
                let Some(host) = member.host() else {
                    continue;
                };
                let id = encode(&member.key);
                for decision in &decisions {
                    let mark = (id.clone(), decision.row.clone());
                    let now = limits::now();
                    if let Some((_, sent)) = reset_told.get(&mark).filter(|(at, sent)| {
                        *at == decision.at && now.saturating_sub(*sent) < 600_000
                    }) {
                        wake(sent.saturating_add(600_000));
                        continue;
                    }
                    if send_reset(&relay, host.clone(), ResetMail::decision(decision.clone())).await
                    {
                        let sent = limits::now();
                        reset_told.insert(mark, (decision.at, sent));
                        wake(sent.saturating_add(600_000));
                    } else {
                        wake(now.saturating_add(60_000));
                    }
                }
            }
        }
        if connected && !names.is_empty() {
            let at = limits::now();
            let snapshot = service.peer_snapshot(scope.clone(), at);
            let contents = json!([snapshot.accounts, snapshot.notice_setup]);
            let checked = snapshot
                .accounts
                .iter()
                .map(|a| a.facts.checked)
                .collect::<Vec<_>>();
            for member in &view.roster.members {
                let Some(host) = member.host() else {
                    continue;
                };
                let id = encode(&member.key);
                let old = told.get(&id);
                if let Some(when) =
                    old.and_then(|old| old.wait_until(&contents, &checked, scope.as_deref(), at))
                {
                    wake(when);
                    continue;
                }
                let mut snapshot = snapshot.clone();
                snapshot.ask = old.is_none();
                if send(&relay, host, snapshot).await {
                    told.insert(
                        id,
                        Told {
                            at,
                            contents: contents.clone(),
                            checked: checked.clone(),
                            scope: scope.clone(),
                        },
                    );
                    wake(at.saturating_add(600_000));
                } else {
                    wake(at.saturating_add(60_000));
                }
            }
        }
        tokio::select! {
            changed=changes.changed()=>{if changed.is_err(){break;}},
            changed=directory.changed()=>{if changed.is_err(){break;}},
            _=at_deadline(deadline)=>{service.answer(limits::now());},
            _=refreshes.recv()=>{
                let view=directory.view();
                let scope=setup.config_receipt_scope().map(|(id,_)|id);
                let now=limits::now();
                for (id,rows) in service.peer_refresh_targets(now){
                    if let Some(host)=view.roster.members.iter().find(|m|encode(&m.key)==id).and_then(|m|m.host()){
                        let mut snapshot=service.peer_snapshot(scope.clone(),now);
                        snapshot.refresh=rows;
                        send(&relay,host,snapshot).await;
                    }
                }
            },
            event=mail.recv()=>match event {
                Ok(event)=> if let RelayEvent::Mail{key,envelope,..}=event.as_ref() {
                    let view=directory.view();
                    let Some(member)=view.roster.member(key) else {continue;};
                    if let Some(value)=envelope.0.get("seat").filter(|value|value["kind"]=="reset") {
                        let Ok(given)=serde_json::from_value::<ResetMail>(value.clone()) else{continue;};
                        let from=encode(key);
                        if let Some(answer)=given.answer {resets.take_answer(&from,answer);}
                        else if let Some(decision)=given.always {if resets.take_decision(&from,decision).await.is_err(){eprintln!("Couldn't keep peer reset permission.");}}
                        else if let Some(ask)=given.ask && let Some(host)=member.host(){let resets=resets.clone();let relay=relay.clone();tokio::spawn(async move{let answer=resets.take_ask(&from,ask).await;send_reset(&relay,host,ResetMail::answer(answer)).await;});}
                        continue;
                    }
                    let Some(value)=envelope.0.get("limits") else {continue;};
                    let scope=setup.config_receipt_scope().map(|(id,_)|id);
                    let now=limits::now();
                    let Some(reading)=Reading::from(value, now, scope.as_deref()) else {continue;};
                    let id=encode(key);
                    let ask=reading.ask;
                    service.take_peer(&id,&member.name,reading,now);
                    if ask && answered.get(&id).is_none_or(|at|now.saturating_sub(*at)>=60_000) && let Some(host)=member.host() {
                        let snapshot=service.peer_snapshot(scope.clone(),now);
                        let contents=json!([snapshot.accounts,snapshot.notice_setup]);
                        let checked=snapshot.accounts.iter().map(|a|a.facts.checked).collect();
                        if send(&relay,host,snapshot).await {
                            answered.insert(id.clone(),now);
                            told.insert(id,Told{at:now,contents,checked,scope});
                        }
                    }
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>told.clear(),
                Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
            },
        }
    }
}

async fn send(relay: &RelayHandle, host: kuvo_wire::keys::Host, snapshot: Reading) -> bool {
    let envelope = Envelope(Map::from_iter([
        ("limits".into(), snapshot.wire()),
        ("at".into(), json!(snapshot.at)),
    ]));
    relay.sender.mail(host, envelope).await.is_ok()
}

#[derive(Clone)]
pub struct ResetTransport {
    pub directory: Reader,
    pub relay: RelayHandle,
}
impl PeerTransport for ResetTransport {
    fn known(&self) -> bool {
        self.directory.view().roster.here.is_some()
    }
    fn member(&self, key: &str) -> bool {
        self.directory
            .view()
            .roster
            .members
            .iter()
            .any(|m| encode(&m.key) == key)
    }
    fn reachable(&self, key: &str) -> bool {
        let view = self.directory.view();
        view.relay_connected
            && view
                .roster
                .members
                .iter()
                .any(|m| encode(&m.key) == key && m.host().is_some())
    }
    fn send<'a>(
        &'a self,
        key: &'a str,
        mail: ResetMail,
    ) -> futures_core::future::BoxFuture<'a, bool> {
        Box::pin(async move {
            let host = self
                .directory
                .view()
                .roster
                .members
                .iter()
                .find(|m| encode(&m.key) == key)
                .and_then(|m| m.host());
            match host {
                Some(host) => send_reset(&self.relay, host, mail).await,
                None => false,
            }
        })
    }
}
async fn send_reset(relay: &RelayHandle, host: kuvo_wire::keys::Host, mail: ResetMail) -> bool {
    let at = mail.at;
    let Ok(value) = serde_json::to_value(mail) else {
        return false;
    };
    let envelope = Envelope(Map::from_iter([
        ("seat".into(), value),
        ("at".into(), json!(at)),
    ]));
    relay.sender.mail(host, envelope).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_unchanged_peer_heartbeats_or_throttled_actual_changes_have_deadlines() {
        let told = Told {
            at: 1000,
            contents: json!(["old"]),
            checked: vec![Some(1000)],
            scope: Some("setup".into()),
        };
        assert_eq!(
            told.wait_until(&json!(["old"]), &[Some(1000)], Some("setup"), 2000),
            Some(601_000)
        );
        assert_eq!(
            told.wait_until(&json!(["changed"]), &[Some(1000)], Some("setup"), 2000),
            Some(61_000)
        );
        assert_eq!(
            told.wait_until(&json!(["changed"]), &[Some(2000)], Some("setup"), 2000),
            None
        );
        assert_eq!(
            told.wait_until(&json!(["changed"]), &[Some(1000)], Some("new-setup"), 2000),
            None
        );
        assert_eq!(
            told.wait_until(&json!(["old"]), &[Some(1000)], Some("setup"), 601_000),
            None
        );
    }
}
