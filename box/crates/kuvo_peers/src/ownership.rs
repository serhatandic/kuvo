//! Owner routing and authenticated peer forwarding for session effects.
//! The engine owns request bodies and its durable submission receipts.

use std::{
    collections::{BTreeMap, HashSet},
    io,
    sync::Arc,
    time::Duration,
};

use kuvo_wire::{
    json::ordinary,
    keys::{decode_exact, encode},
    mail::Envelope,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::{
    devices::Devices,
    directory::{ComputerKey, RecordScope},
    parts::work_queue_mails,
    relay::{RelayEvent, RelayHandle},
    setup_lists::{Reader, View},
};

const MAX_SCOPED_BODY: usize = 128 * 1024;
const MAX_ADDRESSED_BODY: usize = 5 * 1024 * 1024;

/// The accepted setup actor supplies its view separately from peer reach.
/// A cached grant at a member may need a fresh verification after restart.
#[derive(Clone, Default)]
pub struct SetupState {
    pub id: Option<String>,
    pub replacements: BTreeMap<ComputerKey, ComputerKey>,
    pub grants_current: bool,
    pub grants_disabled: bool,
}

#[derive(Clone, Debug)]
pub struct PeerOwner {
    pub key: Option<ComputerKey>,
    pub name: String,
    pub local: bool,
    pub reachable: bool,
    pub features: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Scope {
    pub setup: Option<String>,
    pub here: Option<ComputerKey>,
    pub owner: Option<PeerOwner>,
    pub(crate) letter_home: Option<(ComputerKey, Option<crate::reach::Millis>)>,
    /// Letters can follow the confirmed destination even while cleanup fences the source.
    pub(crate) letter_held: bool,
    pub receipt_issuer: Option<ComputerKey>,
    pub held: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingState {
    Allowed,
    Revoked,
    Checking,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    Allowed,
    OwnerChanged,
    Revoked,
    Checking,
}

#[derive(Clone)]
pub struct Handle {
    reader: Reader,
    devices: Devices,
    relay: Option<RelayHandle>,
    accepted: Arc<dyn Fn() -> SetupState + Send + Sync>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub setup: String,
    pub session: String,
    pub operation: String,
    pub submitter: Submitter,
    pub action: Action,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Submitter {
    Computer,
    Paired {
        #[serde(with = "key_text")]
        key: ComputerKey,
        pairing: String,
    },
    Ssh {
        #[serde(with = "key_text")]
        key: ComputerKey,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    Submit {
        draft: Value,
    },
    Outcome {
        #[serde(with = "key_text")]
        issuer: ComputerKey,
        query: Value,
    },
    Deliver {
        message: Value,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

pub struct Incoming {
    peer: ComputerKey,
    request: Request,
    nonce: String,
}

impl Incoming {
    pub fn peer(&self) -> ComputerKey {
        self.peer
    }
    pub fn request(&self) -> &Request {
        &self.request
    }
}

pub struct IncomingRequests {
    handle: Handle,
    events: broadcast::Receiver<Arc<RelayEvent>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The relay isn’t connected.")]
    Offline,
    #[error("That computer isn’t one of yours.")]
    UnknownPeer,
    #[error("The accepted setup changed.")]
    SetupChanged,
    #[error("The owner request is invalid.")]
    Invalid,
    #[error("The owner message exceeds the relay limit.")]
    TooLarge,
    #[error("The other computer didn’t answer in time.")]
    Timeout,
    #[error("Checking ownership: {0}")]
    Storage(#[from] io::Error),
    #[error(transparent)]
    Relay(#[from] crate::relay::Error),
}

#[derive(Deserialize, Serialize)]
struct RequestPacket {
    nonce: String,
    #[serde(with = "key_text")]
    to: ComputerKey,
    request: Request,
}

#[derive(Deserialize, Serialize)]
struct ReplyPacket {
    nonce: String,
    #[serde(with = "key_text")]
    to: ComputerKey,
    setup: String,
    operation: String,
    reply: Reply,
}

impl Handle {
    pub fn new(
        reader: Reader,
        relay: Option<RelayHandle>,
        devices: Devices,
        accepted: impl Fn() -> SetupState + Send + Sync + 'static,
    ) -> Self {
        Self {
            reader,
            relay,
            devices,
            accepted: Arc::new(accepted),
        }
    }

    /// Read the durable move journal again before deciding where an effect can
    /// run. The accepted replacement is independent of the current live owner.
    pub fn scope(
        &self,
        session: &str,
        original_issuer: Option<ComputerKey>,
    ) -> Result<Scope, Error> {
        if !session_id(session) {
            return Err(Error::Invalid);
        }
        let setup = (self.accepted)();
        let view = self.reader.view();
        let mut inventory = view.local.ownership.clone();
        inventory.frozen.clear();
        inventory.journal.clear();
        inventory.replacements = resolved_replacements(&view, &setup);
        crate::moves::locate_checked(&self.reader.run, &mut inventory)?;
        if view.local.holds(session) {
            inventory
                .held
                .insert((RecordScope::Session, session.into()));
        }
        let now = self.reader.now();
        let owner = inventory.owner_of(
            RecordScope::Session,
            session,
            &view.roster,
            &view.announced,
            &view.reach,
            now,
        );
        let owner = owner.map(|owner| {
            let features = if owner.local {
                view.features().into_iter().map(str::to_owned).collect()
            } else {
                owner
                    .key
                    .and_then(|key| view.announced.get(&encode(&key)))
                    .and_then(|said| said.features.clone())
                    .unwrap_or_default()
            };
            PeerOwner {
                key: owner.key,
                name: owner.name,
                features,
                local: owner.local,
                reachable: owner.local || (view.relay_connected && owner.reachable),
            }
        });
        let held = inventory.frozen.contains(session);
        let retiring = inventory
            .journal
            .get(&(RecordScope::Session, session.into()))
            .is_some_and(|move_to| move_to.retiring);
        Ok(Scope {
            setup: setup.id,
            here: view.roster.here,
            owner,
            // A newer move announcement can locate a delivery without
            // granting authority to perform a forwarded mutation.
            letter_home: view
                .move_address(session, now, &inventory)
                .and_then(|(key, at)| Some((decode_exact(&key)?, at))),
            letter_held: held && !retiring,
            receipt_issuer: original_issuer
                .map(|key| *inventory.replacements.get(&key).unwrap_or(&key)),
            held,
        })
    }

    pub fn current_pairing(&self, key: &ComputerKey, pairing: &str) -> PairingState {
        let accepted = (self.accepted)();
        if accepted.grants_disabled {
            return PairingState::Revoked;
        }
        self.devices
            .forwarded_pairing(key, pairing, accepted.grants_current)
    }

    pub fn receipt_issuers(&self) -> BTreeMap<ComputerKey, ComputerKey> {
        resolved_replacements(&self.reader.view(), &(self.accepted)())
    }

    /// Known remote members in setup order, including computers that are offline.
    pub fn members(&self) -> Vec<PeerOwner> {
        if (self.accepted)().id.is_none() {
            return Vec::new();
        }
        let view = self.reader.view();
        let now = self.reader.now();
        view.roster
            .members
            .iter()
            .map(|peer| {
                let key = encode(&peer.key);
                let announcement = view.announced.get(&key);
                let heard = announcement.map(|said| said.heard.unwrap_or(said.at));
                PeerOwner {
                    key: Some(peer.key),
                    name: peer.name.clone(),
                    local: false,
                    reachable: view.relay_connected
                        && peer.address.is_some()
                        && !view.reach.peer_is_away(&key, heard, now),
                    features: announcement
                        .and_then(|said| said.features.clone())
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Keep receipt lookup available when the session moved. Only an approved
    /// replacement may stand in for the immutable issuer of that receipt.
    /// The effect itself still requires current ownership and a current grant.
    pub fn admit(&self, incoming: &Incoming) -> Result<Admission, Error> {
        let view = self.reader.view();
        if view.roster.member(&incoming.peer).is_none() {
            return Err(Error::UnknownPeer);
        }
        let issuer = match &incoming.request.action {
            Action::Outcome { issuer, .. } => Some(*issuer),
            _ => None,
        };
        let scope = self.scope(&incoming.request.session, issuer)?;
        if scope.setup.as_deref() != Some(incoming.request.setup.as_str()) {
            return Err(Error::SetupChanged);
        }
        let local = if issuer.is_some() {
            scope.here.is_some() && scope.receipt_issuer == scope.here
        } else {
            scope.owner.as_ref().is_some_and(|owner| owner.local) && !scope.held
        };
        if !local {
            return Ok(Admission::OwnerChanged);
        }
        Ok(match &incoming.request.submitter {
            Submitter::Computer => Admission::Allowed,
            Submitter::Paired { key, pairing } => match self.current_pairing(key, pairing) {
                PairingState::Allowed => Admission::Allowed,
                PairingState::Revoked => Admission::Revoked,
                PairingState::Checking => Admission::Checking,
            },
            Submitter::Ssh { key }
                if view.roster.here == Some(*key) || view.roster.member(key).is_some() =>
            {
                Admission::Allowed
            }
            Submitter::Ssh { .. } => Admission::Revoked,
        })
    }

    pub async fn request(
        &self,
        peer: ComputerKey,
        request: Request,
        wait: Duration,
    ) -> Result<Reply, Error> {
        request.validate()?;
        let relay = self.relay.as_ref().ok_or(Error::Offline)?;
        let mut events = relay.subscribe().ok_or(Error::Offline)?;
        let (here, host) = self.address(peer, &request.setup)?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let nonce = encode(&random);
        let packet = RequestPacket {
            nonce: nonce.clone(),
            to: peer,
            request,
        };
        let wait_reply = async {
            send(relay, host, "ownerRequest", &packet, self.reader.now()).await?;
            loop {
                let event = next(&mut events).await?;
                let RelayEvent::Mail { key, envelope, .. } = event.as_ref() else {
                    continue;
                };
                if *key != peer {
                    continue;
                }
                let Some(answer) = decode::<ReplyPacket>(envelope, "ownerReply") else {
                    continue;
                };
                if answer.nonce == nonce
                    && answer.to == here
                    && answer.setup == packet.request.setup
                    && answer.operation == packet.request.operation
                    && status(answer.reply.status)
                {
                    self.address(peer, &packet.request.setup)?;
                    return Ok(answer.reply);
                }
            }
        };
        tokio::time::timeout(wait, wait_reply)
            .await
            .map_err(|_| Error::Timeout)?
    }

    pub fn incoming(&self) -> Result<IncomingRequests, Error> {
        let events = self
            .relay
            .as_ref()
            .and_then(RelayHandle::subscribe)
            .ok_or(Error::Offline)?;
        Ok(IncomingRequests {
            handle: self.clone(),
            events,
        })
    }

    pub async fn reply(&self, incoming: &Incoming, reply: Reply) -> Result<(), Error> {
        if !status(reply.status) {
            return Err(Error::Invalid);
        }
        let relay = self.relay.as_ref().ok_or(Error::Offline)?;
        let (_, host) = self.address(incoming.peer, &incoming.request.setup)?;
        let packet = ReplyPacket {
            nonce: incoming.nonce.clone(),
            to: incoming.peer,
            setup: incoming.request.setup.clone(),
            operation: incoming.request.operation.clone(),
            reply,
        };
        send(relay, host, "ownerReply", &packet, self.reader.now()).await
    }

    fn address(
        &self,
        peer: ComputerKey,
        setup: &str,
    ) -> Result<(ComputerKey, kuvo_wire::keys::Host), Error> {
        if (self.accepted)().id.as_deref() != Some(setup) {
            return Err(Error::SetupChanged);
        }
        let view = self.reader.view();
        let here = view.roster.here.ok_or(Error::SetupChanged)?;
        let host = view
            .roster
            .member(&peer)
            .and_then(|member| member.host())
            .ok_or(Error::UnknownPeer)?;
        Ok((here, host))
    }

    /// The accepted member destination of an ordinary Work Queue action.
    pub fn work_queue_destination(
        &self,
        peer: ComputerKey,
    ) -> Result<(Arc<View>, String, kuvo_wire::keys::Host), Error> {
        let setup = (self.accepted)().id.ok_or(Error::SetupChanged)?;
        let (_, host) = self.address(peer, &setup)?;
        Ok((self.reader.view(), setup, host))
    }
}

impl IncomingRequests {
    pub async fn recv(&mut self) -> Result<Incoming, Error> {
        loop {
            let event = next(&mut self.events).await?;
            let RelayEvent::Mail { key, envelope, .. } = event.as_ref() else {
                continue;
            };
            let Some(packet) = decode::<RequestPacket>(envelope, "ownerRequest") else {
                continue;
            };
            if packet.request.validate().is_err() || decode_exact::<16>(&packet.nonce).is_none() {
                continue;
            }
            if self
                .handle
                .address(*key, &packet.request.setup)
                .is_ok_and(|(here, _)| here == packet.to)
            {
                return Ok(Incoming {
                    peer: *key,
                    request: packet.request,
                    nonce: packet.nonce,
                });
            }
        }
    }
}

fn resolved_replacements(view: &View, setup: &SetupState) -> BTreeMap<ComputerKey, ComputerKey> {
    let accepted = |key: &ComputerKey| {
        view.roster.here.as_ref() == Some(key) || view.roster.member(key).is_some()
    };
    setup
        .replacements
        .keys()
        .filter_map(|key| {
            if accepted(key) {
                return None;
            }
            let mut seen = HashSet::new();
            let mut issuer = key;
            while seen.insert(issuer) {
                issuer = setup.replacements.get(issuer)?;
                if accepted(issuer) {
                    return Some((*key, *issuer));
                }
            }
            None
        })
        .collect()
}

impl Request {
    fn validate(&self) -> Result<(), Error> {
        let valid = self
            .setup
            .strip_prefix("set_")
            .is_some_and(|token| decode_exact::<16>(token).is_some())
            && session_id(&self.session)
            && decode_exact::<32>(&self.operation).is_some()
            && match &self.submitter {
                Submitter::Paired { pairing, .. } => pairing.is_empty() || token(pairing, 32),
                _ => true,
            };
        if !valid {
            return Err(Error::Invalid);
        }
        let (body, max) = match &self.action {
            Action::Submit { draft } => (draft, MAX_SCOPED_BODY),
            Action::Deliver { message } => {
                if let Some(draft) = message.get("draft") {
                    (draft, MAX_ADDRESSED_BODY)
                } else if let Some(prompt) = message.get("prompt") {
                    (prompt, MAX_SCOPED_BODY)
                } else {
                    (message, MAX_ADDRESSED_BODY)
                }
            }
            Action::Outcome { .. } => return Ok(()),
        };
        if ordinary(body).len() > max {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}

pub(crate) fn session_id(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|id| token(id, 95))
}

fn token(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn status(status: u16) -> bool {
    (200..=599).contains(&status)
}

async fn next(events: &mut broadcast::Receiver<Arc<RelayEvent>>) -> Result<Arc<RelayEvent>, Error> {
    loop {
        match events.recv().await {
            Ok(event) => return Ok(event),
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return Err(Error::Offline),
        }
    }
}

async fn send(
    relay: &RelayHandle,
    host: kuvo_wire::keys::Host,
    field: &str,
    packet: &impl Serialize,
    at: i64,
) -> Result<(), Error> {
    let mails = work_queue_mails(json!({(field): packet}), at).ok_or(Error::TooLarge)?;
    for mail in mails {
        relay.sender.mail(host.clone(), mail).await?;
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(envelope: &Envelope, field: &str) -> Option<T> {
    if let Some(work_queue) = envelope.0.get("workQueue") {
        if work_queue.get("memory").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        return serde_json::from_value(work_queue.get(field)?.clone()).ok();
    }
    // The first Rust ownership interface used one top-level mail field.
    serde_json::from_value(envelope.0.get(field)?.clone()).ok()
}

mod key_text {
    use super::*;
    use serde::{Deserializer, Serializer};
    pub(super) fn serialize<S: Serializer>(
        key: &ComputerKey,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode(key))
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ComputerKey, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode_exact(&text).ok_or_else(|| serde::de::Error::custom("invalid computer key"))
    }
}
