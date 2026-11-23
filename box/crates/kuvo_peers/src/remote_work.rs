//! Work calls share the resident relay. The engine supplies local work facts and owns effects.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::Arc,
    time::Duration,
};

use kuvo_wire::{
    keys::{decode_exact, encode},
    mail::Envelope,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
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
const ROUND: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub enum Outcome {
    Answer(Answer),
    Failed(String),
    Away { kept: bool, sent: bool },
    Offline { kept: bool },
    Older,
}

struct Offer {
    key: ComputerKey,
    lead: String,
    request: Request,
    keep: bool,
    quiet: bool,
    reply: oneshot::Sender<io::Result<Outcome>>,
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
}

#[derive(Clone)]
pub struct WorkHandle {
    send: mpsc::Sender<Command>,
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
        self.send
            .send(Command::Call(Box::new(Offer {
                key,
                lead,
                request,
                keep,
                quiet,
                reply,
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
    Ok((WorkHandle { send }, tokio::spawn(owner.run(events, offers))))
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
    ) {
        let mut timer = tokio::time::interval(ROUND);
        timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            let result = tokio::select! {
                Some(offer) = offers.recv() => match offer {
                    Command::Call(offer) => self.offer(&self.reader.view(), self.reader.now(), *offer),
                    Command::Rows(rows) => { self.snapshots.update(rows); Ok(()) }
                },
                event = events.recv() => match event {
                    Ok(event) => self.event(&self.reader.view(), self.reader.now(), &event).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => Ok(()),
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = timer.tick() => self.round(&self.reader.view(), self.reader.now()).await,
            };
            if let Err(error) = result {
                eprintln!("Remote work: {error}");
            }
        }
    }

    fn offer(&mut self, view: &View, now: Millis, offer: Offer) -> io::Result<()> {
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
        let status = peer_state(view, offer.key);
        if !supported || matches!(status, PeerState::Unknown | PeerState::Removed) {
            let _ = offer.reply.send(Ok(Outcome::Older));
            return Ok(());
        }
        let wait = view.relay_connected && status != PeerState::Away;
        let keep = offer.keep && !offer.request.tool.waits();
        if !wait && !keep {
            let _ = offer
                .reply
                .send(Ok(unavailable(view.relay_connected, false)));
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
                },
            );
        } else {
            let _ = offer
                .reply
                .send(Ok(unavailable(view.relay_connected, true)));
        }
        Ok(())
    }

    async fn round(&mut self, view: &View, now: Millis) -> io::Result<()> {
        let expired: Vec<_> = self
            .waiting
            .values()
            .filter(|call| call.reply.is_closed() || Instant::now() >= call.until)
            .map(|call| call.id.clone())
            .collect();
        for id in expired {
            self.stop_waiting(&id, now)?;
        }
        let sends = self.calls.round(
            now,
            view.relay_connected,
            |key| peer_state(view, key),
            |session| home(view, &self.reader, session, now),
        )?;
        for send in sends {
            if let Err(error) = self.send(view, send).await {
                eprintln!("Delivering remote work: {error}");
            }
        }
        for (key, envelope) in self
            .snapshots
            .round(view, now, |session| home(view, &self.reader, session, now))?
        {
            self.mail(view, key, envelope).await;
        }
        for (key, envelope) in
            self.calls
                .retry_answers(now, view.relay_connected, &view.roster.name)?
        {
            self.mail(view, key, envelope).await;
        }
        Ok(())
    }

    fn stop_waiting(&mut self, id: &Id, now: Millis) -> io::Result<()> {
        if let Some(waiting) = self.waiting.remove(id.as_str()) {
            let sent = self.calls.sent(id);
            let kept = waiting.keep && !waiting.reply.is_closed();
            self.calls.cancel_wait(id, kept, now)?;
            let _ = waiting.reply.send(Ok(Outcome::Away { kept, sent }));
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
                        self.stop_waiting(&id, now)?;
                    }
                }
            }
            RelayEvent::Mail { key, envelope, .. } if view.roster.member(key).is_some() => {
                if self.away.remove(key) {
                    self.calls.heard(*key, now);
                }
                if let Some(call) = envelope.0.get("call") {
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

fn decode<T: serde::de::DeserializeOwned>(envelope: &Envelope, field: &str) -> Option<T> {
    serde_json::from_value(envelope.0.get(field)?.clone()).ok()
}

fn unavailable(online: bool, kept: bool) -> Outcome {
    if online {
        Outcome::Away { kept, sent: false }
    } else {
        Outcome::Offline { kept }
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
    if let Some(key) = ownership.journal.get(&entry) {
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
