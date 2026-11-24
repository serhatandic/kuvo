//! HTTP and line routes for the Library (`kuvo/library…`, `kuvo/setup/library/events`),
//! matching `box/src/link/library/routes.ts` and `box/src/link/areas/library.ts`.

use std::{collections::HashMap, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::stream;
use http::{
    HeaderValue, StatusCode,
    header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
};
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    routes::{
        get_kuvo_library, get_kuvo_library_id, get_kuvo_library_id_file,
        get_kuvo_library_id_thumbnail, get_kuvo_setup_library_events, post_kuvo_library_id_hide,
    },
    shape::{Primitive, Shape},
    version::{FormatRefusalCode, UnsupportedFormatVersion},
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use super::{
    peers::{Asked, LIST_LIMIT, LibraryPeers, PartState, PeerComputer, merged},
    service::LibraryService,
    store::{Filter, Item, LibraryKind},
};

pub const FEATURES: &[&str] = &["library", "line.merged-library"];

static ANY_SHAPE: Shape = Shape::Primitive(Primitive::Any);

struct GetLibraryEvents;
impl RouteSpec for GetLibraryEvents {
    type Body = ();
    const ROUTE: &'static Route = &Route {
        method: "GET",
        path: "kuvo/library/events",
        feature: "library",
        answers: &[(200, &ANY_SHAPE)],
        body: None,
    };
}
impl JsonStatus<200> for GetLibraryEvents {
    type Body = Value;
}

#[derive(Clone)]
pub enum LibraryHandle {
    Ready(LibraryService),
    UnsupportedFormat(String),
    Unavailable,
}

impl LibraryHandle {
    fn service(&self) -> Result<&LibraryService, Refusal> {
        match self {
            Self::Ready(service) => Ok(service),
            Self::UnsupportedFormat(msg) => Err(Refusal::unsupported_record(
                UnsupportedFormatVersion {
                    code: FormatRefusalCode::UnsupportedFormatVersion,
                    message: msg.clone(),
                },
            )),
            Self::Unavailable => Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer's Library isn't running. Try again in a moment.",
            )),
        }
    }
}

pub fn filter_of(params: &HashMap<String, String>) -> Result<Filter, String> {
    let mut filter = Filter::default();
    if let Some(source) = params.get("source").filter(|s| !s.is_empty()) {
        let valid_bot = source
            .strip_prefix("bot_")
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()));
        if source != "all" && source != "you" && !valid_bot {
            return Err("Say whose: all, you or a bot's id (source).".to_owned());
        }
        filter.source = Some(source.clone());
    }
    if let Some(kind_str) = params.get("kind").filter(|s| !s.is_empty() && *s != "all") {
        let Some(kind) = LibraryKind::parse(kind_str) else {
            return Err(format!("There's no kind called {kind_str}."));
        };
        filter.kind = Some(kind);
    }
    if let Some(words) = params.get("q") {
        let trimmed = words.trim();
        if !trimmed.is_empty() {
            filter.query = Some(trimmed.chars().take(200).collect());
        }
    }
    if params.get("hidden").map(String::as_str) == Some("1") {
        filter.hidden = true;
    }
    Ok(filter)
}

fn parse_query(path: &str) -> HashMap<String, String> {
    let Some((_, qs)) = path.split_once('?') else {
        return HashMap::new();
    };
    form_urlencoded::parse(qs.as_bytes()).into_owned().collect()
}

fn list_query_string(params: &HashMap<String, String>) -> String {
    let mut ser = form_urlencoded::Serializer::new(String::new());
    for key in ["source", "kind", "q", "hidden"] {
        if let Some(v) = params.get(key).filter(|s| !s.is_empty()) {
            let truncated: String = v.chars().take(200).collect();
            ser.append_pair(key, &truncated);
        }
    }
    ser.finish()
}

fn decode_param(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned()
}

fn parse_json_body(bytes: &[u8]) -> Result<Value, Refusal> {
    if bytes.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(bytes).map_err(|_| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Say whether it's removed (hidden).",
        )
    })
}

fn local_computer(handle: &LibraryHandle, peers: Option<&dyn LibraryPeers>) -> PeerComputer {
    if let Some(peers) = peers {
        return peers.local();
    }
    let name = match handle {
        LibraryHandle::Ready(service) => service.computer_name(),
        _ => String::new(),
    };
    PeerComputer {
        key: "here".to_owned(),
        name,
        here: true,
    }
}

