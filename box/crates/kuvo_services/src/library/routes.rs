//! HTTP and line routes for the Library (`kuvo/library…`, `kuvo/setup/library/events`),
//! matching `box/src/link/library/routes.ts` and `box/src/link/areas/library.ts`.

use kuvo_os::timer_fires::{self, Timer};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http::{HeaderValue, StatusCode, header::CACHE_CONTROL};
use kuvo_protocol::{
    ByteStatus, JsonStatus, Route, RouteSpec,
    routes::{
        get_kuvo_library, get_kuvo_library_id, get_kuvo_library_id_file,
        get_kuvo_library_id_thumbnail, get_kuvo_setup_library_events, post_kuvo_library_id_hide,
    },
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Map, Value, json};

use crate::artifacts::routes::{
    any_get_route, bytes_reply, decode_param, merged_ndjson, parse_query, recv_broadcast_opt,
    unsupported_refusal, watch_opt,
};

use super::{
    peers::{Asked, LIST_LIMIT, LibraryPeers, PartState, PeerComputer, merged},
    service::LibraryService,
    store::{Filter, Item, LibraryKind},
};

pub const FEATURES: &[&str] = &["library", "line.merged-library"];
const UNREADABLE: &str = "This computer’s Library can’t be read right now. Try again in a moment.";

any_get_route!(GetLibraryEvents, "kuvo/library/events", "library");

#[derive(Clone)]
pub enum LibraryHandle {
    Ready(LibraryService),
    UnsupportedFormat(String),
    Unavailable,
}

impl LibraryHandle {
    fn service(&self) -> Result<&LibraryService, Refusal> {
        match self {
            Self::Ready(service) if service.artifacts_ready() => Ok(service),
            Self::UnsupportedFormat(msg) => Err(unsupported_refusal(msg.clone())),
            Self::Unavailable | Self::Ready(_) => Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer's Library isn't running. Try again in a moment.",
            )),
        }
    }

    fn item(&self, id: &str) -> Result<(&LibraryService, Item), Refusal> {
        let service = self.service()?;
        let item = service.store().get(id)
            .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "That isn't in this computer's Library."))?;
        Ok((service, item))
    }
}

pub fn filter_of(params: &BTreeMap<String, String>) -> Result<Filter, String> {
    let mut filter = Filter::default();
    if let Some(source) = params.get("source").filter(|s| !s.is_empty()) {
        let valid_bot = source.strip_prefix("bot_")
            .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric()));
        if source != "all" && source != "you" && !valid_bot {
            return Err("Say whose: all, you or a bot's id (source).".to_owned());
        }
        filter.source = Some(source.clone());
    }
    if let Some(kind_str) = params.get("kind").filter(|s| !s.is_empty() && *s != "all") {
        filter.kind = Some(LibraryKind::parse(kind_str)
            .ok_or_else(|| format!("There's no kind called {kind_str}."))?);
    }
    if let Some(trimmed) = params.get("q").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        filter.query = Some(trimmed.chars().take(200).collect());
    }
    filter.hidden = params.get("hidden").map(String::as_str) == Some("1");
    Ok(filter)
}

fn list_query_string(params: &BTreeMap<String, String>) -> String {
    let mut ser = form_urlencoded::Serializer::new(String::new());
    for key in ["source", "kind", "q", "hidden"] {
        if let Some(v) = params.get(key).filter(|s| !s.is_empty()) {
            let truncated: String = v.chars().take(200).collect();
            ser.append_pair(key, &truncated);
        }
    }
    ser.finish()
}

fn local_computer(handle: &LibraryHandle, peers: Option<&dyn LibraryPeers>) -> PeerComputer {
    if let Some(peers) = peers { return peers.local(); }
    let name = match handle { LibraryHandle::Ready(s) => s.computer_name(), _ => String::new() };
    PeerComputer { key: "here".to_owned(), name, here: true }
}

fn own_part(handle: &LibraryHandle, peers: Option<&dyn LibraryPeers>, filter: Option<&Filter>) -> Asked {
    let computer = local_computer(handle, peers);
    match (handle, filter) {
        (LibraryHandle::Ready(service), Some(filter)) if service.artifacts_ready() => Asked {
            computer,
            state: PartState::Here,
            part: Some(service.listed_local(filter, LIST_LIMIT)),
            seen: None,
        },
        _ => Asked {
            computer,
            state: PartState::Failed(UNREADABLE.to_owned()),
            part: None,
            seen: None,
        },
    }
}

