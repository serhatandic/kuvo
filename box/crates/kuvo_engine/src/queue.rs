//! Local queue state and its ordering across the resident and OpenCode inbox.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::{BTreeMap, HashSet},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http::{Method, StatusCode};
use kuvo_protocol::routes::{
    delete_kuvo_sessions_id_queue_item as remove, get_kuvo_sessions_id_queue as read,
    get_kuvo_sessions_id_queue_events as events, get_kuvo_sessions_queues_events as overview,
    post_kuvo_sessions_id_queue_hold as hold, post_kuvo_sessions_id_queue_item_ack as ack,
    post_kuvo_sessions_id_queue_item_now as now, post_kuvo_sessions_id_queue_item_steer as steer,
    post_kuvo_sessions_id_queue_reorder as reorder, post_kuvo_sessions_id_queue_resume as resume,
    post_kuvo_sessions_id_stop as stop,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteCall, RouteReply};
use kuvo_store::format;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, watch};

use crate::{
    Engine, MoveEngineError, api, disk,
    lifetime::{Demand, Lease},
    parts, send, sessions,
};

const ARRIVING_FOR: u64 = 120_000;

mod handoff;
pub(crate) mod limit;
mod target;
mod views;

pub(crate) use target::{Place, Target};
#[cfg(windows)]
mod windows_socket;

#[derive(Clone)]
pub(crate) struct QueueStore {
    run: PathBuf,
    /// The coding agents, whose lines are told what a conversation's queue holds.
    pub(crate) agents: Arc<std::sync::OnceLock<Arc<dyn crate::AgentSessionSource>>>,
    edits: Arc<Semaphore>,
    changes: watch::Sender<u64>,
    limits: limit::Classifier,
    views: views::Views,
}

type Index = BTreeMap<String, Queue>;

#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Queue {
    paused: bool,
    held: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    waiting: Vec<Waiting>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    order: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    came_held: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    arriving: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resumed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resumed_limit_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit_stop: Option<LimitStop>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inbox_moved: Vec<String>,
    format_version: u64,
    #[serde(flatten)]
    handoffs: handoff::Handoffs,
    #[serde(flatten)]
    unknown: BTreeMap<String, Value>,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
