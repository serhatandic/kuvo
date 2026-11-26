//! Snapshots of moves announced by authenticated setup peers.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use kuvo_store::format::{self, ObjectFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    Role, Shown, journals,
    listing::{WithWords, name_of, place_name, plain_name},
};
use crate::{
    directory::{ComputerKey, Roster},
    reach::Millis,
};

const ELSEWHERE: ObjectFormat = ObjectFormat::additive("Moves on other computers");
const KEEP_NOTICES: Millis = 180_000;
const MOST_MOVES: usize = 20;
const MOST_SESSIONS: usize = 50;

#[derive(Clone, Deserialize, Serialize)]
struct Said {
    at: Millis,
    moves: Vec<Shown>,
}

#[derive(Clone, Default)]
pub(crate) struct Notices(BTreeMap<String, Said>);

#[derive(Serialize)]
pub(super) struct Elsewhere<'a> {
    #[serde(rename = "box")]
    computer: &'a str,
    moves: Vec<WithWords>,
}

impl Notices {
    fn path(run: &Path) -> std::path::PathBuf {
        run.join("moves/elsewhere.json")
    }

    pub(crate) fn load(run: &Path) -> Self {
        let Ok(Some(record)) = ELSEWHERE.read(&Self::path(run)) else {
            return Self::default();
        };
        Self(
            record
                .fields
                .into_iter()
                .filter_map(|(key, value)| Some((key, serde_json::from_value(value).ok()?)))
                .collect(),
        )
    }

    pub(crate) fn take(
        &mut self,
        run: &Path,
        roster: &Roster,
        key: ComputerKey,
        value: Value,
        now: Millis,
    ) -> std::io::Result<()> {
        let Some(moves) = value.get("moves").and_then(Value::as_array) else {
            return Ok(());
        };
        // Don't overwrite a copy that a newer resident wrote.
        if let Err(error @ format::Error::Unsupported { .. }) = ELSEWHERE.read(&Self::path(run)) {
            return Err(std::io::Error::other(error));
        }
        let mut next = self.0.clone();
        next.retain(|key, _| name_of(roster, key).is_some());
        let moves = moves
            .iter()
            .take(MOST_MOVES)
            .filter_map(taken_move)
            .collect();
        next.insert(kuvo_wire::keys::encode(&key), Said { at: now, moves });
        self.persist(run, next)
    }

    pub(crate) fn next_expiry(&self) -> Option<Millis> {
        self.0
            .values()
            .filter(|said| !said.moves.is_empty())
            .map(|said| said.at.saturating_add(KEEP_NOTICES))
            .min()
    }

    pub(crate) fn expire(&mut self, run: &Path, now: Millis) -> std::io::Result<()> {
        if !self
            .0
            .values()
            .any(|said| !said.moves.is_empty() && now.saturating_sub(said.at) >= KEEP_NOTICES)
        {
            return Ok(());
        }
        let mut next = self.0.clone();
        for said in next
            .values_mut()
            .filter(|said| now.saturating_sub(said.at) >= KEEP_NOTICES)
        {
            said.moves.clear();
        }
        self.persist(run, next)
    }

    fn persist(&mut self, run: &Path, next: BTreeMap<String, Said>) -> std::io::Result<()> {
        if let Err(error @ format::Error::Unsupported { .. }) = ELSEWHERE.read(&Self::path(run)) {
            return Err(std::io::Error::other(error));
        }
        let fields = next
            .iter()
            .map(|(key, said)| Ok((key.clone(), serde_json::to_value(said)?)))
            .collect::<serde_json::Result<_>>()?;
        crate::kept::write(&Self::path(run), &ELSEWHERE.encode(fields))?;
        self.0 = next;
        Ok(())
    }

    pub(super) fn has(&self, key: &ComputerKey) -> bool {
        self.0.contains_key(&kuvo_wire::keys::encode(key))
    }

    pub(super) fn visible(&self, roster: &Roster, now: Millis) -> Vec<Elsewhere<'_>> {
        let here = roster.here.map(|key| kuvo_wire::keys::encode(&key));
        let mut boxes: Vec<_> = self
            .0
            .iter()
            .filter_map(|(key, said)| {
                if name_of(roster, key).is_none() || now.saturating_sub(said.at) >= KEEP_NOTICES {
                    return None;
                }
                let moves: Vec<_> = said
                    .moves
                    .iter()
                    .filter(|movement| {
                        here.as_ref()
                            .is_none_or(|here| &movement.from != here && &movement.to != here)
                    })
                    .cloned()
                    .map(|movement| {
                        let peer = if movement.role == Role::From {
                            &movement.to
                        } else {
                            &movement.from
                        };
                        let computer = name_of(roster, peer);
                        let destination = place_name(name_of(roster, &movement.to));
                        movement.with_words(computer, Some(&destination))
                    })
                    .collect();
                (!moves.is_empty()).then_some(Elsewhere {
                    computer: key,
                    moves,
                })
            })
            .collect();
        // Accepted keys are ASCII base64url. Node's default ICU collation ignores case at
        // the primary level, then puts lowercase before uppercase.
        boxes.sort_by(|a, b| compare_key(a.computer, b.computer));
        boxes
    }
}

