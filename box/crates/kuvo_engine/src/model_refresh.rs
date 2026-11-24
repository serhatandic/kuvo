//! Reopen the engine's account-backed catalogs without interrupting its work.

use std::{collections::HashSet, time::Duration};

use http::{Method, StatusCode};
use kuvo_protocol::routes::{
    get_kuvo_accounts_refreshable as refreshable, post_kuvo_accounts_provider_refresh as refresh,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::{
    Engine, api, disk,
    lifetime::{Demand, Lease},
    sessions,
};

const PROVIDERS: &[&str] = &[
    "digitalocean",
    "github-copilot",
    "gitlab",
    "modal",
    "opencode",
];
const DUE: &str = "run/models-due.json";

#[derive(Deserialize, Serialize)]
struct Due {
    engine: String,
    folders: Vec<String>,
}

fn identity(lease: &Lease) -> String {
    format!("{}:{}", lease.record.pid, lease.record.port)
}

fn unavailable() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The engine isn't answering. Try again in a moment.",
    )
}

fn restart_required() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This engine can't ask for its models again without restarting. Restart it in Settings › Engine.",
    )
}

fn shared_refresh(provider: &str) -> Option<&'static str> {
    match provider {
        "opencode" => Some("two minutes"),
        "digitalocean" | "github-copilot" | "modal" => Some("twenty minutes"),
        _ => None,
    }
}

async fn open(engine: &Engine, lease: &Lease) -> Result<Vec<String>, Refusal> {
    let locations = engine
        .call_raw(lease, Method::GET, "api/debug/location", None, api::WAIT)
        .await
        .map_err(|_| unavailable())?;
    let mut seen = HashSet::new();
    Ok(array(&locations)
        .into_iter()
        .flatten()
        .filter_map(|location| location["directory"].as_str())
        .filter(|folder| !folder.is_empty() && seen.insert(*folder))
        .map(str::to_owned)
        .collect())
}

fn array(value: &Value) -> Option<&Vec<Value>> {
    value.as_array().or_else(|| value["data"].as_array())
}

async fn session_locations(engine: &Engine, lease: &Lease) -> Option<HashSet<String>> {
    let active = engine
        .call_raw(lease, Method::GET, "api/session/active", None, api::WAIT)
        .await
        .ok()?;
    let sessions = api::active_ids(active.get("data")?)?;
    let mut locations = HashSet::new();
    for id in sessions {
        let session = engine
            .call::<Value>(lease, Method::GET, &sessions::path(id), None)
            .await
            .ok()?;
        locations.insert(session.pointer("/location/directory")?.as_str()?.to_owned());
    }
    Some(locations)
}

async fn close_quiet(
    engine: &Engine,
    lease: &Lease,
    folders: Vec<String>,
) -> Result<Vec<String>, Refusal> {
    let mut waiting = Vec::new();
    for folder in folders {
        if session_locations(engine, lease)
            .await
            .is_none_or(|active| active.contains(&folder))
            || crate::proxy::folder_busy(&engine.upstream, &lease.record, &folder).await
        {
            waiting.push(folder);
            continue;
        }
        let query = url_for_close(&folder);
        let closed = engine
            .upstream
            .request(&lease.record, Method::DELETE, &query)
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        match closed.map(|reply| reply.status()) {
            Ok(status) if status.is_success() => {}
            Ok(StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED) => {
                return Err(restart_required());
            }
            _ => waiting.push(folder),
        }
    }
    Ok(waiting)
}

fn url_for_close(folder: &str) -> String {
    format!(
        "api/debug/location?location%5Bdirectory%5D={}",
        api::segment(folder)
    )
}

