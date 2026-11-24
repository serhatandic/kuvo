//! Read an engine transcript through the resident's normal dispatcher.
use std::{collections::HashSet, time::Duration};

use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use kuvo_server::{Caller, DispatchRequest, Dispatcher, LocalOrigin, Transport};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const LIMIT: usize = 20_000;
const MAX_PAGE_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct Error {
    pub message: String,
    pub retry: bool,
}

#[derive(Deserialize)]
struct Page {
    data: Vec<Value>,
    cursor: Option<Cursor>,
}

#[derive(Deserialize)]
struct Cursor {
    next: Option<String>,
    previous: Option<String>,
}

pub(super) async fn read(
    dispatcher: &Dispatcher,
    session: &str,
    info: &Value,
    query: Option<&str>,
) -> Result<String, Error> {
    let root = format!("api/session/{session}/message?limit=200");
    let mut page = read_page(dispatcher, &format!("{root}&order=desc")).await?;
    let mut messages = Vec::new();
    let mut ids = HashSet::new();
    let mut missing = None;
    let undo = info.pointer("/revert/messageID").and_then(Value::as_str);
    for pages in 0..=100 {
        if page.data.is_empty()
            || page.data[0]["id"]
                .as_str()
                .is_some_and(|id| ids.contains(id))
        {
            break;
        }
        ids.extend(
            page.data
                .iter()
                .filter_map(|message| message["id"].as_str().map(str::to_owned)),
        );
        messages.extend(page.data);
        if pages == 100 {
            break;
        }
        let Some(cursor) = page
            .cursor
            .and_then(|cursor| cursor.next.or(cursor.previous))
        else {
            break;
        };
        let cursor = utf8_percent_encode(&cursor, NON_ALPHANUMERIC);
        match read_page(dispatcher, &format!("{root}&cursor={cursor}")).await {
            Ok(older) => page = older,
            Err(error) => {
                if undo.is_some_and(|id| !messages.iter().any(|message| message["id"] == id)) {
                    return Err(error);
                }
                missing = Some(error.message);
                break;
            }
        }
    }
    messages.reverse();
    if let Some(undo) = undo
        && let Some(at) = messages.iter().position(|message| message["id"] == undo)
    {
        messages.truncate(at);
    }
    let title = info["title"]
        .as_str()
        .filter(|title| !title.is_empty())
        .unwrap_or("New session");
    let words = excerpt(
        &markdown(dispatcher, &messages, title).await?,
        query,
        missing.is_some(),
    );
    Ok(match missing {
        Some(error) => format!("Some older messages couldn't be read: {error}\n\n{words}"),
        None => words,
    })
}

async fn read_page(dispatcher: &Dispatcher, path: &str) -> Result<Page, Error> {
    match request(dispatcher, Method::GET, path, None, Duration::from_secs(10)).await {
        Err(error) if error.retry => {
            tokio::time::sleep(Duration::from_millis(200)).await;
            request(dispatcher, Method::GET, path, None, Duration::from_secs(20)).await
        }
        answer => answer,
    }
}

async fn request<T: DeserializeOwned>(
    dispatcher: &Dispatcher,
    method: Method,
    path: &str,
    body: Option<Value>,
    wait: Duration,
) -> Result<T, Error> {
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let request = DispatchRequest {
        method,
        path: path.to_owned(),
        headers: HeaderMap::new(),
        body: body
            .map_or_else(Vec::new, |body| body.to_string().into_bytes())
            .into(),
        caller: Caller::Local(Transport::Http {
            origin: LocalOrigin::Direct,
            internal: true,
        }),
    };
    tokio::time::timeout(wait, async {
        let reply = dispatcher.dispatch(request, cancel).await;
        let status = reply.status;
        let bytes = Limited::new(reply.body, MAX_PAGE_BYTES)
            .collect()
            .await
            .map_err(|e| Error {
                message: e.to_string(),
                retry: true,
            })?
            .to_bytes();
        if !status.is_success() {
            let payload: Value = serde_json::from_slice(&bytes).unwrap_or_default();
            return Err(Error {
                message: payload["message"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "Its engine couldn't return the messages (HTTP {}).",
                            status.as_u16()
                        )
                    }),
                retry: status.is_server_error()
                    || matches!(
                        status,
                        StatusCode::NOT_FOUND | StatusCode::TOO_MANY_REQUESTS
                    ),
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| Error {
            message: "Its engine didn't return the messages.".into(),
            retry: true,
        })
    })
    .await
    .unwrap_or_else(|_| {
        Err(Error {
            message: "Its engine didn't answer in time.".into(),
            retry: true,
        })
    })
}

