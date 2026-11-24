//! HTTP and line routes for artifacts (`kuvo/artifacts…`) and merged setup outputs
//! (`kuvo/setup/outputs…`), matching `box/src/link/artifacts/routes.ts` and `areas/setup-outputs.ts`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::stream;
use http::{
    HeaderValue, StatusCode,
    header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
};
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    routes::{
        get_kuvo_artifacts, get_kuvo_artifacts_id, get_kuvo_artifacts_id_versions_n,
        get_kuvo_artifacts_id_versions_n_changes, get_kuvo_artifacts_storage,
        get_kuvo_artifacts_timeline, get_kuvo_setup_outputs, get_kuvo_setup_outputs_events,
        post_kuvo_artifacts_id_pin, post_kuvo_artifacts_id_restore, post_kuvo_artifacts_storage,
        post_kuvo_artifacts_storage_clear,
    },
    shape::{Primitive, Shape},
    version::{FormatRefusalCode, UnsupportedFormatVersion},
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use super::{
    changes::changes,
    kinds::{Kind, is_prose, is_text},
    policy::NoCopy,
    store::{
        Artifact, ArtifactHeader, ArtifactStore, PinBy, Scope, Summary, Version, VersionStatus,
        parse_seconds,
    },
};

pub const FEATURES: &[&str] = &["artifacts", "line.merged-outputs"];

static ANY_SHAPE: Shape = Shape::Primitive(Primitive::Any);

struct GetArtifactsEvents;
impl RouteSpec for GetArtifactsEvents {
    type Body = ();
    const ROUTE: &'static Route = &Route {
        method: "GET",
        path: "kuvo/artifacts/events",
        feature: "artifacts",
        answers: &[(200, &ANY_SHAPE)],
        body: None,
    };
}
impl JsonStatus<200> for GetArtifactsEvents {
    type Body = Value;
}

struct GetArtifactsTimelineEvents;
impl RouteSpec for GetArtifactsTimelineEvents {
    type Body = ();
    const ROUTE: &'static Route = &Route {
        method: "GET",
        path: "kuvo/artifacts/timeline/events",
        feature: "artifacts",
        answers: &[(200, &ANY_SHAPE)],
        body: None,
    };
}
impl JsonStatus<200> for GetArtifactsTimelineEvents {
    type Body = Value;
}

/// What `kuvo/setup/outputs` needs from the peer layer and session list.
#[derive(Clone, Debug)]
pub struct OutputPeerCopy {
    pub computer_id: String,
    pub computer_name: String,
    pub away: bool,
    pub last_seen: Option<i64>,
    pub artifacts: Vec<PeerOutputRow>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct PeerOutputRow {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: Kind,
    #[serde(rename = "type")]
    pub mime: String,
    pub gone: Option<f64>,
    pub updated: Option<f64>,
    pub pinned: bool,
    pub latest: PeerOutputLatest,
    pub sessions: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct PeerOutputLatest {
    pub n: u64,
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PendingComputer {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct OutputsSnapshot {
    pub local_id: String,
    pub local_name: String,
    /// Map from parent session ID to child session IDs across all known computers.
    pub children: HashMap<String, Vec<String>>,
    pub peers: Vec<OutputPeerCopy>,
    pub pending: Vec<PendingComputer>,
}

pub type OutputsFuture<'a> = Pin<Box<dyn Future<Output = Option<OutputsSnapshot>> + Send + 'a>>;

pub trait OutputsPeers: Send + Sync {
    fn snapshot(&self) -> OutputsFuture<'_>;
    fn changes(&self) -> watch::Receiver<u64>;
}

/// Runtime handle to either a healthy `ArtifactStore` or a startup format refusal.
#[derive(Clone)]
pub enum ArtifactHandle {
    Ready(ArtifactStore),
    UnsupportedFormat(String),
    Unavailable,
}

impl ArtifactHandle {
    fn store(&self) -> Result<&ArtifactStore, Refusal> {
        match self {
            Self::Ready(store) => Ok(store),
            Self::UnsupportedFormat(msg) => Err(Refusal::unsupported_record(
                UnsupportedFormatVersion {
                    code: FormatRefusalCode::UnsupportedFormatVersion,
                    message: msg.clone(),
                },
            )),
            Self::Unavailable => Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer's record of what its agents made isn't running. Try again in a moment.",
            )),
        }
    }
}

pub fn register(
    registry: &mut Registry,
    handle: ArtifactHandle,
    peers: Option<Arc<dyn OutputsPeers>>,
) {
    // GET kuvo/artifacts
    let h = handle.clone();
    registry.register::<get_kuvo_artifacts::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let query = parse_query(&call.request.path);
        async move {
            let store = h.store()?;
            let scope = scope_of(&query)?;
            let limit = parse_limit(&query);
            let before = query.get("before").filter(|s| !s.is_empty()).map(String::as_str);
            let body = list_json(store, &scope, limit, before);
            Ok(RouteReply::json_as_kept::<200>(body))
        }
    });

