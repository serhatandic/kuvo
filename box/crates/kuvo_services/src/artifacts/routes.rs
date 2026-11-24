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
    ByteStatus, JsonStatus, Route, RouteSpec,
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
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

use super::{
    changes::changes,
    kinds::{Kind, is_prose, is_text},
    policy::NoCopy,
    store::{
        Artifact, ArtifactHeader, ArtifactStore, PinBy, Scope, StoreError, Summary, Version,
        VersionStatus, parse_seconds,
    },
};

pub const FEATURES: &[&str] = &["artifacts", "line.merged-outputs"];

pub(crate) static ANY_SHAPE: Shape = Shape::Primitive(Primitive::Any);

macro_rules! any_get_route {
    ($name:ident, $path:literal, $feature:literal) => {
        pub(crate) struct $name;
        impl RouteSpec for $name {
            type Body = ();
            const ROUTE: &'static Route = &Route {
                method: "GET",
                path: $path,
                feature: $feature,
                answers: &[(200, &$crate::artifacts::routes::ANY_SHAPE)],
                body: None,
            };
        }
        impl JsonStatus<200> for $name {
            type Body = Value;
        }
    };
}
pub(crate) use any_get_route;

any_get_route!(GetArtifactsEvents, "kuvo/artifacts/events", "artifacts");
any_get_route!(GetArtifactsTimelineEvents, "kuvo/artifacts/timeline/events", "artifacts");

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
pub struct PendingComputer { pub id: String, pub name: String }

#[derive(Clone, Debug)]
pub struct OutputsSnapshot {
    pub local_id: String,
    pub local_name: String,
    pub children: HashMap<String, Vec<String>>,
    pub peers: Vec<OutputPeerCopy>,
    pub pending: Vec<PendingComputer>,
}

pub type OutputsFuture<'a> = Pin<Box<dyn Future<Output = Option<OutputsSnapshot>> + Send + 'a>>;

pub trait OutputsPeers: Send + Sync {
    fn snapshot(&self) -> OutputsFuture<'_>;
    fn changes(&self) -> watch::Receiver<u64>;
}

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
            Self::UnsupportedFormat(msg) => Err(unsupported_refusal(msg.clone())),
            Self::Unavailable => Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer's record of what its agents made isn't running. Try again in a moment.",
            )),
        }
    }
}

pub(crate) fn unsupported_refusal(message: String) -> Refusal {
    Refusal::unsupported_record(UnsupportedFormatVersion {
        code: FormatRefusalCode::UnsupportedFormatVersion,
        message,
    })
}

pub(crate) fn parse_query(path: &str) -> BTreeMap<String, String> {
    path.split_once('?')
        .map_or_else(BTreeMap::new, |(_, qs)| form_urlencoded::parse(qs.as_bytes()).into_owned().collect())
}

