//! The TypeScript forwarded backup messages ride in authenticated peer mail.
//! The resident correlates replies in memory because it owns both endpoints.
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_peers::{
    relay::{RelayEvent, RelayHandle, Sender},
    setup_lists::Reader,
};
use kuvo_wire::{
    documents::PART_SIZE,
    keys::{Host, encode},
    mail::{Envelope, MAX_PLAINTEXT},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{runtime::Handle, sync::broadcast, task::JoinHandle};

use super::{Mail, Method, REFUSAL, Receiver, Reply, Request, Scope};
use crate::backup::{Error, Result, Source, random, url_token};
use crate::{
    SetupHandle,
    actor::{Command, LINK},
    model::{Link, SharedKind},
    storage::Files,
};

pub const FEATURE: &str = "box.backups.relay";
const EXPIRES: Duration = Duration::from_secs(60);
const RETRY: Duration = Duration::from_secs(5);
const MAX_PARTS: usize = 16;
const MAX_ASSEMBLIES: usize = 32;

#[derive(Deserialize, Serialize)]
struct Act {
    id: String,
    area: String,
    method: Method,
    item: String,
    body: Value,
    at: i64,
}

impl Act {
    fn clock_refusal(&self, now: i64, own: &str, other: &str) -> Option<Reply> {
        (now.saturating_sub(self.at) > 600_000 || self.at.saturating_sub(now) > 60_000)
            .then(|| Reply {
                status: 409,
                value: json!({ "message": format!("{own}'s clock and {other}'s are too far apart to take it: check both computers' time.") }),
            })
    }
}

#[cfg(test)]
#[test]
fn relay_backup_rejects_a_bad_clock_at_both_edges() {
    let now = 1_795_046_400_000;
    let incoming = |at| {
        serde_json::from_value::<Act>(json!({
            "id": "wqa_00000000000000000000", "area": "backup",
            "method": "GET", "item": "b_aaaaaaaaaaaaaaaaaaaaaa", "body": {}, "at": at,
        }))
        .unwrap()
        .clock_refusal(now, "Receiving", "Sending")
    };
    for at in [now - 600_001, now + 60_001, i64::MIN, i64::MAX] {
        let reply = incoming(at).unwrap();
        assert_eq!(reply.status, 409);
        assert_eq!(
            reply.value,
            json!({
                "message": "Receiving's clock and Sending's are too far apart to take it: check both computers' time."
            })
        );
    }
    for at in [now - 600_000, now, now + 60_000] {
        assert!(incoming(at).is_none());
    }
}

#[derive(Deserialize, Serialize)]
struct Acted {
    id: String,
    #[serde(flatten)]
    reply: Reply,
}

#[derive(Deserialize, Serialize)]
struct Part {
    whole: String,
    i: usize,
    of: usize,
    d: String,
    s: String,
}

struct Assembly {
    deadline: Instant,
    parts: Vec<Option<String>>,
}

#[derive(Default)]
struct Assemblies(BTreeMap<([u8; 32], String), Assembly>);

impl Assemblies {
    fn take(&mut self, from: [u8; 32], value: Value) -> Option<Value> {
        let Some(part) = value.get("part") else {
            return Some(value);
        };
        let part: Part = serde_json::from_value(part.clone()).ok()?;
        if part.i >= part.of
            || !(2..=MAX_PARTS).contains(&part.of)
            || !part.s.is_empty()
            || !url_token(&part.whole, 43)
            || part.d.is_empty()
            || part.d.len() > PART_SIZE
            || !url_token(&part.d, part.d.len())
        {
            return None;
        }
        let now = Instant::now();
        self.0.retain(|_, assembly| assembly.deadline > now);
        let key = (from, part.whole);
        if !self.0.contains_key(&key) && self.0.len() >= MAX_ASSEMBLIES {
            return None;
        }
        let assembly = self.0.entry(key.clone()).or_insert_with(|| Assembly {
            deadline: now + EXPIRES,
            parts: vec![None; part.of],
        });
        if assembly.parts.len() != part.of {
            return None;
        }
        assembly.parts[part.i] = Some(part.d);
        if assembly.parts.iter().any(Option::is_none) {
            return None;
        }
        let complete: String = self.0.remove(&key)?.parts.into_iter().flatten().collect();
        if digest(&complete) != key.1 {
            return None;
        }
        let decoded = URL_SAFE_NO_PAD.decode(complete).ok()?;
        serde_json::from_slice::<Value>(&decoded)
            .ok()
            .filter(|value| value.is_object() && value.get("part").is_none())
    }
}

fn digest(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

async fn send(sender: &Sender, host: &Host, payload: Value) -> Result<()> {
    let now = jiff::Timestamp::now().as_millisecond();
    let envelope = |value| {
        Envelope(super::super::super::object(
            json!({ "at": now, "workQueue": value }),
        ))
    };
    let one = envelope(payload.clone());
    let send = |mail| sender.mail(host.clone(), mail);
    let online = |error: kuvo_peers::relay::Error| Error::Detail(error.to_string());
    if one.encode().len() <= MAX_PLAINTEXT {
        send(one).await.map_err(online)?;
        return Ok(());
    }
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?);
    let parts = encoded.len().div_ceil(PART_SIZE);
    let whole = digest(&encoded);
    if parts > MAX_PARTS {
        return Err(Error::Message("The encrypted backup chunk is too large."));
    }
    for (i, d) in encoded.as_bytes().chunks(PART_SIZE).enumerate() {
        let part = Part {
            whole: whole.clone(),
            i,
            of: parts,
            d: String::from_utf8_lossy(d).into_owned(),
            s: String::new(),
        };
        send(envelope(json!({ "part": part })))
            .await
            .map_err(online)?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct PeerMail {
    relay: RelayHandle,
    peers: Reader,
    runtime: Handle,
}

impl PeerMail {
    pub(crate) fn view(&self) -> Arc<kuvo_peers::setup_lists::View> {
        self.peers.view()
    }

    pub fn available(&self, to: &str) -> bool {
        kuvo_wire::keys::decode_exact(to).is_some_and(|key| {
            let view = self.peers.view();
            view.roster
                .member(&key)
                .and_then(|member| member.host())
                .is_some()
                && view.advertises(&key, FEATURE)
        })
    }

    pub(crate) fn approval(
        &self,
        primary: &str,
        request: &crate::backup::boundary::Approval,
    ) -> Result<Reply> {
        self.runtime.block_on(self.ask(
            primary,
            "ownership",
            Method::Get,
            "backup-restore",
            serde_json::to_value(request)?,
        ))
    }

    async fn ask(
        &self,
        to: &str,
        area: &str,
        method: Method,
        item: &str,
        body: Value,
    ) -> Result<Reply> {
        if !self.available(to) {
            return Err(Error::Message(
                "Choose an enrolled computer that supports Relay backups.",
            ));
        }
        let key = kuvo_wire::keys::decode_exact(to).expect("available key was validated");
        let view = self.peers.view();
        let host = view
            .roster
            .member(&key)
            .and_then(|member| member.host())
            .ok_or(Error::Message(REFUSAL))?;
        let mut events = self
            .relay
            .subscribe()
            .ok_or(Error::Message("The relay is offline."))?;
        let id = format!("wqa_{}", URL_SAFE_NO_PAD.encode(random::<16>()?));
        let payload = json!({ "act": Act {
            id: id.clone(), area: area.into(), method, item: item.into(), body,
            at: jiff::Timestamp::now().as_millisecond(),
        }});
        let name = view
            .roster
            .member(&key)
            .expect("recipient still in this roster")
            .name
            .clone();
        let exchange = async {
            let mut retry = tokio::time::interval(RETRY);
            retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut parts = Assemblies::default();
            loop {
                tokio::select! {
                    _ = retry.tick() => send(&self.relay.sender, &host, payload.clone()).await?,
                    event = events.recv() => match event {
                        Ok(event) => match &*event {
                            RelayEvent::Mail { from, key: sender, envelope } if *sender == host.key && *from == host.address => {
                                if let Some(value) = envelope.0.get("workQueue").cloned().and_then(|value| parts.take(key, value))
                                    && let Some(answer) = value.get("acted")
                                    && let Ok(acted) = serde_json::from_value::<Acted>(answer.clone())
                                    && acted.id == id
                                {
                                    return Ok(acted.reply);
                                }
                            }
                            RelayEvent::Unreachable(from) | RelayEvent::Gone { address: from, .. } if *from == host.address =>
                                return Err(Error::Detail(format!("{name} stopped answering, so this computer can't tell whether it was done there. Look again once it's back."))),
                            _ => (),
                        },
                        Err(broadcast::error::RecvError::Lagged(_)) => (),
                        Err(broadcast::error::RecvError::Closed) => return Err(Error::Message("The relay is offline.")),
                    },
                }
            }
        };
        tokio::time::timeout(EXPIRES, exchange).await
            .map_err(|_| Error::Detail(format!("{name} didn't answer in time, so this computer can't tell whether it was done there. Look again in a moment.")))?
    }
}

impl Mail for PeerMail {
    fn request(
        &self,
        to: &str,
        method: Method,
        source: &Source,
        request: Request,
    ) -> Result<Reply> {
        // Called from the backup service's blocking filesystem worker, never
        // from the relay task that delivers its replies.
        self.runtime.block_on(self.ask(
            to,
            "backup",
            method,
            &source.to_string(),
            serde_json::to_value(request)?,
        ))
    }
}

struct Accepted {
    files: Files,
    setup: SetupHandle,
}

impl Scope for Accepted {
    fn owners(
        &self,
        sender: &str,
        setup: &str,
        source: &Source,
        deposit: bool,
    ) -> Result<Vec<String>> {
        let deny = || Error::Message(REFUSAL);
        let local = self
            .setup
            .rosters()
            .borrow()
            .as_ref()
            .and_then(|roster| roster.here.as_ref().map(|key| encode(key)))
            .ok_or_else(deny)?;
        let link: Link = self
            .files
            .read(LINK)
            .map_err(|_| deny())?
            .ok_or_else(deny)?;
        let current = link.setup().map_err(|_| deny())?;
        if current.position.id != setup
            || current
                .extra
                .get("conflict")
                .is_some_and(|found| !found.is_null() && found != &Value::Bool(false))
        {
            return Err(deny());
        }
        let membership = link.member_list().map_err(|_| deny())?;
        let enrolled = |key: &str| {
            membership.boxes.iter().any(|member| member.key == key)
                && !membership.left.iter().any(|left| left == key)
        };
        if !enrolled(sender) || !enrolled(&local) || sender == local {
            return Err(deny());
        }
        let mut owners = vec![sender.to_owned()];
        if !deposit {
            for old in &membership.left {
                let key = format!("replacement:{old}");
                let assigned = [
                    SharedKind::OwnershipRecovery,
                    SharedKind::OwnershipAssignments,
                ]
                .into_iter()
                .find_map(|kind| {
                    link.shared
                        .get(&kind)
                        .and_then(|entry| entry.value.get(&key))
                });
                if let Some(assigned) = assigned
                    && !enrolled(old)
                    && assigned.get("computer").and_then(Value::as_str) == Some(sender)
                    && assigned
                        .get("source")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name == source.0)
                {
                    owners.push(old.clone());
                }
            }
        }
        Ok(owners)
    }
}

pub struct Receiving(JoinHandle<()>);
impl Drop for Receiving {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn start(
    home: PathBuf,
    account: PathBuf,
    setup: SetupHandle,
    peers: Reader,
    relay: RelayHandle,
) -> Option<(PeerMail, Receiving)> {
    let mut events = relay.subscribe()?;
    let receiver = Arc::new(Receiver::new(
        account,
        Accepted {
            files: Files::new(home),
            setup: setup.clone(),
        },
    ));
    let net = PeerMail {
        relay: relay.clone(),
        peers: peers.clone(),
        runtime: Handle::current(),
    };
    let task = tokio::spawn(async move {
        let mut parts = Assemblies::default();
        let mut pending = HashMap::new();
        // At most a receiver's four active uploads plus concurrent reads. The
        // storage lock is the final arbiter of simultaneous operations.
        let (completed, mut completions) = tokio::sync::mpsc::channel(16);
        loop {
            tokio::select! {
                Some(finished) = completions.recv() => { pending.remove(&finished); }
                event = events.recv() => {
                    let event = match event {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let RelayEvent::Mail { from, key, envelope } = &*event else { continue };
                    let view = peers.view();
                    let Some(host) = view.roster.member(key).and_then(|member| member.host()).filter(|host| host.address == *from) else { continue };
                    let Some(payload) = envelope.0.get("workQueue").cloned().and_then(|value| parts.take(*key, value)) else { continue };
                    let Some(act) = payload.get("act").and_then(|act| serde_json::from_value::<Act>(act.clone()).ok())
                        .filter(|act| (act.area == "backup" || act.area == "ownership" && act.method == Method::Get && act.item == "backup-restore") && act.id.strip_prefix("wqa_")
                            .is_some_and(|id| (16..=40).contains(&id.len()) && url_token(id, id.len()))) else { continue };
                    let own = view.roster.name.clone();
                    let other = view.roster.member(key).expect("member matches host").name.clone();
                    let flight = (*key, act.id.clone());
                    if pending.contains_key(&flight) || pending.len() >= 16 { continue; }
                    let (receiver, sender, done) = (receiver.clone(), relay.sender.clone(), completed.clone());
                    let who = encode(key);
                    let setup = setup.clone();
                    let finished = flight.clone();
                    let task = tokio::spawn(async move {
                        let now = jiff::Timestamp::now().as_millisecond();
                        let reply = if let Some(refusal) = act.clock_refusal(now, &own, &other) {
                            refusal
                        } else {
                            apply_message(receiver, setup, who, act.area, act.method, act.item, act.body).await
                        };
                        let _ = send(&sender, &host, json!({ "acted": Acted { id: act.id, reply } })).await;
                        let _ = done.send(finished).await;
                    });
                    pending.insert(flight, task);
                }
            }
        }
        for (_, task) in pending {
            task.abort();
        }
    });
    Some((net, Receiving(task)))
}

async fn apply_message(
    receiver: Arc<Receiver<Accepted>>,
    setup: SetupHandle,
    sender: String,
    area: String,
    method: Method,
    item: String,
    body: Value,
) -> Reply {
    let unavailable = || Reply {
        status: 503,
        value: json!({ "message": "The receiving computer couldn’t use the backup." }),
    };
    if area == "ownership" {
        let Ok(request) = serde_json::from_value(body) else {
            return unavailable();
        };
        return match setup
            .request(Command::BackupApproval { sender, request })
            .await
        {
            Ok(answer) => Reply {
                status: answer.status,
                value: answer.value,
            },
            Err(_) => unavailable(),
        };
    }
    let (Ok(source), Ok(body)) = (Source::try_from(item), serde_json::from_value(body)) else {
        return Reply {
            status: 400,
            value: json!({ "message": "Choose a backup source and point." }),
        };
    };
    tokio::task::spawn_blocking(move || {
        receiver.apply(&sender, method, &source, body, SystemTime::now())
    })
    .await
    .unwrap_or_else(|_| unavailable())
}
