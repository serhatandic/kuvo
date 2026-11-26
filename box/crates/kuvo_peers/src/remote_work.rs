//! Work calls share the resident relay. The engine supplies local work facts and owns effects.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    time::Duration,
};

use kuvo_wire::{
    keys::{decode_exact, encode},
    mail::Envelope,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot, Notify},
    task::JoinHandle,
    time::Instant,
};

use crate::{
    calls::{self, Answer, Calls, Home, Outgoing, PeerState, ReceivedAnswer, RemoteWork, Request},
    directory::{ComputerKey, RecordScope},
    outbox::{Delivered, Id},
    reach::Millis,
    relay::{RelayEvent, RelayHandle},
    setup_lists::{Reader, View},
    work_rows::{RowsByLead, Snapshots},
};

const WAIT: Duration = Duration::from_secs(20);
const RETRY_FAILED: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub enum Outcome {
    Answer(Answer),
    Failed(String),
    /// `unreachable`: the relay said that computer isn't there (else it didn't answer in time).
    Away {
        kept: bool,
        sent: bool,
        unreachable: bool,
    },
    Offline {
        kept: bool,
    },
    Older,
}

struct Offer {
    key: ComputerKey,
    lead: String,
    request: Request,
    keep: bool,
    quiet: bool,
    reply: oneshot::Sender<io::Result<Outcome>>,
    cancelled: Arc<AtomicBool>,
}

enum Command {
    Call(Box<Offer>),
    Rows(RowsByLead),
}

struct Waiting {
    id: Id,
    key: ComputerKey,
    until: Instant,
    keep: bool,
    reply: oneshot::Sender<io::Result<Outcome>>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct WorkHandle {
    send: mpsc::Sender<Command>,
    closed: Arc<Notify>,
}

struct CallDropped { cancelled: Arc<AtomicBool>, closed: Arc<Notify> }
impl Drop for CallDropped {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.closed.notify_one();
    }
}

impl WorkHandle {
    /// The caller supplies its locally resolved lead and rules. Use `keep` for mutations whose
    /// answer can go to the lead's queue when the calling tool has gone away.
    pub async fn call(
        &self,
        key: ComputerKey,
        lead: String,
        request: Request,
        keep: bool,
        quiet: bool,
    ) -> io::Result<Outcome> {
        let (reply, wait) = oneshot::channel();
        // A dropped tool request wakes its owner without checking every waiter on a timer.
        let cancelled = Arc::new(AtomicBool::new(false));
        let _closed = CallDropped { cancelled: cancelled.clone(), closed: self.closed.clone() };
        self.send
            .send(Command::Call(Box::new(Offer {
                key,
                lead,
                request,
                keep,
                quiet,
                reply,
                cancelled,
            })))
            .await
            .map_err(|_| stopped())?;
        wait.await.map_err(|_| stopped())?
    }

    /// Publish a complete engine work snapshot. After an engine read error keep its previous
    /// snapshot. An empty successful snapshot clears the rows last told.
    pub async fn publish(&self, rows: RowsByLead) -> io::Result<()> {
        self.send
            .send(Command::Rows(rows))
            .await
            .map_err(|_| stopped())
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "Remote work has stopped")
}

pub(crate) fn start(
    reader: Reader,
    relay: RelayHandle,
    work: Arc<dyn RemoteWork>,
) -> io::Result<(WorkHandle, JoinHandle<()>)> {
    let events = relay.subscribe().ok_or_else(stopped)?;
    let (send, offers) = mpsc::channel(16);
    let closed = Arc::new(Notify::new());
    let calls = Calls::open(reader.run.as_ref());
    let snapshots = Snapshots::open(reader.run.as_ref());
    let owner = Owner {
        reader,
        relay,
        work,
        calls,
        snapshots,
        waiting: BTreeMap::new(),
        away: BTreeSet::new(),
    };
    Ok((WorkHandle { send, closed: closed.clone() }, tokio::spawn(owner.run(events, offers, closed))))
}

struct Owner {
    reader: Reader,
    relay: RelayHandle,
    work: Arc<dyn RemoteWork>,
    calls: Calls,
    snapshots: Snapshots,
    waiting: BTreeMap<String, Waiting>,
    away: BTreeSet<ComputerKey>,
}