pub(crate) fn decode_param(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

pub(crate) fn bytes_reply<R: ByteStatus<200>>(mime: &str, bytes: Vec<u8>, no_cache: bool) -> RouteReply<R> {
    let len = bytes.len();
    let mut reply = RouteReply::bytes::<200>("application/octet-stream", bytes);
    if let Ok(ct) = HeaderValue::from_str(mime) { reply.headers_mut().insert(CONTENT_TYPE, ct); }
    reply.headers_mut().insert(CONTENT_LENGTH, HeaderValue::from(len));
    if no_cache { reply.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache")); }
    reply
}

pub(crate) async fn recv_broadcast_opt<T: Clone>(rx: &mut Option<broadcast::Receiver<T>>) -> bool {
    match rx {
        Some(r) => match r.recv().await {
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => true,
            Err(broadcast::error::RecvError::Closed) => { *rx = None; false }
        },
        None => std::future::pending().await,
    }
}

pub(crate) async fn watch_opt(rx: &mut Option<watch::Receiver<u64>>) -> bool {
    match rx {
        Some(r) => match r.changed().await {
            Ok(()) => true,
            Err(_) => { *rx = None; false }
        },
        None => std::future::pending().await,
    }
}

pub(crate) fn merged_ndjson<R, T, F, Fut>(
    local_rx: Option<broadcast::Receiver<T>>,
    peer_rx: Option<watch::Receiver<u64>>,
    cancel: CancellationToken,
    read: F,
) -> RouteReply<R>
where
    R: JsonStatus<200>,
    <R as JsonStatus<200>>::Body: serde::de::DeserializeOwned + Send + 'static,
    T: Clone + Send + 'static,
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = Option<Value>> + Send,
{
    let stream = stream::unfold(
        (read, local_rx, peer_rx, cancel, None::<String>, true),
        |(read, mut local_rx, mut peer_rx, cancel, mut last, mut first)| async move {
            loop {
                if cancel.is_cancelled() { return None; }
                if !first {
                    let sleep = tokio::time::sleep(Duration::from_secs(3));
                    tokio::pin!(sleep);
                    tokio::select! {
                        () = cancel.cancelled() => return None,
                        () = &mut sleep => {}
                        _ = recv_broadcast_opt(&mut local_rx) => {}
                        _ = watch_opt(&mut peer_rx) => {}
                    }
                }
                first = false;
                if let Some(state) = read().await {
                    let line = serde_json::to_string(&state).unwrap_or_default();
                    if last.as_deref() != Some(line.as_str()) {
                        last = Some(line);
                        let item = serde_json::from_value(state)
                            .map_err(std::io::Error::other)
                            .and_then(JsonLine::<R, 200>::new);
                        return Some((item, (read, local_rx, peer_rx, cancel, last, false)));
                    }
                }
            }
        },
    );
    let mut reply = RouteReply::ndjson::<200>(stream);
    reply.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    reply
}

fn find_artifact(store: &ArtifactStore, id: &str) -> Result<Artifact, Refusal> {
    store.get(id).ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "That isn't in this computer's record."))
}

fn find_version<'a>(artifact: &'a Artifact, n_raw: &str) -> Result<(u64, &'a Version), Refusal> {
    let missing = || Refusal::new(StatusCode::NOT_FOUND, "There's no such version.");
    let n = n_raw.parse::<u64>().map_err(|_| missing())?;
    let v = artifact.versions.iter().find(|v| v.n == n).ok_or_else(missing)?;
    Ok((n, v))
}

fn parse_json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| Value::Object(Map::new()))
}

