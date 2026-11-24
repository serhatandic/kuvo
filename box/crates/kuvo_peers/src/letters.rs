//! Durable ordinary letters and notes. Resolve ownership again for each send
//! and immediately before placing an incoming prompt on a local session.
pub(crate) mod homes;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use kuvo_engine::{
    Engine, LetterDelivery, LetterPrompt, LetterText, MoveEngineError, SessionMove,
    TranscriptError, TranscriptScope,
};
use kuvo_store::{
    files,
    locks::{OnBusy, StaleLock},
};
use kuvo_wire::{
    keys::{decode_exact, encode},
    mail::Envelope,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{broadcast, watch},
    task::{JoinHandle, JoinSet},
    time::{self, Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::{
    directory::{Announcement, ComputerKey},
    moves::{is_mac, plain_name},
    outbox::{
        Delivered, Delivery, FailureContext, Id, IncomingLetter, Letter, Outbox, Pending, Receipts,
    },
    ownership::Handle,
    relay::{RelayEvent, RelayHandle},
    setup_lists::{Reader, View},
};

const MOVED: &str = "It's moving to another computer: it comes again once it's there.";
const LIMIT: u64 = 600_000;
const ANSWER_TEXT: usize = 8000;
const READ_BY_MAIL: usize = 20_000;
const INTRODUCING_FOR: Duration = Duration::from_secs(60);
const PUBLISHING_FOR: Duration = Duration::from_secs(2);

pub fn start(
    reader: Reader,
    relay: RelayHandle,
    engine: Engine,
    ownership: Handle,
) -> io::Result<JoinHandle<()>> {
    let events = relay
        .subscribe()
        .ok_or_else(|| io::Error::other("The resident relay stopped"))?;
    files::create_private_dirs(&reader.run.join("outbox/sending"))?;
    let post = Letters {
        reader,
        relay,
        engine,
        ownership,
        outbox: None,
        receipts: None,
        answers: JoinSet::new(),
        introductions: HashMap::new(),
        incoming_lists: HashMap::new(),
    };
    Ok(tokio::spawn(post.run(events)))
}

struct Letters {
    reader: Reader,
    relay: RelayHandle,
    engine: Engine,
    ownership: Handle,
    outbox: Option<Outbox>,
    receipts: Option<Receipts>,
    answers: JoinSet<Option<(Id, Letter)>>,
    introductions: HashMap<Id, Introduction>,
    incoming_lists: HashMap<ComputerKey, Announcement>,
}

struct Introduction {
    since: Instant,
    wait_for_list: Option<bool>,
}

struct ReadFailure {
    message: String,
    retry: bool,
}

#[derive(Deserialize)]
struct Note {
    from: String,
    words: String,
    delivery: NoteDelivery,
    line: Option<String>,
    about: Option<Value>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum NoteDelivery {
    Queue,
    Steer,
}

#[derive(Deserialize)]
struct ToolReply {
    #[serde(default)]
    read: bool,
    text: Option<String>,
    to: Option<String>,
    from: Option<String>,
    name: Option<String>,
}

impl Letters {
    async fn run(mut self, mut events: broadcast::Receiver<Arc<RelayEvent>>) {
        let mut tick = time::interval(Duration::from_secs(3));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            let result = tokio::select! {
                event = events.recv() => match event {
                    Ok(event) => match event.as_ref() {
                        RelayEvent::Mail { key, envelope, .. } => self.receive(*key, envelope).await,
                        RelayEvent::Connected(_) => self.send_due().await,
                        _ => Ok(()),
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => Ok(()),
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                result = self.answers.join_next(), if !self.answers.is_empty() => {
                    match result {
                        Some(Ok(Some((id, letter)))) => self.leave_answer(&id, letter),
                        Some(Err(error)) => Err(io::Error::other(error)),
                        _ => Ok(()),
                    }
                }
                _ = tick.tick() => self.send_due().await,
            };
            if let Err(error) = result {
                eprintln!("Keeping letters: {error}");
            }
        }
    }

    fn open_outbox(&mut self) -> io::Result<Option<&mut Outbox>> {
        if self.outbox.is_none()
            && std::fs::read_dir(self.reader.run.join("outbox/sending"))?
                .next()
                .transpose()?
                .is_some()
        {
            self.outbox()?;
        }
        Ok(self.outbox.as_mut())
    }

    fn outbox(&mut self) -> io::Result<&mut Outbox> {
        if self.outbox.is_none() {
            self.outbox = Some(Outbox::open(&self.reader.run).map_err(io::Error::other)?);
        }
        Ok(self.outbox.as_mut().expect("opened outbox"))
    }

    fn receipts(&mut self) -> io::Result<&mut Receipts> {
        if self.receipts.is_none() {
            self.receipts = Some(Receipts::open(&self.reader.run).map_err(io::Error::other)?);
        }
        Ok(self.receipts.as_mut().expect("opened receipts for letters"))
    }

    fn leave_answer(&mut self, id: &Id, mut letter: Letter) -> io::Result<()> {
        letter.at = Some(self.reader.now().max(0) as u64);
        self.outbox()?
            .enqueue(id, &letter)
            .map_err(io::Error::other)?;
        Ok(())
    }

    async fn send_due(&mut self) -> io::Result<()> {
        let view = self.reader.view();
        let wall = self.reader.now();
        let now = wall.max(0) as u64;
        let run = self.reader.run.clone();
        if let Err(error) = homes::learn_moves(&run, &view, wall) {
            eprintln!("Remembering moved conversations: {error}");
        }
        self.recover_replies(&view, now)?;
        let Some(outbox) = self.open_outbox()? else {
            return Ok(());
        };
        if let Some(here) = view.roster.here {
            outbox
                .finish_failures(&here, now, |id, letter| {
                    failure_context(&view, &run, here, id, letter, now)
                })
                .map_err(io::Error::other)?;
        }
        let pending = outbox.due(now, true).map_err(io::Error::other)?;
        let due: HashSet<_> = pending.iter().map(|each| &each.id).collect();
        self.introductions.retain(|id, _| due.contains(id));
        let homes = if pending.iter().any(|each| each.letter.note.is_some()) {
            homes::read(&run)?
        } else {
            Default::default()
        };
        for each in pending {
            let Ok(scope) = self.ownership.scope(&each.letter.to, None) else {
                continue;
            };
            if scope.letter_held {
                continue;
            }
            let owner = scope.owner;
            let learned = homes.get(&each.letter.to).filter(|home| {
                each.letter.note.is_some()
                    && !owner.as_ref().is_some_and(|owner| owner.local)
                    && scope
                        .letter_home
                        .is_none_or(|(_, at)| at.is_some_and(|at| home.newer_than(at)))
            });
            let retained = learned
                .and_then(homes::Home::key)
                .filter(|key| Some(*key) == view.roster.here || view.roster.member(key).is_some());
            let known = retained
                .or_else(|| scope.letter_home.map(|(key, _)| key))
                .or_else(|| owner.and_then(|owner| owner.key));
            let key = known.or_else(|| each.letter.peer.as_deref().and_then(decode_exact));
            let Some(key) = key else {
                continue;
            };
            let here = view.roster.here == Some(key);
            let host = (!here)
                .then(|| view.roster.member(&key).and_then(|peer| peer.host()))
                .flatten();
            if !here
                && (!view.relay_connected || host.is_none() || !view.work_admitted(&encode(&key)))
            {
                continue;
            }
            if !here && !self.introduced(&key, &each).await {
                continue;
            }
            let Some(mail) = self
                .outbox
                .as_mut()
                .expect("opened outbox")
                .attempt(&each.id, &key, now)
                .map_err(io::Error::other)?
            else {
                continue;
            };
            if let Some(host) = host {
                if self.relay.sender.mail(host, mail).await.is_err() {
                    self.outbox
                        .as_mut()
                        .expect("opened outbox")
                        .cancel_try(&each.id);
                }
            } else if let Some(letter) = envelope_field::<IncomingLetter>(&mail, "letter") {
                let receipt = self.take(&view, key, letter).await?;
                if let Some(receipt) = receipt {
                    self.outbox
                        .as_mut()
                        .expect("opened outbox")
                        .settle(&key, &receipt, now)
                        .map_err(io::Error::other)?;
                }
            }
        }
        Ok(())
    }

    async fn introduced(&mut self, peer: &ComputerKey, pending: &Pending) -> bool {
        // A transcript reply belongs to an authenticated request. Its reader
        // already knows the conversation, including one that is archived.
        if pending.letter.answers.is_some() && pending.letter.read.is_some() {
            return true;
        }
        let Some(sender) = pending
            .letter
            .from
            .as_deref()
            .filter(|sender| !sender.is_empty())
        else {
            return true;
        };
        let sent = self.reader.agent_sent_to(peer, sender);
        if sent == Some(true) {
            return true;
        }
        let introduction = self
            .introductions
            .entry(pending.id.clone())
            .or_insert_with(|| Introduction {
                since: Instant::now(),
                wait_for_list: None,
            });
        if introduction.since.elapsed() >= INTRODUCING_FOR {
            return true;
        }
        // An older list can omit a newly created root while the engine has
        // it already. Check once before treating a missing name as unknown.
        if introduction.wait_for_list.is_none() {
            let fresh = time::timeout(Duration::from_secs(10), async {
                self.engine.session_move().await?.lookup(sender).await
            })
            .await;
            if let Ok(Ok(Some(info))) = &fresh
                && let Some(imported) = info
                    .pointer("/metadata/kuvo.moveImport")
                    .and_then(Value::as_str)
                && crate::moves::receipt::Receipts::open(&self.reader.run)
                    .introduced_by_move(sender, peer, imported)
                    .unwrap_or(false)
            {
                return true;
            }
            let now = self.reader.now().max(0) as u64;
            let recent = |at| now.saturating_sub(at) < INTRODUCING_FOR.as_millis() as u64;
            introduction.wait_for_list = Some(match fresh {
                Ok(Ok(Some(info))) => {
                    !archived(&info)
                        && info["parentID"].is_null()
                        && info
                            .pointer("/metadata/kuvo/seat")
                            .is_none_or(Value::is_null)
                        && info["title"]
                            .as_str()
                            .is_some_and(|title| !title.trim().is_empty())
                        && info
                            .pointer("/time/created")
                            .and_then(Value::as_u64)
                            .is_some_and(recent)
                }
                Ok(Ok(None) | Err(MoveEngineError::InvalidTree)) => false,
                // The engine may still be opening its session list. An error
                // doesn't establish that a newly queued message has no sender.
                _ => pending.letter.at.is_some_and(recent),
            });
        }
        sent == Some(false) && introduction.wait_for_list == Some(false)
    }

    fn recover_replies(&mut self, view: &View, now: u64) -> io::Result<()> {
        let replies = self.reader.run.join("replies");
        let entries = match std::fs::read_dir(&replies) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            let extension = path.extension().and_then(|ext| ext.to_str());
            if !matches!(extension, Some("json" | "gaveup")) {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|id| Id::try_from(id.to_owned()).ok())
            else {
                continue;
            };
            if extension == Some("gaveup") {
                self.give_up(&id, &path)?;
                continue;
            }
            if tool_alive(&self.reader.run, &id, now) {
                continue;
            }
            let Some(bytes) = files::read(&path)? else {
                continue;
            };
            let Ok(reply) = serde_json::from_slice::<ToolReply>(&bytes) else {
                continue;
            };
            if !reply.read {
                let (Some(to), Some(words)) = (reply.to, reply.text) else {
                    continue;
                };
                let Ok(scope) = self.ownership.scope(&to, None) else {
                    continue;
                };
                if scope.letter_held {
                    continue;
                }
                let name = reply.name.as_deref().unwrap_or("another agent");
                let from = reply.from.as_deref().unwrap_or("unknown");
                let delivery = Id::try_from(format!(
                    "ltr_{}",
                    &encode(&Sha256::digest(format!("{} left answer", id.as_str())))[..22]
                ))
                .map_err(io::Error::other)?;
                // This stable ID survives a restart between enqueuing the note
                // and removing the tool's copy of its answer.
                self.leave_answer(
                    &delivery,
                    Letter {
                        to,
                        text: letter_prompt(name, from, &words, false),
                        from: None,
                        at: None,
                        peer: view.roster.here.map(|key| encode(&key)),
                        sent_to: None,
                        wait: false,
                        answers: None,
                        read: None,
                        note: json!({"from":name, "words":words, "delivery":"steer"})
                            .as_object()
                            .cloned(),
                        refused: false,
                        other: Default::default(),
                    },
                )?;
            }
            files::remove_file_synced(&path)?;
            let gave_up = path.with_extension("gaveup");
            if gave_up.try_exists()? {
                self.give_up(&id, &gave_up)?;
            }
        }
        Ok(())
    }

    fn give_up(&mut self, id: &Id, marker: &Path) -> io::Result<()> {
        if self.outbox()?.give_up(id).map_err(io::Error::other)? {
            files::remove_file_synced(&marker.with_extension("alive"))?;
            files::remove_file_synced(marker)?;
        }
        Ok(())
    }

    async fn receive(&mut self, key: ComputerKey, envelope: &Envelope) -> io::Result<()> {
        let view = self.reader.view();
        let Some(peer) = view.roster.member(&key) else {
            return Ok(());
        };
        let key_text = encode(&key);
        let published = view.announced.get(&key_text).map_or(0, |entry| entry.at);
        self.incoming_lists.retain(|key, entry| {
            view.roster.member(key).is_some()
                && view.announced.get(&encode(key)).map_or(0, |list| list.at) < entry.at
        });
        if let Some(agents) = envelope.0.get("agents").cloned()
            && let Some(agents) = Announcement::received(agents, peer, self.reader.now())
            && agents.at > published
            && self
                .incoming_lists
                .get(&key)
                .is_none_or(|seen| agents.at > seen.at)
        {
            self.incoming_lists.insert(key, agents);
        }
        let now = self.reader.now().max(0) as u64;
        if let Some(delivered) = envelope_field::<Delivered>(envelope, "delivered")
            && let Some(outbox) = self.open_outbox()?
        {
            outbox
                .settle(&key, &delivered, now)
                .map_err(io::Error::other)?;
        }
        let Some(letter) = envelope_field::<IncomingLetter>(envelope, "letter") else {
            return Ok(());
        };
        // Both actors see authenticated mail in relay order. If the preceding list
        // named this sender, give the directory time to publish that precise list.
        let view = match self.incoming_lists.get(&key) {
            Some(list)
                if letter.letter.from.as_ref().is_some_and(|sender| {
                    list.list.iter().any(|agent| &agent.session == sender)
                }) =>
            {
                self.after_announcement(&key_text, list.at).await
            }
            _ => view,
        };
        if view.work_admitted(&key_text)
            && let Some(host) = view.roster.member(&key).and_then(|peer| peer.host())
            && let Some(reply) = self.take(&view, key, letter).await?
        {
            let _ = self.relay.sender.mail(host, reply.envelope(now)).await;
        }
        Ok(())
    }

    async fn after_announcement(&mut self, key: &str, at: i64) -> Arc<View> {
        let _ = time::timeout(PUBLISHING_FOR, async {
            while self
                .reader
                .view()
                .announced
                .get(key)
                .is_none_or(|list| list.at < at)
            {
                self.reader.changed().await?;
            }
            Ok::<(), watch::error::RecvError>(())
        })
        .await;
        self.reader.view()
    }

    async fn take(
        &mut self,
        view: &View,
        key: ComputerKey,
        mut envelope: IncomingLetter,
    ) -> io::Result<Option<Delivered>> {
        let now = self.reader.now().max(0) as u64;
        // Remote work calls and seats own their own dispatch and receipts.
        if envelope.letter.other.contains_key("call") || envelope.letter.other.contains_key("seat")
        {
            return Ok(None);
        }
        let note = envelope
            .letter
            .note
            .clone()
            .and_then(|fields| serde_json::from_value::<Note>(fields.into()).ok())
            .filter(|note| {
                !note.from.trim().is_empty()
                    && note
                        .line
                        .as_deref()
                        .is_none_or(|line| !line.trim().is_empty())
            });
        if note.is_none() {
            envelope.letter.note = None;
        }
        if let Some(receipt) = envelope.refusal(now) {
            return Ok(Some(receipt));
        }
        let IncomingLetter { id, letter } = envelope;
        if self.receipts()?.seen(&id, now).map_err(io::Error::other)? {
            return Ok(Some(delivered(id)));
        }
        if let Some(answered) = &letter.answers
            && (self
                .accept_answer(view, key, &letter, answered, now)
                .await?
                || letter.read.is_some())
        {
            self.receipts()?
                .accepted(&id, note.is_some(), now)
                .map_err(io::Error::other)?;
            return Ok(Some(delivered(id)));
        }
        if letter.read.is_some() {
            return self.read_transcript(key, id, letter, now).await.map(Some);
        }
        let scope = self
            .ownership
            .scope(&letter.to, None)
            .map_err(io::Error::other)?;
        let place = scope.owner;
        if scope.held || place.as_ref().is_some_and(|owner| !owner.local) {
            let lives = note
                .as_ref()
                .and_then(|_| place.filter(|owner| !owner.local))
                .and_then(|owner| owner.key)
                .map(|key| encode(&key));
            return Ok(Some(refused(id, MOVED, true, lives)));
        }
        let engine = match self.engine.session_move().await {
            Ok(engine) => engine,
            Err(error) => {
                return Ok(Some(refused(
                    id,
                    &format!("It didn't reach it: {error}"),
                    true,
                    None,
                )));
            }
        };
        let session = match engine.lookup(&letter.to).await {
            Ok(Some(session)) => session,
            Ok(None) => {
                if note.is_some()
                    && let Some(home) = homes::read(&self.reader.run)?
                        .get(&letter.to)
                        .and_then(homes::Home::key)
                        .filter(|key| view.roster.member(key).is_some())
                {
                    return Ok(Some(refused(
                        id,
                        "It lives on another computer now: it comes again there.",
                        true,
                        Some(encode(&home)),
                    )));
                }
                return Ok(Some(refused(
                    id,
                    "Its coding agents aren't running yet.",
                    true,
                    None,
                )));
            }
            Err(error) => {
                return Ok(Some(refused(
                    id,
                    &format!("It didn't reach it: {error}"),
                    true,
                    None,
                )));
            }
        };
        let child = session["parentID"].as_str().is_some();
        let seat = session
            .pointer("/metadata/kuvo/seat")
            .is_some_and(|seat| !seat.is_null());
        let archived = archived(&session);
        if archived && !child && !seat && note.is_some() {
            return Ok(Some(refused(
                id,
                "It's archived: it comes again once it's brought back.",
                true,
                None,
            )));
        }
        if archived || child || seat {
            return Ok(Some(refused(
                id,
                "That conversation doesn't take messages.",
                false,
                None,
            )));
        }
        if note.is_none() && view.roster.here != Some(key) {
            let agents = view.announced_agents();
            if !agents.is_empty() && !agents.iter().any(|agent| agent.session == letter.to) {
                let here = view.roster.local();
                let arrived = crate::moves::recently_arrived(
                    &self.reader.run,
                    &letter.to,
                    &here.id,
                    self.reader.now(),
                )
                .map_err(io::Error::other)?;
                if !arrived {
                    let under = match letter.from.as_deref() {
                        Some(lead) => started_under(&engine, &session, lead).await,
                        None => Ok(false),
                    };
                    match under {
                        Ok(true) => {}
                        Ok(false) => {
                            return Ok(Some(refused(
                                id,
                                "That conversation isn't one this computer lists, nor one you started: write to one that list_agents names.",
                                false,
                                None,
                            )));
                        }
                        Err(error) => {
                            return Ok(Some(refused(
                                id,
                                &format!("It didn't reach it: {error}"),
                                true,
                                None,
                            )));
                        }
                    }
                }
            }
        }
        let key_text = encode(&key);
        let counted_as = format!(
            "{}:{key_text}",
            if note.is_some() { "note" } else { "peer" }
        );
        if !count(&self.reader.run, &counted_as, now, true)? {
            return Ok(Some(refused(
                id,
                "It's sent a lot of messages in the last few minutes.",
                true,
                None,
            )));
        }
        let text = cut(&letter.text, 32_000).to_owned();
        let mut sender = None;
        let (immediate, when_queued, from, words, steer) = if let Some(ref note) = note {
            let from = cut(&note.from, 200).to_owned();
            let metadata = note
                .line
                .as_ref()
                .map(|line| json!({"kuvo":{"note":{"from":from,"line":cut(line,32_000)}}}));
            (
                LetterText { text, metadata },
                None,
                from,
                cut(&note.words, 32_000).to_owned(),
                note.delivery == NoteDelivery::Steer,
            )
        } else {
            sender = self.sender(view, key, letter.from.as_deref());
            let name = sender
                .as_ref()
                .map(|(_, name)| name.to_string())
                .unwrap_or_else(|| unknown_sender(view, &key));
            let address = sender.as_ref().map_or("unknown", |(from, _)| *from);
            let compose = |waiting| {
                let hint = letter_hint(address, waiting);
                LetterText {
                    text: format!("[Message from {name} · {address}] ({hint})\n\n{text}"),
                    metadata: Some(
                        json!({"kuvo":{"parts":{"words":text,"mail":{"name":name,"session":address,"hint":hint}}}}),
                    ),
                }
            };
            let waiting = sender.is_some() && letter.wait;
            (
                compose(waiting),
                waiting.then(|| compose(false)),
                name,
                text,
                true,
            )
        };
        let Delivery::Submit { prompt: prompt_id } = self
            .receipts()?
            .begin(&id, note.is_some(), now)
            .map_err(io::Error::other)?
        else {
            return Ok(Some(delivered(id)));
        };
        match engine
            .deliver_letter(
                &letter.to,
                LetterPrompt {
                    id: prompt_id.clone(),
                    immediate,
                    when_queued,
                    from,
                    words,
                    steer,
                },
            )
            .await
        {
            Ok(delivered_letter) => {
                self.receipts()?
                    .accepted(&id, note.is_some(), now)
                    .map_err(io::Error::other)?;
                if view.roster.here != Some(key)
                    && let Some(about) = note
                        .as_ref()
                        .and_then(|note| note.about.as_ref())
                        .and_then(Value::as_array)
                {
                    let sessions = about
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|session| crate::ownership::session_id(session))
                        .take(100);
                    let made = letter
                        .other
                        .get("made")
                        .and_then(Value::as_u64)
                        .unwrap_or(now);
                    if let Err(error) =
                        homes::remember(&self.reader.run, sessions, &key, made.min(now))
                    {
                        eprintln!("Remembering the computer named in a note: {error}");
                    }
                }
                let mut receipt = delivered(id.clone());
                if note.is_none() && letter.wait {
                    if let (Some((from, _)), LetterDelivery::Accepted) =
                        (&sender, &delivered_letter)
                    {
                        self.answers.spawn(wait_for_answer(
                            engine,
                            letter.to.clone(),
                            prompt_id,
                            (*from).to_owned(),
                            key,
                            id,
                        ));
                    } else {
                        receipt.answer = Some(false);
                        receipt.why = Some(match (sender, delivered_letter) {
                            (Some(_), LetterDelivery::Queued(reason)) => reason,
                            _ => "Its answer can't come back to you: it doesn't know who you are."
                                .into(),
                        });
                    }
                }
                Ok(Some(receipt))
            }
            Err(error) => {
                count(&self.reader.run, &counted_as, now, false)?;
                let message = if error.reply().status.as_u16() == 409 {
                    MOVED
                } else {
                    "It didn't reach it: the engine isn't answering."
                };
                Ok(Some(refused(id, message, true, None)))
            }
        }
    }

    async fn read_transcript(
        &mut self,
        key: ComputerKey,
        id: Id,
        request: Letter,
        now: u64,
    ) -> io::Result<Delivered> {
        const MOVING_READ: &str = "It's moving to another computer: it's read once it's there.";
        let Some(from) = request.from.filter(|from| {
            from.strip_prefix("ses_").is_some_and(|suffix| {
                (1..=60).contains(&suffix.len())
                    && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
        }) else {
            return Ok(refused(id, "Its answer has nowhere to go.", false, None));
        };
        let moving = |to: &str| {
            self.ownership
                .scope(to, None)
                .map(|scope| {
                    scope.held
                        || scope.owner.is_some_and(|owner| !owner.local)
                        || scope
                            .letter_home
                            .is_some_and(|(home, _)| Some(home) != scope.here)
                })
                .map_err(io::Error::other)
        };
        if moving(&request.to)? {
            return Ok(refused(id, MOVING_READ, true, None));
        }
        let result = self
            .transcript(
                &request.to,
                &from,
                request
                    .read
                    .as_ref()
                    .and_then(|read| read.get("query"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty()),
            )
            .await;
        let words = match result {
            Ok(words) => words,
            Err(error) => return Ok(refused(id, &error.message, error.retry, None)),
        };
        if moving(&request.to)? {
            return Ok(refused(id, MOVING_READ, true, None));
        }
        let hash = Sha256::digest(format!("{} read by {}", id.as_str(), encode(&key)));
        let answer =
            Id::try_from(format!("ltr_{}", &encode(&hash)[..22])).map_err(io::Error::other)?;
        self.leave_answer(
            &answer,
            Letter {
                to: from,
                from: Some(request.to),
                text: words,
                at: None,
                peer: Some(encode(&key)),
                sent_to: None,
                wait: false,
                answers: Some(id.clone()),
                read: Some(Default::default()),
                note: None,
                refused: false,
                other: Default::default(),
            },
        )?;
        self.receipts()?
            .accepted(&id, false, now)
            .map_err(io::Error::other)?;
        Ok(delivered(id))
    }

    async fn transcript(
        &self,
        session: &str,
        from: &str,
        query: Option<&str>,
    ) -> Result<String, ReadFailure> {
        let unavailable = |error: MoveEngineError| ReadFailure {
            message: format!("It couldn't be read: {error}"),
            retry: true,
        };
        let engine = self.engine.session_move().await.map_err(unavailable)?;
        let info = engine
            .lookup(session)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| ReadFailure {
                message: "Its coding agents aren't running here now: it comes again once they are."
                    .into(),
                retry: true,
            })?;
        let refuse = |message: &str| ReadFailure {
            message: message.into(),
            retry: false,
        };
        if info["parentID"].as_str().is_some_and(|s| !s.is_empty()) {
            return Err(refuse("It's a subagent's conversation."));
        }
        if info
            .pointer("/metadata/kuvo/seat")
            .is_some_and(|seat| !seat.is_null())
        {
            return Err(refuse("It's a seat in a group chat."));
        }
        if archived(&info)
            && (engine.lookup(from).await.map_err(unavailable)?.is_some()
                || !started_under(&engine, &info, from)
                    .await
                    .map_err(unavailable)?)
        {
            return Err(refuse(
                "It's archived, and only the conversation that started this work can read it.",
            ));
        }
        let title = info["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .unwrap_or("New session");
        self.engine
            .transcript(
                session,
                title,
                query,
                TranscriptScope::Peer,
                &CancellationToken::new(),
            )
            .await
            .map_err(|error| ReadFailure {
                message: format!("It couldn't be read: {error}"),
                retry: !matches!(error, TranscriptError::Refused(_)),
            })
    }

    fn sender<'a>(
        &self,
        view: &'a View,
        key: ComputerKey,
        from: Option<&'a str>,
    ) -> Option<(&'a str, Cow<'a, str>)> {
        let from = from?;
        let scope = self.ownership.scope(from, None).ok()?;
        if scope.owner?.key != Some(key) {
            return None;
        }
        let name = if view.roster.here == Some(key) {
            Cow::Borrowed(
                view.local
                    .list
                    .as_ref()?
                    .sessions
                    .get(from)?
                    .facts
                    .title
                    .as_deref()?,
            )
        } else if let Some(agent) = view
            .announced
            .get(&encode(&key))
            .and_then(|said| said.list.iter().find(|agent| agent.session == from))
        {
            Cow::Borrowed(agent.name.as_str())
        } else {
            Cow::Owned(crate::moves::switched_sender(&self.reader.run, from, &key)?)
        };
        Some((from, name))
    }

    async fn accept_answer(
        &mut self,
        view: &View,
        key: ComputerKey,
        letter: &Letter,
        answered: &Id,
        now: u64,
    ) -> io::Result<bool> {
        let run = self.reader.run.clone();
        let sender = self.sender(view, key, letter.from.as_deref());
        // The engine can already answer for a moved-in conversation while
        // the last full session list still omits it.
        let local_name = if sender.is_none()
            && view.roster.here == Some(key)
            && letter.read.is_none()
            && let Some(from) = letter.from.as_deref()
            && let Ok(engine) = self.engine.session_move().await
            && let Ok(Some(info)) = engine.lookup(from).await
        {
            Some((
                from,
                info["title"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .unwrap_or("New session")
                    .to_owned(),
            ))
        } else {
            None
        };
        let sender = sender.or_else(|| local_name.map(|(from, name)| (from, Cow::Owned(name))));
        // The awaited letter may be the only record left after a restart.
        self.outbox()?
            .answer(&key, answered, now, |sent| {
                if sent.read.is_some() {
                    if !tool_alive(&run, answered, now) {
                        return None;
                    }
                    let words = letter
                        .note
                        .as_ref()
                        .and_then(|note| note.get("words"))
                        .and_then(Value::as_str)
                        .unwrap_or(&letter.text);
                    let mut reply = json!({"text":cut(words, READ_BY_MAIL), "read":true});
                    if letter.refused {
                        reply["isError"] = true.into();
                    }
                    return Some(reply);
                }
                let fallback = unknown_sender(view, &key);
                let (from, name) = sender
                    .as_ref()
                    .map_or(("unknown", fallback.as_str()), |(from, name)| {
                        (*from, name.as_ref())
                    });
                Some(
                    json!({"text":cut(&letter.text, ANSWER_TEXT), "to":sent.from,
                "from":from, "name":name}),
                )
            })
            .map_err(io::Error::other)
    }
}

fn archived(info: &Value) -> bool {
    info.pointer("/metadata/kuvo.archived") == Some(&Value::Bool(true))
        || info.pointer("/metadata/kuvo/archived") == Some(&Value::Bool(true))
        || info
            .pointer("/time/archived")
            .and_then(Value::as_u64)
            .is_some_and(|at| at != 0)
}

async fn started_under(
    engine: &SessionMove,
    info: &Value,
    lead: &str,
) -> Result<bool, kuvo_engine::MoveEngineError> {
    let mut above = info.clone();
    for _ in 0..20 {
        let metadata = &above["metadata"]["kuvo"];
        let Some(parent) = metadata["startedBy"].as_str().filter(|id| !id.is_empty()) else {
            return Ok(false);
        };
        let contains_lead = metadata["line"].as_array().map_or(parent == lead, |line| {
            line.iter()
                .rev()
                .filter_map(Value::as_str)
                .take(20)
                .any(|id| id == lead)
        });
        if contains_lead {
            return Ok(true);
        }
        let Some(parent) = engine.lookup(parent).await? else {
            return Ok(false);
        };
        if parent["parentID"].as_str().is_some_and(|id| !id.is_empty()) {
            return Ok(false);
        }
        above = parent;
    }
    Ok(false)
}

fn letter_hint(address: &str, waiting: bool) -> std::borrow::Cow<'_, str> {
    if address == "unknown" {
        "It can't be answered: who sent it isn't known.".into()
    } else if waiting {
        "They're waiting for your reply: just answer, in your words.".into()
    } else {
        format!("To answer, use send_message to {address}.").into()
    }
}

fn letter_prompt(name: &str, address: &str, text: &str, waiting: bool) -> String {
    let hint = letter_hint(address, waiting);
    format!("[Message from {name} · {address}] ({hint})\n\n{text}")
}

async fn wait_for_answer(
    engine: SessionMove,
    session: String,
    prompt: String,
    addressed: String,
    peer: ComputerKey,
    answers: Id,
) -> Option<(Id, Letter)> {
    let text = time::timeout(Duration::from_millis(LIMIT), async {
        let began = Instant::now();
        let mut running = false;
        loop {
            time::sleep(Duration::from_secs(2)).await;
            if engine.running(&session).await.unwrap_or(false) {
                running = true;
                continue;
            }
            if !running && began.elapsed() < Duration::from_secs(6) {
                continue;
            }
            let messages = engine.recent_messages(&session, 20).await.ok().flatten();
            if let Some(text) = messages
                .as_deref()
                .and_then(|messages| answer_after(messages, &prompt))
            {
                return Some(text);
            }
            if running {
                return None;
            }
        }
    })
    .await
    .ok()
    .flatten()?;
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).ok()?;
    let id = Id::try_from(format!("ltr_{}", encode(&nonce))).ok()?;
    let letter = Letter {
        from: Some(session),
        to: addressed,
        text,
        answers: Some(answers),
        peer: Some(encode(&peer)),
        at: None,
        sent_to: None,
        note: None,
        read: None,
        wait: false,
        refused: false,
        other: Default::default(),
    };
    Some((id, letter))
}

fn answer_after(messages: &[Value], prompt: &str) -> Option<String> {
    let mut text = Vec::new();
    let mut answered = false;
    for message in messages
        .iter()
        .rev()
        .filter(|message| message["id"].as_str().is_some_and(|id| id > prompt))
    {
        match message["type"].as_str() {
            Some("assistant") => {
                let parts = message["content"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let words = parts
                    .iter()
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                answered = !words.is_empty() && !parts.iter().any(|part| part["type"] == "tool");
                if !words.is_empty() {
                    text.push(words);
                }
            }
            Some("user") if answered => break,
            _ => {}
        }
    }
    (!text.is_empty()).then(|| cut(&text.join("\n\n"), ANSWER_TEXT).to_owned())
}

fn unknown_sender(view: &View, key: &ComputerKey) -> String {
    let name = view
        .roster
        .member(key)
        .map(|peer| plain_name(&peer.name))
        .unwrap_or_default();
    let place = if name.is_empty() {
        "another computer"
    } else {
        &name
    };
    format!("an agent on {place}")
}

fn tool_alive(run: &Path, id: &Id, now: u64) -> bool {
    let replies = run.join("replies");
    if replies.join(format!("{}.gaveup", id.as_str())).exists() {
        return false;
    }
    std::fs::metadata(replies.join(format!("{}.alive", id.as_str())))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .is_some_and(|updated| u128::from(now).saturating_sub(updated.as_millis()) <= 10_000)
}

fn failure_context(
    view: &View,
    run: &Path,
    here: ComputerKey,
    id: &Id,
    letter: &Letter,
    now: u64,
) -> FailureContext {
    let peer = letter.sent_to.as_deref();
    let title = peer
        .and_then(|peer| view.announced.get(peer))
        .into_iter()
        .chain(view.announced.values())
        .flat_map(|copy| &copy.list)
        .find(|agent| agent.session == letter.to)
        .map(|agent| agent.name.as_str())
        .or_else(|| {
            view.local
                .list
                .as_ref()?
                .sessions
                .get(&letter.to)?
                .facts
                .title
                .as_deref()
        })
        .unwrap_or_default();
    let mac = is_mac(run);
    let computer = peer
        .and_then(decode_exact)
        .and_then(|peer| {
            if peer == here {
                Some(if mac { "this Mac" } else { "this machine" })
            } else {
                view.roster.member(&peer).map(|peer| peer.name.as_str())
            }
        })
        .unwrap_or_default();
    let tool_waiting = !mac && letter.wait && tool_alive(run, id, now);
    FailureContext {
        conversation: plain_name(title),
        computer: plain_name(computer),
        tool_waiting,
    }
}

fn delivered(id: Id) -> Delivered {
    Delivered {
        id,
        ok: true,
        why: None,
        retry: None,
        answer: None,
        lives: None,
    }
}

fn refused(id: Id, why: &str, retry: bool, lives: Option<String>) -> Delivered {
    Delivered {
        id,
        ok: false,
        why: Some(why.into()),
        retry: retry.then_some(true),
        answer: None,
        lives,
    }
}

fn envelope_field<T: serde::de::DeserializeOwned>(mail: &Envelope, field: &str) -> Option<T> {
    serde_json::from_value(mail.0.get(field)?.clone()).ok()
}

fn cut(text: &str, most: usize) -> &str {
    let mut used = 0;
    for (start, ch) in text.char_indices() {
        used += ch.len_utf16();
        if used > most {
            return &text[..start];
        }
    }
    text
}

fn count(run: &Path, key: &str, at: u64, add: bool) -> io::Result<bool> {
    let _lock = StaleLock::acquire(
        &run.join(".tool-limits.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    let file = run.join("tool-limits.json");
    let mut saved = files::read(&file)?
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let mut messages = saved["messages"].as_object().cloned().unwrap_or_default();
    messages.retain(|_, events| {
        if let Some(events) = events.as_array_mut() {
            events.retain(|value| {
                value
                    .as_u64()
                    .is_some_and(|when| at.saturating_sub(when) < LIMIT)
            });
            return !events.is_empty();
        }
        false
    });
    let events = messages
        .entry(key)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("kept counts are arrays");
    if add && events.len() >= 20 {
        return Ok(false);
    }
    if add {
        events.push(at.into());
    } else if let Some(index) = events.iter().position(|when| when.as_u64() == Some(at)) {
        events.remove(index);
    }
    if events.is_empty() {
        messages.remove(key);
    }
    saved["messages"] = messages.into();
    files::write_atomic(&file, &serde_json::to_vec(&saved)?)?;
    Ok(true)
}
