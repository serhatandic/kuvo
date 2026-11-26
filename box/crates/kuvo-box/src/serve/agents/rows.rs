//! Settings › Accounts' rows, the agents' program updates and update notices, and the agents'
//! events (`kuvo/agents/api/event`).

use kuvo_os::timer_fires::{self, Timer};
use std::time::Duration;

use bytes::Bytes;
use kuvo_agents::{
    account_rows::{AccountRows, Refused},
    feed::{Feed, FeedEvent},
    session_runs::SessionRuns,
    session_store::SessionStore,
};
use kuvo_protocol::routes::{
    delete_kuvo_agents_api_kuvo_updates_notices_id as seen_notice,
    delete_kuvo_agents_api_kuvo_updates_provider as forget_update,
    get_kuvo_agents_api_event as events, get_kuvo_agents_api_integration as list,
    get_kuvo_agents_api_integration_provider as one,
    get_kuvo_agents_api_kuvo_updates_notices as notices,
    post_kuvo_agents_api_integration_provider_install as install,
    post_kuvo_agents_api_kuvo_updates as update_all,
    post_kuvo_agents_api_kuvo_updates_provider as update_one,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::{located, provider};

fn answer<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value).map_err(|error| super::refuse(error.into()))
}

fn refused(refused: Refused) -> Refusal {
    let status =
        http::StatusCode::from_u16(refused.status).unwrap_or(http::StatusCode::SERVICE_UNAVAILABLE);
    Refusal::new(status, refused.message)
}

/// A query parameter's value.
fn query<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    path.split_once('?')?.1.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then_some(value)
    })
}

/// The conversation's folder, as its events say where they're from.
async fn directory(store: &SessionStore, session: &str) -> Option<String> {
    let header = store.read(session).await.ok()??;
    header.directory().map(str::to_owned)
}

/// What each conversation's agent keeps going changed: `session.background.updated`.
fn follow_background(feed: Feed, store: SessionStore) {
    tokio::spawn(async move {
        let background = store.background().clone();
        let mut changes = background.changes();
        let mut before = background.work(true);
        while changes.changed().await.is_ok() {
            let now = background.work(true);
            let changed: Vec<String> = before
                .keys()
                .chain(now.keys())
                .filter(|session| before.get(*session) != now.get(*session))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            for session in changed {
                let directory = directory(&store, &session).await;
                feed.emit_from(
                    "session.background.updated",
                    json!({ "sessionID": session }),
                    directory,
                );
            }
            before = now;
        }
    });
}

