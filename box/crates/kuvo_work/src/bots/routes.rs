//! `kuvo/bots` and its events: this box's bots, each with what it's doing now.

use std::time::Duration;

use http::StatusCode;
use kuvo_protocol::routes::{get_kuvo_bots as list, get_kuvo_bots_events as events};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::Value;

use super::Bots;

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

pub(super) fn register(bots: Bots, routes: &mut Registry) {
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
                        changed = sessions.changed() => tokio::time::sleep(if changed { SETTLES } else { EVERY }).await,
                        changed = peers.changed() => if changed.is_err() { tokio::time::sleep(EVERY).await },
                        () = tokio::time::sleep(turning) => {}
                    }
                }
            }))
        }
    });
}
