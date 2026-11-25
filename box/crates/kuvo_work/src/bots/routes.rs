//! `kuvo/bots` and its events: this box's bots, each with what it's doing now.

use kuvo_os::timer_fires::{self, Timer};
use std::time::Duration;

use http::StatusCode;
use kuvo_protocol::routes::{
    delete_kuvo_bots_id as delete, get_kuvo_bots as list, get_kuvo_bots_events as events,
    get_kuvo_bots_templates as templates, patch_kuvo_bots_id as change, post_kuvo_bots as make,
    post_kuvo_bots_id_arrived as arrived, post_kuvo_bots_id_seen as seen,
    post_kuvo_bots_retell as retell,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::Value;

use super::{Bots, held};

/// How often the list is looked at again with nothing said to change (another process's writes).
const EVERY: Duration = Duration::from_secs(3);
/// A run's end and what follows it a moment after are one change.
const SETTLES: Duration = Duration::from_millis(250);

async fn said(bots: &Bots) -> Result<(Value, Option<i64>), Refusal> {
    let bots = bots.clone();
    tokio::task::spawn_blocking(move || bots.said())
        .await
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn refused((status, message): (u16, String)) -> Refusal {
    Refusal::new(
        StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        message,
    )
}

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

/// A bot's id from its path, as it was sent.
fn bot_of<T>(call: &kuvo_server::RouteCall<T>) -> Result<String, Refusal> {
    call.param("id")
        .and_then(|id| percent_encoding::percent_decode_str(id).decode_utf8().ok())
        .map(|id| id.into_owned())
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "That bot isn’t there any more."))
}

/// A request's body read as JSON, as TypeScript's `parsed` reads it: none for none or what isn't.
fn body_of<T>(call: &kuvo_server::RouteCall<T>) -> Value {
    serde_json::from_slice(&call.request.body).unwrap_or(Value::Null)
}

/// A change or delete for a bot on another of your computers, held here for it: `on` names that
/// computer for one this box hasn't heard of yet, `removed` one it doesn't know now.
async fn hold<T>(
    bots: &Bots,
    id: &str,
    given: held::Given,
    call: &kuvo_server::RouteCall<T>,
) -> Result<(u16, Value), Refusal> {
    let query = call
        .request
        .path
        .split_once('?')
        .map_or("", |(_, query)| query);
    let pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_encoding::percent_decode_str(&value.replace('+', " "))
                .decode_utf8_lossy()
                .into_owned();
            (key.to_owned(), value)
        })
        .collect();
    let on = pairs
        .iter()
        .find(|(key, _)| key == "on")
        .map(|(_, value)| value.as_str());
    let removed = pairs.iter().any(|(key, _)| key == "removed");
    let on = bots.owner_elsewhere(id).or(on.map(str::to_owned));
    bots.hold(id, given, on.as_deref(), removed, &call.request.caller)
        .await
        .map_err(refused)
}

