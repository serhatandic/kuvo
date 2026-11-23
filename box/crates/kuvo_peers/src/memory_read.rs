//! An authenticated Work Queue read with no outbox, reply files, or reach changes.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    io,
    sync::Arc,
    time::Duration,
};

use kuvo_wire::{
    keys::{Address, Host, decode_exact, encode},
    mail::Envelope,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::{
    sync::{broadcast, oneshot},
    task::JoinSet,
    time::Instant,
};

use crate::{
    directory::{ComputerKey, RecordScope},
    relay::{RelayEvent, RelayHandle},
    setup_lists::Reader,
};

pub const FEATURE: &str = "work-queue.forward.memory";
pub const SEARCH_FEATURE: &str = "search.setup";
const FENCE: &[u8] = b"kuvo-fence:";
const MAX_ACTIVE: usize = 16;
const MAX_RECENT: usize = 2_000;

#[derive(Clone)]
pub struct Handle {
    reader: Reader,
    relay: Option<RelayHandle>,
    setup: Arc<dyn Fn() -> Option<String> + Send + Sync>,
}

#[derive(Clone, Debug)]
pub struct Read {
    pub area: Option<String>,
    pub item: String,
    pub kind: Kind,
    pub body: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    Record,
    File { name: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Answer {
    pub status: u16,
    #[serde(default)]
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Ready,
    Away,
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The relay isn't connected.")]
    Offline,
    #[error("That computer isn't one of yours.")]
    UnknownPeer,
    #[error("The accepted setup changed.")]
    SetupChanged,
    #[error("That isn't a valid read request.")]
    Invalid,
    #[error("The read exceeds the relay mail limit.")]
    TooLarge,
    #[error("The other computer didn't answer the read.")]
    Timeout,
    #[error("The other computer isn't reachable.")]
    Unreachable,
    #[error(transparent)]
    Storage(#[from] io::Error),
    #[error(transparent)]
    Relay(#[from] crate::relay::Error),
}

#[derive(Deserialize, Serialize)]
struct Act {
    id: String,
    at: i64,
    method: String,
    item: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Map<String, Value>>,
}

#[derive(Deserialize, Serialize)]
struct Acted {
    id: String,
    #[serde(flatten)]
    answer: Answer,
}

impl Handle {
    pub fn new(
        reader: Reader,
        relay: Option<RelayHandle>,
        setup: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            reader,
            relay,
            setup: Arc::new(setup),
        }
    }

    pub fn standing(&self, peer: ComputerKey, feature: &str) -> Standing {
        let view = self.reader.view();
        if view
            .roster
            .member(&peer)
            .is_none_or(|member| member.address.is_none())
        {
            return Standing::Away;
        }
        let key = encode(&peer);
        if !view.relay_connected || view.reach.away_since(&key).is_some() {
            let said = view.announced.get(&key);
            if view
                .reach
                .standing_of(
                    &key,
                    true,
                    said.and_then(|value| value.heard),
                    said.and_then(crate::directory::Announcement::release),
                    self.reader.now(),
                )
                .away()
            {
                return Standing::Away;
            }
            return Standing::Unavailable;
        }
        if view.advertises(&peer, feature) && view.advertises(&peer, FEATURE) {
            Standing::Ready
        } else {
            Standing::Unavailable
        }
    }

    /// The same durable destinations used by lists. Unrelated duplicate
    /// inventory rows are left for the search merge to settle.
    pub fn moved_sessions(&self, sessions: &[&str]) -> BTreeMap<String, ComputerKey> {
        let view = self.reader.view();
        let mut ownership = view.local.ownership.clone();
        crate::moves::locate(&self.reader.run, &mut ownership);
        let now = self.reader.now();
        sessions
            .iter()
            .filter_map(|&id| {
                let record = (RecordScope::Session, id.to_owned());
                let key = ownership
                    .journal
                    .get(&record)
                    .map(|key| *ownership.replacements.get(key).unwrap_or(key))
                    .or_else(|| view.moved(id, now).and_then(|moved| decode_exact(&moved)))?;
                Some((record.1, key))
            })
            .collect()
    }

    pub fn missed(&self, peer: ComputerKey) -> Standing {
        if self
            .reader
            .view()
            .reach
            .away_since(&encode(&peer))
            .is_some()
        {
            Standing::Away
        } else {
            Standing::Unavailable
        }
    }

    fn destination(&self, peer: ComputerKey, setup: &str) -> Result<Host, Error> {
        if (self.setup)().as_deref() != Some(setup) {
            return Err(Error::SetupChanged);
        }
        self.reader
            .view()
            .roster
            .member(&peer)
            .and_then(|member| member.host())
            .ok_or(Error::UnknownPeer)
    }

    pub async fn request(&self, peer: ComputerKey, read: Read) -> Result<Answer, Error> {
        let wait = read.wait();
        let relay = self.relay.as_ref().ok_or(Error::Offline)?;
        let mut events = relay.subscribe().ok_or(Error::Offline)?;
        let setup = (self.setup)().ok_or(Error::SetupChanged)?;
        let host = self.destination(peer, &setup)?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let id = format!("wqa_{}", encode(&random));
        let at = self.reader.now();
        let act = Act::new(id.clone(), at, read);
        if !act.valid() {
            return Err(Error::Invalid);
        }
        let mails = crate::parts::memory_work_queue_mails(json!({"act": act}), at)
            .ok_or(Error::TooLarge)?;
        let answer = async {
            let (could_not_deliver, mut failed) = oneshot::channel();
            relay
                .sender
                .memory_mail(host, mails, Some(could_not_deliver))
                .await?;
            let mut watching_delivery = true;
            loop {
                let event = tokio::select! {
                    result = &mut failed, if watching_delivery => {
                        if result.is_ok() {
                            return Err(Error::Unreachable);
                        }
                        watching_delivery = false;
                        continue;
                    }
                    event = receive(&mut events) => event?,
                };
                if let Some(answer) = self.check_reply(&event, peer, &id, &setup)? {
                    return Ok(answer);
                }
            }
        };
        tokio::time::timeout(wait, answer)
            .await
            .map_err(|_| Error::Timeout)?
    }

    fn check_reply(
        &self,
        event: &RelayEvent,
        peer: ComputerKey,
        id: &str,
        setup: &str,
    ) -> Result<Option<Answer>, Error> {
        let RelayEvent::Mail { key, envelope, .. } = event else {
            return Ok(None);
        };
        if *key != peer {
            return Ok(None);
        }
        let Some(acted) = packet::<Acted>(envelope, "acted") else {
            return Ok(None);
        };
        if acted.id != id || !(200..=599).contains(&acted.answer.status) {
            return Ok(None);
        }
        self.destination(peer, setup)?;
        Ok(Some(acted.answer))
    }

    /// Each service selects the read operations it accepts. No method other
    /// than GET reaches the handler, even from an authenticated computer.
    pub fn serve<F, Fut>(&self, handler: F) -> Option<tokio::task::JoinHandle<()>>
    where
        F: Fn(ComputerKey, Read) -> Fut + Send + 'static,
        Fut: Future<Output = Answer> + Send + 'static,
    {
        let events = self.relay.as_ref().and_then(RelayHandle::subscribe)?;
        let handle = self.clone();
        Some(tokio::spawn(async move {
            handle.serve_events(events, handler).await
        }))
    }

    async fn serve_events<F, Fut>(
        &self,
        mut events: broadcast::Receiver<Arc<RelayEvent>>,
        handler: F,
    ) where
        F: Fn(ComputerKey, Read) -> Fut + Send,
        Fut: Future<Output = Answer> + Send + 'static,
    {
        let mut active = JoinSet::new();
        let mut recent = HashMap::<(ComputerKey, String), Instant>::new();
        loop {
            let event = tokio::select! {
                result = active.join_next(), if !active.is_empty() => {
                    if let Some(Ok(key)) = result {
                        recent.insert(key, Instant::now() + Duration::from_secs(4));
                    }
                    continue;
                }
                event = receive(&mut events), if active.len() < MAX_ACTIVE => match event {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            let RelayEvent::Mail { key, envelope, .. } = event.as_ref() else {
                continue;
            };
            let Some(act) = packet::<Act>(envelope, "act").filter(Act::valid) else {
                continue;
            };
            let Some(setup) = (self.setup)() else {
                continue;
            };
            if self.destination(*key, &setup).is_err() {
                continue;
            }
            let now = Instant::now();
            recent.retain(|_, deadline| *deadline > now);
            let key = (*key, act.id.clone());
            if recent.contains_key(&key) || recent.len() == MAX_RECENT {
                continue;
            }
            let at = self.reader.now();
            let sent = act.at;
            let read = act.into_read();
            let wait = read.wait();
            recent.insert(key.clone(), now + wait + Duration::from_secs(4));
            let view = self.reader.view();
            let clock = at.saturating_sub(sent) > 600_000 || sent.saturating_sub(at) > 60_000;
            let future = (!clock).then(|| handler(key.0, read));
            let handle = self.clone();
            active.spawn(async move {
                let answer = match future {
                    Some(read) => match tokio::time::timeout(wait, read).await {
                        Ok(answer) => answer,
                        Err(_) => return key,
                    },
                    None => Answer {
                        status: 409,
                        value: json!({"message": format!(
                            "{}'s clock and {}'s are too far apart to take it: check both computers' time.",
                            view.roster.name, view.roster.member(&key.0).map_or("", |member| &member.name)
                        )}),
                    },
                };
                let _ = handle.send_reply(key.0, &setup, &key.1, answer).await;
                key
            });
        }
    }

    async fn send_reply(
        &self,
        peer: ComputerKey,
        setup: &str,
        id: &str,
        answer: Answer,
    ) -> Result<(), Error> {
        let relay = self.relay.as_ref().ok_or(Error::Offline)?;
        let host = self.destination(peer, setup)?;
        let payload = json!({"acted": Acted { id: id.to_owned(), answer }});
        let mails = crate::parts::memory_work_queue_mails(payload, self.reader.now())
            .ok_or(Error::TooLarge)?;
        relay.sender.memory_mail(host, mails, None).await?;
        Ok(())
    }
}

impl Read {
    fn wait(&self) -> Duration {
        Duration::from_secs(
            if matches!(self.kind, Kind::File { .. }) || self.area.as_deref() == Some("backup") {
                60
            } else {
                20
            },
        )
    }
}

impl Act {
    fn new(id: String, at: i64, read: Read) -> Self {
        let (action, name) = match read.kind {
            Kind::Record => (None, None),
            Kind::File { name } => (Some("file".to_owned()), Some(name)),
        };
        Self {
            id,
            at,
            method: "GET".to_owned(),
            item: read.item,
            area: read.area,
            action,
            name,
            body: read.body,
        }
    }

    fn valid(&self) -> bool {
        let short = |text: &str| (1..=300).contains(&text.encode_utf16().count());
        let id = self.id.strip_prefix("wqa_").is_some_and(|id| {
            (16..=40).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        });
        id && self.method == "GET"
            && short(&self.item)
            && match self.action.as_deref() {
                None => true,
                Some("file") => self.name.as_deref().is_some_and(short),
                _ => false,
            }
    }

    fn into_read(self) -> Read {
        let kind = if self.action.as_deref() == Some("file") {
            Kind::File {
                name: self.name.unwrap_or_default(),
            }
        } else {
            Kind::Record
        };
        Read {
            area: self.area,
            item: self.item,
            kind,
            body: self.body,
        }
    }
}

pub(crate) fn is_memory(envelope: &Envelope) -> bool {
    envelope
        .0
        .get("workQueue")
        .and_then(|queue| queue["memory"].as_bool())
        == Some(true)
}

fn packet<T: serde::de::DeserializeOwned>(envelope: &Envelope, name: &str) -> Option<T> {
    if !is_memory(envelope) {
        return None;
    }
    let value = envelope.0.get("workQueue")?.get(name)?;
    if value
        .get("formatVersion")
        .is_some_and(|version| !matches!(version.as_u64(), Some(0 | 1)))
    {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

async fn receive(
    events: &mut broadcast::Receiver<Arc<RelayEvent>>,
) -> Result<Arc<RelayEvent>, Error> {
    loop {
        match events.recv().await {
            Ok(event) => return Ok(event),
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return Err(Error::Offline),
        }
    }
}

type Failure = Option<oneshot::Sender<()>>;
type Delivery = (Address, Failure);

/// Relay failures are ordered with its self-deliveries. The relay retains the
/// reader's failure sender only between the echoes for that read.
#[derive(Default)]
pub(crate) struct Fences {
    next: u64,
    pending: HashMap<u64, Option<Delivery>>,
    active: Option<Delivery>,
}

impl Fences {
    pub(crate) fn pair(&mut self, to: Address, fail: Failure) -> (Vec<u8>, Vec<u8>) {
        (self.mark(Some((to, fail))), self.mark(None))
    }

    fn mark(&mut self, value: Option<Delivery>) -> Vec<u8> {
        self.next = self.next.wrapping_add(1);
        self.pending.insert(self.next, value);
        [FENCE, format!("{:016x}", self.next).as_bytes()].concat()
    }

    pub(crate) fn echo(&mut self, payload: &[u8]) {
        if let Some(mark) = payload
            .strip_prefix(FENCE)
            .and_then(|rest| std::str::from_utf8(rest).ok())
            .and_then(|text| u64::from_str_radix(text, 16).ok())
            && let Some(delivery) = self.pending.remove(&mark)
        {
            self.active = delivery;
        }
    }

    pub(crate) fn unreachable(&mut self, address: Address) -> bool {
        if let Some((to, failure)) = &mut self.active
            && *to == address
        {
            if let Some(failure) = failure.take() {
                let _ = failure.send(());
            }
            return true;
        }
        false
    }
}