async fn reopen_all(
    engine: &Engine,
    lease: &Lease,
    mut folders: Vec<String>,
    within: &str,
) -> Result<(), Refusal> {
    let end = Instant::now() + Duration::from_secs(10);
    let later = || {
        Refusal::new(
            StatusCode::CONFLICT,
            format!(
                "Something's going on in a project on this computer (a run, a terminal, a command or a question waiting on you), and its engine asks for these models again only once every project is closed: they show by themselves within {within}."
            ),
        )
    };
    while !folders.is_empty() {
        let active = session_locations(engine, lease).await.ok_or_else(later)?;
        for folder in &folders {
            if active.contains(folder)
                || crate::proxy::folder_busy(&engine.upstream, &lease.record, folder).await
            {
                return Err(later());
            }
        }
        if !close_quiet(engine, lease, folders).await?.is_empty() {
            return Err(later());
        }
        folders = open(engine, lease).await?;
        if !folders.is_empty() {
            if Instant::now() >= end {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    format!(
                        "A project on this computer kept opening again as its engine closed them, so it couldn't ask for these models again now: try again in a moment, or they show by themselves within {within}."
                    ),
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    // Reopen the default location after the engine has let the shared catalog go.
    let _ = engine
        .call_raw(lease, Method::GET, "api/model", None, api::WAIT)
        .await;
    Ok(())
}

async fn defer(engine: &Engine, lease: &Lease, waiting: &[String]) -> Result<(), Refusal> {
    let path = engine.home.join(DUE);
    let identity = identity(lease);
    let mut due = disk::read::<Due>(&path)
        .await
        .ok()
        .flatten()
        .filter(|due| due.engine == identity)
        .unwrap_or(Due {
            engine: identity,
            folders: Vec::new(),
        });
    for folder in waiting {
        if !due.folders.contains(folder) {
            due.folders.push(folder.clone());
        }
    }
    if !due.folders.is_empty() {
        disk::durable(path, due).await.map_err(|_| unavailable())?;
    }
    Ok(())
}

pub(crate) fn work_ended(kind: &str) -> bool {
    matches!(
        kind,
        "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted"
            | "pty.exited"
            | "pty.deleted"
            | "shell.exited"
            | "shell.deleted"
            | "permission.replied"
            | "permission.rejected"
            | "form.replied"
            | "form.cancelled"
    )
}

pub(crate) fn watch(
    engine: Engine,
) -> (tokio::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (nudge, mut pending) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move {
        let path = engine.home.join(DUE);
        while pending.recv().await.is_some() {
            if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                continue;
            }
            let _turn = engine.turn("model-refresh").await;
            if let Some(lease) = engine.acquire(Demand::Existing).await {
                settle_in_turn(&engine, &lease).await;
            }
        }
    });
    (nudge, task)
}

async fn settle_in_turn(engine: &Engine, lease: &Lease) {
    let path = engine.home.join(DUE);
    let Ok(Some(mut due)) = disk::read::<Due>(&path).await else {
        return;
    };
    if due.engine != identity(lease) {
        let _ = tokio::fs::remove_file(path).await;
        return;
    }
    let Ok(open) = open(engine, lease).await else {
        return;
    };
    due.folders.retain(|folder| open.contains(folder));
    let read = due.folders.len();
    if let Ok(waiting) = close_quiet(engine, lease, due.folders).await {
        if waiting.len() < read {
            let _ = engine
                .call_raw(lease, Method::GET, "api/model", None, api::WAIT)
                .await;
        }
        if !waiting.is_empty() {
            due.folders = waiting;
            let _ = disk::durable(path, due).await;
            return;
        }
    }
    let _ = tokio::fs::remove_file(path).await;
}

pub(crate) async fn revalidate_picker(engine: &Engine, lease: &Lease, directory: &str) {
    let _turn = engine.turn("model-refresh").await;
    let Ok(providers) = engine
        .upstream
        .get_data::<Vec<Value>>(
            &lease.record,
            "api/provider",
            &api::folder_header(directory),
            api::WAIT,
        )
        .await
    else {
        return;
    };
    let listed = providers
        .iter()
        .filter_map(|value| value["id"].as_str())
        .filter(|id| PROVIDERS.contains(id))
        .collect::<Vec<_>>();
    if listed.is_empty() {
        return;
    }
    let Ok(folders) = open(engine, lease).await else {
        return;
    };
    if let Some(within) = listed.iter().find_map(|provider| shared_refresh(provider))
        && reopen_all(engine, lease, folders.clone(), within)
            .await
            .is_ok()
    {
        return;
    }
    if listed.contains(&"gitlab") {
        let folders = folders
            .into_iter()
            .filter(|folder| {
                directory.is_empty()
                    || std::path::Path::new(directory) == std::path::Path::new(folder)
            })
            .collect();
        if let Ok(waiting) = close_quiet(engine, lease, folders).await
            && !waiting.is_empty()
            && defer(engine, lease, &waiting).await.is_ok()
        {
            settle_in_turn(engine, lease).await;
        }
    }
}

async fn refresh(engine: &Engine, provider: &str) -> Result<Vec<String>, Refusal> {
    let lease = engine.acquire(Demand::Existing).await.ok_or_else(|| {
        Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.")
    })?;
    if !PROVIDERS.contains(&provider) {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "Its models come from a catalog the engine reads again by itself: they show within a few minutes.",
        ));
    }
    let _turn = engine.turn("model-refresh").await;
    engine
        .call_raw(&lease, Method::GET, "api/provider", None, api::WAIT)
        .await
        .map_err(|_| unavailable())?;
    let route = format!("api/integration/{}", api::segment(provider));
    engine
        .call_raw(&lease, Method::GET, &route, None, api::WAIT)
        .await
        .map_err(|error| {
            if error.missing() {
                Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That provider isn't on this computer's engine any more.",
                )
            } else {
                unavailable()
            }
        })?;
    let folders = open(engine, &lease).await?;
    if let Some(within) = shared_refresh(provider) {
        reopen_all(engine, &lease, folders, within).await?;
        return Ok(Vec::new());
    }
    let waiting = close_quiet(engine, &lease, folders).await?;
    defer(engine, &lease, &waiting).await?;
    let _ = engine
        .call_raw(&lease, Method::GET, "api/model", None, api::WAIT)
        .await;
    settle_in_turn(engine, &lease).await;
    Ok(waiting)
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    routes.register::<refreshable::Spec, _, _>(Access::Any, |_| async {
        Ok(RouteReply::json::<200>(sessions::typed(
            json!({ "providers": PROVIDERS }),
        )?))
    });
    let here = engine.clone();
    routes.register::<refresh::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        async move {
            let provider =
                percent_encoding::percent_decode_str(call.param("provider").unwrap_or_default())
                    .decode_utf8()
                    .ok()
                    .filter(|id| !id.is_empty() && !id.contains('/'))
                    .ok_or_else(|| Refusal::json(StatusCode::NOT_FOUND, json!({})))?;
            let waiting = refresh(&here, &provider).await?;
            Ok(RouteReply::json::<200>(sessions::typed(
                json!({ "waiting": waiting }),
            )?))
        }
    });
}
