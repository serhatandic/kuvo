//! Search this computer's conversations (the engine's and the coding agents'), with a cache keyed
//! by each one's revision.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use http::StatusCode;
use kuvo_protocol::routes::{get_kuvo_search as route, get_kuvo_setup_search as setup};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::{
    Engine, api, disk,
    lifetime::{Demand, Lease},
    sessions,
};

const MOST_TEXT: usize = 400_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchResult {
    pub session: String,
    pub snippet: String,
    pub info: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computer: Option<SearchComputer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchComputer {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug)]
pub enum SearchAvailability {
    Ready,
    Away,
    Unavailable,
}

pub struct SearchPeer {
    pub key: crate::ComputerKey,
    pub computer: SearchComputer,
    pub availability: SearchAvailability,
}

pub type PeerSearch<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<SearchResult>, SearchAvailability>> + Send + 'a>>;

pub trait SearchPeers: Send + Sync {
    fn local(&self) -> SearchComputer;
    fn peers(&self) -> Vec<SearchPeer>;
    fn moved(&self, sessions: &[&str]) -> BTreeMap<String, crate::ComputerKey>;
    fn search(&self, key: crate::ComputerKey, words: String, limit: usize) -> PeerSearch<'_>;
}

#[derive(Clone, Deserialize, Serialize)]
struct Kept {
    version: u8,
    updated: u64,
    text: String,
    folded: String,
}

fn string<'a>(row: &'a Value, key: &str) -> &'a str {
    row[key].as_str().unwrap_or_default()
}

fn hidden(row: &Value) -> bool {
    ["engineFor", "seat", "reviewPart"]
        .iter()
        .any(|key| match &row["metadata"]["kuvo"][*key] {
            Value::Null | Value::Bool(false) => false,
            Value::String(value) => !value.is_empty(),
            Value::Number(value) => value.as_f64() != Some(0.),
            _ => true,
        })
}

fn updated(session: &Value) -> u64 {
    session["time"]["updated"].as_u64().unwrap_or_default()
}

fn fold(text: &str) -> String {
    text.chars()
        .map(|letter| {
            let plain: Vec<_> = letter
                .to_lowercase()
                .nfd()
                .filter(|ch| !is_combining_mark(*ch))
                .nfc()
                .collect();
            let ch = if plain.len() == 1 { plain[0] } else { letter };
            let code = u32::from(ch);
            if code <= 0xffff {
                ch
            } else {
                char::from_u32(0xe000 + code % 0x1900).expect("private use character")
            }
        })
        .collect()
}