struct LimitStop {
    message: String,
    #[serde(rename = "resetsAt", skip_serializing_if = "Option::is_none")]
    resets_at: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    releasing: bool,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
struct Waiting {
    id: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
    #[serde(default)]
    at: u64,
}

#[derive(Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Delivery {
    Steer,
    #[default]
    Queue,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Prompt {
    pub id: String,
    pub text: String,
    pub delivery: Delivery,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct Inbox {
    pub(crate) id: String,
    delivery: Delivery,
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    text: Option<String>,
    #[serde(default)]
    files: Vec<Value>,
    metadata: Option<Value>,
}

impl Inbox {
    fn text(&self) -> Option<&str> {
        self.payload
            .as_ref()
            .and_then(|payload| payload.text.as_deref())
            .or(self.text.as_deref())
    }

    pub(crate) fn text_is(&self, text: &str) -> bool {
        self.text() == Some(text)
    }

    fn is_message(&self) -> bool {
        self.kind.as_deref() != Some("compaction")
    }

    fn metadata(&self) -> Option<&Value> {
        self.payload.as_ref()?.metadata.as_ref()
    }

    fn files(&self) -> &[Value] {
        self.payload.as_ref().map_or(&[], |payload| &payload.files)
    }

    fn shown(&self) -> (String, Option<String>) {
        let text = self.text().unwrap_or_default();
        if let Some(note) = self.metadata().and_then(|data| data.pointer("/kuvo/note"))
            && let (Some(from), Some(line)) = (note["from"].as_str(), note["line"].as_str())
        {
            return (line.to_owned(), Some(from.to_owned()));
        }
        if let Some(parts) = self.metadata().and_then(|data| data.pointer("/kuvo/parts"))
            && let Some(words) = parts["words"].as_str()
        {
            return (
                words.to_owned(),
                parts
                    .pointer("/mail/name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            );
        }
        let parts = parts::read_parts(text);
        (
            parts.words.to_owned(),
            parts.mail.map(|mail| mail.name.to_owned()),
        )
    }
}

impl QueueStore {
    pub(crate) fn open(home: &std::path::Path) -> Self {
        Self {
            run: home.join("run"),
            agents: Arc::default(),
            edits: Arc::new(Semaphore::new(1)),
            changes: watch::channel(0).0,
            limits: limit::Classifier::new(),
            views: views::Views::new(),
        }
    }

    async fn index(&self) -> io::Result<Index> {
        let index: BTreeMap<String, Value> = disk::read(&self.run.join("queues.json"))
            .await?
            .unwrap_or_default();
        index
            .into_iter()
            .map(|(session, mut row)| {
                let version =
                    format::check_version("Conversation queue", row.get("formatVersion"), 0, 1)
                        .map_err(io::Error::other)?;
                if let Some(fields) = row.as_object_mut() {
                    fields.insert("formatVersion".into(), json!(version));
                }
                Ok((session, serde_json::from_value(row)?))
            })
            .collect()
    }

    pub(crate) async fn get(&self, session: &str) -> io::Result<Queue> {
        Ok(self.index().await?.remove(session).unwrap_or_default())
    }

    pub(crate) async fn display_deadline(&self, home: &Path, session: &str) -> Option<u64> {
        self.get(session).await.ok()?.display_deadline(home)
    }

    pub(crate) async fn discard(&self, session: &str) -> io::Result<()> {
        let prior = self.get(session).await?;
        self.verify_waiting(&prior).await?;
        self.put(session, |state| *state = Queue::default()).await?;
        for row in prior.waiting {
            match tokio::fs::remove_file(self.waiting_file(&row.id)?).await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn put<F>(&self, session: &str, edit: F) -> io::Result<Queue>
    where
        F: FnOnce(&mut Queue),
    {
        let edit_guard = self
            .edits
            .clone()
            .acquire_owned()
            .await
            .expect("queue edits stay open");
        let mut index = self.index().await?;
        let before = index.get(session).cloned();
        let state = index.entry(session.to_owned()).or_default();
        edit(state);
        let now = disk::now();
        state.format_version = 1;
        state.arriving.retain(|_, expires| *expires > now);
        if state
            .resumed
            .is_some_and(|at| now.saturating_sub(at) >= 60_000)
        {
            state.resumed = None;
        }
        let result = state.clone();
        index.retain(|_, queue| queue.has_state());
        if before.as_ref() == index.get(session) {
            return Ok(result);
        }
        let path = self.run.join("queues.json");
        let store = self.clone();
        let held_changed = before.map(|queue| queue.held) != Some(result.held.clone());
        tokio::task::spawn_blocking(move || {
            let _edit = edit_guard;
            disk::write(&path, &index)?;
            store.notify();
            Ok::<_, io::Error>(())
        })
        .await??;
        // (A coding agent's line keeps what's held, as the engine's inbox does.)
        if held_changed && let Some(agents) = self.agents.get() {
            let _ = agents.hold_line(session, result.held.clone()).await;
        }
        Ok(result)
    }

    pub(crate) fn notify(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub(crate) fn changes(&self) -> watch::Receiver<u64> { self.changes.subscribe() }

    fn waiting_file(&self, id: &str) -> io::Result<PathBuf> {
        if !send::valid_id(id) {
            return Err(io::Error::other("Invalid waiting message ID."));
        }
        Ok(self.run.join("waiting").join(format!("{id}.json")))
    }

    async fn waiting_prompt(&self, id: &str) -> io::Result<Option<Prompt>> {
        let stored = disk::read::<Value>(&self.waiting_file(id)?).await?;
        stored
            .map(|value| {
                format::check_version("Waiting message", value.get("formatVersion"), 0, 1)
                    .map_err(io::Error::other)?;
                serde_json::from_value(value).map_err(io::Error::from)
            })
            .transpose()
    }

    async fn verify_waiting(&self, queue: &Queue) -> io::Result<()> {
        for item in &queue.waiting {
            self.waiting_prompt(&item.id).await?;
        }
        Ok(())
    }

    async fn save_waiting(&self, prompt: &Prompt) -> io::Result<()> {
        self.waiting_prompt(&prompt.id).await?;
        let mut value = serde_json::to_value(prompt)?;
        value["formatVersion"] = json!(1);
        disk::durable(self.waiting_file(&prompt.id)?, value).await
    }

    pub(crate) async fn keep(
        &self,
        session: &str,
        prompt: &Prompt,
        words: Option<&str>,
        from: Option<String>,
        before: Option<(&str, &[Inbox])>,
        engine_target: bool,
    ) -> io::Result<bool> {
        let current = self.get(session).await?;
        let arriving = current.arriving(&prompt.id);
        let release = engine_target
            && (crate::release::holding(&self.run)
                || (crate::release::releasing(&self.run) && !current.waiting.is_empty()));
        if !release
            && !current.paused
            && !current
                .limit_stop
                .as_ref()
                .is_some_and(|stop| stop.releasing)
            && current.inbox_moved.is_empty()
        {
            return Ok(false);
        }
        if !release
            && prompt.delivery == Delivery::Steer
            && !arriving
            && current.inbox_moved.is_empty()
            && !current
                .limit_stop
                .as_ref()
                .is_some_and(|stop| stop.releasing)
        {
            return Ok(false);
        }
        let mut prompt = prompt.clone();
        if arriving {
            prompt.delivery = Delivery::Queue;
        }
        self.save_waiting(&prompt).await?;
        let waiting = Waiting {
            id: prompt.id.clone(),
            text: words.unwrap_or(&prompt.text).to_owned(),
            files: file_names(&prompt.files),
            at: disk::now(),
            from,
        };
        self.put(session, |state| {
            state.waiting.retain(|row| row.id != prompt.id);
            if arriving {
                state.waiting.push(waiting);
                state.arriving.remove(&prompt.id);
                state.held.retain(|id| id != &prompt.id);
                let at = state
                    .held
                    .iter()
                    .position(|id| !state.came_held.contains(id))
                    .unwrap_or(state.held.len());
                state.held.insert(at, prompt.id.clone());
                state.came_held.push(prompt.id.clone());
                state.paused = true;
            } else {
                let (anchor, inbox) =
                    before.map_or((None, &[][..]), |(anchor, inbox)| (Some(anchor), inbox));
                state.restore_before(waiting, anchor, inbox);
            }
        })
        .await?;
        Ok(true)
    }

    pub(crate) async fn release_pending(
        &self,
        engine: &Engine,
        lease: Option<&Lease>,
    ) -> io::Result<bool> {
        for (session, state) in self.index().await? {
            if state.paused
                || state.limit_stop.is_some()
                || state.waiting.is_empty()
                || engine
                    .agent_session(&session)
                    .await
                    .map_err(|_| {
                        io::Error::other("Could not check the owner of a waiting message.")
                    })?
                    .is_some()
            {
                continue;
            }
            if let Some(lease) = lease {
                match engine
                    .call::<Value>(lease, Method::GET, &sessions::path(&session), None)
                    .await
                {
                    // Preserve this queue, but a confirmed absent session has
                    // no turn to release and must not hold every other update.
                    Err(error) if error.missing() => continue,
                    Err(error) => return Err(io::Error::other(error)),
                    Ok(_) => {}
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    async fn clear(&self, session: &str, id: &str) -> io::Result<Queue> {
        let state = self
            .put(session, |state| {
                state.waiting.retain(|waiting| waiting.id != id);
                state.held.retain(|held| held != id);
                state.order.retain(|ordered| ordered != id);
                state.came_held.retain(|held| held != id);
                state.handoffs.clear(id);
            })
            .await?;
        if let Ok(path) = self.waiting_file(id) {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(state)
    }
}

impl Queue {
    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn letter_wait_reason(&self, home: &Path) -> String {
        let returns = "Its answer, if it writes one, comes to you as a message.";
        if !self.paused {
            let line = self.shown(home)["line"]
                .as_str()
                .map_or_else(String::new, |line| format!(" {line}"));
            return format!("Your message is queued for that conversation.{line} {returns}");
        }
        if let Some(limit) = &self.limit_stop {
            let until = if limit.resets_at.is_some() {
                "for the reset or an explicit resume"
            } else {
                "for the conversation to resume"
            };
            return format!(
                "That conversation stopped. {} Your message waits {until}. {returns}",
                limit::line(home, limit)
            );
        }
        if self.user_paused == Some(true) {
            return "That conversation is stopped: the user paused it. Your message waits there and goes once they resume it; its answer, if it writes one, comes to you as a message.".into();
        }
        format!(
            "That conversation has a paused queue. Your message waits for the conversation to resume. {returns}"
        )
    }

    fn shown_in_overview(&self) -> bool {
        self.paused || !self.held.is_empty() || !self.waiting.is_empty()
    }

    fn has_state(&self) -> bool {
        self.shown_in_overview()
            || !self.order.is_empty()
            || self.resumed.is_some()
            || self.resumed_limit_at.is_some()
            || self.limit_stop.is_some()
            || !self.arriving.is_empty()
            || !self.inbox_moved.is_empty()
            || !self.handoffs.is_empty()
            || !self.unknown.is_empty()
    }

    pub(crate) fn arriving(&self, id: &str) -> bool {
        self.arriving
            .get(id)
            .is_some_and(|expires| *expires > disk::now())
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.waiting.iter().any(|item| item.id == id)
    }

    /// Whether something from `from` waits here on the box with these words.
    pub(crate) fn waits_from(&self, from: &str, words: &str) -> bool {
        self.waiting
            .iter()
            .any(|item| item.from.as_deref() == Some(from) && item.text == words)
    }

    fn keep_held(&mut self, mut moved: Vec<String>, inbox: &[Inbox]) {
        let waiting = &self.waiting;
        let present = |id: &str| {
            waiting.iter().any(|row| row.id == id)
                || inbox.iter().any(|row| row.id == id && row.is_message())
        };
        moved.retain(|id| present(id));
        self.held.retain(|id| !moved.contains(id) && present(id));
        if !self.order.is_empty() && !moved.is_empty() {
            self.order.retain(|id| !moved.contains(id) && present(id));
            self.order.splice(..0, moved.iter().cloned());
        }
        moved.append(&mut self.held);
        self.held = moved;
        self.paused |= inbox.iter().any(Inbox::is_message) || !self.waiting.is_empty();
    }

    fn display_deadline(&self, home: &Path) -> Option<u64> {
        if !self.paused { return None; }
        limit::next_display_change(home, self.limit_stop.as_ref()?)
    }

    pub(crate) fn shown(&self, home: &Path) -> Value {
        let mut value = json!({ "paused": self.paused, "held": self.held });
        if !self.waiting.is_empty() {
            value["waiting"] = json!(self.waiting);
        }
        if self.paused {
            if let Some(stop) = &self.limit_stop {
                value["line"] = json!(limit::line(home, stop));
            }
        } else if !self.waiting.is_empty() {
            if crate::release::holding(&home.join("run")) {
                value["line"] = json!(crate::release::HOLD_LINE);
            } else if !self.inbox_moved.is_empty() {
                value["line"] = json!("Messages wait while Kuvo checks the engine’s inbox.");
            }
        }
        value
    }

    fn order<'a>(&'a self, inbox: &'a [Inbox]) -> Vec<&'a str> {
        let exists = |id: &str| {
            inbox.iter().any(|row| row.id == id && row.is_message()) || self.contains(id)
        };
        let mut seen = HashSet::new();
        let base = self
            .held
            .iter()
            .map(String::as_str)
            .filter(|id| exists(id))
            .chain(
                inbox
                    .iter()
                    .filter(|item| item.is_message() && item.delivery == Delivery::Steer)
                    .map(|row| row.id.as_str()),
            )
            .chain(
                inbox
                    .iter()
                    .filter(|item| item.is_message() && item.delivery == Delivery::Queue)
                    .map(|row| row.id.as_str()),
            )
            .chain(self.waiting.iter().map(|row| row.id.as_str()))
            .filter(|id| seen.insert(*id))
            .collect::<Vec<_>>();
        if self.order.is_empty() {
            return base;
        }
        let fixed = |id: &str| {
            !self.held.iter().any(|held| held == id)
                && inbox
                    .iter()
                    .any(|row| row.id == id && row.delivery == Delivery::Steer)
        };
        let mut available = base
            .iter()
            .copied()
            .filter(|id| !fixed(id))
            .collect::<HashSet<_>>();
        let mut ranked = self
            .order
            .iter()
            .map(String::as_str)
            .chain(base.iter().copied())
            .filter(|id| available.remove(id))
            .collect::<Vec<_>>()
            .into_iter();
        base.into_iter()
            .map(|id| {
                if fixed(id) {
                    id
                } else {
                    ranked.next().expect("each movable item has a rank")
                }
            })
            .collect()
    }

    fn restore_before(&mut self, item: Waiting, before: Option<&str>, inbox: &[Inbox]) {
        self.held.retain(|id| *id != item.id);
        let order = self.order(inbox);
        let position = before.and_then(|anchor| {
            order
                .iter()
                .position(|id| *id == anchor)
                .map(|at| (at, anchor))
        });
        let Some((at, anchor)) = position else {
            self.waiting.push(item);
            return;
        };
        if let Some(at) = self.order.iter().position(|id| id == anchor) {
            self.order.insert(at, item.id.clone());
        } else if let Some(at) = self.held.iter().position(|id| id == anchor) {
            self.held.insert(at, item.id.clone());
        } else if inbox.iter().any(|row| row.id == anchor) {
            // The inbox can only put a message at its front. Keep the prefix
            // together on the resident so this message takes its original turn.
            let prefix = order[..at]
                .iter()
                .filter(|id| !self.held.iter().any(|held| held == **id))
                .map(|id| (*id).to_owned())
                .collect::<Vec<_>>();
            self.held.extend(prefix);
            self.held.push(item.id.clone());
        } else if let Some(at) = self.waiting.iter().position(|row| row.id == anchor) {
            self.waiting.insert(at, item);
            return;
        }
        self.waiting.push(item);
    }

    fn release(&mut self) {
        self.paused = false;
        let now = disk::now();
        self.resumed = Some(now);
        if self.limit_stop.as_ref().is_some_and(|stop| !stop.releasing) {
            self.resumed_limit_at = Some(now);
        }
        self.limit_stop = None;
        self.user_paused = None;
        self.arriving.clear();
    }
}

fn file_names(files: &[Value]) -> Vec<String> {
    files
        .iter()
        .filter(|file| file.is_object())
        .map(|file| {
            file["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    let uri = file
                        .pointer("/source/uri")
                        .or_else(|| file.get("uri"))?
                        .as_str()?;
                    let path = reqwest::Url::parse(uri).ok()?.to_file_path().ok()?;
                    Some(path.file_name()?.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "file".into())
        })
        .collect()
}

async fn with_items(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    queue: Queue,
    inbox: &[Inbox],
) -> Result<Value, Refusal> {
    let queue = handoff::pending(engine, target, session, queue, inbox).await?;
    let mut shown = queue.shown(&engine.home);
    let mut items = Vec::new();
    for id in queue.order(inbox) {
        let held = queue.held.iter().any(|held| held == id);
        if let Some(waiting) = queue.waiting.iter().find(|row| row.id == id) {
            let payload = engine
                .queues
                .waiting_prompt(id)
                .await
                .map_err(save_failed)?;
            let delivery = payload
                .as_ref()
                .map_or(Delivery::Queue, |message| message.delivery);
            let wake = payload
                .as_ref()
                .and_then(|message| wake_summary(&message.text, message.metadata.as_ref()));
            let words = wake.as_deref().unwrap_or(&waiting.text);
            items.push(json!({ "id": id, "words": words, "files": waiting.files,
                "from": waiting.from, "on": "box", "delivery": delivery, "held": held }));
        } else if let Some(item) = inbox.iter().find(|row| row.id == id) {
            let (words, from) = item.shown();
            let (words, from) = if from.is_none()
                && let Some(summary) =
                    wake_summary(item.text().unwrap_or_default(), item.metadata())
            {
                (summary, Some("Wake-up".to_owned()))
            } else {
                (words, from)
            };
            items.push(
                json!({ "id": id, "words": words, "files": file_names(item.files()),
                "from": from, "on": "engine", "delivery": item.delivery, "held": held }),
            );
        }
    }
    shown["items"] = json!(items);
    Ok(shown)
}

fn wake_summary(text: &str, metadata: Option<&Value>) -> Option<String> {
    let note = text.strip_prefix("[Wake-up] ")?.trim();
    let stored = metadata
        .and_then(|metadata| metadata.pointer("/kuvo/wake/summary"))
        .and_then(Value::as_str)
        .map(|summary| summary.split_whitespace().collect::<Vec<_>>().join(" "));
    if let Some(stored) = stored.filter(|summary| !summary.is_empty()) {
        return Some(String::from_utf16_lossy(
            &stored.encode_utf16().take(300).collect::<Vec<_>>(),
        ));
    }
    let line = note
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let plain = line.replace(['*', '`'], "");
    let plain = plain
        .trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '#' | '>' | '-'))
        .trim();
    let sentence = plain
        .char_indices()
        .find_map(|(index, ch)| {
            let after = index + ch.len_utf8();
            (matches!(ch, '.' | '!' | '?' | '…')
                && plain[after..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace))
            .then_some(&plain[..after])
        })
        .unwrap_or(plain);
    if sentence.is_empty() {
        Some("Wake-up".to_owned())
    } else if sentence.encode_utf16().count() > 300 {
        let clipped =
            String::from_utf16_lossy(&sentence.encode_utf16().take(299).collect::<Vec<_>>());
        Some(format!("{}…", clipped.trim_end()))
    } else {
        Some(sentence.to_owned())
    }
}

pub(crate) fn format_refusal(error: &io::Error) -> Option<Refusal> {
    use kuvo_protocol::version::{FormatRefusalCode, UnsupportedFormatVersion};

    let format = error.get_ref()?.downcast_ref::<format::Error>()?;
    matches!(format, format::Error::Unsupported { .. }).then(|| {
        Refusal::json(
            StatusCode::CONFLICT,
            json!(UnsupportedFormatVersion {
                code: FormatRefusalCode::UnsupportedFormatVersion,
                message: error.to_string(),
            }),
        )
    })
}

fn save_failed(error: io::Error) -> Refusal {
    if let Some(refusal) = format_refusal(&error) {
        return refusal;
    }
    Refusal::new(
        StatusCode::BAD_GATEWAY,
        format!("This computer couldn’t save the queue: {error}"),
    )
}

pub(crate) async fn inbox(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
) -> Result<Vec<Inbox>, api::Error> {
    target.inbox(engine, session).await
}

pub(crate) async fn reached(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    id: &str,
) -> Result<bool, api::Error> {
    if inbox(engine, target, session)
        .await?
        .iter()
        .any(|item| item.id == id)
    {
        return Ok(true);
    }
    transcript_has(engine, target, session, id).await
}

async fn transcript_has(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    id: &str,
) -> Result<bool, api::Error> {
    target.has_message(engine, session, id).await
}

pub(crate) async fn command_target(engine: &Engine, session: &str) -> Result<Place, Refusal> {
    if let Some(place) = Place::agents(engine, session).await? {
        return Ok(place);
    }
    let lease = api::start(engine).await?;
    // (As the queue routes say it, for any of their conversations.)
    let path = sessions::path(session);
    match engine.call::<Value>(&lease, Method::GET, &path, None).await {
        Ok(_) => Ok(Place::Engine(lease)),
        Err(error) if error.missing() => Err(Refusal::new(
            StatusCode::NOT_FOUND,
            "There's no such conversation here.",
        )),
        Err(_) => Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The engine isn't answering. Try again in a moment.",
        )),
    }
}

pub(crate) async fn check_add_target(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
) -> Result<(), Refusal> {
    // The coding agents' conversation was found where its place was.
    let Target::Engine(lease) = target else {
        return Ok(());
    };
    match engine
        .call_raw(
            lease,
            Method::GET,
            &sessions::path(session),
            None,
            Duration::from_secs(1),
        )
        .await
    {
        Ok(answer) if answer["data"]["id"] == session => Ok(()),
        Err(error) if error.missing() => Err(Refusal::new(
            StatusCode::NOT_FOUND,
            "There's no such conversation here.",
        )),
        Err(error) if inactive_socket(&error, lease.record.port).await => Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The engine isn't running.",
        )),
        _ => Err(Refusal::json(
            StatusCode::GATEWAY_TIMEOUT,
            json!({
                "message": "This computer is busy: its engine didn't answer in time. Try again in a moment.",
                "busy": true,
            }),
        )),
    }
}

async fn inactive_socket(error: &api::Error, port: u16) -> bool {
    if error.connection_refused() {
        return true;
    }
    #[cfg(windows)]
    if matches!(error, api::Error::Transport(error) if error.is_timeout()) {
        // Consult the listener table when WinSock did not report refusal
        // before the request deadline. An existing listener can still be busy.
        return tokio::task::spawn_blocking(move || windows_socket::closed(port))
            .await
            .unwrap_or(false);
    }
    let _ = port;
    false
}

pub(crate) async fn snapshot(
    engine: &Engine,
    session: &str,
    items: bool,
) -> Result<Value, Refusal> {
    let queue = engine.queues.get(session).await.map_err(save_failed)?;
    if !items {
        return Ok(queue.shown(&engine.home));
    }
    if let Some(place) = Place::passive(engine, session).await
        && let Ok(inbox) = inbox(engine, place.target(), session).await
    {
        with_items(engine, place.target(), session, queue, &inbox).await
    } else {
        let mut shown = queue.shown(&engine.home);
        shown["items"] = Value::Null;
        Ok(shown)
    }
}

impl Engine {
    /// Read the queue as the owning computer would show it locally.
    pub async fn owner_queue(&self, session: &str) -> Result<Value, Refusal> {
        let lease = self.acquire(Demand::Passive).await.ok_or_else(|| {
            Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The engine isn't running.")
        })?;
        sessions::read(self, &lease, &sessions::path(session)).await?;
        snapshot(self, session, false).await
    }
}

fn queue_failed(error: api::Error) -> Refusal {
    Refusal::new(StatusCode::BAD_GATEWAY, error.to_string())
}

pub(crate) async fn hold_arrival(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    id: &str,
) -> io::Result<()> {
    let rows = inbox(engine, target, session)
        .await
        .map_err(io::Error::other)?;
    let found = match rows.iter().find(|row| row.id == id && row.is_message()) {
        Some(row) if row.delivery == Delivery::Steer => {
            set_delivery(engine, target, session, id, Delivery::Queue)
                .await
                .is_ok()
        }
        Some(_) => true,
        None => false,
    };
    let mut held = Vec::new();
    if found {
        held.push(id.to_owned());
        for row in rows
            .iter()
            .filter(|row| row.id != id && row.is_message() && row.delivery == Delivery::Steer)
        {
            if set_delivery(engine, target, session, &row.id, Delivery::Queue)
                .await
                .is_ok()
            {
                held.push(row.id.clone());
            }
        }
    }
    engine
        .queues
        .put(session, |state| {
            state.arriving.remove(id);
            if found {
                let mut old = std::mem::take(&mut state.held);
                old.retain(|item| !held.contains(item));
                state.held = held;
                state.held.extend(old);
                state.paused = true;
            }
        })
        .await?;
    Ok(())
}

async fn set_delivery(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    id: &str,
    delivery: Delivery,
) -> Result<(), api::Error> {
    target.set_delivery(engine, session, id, delivery).await
}

pub(crate) async fn interrupt(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    resume: bool,
) -> Result<(), api::Error> {
    target.interrupt(engine, session, resume).await
}

async fn holding(
    engine: &Engine,
    session: &str,
    given: Vec<String>,
    stopping: bool,
) -> Result<Value, Refusal> {
    let _admitted = crate::session_move::admit_queue_hold(engine, session).await?;
    let lease = command_target(engine, session).await?;
    if stopping {
        if !given.is_empty() {
            engine
                .queues
                .put(session, |queue| {
                    let expires = disk::now() + ARRIVING_FOR;
                    queue.paused = true;
                    queue.user_paused = Some(true);
                    queue.limit_stop = None;
                    queue
                        .arriving
                        .extend(given.iter().map(|id| (id.clone(), expires)));
                })
                .await
                .map_err(save_failed)?;
        }
        // Let in-flight sends finish their handoff before interrupting. A late
        // named send still sees the durable hold once this deadline passes.
        let _turn = tokio::time::timeout(std::time::Duration::from_secs(10), engine.turn(session))
            .await
            .ok();
        return hold_admitted(engine, lease.target(), session, given, Hold::UserStop).await;
    }
    let _turn = engine.turn(session).await;
    hold_admitted(engine, lease.target(), session, given, Hold::Queue).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    Queue,
    UserStop,
    LeadStop,
}

pub(crate) async fn hold_on(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    given: Vec<String>,
    how: Hold,
) -> Result<Value, Refusal> {
    let _admitted = crate::session_move::admit(engine, session, None).await?;
    hold_admitted(engine, target, session, given, how).await
}

async fn hold_admitted(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    given: Vec<String>,
    how: Hold,
) -> Result<Value, Refusal> {
    if how == Hold::UserStop || !given.is_empty() {
        engine
            .queues
            .put(session, |queue| {
                let expires = disk::now() + ARRIVING_FOR;
                queue
                    .arriving
                    .extend(given.iter().map(|id| (id.clone(), expires)));
                if how == Hold::UserStop {
                    queue.paused = true;
                    queue.user_paused = Some(true);
                    queue.limit_stop = None;
                }
            })
            .await
            .map_err(save_failed)?;
    }
    let initial = inbox(engine, target, session).await.map_err(queue_failed)?;
    let mut held = Vec::new();
    for id in given.iter().chain(
        initial
            .iter()
            .filter(|row| row.delivery == Delivery::Steer)
            .map(|row| &row.id),
    ) {
        if held.contains(id) {
            continue;
        }
        if let Some(item) = initial.iter().find(|row| &row.id == id && row.is_message())
            && (item.delivery == Delivery::Queue
                || set_delivery(engine, target, session, id, Delivery::Queue)
                    .await
                    .is_ok())
        {
            held.push(id.clone());
        }
    }
    let remaining = inbox(engine, target, session).await.unwrap_or(initial);
    for summary in remaining
        .iter()
        .filter(|row| !row.is_message() && how == Hold::UserStop)
    {
        let _ = target.take_back(engine, session, &summary.id).await;
    }
    let queue = engine
        .queues
        .put(session, |queue| queue.keep_held(held, &remaining))
        .await
        .map_err(save_failed)?;
    if how != Hold::Queue {
        interrupt(engine, target, session, false)
            .await
            .map_err(queue_failed)?;
    }
    Ok(queue.shown(&engine.home))
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Advance {
    Steer,
    Now,
    AfterTurn,
}

pub(crate) async fn advance(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    id: &str,
    how: Advance,
) -> Result<bool, Refusal> {
    if release_holds(engine, target) {
        return if how == Advance::AfterTurn {
            Ok(true)
        } else {
            advance_held(engine, session, id).await
        };
    }
    let current = engine.queues.get(session).await.map_err(save_failed)?;
    let running = is_running(engine, target, session)
        .await
        .map_err(queue_failed)?;
    if how == Advance::AfterTurn && running {
        return Ok(true);
    }
    if current.contains(id) {
        if !handoff::send(engine, target, session, id, true).await? {
            return Ok(false);
        }
        if how != Advance::AfterTurn {
            engine
                .queues
                .put(session, Queue::release)
                .await
                .map_err(save_failed)?;
        }
        if running {
            match target {
                Target::Agents(source) if how == Advance::Now => {
                    match source.advance_line(session, id, true).await {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(save_failed(error)),
                    }
                }
                Target::Agents(_) => {}
                Target::Engine(_) => interrupt(engine, target, session, true)
                    .await
                    .map_err(queue_failed)?,
            }
        }
        return Ok(true);
    }
    if saved_removal(engine, session, id)
        .await
        .map_err(save_failed)?
        .is_some_and(|removal| !removal.complete)
    {
        return Ok(false);
    }
    let inbox = inbox(engine, target, session).await.map_err(queue_failed)?;
    let Some(item) = inbox.iter().find(|item| item.id == id && item.is_message()) else {
        return Ok(false);
    };
    let now = how == Advance::Now || !running;
    match target {
        // (Let go first, so its line may start it; with nothing running, what's steered starts
        // now, as the engine's does.)
        Target::Agents(source) => {
            engine
                .queues
                .put(session, |queue| {
                    queue.held.retain(|held| held != id);
                    queue.came_held.retain(|held| held != id);
                })
                .await
                .map_err(save_failed)?;
            match source.advance_line(session, id, now).await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(save_failed(error)),
            }
        }
        Target::Engine(_) if item.delivery == Delivery::Queue => {
            if let Err(error) = set_delivery(engine, target, session, id, Delivery::Steer).await {
                if error.missing() {
                    return Ok(false);
                }
                return Err(queue_failed(error));
            }
        }
        Target::Engine(_) => {}
    }
    engine
        .queues
        .put(session, |queue| {
            queue.held.retain(|held| held != id);
            queue.order.retain(|ordered| ordered != id);
            queue.came_held.retain(|held| held != id);
            if now && how != Advance::AfterTurn {
                queue.release();
            }
        })
        .await
        .map_err(save_failed)?;
    if matches!(target, Target::Engine(_)) && now && (running || item.delivery == Delivery::Steer) {
        interrupt(engine, target, session, true)
            .await
            .map_err(queue_failed)?;
    }
    Ok(true)
}

fn release_holds(engine: &Engine, target: Target<'_>) -> bool {
    matches!(target, Target::Engine(_)) && crate::release::holding(&engine.home.join("run"))
}

// Send Now and Steer edit the saved queue during a release hold; they never
// interrupt the old engine or start a turn on it.
async fn advance_held(engine: &Engine, session: &str, id: &str) -> Result<bool, Refusal> {
    if saved_removal(engine, session, id)
        .await
        .map_err(save_failed)?
        .is_some_and(|row| !row.complete)
    {
        return Ok(false);
    }
    let current = engine.queues.get(session).await.map_err(save_failed)?;
    if !current.contains(id) {
        return Ok(false);
    }
    if engine
        .queues
        .waiting_prompt(id)
        .await
        .map_err(save_failed)?
        .is_none()
    {
        return Err(save_failed(io::Error::other(
            "A waiting message could not be read.",
        )));
    }
    let saved = engine
        .queues
        .put(session, |queue| {
            if let Some(at) = queue.waiting.iter().position(|row| row.id == id) {
                let row = queue.waiting.remove(at);
                queue.waiting.insert(0, row);
                queue.release();
                queue.held.retain(|held| held != id);
                queue.came_held.retain(|held| held != id);
                queue.order.retain(|ordered| ordered != id);
                queue.order.insert(0, id.to_owned());
            }
        })
        .await
        .map_err(save_failed)?;
    Ok(saved.waiting.first().is_some_and(|row| row.id == id))
}

async fn is_running(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
) -> Result<bool, api::Error> {
    target.is_running(engine, session).await
}

async fn resume_queue(engine: &Engine, session: &str) -> Result<Value, Refusal> {
    let _turn = engine.turn(session).await;
    let _admitted = crate::session_move::admit(engine, session, None).await?;
    let lease = command_target(engine, session).await?;
    resume_on(engine, lease.target(), session).await
}

pub(crate) async fn resume_move(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    movement: &str,
) -> Result<(), Refusal> {
    let _turn = engine.turn(session).await;
    let _admitted = crate::session_move::admit_turn(engine, session, Some(movement)).await?;
    resume_on(engine, target, session).await.map(|_| ())
}

async fn resume_on(engine: &Engine, target: Target<'_>, session: &str) -> Result<Value, Refusal> {
    let current = engine.queues.get(session).await.map_err(save_failed)?;
    engine
        .queues
        .verify_waiting(&current)
        .await
        .map_err(save_failed)?;
    let saved = engine
        .queues
        .put(session, Queue::release)
        .await
        .map_err(save_failed)?;
    if release_holds(engine, target) {
        return Ok(saved.shown(&engine.home));
    }
    if !is_running(engine, target, session)
        .await
        .map_err(queue_failed)?
    {
        let inbox = inbox(engine, target, session).await.map_err(queue_failed)?;
        if let Some(id) = saved.order(&inbox).first() {
            advance(engine, target, session, id, Advance::AfterTurn).await?;
        }
    }
    Ok(engine
        .queues
        .get(session)
        .await
        .map_err(save_failed)?
        .shown(&engine.home))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CarriedQueue {
    paused: bool,
    held: Vec<String>,
    waiting: Vec<CarriedWaiting>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit_stop: Option<LimitStop>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    user_paused: bool,
}

#[derive(Deserialize, Serialize)]
struct CarriedWaiting {
    shown: Waiting,
    message: Prompt,
}

pub(crate) async fn pack_move(
    engine: &Engine,
    lease: &Lease,
    sessions: &[String],
) -> Result<BTreeMap<String, Value>, MoveEngineError> {
    let mut packed = BTreeMap::new();
    for session in sessions {
        let state = engine
            .queues
            .get(session)
            .await
            .map_err(MoveEngineError::Storage)?;
        let entries = inbox(engine, Target::Engine(lease), session).await?;
        if !state.paused && !entries.is_empty() {
            return Err(MoveEngineError::WaitingQueue);
        }
        if !state.paused && state.waiting.is_empty() {
            continue;
        }
        if state
            .waiting
            .iter()
            .map(|item| &item.id)
            .collect::<HashSet<_>>()
            .len()
            != state.waiting.len()
        {
            return Err(MoveEngineError::WaitingQueue);
        }
        let mut ids = BTreeMap::new();
        let mut waiting = Vec::new();
        for id in state.order(&entries) {
            let carried = if let Some(shown) = state.waiting.iter().find(|row| row.id == id) {
                let mut message = engine
                    .queues
                    .waiting_prompt(id)
                    .await
                    .map_err(|error| MoveEngineError::UnreadableQueue(Some(error)))?
                    .ok_or(MoveEngineError::UnreadableQueue(None))?;
                message.files = carry_prompt_files(std::mem::take(&mut message.files))
                    .await
                    .map_err(|error| MoveEngineError::UnreadableQueue(Some(error)))?;
                let mut shown = shown.clone();
                shown.files = file_names(&message.files);
                CarriedWaiting { shown, message }
            } else {
                let entry = entries
                    .iter()
                    .find(|entry| entry.id == id && entry.kind.as_deref() == Some("user"))
                    .ok_or(MoveEngineError::WaitingQueue)?;
                let own_id = if send::valid_id(id) {
                    id.into()
                } else {
                    format!("msg_{}", &hex::encode(Sha256::digest(id.as_bytes()))[..32])
                };
                let files = carry_prompt_files(entry.files().to_owned())
                    .await
                    .map_err(|error| MoveEngineError::UnreadableQueue(Some(error)))?;
                let (words, from) = entry.shown();
                CarriedWaiting {
                    shown: Waiting {
                        id: own_id,
                        text: words,
                        from,
                        files: file_names(&files),
                        at: disk::now(),
                    },
                    message: Prompt {
                        id: String::new(),
                        text: entry.text().ok_or(MoveEngineError::WaitingQueue)?.into(),
                        delivery: Delivery::Queue,
                        files,
                        metadata: entry.metadata().cloned(),
                    },
                }
            };
            let mut carried = carried;
            carried.message.id = carried.shown.id.clone();
            ids.insert(id, carried.shown.id.clone());
            waiting.push(carried);
        }
        if ids.values().collect::<HashSet<_>>().len() != ids.len() {
            return Err(MoveEngineError::WaitingQueue);
        }
        let carried = CarriedQueue {
            paused: state.paused,
            held: state
                .held
                .iter()
                .filter_map(|id| ids.get(id.as_str()).cloned())
                .collect(),
            order: if state.order.is_empty() {
                Vec::new()
            } else {
                waiting.iter().map(|entry| entry.shown.id.clone()).collect()
            },
            waiting,
            limit_stop: state.limit_stop,
            user_paused: state.user_paused == Some(true),
        };
        packed.insert(
            session.clone(),
            serde_json::to_value(carried)
                .map_err(|error| MoveEngineError::Storage(error.into()))?,
        );
    }
    Ok(packed)
}

pub(crate) async fn place_move(
    engine: &Engine,
    sessions: &[String],
    wanted: BTreeMap<String, Value>,
) -> io::Result<Vec<String>> {
    let mut all_ids = HashSet::new();
    let mut queues = BTreeMap::new();
    for (id, row) in wanted {
        let mut row: CarriedQueue = serde_json::from_value(row)?;
        let mut ids = HashSet::new();
        if !sessions.contains(&id) {
            return Err(unreadable_move_queue());
        }
        for carried in &mut row.waiting {
            if carried.shown.id != carried.message.id
                || !send::valid_id(&carried.shown.id)
                || !all_ids.insert(carried.shown.id.clone())
            {
                return Err(unreadable_move_queue());
            }
            ids.insert(carried.shown.id.clone());
            portable_files(&carried.message.files)?;
            engine.queues.waiting_prompt(&carried.shown.id).await?;
        }
        if row
            .held
            .iter()
            .chain(&row.order)
            .any(|id| !ids.contains(id))
            || row.order.iter().collect::<HashSet<_>>().len() != row.order.len()
        {
            return Err(unreadable_move_queue());
        }
        queues.insert(id, row);
    }
    let mut unpaused = Vec::new();
    for (id, row) in queues {
        let _turn = engine.turn(&id).await;
        forget_moved(engine, &id).await?;
        for waiting in &row.waiting {
            engine.queues.save_waiting(&waiting.message).await?;
        }
        engine
            .queues
            .put(&id, |state| {
                state.paused = true;
                state.waiting = row.waiting.into_iter().map(|item| item.shown).collect();
                state.held = row.held;
                state.order = row.order;
                state.limit_stop = row.limit_stop.filter(|_| row.paused);
                state.user_paused = (row.paused && row.user_paused).then_some(true);
            })
            .await?;
        if !row.paused {
            unpaused.push(id);
        }
    }
    Ok(unpaused)
}

pub(crate) async fn forget_moved(engine: &Engine, session: &str) -> io::Result<()> {
    let state = engine.queues.get(session).await?;
    for entry in &state.waiting {
        engine.queues.waiting_prompt(&entry.id).await?;
    }
    engine
        .queues
        .put(session, |queue| *queue = Queue::default())
        .await?;
    for entry in state.waiting {
        let path = engine.queues.waiting_file(&entry.id)?;
        tokio::task::spawn_blocking(move || kuvo_store::files::remove_file_synced(&path)).await??;
    }
    Ok(())
}

fn portable_files(files: &[Value]) -> io::Result<Vec<Value>> {
    let portable = sendable(files);
    if portable.len() != files.len()
        || portable.iter().any(|file| {
            !file["uri"].as_str().is_some_and(|uri| {
                uri.starts_with("data:")
                    && uri
                        .split_once(',')
                        .is_some_and(|(kind, _)| kind.ends_with(";base64"))
            })
        })
    {
        return Err(unreadable_move_queue());
    }
    Ok(portable)
}

async fn carry_prompt_files(files: Vec<Value>) -> io::Result<Vec<Value>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    tokio::task::spawn_blocking(move || files.iter().map(carry_prompt_file).collect()).await?
}

fn carry_prompt_file(file: &Value) -> io::Result<Value> {
    const MAX: u64 = 20 * 1024 * 1024;
    let name = file["name"].as_str();
    let uri = file
        .pointer("/source/uri")
        .and_then(Value::as_str)
        .or_else(|| file["uri"].as_str());
    let mime = file["mime"].as_str().filter(|mime| !mime.is_empty());
    let (uri, name) = if let Some(uri) = uri.filter(|uri| {
        uri.starts_with("data:")
            && uri
                .split_once(',')
                .is_some_and(|(label, _)| label.ends_with(";base64"))
    }) {
        (uri.to_owned(), name.map(str::to_owned))
    } else if let Some(data) = file["data"].as_str() {
        let mime = mime.unwrap_or("application/octet-stream");
        (
            format!("data:{mime};base64,{data}"),
            name.map(str::to_owned),
        )
    } else {
        let path = uri
            .filter(|uri| uri.starts_with("file:"))
            .and_then(|uri| url::Url::parse(uri).ok())
            .and_then(|url| url.to_file_path().ok())
            .ok_or_else(unreadable_move_queue)?;
        let before = std::fs::metadata(&path).map_err(|_| unreadable_move_queue())?;
        if !before.is_file() || before.len() > MAX {
            return Err(unreadable_move_queue());
        }
        let file = std::fs::File::open(&path).map_err(|_| unreadable_move_queue())?;
        let mut bytes = Vec::with_capacity(before.len() as usize);
        file.take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unreadable_move_queue())?;
        if bytes.len() as u64 != before.len() {
            return Err(unreadable_move_queue());
        }
        let mime = mime.unwrap_or_else(|| carried_mime(&path));
        let name = name
            .or_else(|| path.file_name().and_then(|name| name.to_str()))
            .map(str::to_owned);
        (
            format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
            name,
        )
    };
    let mut carried = json!({"uri":uri});
    if let Some(name) = name {
        carried["name"] = name.into();
    }
    Ok(carried)
}

fn carried_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("heic") => "image/heic",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("md") => "text/markdown",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn unreadable_move_queue() -> io::Error {
    io::Error::other("Some messages waiting to be sent didn’t come through. Try again.")
}

fn queue_item(call: &RouteCall<()>) -> Result<String, Refusal> {
    percent_decode_str(call.param("item").unwrap_or_default())
        .decode_utf8()
        .ok()
        .filter(|id| !id.is_empty())
        .map(|id| id.into_owned())
        .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Give a message ID."))
}

fn not_waiting() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "That message isn't waiting any more.",
    )
}

fn follow_files(home: &Path) -> kuvo_os::watch::FileSet {
    let run = home.join("run");
    let mut files = kuvo_os::watch::FileSet::new(kuvo_os::watch::WatchHub::default());
    files.set_targets(vec![run.join("queues.json"), run.join("fleet-engine-hold.json"),
        home.join("wakes/timezone"), home.join("box/time-zone.json")], vec![(run.join("waiting"), false)]);
    files
}

async fn display_due(at: Option<u64>) {
    match at {
        Some(at) => timer_fires::sleep(Timer::EngineQueueFollow, std::time::Duration::from_millis(at.saturating_sub(disk::now()).max(1))).await,
        None => std::future::pending().await,
    }
}

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let local = engine.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            let session = sessions::id(&call)?;
            let wants = crate::query(&call.request.path, "items").as_deref() == Some("1");
            let mut shown = snapshot(&engine, &session, false).await?;
            if wants {
                let seen = engine.queues.views.refresh(&engine, &session).await;
                shown["items"] = seen.items.as_deref().cloned().unwrap_or_default();
            }
            Ok(RouteReply::json::<200>(sessions::typed(shown)?))
        }
    });

