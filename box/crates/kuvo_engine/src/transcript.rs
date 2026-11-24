//! The bounded transcript read used by local agents and by peer letters.

use std::{
    collections::{BTreeSet, VecDeque},
    fmt,
    time::Duration,
};

use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::{
    Engine, api,
    lifetime::{Demand, Lease},
    parts, sessions,
};

#[derive(Clone, Copy)]
pub enum TranscriptScope {
    Local,
    Peer,
}

impl TranscriptScope {
    fn limit(self) -> usize {
        match self {
            Self::Local => 24_000,
            Self::Peer => 20_000,
        }
    }
}

#[derive(Debug)]
pub enum TranscriptError {
    Cancelled,
    Gone,
    Refused(String),
    Unavailable(String),
}

impl fmt::Display for TranscriptError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => out.write_str("The transcript read was cancelled."),
            Self::Gone => out.write_str("That conversation isn't here."),
            Self::Refused(message) | Self::Unavailable(message) => out.write_str(message),
        }
    }
}

impl std::error::Error for TranscriptError {}

#[derive(Deserialize)]
struct Page {
    data: Vec<Message>,
    cursor: Option<Cursor>,
}

#[derive(Deserialize)]
struct Cursor {
    next: Option<String>,
    previous: Option<String>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    User,
    Assistant,
    Shell,
    Idle,
    Compaction,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Message {
    id: String,
    #[serde(rename = "type")]
    kind: Kind,
    text: Option<String>,
    status: Option<String>,
    reason: Option<String>,
    outcome: Option<String>,
    #[serde(default)]
    files: Vec<File>,
    #[serde(default)]
    content: Vec<Part>,
    command: Option<String>,
    output: Option<ShellOutput>,
    error: Option<Failure>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Deserialize)]
struct File {
    name: Option<String>,
}

#[derive(Deserialize)]
struct ShellOutput {
    output: Option<String>,
}