pub fn register(registry: &mut Registry, handle: ArtifactHandle, peers: Option<Arc<dyn OutputsPeers>>) {
    let h = handle.clone();
    registry.register::<get_kuvo_artifacts::Spec, _, _>(Access::Any, move |call| {
        let (h, query) = (h.clone(), parse_query(&call.request.path));
        async move {
            let store = h.store()?;
            let scope = scope_of(&query)?;
            let before = query.get("before").filter(|s| !s.is_empty()).map(String::as_str);
            Ok(RouteReply::json_as_kept::<200>(list_json(store, &scope, parse_limit(&query), before)))
        }
    });

    let h = handle.clone();
    registry.register::<GetArtifactsEvents, _, _>(Access::Any, move |call| {
        let (h, query, cancel) = (h.clone(), parse_query(&call.request.path), call.cancel.clone());
        async move {
            let store = h.store()?.clone();
            let scope = scope_of(&query)?;
            Ok(followed_artifacts(store.clone(), move || list_json(&store, &scope, 200, None), cancel))
        }
    });

    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_timeline::Spec, _, _>(Access::Any, move |call| {
        let (h, query) = (h.clone(), parse_query(&call.request.path));
        async move {
            let store = h.store()?;
            store.settled().await;
            let scope = scope_of(&query)?;
            let before = query.get("before").filter(|s| !s.is_empty()).map(String::as_str);
            Ok(RouteReply::json_as_kept::<200>(timeline_json(store, &scope, parse_limit(&query), before)))
        }
    });

    let h = handle.clone();
    registry.register::<GetArtifactsTimelineEvents, _, _>(Access::Any, move |call| {
        let (h, query, cancel) = (h.clone(), parse_query(&call.request.path), call.cancel.clone());
        async move {
            let store = h.store()?.clone();
            store.settled().await;
            let scope = scope_of(&query)?;
            Ok(followed_artifacts(store.clone(), move || timeline_json(&store, &scope, 200, None), cancel))
        }
    });

    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_storage::Spec, _, _>(Access::Any, move |_| {
        let h = h.clone();
        async move {
            let store = h.store()?;
            store.settled().await;
            Ok(RouteReply::json_as_kept::<200>(json!(store.storage())))
        }
    });

    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_storage::Spec, _, _>(Access::Any, move |call| {
        let (h, raw) = (h.clone(), call.request.body.clone());
        async move {
            let store = h.store()?;
            store.settled().await;
            let given = parse_json_body(&raw);
            let budget = given.get("budget").and_then(Value::as_f64).filter(|b| b.is_finite() && *b >= 0.0)
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Say the budget for older versions, in bytes (budget)."))?;
            let current = store.storage();
            if given.get("was").is_some_and(|w| w.as_f64() != Some(current.budget as f64))
                || given.get("wasOlder").is_some_and(|w| w.as_f64() != Some(current.older as f64))
            {
                return Err(Refusal::new(StatusCode::CONFLICT, "It changed since you read it."));
            }
            let confirmed = given.get("confirmed").and_then(Value::as_bool) == Some(true);
            match store.set_budget(budget.floor() as u64, confirmed) {
                Ok(Some(msg)) => Err(Refusal::new(StatusCode::BAD_REQUEST, msg)),
                Ok(None) => Ok(RouteReply::json_as_kept::<200>(json!(store.storage()))),
                Err(StoreError::UnsupportedFormat(msg)) => Err(unsupported_refusal(msg)),
                Err(err) => Err(Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
            }
        }
    });

    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_storage_clear::Spec, _, _>(Access::Any, move |call| {
        let (h, raw) = (h.clone(), call.request.body.clone());
        async move {
            let store = h.store()?;
            store.settled().await;
            let given = parse_json_body(&raw);
            let current = store.storage();
            if given.get("was").is_some_and(|w| w.as_f64() != Some(current.older as f64)) {
                return Err(Refusal::new(StatusCode::CONFLICT, "It changed since you read it."));
            }
            let before_iso = given.get("before").and_then(Value::as_f64).filter(|v| v.is_finite())
                .and_then(|s| jiff::Timestamp::from_millisecond((s * 1000.0) as i64).ok())
                .map(super::store::stamp);
            let cleared = store.clear_older(before_iso.as_deref());
            let mut obj = serde_json::to_value(store.storage()).unwrap();
            obj.as_object_mut().unwrap().insert("cleared".into(), json!(cleared));
            Ok(RouteReply::json_as_kept::<200>(obj))
        }
    });

    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_id::Spec, _, _>(Access::Any, move |call| {
        let (h, id) = (h.clone(), decode_param(call.param("id").unwrap_or("")));
        async move {
            let store = h.store()?;
            store.settled().await;
            let artifact = find_artifact(store, &id)?;
            let checked = store.check_gone(&artifact.id).await.unwrap_or(artifact);
            Ok(RouteReply::json_as_kept::<200>(client_artifact(&checked, store)))
        }
    });

    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_id_pin::Spec, _, _>(Access::Any, move |call| {
        let (h, id, raw) = (h.clone(), decode_param(call.param("id").unwrap_or("")), call.request.body.clone());
        async move {
            let store = h.store()?;
            store.settled().await;
            let artifact = find_artifact(store, &id)?;
            let given = parse_json_body(&raw);
            let pinned = given.get("pinned").and_then(Value::as_bool)
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Say whether it's pinned (pinned)."))?;
            let note = given.get("note").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
            let changed = store.pin(&artifact.id, pinned.then_some((PinBy::You, note))).await
                .ok_or_else(|| Refusal::new(StatusCode::CONFLICT, "It couldn't be kept now. Try again in a moment."))?;
            Ok(RouteReply::json_as_kept::<200>(client_artifact(&changed, store)))
        }
    });

    let h = handle.clone();
    registry.register::<post_kuvo_artifacts_id_restore::Spec, _, _>(Access::Any, move |call| {
        let (h, id, raw) = (h.clone(), decode_param(call.param("id").unwrap_or("")), call.request.body.clone());
        async move {
            let store = h.store()?;
            store.settled().await;
            let artifact = find_artifact(store, &id)?;
            let version = parse_json_body(&raw).get("version").and_then(Value::as_f64)
                .filter(|v| v.is_finite() && v.fract() == 0.0 && *v >= 0.0)
                .map(|v| v as u64)
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Say which version to put back (version)."))?;
            let restored = store.restore(&artifact.id, version, None).await
                .map_err(|msg| Refusal::new(StatusCode::CONFLICT, msg))?;
            Ok(RouteReply::json_as_kept::<200>(client_artifact(&restored, store)))
        }
    });

    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_id_versions_n::Spec, _, _>(Access::Any, move |call| {
        let (h, id, n_raw, query) = (
            h.clone(),
            decode_param(call.param("id").unwrap_or("")),
            decode_param(call.param("n").unwrap_or("")),
            parse_query(&call.request.path),
        );
        async move {
            let store = h.store()?;
            let artifact = find_artifact(store, &id)?;
            let (n, version) = find_version(&artifact, &n_raw)?;
            let bytes = if query.get("preview").map(String::as_str) == Some("1") {
                store.image_preview(&artifact.id, n).await.flatten()
            } else {
                store.bytes(&artifact.id, n).flatten()
            }
            .ok_or_else(|| Refusal::new(StatusCode::GONE, not_kept(version)))?;
            Ok(bytes_reply(&artifact.mime, bytes, false))
        }
    });

    let h = handle.clone();
    registry.register::<get_kuvo_artifacts_id_versions_n_changes::Spec, _, _>(Access::Any, move |call| {
        let (h, id, n_raw, query) = (
            h.clone(),
            decode_param(call.param("id").unwrap_or("")),
            decode_param(call.param("n").unwrap_or("")),
            parse_query(&call.request.path),
        );
        async move {
            let store = h.store()?;
            store.settled().await;
            let artifact = find_artifact(store, &id)?;
            let (n, version) = find_version(&artifact, &n_raw)?;
            let from = match query.get("from") {
                Some(raw) => raw.parse::<u64>().ok().and_then(|m| artifact.versions.iter().find(|v| v.n == m)),
                None => artifact.versions.iter().rev().find(|v| v.n < n),
            };
            if version.reason.is_some_and(NoCopy::is_private) {
                return Err(Refusal::new(StatusCode::GONE, not_kept(version)));
            }
            if let Some(patch) = &version.patch {
                return Ok(RouteReply::json_as_kept::<200>(json!({
                    "style": "lines", "patch": patch,
                    "added": version.lines.as_ref().map_or(0, |l| l.added),
                    "removed": version.lines.as_ref().map_or(0, |l| l.removed),
                    "from": from.map(|v| v.n), "to": n,
                })));
            }
            if !is_text(&artifact.mime) {
                let style = if artifact.kind == Kind::Image || artifact.mime == "application/pdf" { "pair" } else { "none" };
                return Ok(RouteReply::json_as_kept::<200>(json!({ "style": style, "from": from.map(|v| v.n), "to": n })));
            }
            let read_v = |v: &Version| if v.status == VersionStatus::Deleted { Some(Vec::new()) } else { store.bytes(&artifact.id, v.n).flatten() };
            let after = read_v(version);
            let before = from.map_or_else(|| Some(Vec::new()), read_v);
            let (Some(before_bytes), Some(after_bytes)) = (before, after) else {
                let msg = if version.kept { "The version before isn't kept, so what changed can't be shown." } else { not_kept(version) };
                return Err(Refusal::new(StatusCode::GONE, msg));
            };
            let mut diff = serde_json::to_value(changes(
                &String::from_utf8_lossy(&before_bytes),
                &String::from_utf8_lossy(&after_bytes),
                is_prose(&artifact.path),
            )).unwrap();
            if let Some(obj) = diff.as_object_mut() {
                obj.insert("from".into(), from.map_or(Value::Null, |v| Value::from(v.n)));
                obj.insert("to".into(), Value::from(n));
            }
            Ok(RouteReply::json_as_kept::<200>(diff))
        }
    });

    let (h, p) = (handle.clone(), peers.clone());
    registry.register::<get_kuvo_setup_outputs::Spec, _, _>(Access::Any, move |call| {
        let (h, p, query) = (h.clone(), p.clone(), parse_query(&call.request.path));
        async move {
            let session = validate_session_query(&query)?;
            let before = parse_outputs_before(query.get("before").map(String::as_str));
            let state = build_setup_outputs(&h, p.as_deref(), &session, before.as_ref()).await
                .ok_or_else(|| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The outputs aren't ready yet."))?;
            Ok(RouteReply::json_as_kept::<200>(state))
        }
    });

    registry.register::<get_kuvo_setup_outputs_events::Spec, _, _>(Access::Any, move |call| {
        let (h, p, query, cancel) = (handle.clone(), peers.clone(), parse_query(&call.request.path), call.cancel.clone());
        async move {
            let session = validate_session_query(&query)?;
            let before = parse_outputs_before(query.get("before").map(String::as_str));
            let local_rx = match &h { ArtifactHandle::Ready(store) => Some(store.subscribe()), _ => None };
            let peer_rx = p.as_ref().map(|peers| peers.changes());
            Ok(merged_ndjson::<get_kuvo_setup_outputs_events::Spec, _, _, _>(
                local_rx,
                peer_rx,
                cancel,
                move || {
                    let (h, p, session, before) = (h.clone(), p.clone(), session.clone(), before.clone());
                    async move { build_setup_outputs(&h, p.as_deref(), &session, before.as_ref()).await }
                },
            ))
        }
    });
}

fn followed_artifacts<R>(
    store: ArtifactStore,
    read: impl Fn() -> Value + Send + 'static,
    cancel: CancellationToken,
) -> RouteReply<R>
where
    R: JsonStatus<200>,
    <R as JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    let stream = stream::unfold(
        (store.subscribe(), read, cancel, None::<String>, true),
        |(mut rx, read, cancel, mut last, first)| async move {
            loop {
                if cancel.is_cancelled() { return None; }
                if !first {
                    tokio::select! {
                        () = cancel.cancelled() => return None,
                        res = rx.recv() => if matches!(res, Err(broadcast::error::RecvError::Closed)) { return None; }
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
                    let item = serde_json::from_value(value).map_err(std::io::Error::other).and_then(JsonLine::<R, 200>::new);
                    return Some((item, (rx, read, cancel, last, false)));
                }
                if first { return None; }
            }
        },
    );
    let mut reply = RouteReply::ndjson::<200>(stream);
    reply.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    reply
}

pub fn local_output_copy(store: &ArtifactStore, sessions: Option<&[String]>) -> Vec<PeerOutputRow> {
    store.session_outputs(sessions).into_iter().map(|s| PeerOutputRow {
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
            reason: s.latest.reason.map(|r| serde_json::to_value(r).unwrap().as_str().unwrap().to_owned()),
        },
        sessions: s.header.where_.sessions,
    }).collect()
}

type OutputOrder = (i64, f64, String, String);

fn parse_outputs_before(raw: Option<&str>) -> Option<OutputOrder> {
    let bytes = URL_SAFE_NO_PAD.decode(raw?).ok()?;
    let arr = serde_json::from_slice::<Value>(&bytes).ok()?.as_array()?.clone();
    if arr.len() != 4 { return None; }
    Some((arr[0].as_i64()?, arr[1].as_f64()?, arr[2].as_str()?.to_owned(), arr[3].as_str()?.to_owned()))
}

fn compare_output_order(a: &OutputOrder, b: &OutputOrder) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| b.1.total_cmp(&a.1)).then_with(|| b.2.cmp(&a.2)).then_with(|| b.3.cmp(&a.3))
}