    let local = engine.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            let session = sessions::id(&call)?;
            let mut updates = engine.queues.changes.subscribe();
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut previous = None;
                let mut files = follow_files(&engine.home);
                let mut states = engine.states();
                loop {
                    updates.borrow_and_update();
                    let sample = engine.queues.views.refresh(&engine, &session).await;
                    let requested = sample.requested();
                    let seen = sample.complete().await;
                    let retry = requested && seen.is_none();
                    let deadline = engine.queues.display_deadline(&engine.home, &session).await;
                    if updates.has_changed().unwrap_or_default() {
                        continue;
                    }
                    if let Some(value) = seen && previous.as_ref() != Some(&value) {
                        yield JsonLine::new(serde_json::from_value::<events::Status200>(value.as_ref().clone())?)?;
                        previous = Some(value);
                    }
                    tokio::select! {
                        () = call.cancel.cancelled() => break,
                        result = updates.changed() => if result.is_err() { break; },
                        _ = files.changed() => {},
                        result = states.changed() => if result.is_err() { break; },
                        _ = display_due(deadline) => {},
                        _ = timer_fires::sleep(Timer::EngineQueueFollow, std::time::Duration::from_secs(2)), if retry => {},
                    }
                }
            }))
        }
    });

    let local = engine.clone();
    routes.register::<overview::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            let mut updates = engine.queues.changes.subscribe();
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut previous = None;
                let mut files = follow_files(&engine.home);
                loop {
                    let index = engine.queues.index().await?;
                    let deadline = index.values().filter_map(|state| state.display_deadline(&engine.home)).min();
                    let queues = index.iter().filter(|(_, state)| state.shown_in_overview())
                        .map(|(id, state)| (id, state.shown(&engine.home))).collect::<BTreeMap<_, _>>();
                    let value = json!({ "queues": queues });
                    if previous.as_ref() != Some(&value) {
                        previous = Some(value.clone());
                        yield JsonLine::new(serde_json::from_value::<overview::Status200>(value)?)?;
                    }
                    tokio::select! {
                        () = call.cancel.cancelled() => break,
                        result = updates.changed() => if result.is_err() { break; },
                        _ = files.changed() => {},
                        _ = display_due(deadline) => {},
                    }
                }
            }))
        }
    });

    let local = engine.clone();
    routes.register::<hold::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { pause_reply(&engine, call, false).await }
    });
    let local = engine.clone();
    routes.register::<stop::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { pause_reply(&engine, call, true).await }
    });
    let local = engine.clone();
    routes.register::<resume::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            let session = sessions::id(&call)?;
            Ok(RouteReply::json::<200>(sessions::typed(
                resume_queue(&engine, &session).await?,
            )?))
        }
    });
    let local = engine.clone();
    routes.register::<now::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { advance_reply(&engine, call, Advance::Now).await }
    });
    let local = engine.clone();
    routes.register::<steer::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { advance_reply(&engine, call, Advance::Steer).await }
    });
    let local = engine.clone();
    routes.register::<reorder::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { reorder_reply(&engine, call).await }
    });
    let local = engine.clone();
    routes.register::<remove::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { remove_reply(&engine, call).await }
    });
    let local = engine.clone();
    routes.register::<ack::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { acknowledge(&engine, call).await }
    });
}