fn own_part(
    handle: &LibraryHandle,
    peers: Option<&dyn LibraryPeers>,
    filter: &Filter,
) -> Asked {
    let computer = local_computer(handle, peers);
    match handle {
        LibraryHandle::Ready(service) => Asked {
            computer,
            state: PartState::Here,
            part: Some(service.listed_local(filter, LIST_LIMIT)),
            seen: None,
        },
        _ => Asked {
            computer,
            state: PartState::Failed(
                "This computer’s Library can’t be read right now. Try again in a moment."
                    .to_owned(),
            ),
            part: None,
            seen: None,
        },
    }
}

fn is_local_computer(
    key: Option<&str>,
    handle: &LibraryHandle,
    peers: Option<&dyn LibraryPeers>,
) -> bool {
    let Some(key) = key.filter(|s| !s.is_empty()) else {
        return true;
    };
    if key == "here" {
        return true;
    }
    key == local_computer(handle, peers).key
}

pub fn register(
    registry: &mut Registry,
    handle: LibraryHandle,
    peers: Option<Arc<dyn LibraryPeers>>,
) {
    // GET kuvo/library
    let h = handle.clone();
    let p = peers.clone();
    registry.register::<get_kuvo_library::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let p = p.clone();
        let params = parse_query(&call.request.path);
        async move {
            let query = list_query_string(&params);
            let here = match filter_of(&params) {
                Ok(filter) => own_part(&h, p.as_deref(), &filter),
                Err(_) => Asked {
                    computer: local_computer(&h, p.as_deref()),
                    state: PartState::Failed(
                        "This computer’s Library can’t be read right now. Try again in a moment."
                            .to_owned(),
                    ),
                    part: None,
                    seen: None,
                },
            };
            let there = match p.as_deref() {
                Some(peers) => peers.ask_peers(&query).await,
                None => Vec::new(),
            };
            let mut all = Vec::with_capacity(1 + there.len());
            all.push(here);
            all.extend(there);
            Ok(RouteReply::json_as_kept::<200>(merged(&all)))
        }
    });

    // GET kuvo/library/events
    let h = handle.clone();
    let p = peers.clone();
    registry.register::<GetLibraryEvents, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let p = p.clone();
        let params = parse_query(&call.request.path);
        let cancel = call.cancel.clone();
        async move {
            let filter = filter_of(&params).ok();
            let query = list_query_string(&params);
            let local_rx = match &h {
                LibraryHandle::Ready(service) => Some(service.store().subscribe()),
                _ => None,
            };
            let initial_there = match p.as_deref() {
                Some(peers) => peers.kept_peers(&query),
                None => Vec::new(),
            };
            let stream = stream::unfold(
                (
                    h,
                    p,
                    filter,
                    query,
                    local_rx,
                    cancel,
                    initial_there,
                    None::<String>,
                    true,
                ),
                |(h, p, filter, query, mut local_rx, cancel, mut there, mut last, first)| async move {
                    loop {
                        if cancel.is_cancelled() {
                            return None;
                        }
                        if !first {
                            let refresh = tokio::time::sleep(Duration::from_secs(20));
                            tokio::pin!(refresh);
                            let mut refresh_peers = false;
                            tokio::select! {
                                () = cancel.cancelled() => return None,
                                () = &mut refresh => { refresh_peers = true; }
                                res = recv_item(&mut local_rx) => {
                                    if !res { continue; }
                                    tokio::select! {
                                        () = cancel.cancelled() => return None,
                                        () = tokio::time::sleep(Duration::from_millis(100)) => {}
                                    }
                                    if let Some(rx) = &mut local_rx {
                                        while rx.try_recv().is_ok() {}
                                    }
                                }
                            }
                            if refresh_peers && let Some(peers) = p.as_deref() {
                                there = peers.ask_peers(&query).await;
                            }
                        }
                        let here = match &filter {
                            Some(f) => own_part(&h, p.as_deref(), f),
                            None => Asked {
                                computer: local_computer(&h, p.as_deref()),
                                state: PartState::Failed(
                                    "This computer’s Library can’t be read right now. Try again in a moment."
                                        .to_owned(),
                                ),
                                part: None,
                                seen: None,
                            },
                        };
                        let mut all = Vec::with_capacity(1 + there.len());
                        all.push(here);
                        all.extend(there.clone());
                        let state = merged(&all);
                        let line = serde_json::to_string(&state).unwrap_or_default();
                        if last.as_deref() != Some(line.as_str()) {
                            last = Some(line);
                            let item = JsonLine::<GetLibraryEvents, 200>::new(state);
                            return Some((
                                item,
                                (h, p, filter, query, local_rx, cancel, there, last, false),
                            ));
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

    // GET kuvo/setup/library/events
    let h = handle.clone();
    let p = peers.clone();
    registry.register::<get_kuvo_setup_library_events::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let p = p.clone();
        let params = parse_query(&call.request.path);
        let cancel = call.cancel.clone();
        async move {
            let filter = filter_of(&params)
                .map_err(|msg| Refusal::new(StatusCode::BAD_REQUEST, msg))?;
            let query = list_query_string(&params);
            let local_rx = match &h {
                LibraryHandle::Ready(service) => Some(service.store().subscribe()),
                _ => None,
            };
            let peer_rx = p.as_ref().map(|peers| peers.changes());
            let stream = stream::unfold(
                (h, p, filter, query, local_rx, peer_rx, cancel, None::<String>, true),
                |(h, p, filter, query, mut local_rx, mut peer_rx, cancel, mut last, first)| async move {
                    if !first {
                        let sleep = tokio::time::sleep(Duration::from_secs(3));
                        tokio::pin!(sleep);
                        tokio::select! {
                            () = cancel.cancelled() => return None,
                            () = &mut sleep => {}
                            _ = recv_item(&mut local_rx) => {}
                            _ = watch_u64(&mut peer_rx) => {}
                        }
                    }
                    loop {
                        if cancel.is_cancelled() {
                            return None;
                        }
                        if let Some(state) =
                            build_merged_library(&h, p.as_deref(), &filter, &query).await
                        {
                            let line = serde_json::to_string(&state).unwrap_or_default();
                            if last.as_deref() != Some(line.as_str()) {
                                last = Some(line);
                                let item = serde_json::from_value(state)
                                    .map_err(std::io::Error::other)
                                    .and_then(JsonLine::<get_kuvo_setup_library_events::Spec, 200>::new);
                                return Some((
                                    item,
                                    (h, p, filter, query, local_rx, peer_rx, cancel, last, false),
                                ));
                            }
                        }
                        let sleep = tokio::time::sleep(Duration::from_secs(3));
                        tokio::pin!(sleep);
                        tokio::select! {
                            () = cancel.cancelled() => return None,
                            () = &mut sleep => {}
                            _ = recv_item(&mut local_rx) => {}
                            _ = watch_u64(&mut peer_rx) => {}
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

    // GET kuvo/library/:id
    let h = handle.clone();
    let p = peers.clone();
    registry.register::<get_kuvo_library_id::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let p = p.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        let params = parse_query(&call.request.path);
        async move {
            let computer = params.get("computer").map(String::as_str);
            if !is_local_computer(computer, &h, p.as_deref()) {
                return forward_json_item(p.as_deref(), computer.unwrap(), "GET", &id, None).await;
            }
            let service = h.service()?;
            let Some(item) = service.store().get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's Library.",
                ));
            };
            Ok(RouteReply::json_as_kept::<200>(service.client_item(&item)))
        }
    });

    // GET kuvo/library/:id/file
    let h = handle.clone();
    let p = peers.clone();
    registry.register::<get_kuvo_library_id_file::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let p = p.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        let params = parse_query(&call.request.path);
        async move {
            let computer = params.get("computer").map(String::as_str);
            if !is_local_computer(computer, &h, p.as_deref()) {
                return forward_bytes_item(
                    p.as_deref(),
                    computer.unwrap(),
                    &format!("{id}/file"),
                    false,
                )
                .await;
            }
            let service = h.service()?;
            let Some(item) = service.store().get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's Library.",
                ));
            };
            let Some(bytes) = service.bytes(&item) else {
                return Err(Refusal::new(
                    StatusCode::GONE,
                    "It's no longer on this computer.",
                ));
            };
            Ok(bytes_reply(&item.mime, bytes, false))
        }
    });

    // GET kuvo/library/:id/thumbnail
    let h = handle.clone();
    let p = peers.clone();
    registry.register::<get_kuvo_library_id_thumbnail::Spec, _, _>(Access::Any, move |call| {
        let h = h.clone();
        let p = p.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        let params = parse_query(&call.request.path);
        async move {
            let computer = params.get("computer").map(String::as_str);
            if !is_local_computer(computer, &h, p.as_deref()) {
                return forward_bytes_item(
                    p.as_deref(),
                    computer.unwrap(),
                    &format!("{id}/thumbnail"),
                    true,
                )
                .await;
            }
            let service = h.service()?;
            let Some(item) = service.store().get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's Library.",
                ));
            };
            let made = if service.thumbnailable(&item) {
                service.thumbnail(&item).await
            } else {
                None
            };
            let Some(made) = made else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There's no thumbnail of it.",
                ));
            };
            Ok(bytes_reply(&made.mime, made.bytes, true))
        }
    });

    // POST kuvo/library/:id/hide
    registry.register::<post_kuvo_library_id_hide::Spec, _, _>(Access::Any, move |call| {
        let h = handle.clone();
        let p = peers.clone();
        let id = decode_param(call.param("id").unwrap_or(""));
        let params = parse_query(&call.request.path);
        let raw = call.request.body.clone();
        async move {
            let body = parse_json_body(&raw)?;
            let computer = params.get("computer").map(String::as_str);
            if !is_local_computer(computer, &h, p.as_deref()) {
                let map = body.as_object().cloned().unwrap_or_default();
                return forward_json_item(
                    p.as_deref(),
                    computer.unwrap(),
                    "POST",
                    &format!("{id}/hide"),
                    Some(map),
                )
                .await;
            }
            let service = h.service()?;
            let Some(item) = service.store().get(&id) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "That isn't in this computer's Library.",
                ));
            };
            let hidden = match body.get("hidden") {
                None => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => {
                    return Err(Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Say whether it's removed (hidden).",
                    ));
                }
            };
            let updated = service.store().hide(&item.id, hidden).unwrap_or(item);
            Ok(RouteReply::json_as_kept::<200>(service.client_item(&updated)))
        }
    });
}

