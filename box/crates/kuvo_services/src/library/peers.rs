//! Cross-computer Library gathering, peer copy caching (`run/library/peers/<key>.json`),
//! and peer act handling (`library_for_peer`), matching `box/src/link/areas/library.ts`.

use std::{
    collections::HashMap,
    fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use super::{
    service::LibraryService,
    store::{Filter, LibraryKind, matches_library},
};
use crate::artifacts::routes::PendingComputer;

pub const LIST_LIMIT: usize = 1000;
const MOST_KEPT: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerComputer {
    pub key: String,
    pub name: String,
    pub here: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartState {
    Here,
    There,
    Away(String),
    Older(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Asked {
    pub computer: PeerComputer,
    pub state: PartState,
    pub part: Option<Value>,
    pub seen: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeptEntry {
    pub query: String,
    pub seen: i64,
    pub part: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct KeptFile {
    lists: Vec<KeptEntry>,
}

pub fn valid_peer_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn is_part(val: &Value) -> bool {
    val.as_object()
        .and_then(|o| o.get("items"))
        .is_some_and(Value::is_array)
}

fn kept_file(peers_dir: &Path, key: &str) -> PathBuf {
    peers_dir.join(format!("{key}.json"))
}

pub fn kept_of(peers_dir: &Path, key: &str) -> Vec<KeptEntry> {
    if !valid_peer_key(key) {
        return Vec::new();
    }
    let Ok(raw) = fs::read_to_string(kept_file(peers_dir, key)) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<KeptFile>(&raw) else {
        return Vec::new();
    };
    parsed
        .lists
        .into_iter()
        .filter(|each| is_part(&each.part))
        .collect()
}

pub fn kept_part(peers_dir: &Path, key: &str, query: &str) -> Option<KeptEntry> {
    kept_of(peers_dir, key)
        .into_iter()
        .find(|each| each.query == query)
}

pub fn keep_part(peers_dir: &Path, key: &str, query: &str, part: Value, seen: i64) {
    if !valid_peer_key(key) {
        return;
    }
    let has_q = form_urlencoded::parse(query.as_bytes()).any(|(k, _)| k == "q");
    if has_q {
        return;
    }
    let mut lists = vec![KeptEntry {
        query: query.to_owned(),
        seen,
        part,
    }];
    for entry in kept_of(peers_dir, key) {
        if entry.query != query {
            lists.push(entry);
        }
    }
    lists.truncate(MOST_KEPT);
    if let Ok(encoded) = serde_json::to_string(&KeptFile { lists }) {
        let _ = kuvo_store::files::write_secret(&kept_file(peers_dir, key), encoded.as_bytes());
    }
}

pub fn last_seen_message(name: &str) -> String {
    format!(
        "{name} isn’t answering now, so its part of the Library is as it was when last seen."
    )
}

pub fn away_hidden_message(name: &str) -> String {
    format!("{name} isn’t answering now, so its part of the Library isn’t shown.")
}

pub fn older_message(name: &str) -> String {
    format!("{name} needs a newer Kuvo Code to show its Library here.")
}

/// Merge multiple computers' parts into the `LIBRARY` route answer shape.
pub fn merged(parts: &[Asked]) -> Value {
    let mut items: Vec<Value> = Vec::new();
    let mut bots: Vec<Value> = Vec::new();
    let mut seen_bots: Vec<String> = Vec::new();
    let mut you = false;
    let mut more = false;

    for asked in parts {
        if let Some(part) = &asked.part {
            let mut label = Map::new();
            label.insert("key".into(), Value::String(asked.computer.key.clone()));
            label.insert("name".into(), Value::String(asked.computer.name.clone()));
            if asked.computer.here {
                label.insert("here".into(), Value::Bool(true));
            }
            let label_val = Value::Object(label);
            if let Some(part_items) = part.get("items").and_then(Value::as_array) {
                for item in part_items {
                    if let Some(obj) = item.as_object() {
                        let mut cloned = obj.clone();
                        cloned.insert("computer".into(), label_val.clone());
                        items.push(Value::Object(cloned));
                    }
                }
            }
            if let Some(sources) = part.get("sources").and_then(Value::as_object) {
                if sources.get("you").and_then(Value::as_bool) == Some(true) {
                    you = true;
                }
                if let Some(src_bots) = sources.get("bots").and_then(Value::as_array) {
                    for bot in src_bots {
                        let Some(id) = bot.get("id").and_then(Value::as_str) else {
                            continue;
                        };
                        if seen_bots.iter().any(|existing| existing == id) {
                            continue;
                        }
                        seen_bots.push(id.to_owned());
                        let mut entry = Map::new();
                        entry.insert("id".into(), Value::String(id.to_owned()));
                        entry.insert(
                            "name".into(),
                            bot.get("name")
                                .and_then(Value::as_str)
                                .map_or(Value::Null, |s| Value::String(s.to_owned())),
                        );
                        if let Some(look) = bot.get("look").and_then(Value::as_object) {
                            entry.insert("look".into(), Value::Object(look.clone()));
                        }
                        entry.insert("computer".into(), Value::String(asked.computer.key.clone()));
                        bots.push(Value::Object(entry));
                    }
                }
            }
            if part.get("more").and_then(Value::as_bool) == Some(true) {
                more = true;
            }
        }
    }

    items.sort_by(|a, b| {
        let a_at = a.get("at").and_then(Value::as_f64).unwrap_or(0.0);
        let b_at = b.get("at").and_then(Value::as_f64).unwrap_or(0.0);
        b_at.total_cmp(&a_at).then_with(|| {
            let a_id = a.get("id").and_then(Value::as_str).unwrap_or("");
            let b_id = b.get("id").and_then(Value::as_str).unwrap_or("");
            a_id.cmp(b_id)
        })
    });
    let truncated = items.len() > LIST_LIMIT;
    items.truncate(LIST_LIMIT);

    let computers: Vec<Value> = parts
        .iter()
        .map(|asked| {
            let mut obj = Map::new();
            obj.insert("key".into(), Value::String(asked.computer.key.clone()));
            obj.insert("name".into(), Value::String(asked.computer.name.clone()));
            if asked.computer.here {
                obj.insert("here".into(), Value::Bool(true));
            }
            match &asked.state {
                PartState::Here => {
                    obj.insert("state".into(), Value::String("here".into()));
                }
                PartState::There => {
                    obj.insert("state".into(), Value::String("there".into()));
                }
                PartState::Away(msg) => {
                    obj.insert("state".into(), Value::String("away".into()));
                    obj.insert("message".into(), Value::String(msg.clone()));
                }
                PartState::Older(msg) => {
                    obj.insert("state".into(), Value::String("older".into()));
                    obj.insert("message".into(), Value::String(msg.clone()));
                }
                PartState::Failed(msg) => {
                    obj.insert("state".into(), Value::String("failed".into()));
                    obj.insert("message".into(), Value::String(msg.clone()));
                }
            }
            if let Some(seen) = asked.seen {
                obj.insert("seen".into(), Value::from(seen));
            }
            Value::Object(obj)
        })
        .collect();

    json!({
        "items": items,
        "more": more || truncated,
        "sources": { "bots": bots, "you": you },
        "computers": computers,
    })
}

/// Filter a peer's full `library-v1.json` snapshot by `filter` on this computer.
pub fn filter_peer_copy(copy: &Value, filter: &Filter, computer_name: &str, away: bool) -> Option<Value> {
    let items = copy.get("items")?.as_array()?;
    let bot_names: HashMap<String, String> = copy
        .get("botNames")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let filtered: Vec<Value> = items
        .iter()
        .filter(|item| {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            let kind = item
                .get("kind")
                .and_then(Value::as_str)
                .and_then(LibraryKind::parse)
                .unwrap_or(LibraryKind::Other);
            let bot = item.get("bot").and_then(Value::as_str);
            let is_hidden = item.get("hidden").is_some_and(|v| !v.is_null());
            matches_library(
                filter,
                name,
                kind,
                bot,
                is_hidden,
                |id| bot_names.get(id).cloned(),
                computer_name,
            )
        })
        .map(|item| {
            if away && let Some(obj) = item.as_object() {
                let mut cloned = obj.clone();
                cloned.insert("there".into(), Value::Bool(false));
                cloned.insert("thumbnail".into(), Value::Bool(false));
                Value::Object(cloned)
            } else {
                item.clone()
            }
        })
        .collect();
    let sources = copy
        .get("sources")
        .cloned()
        .unwrap_or_else(|| json!({ "bots": [], "you": false }));
    Some(json!({
        "items": filtered,
        "sources": sources,
    }))
}

/// Answer an incoming peer act in the `library` area (`list`, `<id>`, `<id>/file`,
/// `<id>/thumbnail`, `<id>/hide`), matching `libraryForPeer` in `areas/library.ts`.
pub async fn library_for_peer(
    service: &LibraryService,
    method: &str,
    item: &str,
    body: Option<&Map<String, Value>>,
) -> (u16, Value) {
    if !is_valid_peer_item(item) {
        return (404, json!({ "message": "No such route." }));
    }
    let hides = item.ends_with("/hide");
    let upper = method.to_ascii_uppercase();
    if hides != (upper == "POST") {
        return (
            405,
            json!({ "message": if hides { "Remove it with POST." } else { "Read it with GET." } }),
        );
    }
    if item == "list" {
        let query_str = body
            .and_then(|b| b.get("query"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let truncated: String = query_str.chars().take(1000).collect();
        let params: HashMap<String, String> = form_urlencoded::parse(truncated.as_bytes())
            .into_owned()
            .collect();
        return match super::routes::filter_of(&params) {
            Ok(filter) => (200, service.listed_local(&filter, LIST_LIMIT)),
            Err(msg) => (400, json!({ "message": msg })),
        };
    }
    let mut parts = item.split('/');
    let id = parts.next().unwrap_or("");
    let sub = parts.next();
    let Some(found) = service.store().get(id) else {
        return (404, json!({ "message": "That isn't in this computer's Library." }));
    };
    match sub {
        None => (200, service.client_item(&found)),
        Some("file") => match service.bytes(&found) {
            Some(bytes) => (
                200,
                json!({ "type": found.mime, "data": STANDARD.encode(&bytes) }),
            ),
            None => (410, json!({ "message": "It's no longer on this computer." })),
        },
        Some("thumbnail") => {
            if !service.thumbnailable(&found) {
                return (404, json!({ "message": "There's no thumbnail of it." }));
            }
            match service.thumbnail(&found).await {
                Some(made) => (
                    200,
                    json!({ "type": made.mime, "data": STANDARD.encode(&made.bytes) }),
                ),
                None => (404, json!({ "message": "There's no thumbnail of it." })),
            }
        }
        Some("hide") => {
            let hidden = body
                .and_then(|b| b.get("hidden"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            match service.store().hide(&found.id, hidden) {
                Some(updated) => (200, service.client_item(&updated)),
                None => (404, json!({ "message": "That isn't in this computer's Library." })),
            }
        }
        _ => (404, json!({ "message": "No such route." })),
    }
}

pub fn is_valid_peer_item(item: &str) -> bool {
    if item == "list" {
        return true;
    }
    let Some(rest) = item.strip_prefix("lib_") else {
        return false;
    };
    let (hex_part, tail) = match rest.split_once('/') {
        Some((h, t)) => (h, Some(t)),
        None => (rest, None),
    };
    if hex_part.len() != 24
        || !hex_part
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return false;
    }
    matches!(tail, None | Some("file" | "thumbnail" | "hide"))
}

/// Snapshot of peers' library state for `kuvo/setup/library/events` and `kuvo/library`.
#[derive(Clone, Debug)]
pub struct PeerLibraryCopy {
    pub computer: PeerComputer,
    pub away: bool,
    pub older: bool,
    pub seen: Option<i64>,
    pub copy: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct LibraryPeerSnapshot {
    pub local: PeerComputer,
    pub peers: Vec<PeerLibraryCopy>,
    pub pending: Vec<PendingComputer>,
}

pub type PeerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait LibraryPeers: Send + Sync {
    fn local(&self) -> PeerComputer;
    fn peers_dir(&self) -> PathBuf;
    fn ask_peers<'a>(&'a self, query: &'a str) -> PeerFuture<'a, Vec<Asked>>;
    fn kept_peers(&self, query: &str) -> Vec<Asked>;
    fn ask_item<'a>(
        &'a self,
        computer_key: &'a str,
        method: &'a str,
        item: &'a str,
        body: Option<Map<String, Value>>,
    ) -> PeerFuture<'a, Result<(u16, Value), (u16, String)>>;
    fn merged_snapshot<'a>(&'a self, filter: &'a Filter, query: &'a str)
        -> PeerFuture<'a, (Vec<Asked>, Vec<PendingComputer>)>;
    fn changes(&self) -> watch::Receiver<u64>;
}