async fn pause_reply<R>(
    engine: &Engine,
    call: RouteCall<()>,
    stopping: bool,
) -> Result<RouteReply<R>, Refusal>
where
    R: kuvo_protocol::JsonStatus<200>,
    <R as kuvo_protocol::JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    let session = sessions::id(&call)?;
    let given = sessions::body(&call).unwrap_or_default();
    let items: Vec<String> = given
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    // (A session someone started stops as their work does: what it left running and its lead too.)
    let stops = engine.work_stops.get().filter(|_| stopping);
    if let Some(stopped) = match stops {
        Some(stops) => stops.stop(&session, items.clone()).await,
        None => None,
    } {
        return Ok(RouteReply::json::<200>(sessions::typed(stopped?)?));
    }
    Ok(RouteReply::json::<200>(sessions::typed(
        holding(engine, &session, items, stopping).await?,
    )?))
}

async fn advance_reply<R>(
    engine: &Engine,
    call: RouteCall<()>,
    how: Advance,
) -> Result<RouteReply<R>, Refusal>
where
    R: kuvo_protocol::JsonStatus<200>,
    <R as kuvo_protocol::JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    let session = sessions::id(&call)?;
    let id = queue_item(&call)?;
    let engine = engine.clone();
    let work = async move {
        let _turn = engine.turn(&session).await;
        let _admitted = crate::session_move::admit(&engine, &session, None).await?;
        let lease = command_target(&engine, &session).await?;
        if !advance(&engine, lease.target(), &session, &id, how).await? {
            return Err(not_waiting());
        }
        Ok::<_, Refusal>(
            engine
                .queues
                .get(&session)
                .await
                .map_err(save_failed)?
                .shown(&engine.home),
        )
    };
    let shown = if how == Advance::Now {
        // The caller's deadline cannot cancel a native POST whose acceptance
        // is still being checked. The task owns its turn through settlement.
        let sending = tokio::spawn(work);
        match tokio::time::timeout(Duration::from_secs(30), sending).await {
            Ok(Ok(shown)) => shown?,
            Ok(Err(error)) => {
                eprintln!("Sending a queued message: {error}");
                return Err(send_now_checking());
            }
            Err(_) => return Err(send_now_checking()),
        }
    } else {
        work.await?
    };
    Ok(RouteReply::json::<200>(sessions::typed(shown)?))
}

