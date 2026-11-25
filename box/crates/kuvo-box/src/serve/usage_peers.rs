//! Plan snapshots on the authenticated peer-mail connection. Unlike log readers,
//! this only runs while a resident is linked, and never asks a provider on a timer.
use kuvo_peers::{
    relay::{RelayEvent, RelayHandle},
    setup_lists::Reader,
};
use kuvo_services::usage::{
    limits::{self, Service},
    limits_peers::Reading,
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

pub async fn follow_limits(
    service: Service,
    mut directory: Reader,
    relay: RelayHandle,
    setup: SetupHandle,
) {
    let Some(mut mail) = relay.subscribe() else {
        return;
    };
    let mut changes = service.subscribe();
    let mut refreshes = service.remote_refreshes();
    let mut told = BTreeMap::<String, Told>::new();
    let mut answered = BTreeMap::<String, u64>::new();
    let mut connected = false;
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
        }
        connected = view.relay_connected;
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
                if old.is_some_and(|old| {
                    at.saturating_sub(old.at) < 600_000
                        && (old.contents == contents
                            || (old.checked == checked
                                && old.scope == scope
                                && at.saturating_sub(old.at) < 60_000))
                }) {
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
                }
            }
        }
        tokio::select! {
            changed=changes.changed()=>{if changed.is_err(){break;}},
            changed=directory.changed()=>{if changed.is_err(){break;}},
            _=tick.tick()=>{},
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
                    let Some(value)=envelope.0.get("limits") else {continue;};
                    let scope=setup.config_receipt_scope().map(|(id,_)|id);
                    let now=limits::now();
                    let Some(reading)=Reading::from(value, now, scope.as_deref()) else {continue;};
                    let id=encode(key);
                    let ask=reading.ask;
                    service.take_peer(&id,&member.name,reading,now);
                    if ask && answered.get(&id).is_none_or(|at|now.saturating_sub(*at)>=60_000) {
                        if let Some(host)=member.host() {
                            let snapshot=service.peer_snapshot(scope.clone(),now);
                            let contents=json!([snapshot.accounts,snapshot.notice_setup]);
                            let checked=snapshot.accounts.iter().map(|a|a.facts.checked).collect();
                            if send(&relay,host,snapshot).await {
                                answered.insert(id.clone(),now);
                                told.insert(id,Told{at:now,contents,checked,scope});
                            }
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