    // GET kuvo/artifacts/events
    let h = handle.clone();
    registry.register::<GetArtifactsEvents, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let query = parse_query(&call.request.path);
        let cancel = call.cancel.clone();
        async move {
            let store = h.store()?.clone();
            let scope = scope_of(&query)?;
            Ok(followed_artifacts(
                store.clone(),
                move || list_json(&store, &scope, 200, None),
                cancel,
            ))
        }
    });

    // GET kuvo/artifacts/timeline
    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_timeline::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let query = parse_query(&call.request.path);
        async move {
            let store = h.store()?;
            store.settled().await;
            let scope = scope_of(&query)?;
            let limit = parse_limit(&query);
            let before = query.get("before").filter(|s| !s.is_empty()).map(String::as_str);
            let body = timeline_json(store, &scope, limit, before);
            Ok(RouteReply::json_as_kept::<200>(body))
        }
    });

    // GET kuvo/artifacts/timeline/events
    let h = handle.clone();
    registry.register::<GetArtifactsTimelineEvents, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let query = parse_query(&call.request.path);
        let cancel = call.cancel.clone();
        async move {
            let store = h.store()?.clone();
            store.settled().await;
            let scope = scope_of(&query)?;
            Ok(followed_artifacts(
                store.clone(),
                move || timeline_json(&store, &scope, 200, None),
                cancel,
            ))
        }
    });

    // GET kuvo/artifacts/storage
    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_storage::Spec, _, _>(Access::Any, move |_| {
        let h = h.clone();
        async move {
            let store = h.store()?;
            store.settled().await;
            Ok(RouteReply::json_as_kept::<200>(json!(store.storage())))
        }
    });

    // POST kuvo/artifacts/storage
    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_storage::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let raw = call.request.body.clone();
        async move {
            let store = h.store()?;
            store.settled().await;
            let given = parse_json_body(&raw);
            let budget = given
                .get("budget")
                .and_then(Value::as_f64)
                .filter(|b| b.is_finite() && *b >= 0.0);
            let Some(budget_f) = budget else {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Say the budget for older versions, in bytes (budget).",
                ));
            };
            let current = store.storage();
            if let Some(was) = given.get("was")
                && was.as_f64() != Some(current.budget as f64)
            {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "It changed since you read it.",
                ));
            }
            if let Some(was_older) = given.get("wasOlder")
                && was_older.as_f64() != Some(current.older as f64)
            {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "It changed since you read it.",
                ));
            }
            let confirmed = given.get("confirmed").and_then(Value::as_bool) == Some(true);
            match store.set_budget(budget_f.floor() as u64, confirmed) {
                Ok(Some(msg)) => Err(Refusal::new(StatusCode::BAD_REQUEST, msg)),
                Ok(None) => Ok(RouteReply::json_as_kept::<200>(json!(store.storage()))),
                Err(super::store::StoreError::UnsupportedFormat(msg)) => {
                    Err(Refusal::unsupported_record(UnsupportedFormatVersion {
                        code: FormatRefusalCode::UnsupportedFormatVersion,
                        message: msg,
                    }))
                }
                Err(err) => Err(Refusal::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    err.to_string(),
                )),
            }
        }
    });

    // POST kuvo/artifacts/storage/clear
    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_storage_clear::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let raw = call.request.body.clone();
        async move {
            let store = h.store()?;
            store.settled().await;
            let given = parse_json_body(&raw);
            let current = store.storage();
            if let Some(was) = given.get("was")
                && was.as_f64() != Some(current.older as f64)
            {
                return Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "It changed since you read it.",
                ));
            }
            let before_iso = given
                .get("before")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .and_then(|secs| jiff::Timestamp::from_millisecond((secs * 1000.0) as i64).ok())
                .map(super::store::stamp);
            let cleared = store.clear_older(before_iso.as_deref());
            let storage = store.storage();
            Ok(RouteReply::json_as_kept::<200>(json!({
                "cleared": cleared,
                "copies": storage.copies,
                "older": storage.older,
                "budget": storage.budget,
                "free": storage.free,
                "floor": storage.floor,
                "low": storage.low,
            })))
        }
    });

    // GET kuvo/artifacts/:id
    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_id::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        async move {
            let store = h.store()?;
            store.settled().await;
            let Some(artifact) = store.get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's record.",
                ));
            };
            let checked = store.check_gone(&artifact.id).await.unwrap_or(artifact);
            Ok(RouteReply::json_as_kept::<200>(client_artifact(&checked, store)))
        }
    });

    // POST kuvo/artifacts/:id/pin
    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_id_pin::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        let raw = call.request.body.clone();
        async move {
            let store = h.store()?;
            store.settled().await;
            let Some(artifact) = store.get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's record.",
                ));
            };
            let given = parse_json_body(&raw);
            let Some(pinned) = given.get("pinned").and_then(Value::as_bool) else {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Say whether it's pinned (pinned).",
                ));
            };
            let note = given
                .get("note")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let next = if pinned { Some((PinBy::You, note)) } else { None };
            match store.pin(&artifact.id, next).await {
                Some(changed) => Ok(RouteReply::json_as_kept::<200>(client_artifact(
                    &changed, store,
                ))),
                None => Err(Refusal::new(
                    StatusCode::CONFLICT,
                    "It couldn't be kept now. Try again in a moment.",
                )),
            }
        }
    });

    // POST kuvo/artifacts/:id/restore
    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_id_restore::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        let raw = call.request.body.clone();
        async move {
            let store = h.store()?;
            store.settled().await;
            let Some(artifact) = store.get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's record.",
                ));
            };
            let given = parse_json_body(&raw);
            let version = given
                .get("version")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && v.fract() == 0.0 && *v >= 0.0)
                .map(|v| v as u64);
            let Some(version) = version else {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Say which version to put back (version).",
                ));
            };
            match store.restore(&artifact.id, version, None).await {
                Ok(restored) => Ok(RouteReply::json_as_kept::<200>(client_artifact(
                    &restored, store,
                ))),
                Err(msg) => Err(Refusal::new(StatusCode::CONFLICT, msg)),
            }
        }
    });

    // GET kuvo/artifacts/:id/versions/:n
    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_id_versions_n::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        let n_raw = decode_param(call.param("n").unwrap_or(""));
        let query = parse_query(&call.request.path);
        async move {
            let store = h.store()?;
            let Some(artifact) = store.get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's record.",
                ));
            };
            let Ok(n) = n_raw.parse::<u64>() else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There's no such version.",
                ));
            };
            let Some(version) = artifact.versions.iter().find(|v| v.n == n) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There's no such version.",
                ));
            };
            let bytes = if query.get("preview").map(String::as_str) == Some("1") {
                store.image_preview(&artifact.id, n).await.flatten()
            } else {
                store.bytes(&artifact.id, n).flatten()
            };
            let Some(bytes) = bytes else {
                return Err(Refusal::new(StatusCode::GONE, not_kept(version)));
            };
            let len = bytes.len();
            let mut reply = RouteReply::bytes::<200>("application/octet-stream", bytes);
            if let Ok(ct) = HeaderValue::from_str(&artifact.mime) {
                reply.headers_mut().insert(CONTENT_TYPE, ct);
            }
            reply
                .headers_mut()
                .insert(CONTENT_LENGTH, HeaderValue::from(len));
            Ok(reply)
        }
    });

    // GET kuvo/artifacts/:id/versions/:n/changes
    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_id_versions_n_changes::Spec, _, _>(
        Access::Any,
        move |call| {
            let h = h.clone();
            let id = decode_param(call.param("id").unwrap_or(""));
            let n_raw = decode_param(call.param("n").unwrap_or(""));
            let query = parse_query(&call.request.path);
            async move {
                let store = h.store()?;
                store.settled().await;
                let Some(artifact) = store.get(&id) else {
                    return Err(Refusal::new(
                        StatusCode::NOT_FOUND,
                        "That isn't in this computer's record.",
                    ));
                };
                let Ok(n) = n_raw.parse::<u64>() else {
                    return Err(Refusal::new(
                        StatusCode::NOT_FOUND,
                        "There's no such version.",
                    ));
                };
                let Some(version) = artifact.versions.iter().find(|v| v.n == n) else {
                    return Err(Refusal::new(
                        StatusCode::NOT_FOUND,
                        "There's no such version.",
                    ));
                };
                let from = if let Some(raw_from) = query.get("from") {
                    raw_from
                        .parse::<u64>()
                        .ok()
                        .and_then(|m| artifact.versions.iter().find(|v| v.n == m))
                } else {
                    artifact.versions.iter().rev().find(|v| v.n < n)
                };
                if version.reason.is_some_and(NoCopy::is_private) {
                    return Err(Refusal::new(StatusCode::GONE, not_kept(version)));
                }
                if let Some(patch) = &version.patch {
                    return Ok(RouteReply::json_as_kept::<200>(json!({
                        "style": "lines",
                        "patch": patch,
                        "added": version.lines.as_ref().map_or(0, |l| l.added),
                        "removed": version.lines.as_ref().map_or(0, |l| l.removed),
                        "from": from.map(|v| v.n),
                        "to": n,
                    })));
                }
                if !is_text(&artifact.mime) {
                    let style = if artifact.kind == Kind::Image || artifact.mime == "application/pdf"
                    {
                        "pair"
                    } else {
                        "none"
                    };
                    return Ok(RouteReply::json_as_kept::<200>(json!({
                        "style": style,
                        "from": from.map(|v| v.n),
                        "to": n,
                    })));
                }
                let after = if version.status == VersionStatus::Deleted {
                    Some(Vec::new())
                } else {
                    store.bytes(&artifact.id, n).flatten()
                };
                let before = match from {
                    None => Some(Vec::new()),
                    Some(f) if f.status == VersionStatus::Deleted => Some(Vec::new()),
                    Some(f) => store.bytes(&artifact.id, f.n).flatten(),
                };
                let (Some(before_bytes), Some(after_bytes)) = (before, after) else {
                    let msg = if version.kept {
                        "The version before isn't kept, so what changed can't be shown."
                    } else {
                        not_kept(version)
                    };
                    return Err(Refusal::new(StatusCode::GONE, msg));
                };
                let before_text = String::from_utf8_lossy(&before_bytes);
                let after_text = String::from_utf8_lossy(&after_bytes);
                let mut diff = serde_json::to_value(changes(
                    &before_text,
                    &after_text,
                    is_prose(&artifact.path),
                ))
                .unwrap();
                if let Some(obj) = diff.as_object_mut() {
                    obj.insert("from".into(), from.map_or(Value::Null, |v| Value::from(v.n)));
                    obj.insert("to".into(), Value::from(n));
                }
                Ok(RouteReply::json_as_kept::<200>(diff))
            }
        },
    );

    // GET kuvo/setup/outputs
    let h = handle.clone();
    let p = peers.clone();
    registry.register::<get_kuvo_setup_outputs::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let p = p.clone();
        let query = parse_query(&call.request.path);
        async move {
            let session = validate_session_query(&query)?;
            let before = parse_outputs_before(query.get("before").map(String::as_str));
            let Some(state) = build_setup_outputs(&h, p.as_deref(), &session, before.as_ref()).await
            else {
                return Err(Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The outputs aren't ready yet.",
                ));
            };
            Ok(RouteReply::json_as_kept::<200>(state))
        }
    });

    // GET kuvo/setup/outputs/events
    registry.register::<get_kuvo_setup_outputs_events::Spec, _, _>(Access::Any, move |call| {
        let h = handle.clone();
        let p = peers.clone();
        let query = parse_query(&call.request.path);
        let cancel = call.cancel.clone();
        async move {
            let session = validate_session_query(&query)?;
            let before = parse_outputs_before(query.get("before").map(String::as_str));
            let local_rx = match &h {
                ArtifactHandle::Ready(store) => Some(store.subscribe()),
                _ => None,
            };
            let peer_rx = p.as_ref().map(|peers| peers.changes());
            let stream = stream::unfold(
                (h, p, session, before, local_rx, peer_rx, cancel, None::<String>, true),
                |(h, p, session, before, mut local_rx, mut peer_rx, cancel, mut last, first)| async move {
                    if !first {
                        loop {
                            let sleep = tokio::time::sleep(Duration::from_secs(3));
                            tokio::pin!(sleep);
                            tokio::select! {
                                () = cancel.cancelled() => return None,
                                () = &mut sleep => break,
                                res = recv_opt(&mut local_rx) => {
                                    if res { break; }
                                }
                                res = watch_opt(&mut peer_rx) => {
                                    if res { break; }
                                }
                            }
                        }
                    }
                    loop {
                        if cancel.is_cancelled() {
                            return None;
                        }
                        if let Some(state) =
                            build_setup_outputs(&h, p.as_deref(), &session, before.as_ref()).await
                        {
                            let line = serde_json::to_string(&state).unwrap_or_default();
                            if last.as_deref() != Some(line.as_str()) {
                                last = Some(line);
                                let item = serde_json::from_value(state)
                                    .map_err(std::io::Error::other)
                                    .and_then(JsonLine::<get_kuvo_setup_outputs_events::Spec, 200>::new);
                                return Some((
                                    item,
                                    (h, p, session, before, local_rx, peer_rx, cancel, last, false),
                                ));
                            }
                        }
                        let sleep = tokio::time::sleep(Duration::from_secs(3));
                        tokio::pin!(sleep);
                        tokio::select! {
                            () = cancel.cancelled() => return None,
                            () = &mut sleep => {}
                            res = recv_opt(&mut local_rx) => {
                                let _ = res;
                            }
                            res = watch_opt(&mut peer_rx) => {
                                let _ = res;
                            }
                        }
                    }
                },
            );
            let mut reply = RouteReply::ndjson::<200>(stream);
            reply
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            Ok(reply)
        }
    });
}

