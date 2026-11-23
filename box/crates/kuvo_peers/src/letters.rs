//! Durable ordinary letters and notes. Resolve ownership again for each send
//! and immediately before placing an incoming prompt on a local session.

use std::{io, path::Path, sync::Arc, time::Duration};

use kuvo_engine::Engine;
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
use tokio::{
    sync::broadcast,
    task::JoinHandle,
    time::{self, MissedTickBehavior},
};

use crate::{
    directory::ComputerKey,
    outbox::{Delivered, Delivery, Id, IncomingLetter, Outbox, Receipts},
    ownership::Handle,
    relay::{RelayEvent, RelayHandle},
    setup_lists::{Reader, View},
};

const MOVED: &str = "It's moving to another computer: it comes again once it's there.";
const LIMIT: u64 = 600_000;

pub fn start(
    reader: Reader,
    relay: RelayHandle,
    engine: Engine,
    ownership: Handle,
) -> io::Result<JoinHandle<()>> {
    let events = relay
        .subscribe()
        .ok_or_else(|| io::Error::other("The resident relay stopped"))?;
    let post = Letters {
        reader,
        relay,
        engine,
        ownership,
        outbox: None,
        receipts: None,
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
}

#[derive(Deserialize)]
struct Note {
    from: String,
    words: String,
    delivery: NoteDelivery,
    line: Option<String>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum NoteDelivery {
    Queue,
    Steer,
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
                        _ => Ok(()),
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => Ok(()),
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = tick.tick() => self.send_due().await,
            };
            if let Err(error) = result {
                eprintln!("Keeping letters: {error}");
            }
        }
    }

    fn open_outbox(&mut self) -> io::Result<Option<&mut Outbox>> {
        if self.outbox.is_none() && self.reader.run.join("outbox/sending").try_exists()? {
            self.outbox = Some(Outbox::open(&self.reader.run).map_err(io::Error::other)?);
        }
        Ok(self.outbox.as_mut())
    }

    fn receipts(&mut self) -> io::Result<&mut Receipts> {
        if self.receipts.is_none() {
            self.receipts = Some(Receipts::open(&self.reader.run).map_err(io::Error::other)?);
        }
        Ok(self.receipts.as_mut().expect("opened receipts for letters"))
    }

    async fn send_due(&mut self) -> io::Result<()> {
        let view = self.reader.view();
        let now = self.reader.now().max(0) as u64;
        let Some(outbox) = self.open_outbox()? else {
            return Ok(());
        };
        let pending = outbox.due(now, true).map_err(io::Error::other)?;
        for each in pending {
            let Ok(scope) = self.ownership.scope(&each.letter.to, None) else {
                continue;
            };
            let owner = scope.owner;
            if scope.held && owner.as_ref().is_none_or(|owner| owner.local) {
                continue;
            }
            let known = owner.as_ref().and_then(|owner| owner.key);
            let key = known.or_else(|| each.letter.peer.as_deref().and_then(decode_exact));
            let Some(key) = key else {
                continue;
            };
            let here = view.roster.here == Some(key);
            let host = (!here)
                .then(|| view.roster.member(&key).and_then(|peer| peer.host()))
                .flatten();
            if !here && (!view.relay_connected || host.is_none()) {
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
                let _ = self.relay.sender.mail(host, mail).await;
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

    async fn receive(&mut self, key: ComputerKey, envelope: &Envelope) -> io::Result<()> {
        let view = self.reader.view();
        let Some(host) = view.roster.member(&key).and_then(|peer| peer.host()) else {
            return Ok(());
        };
        let now = self.reader.now().max(0) as u64;
        if let Some(delivered) = envelope_field::<Delivered>(envelope, "delivered")
            && let Some(outbox) = self.open_outbox()?
        {
            outbox
                .settle(&key, &delivered, now)
                .map_err(io::Error::other)?;
        }
        if let Some(letter) = envelope_field::<IncomingLetter>(envelope, "letter")
            && let Some(reply) = self.take(&view, key, letter).await?
        {
            let _ = self.relay.sender.mail(host, reply.envelope(now)).await;
        }
        Ok(())
    }

    async fn take(
        &mut self,
        view: &View,
        key: ComputerKey,
        mut envelope: IncomingLetter,
    ) -> io::Result<Option<Delivered>> {
        let now = self.reader.now().max(0) as u64;
        // Remote work calls and seats own their own dispatch and receipts.
        if envelope.letter.other.contains_key("call")
            || envelope.letter.other.contains_key("seat")
            || envelope.letter.read.is_some()
            || envelope.letter.answers.is_some()
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
        let archived = session.pointer("/metadata/kuvo.archived") == Some(&Value::Bool(true))
            || session.pointer("/metadata/kuvo/archived") == Some(&Value::Bool(true))
            || session
                .pointer("/time/archived")
                .and_then(Value::as_u64)
                .is_some_and(|at| at != 0);
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
        let mut sender_known = false;
        let (prompt, from, words, steer, metadata) = if let Some(ref note) = note {
            let from = cut(&note.from, 200).to_owned();
            let metadata = note
                .line
                .as_ref()
                .map(|line| json!({"kuvo":{"note":{"from":from,"line":cut(line,32_000)}}}));
            (
                text,
                from,
                cut(&note.words, 32_000).to_owned(),
                note.delivery == NoteDelivery::Steer,
                metadata,
            )
        } else {
            let from = letter.from.as_deref().and_then(|from| {
                let owner = self.ownership.scope(from, None).ok()?.owner;
                let agent = view
                    .announced
                    .get(&key_text)?
                    .list
                    .iter()
                    .find(|agent| agent.session == from)?;
                (owner?.key == Some(key)).then_some((from, agent.name.as_str()))
            });
            sender_known = from.is_some();
            let name = from.map_or_else(
                || {
                    let computer = view
                        .roster
                        .member(&key)
                        .map(|peer| peer.name.split_whitespace().collect::<Vec<_>>().join(" "));
                    format!(
                        "an agent on {}",
                        computer
                            .as_deref()
                            .filter(|name| !name.is_empty())
                            .unwrap_or("another computer")
                    )
                },
                |(_, name)| name.to_owned(),
            );
            let address = from.map_or("unknown", |(from, _)| from);
            let hint = if address == "unknown" {
                "It can't be answered: who sent it isn't known.".into()
            } else if letter.wait {
                "They're waiting for your reply: just answer, in your words.".into()
            } else {
                format!("To answer, use send_message to {address}.")
            };
            let prompt = format!("[Message from {name} · {address}] ({hint})\n\n{text}");
            let metadata = json!({"kuvo":{"parts":{"words":text,"mail":{"name":name,"session":address,"hint":hint}}}});
            (prompt, name, text, true, Some(metadata))
        };
        let Delivery::Submit { prompt: prompt_id } = self
            .receipts()?
            .begin(&id, note.is_some(), now)
            .map_err(io::Error::other)?
        else {
            return Ok(Some(delivered(id)));
        };
        match engine
            .deliver_letter(&letter.to, prompt_id, prompt, from, words, steer, metadata)
            .await
        {
            Ok(kept) => {
                self.receipts()?
                    .accepted(&id, note.is_some(), now)
                    .map_err(io::Error::other)?;
                let mut receipt = delivered(id);
                if note.is_none() && letter.wait && (!sender_known || kept) {
                    receipt.answer = Some(false);
                    receipt.why = Some(if sender_known {
                        "That conversation has a paused queue. Your message waits for the conversation to resume. Its answer, if it writes one, comes to you as a message."
                    } else {
                        "Its answer can't come back to you: it doesn't know who you are."
                    }.into());
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