fn raw_words(written: &str) -> String {
    let mut words = written.trim();
    let mut kept = Vec::new();
    while let Some((start, inside, size)) = [
        "attached-files",
        "review-comments",
        "named-agents",
        "from-widget",
        "from-phone",
    ]
    .into_iter()
    .find_map(|tag| {
        let body = words.strip_suffix(&format!("\n</{tag}>"))?;
        let open = format!("<{tag}>\n");
        let start = body.rfind(&open)?;
        (start == 0 || body.as_bytes()[start - 1] == b'\n').then_some((
            tag,
            body[start + open.len()..].trim(),
            start,
        ))
    }) {
        if start == "review-comments" {
            kept.push(inside.to_owned());
        }
        if start == "attached-files" {
            kept.push(
                inside
                    .lines()
                    .filter_map(|line| {
                        std::path::Path::new(line.trim())
                            .file_name()
                            .and_then(|name| name.to_str())
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        words = words[..size].trim();
    }
    std::iter::once(words.to_owned())
        .chain(kept.into_iter().rev())
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn user_text(message: &Value) -> Option<String> {
    let text = string(message, "text");
    if message
        .pointer("/metadata/kuvo/note")
        .is_some_and(Value::is_object)
        || text.starts_with("[The user answered] ")
        || ["finished", "still working", "failed", "asks", "stopped"]
            .iter()
            .any(|what| text.starts_with(&format!("[Session {what}] \"")))
    {
        return None;
    }
    if let Some(wake) = text.strip_prefix("[Wake-up] ") {
        return Some(wake.to_owned());
    }
    if let Some((heading, body)) = text.split_once("\n\n") {
        if heading.starts_with("[Message from ")
            && heading.contains(" · ")
            && heading.ends_with(')')
        {
            return Some(body.to_owned());
        }
        if heading.starts_with("[Work from ") && heading.contains(" · ") && heading.ends_with(']')
        {
            return Some(
                body.split("\n\n---\n")
                    .next()
                    .unwrap_or(body)
                    .trim()
                    .to_owned(),
            );
        }
    }
    let files = message["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["name"].as_str())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    Some(
        [raw_words(text), files]
            .into_iter()
            .filter(|block| !block.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

fn search_text(messages: &[Value], session: &Value) -> String {
    let dir = format!(
        "{}/",
        session
            .pointer("/location/directory")
            .and_then(Value::as_str)
            .unwrap_or_else(|| string(session, "directory"))
    );
    let cut = session.pointer("/revert/messageID").and_then(Value::as_str);
    let mut said = Vec::new();
    let mut steps = Vec::new();
    for message in messages
        .iter()
        .take_while(|row| row["id"].as_str() != cut || cut.is_none())
    {
        match string(message, "type") {
            "user" => said.extend(user_text(message)),
            "assistant" => {
                for part in message["content"].as_array().into_iter().flatten() {
                    match string(part, "type") {
                        "text" => said.push(string(part, "text").trim().to_owned()),
                        "tool" => {
                            let input = &part["state"]["input"];
                            let files = ["filePath", "path"]
                                .into_iter()
                                .filter_map(|key| input[key].as_str())
                                .map(|file| file.strip_prefix(&dir).unwrap_or(file));
                            let words = ["command", "pattern", "query", "url", "description"]
                                .into_iter()
                                .filter_map(|key| input[key].as_str());
                            let line = files
                                .chain(words)
                                .map(|value| value.split('\n').next().unwrap_or(value))
                                .collect::<Vec<_>>()
                                .join(" ");
                            steps.push(line);
                        }
                        _ => {}
                    }
                }
            }
            "shell" => {
                let output: String = string(&message["output"], "output")
                    .trim()
                    .chars()
                    .take(2_000)
                    .collect();
                let command = string(message, "command");
                said.push(
                    [command, &output]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                );
            }
            _ => {}
        }
    }
    let mut said = said
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    keep_tail(&mut said, MOST_TEXT);
    let room = MOST_TEXT.saturating_sub(said.chars().count() + 2);
    let mut done = steps
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    keep_tail(&mut done, room);
    [said, done]
        .into_iter()
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn keep_tail(text: &mut String, count: usize) {
    if let Some((at, _)) = text.char_indices().rev().nth(count) {
        let tail = at
            + text[at..]
                .chars()
                .next()
                .expect("character boundary")
                .len_utf8();
        text.drain(..tail);
    }
}

fn snippet(text: &str, at: usize, length: usize) -> String {
    let chars: Vec<_> = text.chars().collect();
    let gap = |index: usize| chars.get(index..index + 2) == Some(&['\n', '\n']);
    let mut left = at;
    while left > 0 && (left < 2 || !gap(left - 2)) {
        left -= 1;
    }
    let mut right = at + length;
    while right < chars.len() && !gap(right) {
        right += 1;
    }
    let space = |index: usize| {
        chars
            .get(index)
            .is_none_or(|character| character.is_whitespace())
    };
    let mut start = left.max(at.saturating_sub(16));
    if let Some(boundary) = (left..=start)
        .rev()
        .take(12)
        .find(|&back| back == 0 || space(back - 1))
    {
        start = boundary;
    }
    while start > left && start < at && !space(start - 1) {
        start += 1;
    }
    let mut end = right.min(at + length + 80);
    while end < right && end > at + length && !space(end) {
        end -= 1;
    }
    let words: String = chars[start..end]
        .iter()
        .filter(|&&ch| !matches!(ch, '*' | '`' | '#' | '>'))
        .collect();
    format!(
        "{}{}{}",
        if start > left { "…" } else { "" },
        words.split_whitespace().collect::<Vec<_>>().join(" "),
        if end < right { "…" } else { "" }
    )
}

/// Where a conversation is read: the engine's server, or the coding agents.
#[derive(Clone, Copy)]
enum Place<'a> {
    Engine(&'a Lease),
    Agents(&'a dyn crate::AgentSessionSource),
}

impl Place<'_> {
    fn name(self) -> &'static str {
        match self {
            Self::Engine(_) => "engine",
            Self::Agents(_) => "agents",
        }
    }

    /// Its whole transcript, oldest first; none where it can't be read (or, for the coding
    /// agents, isn't kept here yet).
    async fn messages(self, engine: &Engine, id: &str) -> Option<Vec<Value>> {
        match self {
            Self::Engine(lease) => {
                let route = format!("{}/message", sessions::path(id));
                let mut messages = api::Scan::Messages.read(engine, lease, &route).await.ok()?;
                messages.reverse();
                Some(messages)
            }
            Self::Agents(source) => source.transcript(id).await.ok().flatten(),
        }
    }
}

async fn text_of(engine: &Engine, place: Place<'_>, session: &Value) -> Option<Kept> {
    let id = string(session, "id");
    let name = format!("{}.{id}", place.name());
    let _turn = engine.turn(&format!("search:{name}")).await;
    let file = engine.home.join("search").join(format!("{name}.json"));
    let revision = updated(session);
    if let Some(kept) = disk::read::<Kept>(&file).await.ok().flatten()
        && kept.version == 2
        && kept.updated == revision
    {
        return Some(kept);
    }
    let messages = place.messages(engine, id).await?;
    let text = search_text(&messages, session);
    let kept = Kept {
        version: 2,
        updated: revision,
        folded: fold(&text),
        text,
    };
    let _ = disk::durable(file, kept.clone()).await;
    Some(kept)
}

async fn query(
    engine: &Engine,
    places: &[(Place<'_>, Vec<Value>)],
    words: &str,
    limit: usize,
) -> Vec<SearchResult> {
    // Each conversation once: where both list it, the copy changed last (the engine's on a tie:
    // it reads one the coding agents haven't taken in yet).
    let mut newest = BTreeMap::<&str, (usize, u64)>::new();
    for (at, (_, rows)) in places.iter().enumerate() {
        for row in rows {
            let id = string(row, "id");
            if newest.get(id).is_none_or(|(_, seen)| updated(row) > *seen) {
                newest.insert(id, (at, updated(row)));
            }
        }
    }
    let mut conversations = Vec::new();
    let mut known = BTreeSet::new();
    for (at, (place, rows)) in places.iter().enumerate() {
        let rows: BTreeMap<_, _> = rows
            .iter()
            .filter_map(|row| {
                let id = string(row, "id");
                (sessions::valid_id(id) && !hidden(row) && newest.get(id).is_some_and(|(from, _)| *from == at))
                    .then_some((id, row))
            })
            .collect();
        let mut tops: BTreeMap<_, Vec<_>> = rows
            .iter()
            .filter(|(_, row)| string(row, "parentID").is_empty())
            .map(|(id, _)| (*id, vec![]))
            .collect();
        for row in rows
            .values()
            .filter(|row| !string(row, "parentID").is_empty())
        {
            let mut above = *row;
            for _ in 0..100 {
                let parent = string(above, "parentID");
                if parent.is_empty() {
                    if let Some(children) = tops.get_mut(string(above, "id")) {
                        children.push(*row);
                    }
                    break;
                }
                let Some(next) = rows.get(parent) else { break };
                above = next;
            }
        }
        for (id, mut children) in tops {
            children.sort_by_key(|child| std::cmp::Reverse(updated(child)));
            known.insert(format!("{}.{id}", place.name()));
            known.extend(children.iter().map(|child| format!("{}.{}", place.name(), string(child, "id"))));
            conversations.push((*place, rows[id], children));
        }
    }
    conversations.sort_by_key(|(_, top, _)| std::cmp::Reverse(updated(top)));
    let wanted = fold(words);
    let mut results = Vec::new();
    for (place, top, children) in &conversations {
        let id = string(top, "id");
        for row in std::iter::once(*top).chain(children.iter().copied()) {
            let Some(kept) = text_of(engine, *place, row).await else {
                continue;
            };
            let Some(at) = kept.folded.find(&wanted) else {
                continue;
            };
            let at = kept.folded[..at].chars().count();
            results.push(SearchResult {
                session: id.to_owned(),
                info: (*top).clone(),
                snippet: snippet(&kept.text, at, wanted.chars().count()),
                subagent: (string(row, "id") != id).then(|| row.clone()),
                computer: None,
            });
            break;
        }
        if results.len() >= limit {
            break;
        }
    }
    // Only where the list was read: kept text of a conversation gone since goes.
    let listed: Vec<_> = places.iter().map(|(place, _)| place.name()).collect();
    let cache = engine.home.join("search");
    if let Ok(mut files) = tokio::fs::read_dir(&cache).await {
        while let Ok(Some(file)) = files.next_entry().await {
            let name = file.file_name();
            let Some((place, id)) = name
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|name| name.split_once('.'))
            else {
                continue;
            };
            if listed.contains(&place)
                && sessions::valid_id(id)
                && !known.contains(&format!("{place}.{id}"))
            {
                let _ = tokio::fs::remove_file(file.path()).await;
            }
        }
    }
    results
}

fn limit(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| !value.is_nan() && *value != 0.)
        .unwrap_or(20.)
        .clamp(1., 50.) as usize
}

fn missing_words() -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, "Say what to search for (q).")
}

impl Engine {
    pub async fn search_conversations(
        &self,
        words: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, Refusal> {
        let words = words.trim();
        if words.is_empty() {
            return Err(missing_words());
        }
        // The engine's conversations while it runs (it isn't started for a search), and the
        // coding agents'.
        let lease = self.acquire(Demand::Passive).await;
        let mut places = Vec::new();
        if let Some(lease) = &lease {
            let rows = api::Scan::Sessions
                .read(self, lease, "api/session")
                .await
                .map_err(|_| {
                    Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "This computer's engine didn't answer. Search again in a moment.",
                    )
                })?;
            places.push((Place::Engine(lease), rows));
        }
        if let Some(source) = &self.agent_sessions
            && let Ok(rows) = source.inventory().await
        {
            let rows = rows.into_iter().map(|row| Value::Object(row.header)).collect();
            places.push((Place::Agents(source.as_ref()), rows));
        }
        if places.is_empty() {
            return Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer's engine isn't running yet. Search again in a moment.",
            ));
        }
        Ok(query(self, &places, words, limit.clamp(1, 50)).await)
    }

    pub fn register_setup_search(&self, peers: Arc<dyn SearchPeers>, routes: &mut Registry) {
        let engine = self.clone();
        routes.register::<setup::Spec, _, _>(Access::Any, move |call| {
            let engine = engine.clone();
            let peers = peers.clone();
            async move {
                let words = crate::query(&call.request.path, "q").unwrap_or_default();
                let words = words.trim();
                if words.is_empty() || words.encode_utf16().count() > 512 {
                    return Err(missing_words());
                }
                let limit = limit(crate::query(&call.request.path, "limit").as_deref());
                let value = search_setup(&engine, peers.as_ref(), words, limit);
                let result = call
                    .cancel
                    .run_until_cancelled(value)
                    .await
                    .ok_or_else(|| {
                        Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The search was stopped.")
                    })?;
                Ok(RouteReply::json::<200>(sessions::typed(result)?))
            }
        });
    }
}

async fn search_setup(
    engine: &Engine,
    peers: &dyn SearchPeers,
    words: &str,
    limit: usize,
) -> Value {
    let reads = peers.peers().into_iter().map(|peer| async move {
        let results = match peer.availability {
            SearchAvailability::Ready => peers.search(peer.key, words.to_owned(), limit).await,
            unavailable => Err(unavailable),
        };
        (peer.computer, results)
    });
    let (local, remote) = futures_util::future::join(
        engine.search_conversations(words, limit),
        futures_util::future::join_all(reads),
    )
    .await;
    let here = peers.local();
    let mut away = Vec::new();
    let mut unavailable = Vec::new();
    let mut results = Vec::new();
    let mut add = |computer: SearchComputer, matches: Vec<SearchResult>| {
        for mut row in matches {
            row.computer = Some(computer.clone());
            results.push(row);
        }
    };
    match local {
        Ok(found) => add(here, found),
        Err(_) => unavailable.push(here),
    }
    for (computer, found) in remote {
        match found {
            Ok(found) => add(computer, found),
            Err(SearchAvailability::Away) => away.push(computer),
            Err(_) => unavailable.push(computer),
        }
    }
    let sessions = results
        .iter()
        .map(|row| row.session.as_str())
        .collect::<Vec<_>>();
    let moved = peers.moved(&sessions);
    results.retain(|row| {
        moved.get(&row.session).is_none_or(|key| {
            row.computer
                .as_ref()
                .is_some_and(|computer| kuvo_wire::keys::encode(key) == computer.id)
        })
    });
    results.sort_by(|a, b| {
        updated(&b.info)
            .cmp(&updated(&a.info))
            .then_with(|| compare_ids(&a.session, &b.session))
            .then_with(|| {
                let a = a.computer.as_ref().map_or("", |computer| &computer.id);
                let b = b.computer.as_ref().map_or("", |computer| &computer.id);
                compare_ids(a, b)
            })
    });
    let mut seen = BTreeSet::new();
    results.retain(|row| seen.insert(row.session.clone()));
    results.truncate(limit);
    json!({ "results": results, "awayComputers": away, "unavailableComputers": unavailable })
}

// Session IDs and encoded computer keys use ASCII. Their order follows the
// reference's default collation for that alphabet.
fn compare_ids(a: &str, b: &str) -> Ordering {
    let primary = |byte: u8| match byte {
        b'_' => 0,
        b'-' => 1,
        _ => byte.to_ascii_lowercase(),
    };
    a.bytes()
        .map(primary)
        .cmp(b.bytes().map(primary))
        .then_with(|| b.cmp(a))
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let engine = engine.clone();
    routes.register::<route::Spec, _, _>(Access::Any, move |call| {
        let engine = engine.clone();
        async move {
            let words = crate::query(&call.request.path, "q").unwrap_or_default();
            let limit = limit(crate::query(&call.request.path, "limit").as_deref());
            let found = engine.search_conversations(&words, limit).await?;
            Ok(RouteReply::json::<200>(sessions::typed(
                json!({ "results": found }),
            )?))
        }
    });
}