async fn recv_opt(rx: &mut Option<tokio::sync::broadcast::Receiver<Artifact>>) -> bool {
    match rx {
        Some(r) => match r.recv().await {
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                *rx = None;
                false
            }
        },
        None => std::future::pending().await,
    }
}

async fn watch_opt(rx: &mut Option<watch::Receiver<u64>>) -> bool {
    match rx {
        Some(r) => match r.changed().await {
            Ok(()) => true,
            Err(_) => {
                *rx = None;
                false
            }
        },
        None => std::future::pending().await,
    }
}

fn followed_artifacts<R>(
    store: ArtifactStore,
    read: impl Fn() -> Value + Send + 'static,
    cancel: tokio_util::sync::CancellationToken,
) -> RouteReply<R>
where
    R: JsonStatus<200>,
    <R as JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    let rx = store.subscribe();
    let stream = stream::unfold(
        (rx, read, cancel, None::<String>, true),
        |(mut rx, read, cancel, mut last, first)| async move {
            loop {
                if cancel.is_cancelled() {
                    return None;
                }
                if !first {
                    tokio::select! {
                        () = cancel.cancelled() => return None,
                        res = rx.recv() => {
                            match res {
                                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                            }
                        }
                    }
                    tokio::select! {
                        () = cancel.cancelled() => return None,
                        () = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                    while rx.try_recv().is_ok() {}
                }
                let value = read();
                let line = serde_json::to_string(&value).unwrap_or_default();
                if last.as_deref() != Some(line.as_str()) {
                    last = Some(line);
                    let item = serde_json::from_value(value)
                        .map_err(std::io::Error::other)
                        .and_then(JsonLine::<R, 200>::new);
                    return Some((item, (rx, read, cancel, last, false)));
                }
                if first {
                    // Continue waiting for next change after empty initial diff (impossible since last is None).
                    return None;
                }
            }
        },
    );
    let mut reply = RouteReply::ndjson::<200>(stream);
    reply
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    reply
}