#[derive(Deserialize)]
struct Failure {
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Part {
    Text {
        text: Option<String>,
    },
    Tool {
        name: Option<String>,
        state: Option<ToolState>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ToolState {
    #[serde(default)]
    input: Value,
}

impl Engine {
    /// Reads the native conversation after the caller has resolved its current owner.
    pub async fn transcript(
        &self,
        session: &str,
        title: &str,
        query: Option<&str>,
        scope: TranscriptScope,
        cancel: &CancellationToken,
    ) -> Result<String, TranscriptError> {
        cancel
            .run_until_cancelled(async {
                if !sessions::valid_id(session) {
                    return Err(TranscriptError::Gone);
                }
                let lease = self.acquire(Demand::Passive).await.ok_or_else(|| {
                    TranscriptError::Unavailable(
                        "Its engine or coding agents are restarting or aren't running.".into(),
                    )
                })?;
                let path = sessions::path(session);
                let session: Value =
                    self.call(&lease, Method::GET, &path, None)
                        .await
                        .map_err(|error| {
                            if error.missing() {
                                TranscriptError::Gone
                            } else {
                                read_error(error)
                            }
                        })?;
                let revert = session.pointer("/revert/messageID").and_then(Value::as_str);
                let route = format!("{path}/message?limit=200");
                let latest = page(self, &lease, &format!("{route}&order=desc")).await?;
                let mut cursor = latest.cursor;
                let mut messages: VecDeque<_> = latest.data.into_iter().rev().collect();
                let mut ids = messages
                    .iter()
                    .map(|message| message.id.clone())
                    .collect::<BTreeSet<_>>();
                let mut partial = None;
                for _ in 0..100 {
                    if messages.is_empty() {
                        break;
                    }
                    let Some(after) = cursor
                        .take()
                        .and_then(|cursor| cursor.next.or(cursor.previous))
                    else {
                        break;
                    };
                    let older = match page(
                        self,
                        &lease,
                        &format!("{route}&cursor={}", api::segment(&after)),
                    )
                    .await
                    {
                        Ok(older) => older,
                        Err(error) if revert.is_some_and(|id| !ids.contains(id)) => {
                            return Err(error);
                        }
                        Err(error) => {
                            partial = Some(error);
                            break;
                        }
                    };
                    if older
                        .data
                        .first()
                        .is_none_or(|message| ids.contains(&message.id))
                    {
                        break;
                    }
                    for message in older.data {
                        ids.insert(message.id.clone());
                        messages.push_front(message);
                    }
                    cursor = older.cursor;
                }
                if let Some(before) =
                    revert.and_then(|id| messages.iter().position(|message| message.id == id))
                {
                    messages.truncate(before);
                }
                let text = markdown(title, messages.iter());
                let text = excerpt(&text, query, scope.limit(), partial.is_some());
                Ok(match partial {
                    Some(error) => {
                        format!("Some older messages couldn't be read: {error}\n\n{text}")
                    }
                    None => text,
                })
            })
            .await
            .ok_or(TranscriptError::Cancelled)?
    }
}

async fn page(engine: &Engine, lease: &Lease, route: &str) -> Result<Page, TranscriptError> {
    let read = |wait| async move {
        let answer = engine
            .call_raw(lease, Method::GET, route, None, Duration::from_secs(wait))
            .await
            .map_err(read_error)?;
        serde_json::from_value(answer).map_err(|_| {
            TranscriptError::Unavailable("Its engine didn't return the messages.".into())
        })
    };
    match read(10).await {
        reply @ (Ok(_) | Err(TranscriptError::Refused(_))) => reply,
        Err(_) => {
            tokio::time::sleep(Duration::from_millis(200)).await;
            read(20).await
        }
    }
}

fn read_error(error: api::Error) -> TranscriptError {
    let words = |status: StatusCode, message: String| {
        if message == format!("It answered {}.", status.as_u16()) {
            format!(
                "Its engine couldn't return the messages (HTTP {}).",
                status.as_u16()
            )
        } else {
            message
        }
    };
    match error {
        api::Error::Status(status, message)
            if status.is_client_error()
                && !matches!(
                    status,
                    StatusCode::NOT_FOUND | StatusCode::TOO_MANY_REQUESTS
                ) =>
        {
            TranscriptError::Refused(words(status, message))
        }
        api::Error::Status(status, message) => TranscriptError::Unavailable(words(status, message)),
        api::Error::Transport(ref error) if error.is_timeout() => TranscriptError::Unavailable(
            "Its engine or coding agents didn't answer in time.".into(),
        ),
        other if other.connection_refused() => TranscriptError::Unavailable(
            "Its engine or coding agents are restarting or aren't running.".into(),
        ),
        other => TranscriptError::Unavailable(other.to_string()),
    }
}

struct Markdown<'a> {
    agent: &'a str,
    speaker: String,
    blocks: Vec<String>,
    steps: Vec<String>,
}

impl Markdown<'_> {
    fn block(&mut self, speaker: &str, words: String) {
        if self.speaker != speaker {
            self.blocks.push(format!("**{speaker}**"));
            self.speaker = speaker.to_owned();
        }
        if !words.is_empty() {
            self.blocks.push(words);
        }
    }

    fn flush_steps(&mut self) {
        if !self.steps.is_empty() {
            let steps = std::mem::take(&mut self.steps).join("\n");
            self.block(self.agent, steps);
        }
    }

    fn say(&mut self, speaker: &str, words: String) {
        self.flush_steps();
        self.block(speaker, words);
    }
}

fn markdown<'a>(title: &str, messages: impl Iterator<Item = &'a Message>) -> String {
    let mut output = Markdown {
        agent: title,
        speaker: String::new(),
        blocks: vec![format!("# {title}")],
        steps: Vec::new(),
    };
    let mut manual_summary_failed = false;
    for message in messages {
        match message.kind {
            Kind::User => user_message(&mut output, message),
            Kind::Assistant => {
                for part in &message.content {
                    match part {
                        Part::Text { text: Some(words) } if !words.trim().is_empty() => {
                            output.say(title, words.trim().to_owned())
                        }
                        Part::Tool { name, state } => {
                            let name = name.as_deref().unwrap_or_default();
                            let tool = name.strip_prefix("kuvo-browser_").unwrap_or(name);
                            let answer = state
                                .as_ref()
                                .and_then(|state| {
                                    [
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
                                    .iter()
                                    .find_map(|key| {
                                        state.input.get(key).filter(|value| !value.is_null())
                                    })
                                })
                                .and_then(Value::as_str)
                                .filter(|input| !input.is_empty());
                            output.steps.push(match answer {
                                Some(input) => format!(
                                    "- {tool}: {}",
                                    take(input.split('\n').next().unwrap_or_default(), 120)
                                ),
                                None => format!("- {tool}"),
                            });
                        }
                        _ => {}
                    }
                }
            }
            Kind::Shell => {
                let command = message.command.as_deref().unwrap_or("undefined");
                let mut text = format!("Ran `{command}`");
                if let Some(shell) = message
                    .output
                    .as_ref()
                    .and_then(|shell| shell.output.as_deref())
                    .map(str::trim)
                    .filter(|shell| !shell.is_empty())
                {
                    text.push_str(&format!("\n\n```\n{shell}\n```"));
                }
                output.say("User", text);
            }
            Kind::Idle
                if message.outcome.as_deref() == Some("failed") && !manual_summary_failed =>
            {
                let why = message
                    .error
                    .as_ref()
                    .and_then(|failure| failure.message.as_deref())
                    .map(str::trim)
                    .filter(|why| !why.is_empty())
                    .unwrap_or("the engine gave no reason.");
                output.say("Stopped", format!("It stopped with an error: {why}"));
            }
            Kind::Compaction => match message.status.as_deref() {
                Some("completed") => {
                    output.say(title, "_Summarized the conversation so far._".into())
                }
                Some("failed") => {
                    output.say(title, "_Couldn't summarize the conversation._".into())
                }
                _ => {}
            },
            _ => {}
        }
        manual_summary_failed = message.kind == Kind::Compaction
            && message.reason.as_deref() == Some("manual")
            && message.status.as_deref() == Some("failed");
    }
    output.flush_steps();
    output.blocks.join("\n\n") + "\n"
}

fn user_message(output: &mut Markdown<'_>, message: &Message) {
    let written = message.text.as_deref().unwrap_or_default();
    let note = message
        .metadata
        .pointer("/kuvo/note")
        .filter(|note| note.is_object());
    let marked = note.and_then(|note| note["line"].as_str().and(note["from"].as_str()));
    let from = marked.or_else(|| {
        if written.starts_with("[The user answered] ") {
            return Some("Your answer");
        }
        if ["finished", "still working", "failed", "asks", "stopped"]
            .iter()
            .any(|kind| {
                written
                    .strip_prefix(&format!("[Session {kind}] \""))
                    .is_some_and(|title| {
                        title
                            .split('\n')
                            .next()
                            .is_some_and(|line| line.contains('"'))
                    })
            })
        {
            return Some("Its sessions");
        }
        note.map(|_| "Kuvo Code")
    });
    if let Some(from) = from {
        output.say(from, written.trim().to_owned());
        return;
    }
    let parsed = parts::read_parts(written);
    let kept = message
        .metadata
        .pointer("/kuvo/parts")
        .filter(|parts| parts["words"].is_string());
    let (words, mail) = kept.map_or_else(
        || (parsed.words, parsed.mail.as_ref().map(|mail| mail.name)),
        |parts| {
            (
                parts["words"].as_str().unwrap_or_default(),
                parts.pointer("/mail/name").and_then(Value::as_str),
            )
        },
    );
    if let Some(from) = mail {
        output.say(&format!("Message from {from}"), words.to_owned());
    } else if let Some(wake) = written.strip_prefix("[Wake-up] ") {
        output.say("Wake-up", wake.to_owned());
    } else if let Some((from, body)) = work_note(written) {
        output.say(&format!("Work from {from}"), body.trim().to_owned());
    } else {
        let files: Vec<_> = message
            .files
            .iter()
            .filter_map(|file| file.name.as_deref())
            .filter(|file| !file.is_empty())
            .collect();
        let mut text = words.trim().to_owned();
        if !files.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str("Attached: ");
            text.push_str(&files.join(", "));
        }
        if !text.is_empty() {
            output.say("User", text);
        }
    }
}

