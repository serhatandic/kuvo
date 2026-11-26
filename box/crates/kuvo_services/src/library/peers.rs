//! Cross-computer Library gathering, peer copy caching (`run/library/peers/<key>.json`),
//! and peer act handling (`library_for_peer`), matching `box/src/link/areas/library.ts`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
pub struct PeerComputer { pub key: String, pub name: String, pub here: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartState { Here, There, Away(String), Older(String), Failed(String) }

#[derive(Clone, Debug)]
pub struct Asked {
    pub computer: PeerComputer,
    pub state: PartState,
    pub part: Option<Value>,
    pub seen: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeptEntry { pub query: String, pub seen: i64, pub part: Value }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct KeptFile { lists: Vec<KeptEntry> }

pub fn valid_peer_key(key: &str) -> bool {
    key.len() == 43 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn is_part(val: &Value) -> bool { val.get("items").is_some_and(Value::is_array) }
fn kept_file(peers_dir: &Path, key: &str) -> PathBuf { peers_dir.join(format!("{key}.json")) }

pub fn kept_of(peers_dir: &Path, key: &str) -> Vec<KeptEntry> {
    if !valid_peer_key(key) { return Vec::new(); }
    kuvo_store::files::text_of(&kept_file(peers_dir, key))
        .and_then(|raw| serde_json::from_str::<KeptFile>(&raw).ok())
        .map_or_else(Vec::new, |f| f.lists.into_iter().filter(|e| is_part(&e.part)).collect())
}

pub fn kept_part(peers_dir: &Path, key: &str, query: &str) -> Option<KeptEntry> {
    kept_of(peers_dir, key).into_iter().find(|e| e.query == query)
}

pub fn keep_part(peers_dir: &Path, key: &str, query: &str, part: Value, seen: i64) {
    if !valid_peer_key(key) || form_urlencoded::parse(query.as_bytes()).any(|(k, _)| k == "q") { return; }
    let mut lists = vec![KeptEntry { query: query.to_owned(), seen, part }];
    lists.extend(kept_of(peers_dir, key).into_iter().filter(|e| e.query != query));
    lists.truncate(MOST_KEPT);
    if let Ok(encoded) = serde_json::to_string(&KeptFile { lists }) {
        let _ = kuvo_store::files::write_secret(&kept_file(peers_dir, key), encoded.as_bytes());
    }
}

pub fn last_seen_message(name: &str) -> String {
    format!("{name} isn’t answering now, so its part of the Library is as it was when last seen.")
}
pub fn away_hidden_message(name: &str) -> String {
    format!("{name} isn’t answering now, so its part of the Library isn’t shown.")
}
pub fn older_message(name: &str) -> String {
    format!("{name} needs a newer Kuvo Code to show its Library here.")
}

/// Merge multiple computers' parts into the `LIBRARY` route answer shape.
pub fn merged(parts: &[Asked]) -> Value {
    let (mut items, mut bots, mut seen_bots, mut you, mut more) = (Vec::new(), Vec::new(), HashSet::new(), false, false);
    for asked in parts {
        let Some(part) = &asked.part else { continue };
        let mut label = json!({ "key": asked.computer.key, "name": asked.computer.name });
        if asked.computer.here { label["here"] = json!(true); }
        for item in part.get("items").and_then(Value::as_array).into_iter().flatten() {
            if let Some(obj) = item.as_object() {
                let mut cloned = obj.clone();
                cloned.insert("computer".into(), label.clone());
                items.push(Value::Object(cloned));
            }
        }
        if let Some(sources) = part.get("sources").and_then(Value::as_object) {
            you |= sources.get("you").and_then(Value::as_bool) == Some(true);
            for bot in sources.get("bots").and_then(Value::as_array).into_iter().flatten() {
                let Some(id) = bot.get("id").and_then(Value::as_str) else { continue };
                if !seen_bots.insert(id.to_owned()) { continue; }
                let mut entry = json!({
                    "id": id,
                    "name": bot.get("name").and_then(Value::as_str),
                    "computer": asked.computer.key,
                });
                if let Some(look) = bot.get("look").and_then(Value::as_object) {
                    entry["look"] = Value::Object(look.clone());
                }
                bots.push(entry);
            }
        }
        more |= part.get("more").and_then(Value::as_bool) == Some(true);
    }

    items.sort_by(|a, b| {
        let at = |v: &Value| v.get("at").and_then(Value::as_f64).unwrap_or(0.0);
        fn id(v: &Value) -> &str { v.get("id").and_then(Value::as_str).unwrap_or("") }
        at(b).total_cmp(&at(a)).then_with(|| id(a).cmp(id(b)))
    });
    let truncated = items.len() > LIST_LIMIT;
    items.truncate(LIST_LIMIT);

    let computers: Vec<Value> = parts.iter().map(|asked| {
        let (state, msg) = match &asked.state {
            PartState::Here => ("here", None),
            PartState::There => ("there", None),
            PartState::Away(m) => ("away", Some(m.as_str())),
            PartState::Older(m) => ("older", Some(m.as_str())),
            PartState::Failed(m) => ("failed", Some(m.as_str())),
        };
        let mut obj = json!({ "key": asked.computer.key, "name": asked.computer.name, "state": state });
        if asked.computer.here { obj["here"] = json!(true); }
        if let Some(m) = msg { obj["message"] = json!(m); }
        if let Some(seen) = asked.seen { obj["seen"] = json!(seen); }
        obj
    }).collect();

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
    let bot_names: HashMap<String, String> = copy.get("botNames").and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        .unwrap_or_default();
    let filtered: Vec<Value> = items.iter().filter(|item| {
        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
        let kind = item.get("kind").and_then(Value::as_str).and_then(LibraryKind::parse).unwrap_or(LibraryKind::Other);
        let bot = item.get("bot").and_then(Value::as_str);
        let is_hidden = item.get("hidden").is_some_and(|v| !v.is_null());
        matches_library(filter, name, kind, bot, is_hidden, |id| bot_names.get(id).cloned(), computer_name)
    }).map(|item| {
        if away && let Some(obj) = item.as_object() {
            let mut cloned = obj.clone();
            cloned.insert("there".into(), json!(false));
            cloned.insert("thumbnail".into(), json!(false));
            Value::Object(cloned)
        } else {
            item.clone()
        }
    }).collect();
    let sources = copy.get("sources").cloned().unwrap_or_else(|| json!({ "bots": [], "you": false }));
    Some(json!({ "items": filtered, "sources": sources }))
}

/// Answer an incoming peer act in the `library` area (`list`, `<id>`, `<id>/file`,
/// `<id>/thumbnail`, `<id>/hide`), matching `libraryForPeer` in `areas/library.ts`.
pub async fn library_for_peer(service: &LibraryService, method: &str, item: &str, body: Option<&Map<String, Value>>) -> (u16, Value) {
    if !is_valid_peer_item(item) { return (404, json!({ "message": "No such route." })); }
    let hides = item.ends_with("/hide");
    if hides != (method.eq_ignore_ascii_case("POST")) {
        return (405, json!({ "message": if hides { "Remove it with POST." } else { "Read it with GET." } }));
    }
    if item == "list" {
        let q: String = body.and_then(|b| b.get("query")).and_then(Value::as_str).unwrap_or("").chars().take(1000).collect();
        let params: BTreeMap<String, String> = form_urlencoded::parse(q.as_bytes()).into_owned().collect();
        return match super::routes::filter_of(&params) {
            Ok(filter) => (200, service.listed_local(&filter, LIST_LIMIT)),
            Err(msg) => (400, json!({ "message": msg })),
        };
    }
    let (id, sub) = item.split_once('/').map_or((item, None), |(i, s)| (i, Some(s)));
    let Some(found) = service.store().get(id) else {
        return (404, json!({ "message": "That isn't in this computer's Library." }));
    };
    match sub {
        None => (200, service.client_item(&found)),
        Some("file") => match service.bytes(&found) {
            Some(bytes) => (200, json!({ "type": found.mime, "data": STANDARD.encode(&bytes) })),
            None => (410, json!({ "message": "It's no longer on this computer." })),
        },
        Some("thumbnail") => match if service.thumbnailable(&found) { service.thumbnail(&found).await } else { None } {
            Some(made) => (200, json!({ "type": made.mime, "data": STANDARD.encode(&made.bytes) })),
            None => (404, json!({ "message": "There's no thumbnail of it." })),
        },
        Some("hide") => {
            let hidden = body.and_then(|b| b.get("hidden")).and_then(Value::as_bool).unwrap_or(true);
            match service.store().hide(&found.id, hidden) {
                Some(updated) => (200, service.client_item(&updated)),
                None => (404, json!({ "message": "That isn't in this computer's Library." })),
            }
        }
        _ => (404, json!({ "message": "No such route." })),
    }
}

pub fn is_valid_peer_item(item: &str) -> bool {
    if item == "list" { return true; }
    let Some(rest) = item.strip_prefix("lib_") else { return false };
    let (hex_part, tail) = rest.split_once('/').map_or((rest, None), |(h, t)| (h, Some(t)));
    hex_part.len() == 24
        && hex_part.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && matches!(tail, None | Some("file" | "thumbnail" | "hide"))
}

pub type PeerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait LibraryPeers: Send + Sync {
    fn local(&self) -> PeerComputer;
    fn peers_dir(&self) -> PathBuf;
    fn ask_peers<'a>(&'a self, query: &'a str) -> PeerFuture<'a, Vec<Asked>>;
    fn kept_peers(&self, query: &str) -> Vec<Asked>;
    fn ask_item<'a>(&'a self, computer_key: &'a str, method: &'a str, item: &'a str, body: Option<Map<String, Value>>) -> PeerFuture<'a, Result<(u16, Value), (u16, String)>>;
    fn merged_snapshot<'a>(&'a self, filter: &'a Filter, query: &'a str) -> PeerFuture<'a, (Vec<Asked>, Vec<PendingComputer>)>;
    fn changes(&self) -> watch::Receiver<u64>;
    fn next_change(&self, _query: &str) -> Option<std::time::Duration> { None }
    fn legacy_keys(&self) -> std::collections::BTreeSet<String> { Default::default() }
    fn ask_legacy<'a>(&'a self, query: &'a str) -> PeerFuture<'a, Vec<Asked>> { self.ask_peers(query) }
}