pub fn local_output_copy(store: &ArtifactStore, sessions: Option<&[String]>) -> Vec<PeerOutputRow> {
    store
        .session_outputs(sessions)
        .into_iter()
        .map(|s| PeerOutputRow {
            id: s.header.id,
            path: s.header.path,
            name: s.header.name,
            kind: s.header.kind,
            mime: s.header.mime,
            gone: s.header.gone.as_deref().and_then(parse_seconds),
            updated: parse_seconds(&s.header.updated),
            pinned: s.header.pinned.is_some(),
            latest: PeerOutputLatest {
                n: s.latest.n,
                hash: s.latest.hash,
                reason: s
                    .latest
                    .reason
                    .map(|r| serde_json::to_value(r).unwrap().as_str().unwrap().to_owned()),
            },
            sessions: s.header.where_.sessions,
        })
        .collect()
}

type OutputOrder = (i64, f64, String, String);

fn parse_outputs_before(raw: Option<&str>) -> Option<OutputOrder> {
    let raw = raw?;
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let val: Value = serde_json::from_slice(&bytes).ok()?;
    let arr = val.as_array()?;
    if arr.len() != 4 {
        return None;
    }
    Some((
        arr[0].as_i64()?,
        arr[1].as_f64()?,
        arr[2].as_str()?.to_owned(),
        arr[3].as_str()?.to_owned(),
    ))
}

