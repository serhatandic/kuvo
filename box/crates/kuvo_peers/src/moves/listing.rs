use std::{collections::BTreeSet, io, path::Path, time::Duration};

use http::StatusCode;
use kuvo_protocol::routes::{
    get_kuvo_moves, get_kuvo_moves_events, get_kuvo_moves_places, get_kuvo_moves_receipt,
    post_kuvo_moves,
};
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
pub(super) const CLEANUP: &str =
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

pub(crate) fn plain_name(value: &str) -> String {
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

pub(crate) fn is_mac(run: &Path) -> bool {
    std::env::var("KUVO_BOX").as_deref() == Ok("mac")
        || run.parent().is_some_and(|home| {
            std::fs::read_to_string(home.join("box/kind")).is_ok_and(|kind| kind.trim() == "mac")
        })
}

pub(super) fn checkpointed(run: &Path) -> bool {
    super::read_json(&run.join("link.json"))
        .ok()
        .flatten()
        .is_some_and(|link| link["setup"]["checkpointed"] == true && link["setup"]["aside"] != true)
}

pub(super) fn supports_moves(view: &View, key: &str, checkpointed: bool) -> bool {
    let features = view
        .announced
        .get(key)
        .and_then(|entry| entry.features.as_ref());
    view.work_admitted(key)
        && features.is_none_or(|features| features.iter().any(|feature| feature == "moves"))
        && (!checkpointed
            || features.is_some_and(|features| {
                features
                    .iter()
                    .any(|feature| feature == "moves.two-computers")
            }))
}

pub(super) fn needs_update(name: &str) -> String {
    format!("{name} needs a newer Kuvo Code before things can move to or from it.")
}

fn show(record: &Journal, view: &View) -> Option<WithWords> {
    Some(
        record
            .shown_waiting(&view.move_waiting)?
            .with_words(name_of(&view.roster, &record.peer), None),
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
        .filter_map(|record| show(record, view))
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
    status: Option<u16>,
}

enum Report {
    Moving(WithWords),
    Done(WithWords, Option<String>),
    Failed(StatusCode, String),
}

impl Report {
    fn failed(message: String, status: Option<u16>) -> Self {
        let status = status
            .and_then(|status| StatusCode::from_u16(status).ok())
            .unwrap_or(StatusCode::CONFLICT);
        Self::Failed(status, message)
    }

    fn public(self) -> Value {
        match self {
            Self::Moving(shown) => json!({ "state": "moving", "move": shown }),
            Self::Done(shown, note) => {
                let mut result = json!({ "state": "done", "message": format!("Moved to {}.", shown.place), "move": shown });
                if let Some(note) = note {
                    result["note"] = note.into();
                }
                result
            }
            Self::Failed(_, message) => json!({ "state": "failed", "message": message }),
        }
    }
}

fn report(view: &View, run: &Path, id: &str) -> Result<Report, Refusal> {
    let record = journals(run)
        .map_err(format_refusal)?
        .into_iter()
        .find(|record| record.id == id);
    if let Some(record) = record.as_ref().filter(|record| record.role == Role::From)
        && let Some(message) = record.failed.as_ref().filter(|message| !message.is_empty())
    {
        let status = record
            .extra
            .get("failedStatus")
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok());
        return Ok(Report::failed(message.clone(), status));
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
                return Ok(Report::failed(message, outcome.status));
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
        let movement = if done {
            record.shown()
        } else {
            record.shown_waiting(&view.move_waiting)
        };
        (movement, done, note)
    } else {
        return Err(result_unavailable());
    };
    let movement = movement
        .filter(|movement| movement.role == Role::From)
        .ok_or_else(result_unavailable)?;
    let peer = name_of(&view.roster, &movement.to);
    let shown = movement.with_words(peer, None);
    if done {
        Ok(Report::Done(shown, note.filter(|note| !note.is_empty())))
    } else {
        Ok(Report::Moving(shown))
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

pub(super) fn save_outcome(
    run: &Path,
    row: &Journal,
    note: Option<&str>,
    failure: Option<(u16, &str)>,
) -> io::Result<()> {
    let value = if let Some((status, message)) = failure {
        json!({"ok":false, "status":status, "message":message})
    } else {
        let mut value = json!({"ok":true,"move":row.shown()});
        if row.extra.get("verifiedPayloads") == Some(&Value::Bool(true)) {
            value["verifiedPayloads"] = true.into();
        }
        if let Some(note) = note {
            value["note"] = note.into();
        }
        value
    };
    let Value::Object(fields) = value else {
        unreachable!()
    };
    let file = run.join("moves").join(format!("{}.outcome.json", row.id));
    if OUTCOME
        .read(&file)
        .map_err(io::Error::other)?
        .is_some_and(|before| before.fields == fields)
    {
        return Ok(());
    }
    kuvo_store::files::write_atomic_through(
        &file,
        &serde_json::to_vec(&OUTCOME.encode(fields))?,
        run,
    )
}

#[derive(Serialize)]
struct Stays {
    #[serde(skip_serializing_if = "zero")]
    sessions: usize,
    #[serde(skip_serializing_if = "zero")]
    asks: usize,
}

fn zero(count: &usize) -> bool {
    *count == 0
}

#[derive(Deserialize)]
struct WorkAsks {
    asks: Vec<WorkAsk>,
}

#[derive(Deserialize)]
struct WorkAsk {
    id: String,
    action: String,
    session: Option<String>,
    answered: Option<Value>,
}

fn stays_here(view: &View, run: &Path, root: &str) -> Stays {
    let sessions = view.local.list.as_ref().map_or(0, |list| {
        list.sessions
            .keys()
            .filter(|id| {
                let mut above = id.as_str();
                for _ in 0..20 {
                    let Some(row) = list.sessions.get(above).filter(|row| {
                        row.facts.parent.is_none()
                            && row.facts.archived != Some(true)
                            && row.facts.away != Some(true)
                    }) else {
                        break;
                    };
                    let Some(line) = &row.facts.line else {
                        break;
                    };
                    if line.iter().any(|id| id == root) {
                        return true;
                    }
                    let Some(starter) = line.last() else {
                        break;
                    };
                    above = starter;
                }
                false
            })
            .count()
    });
    let asks = crate::kept::read::<WorkAsks>(&run.join("work/asks.json")).map_or(0, |book| {
        book.asks
            .into_iter()
            .filter(|ask| {
                !ask.id.is_empty()
                    && matches!(
                        ask.action.as_str(),
                        "makeProject" | "cloneProject" | "resetLimits"
                    )
                    && ask.session.as_deref() == Some(root)
                    && ask.answered.is_none()
            })
            .count()
    });
    Stays { sessions, asks }
}

pub(crate) fn register_start(
    registry: &mut Registry,
    peers: &Reader,
    driver: super::Handle,
    engine: kuvo_engine::Engine,
) {
    let look = peers.clone();
    registry.register::<get_kuvo_moves_places::Spec, _, _>(Access::Any, move |call| {
        let look = look.clone();
        let engine = engine.clone();
        async move {
            let id = query(&call.request.path, "id").filter(|id| !id.is_empty());
            let kind = query(&call.request.path, "kind").and_then(|kind| serde_json::from_value::<super::Kind>(kind.into()).ok());
            let (Some(id), Some(kind)) = (id, kind) else {
                return Err(Refusal::new(StatusCode::BAD_REQUEST, "Say what: ?kind=bot|chat|group&id=…"));
            };
            if kind != super::Kind::Chat {
                return Err(Refusal::new(StatusCode::NOT_FOUND, "It isn’t on this computer any more."));
            }
            let session = super::driver::chat(&engine, &look.run, &id).await.map_err(|_| Refusal::new(
                StatusCode::NOT_FOUND, "It isn’t on this computer any more.",
            ))?;
            let what = super::driver::chat_name(&session);
            let view = look.view();
            let source = plain_name(&view.roster.name);
            let moving = journals(&look.run).map_err(format_refusal)?.iter().any(|row| row.live() && row.covers(&id));
            let background = super::background::refusal(&look.run, [id.as_str()], &what, look.now());
            let stays = stays_here(&view, &look.run, &id);
            let busy = match engine.session_move().await {
                Ok(engine) => engine.running(&id).await.unwrap_or(false),
                Err(_) => false,
            };
            let waits = if busy { " A turn is running. The move waits up to a minute for it to finish." } else { "" };
            let trash = if is_mac(&look.run) { "the Trash" } else { "Kuvo Code’s trash folder" };
            let accepted = checkpointed(&look.run);
            let places: Vec<_> = view.roster.members.iter().filter(|peer| Some(peer.key) != view.roster.here).map(|peer| {
                let key = kuvo_wire::keys::encode(&peer.key);
                let entry = view.announced.get(&key);
                let shown = plain_name(&peer.name);
                let name = if !shown.is_empty() { &shown } else if peer.kind == crate::session::ComputerKind::Mac { "Mac" } else { "Machine" };
                let why = if moving { Some((super::driver::MOVING.into(), "Moving already")) }
                    else if let Some(reason) = &background { Some((reason.clone(), "Running something")) }
                    else if !supports_moves(&view, &key, accepted) {
                        Some((needs_update(if view.local_work_admitted() { name } else { "This computer" }), "Needs a newer Kuvo Code"))
                    } else if view.reach.peer_is_away(&key, entry.and_then(|entry| entry.heard), look.now()) {
                        Some((format!("{name} isn’t connected. Connect to it, then try again."), "Can’t be reached"))
                    } else { None };
                if let Some((why, note)) = why {
                    json!({"key":key,"name":name,"kind":peer.kind,"ok":false,"why":why,"note":note})
                } else {
                    let mut message = format!("Its conversation and wake-ups move with it.{waits} New messages and wake-ups wait during the move.");
                    if stays.sessions != 0 {
                        message.push_str(&format!(" Sessions it started stay on {source}. It can still reach them from {name}."));
                    }
                    if stays.asks != 0 {
                        message.push_str(&format!(" Its unanswered questions stay on {source}."));
                    }
                    message.push_str(&format!(" Vault items, integrations and browser sign-ins stay on {source}. The original goes to {trash} on {source}."));
                    json!({"key":key,"name":name,"kind":peer.kind,"ok":true,"confirm": {
                        "title":format!("Move {what} to {name}?"), "label":"Move",
                        "message":message,
                    }})
                }
            }).collect();
            let answer = if stays.sessions != 0 || stays.asks != 0 {
                json!({"places":places, "stays":stays})
            } else {
                json!({"places":places})
            };
            Ok(RouteReply::json::<200>(route_shape(answer)?))
        }
    });
    let peers = peers.clone();
    registry.register::<post_kuvo_moves::Spec, _, _>(Access::Any, move |call| {
        let peers = peers.clone();
        let driver = driver.clone();
        async move {
            let request: super::driver::Request = serde_json::from_slice(&call.request.body).map_err(|_| Refusal::new(
                StatusCode::BAD_REQUEST,
                r#"Say what moves where: {"kind": "bot" | "chat" | "group", "id": …, "to": <its computer's key>}."#,
            ))?;
            let immediate = request.receipt;
            let row = driver.begin(request).await?;
            if immediate {
                let move_view = show(&row, &peers.view()).ok_or_else(result_unavailable)?;
                return Ok(RouteReply::json::<202>(route_shape(json!({"move":move_view}))?));
            }
            loop {
                match report(&peers.view(), &peers.run, &row.id)? {
                    Report::Done(shown, note) => {
                        let mut reply = json!({"move":shown.movement});
                        if let Some(note) = note { reply["note"] = note.into(); }
                        return Ok(RouteReply::json::<200>(route_shape(reply)?));
                    }
                    Report::Failed(status, message) => return Err(Refusal::new(status, message)),
                    Report::Moving(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
        }
    });
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
                .map(Report::public)
                .and_then(route_shape)
                .map(RouteReply::json::<200>),
        )
    });
}