fn validate_session_query(query: &BTreeMap<String, String>) -> Result<String, Refusal> {
    let session = query.get("session").map(String::as_str).unwrap_or("");
    let valid = session.strip_prefix("ses_").is_some_and(|r| {
        !r.is_empty() && r.len() <= 95 && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    valid.then(|| session.to_owned()).ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Choose a conversation."))
}

async fn build_setup_outputs(
    handle: &ArtifactHandle,
    peers: Option<&dyn OutputsPeers>,
    root_session: &str,
    before: Option<&OutputOrder>,
) -> Option<Value> {
    let snap = match peers { Some(p) => p.snapshot().await, None => None };
    let mut sessions = HashSet::from([root_session.to_owned()]);
    if let Some(snap) = &snap {
        let mut queue = vec![root_session.to_owned()];
        while let Some(s) = queue.pop() {
            for child in snap.children.get(&s).into_iter().flatten() {
                if sessions.insert(child.clone()) { queue.push(child.clone()); }
            }
        }
    }
    let session_list: Vec<String> = sessions.iter().cloned().collect();
    let here_rows = match handle { ArtifactHandle::Ready(store) => Some(local_output_copy(store, Some(&session_list))), _ => None };
    let peer_copies = snap.as_ref().map_or(&[][..], |s| s.peers.as_slice());
    let pending = snap.as_ref().map_or(&[][..], |s| s.pending.as_slice());
    if here_rows.is_none() && peer_copies.is_empty() && pending.is_empty() { return None; }

    let local_id = snap.as_ref().map_or("here", |s| s.local_id.as_str());
    let local_name = snap.as_ref().map_or("this computer", |s| s.local_name.as_str());
    let mut rows: Vec<(OutputOrder, Value)> = Vec::new();
    let mut push_row = |row: &PeerOutputRow, comp_id: &str, comp_name: &str, here: bool, away: bool, last_seen: Option<i64>| {
        let row_id = format!("{comp_id}:{}", row.id);
        let ord: OutputOrder = (i64::from(row.pinned), row.updated.unwrap_or(0.0), row_id.clone(), comp_id.to_owned());
        let mut computer = json!({ "id": comp_id, "name": comp_name });
        if here { computer["here"] = json!(true); }
        let mut val = json!({
            "id": row_id, "artifactId": row.id, "path": row.path, "name": row.name,
            "kind": row.kind, "type": row.mime, "gone": row.gone, "updated": row.updated,
            "pinned": row.pinned, "latest": row.latest, "computer": computer, "away": away,
        });
        if away && let Some(seen) = last_seen { val["lastSeen"] = json!(seen); }
        rows.push((ord, val));
    };
    for row in here_rows.iter().flatten() { push_row(row, local_id, local_name, true, false, None); }
    for peer in peer_copies {
        for row in peer.artifacts.iter().filter(|r| r.sessions.iter().any(|s| sessions.contains(s))) {
            push_row(row, &peer.computer_id, &peer.computer_name, false, peer.away, peer.last_seen);
        }
    }
    if let Some(before) = before {
        rows.retain(|(ord, _)| compare_output_order(ord, before) == std::cmp::Ordering::Greater);
    }
    rows.sort_by(|(a, _), (b, _)| compare_output_order(a, b));
    let has_more = rows.len() > 200;
    rows.truncate(200);
    let next = has_more.then(|| {
        rows.last().map(|(ord, _)| URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!([ord.0, ord.1, ord.2, ord.3])).unwrap()))
    }).flatten();
    let artifacts: Vec<Value> = rows.into_iter().map(|(_, v)| v).collect();
    let pending_json: Vec<Value> = pending.iter().map(|c| json!({ "id": c.id, "name": c.name })).collect();
    Some(json!({ "artifacts": artifacts, "next": next, "pendingComputers": pending_json }))
}