fn compare_output_order(a: &OutputOrder, b: &OutputOrder) -> std::cmp::Ordering {
    b.0.cmp(&a.0)
        .then_with(|| b.1.total_cmp(&a.1))
        .then_with(|| b.2.cmp(&a.2))
        .then_with(|| b.3.cmp(&a.3))
}

fn validate_session_query(query: &BTreeMap<String, String>) -> Result<String, Refusal> {
    let session = query.get("session").map(String::as_str).unwrap_or("");
    let valid = session
        .strip_prefix("ses_")
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest.len() <= 95
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        });
    if !valid {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Choose a conversation.",
        ));
    }
    Ok(session.to_owned())
}

async fn build_setup_outputs(
    handle: &ArtifactHandle,
    peers: Option<&dyn OutputsPeers>,
    root_session: &str,
    before: Option<&OutputOrder>,
) -> Option<Value> {
    let snap = match peers {
        Some(p) => p.snapshot().await,
        None => None,
    };
    let mut sessions = HashSet::new();
    sessions.insert(root_session.to_owned());
    if let Some(snap) = &snap {
        let mut queue = vec![root_session.to_owned()];
        while let Some(s) = queue.pop() {
            if let Some(children) = snap.children.get(&s) {
                for child in children {
                    if sessions.insert(child.clone()) {
                        queue.push(child.clone());
                    }
                }
            }
        }
    }
    let session_list: Vec<String> = sessions.iter().cloned().collect();
    let here_rows = match handle {
        ArtifactHandle::Ready(store) => Some(local_output_copy(store, Some(&session_list))),
        _ => None,
    };
    let peer_copies = snap.as_ref().map(|s| s.peers.as_slice()).unwrap_or(&[]);
    let pending = snap.as_ref().map(|s| s.pending.as_slice()).unwrap_or(&[]);
    if here_rows.is_none() && peer_copies.is_empty() && pending.is_empty() {
        return None;
    }

    let local_id = snap
        .as_ref()
        .map(|s| s.local_id.clone())
        .unwrap_or_else(|| "here".to_owned());
    let local_name = snap
        .as_ref()
        .map(|s| s.local_name.clone())
        .unwrap_or_else(|| "this computer".to_owned());

    let mut rows: Vec<(OutputOrder, Value)> = Vec::new();
    let mut push_row = |row: &PeerOutputRow,
                        comp_id: &str,
                        comp_name: &str,
                        here: bool,
                        away: bool,
                        last_seen: Option<i64>| {
        let row_id = format!("{comp_id}:{}", row.id);
        let ord: OutputOrder = (
            i64::from(row.pinned),
            row.updated.unwrap_or(0.0),
            row_id.clone(),
            comp_id.to_owned(),
        );
        let mut computer = Map::new();
        computer.insert("id".into(), Value::String(comp_id.to_owned()));
        computer.insert("name".into(), Value::String(comp_name.to_owned()));
        if here {
            computer.insert("here".into(), Value::Bool(true));
        }
        let mut obj = Map::new();
        obj.insert("id".into(), Value::String(row_id));
        obj.insert("artifactId".into(), Value::String(row.id.clone()));
        obj.insert("path".into(), Value::String(row.path.clone()));
        obj.insert("name".into(), Value::String(row.name.clone()));
        obj.insert("kind".into(), serde_json::to_value(row.kind).unwrap());
        obj.insert("type".into(), Value::String(row.mime.clone()));
        obj.insert("gone".into(), row.gone.map_or(Value::Null, Value::from));
        obj.insert("updated".into(), row.updated.map_or(Value::Null, Value::from));
        obj.insert("pinned".into(), Value::Bool(row.pinned));
        obj.insert("latest".into(), serde_json::to_value(&row.latest).unwrap());
        obj.insert("computer".into(), Value::Object(computer));
        obj.insert("away".into(), Value::Bool(away));
        if away && let Some(seen) = last_seen {
            obj.insert("lastSeen".into(), Value::from(seen));
        }
        rows.push((ord, Value::Object(obj)));
    };

    if let Some(here) = &here_rows {
        for row in here {
            push_row(row, &local_id, &local_name, true, false, None);
        }
    }
    for peer in peer_copies {
        for row in &peer.artifacts {
            if row.sessions.iter().any(|s| sessions.contains(s)) {
                push_row(
                    row,
                    &peer.computer_id,
                    &peer.computer_name,
                    false,
                    peer.away,
                    peer.last_seen,
                );
            }
        }
    }

    if let Some(before) = before {
        rows.retain(|(ord, _)| compare_output_order(ord, before) == std::cmp::Ordering::Greater);
    }
    rows.sort_by(|(a, _), (b, _)| compare_output_order(a, b));
    let has_more = rows.len() > 200;
    rows.truncate(200);
    let next = if has_more {
        rows.last().map(|(ord, _)| {
            let encoded = serde_json::to_vec(&json!([ord.0, ord.1, ord.2, ord.3])).unwrap();
            URL_SAFE_NO_PAD.encode(encoded)
        })
    } else {
        None
    };
    let artifacts: Vec<Value> = rows.into_iter().map(|(_, v)| v).collect();
    let pending_json: Vec<Value> = pending
        .iter()
        .map(|c| json!({ "id": c.id, "name": c.name }))
        .collect();
    Some(json!({
        "artifacts": artifacts,
        "next": next,
        "pendingComputers": pending_json,
    }))
}