impl Owner {
    async fn run(
        mut self,
        mut events: broadcast::Receiver<Arc<RelayEvent>>,
        mut offers: mpsc::Receiver<Command>,
        closed: Arc<Notify>,
    ) {
        let hub = kuvo_os::watch::WatchHub::default();
        let mut files = kuvo_os::watch::FileSet::new(hub.clone());
        let mut records = kuvo_os::watch::FileSet::new(hub);
        let run = self.reader.run.as_ref();
        files.set_targets(
            vec![run.join("work/homes.json"), run.join("moves/moved.json"), run.join("test-setup-clock.json")],
            vec![(run.join("outbox/sending"), false)],
        );
        records.set_targets(Vec::new(), vec![(run.join("work/call-results"), false), (run.join("work/call-answers"), false)]);
        let mut view = self.reader.view();
        let mut next = self.recover().await;
        loop {
            let relevant = tokio::select! {
                offer = offers.recv() => match offer {
                    Some(Command::Call(offer)) => {
                        if let Err(error) = self.offer(&self.reader.view(), self.reader.now(), *offer) {
                            eprintln!("Remote work: {error}");
                        }
                        true
                    }
                    Some(Command::Rows(rows)) => { self.snapshots.update(rows); true }
                    None => return,
                },
                event = events.recv() => match event {
                    Ok(event) => {
                        let relevant = self.relevant(&event);
                        if relevant && let Err(error) = self.event(&self.reader.view(), self.reader.now(), &event).await {
                            eprintln!("Remote work: {error}");
                        }
                        relevant
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => { self.calls.invalidate_results(); true },
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                change = self.reader.changed() => {
                    if change.is_err() { return; }
                    let current = self.reader.view();
                    let relevant = routing_changed(&view, &current);
                    view = current;
                    relevant
                }
                _ = files.changed() => true,
                _ = records.changed() => { self.calls.invalidate_results(); true }
                _ = closed.notified() => true,
                _ = wait_round(next) => true,
            };
            if relevant {
                next = self.recover().await;
            }
        }
    }

    fn relevant(&self, event: &RelayEvent) -> bool {
        match event {
            RelayEvent::Connected(_) | RelayEvent::Disconnected(_) | RelayEvent::Unreachable(_) | RelayEvent::Gone { .. } => true,
            RelayEvent::Mail { key, envelope, .. } => self.away.contains(key)
                || ["call", "rows", "delivered", "letter"].iter().any(|field| envelope.0.contains_key(*field)),
            _ => false,
        }
    }

    async fn recover(&mut self) -> Option<Instant> {
        let now = self.reader.now();
        match self.round(&self.reader.view(), now).await {
            Ok(next) => {
                let from_disk = next.map(|at| Instant::now() + Duration::from_millis(at.saturating_sub(now).max(1) as u64));
                self.waiting.values().map(|call| call.until).chain(from_disk).min()
            }
            Err(error) => {
                eprintln!("Remote work: {error}");
                Some(Instant::now() + RETRY_FAILED)
            }
        }
    }

    fn offer(&mut self, view: &View, now: Millis, offer: Offer) -> io::Result<()> {
        if offer.cancelled.load(Ordering::Acquire) { return Ok(()); }
        let name = encode(&offer.key);
        let supported = view
            .announced
            .get(&name)
            .and_then(|entry| entry.features.as_ref())
            .is_some_and(|features| {
                features
                    .iter()
                    .any(|feature| feature == offer.request.tool.feature())
            });
        let status = admitted_state(view, offer.key);
        if !supported || matches!(status, PeerState::Unknown | PeerState::Removed) {
            let _ = offer.reply.send(Ok(Outcome::Older));
            return Ok(());
        }
        let wait = view.relay_connected && status == PeerState::Online;
        let keep = offer.keep && !offer.request.tool.waits();
        let away = status == PeerState::Away;
        if !wait && !keep {
            let _ = offer
                .reply
                .send(Ok(unavailable(view.relay_connected, false, away)));
            return Ok(());
        }
        let id =
            match self
                .calls
                .enqueue(offer.key, offer.lead, offer.request, wait, offer.quiet, now)
            {
                Ok(id) => id,
                Err(error) => {
                    let _ = offer.reply.send(Err(error));
                    return Ok(());
                }
            };
        if wait {
            self.waiting.insert(
                id.as_str().into(),
                Waiting {
                    id,
                    key: offer.key,
                    until: Instant::now() + WAIT,
                    keep,
                    reply: offer.reply,
                    cancelled: offer.cancelled,
                },
            );
        } else {
            let _ = offer
                .reply
                .send(Ok(unavailable(view.relay_connected, true, away)));
        }
        Ok(())
    }

    async fn round(&mut self, view: &View, now: Millis) -> io::Result<Option<Millis>> {
        let expired: Vec<_> = self
            .waiting
            .values()
            .filter(|call| call.cancelled.load(Ordering::Acquire) || call.reply.is_closed() || Instant::now() >= call.until)
            .map(|call| call.id.clone())
            .collect();
        for id in expired {
            self.stop_waiting(&id, now, false)?;
        }
        let (sends, call_at) = self.calls.round(
            now,
            view.relay_connected,
            // (One mid-handover of the coding agents' bridge, or this one on a trial, waits.)
            |key| admitted_state(view, key),
            |session| home(view, &self.reader, session, now),
        )?;
        let mut retry = None;
        for send in sends {
            if let Err(error) = self.send(view, send).await {
                eprintln!("Delivering remote work: {error}");
                retry = Some(now.saturating_add(RETRY_FAILED.as_millis() as i64));
            }
        }
        let (rows, rows_at) = self
            .snapshots
            .round(view, now, |session| home(view, &self.reader, session, now))?;
        for (key, envelope) in rows {
            self.mail(view, key, envelope).await;
        }
        let (answers, answer_at) = self.calls.retry_answers(now, view.relay_connected, &view.roster.name)?;
        for (key, envelope) in answers {
            self.mail(view, key, envelope).await;
        }
        Ok([call_at, rows_at, answer_at, retry].into_iter().flatten().min())
    }

    fn stop_waiting(&mut self, id: &Id, now: Millis, unreachable: bool) -> io::Result<()> {
        if let Some(waiting) = self.waiting.remove(id.as_str()) {
            let sent = self.calls.sent(id);
            let kept = waiting.keep && !waiting.cancelled.load(Ordering::Acquire) && !waiting.reply.is_closed();
            self.calls.cancel_wait(id, kept, now)?;
            let _ = waiting.reply.send(Ok(Outcome::Away {
                kept,
                sent,
                unreachable,
            }));
        }
        Ok(())
    }

    async fn event(&mut self, view: &View, now: Millis, event: &RelayEvent) -> io::Result<()> {
        match event {
            RelayEvent::Unreachable(address) => {
                if let Some(peer) = view.roster.at_address(*address) {
                    self.away.insert(peer.key);
                    let waiting: Vec<_> = self
                        .waiting
                        .values()
                        .filter(|call| call.key == peer.key)
                        .map(|call| call.id.clone())
                        .collect();
                    for id in waiting {
                        self.stop_waiting(&id, now, true)?;
                    }
                }
            }
            RelayEvent::Mail { key, envelope, .. } if view.roster.member(key).is_some() => {
                if self.away.remove(key) {
                    self.calls.heard(*key, now);
                }
                // (Not taken while work isn't admitted between the two: its sender tries again.)
                if let Some(call) = envelope
                    .0
                    .get("call")
                    .filter(|_| view.work_admitted(&encode(key)))
                {
                    let replies = self
                        .calls
                        .receive(
                            *key,
                            call.clone(),
                            now,
                            &view.roster.name,
                            |session| home(view, &self.reader, session, now),
                            self.work.as_ref(),
                        )
                        .await?;
                    for reply in replies {
                        self.mail(view, *key, reply).await;
                    }
                }
                if let Some(rows) = envelope.0.get("rows")
                    && let Some(ack) = self.snapshots.receive(*key, rows.clone(), now)?
                {
                    self.mail(view, *key, ack).await;
                }
                if let Some(receipt) = decode::<Delivered>(envelope, "delivered") {
                    self.snapshots.acknowledge(*key, &receipt)?;
                    self.calls.answer_receipt(*key, &receipt)?;
                    if let Some(send) = self.calls.receipt(*key, &receipt)? {
                        self.send(view, send).await?;
                    }
                }
                if let Some(letter) = decode(envelope, "letter")
                    && let Some(answer) = self.calls.answer(*key, letter, now)
                {
                    self.answer(view, *key, now, answer).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn answer(
        &mut self,
        view: &View,
        key: ComputerKey,
        now: Millis,
        received: ReceivedAnswer,
    ) -> io::Result<()> {
        match received {
            ReceivedAnswer::Ack(ack) => self.mail(view, key, ack).await,
            ReceivedAnswer::New {
                call,
                letter,
                lead,
                mut answer,
                note,
                words,
            } => {
                if let Some(waiter) = self.waiting.remove(call.as_str()) {
                    if let Some(words) = words {
                        answer.text = words;
                    }
                    if waiter
                        .reply
                        .send(Ok(Outcome::Answer(answer.clone())))
                        .is_err()
                        && note
                    {
                        self.work.deliver(&letter, &lead, &answer).await?;
                    }
                } else if note {
                    self.work.deliver(&letter, &lead, &answer).await?;
                }
                self.calls.accept_answer(key, &call, lead, now)?;
                self.mail(view, key, calls::receipt(&letter, None, now))
                    .await;
            }
        }
        Ok(())
    }

    async fn send(&mut self, view: &View, send: Outgoing) -> io::Result<()> {
        match send {
            Outgoing::Mail { key, envelope } => self.mail(view, key, envelope).await,
            Outgoing::Failed(failure) => {
                if let Some(waiter) = self.waiting.remove(failure.id.as_str()) {
                    let _ = waiter
                        .reply
                        .send(Ok(Outcome::Failed(failure.why().to_owned())));
                } else if failure.notifies() {
                    let computer = view
                        .roster
                        .member(&failure.key)
                        .map_or("another computer", |member| &member.name);
                    let answer = Answer::refused(failure.queued_words(computer));
                    self.work
                        .deliver(&failure.id, &failure.lead, &answer)
                        .await?;
                }
                self.calls.finish(&failure.id)?;
            }
        }
        Ok(())
    }

    async fn mail(&self, view: &View, key: ComputerKey, envelope: Envelope) {
        if let Some(host) = view
            .roster
            .member(&key)
            .and_then(crate::directory::Member::host)
        {
            let _ = self.relay.sender.mail(host, envelope).await;
        }
    }
}

async fn wait_round(at: Option<Instant>) {
    match at {
        Some(at) => timer_fires::sleep_until(Timer::RemoteWorkRound, at).await,
        None => std::future::pending().await,
    }
}

/// Routing facts change independently of a peer's heartbeat, activity or update progress.
pub fn routing_changed(before: &View, after: &View) -> bool {
    let members = |view: &View| view.roster.members.iter().map(|peer| {
        let announced = view.announced.get(&encode(&peer.key));
        let features = announced.and_then(|a| a.features.clone());
        let moved = announced.into_iter().flat_map(|a| &a.moved)
            .map(|hint| (hint.session.clone(), hint.key.clone(), hint.at)).collect::<Vec<_>>();
        (peer.key, peer.address, peer.name.clone(), admitted_state(view, peer.key), features, moved)
    }).collect::<Vec<_>>();
    let held = |view: &View| view.local.list.iter().flat_map(|list| &list.sessions)
        .filter(|(_, row)| row.facts.away != Some(true)).map(|(id, _)| id.clone()).collect::<Vec<_>>();
    before.relay_connected != after.relay_connected
        || before.roster.here != after.roster.here
        || before.roster.name != after.roster.name
        || members(before) != members(after)
        || before.local.created.keys().ne(after.local.created.keys())
        || held(before) != held(after)
        || !before.same_ownership_routes(after)
}

fn decode<T: serde::de::DeserializeOwned>(envelope: &Envelope, field: &str) -> Option<T> {
    serde_json::from_value(envelope.0.get(field)?.clone()).ok()
}

fn unavailable(online: bool, kept: bool, unreachable: bool) -> Outcome {
    if online {
        Outcome::Away {
            kept,
            sent: false,
            unreachable,
        }
    } else {
        Outcome::Offline { kept }
    }
}

// A release handoff keeps calls without sending the normal reach probe.
fn admitted_state(view: &View, key: ComputerKey) -> PeerState {
    match peer_state(view, key) {
        PeerState::Online | PeerState::Away if !view.work_admitted(&encode(&key)) => {
            PeerState::Held
        }
        state => state,
    }
}

fn peer_state(view: &View, key: ComputerKey) -> PeerState {
    if view.roster.member(&key).is_none() {
        PeerState::Removed
    } else if view
        .roster
        .member(&key)
        .and_then(crate::directory::Member::host)
        .is_none()
    {
        PeerState::Unknown
    } else if view.reach.away_since(&encode(&key)).is_some() {
        PeerState::Away
    } else {
        PeerState::Online
    }
}

fn home(view: &View, reader: &Reader, session: &str, now: Millis) -> Home {
    let ownership = &view.local.ownership;
    let entry = (RecordScope::Session, session.to_owned());
    if let Some(location) = ownership.journal.get(&entry) {
        let key = &location.key;
        let key = *ownership.replacements.get(key).unwrap_or(key);
        if view.roster.here != Some(key) {
            return Home::Peer(key);
        }
    }
    if ownership.frozen.contains(session) {
        return Home::Frozen;
    }
    if view.local.holds(session)
        || ownership.held.contains(&entry)
        || view.local.created.contains_key(session)
    {
        return Home::Local;
    }
    let known = view.moved(session, now).and_then(|key| decode_exact(&key));
    let recorded = calls::known_home(reader.run.as_ref(), session);
    match known.or(recorded) {
        Some(key) if view.roster.here == Some(key) => Home::Local,
        Some(key) if view.roster.member(&key).is_some() => Home::Peer(key),
        _ => Home::Unknown,
    }
}
