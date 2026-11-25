//! Seat history and Insight pages use the same authenticated relay as limits.
//! No work is scheduled for an isolated computer, and heartbeats don't re-read
//! or price the source logs.
use kuvo_peers::{
    SeatParts,
    relay::{RelayEvent, RelayHandle},
    setup_lists::Reader,
    usage_seat_mails,
};
use kuvo_services::usage::{
    history_peers::Snapshot,
    insight_mail::{Cursor, Mail, Scope},
    insights::Service,
    limits,
};
use kuvo_setup::SetupHandle;
use kuvo_wire::keys::{Host, encode};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

#[derive(Default)]
struct Sent {
    digest: Option<String>,
    fallback_revision: Option<String>,
    at: i64,
    asked: i64,
    read_for: i64,
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Box<dyn std::error::Error>> + Send + 'static,
) -> Option<T> {
    match tokio::task::spawn_blocking(move || f().map_err(|e| e.to_string())).await {
        Ok(Ok(value)) => Some(value),
        error => {
            let note = match error {
                Ok(Err(error)) => error,
                Err(error) => error.to_string(),
                _ => unreachable!(),
            };
            eprintln!("Couldn't exchange usage: {note}");
            None
        }
    }
}
async fn send(relay: &RelayHandle, host: Host, payload: Value, at: i64) -> bool {
    let Some(frames) = usage_seat_mails(payload, at) else {
        return false;
    };
    for frame in frames {
        if relay.sender.mail(host.clone(), frame).await.is_err() {
            return false;
        }
    }
    true
}
async fn send_insight(relay: &RelayHandle, host: Host, mail: Mail, at: i64) -> bool {
    let mut body = serde_json::to_value(mail).expect("Insight metadata");
    body["kind"] = json!("insights");
    send(relay, host, body, at).await
}
async fn snapshot(service: &Service, now: i64) -> Option<Snapshot> {
    let service = service.clone();
    blocking(move || service.history_snapshot(now)).await
}
async fn ask(
    service: &Service,
    relay: &RelayHandle,
    id: &str,
    host: Host,
    refresh: bool,
    at: i64,
) -> bool {
    let service = service.clone();
    let id = id.to_owned();
    let Some(Some(cursor)) = blocking(move || service.peer_cursor(&id)).await else {
        return false;
    };
    send_insight(relay, host, Mail::Pull { cursor, refresh }, at).await
}
async fn page(
    service: &Service,
    relay: &RelayHandle,
    id: &str,
    host: Host,
    cursor: Cursor,
    remember: bool,
    at: i64,
) -> bool {
    let service = service.clone();
    let id = id.to_owned();
    let Some(Some(mail)) = blocking(move || service.peer_page(&id, cursor, remember)).await else {
        return false;
    };
    send_insight(relay, host, mail, at).await
}