async fn build_merged_library(
    handle: &LibraryHandle,
    peers: Option<&dyn LibraryPeers>,
    filter: &Filter,
    query: &str,
) -> Option<Value> {
    let here = own_part(handle, peers, filter);
    let (known, pending) = match peers {
        Some(p) => p.merged_snapshot(filter, query).await,
        None => (Vec::new(), Vec::new()),
    };
    let first_peer_copy = known.iter().any(|part| part.part.is_some());
    if pending.is_empty() && here.part.is_none() && !first_peer_copy {
        return None;
    }
    let mut parts = Vec::with_capacity(1 + known.len());
    parts.push(here);
    parts.extend(known);
    let mut state = merged(&parts);
    if let Some(obj) = state.as_object_mut() {
        if !pending.is_empty() && !first_peer_copy {
            obj.insert("items".into(), Value::Array(Vec::new()));
            obj.insert("more".into(), Value::Bool(false));
        }
        let pending_json: Vec<Value> = pending
            .iter()
            .map(|c| json!({ "id": c.id, "name": c.name }))
            .collect();
        obj.insert("pendingComputers".into(), Value::Array(pending_json));
    }
    Some(state)
}

async fn forward_json_item<R: JsonStatus<200>>(
    peers: Option<&dyn LibraryPeers>,
    computer: &str,
    method: &str,
    item: &str,
    body: Option<Map<String, Value>>,
) -> Result<RouteReply<R>, Refusal> {
    let Some(peers) = peers else {
        return Err(Refusal::new(
            StatusCode::NOT_FOUND,
            "That computer isn’t one of yours any more.",
        ));
    };
    match peers.ask_item(computer, method, item, body).await {
        Ok((200, val)) => Ok(RouteReply::json_as_kept::<200>(val)),
        Ok((status, val)) => Err(Refusal::json(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            val,
        )),
        Err((status, msg)) => Err(Refusal::new(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            msg,
        )),
    }
}