fn parse_limit(query: &BTreeMap<String, String>) -> usize {
    query.get("limit").and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n > 0.0)
        .map_or(200, |n| (n as usize).clamp(1, 500))
}

fn scope_of(query: &BTreeMap<String, String>) -> Result<Scope, Refusal> {
    let get = |k: &str| query.get(k).filter(|s| !s.is_empty()).cloned();
    let scope = Scope {
        session: get("session"),
        bot: get("bot"),
        group: get("group"),
        project: get("project"),
        kind: match get("kind") {
            Some(k) => Some(Kind::parse(&k).ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, format!("There's no kind called {k}.")))?),
            None => None,
        },
        pinned: (query.get("pinned").map(String::as_str) == Some("1")).then_some(true),
        by: get("by"),
    };
    let named = [scope.session.is_some(), scope.bot.is_some(), scope.group.is_some(), scope.project.is_some()]
        .into_iter().filter(|&b| b).count();
    if named != 1 {
        return Err(Refusal::new(StatusCode::BAD_REQUEST, "Say whose: one of session, bot, group or project."));
    }
    Ok(scope)
}

fn client_version(version: &Version) -> Value {
    let mut val = serde_json::to_value(version).unwrap();
    val["at"] = parse_seconds(&version.at).map_or(Value::Null, Value::from);
    val
}