struct Transcript<'a> {
    blocks: Vec<String>,
    speaker: String,
    steps: Vec<String>,
    agent: &'a str,
}

impl Transcript<'_> {
    fn flush(&mut self) {
        if !self.steps.is_empty() {
            let list = std::mem::take(&mut self.steps).join("\n");
            self.say(self.agent, list);
        }
    }

    fn say(&mut self, who: &str, words: String) {
        self.flush();
        if self.speaker != who {
            self.blocks.push(format!("**{who}**"));
            self.speaker = who.into();
        }
        self.blocks.push(words);
    }
}

async fn markdown(
    dispatcher: &Dispatcher,
    messages: &[Value],
    title: &str,
) -> Result<String, Error> {
    let mut text = Transcript {
        blocks: vec![format!("# {title}")],
        speaker: String::new(),
        steps: vec![],
        agent: title,
    };
    for (page_index, page) in messages.chunks(200).enumerate() {
        let parse: Value = request(
            dispatcher, Method::POST, "kuvo/messages/parts",
            Some(json!({
                "texts": page.iter().map(|message| message["text"].as_str().unwrap_or("")).collect::<Vec<_>>()
            })),
            Duration::from_secs(15),
        ).await?;
        for (index, message) in page.iter().enumerate() {
            match message["type"].as_str() {
                Some("user") => {
                    let written = message["text"].as_str().unwrap_or_default();
                    let parts = message
                        .pointer("/metadata/kuvo/parts")
                        .unwrap_or(&parse["parts"][index]);
                    let words = parts["words"].as_str().unwrap_or(written);
                    if let Some(from) = note_speaker(message, written) {
                        text.say(from, written.trim().into());
                    } else if let Some(name) = parts.pointer("/mail/name").and_then(Value::as_str) {
                        text.say(&format!("Message from {name}"), words.into());
                    } else if let Some(wake) = written.strip_prefix("[Wake-up] ") {
                        text.say("Wake-up", wake.into());
                    } else if let Some((from, body)) = written
                        .strip_prefix("[Work from ")
                        .and_then(|work| work.split_once("]\n\n"))
                        .and_then(|(heading, body)| {
                            heading.rsplit_once(" · ").map(|(from, _)| (from, body))
                        })
                    {
                        text.say(
                            &format!("Work from {from}"),
                            body.split("\n\n---\n").next().unwrap_or(body).trim().into(),
                        );
                    } else {
                        let mut blocks = vec![words.trim().to_owned()];
                        let files: Vec<_> = message["files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|file| {
                                file["name"].as_str().filter(|name| !name.is_empty())
                            })
                            .collect();
                        if !files.is_empty() {
                            blocks.push(format!("Attached: {}", files.join(", ")));
                        }
                        blocks.retain(|block| !block.is_empty());
                        if !blocks.is_empty() {
                            text.say("User", blocks.join("\n\n"));
                        }
                    }
                }
                Some("assistant") => {
                    for part in message["content"].as_array().into_iter().flatten() {
                        match part["type"].as_str() {
                            Some("text") => {
                                if let Some(words) = part["text"]
                                    .as_str()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                {
                                    text.say(title, words.into());
                                }
                            }
                            Some("tool") => {
                                let name = part["name"].as_str().unwrap_or_default();
                                let name = name.strip_prefix("kuvo-browser_").unwrap_or(name);
                                let input = &part["state"]["input"];
                                let detail = [
                                    "description",
                                    "filePath",
                                    "path",
                                    "command",
                                    "url",
                                    "pattern",
                                    "query",
                                    "to",
                                    "of",
                                ]
                                .into_iter()
                                .find_map(|field| {
                                    (!input[field].is_null()).then_some(&input[field])
                                })
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty());
                                let line = detail.map_or_else(
                                    || format!("- {name}"),
                                    |detail| {
                                        format!(
                                            "- {name}: {}",
                                            prefix(
                                                detail.split('\n').next().unwrap_or_default(),
                                                120
                                            )
                                        )
                                    },
                                );
                                text.steps.push(line);
                            }
                            _ => {}
                        }
                    }
                }
                Some("shell") => {
                    let command = message["command"].as_str().unwrap_or("undefined");
                    let output = message["output"]["output"]
                        .as_str()
                        .unwrap_or_default()
                        .trim();
                    let mut words = format!("Ran `{command}`");
                    if !output.is_empty() {
                        words.push_str(&format!("\n\n```\n{output}\n```"));
                    }
                    text.say("User", words);
                }
                Some("idle") if message["outcome"] == "failed" => {
                    let previous = (page_index * 200 + index)
                        .checked_sub(1)
                        .and_then(|at| messages.get(at));
                    if previous.is_some_and(|before| {
                        before["type"] == "compaction"
                            && before["status"] == "failed"
                            && before["reason"] == "manual"
                    }) {
                        continue;
                    }
                    let error = message["error"]["message"]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("the engine gave no reason.");
                    text.say("Stopped", format!("It stopped with an error: {error}"));
                }
                Some("compaction") if message["status"] == "completed" => {
                    text.say(title, "_Summarized the conversation so far._".into())
                }
                Some("compaction") if message["status"] == "failed" => {
                    text.say(title, "_Couldn't summarize the conversation._".into())
                }
                _ => {}
            }
        }
    }
    text.flush();
    Ok(text.blocks.join("\n\n") + "\n")
}

