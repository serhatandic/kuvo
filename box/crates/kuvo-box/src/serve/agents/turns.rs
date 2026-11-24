//! Resident entry points for adapter turns.

use http::StatusCode;
use kuvo_agents::{
    inbox::{Delivery, Input},
    permissions::Policy,
    runtime::AdoptedTurn,
    session::{TurnInput, TurnKind},
    session_runs::SessionRuns,
    session_store::SessionStore,
};
use kuvo_protocol::routes::{
    post_kuvo_agents_api_session_id_interrupt as interrupt,
    post_kuvo_agents_api_session_id_prompt as prompt,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Map, Value};
use std::{io, path::PathBuf};

pub(crate) async fn restore(
    runs: &SessionRuns,
    store: &SessionStore,
    home: &std::path::Path,
    adopted: &[AdoptedTurn],
) {
    let mut restoring = tokio::task::JoinSet::new();
    for recovered in adopted {
        let Some(turn) = recovered.note.turn.clone() else {
            continue;
        };
        let runs = runs.clone();
        let store = store.clone();
        let home = home.to_owned();
        let recovered = recovered.clone();
        restoring.spawn(async move {
            let id = recovered.note.session.clone();
            let result = async {
                let Some(session) = store.read(&id).await.map_err(super::refuse)? else {
                    return Ok(());
                };
                let Some(directory) = session.directory().map(PathBuf::from) else {
                    return Ok(());
                };
                let (mut input, policy) = turn_setup(
                    &home,
                    directory,
                    &session.into_fields(),
                    &turn,
                    TurnKind::Recover {
                        note: recovered.note,
                    },
                )?;
                input.model.provider = recovered.provider.to_string();
                let run = runs
                    .run_turn(id.clone(), input, policy)
                    .await
                    .map_err(turn_error)?;
                tokio::time::timeout(std::time::Duration::from_secs(10), run.ready())
                    .await
                    .map_err(|error| turn_error(io::Error::other(error)))?
                    .map_err(turn_error)
            }
            .await;
            if let Err(error) = result {
                eprintln!(
                    "Kuvo could not replay coding-agent turn {id}: {}",
                    error.reply().status
                );
            }
        });
    }
    while restoring.join_next().await.is_some() {}
}

pub(crate) fn register(runs: SessionRuns, routes: &mut Registry) {
    let as_turns = runs.clone();
    routes.register::<prompt::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = as_turns.clone();
        async move {
            let id = session_id(call.param("id"))?;
            let input = read_input(&call.body)?;
            let handoff = call.body["handoffToken"].as_str().map(str::to_owned);
            runs.prompt(id, input, handoff)
                .await
                .map_err(super::refuse)?;
            Ok(RouteReply::json::<200>(prompt::Status200 {}))
        }
    });
    routes.register::<interrupt::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = runs.clone();
        async move {
            let id = session_id(call.param("id"))?;
            let resume = call
                .request
                .path
                .split_once('?')
                .is_some_and(|(_, query)| query.split('&').any(|pair| pair == "resume=true"));
            let interrupted = runs.interrupt_and(id, resume).await.map_err(turn_error)?;
            Ok(RouteReply::json::<200>(interrupt::Status200 {
                interrupted,
            }))
        }
    });
}

pub(super) fn turn_setup(
    home: &std::path::Path,
    folder: PathBuf,
    header: &Map<String, Value>,
    turn: &str,
    kind: TurnKind,
) -> Result<(TurnInput, Policy), Refusal> {
    kuvo_agents::turn_setup::turn_setup(home, folder, header, turn, kind).map_err(super::refuse)
}

/// A message for the conversation's line, as the box hands it over.
pub(super) fn read_input(body: &Value) -> Result<Input, Refusal> {
    let id = match body["id"].as_str() {
        Some(id)
            if !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) =>
        {
            id.to_owned()
        }
        Some(_) => {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "Give the message’s ID.",
            ));
        }
        None => SessionStore::make_id("msg").map_err(super::refuse)?,
    };
    let text = match &body["text"] {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let files = body["files"].as_array().cloned().unwrap_or_default();
    let delivery = match body["delivery"].as_str() {
        Some("queue") => Delivery::Queue,
        _ => Delivery::Steer,
    };
    let mut input = Input::new(id, text, files, delivery);
    input.metadata = body["metadata"].as_object().cloned();
    Ok(input)
}

pub(super) fn session_id(raw: Option<&str>) -> Result<String, Refusal> {
    let raw = raw.unwrap_or_default();
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| super::sessions::missing(raw))
}

fn turn_error(error: io::Error) -> Refusal {
    if error.kind() == io::ErrorKind::AlreadyExists {
        Refusal::new(StatusCode::CONFLICT, error.to_string())
    } else {
        super::refuse(error)
    }
}