fn remote_computer<'a>(
    params: &'a BTreeMap<String, String>,
    handle: &LibraryHandle,
    peers: Option<&dyn LibraryPeers>,
) -> Option<&'a str> {
    let key = params.get("computer").map(String::as_str).filter(|s| !s.is_empty())?;
    (key != "here" && key != local_computer(handle, peers).key).then_some(key)
}

fn library_sources(handle: &LibraryHandle, state: &Value) -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) {
    let LibraryHandle::Ready(service) = handle else { return Default::default(); };
    let paths = state["items"].as_array().into_iter().flatten().filter(|item| item["computer"]["here"] == true)
        .flat_map(|item| {
            let file = item["path"].as_str().map(std::path::PathBuf::from);
            let bot = item["bot"].as_str().map(|id| service.bots_folder().join(id).join("bot.json"));
            file.into_iter().chain(bot)
        }).collect();
    let roots = if cfg!(windows) { vec![service.bots_folder().to_owned()] } else { Vec::new() };
    (paths, roots)
}

pub fn register(registry: &mut Registry, handle: LibraryHandle, peers: Option<Arc<dyn LibraryPeers>>) {
    let (h, p) = (handle.clone(), peers.clone());
    registry.register::<get_kuvo_library::Spec, _, _>(Access::Any, move |call| {
        let (h, p, params) = (h.clone(), p.clone(), parse_query(&call.request.path));
        async move {
            let query = list_query_string(&params);
            let mut all = vec![own_part(&h, p.as_deref(), filter_of(&params).ok().as_ref())];
            if let Some(peers) = p.as_deref() { all.extend(peers.ask_peers(&query).await); }
            Ok(RouteReply::json_as_kept::<200>(merged(&all)))
        }
    });

    let (h, p) = (handle.clone(), peers.clone());
    registry.register::<GetLibraryEvents, _, _>(Access::Any, move |call| {
        let (h, p, params, cancel) = (h.clone(), p.clone(), parse_query(&call.request.path), call.cancel.clone());
        async move {
            let (filter, query) = (filter_of(&params).ok(), list_query_string(&params));
            let mut local_rx = match &h { LibraryHandle::Ready(s) => Some(s.store().subscribe()), _ => None };
            let mut peer_rx = p.as_deref().map(LibraryPeers::changes);
            let mut there = p.as_deref().map_or_else(Vec::new, |peers| peers.kept_peers(&query));
            let mut legacy = Vec::<Asked>::new();
            let mut keys = std::collections::BTreeSet::new();
            let mut next_legacy = None;
            let mut sources = kuvo_os::watch::FileSet::new(kuvo_os::watch::WatchHub::default());
            let stream = async_stream::try_stream! {
                let mut last = None;
                loop {
                    let current_keys = p.as_deref().map_or_else(Default::default, LibraryPeers::legacy_keys);
                    if keys != current_keys {
                        keys = current_keys;
                        legacy.retain(|part| keys.contains(&part.computer.key));
                        next_legacy = (!keys.is_empty()).then(tokio::time::Instant::now);
                    }
                    let mut all = vec![own_part(&h, p.as_deref(), filter.as_ref())];
                    all.extend(there.iter().filter(|part| !legacy.iter().any(|old| old.computer.key == part.computer.key)).cloned());
                    all.extend(legacy.clone());
                    let state = merged(&all);
                    let (paths, roots) = library_sources(&h, &state);
                    sources.set_files_at_roots(paths, roots);
                    let line = serde_json::to_string(&state).unwrap_or_default();
                    if last.as_ref() != Some(&line) {
                        last = Some(line);
                        yield JsonLine::<GetLibraryEvents, 200>::new(state)?;
                    }
                    let due = p.as_deref().and_then(|peers| peers.next_change(&query));
                    let refresh_legacy = tokio::select! {
                        () = cancel.cancelled() => break,
                        _ = recv_broadcast_opt(&mut local_rx) => false,
                        _ = watch_opt(&mut peer_rx) => false,
                        _ = sources.changed() => false,
                        _ = async { match due { Some(delay) => timer_fires::sleep(Timer::LibraryFollow, delay).await, None => std::future::pending().await } } => false,
                        _ = async { match next_legacy { Some(at) => timer_fires::sleep_until(Timer::LibraryFollow, at).await, None => std::future::pending().await } } => true,
                    };
                    if let LibraryHandle::Ready(service) = &h { service.clear_bot_descriptions(); }
                    if let Some(peers) = p.as_deref() {
                        if let Some(filter) = &filter { there = peers.merged_snapshot(filter, &query).await.0; }
                        if refresh_legacy {
                            legacy = peers.ask_legacy(&query).await;
                            next_legacy = (!keys.is_empty()).then(|| tokio::time::Instant::now() + Duration::from_secs(20));
                        }
                    }
                }
            };
            let mut reply = RouteReply::ndjson::<200>(stream);
            reply.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            Ok(reply)
        }
    });

    let (h, p) = (handle.clone(), peers.clone());
    registry.register::<get_kuvo_setup_library_events::Spec, _, _>(Access::Any, move |call| {
        let (h, p, params, cancel) = (h.clone(), p.clone(), parse_query(&call.request.path), call.cancel.clone());
        async move {
            let filter = filter_of(&params).map_err(|msg| Refusal::new(StatusCode::BAD_REQUEST, msg))?;
            let query = list_query_string(&params);
            let local_rx = match &h { LibraryHandle::Ready(s) => Some(s.store().subscribe()), _ => None };
            let peer_rx = p.as_ref().map(|peers| peers.changes());
            let deadlines = p.clone();
            let deadline_query = query.clone();
            let sources = h.clone();
            Ok(merged_ndjson::<get_kuvo_setup_library_events::Spec, _, _, _>(
                local_rx,
                peer_rx,
                cancel,
                move || {
                    let (h, p, filter, query) = (h.clone(), p.clone(), filter.clone(), query.clone());
                    async move {
                        if let LibraryHandle::Ready(service) = &h { service.clear_bot_descriptions(); }
                        build_merged_library(&h, p.as_deref(), &filter, &query).await
                    }
                },
                move || deadlines.as_deref().and_then(|peers| peers.next_change(&deadline_query)),
                move |state| library_sources(&sources, state),
            ))
        }
    });

    let (h, p) = (handle.clone(), peers.clone());
    registry.register::<get_kuvo_library_id::Spec, _, _>(Access::Any, move |call| {
        let (h, p, id, params) = (h.clone(), p.clone(), decode_param(call.param("id").unwrap_or("")), parse_query(&call.request.path));
        async move {
            if let Some(computer) = remote_computer(&params, &h, p.as_deref()) {
                return forward_json(p.as_deref(), computer, "GET", &id, None).await;
            }
            let (service, item) = h.item(&id)?;
            Ok(RouteReply::json_as_kept::<200>(service.client_item(&item)))
        }
    });

    let (h, p) = (handle.clone(), peers.clone());
    registry.register::<get_kuvo_library_id_file::Spec, _, _>(Access::Any, move |call| {
        let (h, p, id, params) = (h.clone(), p.clone(), decode_param(call.param("id").unwrap_or("")), parse_query(&call.request.path));
        async move {
            if let Some(computer) = remote_computer(&params, &h, p.as_deref()) {
                return forward_bytes(p.as_deref(), computer, &format!("{id}/file"), false).await;
            }
            let (service, item) = h.item(&id)?;
            let bytes = service.bytes(&item).ok_or_else(|| Refusal::new(StatusCode::GONE, "It's no longer on this computer."))?;
            Ok(bytes_reply(&item.mime, bytes, false))
        }
    });

    let (h, p) = (handle.clone(), peers.clone());
    registry.register::<get_kuvo_library_id_thumbnail::Spec, _, _>(Access::Any, move |call| {
        let (h, p, id, params) = (h.clone(), p.clone(), decode_param(call.param("id").unwrap_or("")), parse_query(&call.request.path));
        async move {
            if let Some(computer) = remote_computer(&params, &h, p.as_deref()) {
                return forward_bytes(p.as_deref(), computer, &format!("{id}/thumbnail"), true).await;
            }
            let (service, item) = h.item(&id)?;
            let made = if service.thumbnailable(&item) { service.thumbnail(&item).await } else { None }
                .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "There's no thumbnail of it."))?;
            Ok(bytes_reply(&made.mime, made.bytes, true))
        }
    });

    registry.register::<post_kuvo_library_id_hide::Spec, _, _>(Access::Any, move |call| {
        let (h, p, id, params, raw) = (
            handle.clone(), peers.clone(),
            decode_param(call.param("id").unwrap_or("")),
            parse_query(&call.request.path),
            call.request.body.clone(),
        );
        async move {
            let body: Value = if raw.is_empty() {
                Value::Object(Map::new())
            } else {
                serde_json::from_slice(&raw)
                    .map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, "Say whether it's removed (hidden)."))?
            };
            if let Some(computer) = remote_computer(&params, &h, p.as_deref()) {
                return forward_json(p.as_deref(), computer, "POST", &format!("{id}/hide"), Some(body.as_object().cloned().unwrap_or_default())).await;
            }
            let (service, item) = h.item(&id)?;
            let hidden = match body.get("hidden") {
                None => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(Refusal::new(StatusCode::BAD_REQUEST, "Say whether it's removed (hidden).")),
            };
            let updated = service.store().hide(&item.id, hidden).unwrap_or(item);
            Ok(RouteReply::json_as_kept::<200>(service.client_item(&updated)))
        }
    });
}

