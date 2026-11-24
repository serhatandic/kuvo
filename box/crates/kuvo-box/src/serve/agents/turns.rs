//! Resident entry points for adapter turns.

use http::StatusCode;
use kuvo_agents::{
    inbox::{Delivery, Input},
    session_runs::SessionRuns,
    session_store::SessionStore,
};
use kuvo_protocol::routes::{
    post_kuvo_agents_api_session_id_compact as compact,
    post_kuvo_agents_api_session_id_interrupt as interrupt,
    post_kuvo_agents_api_session_id_prompt as prompt,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::Value;
use std::io;

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
    let summaries = runs.clone();
    routes.register::<compact::Spec, _, _>(Access::DesktopOrPaired, move |call| {
        let runs = summaries.clone();
        async move {
            let id = session_id(call.param("id"))?;
            let state = runs.compact(id).await.map_err(super::refuse)?;
            Ok(RouteReply::json::<200>(compact::Status200 {
                data: kuvo_protocol::routes::types::GetKuvoMarketplaceAppsIdStatusStatus200 {
                    state: state.as_str().into(),
                },
            }))
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