fn client_header(header: &ArtifactHeader, store: &ArtifactStore) -> Map<String, Value> {
    let mut val = serde_json::to_value(header).unwrap();
    let obj = val.as_object_mut().unwrap();
    obj.insert("restorable".into(), json!(store.can_restore(header)));
    obj.insert("updated".into(), parse_seconds(&header.updated).map_or(Value::Null, Value::from));
    obj.insert("gone".into(), header.gone.as_deref().and_then(parse_seconds).map_or(Value::Null, Value::from));
    if let Some(shown) = &header.shown {
        obj.insert("shown".into(), parse_seconds(shown).map_or(Value::Null, Value::from));
    }
    obj.insert("pinned".into(), match &header.pinned {
        Some(pinned) => {
            let mut p = json!({ "by": pinned.by, "at": parse_seconds(&pinned.at) });
            if let Some(note) = &pinned.note { p["note"] = json!(note); }
            p
        }
        None => Value::Null,
    });
    std::mem::take(obj)
}

pub fn client_summary(summary: &Summary, store: &ArtifactStore) -> Value {
    let mut obj = client_header(&summary.header, store);
    obj.insert("versions".into(), json!(summary.versions));
    obj.insert("latest".into(), client_version(&summary.latest));
    Value::Object(obj)
}