fn parse_query(path: &str) -> BTreeMap<String, String> {
    let Some((_, qs)) = path.split_once('?') else {
        return BTreeMap::new();
    };
    form_urlencoded::parse(qs.as_bytes())
        .into_owned()
        .collect()
}

fn decode_param(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned()
}

fn parse_json_body(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_slice(bytes).unwrap_or_else(|_| Value::Object(Map::new()))
}

fn parse_limit(query: &BTreeMap<String, String>) -> usize {
    query
        .get("limit")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n > 0.0)
        .map_or(200, |n| (n as usize).clamp(1, 500))
}

fn scope_of(query: &BTreeMap<String, String>) -> Result<Scope, Refusal> {
    let mut named = 0;
    let mut scope = Scope::default();
    if let Some(v) = query.get("session").filter(|s| !s.is_empty()) {
        scope.session = Some(v.clone());
        named += 1;
    }
    if let Some(v) = query.get("bot").filter(|s| !s.is_empty()) {
        scope.bot = Some(v.clone());
        named += 1;
    }
    if let Some(v) = query.get("group").filter(|s| !s.is_empty()) {
        scope.group = Some(v.clone());
        named += 1;
    }
    if let Some(v) = query.get("project").filter(|s| !s.is_empty()) {
        scope.project = Some(v.clone());
        named += 1;
    }
    if named != 1 {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Say whose: one of session, bot, group or project.",
        ));
    }
    if let Some(kind_str) = query.get("kind").filter(|s| !s.is_empty()) {
        let Some(kind) = Kind::parse(kind_str) else {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                format!("There's no kind called {kind_str}."),
            ));
        };
        scope.kind = Some(kind);
    }
    if query.get("pinned").map(String::as_str) == Some("1") {
        scope.pinned = Some(true);
    }
    if let Some(by) = query.get("by").filter(|s| !s.is_empty()) {
        scope.by = Some(by.clone());
    }
    Ok(scope)
}