fn send_now_checking() -> Refusal {
    Refusal::new(
        StatusCode::BAD_GATEWAY,
        "Kuvo is still checking whether the message was sent.",
    )
}

#[derive(Deserialize)]
struct Reorder {
    item: String,
    anchor: String,
    before: bool,
}

async fn reorder_reply(
    engine: &Engine,
    call: RouteCall<()>,
) -> Result<RouteReply<reorder::Spec>, Refusal> {
    let session = sessions::id(&call)?;
    let given: Reorder = serde_json::from_slice(&call.request.body).map_err(|_| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Give the message, its neighbor, and which side to put it on.",
        )
    })?;
    let _admitted = crate::session_move::admit(engine, &session, None).await?;
    let lease = command_target(engine, &session).await?;
    let inbox = inbox(engine, lease.target(), &session)
        .await
        .map_err(queue_failed)?;
    // Read the inbox before taking the writer's turn. An Add or a confirmed
    // Send Now can finish while this read is waiting on OpenCode.
    let _turn = engine.turn(&session).await;
    let saved = engine.queues.get(&session).await.map_err(save_failed)?;
    engine
        .queues
        .verify_waiting(&saved)
        .await
        .map_err(save_failed)?;
    let mut order = saved
        .order(&inbox)
        .into_iter()
        .filter(|id| {
            saved.held.iter().any(|held| held == id)
                || !inbox
                    .iter()
                    .any(|row| row.id == *id && row.delivery == Delivery::Steer)
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let bad_order = || {
        Refusal::new(
            StatusCode::CONFLICT,
            "Those messages can’t be moved now. Check the queue and try again.",
        )
    };
    if given.item == given.anchor || !order.contains(&given.item) {
        return Err(bad_order());
    }
    order.retain(|id| id != &given.item);
    let anchor = order
        .iter()
        .position(|id| id == &given.anchor)
        .ok_or_else(bad_order)?;
    order.insert(anchor + usize::from(!given.before), given.item);
    let queue = engine
        .queues
        .put(&session, |queue| queue.order = order)
        .await
        .map_err(save_failed)?;
    Ok(RouteReply::json::<200>(sessions::typed(
        with_items(engine, lease.target(), &session, queue, &inbox).await?,
    )?))
}

#[derive(Clone, Serialize, Deserialize)]
struct Removal {
    from: RemovalSource,
    removed: Value,
    token: String,
    complete: bool,
    #[serde(default, rename = "formatVersion")]
    format_version: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RemovalSource {
    Inbox,
    Box,
}

fn removal_file(engine: &Engine, session: &str, id: &str) -> PathBuf {
    engine
        .home
        .join("run/queue-removals")
        .join(format!("{}.json", disk::key(session, id)))
}

fn spent_path(engine: &Engine, session: &str, id: &str, token: &str) -> PathBuf {
    engine
        .home
        .join("run/queue-removals/spent")
        .join(disk::key(&disk::key(session, id), token))
}

async fn spend(engine: &Engine, session: &str, id: &str, token: &str) -> io::Result<()> {
    let path = spent_path(engine, session, id, token);
    tokio::task::spawn_blocking(move || kuvo_store::files::write_atomic(&path, &[])).await?
}

async fn forget_removal(engine: &Engine, session: &str, id: &str, token: &str) -> io::Result<()> {
    spend(engine, session, id, token).await?;
    match tokio::fs::remove_file(removal_file(engine, session, id)).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn saved_removal(engine: &Engine, session: &str, id: &str) -> io::Result<Option<Removal>> {
    let saved = disk::read::<Removal>(&removal_file(engine, session, id)).await?;
    let Some(saved) = saved else {
        return Ok(None);
    };
    if saved.format_version > 1 {
        return Err(io::Error::other(
            "Unsupported queue removal receipt format.",
        ));
    }
    if tokio::fs::try_exists(spent_path(engine, session, id, &saved.token)).await? {
        return Ok(None);
    }
    Ok(Some(saved))
}

pub(crate) async fn check_undo(engine: &Engine, session: &str, id: &str) -> io::Result<()> {
    if saved_removal(engine, session, id)
        .await?
        .is_some_and(|saved| !saved.complete)
    {
        return Err(io::Error::other(
            "Kuvo is still checking whether the message was taken back.",
        ));
    }
    Ok(())
}

pub(crate) async fn finish_undo(engine: &Engine, session: &str, id: &str) -> io::Result<()> {
    if let Some(saved) = saved_removal(engine, session, id).await? {
        forget_removal(engine, session, id, &saved.token).await?;
    }
    Ok(())
}

async fn remove_reply(
    engine: &Engine,
    call: RouteCall<()>,
) -> Result<RouteReply<remove::Spec>, Refusal> {
    let session = sessions::id(&call)?;
    let id = queue_item(&call)?;
    let token = crate::query(&call.request.path, "receipt");
    if token.as_deref().is_some_and(|token| !send::valid_id(token)) {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Give a valid removal receipt.",
        ));
    }
    // Do not wait for a native POST to return just to tell the composer that
    // that POST still owns this waiting draft.
    let prior = engine.queues.get(&session).await.map_err(save_failed)?;
    if prior.contains(&id)
        && let Err(refusal) = prior.handoffs.check(&id)
    {
        if let Some(token) = &token
            && tokio::fs::try_exists(spent_path(engine, &session, &id, token))
                .await
                .map_err(save_failed)?
        {
            return Err(not_waiting());
        }
        return Err(refusal);
    }
    let _turn = engine.turn(&session).await;
    let _admitted = crate::session_move::admit(engine, &session, None).await?;
    let file = removal_file(engine, &session, &id);
    let current = engine.queues.get(&session).await.map_err(save_failed)?;
    if let Some(token) = &token
        && tokio::fs::try_exists(spent_path(engine, &session, &id, token))
            .await
            .map_err(save_failed)?
    {
        return Err(not_waiting());
    }
    let existing = saved_removal(engine, &session, &id)
        .await
        .map_err(save_failed)?;
    if let (Some(token), Some(existing)) = (&token, &existing)
        && *token != existing.token
    {
        spend(engine, &session, &id, token)
            .await
            .map_err(save_failed)?;
        return Err(not_waiting());
    }
    let mut lease = Place::passive(engine, &session).await;
    let items = match &lease {
        Some(lease) => inbox(engine, lease.target(), &session).await.ok(),
        None => None,
    };
    let mut removal = if let Some(existing) = existing {
        existing
    } else {
        let before = current
            .order(items.as_deref().unwrap_or_default())
            .into_iter()
            .skip_while(|item| *item != id)
            .nth(1);
        let receipt = token.clone().unwrap_or(send::new_id(engine)?);
        let (from, mut removed) = if let Some(waiting) =
            current.waiting.iter().find(|row| row.id == id)
        {
            if !handoff::removable(
                engine,
                lease.as_ref().map(Place::target),
                &session,
                &id,
                &current.handoffs,
                items.as_deref(),
            )
            .await?
            {
                return Err(not_waiting());
            }
            let prompt = engine
                .queues
                .waiting_prompt(&id)
                .await
                .map_err(save_failed)?
                .ok_or_else(not_waiting)?;
            let mut removed = removed_value(
                &id,
                &prompt.text,
                &waiting.text,
                prompt.delivery,
                &prompt.files,
                prompt.metadata.as_ref(),
            );
            if let Some(from) = &waiting.from {
                removed["from"] = json!(from);
            }
            (RemovalSource::Box, removed)
        } else if let Some(item) = items
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|item| item.id == id && item.is_message())
        {
            let text = item.text().unwrap_or_default();
            let (words, from) = item.shown();
            let mut removed = removed_value(
                &id,
                text,
                &words,
                item.delivery,
                item.files(),
                item.metadata(),
            );
            if let Some(from) = from {
                removed["from"] = json!(from);
            }
            (RemovalSource::Inbox, removed)
        } else {
            if crate::query(&call.request.path, "arriving").as_deref() == Some("1") {
                if send::was_sent(engine, &session, &id).await?
                    || match lease.as_ref() {
                        Some(lease) => reached(engine, lease.target(), &session, &id)
                            .await
                            .unwrap_or(false),
                        None => false,
                    }
                {
                    return Err(Refusal::json(
                        StatusCode::NOT_FOUND,
                        json!({ "message": "It was already sent to the agent.", "delivered": true }),
                    ));
                }
                if send::valid_id(&id) {
                    engine
                        .queues
                        .put(&session, |queue| {
                            queue.arriving.insert(id, disk::now() + ARRIVING_FOR);
                        })
                        .await
                        .map_err(save_failed)?;
                }
                return Err(Refusal::json(
                    StatusCode::CONFLICT,
                    json!({ "message": "It’s still on its way, and is taken out as it arrives.", "arriving": true }),
                ));
            }
            return Err(not_waiting());
        };
        if let Some(before) = before {
            removed["before"] = json!(before);
        }
        let removal = Removal {
            from,
            removed,
            token: receipt,
            complete: false,
            format_version: 1,
        };
        disk::durable(file.clone(), removal.clone())
            .await
            .map_err(save_failed)?;
        removal
    };
    if !removal.complete {
        if matches!(removal.from, RemovalSource::Inbox) {
            let lease = match lease.take() {
                Some(lease) => lease,
                None => command_target(engine, &session).await?,
            };
            let items = match items {
                Some(items) => items,
                None => inbox(engine, lease.target(), &session)
                    .await
                    .map_err(queue_failed)?,
            };
            if items.iter().any(|row| row.id == id) {
                if let Err(error) = lease.target().take_back(engine, &session, &id).await
                    && !error.missing()
                {
                    return Err(queue_failed(error));
                }
                if inbox(engine, lease.target(), &session)
                    .await
                    .map_err(queue_failed)?
                    .iter()
                    .any(|row| row.id == id)
                {
                    return Err(Refusal::new(
                        StatusCode::BAD_GATEWAY,
                        "It couldn't be taken back. Try again in a moment.",
                    ));
                }
            }
            if transcript_has(engine, lease.target(), &session, &id)
                .await
                .map_err(queue_failed)?
            {
                forget_removal(engine, &session, &id, &removal.token)
                    .await
                    .map_err(save_failed)?;
                return Err(not_waiting());
            }
        }
        engine
            .queues
            .clear(&session, &id)
            .await
            .map_err(save_failed)?;
        send::forget(engine, &session, &id).await?;
        removal.complete = true;
        disk::durable(file, removal.clone())
            .await
            .map_err(save_failed)?;
    }
    let mut shown = engine
        .queues
        .get(&session)
        .await
        .map_err(save_failed)?
        .shown(&engine.home);
    let mut removed = removal.removed;
    let carried = sendable(removed["files"].as_array().map_or(&[], Vec::as_slice));
    removed["attached"] =
        json!(crate::attachments::for_edit(&engine.home, &removed, carried).await);
    removed["receipt"] = json!(removal.token);
    shown["removed"] = removed;
    Ok(RouteReply::json::<200>(sessions::typed(shown)?))
}