pub fn client_artifact(artifact: &Artifact, store: &ArtifactStore) -> Value {
    let mut obj = client_header(&artifact.header, store);
    obj.insert("versions".into(), Value::Array(artifact.versions.iter().map(client_version).collect()));
    Value::Object(obj)
}

fn list_json(store: &ArtifactStore, scope: &Scope, limit: usize, before: Option<&str>) -> Value {
    let (artifacts, next) = store.list(scope, limit, before);
    json!({ "artifacts": artifacts.iter().map(|s| client_summary(s, store)).collect::<Vec<_>>(), "next": next })
}

fn timeline_json(store: &ArtifactStore, scope: &Scope, limit: usize, before: Option<&str>) -> Value {
    let (entries, next) = store.timeline(scope, limit, before);
    let items: Vec<Value> = entries.iter().map(|(h, v)| {
        json!({ "artifact": Value::Object(client_header(h, store)), "version": client_version(v) })
    }).collect();
    json!({ "entries": items, "next": next })
}

fn not_kept(version: &Version) -> &'static str {
    if version.status == VersionStatus::Deleted { return "It was deleted then."; }
    match version.reason {
        Some(NoCopy::Secret) => "It looks like it holds a secret, so no copy is kept.",
        Some(NoCopy::Ignored) => "Git ignores it, so no copy is kept.",
        Some(NoCopy::Git) => "Git keeps this file's history: see it in Changes.",
        Some(NoCopy::TooBig) => "It's too big to keep a copy of.",
        Some(NoCopy::LowDisk) => "The disk was nearly full, so no copy was kept.",
        _ => "That version is no longer kept.",
    }
}
