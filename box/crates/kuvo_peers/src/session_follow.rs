//! Live, read-only conversation follows over authenticated peer mail. The owning computer serves
//! the allowlisted read; each subscription has a lease, so a vanished follower leaves no stream.
//! A follower listens to the relay only while someone follows; the owner answers only what it's
//! asked, and nothing runs when nobody follows.

use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use base64::Engine as _;
use bytes::Bytes;
use futures_util::StreamExt;
use http::{HeaderValue, Method, StatusCode, header::CONTENT_TYPE};
use http_body_util::BodyExt;
use kuvo_server::{
    Caller, DispatchRequest, Dispatcher, LocalOrigin, Refusal, Reply, Transport, WeakDispatcher,
};
use kuvo_wire::{keys::encode, mail::Envelope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    sync::{broadcast, mpsc},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    conversation,
    directory::{ComputerKey, Member},
    ownership::{self, PairingState},
    reach::Millis,
    relay::{RelayEvent, RelayHandle},
    setup_lists::{ReachAgain, Reader, View},
};

const LEASE: Duration = Duration::from_secs(35);
const RENEW: Duration = Duration::from_secs(10);
const MAX_STREAMS: usize = 128;
const CHUNK: usize = 32_000;
const MAX_BUFFER: usize = 4 * 1024 * 1024;
/// A newest page carried larger than this isn't kept.
const MOST_KEPT_PAGE: usize = 16 * 1024 * 1024;
pub const FEATURE: &str = "line.peer-conversation";
const UNAVAILABLE: &str = "The conversation’s computer isn’t answering.";
const NO_COMPUTER: &str = "There’s no such computer in this setup.";
const ELSEWHERE: &str = "Open the conversation on its current computer.";
const GONE: &str = "That conversation isn’t there any more.";

/// What of a conversation can be followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Location,
    Session,
    Messages,
    Events,
    Queue,
    QueueEvents,
    Asks,
    AsksEvents,
}

impl Resource {
    const ALL: [Resource; 8] = [
        Resource::Location,
        Resource::Session,
        Resource::Messages,
        Resource::Events,
        Resource::Queue,
        Resource::QueueEvents,
        Resource::Asks,
        Resource::AsksEvents,
    ];

    fn path(self) -> &'static str {
        match self {
            Resource::Location => "location",
            Resource::Session => "session",
            Resource::Messages => "messages",
            Resource::Events => "events",
            Resource::Queue => "queue",
            Resource::QueueEvents => "queue/events",
            Resource::Asks => "asks",
            Resource::AsksEvents => "asks/events",
        }
    }

    fn parse(path: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|resource| resource.path() == path)
    }
}

/// Who a read is for, as the owner checks it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Client {
    Computer,
    Paired { key: String, pairing: String },
}

#[derive(Default, Deserialize, Serialize)]
struct Part {
    sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    end: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    beat: bool,
}