fn removed_value(
    id: &str,
    text: &str,
    words: &str,
    delivery: Delivery,
    files: &[Value],
    metadata: Option<&Value>,
) -> Value {
    let mut removed =
        json!({ "id": id, "text": text, "words": words, "draft": words, "delivery": delivery });
    if !files.is_empty() {
        removed["files"] = json!(files);
    }
    if let Some(metadata) = metadata {
        removed["metadata"] = metadata.clone();
    }
    removed
}

async fn acknowledge(
    engine: &Engine,
    call: RouteCall<()>,
) -> Result<RouteReply<ack::Spec>, Refusal> {
    let session = sessions::id(&call)?;
    let id = queue_item(&call)?;
    let given = sessions::body(&call).unwrap_or_default();
    let token = given
        .get("receipt")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Give the removal receipt."))?;
    let _turn = engine.turn(&session).await;
    if saved_removal(engine, &session, &id)
        .await
        .map_err(save_failed)?
        .is_some_and(|saved| saved.complete && saved.token == token)
    {
        forget_removal(engine, &session, &id, token)
            .await
            .map_err(save_failed)?;
    }
    Ok(RouteReply::json::<200>(sessions::typed(
        engine
            .queues
            .get(&session)
            .await
            .map_err(save_failed)?
            .shown(&engine.home),
    )?))
}

impl Engine {
    /// Stop an OpenCode turn on behalf of the conversation that started it.
    /// The owner of background work also stops its watches, wake-ups and shells.
    pub async fn stop_for_starter(&self, session: &str) -> Result<(), Refusal> {
        self.stop_work(session, false, Vec::new()).await.map(drop)
    }

    /// Stop for a session someone started: what waits held and its turn stopped; `holds` (the
    /// user's Stop, or a turn that was under way) pauses its queue too, as Stop does, `items` the
    /// messages sent as it came. The queue as it's left.
    pub async fn stop_work(
        &self,
        session: &str,
        holds: bool,
        items: Vec<String>,
    ) -> Result<Value, Refusal> {
        if !sessions::valid_id(session) {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "Choose a conversation.",
            ));
        }
        if holds {
            // (Paused first, as Stop is: kept so even where what waits can't be held now.)
            self.pause_for_user(session, false).await?;
            return holding(self, session, items, true).await;
        }
        let lease = command_target(self, session).await?;
        let _turn = self.turn(session).await;
        hold_on(self, lease.target(), session, items, Hold::LeadStop).await
    }

    /// The queue paused for the user, until they write or Resume; `if_limited`: only one a usage
    /// limit paused, so its reset doesn't set it going.
    pub async fn pause_for_user(&self, session: &str, if_limited: bool) -> Result<(), Refusal> {
        if !sessions::valid_id(session) {
            return Ok(());
        }
        let put = self.queues.put(session, |queue| {
            if !if_limited || queue.limit_stop.is_some() {
                queue.paused = true;
                queue.user_paused = Some(true);
                queue.limit_stop = None;
            }
        });
        put.await.map(drop).map_err(save_failed)
    }

    /// What stops a session someone started (Stop on its row), from the box's work.
    pub fn attach_work_stops(&self, stops: Arc<dyn WorkStops>) {
        let _ = self.work_stops.set(stops);
    }
}

