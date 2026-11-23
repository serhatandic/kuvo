//! Search the local OpenCode transcript, with a cache keyed by the session revision.

use std::collections::{BTreeMap, BTreeSet};

use http::StatusCode;
use kuvo_protocol::routes::get_kuvo_search as route;
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

async fn text_of(engine: &Engine, lease: &Lease, session: &Value) -> Option<Kept> {
    let id = string(session, "id");
    let _turn = engine.turn(&format!("search:{id}")).await;
    let file = engine.home.join("search").join(format!("engine.{id}.json"));
    let revision = updated(session);
    if let Some(kept) = disk::read::<Kept>(&file).await.ok().flatten()
        && kept.version == 2
        && kept.updated == revision
    {
        return Some(kept);
    }
    let route = format!("{}/message", sessions::path(id));
    let mut messages = api::Scan::Messages.read(engine, lease, &route).await.ok()?;
    messages.reverse();
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
    lease: &Lease,
    words: &str,
    limit: usize,
) -> Result<Value, Refusal> {
    let rows = api::Scan::Sessions
        .read(engine, lease, "api/session")
        .await
        .map_err(|_| {
            Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer's engine didn't answer. Search again in a moment.",
            )
        })?;
    let rows: BTreeMap<_, _> = rows
        .iter()
        .filter_map(|row| {
            let id = string(row, "id");
            (sessions::valid_id(id) && !hidden(row)).then_some((id, row))
        })
        .collect();
    let mut conversations: BTreeMap<_, Vec<_>> = rows
        .iter()
        .filter(|(_, row)| string(row, "parentID").is_empty())
        .map(|(id, _)| (*id, vec![]))
        .collect();
    for row in rows
        .values()
        .filter(|row| !string(row, "parentID").is_empty())
    {
        let mut at = *row;
        for _ in 0..100 {
            let parent = string(at, "parentID");
            if parent.is_empty() {
                if let Some(children) = conversations.get_mut(string(at, "id")) {
                    children.push(*row);
                }
                break;
            }
            let Some(above) = rows.get(parent) else { break };
            at = above;
        }
    }
    let mut conversations: Vec<_> = conversations.into_iter().collect();
    conversations.sort_by_key(|(id, _)| std::cmp::Reverse(updated(rows[id])));
    let wanted = fold(words);
    let mut results = Vec::new();
    let known: BTreeSet<_> = conversations
        .iter()
        .flat_map(|(id, children)| {
            std::iter::once(*id).chain(children.iter().map(|child| string(child, "id")))
        })
        .collect();
    for (id, children) in &mut conversations {
        children.sort_by_key(|child| std::cmp::Reverse(updated(child)));
        let top = rows[*id];
        for row in std::iter::once(top).chain(children.iter().copied()) {
            let Some(kept) = text_of(engine, lease, row).await else {
                continue;
            };
            let Some(at) = kept.folded.find(&wanted) else {
                continue;
            };
            let at = kept.folded[..at].chars().count();
            let text = snippet(&kept.text, at, wanted.chars().count());
            let mut found = json!({ "session": id, "info": top, "snippet": text });
            if string(row, "id") != *id {
                found["subagent"] = (*row).clone();
            }
            results.push(found);
            break;
        }
        if results.len() >= limit {
            break;
        }
    }
    let cache = engine.home.join("search");
    if let Ok(mut files) = tokio::fs::read_dir(&cache).await {
        while let Ok(Some(file)) = files.next_entry().await {
            if let Some(id) = file
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("engine."))
                .and_then(|name| name.strip_suffix(".json"))
                && sessions::valid_id(id)
                && !known.contains(id)
            {
                let _ = tokio::fs::remove_file(file.path()).await;
            }
        }
    }
    Ok(json!({ "results": results }))
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let engine = engine.clone();
    routes.register::<route::Spec, _, _>(Access::Any, move |call| {
        let engine = engine.clone();
        async move {
            let words = crate::query(&call.request.path, "q").unwrap_or_default();
            let words = words.trim();
            if words.is_empty() {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Say what to search for (q).",
                ));
            }
            let limit = crate::query(&call.request.path, "limit")
                .and_then(|limit| limit.parse::<f64>().ok())
                .filter(|limit| !limit.is_nan() && *limit != 0.)
                .unwrap_or(20.)
                .clamp(1., 50.) as usize;
            let lease = engine.acquire(Demand::Passive).await.ok_or_else(|| {
                Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "This computer's engine isn't running yet. Search again in a moment.",
                )
            })?;
            Ok(RouteReply::json::<200>(sessions::typed(
                query(&engine, &lease, words, limit).await?,
            )?))
        }
    });
}