fn note_speaker<'a>(message: &'a Value, written: &str) -> Option<&'a str> {
    let note = message
        .pointer("/metadata/kuvo/note")
        .and_then(Value::as_object);
    if let Some(note) = note
        && let Some(from) = note.get("from").and_then(Value::as_str)
        && note.get("line").and_then(Value::as_str).is_some()
    {
        return Some(from);
    }
    if written.starts_with("[The user answered] ") {
        return Some("Your answer");
    }
    let news = written
        .strip_prefix("[Session ")
        .and_then(|tail| tail.split_once("] \""));
    if let Some((action, rest)) = news
        && matches!(
            action,
            "finished" | "still working" | "failed" | "asks" | "stopped"
        )
        && rest
            .split_once('"')
            .is_some_and(|(title, _)| !title.contains('\n'))
    {
        return Some("Its sessions");
    }
    note.map(|_| "Kuvo Code")
}

fn prefix(text: &str, count: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(count).collect::<Vec<_>>())
}

fn width(text: &str) -> usize {
    text.encode_utf16().count()
}

fn folded(text: &str) -> String {
    text.nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

fn excerpt(markdown: &str, query: Option<&str>, partial: bool) -> String {
    let paragraphs: Vec<_> = markdown.split("\n\n").collect();
    if let Some(query) = query {
        let wanted = folded(query);
        let hits: Vec<_> = paragraphs
            .iter()
            .enumerate()
            .filter_map(|(i, p)| folded(p).contains(&wanted).then_some(i))
            .collect();
        if hits.is_empty() {
            let intro = if partial {
                "None of the available messages mention"
            } else {
                "Nothing in it mentions"
            };
            return format!("{intro} \"{query}\". Read it without query for its latest part.");
        }
        let mut shown = std::collections::BTreeSet::new();
        let mut used = 0;
        for hit in hits.iter().rev().copied() {
            let around: Vec<_> = (hit.saturating_sub(1)..=(hit + 1).min(paragraphs.len() - 1))
                .filter(|i| !shown.contains(i))
                .collect();
            let size = around
                .iter()
                .map(|&i| width(paragraphs[i]) + 2)
                .sum::<usize>();
            if used + size > LIMIT && !shown.is_empty() {
                break;
            }
            shown.extend(around);
            used += size;
        }
        let message = if hits.len() == 1 {
            "1 place mentions".into()
        } else {
            format!("{} places mention", hits.len())
        };
        let rest = if shown.len() < paragraphs.len() {
            " (the rest left out as […])"
        } else {
            ""
        };
        let mut text = Vec::new();
        let mut last = None;
        for index in shown {
            if last.is_some_and(|before| index > before + 1) {
                text.push("[…]".into());
            }
            text.push(prefix(paragraphs[index], LIMIT));
            last = Some(index);
        }
        return format!("{message} \"{query}\"{rest}:\n\n{}", text.join("\n\n"));
    }
    if width(markdown) <= LIMIT {
        return markdown.into();
    }
    let mut head = paragraphs
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n");
    if width(&head) > LIMIT / 4 {
        head = prefix(&head, LIMIT / 4) + "…";
    }
    let mut used = width(&head);
    let mut tail = vec![];
    for paragraph in paragraphs.iter().skip(3).rev() {
        let size = width(paragraph);
        if used + size + 2 > LIMIT {
            if tail.is_empty() {
                let last = paragraph
                    .encode_utf16()
                    .skip(size.saturating_sub(LIMIT - used))
                    .collect::<Vec<_>>();
                tail.push("…".to_owned() + &String::from_utf16_lossy(&last));
            }
            break;
        }
        tail.push((*paragraph).to_owned());
        used += size + 2;
    }
    tail.reverse();
    format!(
        "{head}\n\n[… the middle is left out; read it with query, to find a part …]\n\n{}",
        tail.join("\n\n")
    )
}