/// A stop's outcome as the box's work gives it: none for a session that isn't someone's work.
pub type WorkStopped<'a> =
    std::pin::Pin<Box<dyn Future<Output = Option<Result<Value, Refusal>>> + Send + 'a>>;

/// Stop on a session someone started, as their work stops: what it left running too, and its lead
/// told. None for a session that isn't someone's work, which stops as any other.
pub trait WorkStops: Send + Sync {
    fn stop<'a>(&'a self, session: &'a str, items: Vec<String>) -> WorkStopped<'a>;
}

impl Engine {
    /// Keep the old engine's inboxes before a coordinated release switch.
    /// A failed inventory or transfer leaves the engine in place and can be retried.
    pub async fn keep_inboxes(&self) -> io::Result<usize> {
        self.keep_inboxes_with(Demand::Existing).await
    }

    pub(crate) async fn keep_inboxes_with(&self, demand: Demand) -> io::Result<usize> {
        let Some(lease) = self.acquire(demand).await else {
            return Ok(0);
        };
        let ids = inbox_inventory(self, &lease).await?;
        let mut moved = 0;
        for session in ids {
            let _turn = self.turn(&session).await;
            let _admitted = crate::session_move::admit(self, &session, None)
                .await
                .map_err(|_| {
                    io::Error::other("An engine inbox cannot move while its conversation is held")
                })?;
            moved += transfer(self, &lease, &session).await?;
        }
        Ok(moved)
    }

    /// Release compatible, unpaused waiting messages after a switch. An
    /// interrupted transfer must finish before even its saved prefix can go.
    pub async fn release_waiting(&self) -> io::Result<usize> {
        let Some(lease) = self.acquire(Demand::Existing).await else {
            return Ok(0);
        };
        let mut sent = 0;
        for (session, state) in self.queues.index().await? {
            if state.paused || state.limit_stop.is_some() || state.waiting.is_empty() {
                continue;
            }
            let _turn = self.turn(&session).await;
            let Ok(_admitted) = crate::session_move::admit(self, &session, None).await else {
                continue;
            };
            if self
                .call::<Value>(&lease, Method::GET, &sessions::path(&session), None)
                .await
                .is_err()
            {
                continue;
            }
            if !state.inbox_moved.is_empty() && transfer(self, &lease, &session).await.is_err() {
                continue;
            }
            match drain_waiting(self, &lease, &session).await {
                Ok(count) => sent += count,
                Err(error) => eprintln!("Releasing the engine's queue: {error}"),
            }
        }
        Ok(sent)
    }
}

async fn inbox_inventory(engine: &Engine, lease: &Lease) -> io::Result<Vec<String>> {
    let mut sessions = Vec::new();
    let mut ids = HashSet::new();
    for archived in [false, true] {
        let mut seen = HashSet::new();
        let mut path = "api/session?limit=100&order=desc".to_owned();
        if archived {
            path.push_str("&archived=true");
        }
        let base = path.clone();
        loop {
            let page = engine
                .upstream
                .request(&lease.record, Method::GET, &path)
                .timeout(api::WAIT)
                .send()
                .await
                .map_err(io::Error::other)?
                .error_for_status()
                .map_err(io::Error::other)?
                .json::<Value>()
                .await
                .map_err(io::Error::other)?;
            let incomplete =
                || io::Error::other("The engine did not list its whole session inventory.");
            let rows = page["data"].as_array().ok_or_else(incomplete)?;
            for session in rows {
                let id = session["id"]
                    .as_str()
                    .filter(|id| sessions::valid_id(id))
                    .ok_or_else(incomplete)?;
                if ids.insert(id.to_owned()) {
                    sessions.push(id.to_owned());
                }
            }
            match &page["cursor"] {
                Value::Null if page.get("cursor").is_some() || rows.len() < 100 => break,
                Value::Object(cursor) if cursor.get("next").is_none_or(Value::is_null) => break,
                Value::Object(cursor) => {
                    let next = cursor["next"]
                        .as_str()
                        .filter(|next| !next.is_empty() && !rows.is_empty())
                        .ok_or_else(incomplete)?;
                    if seen.len() >= 1000 || !seen.insert(next.to_owned()) {
                        return Err(incomplete());
                    }
                    path = format!("{base}&cursor={}", api::segment(next));
                }
                _ => return Err(incomplete()),
            }
        }
    }
    Ok(sessions)
}

async fn transfer(engine: &Engine, lease: &Lease, session: &str) -> io::Result<usize> {
    let rows = inbox(engine, Target::Engine(lease), session)
        .await
        .map_err(io::Error::other)?;
    let state = engine.queues.get(session).await?;
    let mut moved = 0;
    for id in state
        .order(&rows)
        .into_iter()
        .filter(|id| rows.iter().any(|row| row.id == *id && row.is_message()))
    {
        let item = rows.iter().find(|row| row.id == id).expect("item in inbox");
        let text = item
            .text()
            .ok_or_else(|| io::Error::other("The engine did not list the pending message text."))?;
        let id_on_disk = if send::valid_id(id) {
            id.to_owned()
        } else {
            use sha2::{Digest, Sha256};
            format!("msg_{}", hex::encode(&Sha256::digest(id)[..16]))
        };
        let (words, from) = item.shown();
        let prompt = Prompt {
            id: id_on_disk.clone(),
            text: text.to_owned(),
            delivery: item.delivery,
            files: sendable(item.files()),
            metadata: item.metadata().cloned(),
        };
        let already_kept = engine.queues.waiting_prompt(&id_on_disk).await?.is_some();
        engine.queues.save_waiting(&prompt).await?;
        engine
            .queues
            .put(session, |state| {
                let waiting = Waiting {
                    id: id_on_disk.clone(),
                    text: words,
                    from,
                    files: file_names(&prompt.files),
                    at: disk::now(),
                };
                let mut prefix = state.inbox_moved.clone();
                if !prefix.contains(&id_on_disk) {
                    prefix.push(id_on_disk.clone());
                }
                let mut pending = state
                    .waiting
                    .iter()
                    .filter(|row| row.id != id_on_disk)
                    .cloned()
                    .collect::<Vec<_>>();
                pending.push(waiting);
                state.waiting = prefix
                    .iter()
                    .filter_map(|id| pending.iter().find(|row| &row.id == id).cloned())
                    .chain(
                        pending
                            .iter()
                            .filter(|row| !prefix.contains(&row.id))
                            .cloned(),
                    )
                    .collect();
                for known in state.held.iter_mut().chain(state.order.iter_mut()) {
                    if known == id {
                        known.clone_from(&id_on_disk);
                    }
                }
                state.inbox_moved = prefix;
            })
            .await?;
        let path = format!("{}/inbox/{}", sessions::path(session), api::segment(id));
        match engine
            .call::<Value>(lease, Method::DELETE, &path, None)
            .await
        {
            Ok(_) => moved += 1,
            Err(error) if error.missing() && !already_kept => {
                engine.queues.clear(session, &id_on_disk).await?;
                engine
                    .queues
                    .put(session, |state| {
                        state.inbox_moved.retain(|id| id != &id_on_disk)
                    })
                    .await?;
            }
            Err(error) => return Err(io::Error::other(error)),
        }
    }
    if inbox(engine, Target::Engine(lease), session)
        .await
        .map_err(io::Error::other)?
        .iter()
        .any(Inbox::is_message)
    {
        return Err(io::Error::other("The engine still holds pending messages."));
    }
    if !engine.queues.get(session).await?.inbox_moved.is_empty() {
        engine
            .queues
            .put(session, |state| state.inbox_moved.clear())
            .await?;
    }
    Ok(moved)
}

fn sendable(files: &[Value]) -> Vec<Value> {
    files
        .iter()
        .filter_map(|file| {
            let uri = file
                .pointer("/source/uri")
                .or_else(|| file.get("uri"))?
                .as_str()?;
            let mut kept = json!({ "uri": uri });
            if let Some(name) = file["name"].as_str() {
                kept["name"] = json!(name);
            }
            Some(kept)
        })
        .collect()
}

async fn drain_waiting(engine: &Engine, lease: &Lease, session: &str) -> io::Result<usize> {
    let mut steer = !is_running(engine, Target::Engine(lease), session)
        .await
        .map_err(io::Error::other)?;
    let mut sent = 0;
    loop {
        let saved = engine.queues.get(session).await?;
        let order = saved.order(&[]);
        let Some(id) = order
            .first()
            .filter(|_| !saved.paused && saved.inbox_moved.is_empty())
        else {
            break;
        };
        match handoff::send(engine, Target::Engine(lease), session, id, steer).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(io::Error::other(
                    "The waiting message's contents are missing.",
                ));
            }
            Err(_) => {
                return Err(io::Error::other(
                    "The engine didn't confirm the waiting message.",
                ));
            }
        }
        steer = false;
        sent += 1;
    }
    Ok(sent)
}

/// The event reader owns delayed Stops and drains. A new turn can cancel
/// them before they alter the saved queue.
type Delayed = BTreeMap<String, (tokio::time::Instant, tokio_util::sync::CancellationToken)>;

#[derive(Default)]
pub(crate) struct QueueEvents {
    stops: Delayed,
    drains: Delayed,
}

impl Drop for QueueEvents {
    fn drop(&mut self) {
        for (_, token) in self.stops.values().chain(self.drains.values()) {
            token.cancel();
        }
    }
}