fn mail(id: &str, contents: Value) -> Envelope {
    let mut follow = json!({"id": id});
    if let (Some(follow), Value::Object(contents)) = (follow.as_object_mut(), contents) {
        follow.extend(contents);
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    let mut fields = serde_json::Map::new();
    fields.insert("at".into(), at.into());
    fields.insert("sessionFollow".into(), follow);
    Envelope(fields)
}

fn follow_of(event: &RelayEvent) -> Option<(&ComputerKey, &serde_json::Map<String, Value>)> {
    let RelayEvent::Mail { key, envelope, .. } = event else {
        return None;
    };
    let follow = envelope.0.get("sessionFollow")?.as_object()?;
    let id = follow.get("id")?.as_str()?;
    (id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')).then_some((key, follow))
}

fn subscription_id() -> io::Result<String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    random[6] = (random[6] & 0x0f) | 0x40;
    random[8] = (random[8] & 0x3f) | 0x80;
    let hex: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn refused(status: StatusCode, message: &str) -> Reply {
    Refusal::new(status, message.to_owned()).reply()
}

fn local_caller() -> Caller {
    Caller::Local(Transport::Http {
        origin: LocalOrigin::App,
        internal: true,
    })
}

async fn read_here(dispatcher: &Dispatcher, path: String, cancel: CancellationToken) -> Reply {
    dispatcher
        .dispatch(
            DispatchRequest {
                method: Method::GET,
                path,
                headers: Default::default(),
                body: Bytes::new(),
                caller: local_caller(),
            },
            cancel,
        )
        .await
}

/// A conversation's read on the computer that owns it: from its own list for where it is, and
/// from its engine for its pages and events (those of this conversation only).
pub async fn own_conversation(
    reader: &Reader,
    dispatcher: &Dispatcher,
    session: &str,
    resource: Resource,
    query: &str,
    cancel: CancellationToken,
) -> Reply {
    if !conversation::valid_session(session) {
        return refused(StatusCode::NOT_FOUND, GONE);
    }
    let view = reader.view();
    let with_tools = query_value(query, "tools").as_deref() == Some("1");
    let place = match crate::routes::location(&view, &reader.run, session, with_tools, reader.now()) {
        Ok(found) => match serde_json::to_value(&found) {
            Ok(value) if value.get("place").is_some() => value,
            Ok(_) => return refused(StatusCode::CONFLICT, ELSEWHERE),
            Err(error) => return refused(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        },
        Err(refusal) => return refusal.reply(),
    };
    let read = |path: String| read_here(dispatcher, path, cancel.clone());
    match resource {
        Resource::Location => Reply::json(StatusCode::OK, place),
        Resource::Queue => read(format!("kuvo/sessions/{session}/queue?items=1")).await,
        Resource::QueueEvents | Resource::Asks | Resource::AsksEvents => {
            read(format!("kuvo/sessions/{session}/{}", resource.path())).await
        }
        Resource::Session | Resource::Messages | Resource::Events => {
            let prefix = if place["place"] == "agents" { "kuvo/agents/" } else { "" };
            match resource {
                Resource::Events => scoped_events(read(format!("{prefix}api/event")).await, session),
                Resource::Session => read(format!("{prefix}api/session/{session}")).await,
                _ => read(format!("{prefix}api/session/{session}/message?{query}")).await,
            }
        }
    }
}

fn query_value(query: &str, key: &str) -> Option<String> {
    form_urlencoded::parse(query.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

/// Only this conversation's events, whole, from the engine's stream of all of them.
fn scoped_events(reply: Reply, session: &str) -> Reply {
    if reply.status != StatusCode::OK {
        return reply;
    }
    let session = session.to_owned();
    let headers = reply.headers;
    let body = reply.body.into_data_stream();
    let blocks = futures_util::stream::unfold(
        (body, Vec::<u8>::new(), false),
        move |(mut body, mut rest, done)| {
            let session = session.clone();
            async move {
                loop {
                    if let Some((block, after)) = next_block(&rest) {
                        let kept = event_for(&rest[..block], &session).then(|| {
                            let mut out = rest[..block].to_vec();
                            out.extend_from_slice(b"\n\n");
                            Bytes::from(out)
                        });
                        rest.drain(..after);
                        if let Some(kept) = kept {
                            return Some((Ok(kept), (body, rest, done)));
                        }
                        continue;
                    }
                    if done {
                        return None;
                    }
                    match body.next().await {
                        Some(Ok(chunk)) => rest.extend_from_slice(&chunk),
                        Some(Err(error)) => return Some((Err(io::Error::other(error)), (body, rest, true))),
                        None => return None,
                    }
                    if rest.len() > MAX_BUFFER {
                        return Some((
                            Err(io::Error::other("The conversation event is too large.")),
                            (body, Vec::new(), true),
                        ));
                    }
                }
            }
        },
    );
    let mut scoped = Reply::stream(StatusCode::OK, "text/event-stream", blocks);
    scoped.headers = headers;
    scoped
}

/// Where the next whole event block ends, and where the one after it starts.
fn next_block(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|pair| pair == b"\n\n").map(|at| (at, at + 2));
    let crlf = bytes.windows(4).position(|four| four == b"\r\n\r\n").map(|at| (at, at + 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (found, None) | (None, found) => found,
    }
}

fn event_for(block: &[u8], session: &str) -> bool {
    let text = String::from_utf8_lossy(block);
    let lines: Vec<&str> = text.split('\n').map(|line| line.trim_end_matches('\r')).collect();
    let body = lines
        .iter()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if body.is_empty() {
        return lines.iter().all(|line| line.starts_with(':'));
    }
    let Ok(event) = serde_json::from_str::<Value>(&body) else {
        return false;
    };
    let data = event
        .get("data")
        .filter(|data| data.is_object())
        .or_else(|| event.get("properties").filter(|data| data.is_object()));
    let Some(data) = data else {
        return false;
    };
    data.get("sessionID")
        .or_else(|| data.get("info").and_then(|info| info.get("id")))
        .and_then(Value::as_str)
        == Some(session)
}

/// Following another computer's conversations from here.
#[derive(Clone)]
pub struct Follows {
    reader: Reader,
    relay: Option<RelayHandle>,
    reach_again: ReachAgain,
    open: Arc<AtomicUsize>,
}

impl Follows {
    pub fn reach_again(&self) -> &ReachAgain {
        &self.reach_again
    }

    pub fn new(reader: Reader, relay: Option<RelayHandle>, reach_again: ReachAgain) -> Self {
        Self {
            reader,
            relay,
            reach_again,
            open: Arc::default(),
        }
    }

    /// The read `resource` of `session` on `computer` (`here`, or this computer's own key, is a
    /// read here).
    pub async fn read(
        &self,
        dispatcher: Dispatcher,
        caller: &Caller,
        computer: &str,
        session: &str,
        resource: Resource,
        query: &str,
    ) -> Reply {
        let view = self.reader.view();
        let own = view.roster.here.map(|key| encode(&key));
        if computer == "here" || own.as_deref() == Some(computer) {
            return own_conversation(
                &self.reader,
                &dispatcher,
                session,
                resource,
                query,
                CancellationToken::new(),
            )
            .await;
        }
        let client = match caller {
            Caller::Paired { pairing, .. } if pairing.is_current() => Client::Paired {
                key: encode(&pairing.key),
                pairing: pairing.id.clone(),
            },
            Caller::Local(Transport::Http {
                origin: LocalOrigin::App | LocalOrigin::Mac,
                ..
            }) => Client::Computer,
            _ => return refused(StatusCode::FORBIDDEN, "Open the conversation from Kuvo Code."),
        };
        let Some(peer) = view
            .roster
            .members
            .iter()
            .find(|peer| encode(&peer.key) == computer)
            .filter(|_| conversation::valid_session(session))
            .cloned()
        else {
            return refused(StatusCode::NOT_FOUND, NO_COMPUTER);
        };
        if view
            .announced
            .get(computer)
            .is_none_or(|said| !said.supports(FEATURE))
        {
            return refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "Update Kuvo Code on that computer to follow it from here.",
            );
        }
        // The newest page is kept as it's carried, and answered from there while its computer
        // is away (asked again meanwhile, so the next read finds it as soon as it answers).
        let newest = resource == Resource::Messages && query_value(query, "cursor").is_none();
        let welcomed = view.relay_connected && self.relay.is_some();
        if newest && (!welcomed || view.reach.away_since(computer).is_some()) {
            let trying = view.reaching.get(&peer.key).is_some_and(|at| {
                (0..crate::manage::TRY_FOR).contains(&self.reader.now().saturating_sub(*at))
            });
            if welcomed && !trying {
                self.reach_again.ask(peer.key);
            }
            return self.last_copy(&view, &peer, session).await;
        }
        let (Some(relay), Some(host)) = (self.relay.as_ref(), peer.host()) else {
            return refused(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE);
        };
        if !welcomed || self.open.load(Ordering::SeqCst) >= MAX_STREAMS {
            return refused(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE);
        }
        let Some(events) = relay.subscribe() else {
            return refused(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE);
        };
        let Ok(id) = subscription_id() else {
            return refused(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE);
        };
        let opened = Opened {
            follows: self.clone(),
            relay: relay.clone(),
            host,
            peer: peer.clone(),
            session: session.to_owned(),
            id,
        };
        self.open.fetch_add(1, Ordering::SeqCst);
        let resource_path = format!(
            "kuvo/session-read/{session}/{}{}",
            resource.path(),
            if query.is_empty() { String::new() } else { format!("?{query}") }
        );
        let open = json!({"open": {"resource": resource_path, "client": client}});
        let sent = relay.sender.mail(opened.host.clone(), mail(&opened.id, open)).await;
        let unanswered = if sent.is_ok() {
            match opened.head(events, newest).await {
                Ok(answer) => return answer,
                Err(unanswered) => *unanswered,
            }
        } else {
            opened
        };
        unanswered.stop().await;
        if newest {
            self.last_copy(&self.reader.view(), &peer, session).await
        } else {
            refused(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE)
        }
    }

    /// The newest page kept of `session`, said as its computer's last copy while it's away.
    async fn last_copy(&self, view: &View, peer: &Member, session: &str) -> Reply {
        let run = self.reader.run.clone();
        let (key, id) = (peer.key, session.to_owned());
        let kept = tokio::task::spawn_blocking(move || conversation::load(&run, &key, &id))
            .await
            .ok()
            .flatten();
        let standing = view.standing(peer.key, self.reader.now());
        let seen = standing
            .away()
            .then(|| {
                standing
                    .last_seen()
                    .unwrap_or_default()
                    .max(kept.as_ref().map_or(0, |page| page.at))
            })
            .filter(|seen| *seen != 0);
        let mut away = json!({"computer": peer.computer()});
        if let Some(seen) = seen {
            away["lastSeen"] = seen.into();
        }
        if standing.updating() == Some(true) {
            away["updating"] = true.into();
        }
        Reply::json(
            StatusCode::OK,
            json!({"data": kept.map_or_else(Vec::new, |page| page.data), "away": away}),
        )
    }
}

/// One follow under way: its subscription on the owner, while it lasts.
struct Opened {
    follows: Follows,
    relay: RelayHandle,
    host: kuvo_wire::keys::Host,
    peer: Member,
    session: String,
    id: String,
}

enum Heard {
    Part(Part),
    Over,
    Nothing,
}

impl Opened {
    fn still_allowed(&self) -> bool {
        let view = self.follows.reader.view();
        view.relay_connected && view.roster.member(&self.peer.key).is_some()
    }

    async fn stop(self) {
        let _ = self
            .relay
            .sender
            .mail(self.host.clone(), mail(&self.id, json!({"stop": true})))
            .await;
        self.follows.open.fetch_sub(1, Ordering::SeqCst);
    }

    /// What came next from the owner for this follow, keeping it while it waits.
    async fn hear(
        &self,
        events: &mut broadcast::Receiver<Arc<RelayEvent>>,
        heard: &mut Instant,
        renew: &mut tokio::time::Interval,
        received: bool,
    ) -> Heard {
        loop {
            tokio::select! {
                _ = renew.tick() => {
                    let kept = self.relay.sender.mail(self.host.clone(), mail(&self.id, json!({"keep": true}))).await;
                    if !self.still_allowed() || heard.elapsed() >= LEASE || kept.is_err() {
                        return Heard::Over;
                    }
                }
                event = events.recv() => match event {
                    Ok(event) => {
                        if let RelayEvent::Unreachable(address) = event.as_ref() {
                            // The relay found nothing there: what's kept is answered now.
                            if *address == self.host.address && !received {
                                return Heard::Over;
                            }
                            continue;
                        }
                        let Some((key, follow)) = follow_of(&event) else { continue };
                        if *key != self.peer.key || follow.get("id").and_then(Value::as_str) != Some(&self.id) {
                            continue;
                        }
                        let Some(part) = follow.get("part").and_then(|part| serde_json::from_value::<Part>(part.clone()).ok()) else {
                            continue;
                        };
                        *heard = Instant::now();
                        return Heard::Part(part);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => return Heard::Over,
                    Err(broadcast::error::RecvError::Closed) => return Heard::Nothing,
                },
            }
        }
    }

    /// The answer's head once it comes, its body following as it's carried; none when no answer
    /// came (or, for the newest page, the owner's engine isn't answering yet).
    async fn head(
        self,
        mut events: broadcast::Receiver<Arc<RelayEvent>>,
        newest: bool,
    ) -> Result<Reply, Box<Self>> {
        let mut heard = Instant::now();
        let mut renew = tokio::time::interval_at(Instant::now() + RENEW, RENEW);
        let mut sequence = 0;
        let (status, kind) = loop {
            let Heard::Part(part) = self.hear(&mut events, &mut heard, &mut renew, false).await else {
                return Err(Box::new(self));
            };
            if part.sequence != sequence {
                return Err(Box::new(self));
            }
            sequence += 1;
            if part.beat {
                continue;
            }
            let (Some(status), Some(kind)) = (part.status, part.kind) else {
                return Err(Box::new(self));
            };
            if newest && status >= 500 {
                return Err(Box::new(self));
            }
            break (status, kind);
        };
        let (tx, rx) = mpsc::channel::<io::Result<Bytes>>(64);
        let queued = Arc::new(AtomicUsize::new(0));
        let keep = (newest && status == 200).then(Vec::new);
        tokio::spawn(self.carry(events, heard, renew, sequence, tx, queued.clone(), keep));
        let body = futures_util::stream::unfold((rx, queued), |(mut rx, queued)| async move {
            let piece = rx.recv().await?;
            if let Ok(bytes) = &piece {
                queued.fetch_sub(bytes.len(), Ordering::SeqCst);
            }
            Some((piece, (rx, queued)))
        });
        let mut reply = Reply::stream(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            "application/json",
            body,
        );
        if let Ok(kind) = HeaderValue::from_str(&kind) {
            reply.headers.insert(CONTENT_TYPE, kind);
        }
        Ok(reply)
    }

    #[allow(clippy::too_many_arguments)]
    async fn carry(
        self,
        mut events: broadcast::Receiver<Arc<RelayEvent>>,
        mut heard: Instant,
        mut renew: tokio::time::Interval,
        mut sequence: u64,
        tx: mpsc::Sender<io::Result<Bytes>>,
        queued: Arc<AtomicUsize>,
        mut copy: Option<Vec<u8>>,
    ) {
        let mut whole = false;
        loop {
            let part = tokio::select! {
                () = tx.closed() => break,
                heard = self.hear(&mut events, &mut heard, &mut renew, true) => heard,
            };
            let Heard::Part(part) = part else { break };
            if part.sequence != sequence {
                break;
            }
            sequence += 1;
            if let Some(body) = part.body.as_deref() {
                let Ok(body) = base64::engine::general_purpose::STANDARD.decode(body) else {
                    break;
                };
                if queued.fetch_add(body.len(), Ordering::SeqCst) + body.len() > MAX_BUFFER {
                    break;
                }
                if let Some(kept) = copy.as_mut() {
                    if kept.len() + body.len() <= MOST_KEPT_PAGE {
                        kept.extend_from_slice(&body);
                    } else {
                        copy = None;
                    }
                }
                if tx.send(Ok(Bytes::from(body))).await.is_err() {
                    break;
                }
            }
            if part.end {
                whole = true;
                break;
            }
        }
        // Kept before the answer ends, so the next read finds it.
        if let Some(page) = copy.filter(|_| whole) {
            self.keep(page).await;
        }
        drop(tx);
        self.stop().await;
    }

    async fn keep(&self, page: Vec<u8>) {
        let Some(data) = serde_json::from_slice::<Value>(&page)
            .ok()
            .and_then(|page| page.get("data")?.as_array().cloned())
        else {
            return;
        };
        let run = self.follows.reader.run.clone();
        let key = self.peer.key;
        let session = self.session.clone();
        let at: Millis = self.follows.reader.now();
        let _ = tokio::task::spawn_blocking(move || {
            conversation::keep(&run, &key, &session, conversation::Page { at, data })
        })
        .await;
    }
}

/// The owner's side: what others follow of this computer's conversations, each while it's kept.
pub fn serve(
    reader: Reader,
    relay: RelayHandle,
    dispatcher: WeakDispatcher,
    owners: ownership::Handle,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut events = relay.subscribe()?;
    Some(tokio::spawn(async move {
        let serving: Arc<Mutex<HashMap<(ComputerKey, String), Serving>>> = Arc::default();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            let Some((key, follow)) = follow_of(&event) else { continue };
            if follow.contains_key("part") {
                continue;
            }
            let view = reader.view();
            let Some(peer) = view.roster.member(key).cloned() else { continue };
            let Some(host) = peer.host() else { continue };
            let id = follow["id"].as_str().unwrap_or_default().to_owned();
            let slot = (peer.key, id.clone());
            let existing = serving.lock().map(|map| map.get(&slot).cloned()).unwrap_or_default();
            if follow.get("stop") == Some(&Value::Bool(true)) {
                if let Some(existing) = existing {
                    existing.end.cancel();
                }
                continue;
            }
            if follow.get("keep") == Some(&Value::Bool(true)) {
                if let Some(existing) = existing
                    && let Ok(mut renewed) = existing.renewed.lock()
                {
                    *renewed = Instant::now();
                }
                continue;
            }
            if existing.is_some() {
                continue;
            }
            let Some((session, resource, query, client)) = read_of(follow.get("open")) else {
                continue;
            };
            let allowed = match &client {
                Client::Computer => true,
                Client::Paired { key, pairing } => kuvo_wire::keys::decode_exact(key)
                    .is_some_and(|key| owners.current_pairing(&key, pairing) == PairingState::Allowed),
            };
            let full = serving.lock().map_or(true, |map| map.len() >= MAX_STREAMS);
            let Some(dispatcher) = dispatcher.upgrade().filter(|_| allowed && !full) else {
                let status = if allowed { 503 } else { 403 };
                let message = json!({"message": "This conversation can’t be followed from here."});
                let body = base64::engine::general_purpose::STANDARD.encode(message.to_string());
                let part = json!({"part": {"sequence": 0, "status": status, "type": "application/json", "body": body, "end": true}});
                let _ = relay.sender.mail(host, mail(&id, part)).await;
                continue;
            };
            let state = Serving {
                end: CancellationToken::new(),
                renewed: Arc::new(Mutex::new(Instant::now())),
            };
            if let Ok(mut map) = serving.lock() {
                map.insert(slot.clone(), state.clone());
            }
            let answer = Answer {
                reader: reader.clone(),
                relay: relay.clone(),
                host,
                peer: peer.key,
                id,
                sequence: 0,
                state,
            };
            let serving = serving.clone();
            tokio::spawn(async move {
                answer.run(dispatcher, session, resource, query).await;
                if let Ok(mut map) = serving.lock() {
                    map.remove(&slot);
                }
            });
        }
    }))
}

#[derive(Clone)]
struct Serving {
    end: CancellationToken,
    renewed: Arc<Mutex<Instant>>,
}

fn read_of(open: Option<&Value>) -> Option<(String, Resource, String, Client)> {
    let open = open?.as_object()?;
    let resource = open.get("resource")?.as_str().filter(|text| text.len() <= 2048)?;
    let (path, query) = resource.split_once('?').unwrap_or((resource, ""));
    let rest = path.strip_prefix("kuvo/session-read/")?;
    let (session, what) = rest.split_once('/')?;
    if !conversation::valid_session(session) {
        return None;
    }
    let client = serde_json::from_value::<Client>(open.get("client")?.clone()).ok()?;
    Some((session.to_owned(), Resource::parse(what)?, query.to_owned(), client))
}

/// One answer on its way to the follower, in parts, while it's kept.
struct Answer {
    reader: Reader,
    relay: RelayHandle,
    host: kuvo_wire::keys::Host,
    peer: ComputerKey,
    id: String,
    sequence: u64,
    state: Serving,
}

impl Answer {
    fn live(&self) -> bool {
        let view = self.reader.view();
        !self.state.end.is_cancelled()
            && view.relay_connected
            && view.roster.member(&self.peer).is_some()
    }

    async fn send(&mut self, part: Part) -> bool {
        let mut value = serde_json::to_value(&part).unwrap_or_default();
        value["sequence"] = self.sequence.into();
        self.sequence += 1;
        let sent = self
            .relay
            .sender
            .mail(self.host.clone(), mail(&self.id, json!({"part": value})))
            .await
            .is_ok();
        if !sent {
            self.state.end.cancel();
        }
        sent
    }

    async fn run(mut self, dispatcher: Dispatcher, session: String, resource: Resource, query: String) {
        let end = self.state.end.clone();
        let reply = tokio::select! {
            () = end.cancelled() => return,
            reply = own_conversation(&self.reader, &dispatcher, &session, resource, &query, end.clone()) => reply,
        };
        let kind = reply
            .headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_owned();
        let mut body = reply.body.into_data_stream();
        let mut beat = tokio::time::interval_at(Instant::now() + RENEW, RENEW);
        if self
            .send(Part {
                status: Some(reply.status.as_u16()),
                kind: Some(kind),
                ..Part::default()
            })
            .await
        {
            loop {
                tokio::select! {
                    () = end.cancelled() => break,
                    _ = beat.tick() => {
                        let renewed = self.state.renewed.lock().map(|at| at.elapsed()).unwrap_or(LEASE);
                        if !self.live() || renewed >= LEASE || !self.send(Part { beat: true, ..Part::default() }).await {
                            break;
                        }
                    }
                    piece = body.next() => match piece {
                        Some(Ok(bytes)) => {
                            let mut sent = true;
                            for chunk in bytes.chunks(CHUNK) {
                                let body = base64::engine::general_purpose::STANDARD.encode(chunk);
                                sent = self.live() && self.send(Part { body: Some(body), ..Part::default() }).await;
                                if !sent {
                                    break;
                                }
                            }
                            if !sent {
                                break;
                            }
                        }
                        // A bad source ends this opening: the line reopens it.
                        Some(Err(_)) | None => break,
                    },
                }
            }
        }
        if self.reader.view().relay_connected {
            self.send(Part { end: true, ..Part::default() }).await;
        }
    }
}