pub(crate) fn register(
    rows: AccountRows,
    runs: Option<SessionRuns>,
    store: SessionStore,
    routes: &mut Registry,
) {
    follow_background(rows.feed().clone(), store.clone());
    let listing = rows.clone();
    routes.register::<list::Spec, _, _>(Access::Any, move |call| {
        let rows = listing.clone();
        async move {
            // `fresh`: asked anew (you refreshed, or came back to the app).
            let fresh = query(&call.request.path, "fresh") == Some("1");
            let data = rows.list(fresh).await;
            Ok(RouteReply::json::<200>(answer(located(json!(data)))?))
        }
    });

    let reading = rows.clone();
    routes.register::<one::Spec, _, _>(Access::Any, move |call| {
        let rows = reading.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let Some(row) = rows.one(&provider).await else {
                return Err(Refusal::new(
                    http::StatusCode::NOT_FOUND,
                    format!("No agent {provider}."),
                ));
            };
            Ok(RouteReply::json::<200>(answer(located(row))?))
        }
    });

    let installing = rows.clone();
    routes.register::<install::Spec, _, _>(Access::Any, move |call| {
        let rows = installing.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let result = rows.install(&provider).await.map_err(refused)?;
            Ok(RouteReply::json::<200>(answer(json!({ "data": result }))?))
        }
    });

    let all = rows.clone();
    routes.register::<update_all::Spec, _, _>(Access::Any, move |_call| {
        let rows = all.clone();
        async move {
            let updating = rows.update_all().await;
            Ok(RouteReply::json::<200>(answer(
                json!({ "data": { "updating": updating } }),
            )?))
        }
    });

    let updating = rows.clone();
    routes.register::<update_one::Spec, _, _>(Access::Any, move |call| {
        let rows = updating.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let updating = rows.update(&provider).map_err(refused)?;
            Ok(RouteReply::json::<200>(answer(
                json!({ "data": { "updating": updating } }),
            )?))
        }
    });

    let forgetting = rows.clone();
    routes.register::<forget_update::Spec, _, _>(Access::Any, move |call| {
        let rows = forgetting.clone();
        async move {
            let provider = provider(call.param("provider"))?;
            let at = query(&call.request.path, "at").and_then(|at| at.parse().ok());
            rows.forget(&provider, at).map_err(refused)?;
            Ok(RouteReply::json::<200>(answer(json!({}))?))
        }
    });

    let served = rows.clone();
    routes.register::<notices::Spec, _, _>(Access::Any, move |_call| {
        let rows = served.clone();
        async move {
            let notices = rows.notices();
            Ok(RouteReply::json::<200>(answer(
                json!({ "data": { "notices": notices } }),
            )?))
        }
    });

    let seeing = rows.clone();
    routes.register::<seen_notice::Spec, _, _>(Access::Any, move |call| {
        let rows = seeing.clone();
        async move {
            let given = call.param("id").unwrap_or_default();
            let id = percent_encoding::percent_decode_str(given).decode_utf8_lossy();
            rows.seen(&id);
            Ok(RouteReply::json::<200>(answer(json!({}))?))
        }
    });

    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let mut feed = rows.feed().subscribe();
        let mut turns = runs.as_ref().map(SessionRuns::events);
        let store = store.clone();
        async move {
            Ok(RouteReply::stream::<200>(
                "text/event-stream",
                async_stream::stream! {
                    yield Ok(said("server.connected", json!({}), None));
                    let mut beat = timer_fires::interval_at(Timer::WireSse,
                        tokio::time::Instant::now() + HEARTBEAT,
                        HEARTBEAT,
                    );
                    loop {
                        let next = tokio::select! {
                            () = call.cancel.cancelled() => break,
                            _ = beat.tick() => Some(said("server.heartbeat", json!({}), None)),
                            event = feed.recv() => match event {
                                Ok(FeedEvent { kind, data, directory }) => Some(said(&kind, data, directory)),
                                Err(broadcast::error::RecvError::Lagged(_)) => None,
                                Err(broadcast::error::RecvError::Closed) => break,
                            },
                            event = async { turns.as_mut()?.recv().await.ok() }, if turns.is_some() => {
                                match event {
                                    Some(event) => {
                                        let (session, kind, data) = event.engine_words();
                                        Some(said(kind, data, directory(&store, session).await))
                                    }
                                    None => None,
                                }
                            }
                        };
                        if let Some(next) = next {
                            yield Ok(next);
                        }
                    }
                },
            ))
        }
    });
}

/// How often a feed with nothing to say says it's there.
const HEARTBEAT: Duration = Duration::from_secs(15);

/// An event as the feed says it: `data: {id, type, created, data, location?}`.
fn said(kind: &str, data: Value, directory: Option<String>) -> Bytes {
    let mut event = json!({
        "id": format!("evt_{}", uuid_like()),
        "type": kind,
        "created": jiff::Timestamp::now().as_millisecond(),
        "data": data,
    });
    if let Some(directory) = directory {
        event["location"] = json!({ "directory": directory });
    }
    Bytes::from(format!("data: {event}\n\n"))
}

/// A unique enough id for one event.
fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = jiff::Timestamp::now().as_nanosecond();
    format!("{now:x}{:06x}", NEXT.fetch_add(1, Ordering::Relaxed))
}