fn work_note(text: &str) -> Option<(&str, &str)> {
    let (header, body) = text.strip_prefix("[Work from ")?.split_once("]\n\n")?;
    let (name, session) = header.rsplit_once(" · ")?;
    if session.is_empty() || session.contains([']', '\n']) || name.contains('\n') {
        return None;
    }
    Some((
        name,
        body.split_once("\n\n---\n")
            .map_or(body, |(words, _)| words),
    ))
}

fn length(text: &str) -> usize {
    text.encode_utf16().count()
}

fn take(text: &str, count: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(count).collect::<Vec<_>>())
}

fn folded(text: &str) -> String {
    text.nfd()
        .filter(|ch| !is_combining_mark(*ch))
        .collect::<String>()
        .to_lowercase()
}

fn excerpt(markdown: &str, query: Option<&str>, limit: usize, partial: bool) -> String {
    let paragraphs: Vec<_> = markdown.split("\n\n").collect();
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        let wanted = folded(query);
        let hits: Vec<_> = paragraphs
            .iter()
            .enumerate()
            .filter_map(|(at, paragraph)| folded(paragraph).contains(&wanted).then_some(at))
            .collect();
        if hits.is_empty() {
            let what = if partial {
                "None of the available messages mention"
            } else {
                "Nothing in it mentions"
            };
            return format!("{what} \"{query}\". Read it without query for its latest part.");
        }
        let mut used = 0;
        let mut shown = BTreeSet::new();
        for &hit in hits.iter().rev() {
            let around: Vec<_> = (hit.saturating_sub(1)..=(hit + 1).min(paragraphs.len() - 1))
                .filter(|at| !shown.contains(at))
                .collect();
            let size = around
                .iter()
                .map(|&at| length(paragraphs[at]) + 2)
                .sum::<usize>();
            if used + size > limit && !shown.is_empty() {
                break;
            }
            used += size;
            shown.extend(around);
        }
        let skipped = if shown.len() < paragraphs.len() {
            " (the rest left out as […])"
        } else {
            ""
        };
        let mut last = None;
        let mut found = Vec::new();
        for at in shown {
            if last.is_some_and(|previous| at > previous + 1) {
                found.push("[…]".to_owned());
            }
            found.push(take(paragraphs[at], limit));
            last = Some(at);
        }
        let how_many = if hits.len() == 1 {
            "1 place mentions".into()
        } else {
            format!("{} places mention", hits.len())
        };
        return format!("{how_many} \"{query}\"{skipped}:\n\n{}", found.join("\n\n"));
    }
    if length(markdown) <= limit {
        return markdown.to_owned();
    }
    let head = paragraphs
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n");
    let head = if length(&head) > limit / 4 {
        format!("{}…", take(&head, limit / 4))
    } else {
        head
    };
    let mut used = length(&head);
    let mut tail = VecDeque::new();
    for paragraph in paragraphs.into_iter().skip(3).rev() {
        let size = length(paragraph);
        if used + size + 2 > limit {
            if tail.is_empty() {
                let end: Vec<_> = paragraph
                    .encode_utf16()
                    .skip(size.saturating_sub(limit.saturating_sub(used)))
                    .collect();
                tail.push_front(format!("…{}", String::from_utf16_lossy(&end)));
            }
            break;
        }
        used += size + 2;
        tail.push_front(paragraph.to_owned());
    }
    let tail = tail.into_iter().collect::<Vec<_>>().join("\n\n");
    format!("{head}\n\n[… the middle is left out; read it with query, to find a part …]\n\n{tail}")
}