async fn build_merged_library(handle: &LibraryHandle, peers: Option<&dyn LibraryPeers>, filter: &Filter, query: &str) -> Option<Value> {
    let here = own_part(handle, peers, Some(filter));
    let (known, pending) = match peers { Some(p) => p.merged_snapshot(filter, query).await, None => (Vec::new(), Vec::new()) };
    let first_peer_copy = known.iter().any(|part| part.part.is_some());
    if pending.is_empty() && here.part.is_none() && !first_peer_copy { return None; }
    let mut parts = Vec::with_capacity(1 + known.len());
    parts.push(here);
    parts.extend(known);
    let mut state = merged(&parts);
    if let Some(obj) = state.as_object_mut() {
        if !pending.is_empty() && !first_peer_copy {
            obj.insert("items".into(), json!([]));
            obj.insert("more".into(), json!(false));
        }
        let pending_json: Vec<Value> = pending.iter().map(|c| json!({ "id": c.id, "name": c.name })).collect();
        obj.insert("pendingComputers".into(), Value::Array(pending_json));
    }
    Some(state)
}

async fn ask_remote(
    peers: Option<&dyn LibraryPeers>,
    computer: &str,
    method: &str,
    item: &str,
    body: Option<Map<String, Value>>,
) -> Result<Value, Refusal> {
    let peers = peers.ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "That computer isn’t one of yours any more."))?;
    match peers.ask_item(computer, method, item, body).await {
        Ok((200, val)) => Ok(val),
        Ok((status, val)) => Err(Refusal::json(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), val)),
        Err((status, msg)) => Err(Refusal::new(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), msg)),
    }
}

async fn forward_json<R: JsonStatus<200>>(
    peers: Option<&dyn LibraryPeers>,
    computer: &str,
    method: &str,
    item: &str,
    body: Option<Map<String, Value>>,
) -> Result<RouteReply<R>, Refusal> {
    Ok(RouteReply::json_as_kept::<200>(ask_remote(peers, computer, method, item, body).await?))
}

async fn forward_bytes<R: ByteStatus<200>>(
    peers: Option<&dyn LibraryPeers>,
    computer: &str,
    item: &str,
    no_cache: bool,
) -> Result<RouteReply<R>, Refusal> {
    let val = ask_remote(peers, computer, "GET", item, None).await?;
    let (Some(mime), Some(data)) = (val.get("type").and_then(Value::as_str), val.get("data").and_then(Value::as_str)) else {
        return Err(Refusal::new(StatusCode::BAD_GATEWAY, "That computer returned an unreadable file."));
    };
    let bytes = STANDARD.decode(data)
        .map_err(|_| Refusal::new(StatusCode::BAD_GATEWAY, "That computer returned an unreadable file."))?;
    Ok(bytes_reply(mime, bytes, no_cache))
}