pub async fn follow(
    service: Service,
    limits: limits::Service,
    mut directory: Reader,
    relay: RelayHandle,
    setup: SetupHandle,
) {
    let Some(mut inbox) = relay.subscribe() else {
        return;
    };
    let mut updates = service.subscribe_index();
    let mut manual = service.subscribe_manual();
    let mut limit_updates = limits.subscribe();
    let mut rosters = setup.rosters();
    let mut parts = SeatParts::default();
    let mut told = BTreeMap::<String, Sent>::new();
    let mut pulled = BTreeMap::<String, i64>::new();
    let mut connected = false;
    let mut last_scope = Scope::default();
    let mut last_identities = BTreeMap::<String, String>::new();
    let mut cached = None::<Snapshot>;
    let mut dirty = true;
    let mut tell_pages = true;
    let mut force = false;
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let view = directory.view();
        let now = limits::now() as i64;
        let scope_id = setup.config_receipt_scope().map(|(id, _)| id);
        let names = view
            .roster
            .members
            .iter()
            .map(|m| (encode(&m.key), m.name.clone()))
            .collect::<BTreeMap<_, _>>();
        let scope = Scope {
            setup: scope_id.clone(),
            own: view.roster.here.as_ref().map(|key| encode(key)),
            peers: if scope_id.is_some() {
                names
            } else {
                BTreeMap::new()
            },
            historical: setup.insight_origins(),
            connected: view
                .pending("usage.insights", |_| false)
                .into_iter()
                .map(|c| c.id)
                .collect(),
        };
        let hosts = view
            .roster
            .members
            .iter()
            .filter_map(|m| Some((encode(&m.key), m.host()?)))
            .filter(|(id, _)| scope.peers.contains_key(id))
            .collect::<BTreeMap<_, _>>();
        if scope != last_scope {
            let next = scope.clone();
            let worker = service.clone();
            let _ = blocking(move || worker.peer_roster(next)).await;
            if scope_id != last_scope.setup || scope.historical != last_scope.historical {
                told.clear();
                pulled.clear();
                tell_pages = true;
            }
            told.retain(|key, _| hosts.contains_key(key));
            pulled.retain(|key, _| hosts.contains_key(key));
            parts.retain(|key| hosts.contains_key(key));
            last_scope = scope;
        }
        if view.relay_connected && !connected {
            told.clear();
            pulled.clear();
            tell_pages = true;
        }
        connected = view.relay_connected;
        if connected && !hosts.is_empty() {
            let identities = limits
                .peer_snapshot(None, now as u64)
                .accounts
                .into_iter()
                .filter_map(|a| Some((a.provider, a.identity?)))
                .collect::<BTreeMap<_, _>>();
            if identities != last_identities {
                dirty = true;
                last_identities = identities;
            }
            if dirty || cached.is_none() {
                cached = snapshot(&service, now).await;
                dirty = false;
            }
            if let Some(snapshot) = &cached {
                for (id, host) in &hosts {
                    let sent = told.entry(id.clone()).or_default();
                    let mail = if force {
                        let mut mail = snapshot.heartbeat(now);
                        mail["refresh"] = json!(true);
                        Some(mail)
                    } else if sent.digest.is_none() {
                        Some(snapshot.full(now, true, false))
                    } else if sent.fallback_revision.as_ref() != Some(&snapshot.fallback_revision) {
                        Some(snapshot.full(now, false, false))
                    } else if sent.digest.as_ref() != Some(&snapshot.digest) {
                        Some(snapshot.recent(now))
                    } else if now.saturating_sub(sent.at) >= 600_000 {
                        Some(snapshot.heartbeat(now))
                    } else {
                        None
                    };
                    if let Some(mail) = mail {
                        if send(&relay, host.clone(), mail, now).await {
                            sent.digest = Some(snapshot.digest.clone());
                            sent.fallback_revision = Some(snapshot.fallback_revision.clone());
                            sent.at = now;
                        }
                    }
                }
            }
            for (id, host) in &hosts {
                // Cursor retry on a connection/roster change, while an initial
                // index is pending, or at the ordinary ten-minute heartbeat.
                let delay = if last_scope.connected.contains(id) {
                    300_000
                } else {
                    600_000
                };
                if force
                    || pulled
                        .get(id)
                        .is_none_or(|at| now.saturating_sub(*at) >= delay)
                {
                    if ask(&service, &relay, id, host.clone(), force, now).await {
                        pulled.insert(id.clone(), now);
                    }
                }
                if tell_pages {
                    let worker = service.clone();
                    let id_copy = id.clone();
                    if let Some(Some(cursor)) =
                        blocking(move || worker.served_cursor(&id_copy)).await
                    {
                        page(&service, &relay, id, host.clone(), cursor, false, now).await;
                    }
                }
            }
        }
        tell_pages = false;
        force = false;
        tokio::select! {
            changed=directory.changed()=>{if changed.is_err(){break;}},
            changed=rosters.changed()=>{if changed.is_err(){break;}},
            changed=updates.changed()=>{if changed.is_err(){break;}dirty=true;tell_pages=true;},
            changed=limit_updates.changed()=>{if changed.is_err(){break;}},
            request=manual.recv()=>{if request.is_ok(){force=true;dirty=true;tell_pages=true;}},
            _=interval.tick(),if !hosts.is_empty()=>{},
            event=inbox.recv()=>match event{
                Ok(event)=>if let RelayEvent::Mail{key,envelope,..}=event.as_ref(){
                    let id=encode(key);
                    let Some(host)=hosts.get(&id).cloned() else{continue;};
                    if !connected{continue;}
                    let Some(value)=parts.take(&id,envelope)else{continue;};
                    match value["kind"].as_str(){
                        Some("usage")=>{
                            let now=limits::now() as i64;
                            let ask_back=value["ask"]==true;
                            let refresh=value["refresh"]==true;
                            let worker=service.clone();let id_copy=id.clone();
                            let Some(Some(missing))=blocking(move||worker.take_history(&id_copy,&value,now)).await else{continue;};
                            let entry=told.entry(id.clone()).or_default();
                            if refresh&&now.saturating_sub(entry.read_for)>=60_000{
                                entry.read_for=now;
                                let worker=service.clone();
                                tokio::spawn(async move{worker.refresh_usage().await;});
                            }
                            if ask_back||missing&&now.saturating_sub(entry.asked)>=60_000{
                                if cached.is_none(){cached=snapshot(&service,now).await;}
                                if let Some(snapshot)=&cached{
                                    let ask=missing&&!ask_back;
                                    if ask{entry.asked=now;}
                                    if send(&relay,host,snapshot.full(now,ask,false),now).await{entry.digest=Some(snapshot.digest.clone());entry.fallback_revision=Some(snapshot.fallback_revision.clone());entry.at=now;}
                                }
                            }
                        }
                        Some("insights")=>{
                            let Ok(mail)=serde_json::from_value::<Mail>(value)else{continue;};
                            let now=limits::now() as i64;
                            match mail{
                                Mail::Pull{cursor,refresh}=>{
                                    page(&service,&relay,&id,host,cursor,true,now).await;
                                    if refresh{
                                        let entry=told.entry(id.clone()).or_default();
                                        if now.saturating_sub(entry.read_for)>=60_000{
                                            entry.read_for=now;
                                            let worker=service.clone();tokio::spawn(async move{worker.recount().await;});
                                        }
                                    }
                                }
                                mail@Mail::Page{..}=>{
                                    let worker=service.clone();let id_copy=id.clone();
                                    if let Some(Some(cursor))=blocking(move||worker.take_page(&id_copy,mail)).await{
                                        send_insight(&relay,host,Mail::Pull{cursor,refresh:false},now).await;
                                        pulled.insert(id,now);
                                    }
                                }
                            }
                        }
                        _=>{}
                    }
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{told.clear();pulled.clear();},
                Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
            }
        }
    }
}