async fn forward_bytes_item<R: kuvo_protocol::ByteStatus<200>>(
    peers: Option<&dyn LibraryPeers>,
    computer: &str,
    item: &str,
    no_cache: bool,
) -> Result<RouteReply<R>, Refusal> {
    let Some(peers) = peers else {
        return Err(Refusal::new(
            StatusCode::NOT_FOUND,
            "That computer isn’t one of yours any more.",
        ));
    };
    match peers.ask_item(computer, "GET", item, None).await {
        Ok((200, val)) => {
            if let Some(mime) = val.get("type").and_then(Value::as_str)
                && let Some(data) = val.get("data").and_then(Value::as_str)
                && let Ok(bytes) = STANDARD.decode(data)
            {
                return Ok(bytes_reply(mime, bytes, no_cache));
            }
            Err(Refusal::new(
                StatusCode::BAD_GATEWAY,
                "That computer returned an unreadable file.",
            ))
        }
        Ok((status, val)) => Err(Refusal::json(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            val,
        )),
        Err((status, msg)) => Err(Refusal::new(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            msg,
        )),
    }
}

fn bytes_reply<R: kuvo_protocol::ByteStatus<200>>(
    mime: &str,
    bytes: Vec<u8>,
    no_cache: bool,
) -> RouteReply<R> {
    let len = bytes.len();
    let mut reply = RouteReply::bytes::<200>("application/octet-stream", bytes);
    if let Ok(ct) = HeaderValue::from_str(mime) {
        reply.headers_mut().insert(CONTENT_TYPE, ct);
    }
    reply
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from(len));
    if no_cache {
        reply
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    reply
}

async fn recv_item(rx: &mut Option<tokio::sync::broadcast::Receiver<Item>>) -> bool {
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

async fn watch_u64(rx: &mut Option<watch::Receiver<u64>>) -> bool {
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