/// Follow the coding agents' conversations as the engine's: their line and
/// turn events, in the engine's words, drive the same queue reactions.
pub(crate) fn follow_agents(
    engine: Engine,
    mut events: tokio::sync::broadcast::Receiver<crate::AgentEvent>,
) {
    tokio::spawn(async move {
        let stop = tokio_util::sync::CancellationToken::new();
        let _stop = stop.clone().drop_guard();
        let mut queues = QueueEvents::default();
        loop {
            match events.recv().await {
                Ok(event) => queues.hear(
                    &engine,
                    &event.session,
                    event.kind,
                    &event.data,
                    stop.clone(),
                ),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    engine.queues.notify();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

impl QueueEvents {
    pub(crate) fn hear(
        &mut self,
        engine: &Engine,
        session: &str,
        kind: &str,
        data: &Value,
        stop: tokio_util::sync::CancellationToken,
    ) {
        if kind.starts_with("session.inbox.") {
            engine.queues.notify();
        }
        let now = tokio::time::Instant::now();
        self.stops.retain(|_, (expires, _)| *expires > now);
        self.drains.retain(|_, (expires, _)| *expires > now);
        if matches!(
            kind,
            "session.execution.started"
                | "session.execution.succeeded"
                | "session.execution.failed"
                | "session.execution.interrupted"
                | "session.deleted"
        ) && let Some((_, pending)) = self.stops.remove(session)
        {
            pending.cancel();
        }
        if matches!(
            kind,
            "session.execution.started"
                | "session.execution.failed"
                | "session.execution.interrupted"
                | "session.deleted"
        ) && let Some((_, pending)) = self.drains.remove(session)
        {
            pending.cancel();
        }
        let at = disk::now();
        let operation = match kind {
            "session.execution.started" => QueueEvent::Started,
            "session.inbox.enqueued" if own_steer(&data["item"]) => QueueEvent::Reopen,
            "session.idle" if self.stops.contains_key(session) => return,
            "session.execution.succeeded" | "session.idle" | "session.inbox.enqueued" => {
                QueueEvent::Drain
            }
            "session.execution.interrupted" => match data["reason"].as_str() {
                Some("superseded" | "inactivity" | "agent-exit") => QueueEvent::Drain,
                Some("restart") => return,
                _ => QueueEvent::Hold(None),
            },
            "session.execution.failed" => {
                QueueEvent::Failed(engine.queues.limits.classify(&data["error"], at))
            }
            "session.inbox.delivered" | "session.inbox.cancelled" => QueueEvent::Prune,
            "session.deleted" => QueueEvent::Discard,
            _ => return,
        };
        let delay = match &operation {
            QueueEvent::Started | QueueEvent::Reopen => Duration::ZERO,
            QueueEvent::Hold(_) | QueueEvent::Drain => Duration::from_millis(1500),
            _ => Duration::from_millis(100),
        };
        let deadline = now + delay;
        let delay_scope = stop.child_token();
        let delayed = match &operation {
            QueueEvent::Hold(_) => Some(&mut self.stops),
            QueueEvent::Drain => Some(&mut self.drains),
            _ => None,
        };
        if let Some(delayed) = delayed
            && let Some((_, previous)) =
                delayed.insert(session.to_owned(), (deadline, delay_scope.clone()))
        {
            previous.cancel();
        }
        let engine = engine.clone();
        let session = session.to_owned();
        tokio::spawn(async move {
            if delay_scope
                .run_until_cancelled(tokio::time::sleep_until(deadline))
                .await
                .is_none()
            {
                return;
            }
            if let Some(Err(error)) = stop
                .run_until_cancelled(react(&engine, &session, operation, at))
                .await
            {
                eprintln!("Updating the engine's queue: {error}");
            }
        });
    }
}

/// Older messages used these prefixes before automated notes carried their mark.
fn own_steer(item: &Value) -> bool {
    const AUTOMATED: &[&str] = &[
        "[Message from ",
        "[Wake-up]",
        "[Work from ",
        "[Session ",
        "[Goal started]",
        "[Goal changed]",
        "[Goal's end]",
        "[Goal resumed]",
        "[Goal] Nothing of the goal is at work now.",
        "[Not now]",
        "[Limit reset]",
        "[The user answered]",
    ];
    let text = item["payload"]["text"].as_str().unwrap_or_default();
    item["type"] == "user"
        && item["delivery"] == "steer"
        && !item
            .pointer("/payload/metadata/kuvo/note")
            .is_some_and(Value::is_object)
        && !AUTOMATED.iter().any(|prefix| text.starts_with(prefix))
}

pub(crate) fn watch_limits(
    engine: Engine,
    stop: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    limit::watch(engine, stop)
}

#[derive(Default, Deserialize, Serialize)]
struct Scans {
    #[serde(default, rename = "formatVersion")]
    version: u32,
    #[serde(flatten)]
    engines: std::collections::BTreeMap<String, u32>,
}

pub(crate) async fn scan(engine: &Engine, lease: &Lease) -> io::Result<()> {
    let path = engine.home.join("run/queues-scanned.json");
    let mut scans = disk::read::<Scans>(&path).await?.unwrap_or_default();
    if scans.version > 1 {
        return Err(io::Error::other(
            "The queue scan receipt uses a newer format.",
        ));
    }
    let known = scans
        .engines
        .insert(lease.record.port.to_string(), lease.record.pid)
        == Some(lease.record.pid);
    if !known {
        scans.version = 1;
        disk::durable(path, scans).await?;
    }
    let page = engine
        .call::<Vec<Value>>(
            lease,
            Method::GET,
            "api/session?parentID=null&limit=100&order=desc",
            None,
        )
        .await
        .map_err(io::Error::other)?;
    let week_ago = disk::now().saturating_sub(7 * 86_400_000);
    for row in page {
        if row["time"]["updated"].as_u64().unwrap_or_default() < week_ago {
            continue;
        }
        let Some(session) = row["id"].as_str().filter(|id| sessions::valid_id(id)) else {
            continue;
        };
        let Ok(entries) = inbox(engine, Target::Engine(lease), session).await else {
            continue;
        };
        if entries.is_empty() && row["outcome"] != "failed" {
            continue;
        }
        let Ok(active) = engine
            .call::<Value>(lease, Method::GET, "api/session/active", None)
            .await
        else {
            continue;
        };
        if active.get(session).is_some_and(|state| !state.is_null())
            || active
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row == session))
        {
            continue;
        }
        let recent = format!("{}/message?limit=1&order=desc", sessions::path(session));
        let (change, at) = match engine
            .call::<Vec<Value>>(lease, Method::GET, &recent, None)
            .await
        {
            Ok(page) => {
                let ended = page.first().unwrap_or(&Value::Null);
                let at = ended["time"]["created"].as_u64().unwrap_or_else(disk::now);
                let stopped = match ended["type"].as_str() {
                    Some("idle") => {
                        ended["outcome"] != "succeeded"
                            && !(ended["outcome"] == "interrupted"
                                && matches!(
                                    ended["reason"].as_str(),
                                    Some("restart" | "agent-exit")
                                ))
                    }
                    Some("assistant") => ended["error"].is_object(),
                    Some("user") => false,
                    _ => !known,
                };
                let change = if stopped {
                    let limit = engine.queues.limits.classify(&ended["error"], at);
                    if limit
                        .as_ref()
                        .and_then(|stop| stop.resets_at)
                        .is_some_and(|reset| reset <= disk::now())
                    {
                        QueueEvent::Drain
                    } else {
                        QueueEvent::Hold(limit)
                    }
                } else {
                    QueueEvent::Drain
                };
                (change, at)
            }
            Err(_) => (QueueEvent::Hold(None), disk::now()),
        };
        let _ = react(engine, session, change, at).await;
    }
    for (session, state) in engine.queues.index().await? {
        if !state.paused
            && state.limit_stop.is_none()
            && !state.waiting.is_empty()
            && state.inbox_moved.is_empty()
            && engine
                .call::<Value>(lease, Method::GET, &sessions::path(&session), None)
                .await
                .is_ok()
        {
            let _ = react(engine, &session, QueueEvent::Drain, disk::now()).await;
        }
    }
    Ok(())
}

enum QueueEvent {
    Drain,
    Hold(Option<LimitStop>),
    Failed(Option<LimitStop>),
    Prune,
    Discard,
    Started,
    Reopen,
}

async fn react(engine: &Engine, session: &str, operation: QueueEvent, at: u64) -> io::Result<()> {
    let _turn = engine.turn(session).await;
    let Ok(_admitted) = crate::session_move::admit(engine, session, None).await else {
        return Ok(());
    };
    if matches!(operation, QueueEvent::Discard) {
        return engine.queues.discard(session).await;
    }
    if matches!(operation, QueueEvent::Started) {
        engine
            .queues
            .put(session, |state| {
                if state.resumed_limit_at.is_some_and(|resumed| resumed <= at) {
                    state.resumed_limit_at = None;
                }
            })
            .await?;
        return Ok(());
    }
    if matches!(operation, QueueEvent::Reopen) {
        engine
            .queues
            .put(session, |state| {
                if state.limit_stop.is_some() {
                    state.release();
                } else {
                    state.paused = false;
                }
            })
            .await?;
        return Ok(());
    }
    let Some(place) = Place::passive(engine, session).await else {
        return Ok(());
    };
    let lease = place.target();
    let engine_held = release_holds(engine, lease);
    let saved = engine.queues.get(session).await?;
    if matches!(operation, QueueEvent::Prune) && saved.held.is_empty() {
        return Ok(());
    }
    if matches!(operation, QueueEvent::Drain)
        && (engine_held
            || saved.paused
            || saved.limit_stop.is_some()
            || !saved.inbox_moved.is_empty())
    {
        return Ok(());
    }
    if let QueueEvent::Hold(limit) | QueueEvent::Failed(limit) = &operation {
        if saved.resumed.is_some_and(|time| time >= at)
            || limit.is_some() && saved.resumed_limit_at.is_some_and(|time| time >= at)
        {
            return Ok(());
        }
        if matches!(operation, QueueEvent::Failed(_)) && !engine_held {
            let page = lease.recent(engine, session, 6).await.unwrap_or_default();
            if page
                .iter()
                .find(|row| row["type"] != "idle")
                .is_some_and(|row| row["type"] == "compaction" && row["status"] == "failed")
            {
                let entries = inbox(engine, lease, session)
                    .await
                    .map_err(io::Error::other)?;
                drain(engine, lease, session, &saved, &entries).await?;
                return Ok(());
            }
        }
        if let Some(limit) = limit
            && !saved.user_paused.unwrap_or_default()
            && (!saved.paused || saved.limit_stop.is_some())
            && limit.resets_at.is_none_or(|reset| reset > disk::now())
        {
            // A failed inbox read must still leave the reason and reset time saved.
            engine
                .queues
                .put(session, |state| {
                    state.paused = true;
                    state.limit_stop = Some(limit.clone());
                })
                .await?;
        }
    }
    let entries = inbox(engine, lease, session)
        .await
        .map_err(io::Error::other)?;
    let waiting = entries
        .iter()
        .filter(|row| row.is_message())
        .collect::<Vec<_>>();
    match operation {
        QueueEvent::Discard | QueueEvent::Started | QueueEvent::Reopen => {
            unreachable!("handled locally")
        }
        QueueEvent::Prune => {
            engine
                .queues
                .put(session, |state| {
                    state.held.retain(|id| {
                        state.waiting.iter().any(|row| row.id == *id)
                            || waiting.iter().any(|row| row.id == *id)
                    })
                })
                .await?;
        }
        QueueEvent::Hold(_) | QueueEvent::Failed(_) => {
            let mut moved = Vec::new();
            for item in waiting
                .iter()
                .filter(|row| row.delivery == Delivery::Steer && !saved.held.contains(&row.id))
            {
                if set_delivery(engine, lease, session, &item.id, Delivery::Queue)
                    .await
                    .is_ok()
                {
                    moved.push(item.id.clone());
                }
            }
            let remaining = inbox(engine, lease, session).await.unwrap_or(entries);
            for summary in remaining.iter().filter(|row| !row.is_message()) {
                let _ = lease.take_back(engine, session, &summary.id).await;
            }
            engine
                .queues
                .put(session, |state| state.keep_held(moved, &remaining))
                .await?;
        }
        QueueEvent::Drain => {
            drain(engine, lease, session, &saved, &entries).await?;
        }
    }
    Ok(())
}

/// Returns whether there is no remaining retry for the head of this queue.
/// The caller owns the session turn while reading and advancing it.
async fn drain(
    engine: &Engine,
    target: Target<'_>,
    session: &str,
    queue: &Queue,
    entries: &[Inbox],
) -> io::Result<bool> {
    if queue.paused || !queue.inbox_moved.is_empty() {
        return Ok(true);
    }
    let order = queue.order(entries);
    let Some(id) = order.first() else {
        return Ok(true);
    };
    advance(engine, target, session, id, Advance::AfterTurn)
        .await
        .map_err(|_| io::Error::other("The next waiting message could not be sent."))
}