fn client_version(version: &Version) -> Value {
    let mut val = serde_json::to_value(version).unwrap();
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "at".into(),
            parse_seconds(&version.at).map_or(Value::Null, Value::from),
        );
    }
    val
}

fn client_header(header: &ArtifactHeader, store: &ArtifactStore) -> Map<String, Value> {
    let mut val = serde_json::to_value(header).unwrap();
    let obj = val.as_object_mut().unwrap();
    obj.insert("restorable".into(), Value::Bool(store.can_restore(header)));
    obj.insert(
        "updated".into(),
        parse_seconds(&header.updated).map_or(Value::Null, Value::from),
    );
    obj.insert(
        "gone".into(),
        header
            .gone
            .as_deref()
            .and_then(parse_seconds)
            .map_or(Value::Null, Value::from),
    );
    if let Some(shown) = &header.shown {
        obj.insert(
            "shown".into(),
            parse_seconds(shown).map_or(Value::Null, Value::from),
        );
    }
    if let Some(pinned) = &header.pinned {
        let mut p = Map::new();
        p.insert("by".into(), serde_json::to_value(pinned.by).unwrap());
        p.insert(
            "at".into(),
            parse_seconds(&pinned.at).map_or(Value::Null, Value::from),
        );
        if let Some(note) = &pinned.note {
            p.insert("note".into(), Value::String(note.clone()));
        }
        obj.insert("pinned".into(), Value::Object(p));
    } else {
        obj.insert("pinned".into(), Value::Null);
    }
    std::mem::take(obj)
}