fn compare_key(a: &str, b: &str) -> std::cmp::Ordering {
    fn primary(byte: u8) -> u8 {
        match byte {
            b'_' => 0,
            b'-' => 1,
            _ => byte.to_ascii_lowercase(),
        }
    }
    a.bytes()
        .map(primary)
        .cmp(b.bytes().map(primary))
        .then_with(|| {
            a.bytes()
                .map(|byte| byte.is_ascii_uppercase())
                .cmp(b.bytes().map(|byte| byte.is_ascii_uppercase()))
        })
}

fn taken_move(value: &Value) -> Option<Shown> {
    // The peer snapshot allows malformed optional fields and non-string session elements.
    let mut fields = value.as_object()?.clone();
    let sessions = fields.get_mut("sessions")?.as_array_mut()?;
    *sessions = sessions
        .iter()
        .filter_map(Value::as_str)
        .take(MOST_SESSIONS)
        .map(|session| Value::String(short(session, 100)))
        .collect();
    for field in ["sent", "size"] {
        if fields.get(field).is_some_and(|value| !value.is_number()) {
            fields.remove(field);
        }
    }
    if fields.get("waiting").and_then(Value::as_bool) != Some(true) {
        fields.remove("waiting");
    }
    let mut movement: Shown = serde_json::from_value(fields.into()).ok()?;
    movement.id = short(&movement.id, 100);
    movement.thing = short(&movement.thing, 200);
    movement.name = short(&plain_name(&movement.name), 200);
    movement.from = short(&movement.from, 100);
    movement.to = short(&movement.to, 100);
    Some(movement)
}

fn short(text: &str, most: usize) -> String {
    let mut remaining = most;
    text.chars()
        .take_while(|character| {
            if let Some(rest) = remaining.checked_sub(character.len_utf16()) {
                remaining = rest;
                true
            } else {
                false
            }
        })
        .collect()
}

/// Announce on a journal/stage change or an explicit peer request. Active copies
/// keep their own one-minute wire deadline; an empty acknowledged list has none.
#[derive(Default)]
pub(crate) struct Announcer {
    sent: Option<([u8; 32], tokio::time::Instant)>,
}

impl Announcer {
    pub(crate) fn repeat(&mut self) {
        self.sent = None;
    }

    pub(crate) async fn announce(
        &mut self,
        run: &Path,
        roster: &Roster,
        waiting: &BTreeSet<String>,
        relay: &crate::relay::Sender,
        now: Millis,
    ) -> Option<tokio::time::Instant> {
        use std::time::Duration;
        use tokio::time::Instant;
        let current = announcement(run, waiting)?;
        let ticks = Instant::now();
        let every = Duration::from_secs(60);
        if let Some((_, at)) = self.sent.filter(|(digest, _)| *digest == current.digest) {
            if !current.active { return None; }
            let due = at + every;
            if ticks < due { return Some(due); }
        }
        let mut sent = roster.members.is_empty();
        let message = kuvo_wire::mail::Envelope(serde_json::Map::from_iter([
            ("at".into(), json!(now)),
            ("moving".into(), json!({"moves": current.moves})),
        ]));
        for host in roster
            .members
            .iter()
            .filter_map(crate::directory::Member::host)
        {
            sent |= relay.mail(host, message.clone()).await.is_ok();
        }
        if sent {
            self.sent = Some((current.digest, ticks));
            current.active.then(|| ticks + every)
        } else if roster.members.iter().any(|member| member.host().is_some()) {
            // Retry only the actual snapshot that failed to get to the relay.
            Some(ticks + Duration::from_secs(1))
        } else {
            None
        }
    }
}

/// A full snapshot, including the first empty one and the empty one after the
/// last move. A progress byte count doesn't count as a change in stage.
struct Announcement {
    moves: Value,
    digest: [u8; 32],
    active: bool,
}

fn announcement(run: &Path, waiting: &BTreeSet<String>) -> Option<Announcement> {
    use sha2::{Digest, Sha256};

    let moves: Vec<_> = journals(run)
        .ok()?
        .iter()
        .filter(|record| record.listed())
        .take(MOST_MOVES)
        .filter_map(|record| record.shown_waiting(waiting))
        .map(|mut movement| {
            movement.sessions.truncate(MOST_SESSIONS);
            movement
        })
        .collect();
    let changes: Vec<_> = moves
        .iter()
        .map(|movement| (&movement.id, movement.stage, movement.waiting == Some(true)))
        .collect();
    let digest = Sha256::digest(serde_json::to_vec(&changes).ok()?).into();
    Some(Announcement {
        active: !moves.is_empty(),
        moves: serde_json::to_value(moves).ok()?,
        digest,
    })
}
