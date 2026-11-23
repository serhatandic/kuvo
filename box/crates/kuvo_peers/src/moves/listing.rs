use std::{collections::BTreeSet, path::Path};

use http::StatusCode;
use kuvo_protocol::routes::{get_kuvo_moves, get_kuvo_moves_events, get_kuvo_moves_receipt};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::format::{self, ObjectFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Journal, ROUTING_FOR, Role, Shown, Stage, departures, journals, move_id};
use crate::{
    directory::Roster,
    reach::Millis,
    routes::{follow, query},
    setup_lists::{Reader, View},
};

const OUTCOME: ObjectFormat = ObjectFormat::additive("Move outcome");
const CLEANUP: &str =
    "The original is retired. Kuvo will try again to clean up its files on the source computer.";

#[derive(Serialize)]
pub(super) struct WithWords {
    #[serde(flatten)]
    movement: Shown,
    place: String,
    progress: String,
}

impl Shown {
    pub(super) fn with_words(self, peer: Option<&str>, destination: Option<&str>) -> WithWords {
        let place = place_name(peer);
        let to = match self.role {
            Role::From => &place,
            Role::To => destination.unwrap_or("this computer"),
        };
        let progress = if self.waiting == Some(true) {
            format!("Waiting for {place}…")
        } else {
            match self.stage {
                Stage::Holding => format!("Getting ready to move to {to}…"),
                Stage::Copying => format!("Moving to {to}…"),
                Stage::Switching | Stage::Finishing => format!("Finishing the move to {to}…"),
            }
        };
        WithWords {
            movement: self,
            place,
            progress,
        }
    }
}

pub(super) fn plain_name(value: &str) -> String {
    value
        .replace(
            |c: char| {
                matches!(c, '\u{0000}'..='\u{001f}' | '\u{007f}' | '\u{200e}' | '\u{200f}' |
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            },
            " ",
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn place_name(name: Option<&str>) -> String {
    let name = plain_name(name.unwrap_or_default());
    if name.is_empty() {
        "the other computer".into()
    } else {
        name
    }
}

pub(super) fn name_of<'a>(roster: &'a Roster, key: &str) -> Option<&'a str> {
    let key = kuvo_wire::keys::decode_exact(key)?;
    roster.member(&key).map(|member| member.name.as_str())
}

fn show(record: &Journal, roster: &Roster) -> Option<WithWords> {
    Some(
        record
            .shown()?
            .with_words(name_of(roster, &record.peer), None),
    )
}

fn list(view: &View, run: &Path, now: Millis) -> Result<Value, Refusal> {
    let records = journals(run).map_err(format_refusal)?;
    let mut seen = BTreeSet::new();
    let away: Vec<_> = records
        .iter()
        .filter(|record| record.away())
        .flat_map(|record| &record.sessions)
        .filter(|id| seen.insert(*id))
        .collect();
    let moves: Vec<_> = records
        .iter()
        .filter(|record| record.listed())
        .filter_map(|record| show(record, &view.roster))
        .collect();
    let moved: Vec<_> = departures(run)
        .map_err(format_refusal)?
        .into_iter()
        .filter(|record| {
            record.arrived != Some(true)
                && record.queue.is_none()
                && record.session.starts_with("ses_")
                && now.saturating_sub(record.at) < ROUTING_FOR
        })
        .collect();
    let pending = view.pending("line.pending-computers", |key| view.move_notices.has(key));
    let elsewhere = view.move_notices.visible(&view.roster, now);
    let mut result = json!({ "pendingComputers": pending, "away": away, "moves": moves });
    if !moved.is_empty() {
        result["moved"] = json!(moved);
    }
    if !elsewhere.is_empty() {
        result["elsewhere"] = json!(elsewhere);
    }
    Ok(result)
}

#[derive(Deserialize)]
struct Outcome {
    ok: bool,
    #[serde(rename = "move")]
    movement: Option<Shown>,
    note: Option<String>,
    message: Option<String>,
}

fn dead_end(message: String) -> Value {
    json!({ "state": "failed", "message": message })
}

fn report(view: &View, run: &Path, id: &str) -> Result<Value, Refusal> {
    let record = journals(run)
        .map_err(format_refusal)?
        .into_iter()
        .find(|record| record.id == id);
    if let Some(message) = record
        .as_ref()
        .filter(|record| record.role == Role::From)
        .and_then(|record| record.failed.as_ref())
        .filter(|message| !message.is_empty())
    {
        return Ok(dead_end(message.clone()));
    }
    let kept = match move_id(id) {
        Some(_) => match OUTCOME.read(&run.join("moves").join(format!("{id}.outcome.json"))) {
            Ok(Some(versioned)) => serde_json::from_value::<Outcome>(versioned.fields.into()).ok(),
            Err(error @ format::Error::Unsupported { .. }) => return Err(format_refusal(error)),
            _ => None,
        },
        None => None,
    };
    let (movement, done, note) = if let Some(outcome) = kept {
        if !outcome.ok {
            if let Some(message) = outcome.message {
                return Ok(dead_end(message));
            }
            return Err(result_unavailable());
        }
        (outcome.movement, true, outcome.note)
    } else if let Some(record) = record {
        let done = record.role == Role::From && record.stage == Stage::Finishing;
        let note = if done && record.cleanup_pending != Some(false) {
            Some(
                match record.note.as_deref().filter(|note| !note.is_empty()) {
                    Some(note) => format!("{note} {CLEANUP}"),
                    None => CLEANUP.into(),
                },
            )
        } else {
            record.note.clone()
        };
        (record.shown(), done, note)
    } else {
        return Err(result_unavailable());
    };
    let movement = movement
        .filter(|movement| movement.role == Role::From)
        .ok_or_else(result_unavailable)?;
    let peer = name_of(&view.roster, &movement.to);
    let shown = movement.with_words(peer, None);
    if done {
        let mut result = json!({ "state": "done", "message": format!("Moved to {}.", shown.place), "move": shown });
        if let Some(note) = note.filter(|note| !note.is_empty()) {
            result["note"] = note.into();
        }
        Ok(result)
    } else {
        Ok(json!({ "state": "moving", "move": shown }))
    }
}

fn result_unavailable() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "The move’s result isn’t available. Check where it is before trying again.",
    )
}

fn format_refusal(error: format::Error) -> Refusal {
    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn route_shape<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

pub(crate) fn register(registry: &mut Registry, peers: &Reader) {
    let state = peers.clone();
    registry.register::<get_kuvo_moves::Spec, _, _>(Access::Any, move |_| {
        std::future::ready(
            list(&state.view(), &state.run, state.now())
                .and_then(route_shape)
                .map(RouteReply::json::<200>),
        )
    });
    let run = peers.run.clone();
    follow::<get_kuvo_moves_events::Spec>(
        registry,
        peers,
        move |view, now| {
            list(view, &run, now)
                .ok()
                .and_then(|value| route_shape(value).ok())
        },
        None,
    );
    let state = peers.clone();
    registry.register::<get_kuvo_moves_receipt::Spec, _, _>(Access::Any, move |call| {
        let id = query(&call.request.path, "id").unwrap_or_default();
        std::future::ready(
            report(&state.view(), &state.run, &id)
                .and_then(route_shape)
                .map(RouteReply::json::<200>),
        )
    });
}