/// The routes of the bots' keys of their own, gone: bots use this computer's integrations now.
fn keys_route(path: &str) -> bool {
    if path == "kuvo/bots/keys/refresh" {
        return true;
    }
    let Some(rest) = path.strip_prefix("kuvo/bots/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    matches!(parts.as_slice(), [id, "keys"] | [id, "keys", _] if !id.is_empty())
        && parts.iter().all(|part| !part.is_empty())
}

fn register_changes(bots: &Bots, routes: &mut Registry) {
    let here = bots.clone();
    routes.register::<templates::Spec, _, _>(Access::Any, move |_| {
        let bots = here.clone();
        async move { Ok(RouteReply::json::<200>(typed(bots.templates().await?)?)) }
    });
    let here = bots.clone();
    routes.register::<make::Spec, _, _>(Access::Any, move |call| {
        let bots = here.clone();
        async move {
            let made = bots.make(body_of(&call)).await.map_err(refused)?;
            Ok(RouteReply::json::<201>(typed(made)?))
        }
    });
    let here = bots.clone();
    routes.register::<change::Spec, _, _>(Access::Any, move |call| {
        let bots = here.clone();
        async move {
            let id = bot_of(&call)?;
            if bots.lives_elsewhere(&id) {
                let given = held::Given::Change(body_of(&call));
                let (_, held) = hold(&bots, &id, given, &call).await?;
                return Ok(RouteReply::json::<202>(typed(held)?));
            }
            let changed = bots.change(&id, body_of(&call)).await.map_err(refused)?;
            Ok(RouteReply::json::<200>(typed(changed)?))
        }
    });
    let here = bots.clone();
    routes.register::<delete::Spec, _, _>(Access::Any, move |call| {
        let bots = here.clone();
        async move {
            let id = bot_of(&call)?;
            if bots.lives_elsewhere(&id) {
                let (_, held) = hold(&bots, &id, held::Given::Delete, &call).await?;
                return Ok(RouteReply::json::<202>(typed(held)?));
            }
            let deleted = bots.delete(&id).await.map_err(refused)?;
            Ok(RouteReply::json::<200>(typed(deleted)?))
        }
    });
    let here = bots.clone();
    routes.register::<arrived::Spec, _, _>(Access::Any, move |call| {
        let bots = here.clone();
        async move {
            let id = bot_of(&call)?;
            let settled = bots.settle(&id).await.map_err(refused)?;
            Ok(RouteReply::json::<200>(typed(settled)?))
        }
    });
    let here = bots.clone();
    routes.register::<retell::Spec, _, _>(Access::Any, move |_| {
        let bots = here.clone();
        async move {
            bots.freshen_all().await;
            Ok(RouteReply::json::<200>(typed(serde_json::json!({}))?))
        }
    });
    let here = bots.clone();
    routes.register::<seen::Spec, _, _>(Access::Any, move |call| {
        let bots = here.clone();
        async move {
            let id = bot_of(&call)?;
            bots.seen_now(&id).await.map_err(refused)?;
            Ok(RouteReply::json::<200>(typed(serde_json::json!({}))?))
        }
    });
    routes.retire(keys_route, "Bots use this computer's integrations now.");
}

pub(super) fn register(bots: Bots, routes: &mut Registry) {
    register_changes(&bots, routes);
    let here = bots.clone();
    routes.register::<list::Spec, _, _>(Access::Any, move |_| {
        let bots = here.clone();
        async move {
            let (value, _) = said(&bots).await?;
            let typed = serde_json::from_value(value).map_err(|error| {
                Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            })?;
            Ok(RouteReply::json::<200>(typed))
        }
    });
    let here = bots;
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let bots = here.clone();
        async move {
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut mine = bots.changes();
                let mut wakes = bots.inner.wakes.changes();
                let mut sessions = bots.inner.sessions.clone();
                let mut peers = bots.inner.peers.clone();
                let mut previous: Option<Value> = None;
                loop {
                    let (value, turns) = said(&bots).await.map_err(|_| std::io::Error::other("The bots couldn't be read."))?;
                    if previous.as_ref() != Some(&value) {
                        let line = JsonLine::new(serde_json::from_value(value.clone())?)?;
                        previous = Some(value);
                        yield line;
                    }
                    let now = jiff::Timestamp::now().as_millisecond();
                    let turning = turns.map_or(EVERY, |at| Duration::from_millis(u64::try_from(at - now + 1).unwrap_or(0)).min(EVERY));
                    tokio::select! {
                        () = call.cancel.cancelled() => break,
                        _ = mine.changed() => {}
                        _ = wakes.changed() => {}
                        changed = sessions.changed() => timer_fires::sleep(Timer::BotsFollow, if changed { SETTLES } else { EVERY }).await,
                        changed = peers.changed() => if changed.is_err() { timer_fires::sleep(Timer::BotsFollow, EVERY).await },
                        () = timer_fires::sleep(Timer::BotsFollow, turning) => {}
                    }
                }
            }))
        }
    });
}