pub fn client_summary(summary: &Summary, store: &ArtifactStore) -> Value {
    let mut obj = client_header(&summary.header, store);
    obj.insert("versions".into(), Value::from(summary.versions));
    obj.insert("latest".into(), client_version(&summary.latest));
    Value::Object(obj)
}

pub fn client_artifact(artifact: &Artifact, store: &ArtifactStore) -> Value {
    let mut obj = client_header(&artifact.header(), store);
    let versions: Vec<Value> = artifact.versions.iter().map(client_version).collect();
    obj.insert("versions".into(), Value::Array(versions));
    Value::Object(obj)
}

fn list_json(store: &ArtifactStore, scope: &Scope, limit: usize, before: Option<&str>) -> Value {
    let (artifacts, next) = store.list(scope, limit, before);
    let items: Vec<Value> = artifacts
        .iter()
        .map(|s| client_summary(s, store))
        .collect();
    json!({ "artifacts": items, "next": next })
}

fn timeline_json(
    store: &ArtifactStore,
    scope: &Scope,
    limit: usize,
    before: Option<&str>,
) -> Value {
    let (entries, next) = store.timeline(scope, limit, before);
    let items: Vec<Value> = entries
        .iter()
        .map(|(header, version)| {
            json!({
                "artifact": Value::Object(client_header(header, store)),
                "version": client_version(version),
            })
        })
        .collect();
    json!({ "entries": items, "next": next })
}

fn not_kept(version: &Version) -> &'static str {
    if version.status == VersionStatus::Deleted {
        return "It was deleted then.";
    }
    match version.reason {
        Some(NoCopy::Secret) => "It looks like it holds a secret, so no copy is kept.",
        Some(NoCopy::Ignored) => "Git ignores it, so no copy is kept.",
        Some(NoCopy::Git) => "Git keeps this file's history: see it in Changes.",
        Some(NoCopy::TooBig) => "It's too big to keep a copy of.",
        Some(NoCopy::LowDisk) => "The disk was nearly full, so no copy was kept.",
        _ => "That version is no longer kept.",
    }
}
