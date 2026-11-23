//! Local queue state and its ordering across the resident and OpenCode inbox.

use std::{
    collections::{BTreeMap, HashSet},
    io,
    path::PathBuf,
    sync::Arc,
};

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
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};

use crate::{
    Engine, api, disk,
    lifetime::{Demand, Lease},
    parts, send, sessions,
};

const ARRIVING_FOR: u64 = 120_000;

#[derive(Clone)]
pub(crate) struct QueueStore {
    run: PathBuf,
    edits: Arc<Semaphore>,
    changes: watch::Sender<u64>,
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
    user_paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit_stop: Option<LimitStop>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inbox_moved: Vec<String>,
    format_version: u64,
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
    id: String,
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
            edits: Arc::new(Semaphore::new(1)),
            changes: watch::channel(0).0,
        }
    }

    async fn index(&self) -> io::Result<Index> {
        let index: Index = disk::read(&self.run.join("queues.json"))
            .await?
            .unwrap_or_default();
        if let Some(queue) = index.values().find(|queue| queue.format_version > 1) {
            return Err(io::Error::other(format!(
                "Conversation queue format version {} is unsupported (this build writes 1).",
                queue.format_version
            )));
        }
        Ok(index)
    }

    pub(crate) async fn get(&self, session: &str) -> io::Result<Queue> {
        Ok(self.index().await?.remove(session).unwrap_or_default())
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
        tokio::task::spawn_blocking(move || {
            let _edit = edit_guard;
            disk::write(&path, &index)?;
            store.notify();
            Ok::<_, io::Error>(())
        })
        .await??;
        Ok(result)
    }

    pub(crate) fn notify(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

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
                if let Some(format) = value.get("formatVersion")
                    && !matches!(format.as_u64(), Some(0 | 1))
                {
                    return Err(io::Error::other("Unsupported waiting message format."));
                }
                serde_json::from_value(value).map_err(io::Error::from)
            })
            .transpose()
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
        before: Option<&str>,
    ) -> io::Result<bool> {
        let current = self.get(session).await?;
        let arriving = current.arriving(&prompt.id);
        if !current.paused
            && !current
                .limit_stop
                .as_ref()
                .is_some_and(|stop| stop.releasing)
            && current.inbox_moved.is_empty()
        {
            return Ok(false);
        }
        if prompt.delivery == Delivery::Steer
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
            let at = before
                .and_then(|before| state.waiting.iter().position(|row| row.id == before))
                .unwrap_or(state.waiting.len());
            state.waiting.insert(at, waiting);
            if arriving {
                state.arriving.remove(&prompt.id);
                state.held.retain(|id| id != &prompt.id);
                let at = state
                    .held
                    .iter()
                    .position(|id| !state.came_held.contains(id))
                    .unwrap_or(state.held.len());
                state.held.insert(at, prompt.id.clone());
                state.came_held.push(prompt.id.clone());
            }
        })
        .await?;
        Ok(true)
    }

    async fn clear(&self, session: &str, id: &str) -> io::Result<Queue> {
        let state = self
            .put(session, |state| {
                state.waiting.retain(|waiting| waiting.id != id);
                state.held.retain(|held| held != id);
                state.order.retain(|ordered| ordered != id);
                state.came_held.retain(|held| held != id);
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
    fn shown_in_overview(&self) -> bool {
        self.paused || !self.held.is_empty() || !self.waiting.is_empty()
    }

    fn has_state(&self) -> bool {
        self.shown_in_overview()
            || !self.order.is_empty()
            || self.resumed.is_some()
            || !self.arriving.is_empty()
            || !self.inbox_moved.is_empty()
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

    pub(crate) fn shown(&self) -> Value {
        let mut value = json!({ "paused": self.paused, "held": self.held });
        if !self.waiting.is_empty() {
            value["waiting"] = json!(self.waiting);
        }
        if self.paused {
            if let Some(stop) = self
                .limit_stop
                .as_ref()
                .filter(|stop| stop.resets_at.is_none())
            {
                value["line"] = json!(format!(
                    "{} The provider didn't say when it resets.",
                    stop.message
                ));
            }
        } else if !self.waiting.is_empty() && !self.inbox_moved.is_empty() {
            value["line"] = json!("Messages wait while Kuvo checks the engine’s inbox.");
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

    fn release(&mut self) {
        self.paused = false;
        self.resumed = Some(disk::now());
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

async fn with_items(engine: &Engine, queue: &Queue, inbox: &[Inbox]) -> Result<Value, Refusal> {
    let mut shown = queue.shown();
    let mut items = Vec::new();
    for id in queue.order(inbox) {
        let held = queue.held.iter().any(|held| held == id);
        if let Some(waiting) = queue.waiting.iter().find(|row| row.id == id) {
            let payload = engine
                .queues
                .waiting_prompt(id)
                .await
                .map_err(save_failed)?;
            let delivery = payload.map_or(Delivery::Queue, |message| message.delivery);
            items.push(
                json!({ "id": id, "words": waiting.text, "files": waiting.files,
                "from": waiting.from, "on": "box", "delivery": delivery, "held": held }),
            );
        } else if let Some(item) = inbox.iter().find(|row| row.id == id) {
            let (words, from) = item.shown();
            items.push(
                json!({ "id": id, "words": words, "files": file_names(item.files()),
                "from": from, "on": "engine", "delivery": item.delivery, "held": held }),
            );
        }
    }
    shown["items"] = json!(items);
    Ok(shown)
}

fn save_failed(error: io::Error) -> Refusal {
    Refusal::new(
        StatusCode::BAD_GATEWAY,
        format!("This computer couldn’t save the queue: {error}"),
    )
}

pub(crate) async fn inbox(
    engine: &Engine,
    lease: &Lease,
    session: &str,
) -> Result<Vec<Inbox>, api::Error> {
    engine
        .call(
            lease,
            Method::GET,
            &format!("{}/inbox", sessions::path(session)),
            None,
        )
        .await
}

pub(crate) async fn reached(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    id: &str,
) -> Result<bool, api::Error> {
    if inbox(engine, lease, session)
        .await?
        .iter()
        .any(|item| item.id == id)
    {
        return Ok(true);
    }
    transcript_has(engine, lease, session, id).await
}

async fn transcript_has(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    id: &str,
) -> Result<bool, api::Error> {
    let path = format!("{}/message/{}", sessions::path(session), api::segment(id));
    match engine.call::<Value>(lease, Method::GET, &path, None).await {
        Ok(message) if message["id"] == id || message["info"]["id"] == id => Ok(true),
        Err(error) if error.missing() => Ok(false),
        Err(error) => Err(error),
        Ok(_) => Err(api::Error::Status(
            StatusCode::BAD_GATEWAY,
            "The engine did not confirm the message by ID.".into(),
        )),
    }
}

async fn command_target(engine: &Engine, session: &str) -> Result<Lease, Refusal> {
    let lease = api::start(engine).await?;
    sessions::read(engine, &lease, &sessions::path(session)).await?;
    Ok(lease)
}

pub(crate) async fn snapshot(
    engine: &Engine,
    session: &str,
    items: bool,
) -> Result<Value, Refusal> {
    let queue = engine.queues.get(session).await.map_err(save_failed)?;
    if !items {
        return Ok(queue.shown());
    }
    let read = async {
        let lease = engine.acquire(Demand::Passive).await?;
        inbox(engine, &lease, session).await.ok()
    };
    if let Some(inbox) = read.await {
        with_items(engine, &queue, &inbox).await
    } else {
        let mut shown = queue.shown();
        shown["items"] = Value::Null;
        Ok(shown)
    }
}

fn queue_failed(error: api::Error) -> Refusal {
    Refusal::new(StatusCode::BAD_GATEWAY, error.to_string())
}

pub(crate) async fn hold_arrival(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    id: &str,
) -> io::Result<()> {
    let rows = inbox(engine, lease, session)
        .await
        .map_err(io::Error::other)?;
    let found = rows.iter().any(|row| row.id == id && row.is_message());
    let mut held = Vec::new();
    if found {
        held.push(id.to_owned());
        for row in rows
            .iter()
            .filter(|row| row.id != id && row.is_message() && row.delivery == Delivery::Steer)
        {
            if set_delivery(engine, lease, session, &row.id, Delivery::Queue)
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
    lease: &Lease,
    session: &str,
    id: &str,
    delivery: Delivery,
) -> Result<(), api::Error> {
    let path = format!("{}/inbox/{}", sessions::path(session), api::segment(id));
    engine
        .call::<Value>(
            lease,
            Method::PATCH,
            &path,
            Some(&json!({ "delivery": delivery })),
        )
        .await?;
    Ok(())
}

pub(crate) async fn interrupt(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    resume: bool,
) -> Result<(), api::Error> {
    let path = format!(
        "{}/interrupt{}",
        sessions::path(session),
        if resume { "?resume=true" } else { "" }
    );
    engine
        .call::<Value>(lease, Method::POST, &path, None)
        .await?;
    Ok(())
}

async fn holding(
    engine: &Engine,
    session: &str,
    given: Vec<String>,
    stopping: bool,
) -> Result<Value, Refusal> {
    let _turn = engine.turn(session).await;
    let lease = command_target(engine, session).await?;
    hold_on(engine, &lease, session, given, stopping).await
}

pub(crate) async fn hold_on(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    given: Vec<String>,
    stopping: bool,
) -> Result<Value, Refusal> {
    if stopping || !given.is_empty() {
        engine
            .queues
            .put(session, |queue| {
                let expires = disk::now() + ARRIVING_FOR;
                queue
                    .arriving
                    .extend(given.iter().map(|id| (id.clone(), expires)));
                if stopping {
                    queue.paused = true;
                    queue.user_paused = Some(true);
                    queue.limit_stop = None;
                }
            })
            .await
            .map_err(save_failed)?;
    }
    let inbox = inbox(engine, lease, session).await.map_err(queue_failed)?;
    let mut held = Vec::new();
    for id in given.iter().chain(
        inbox
            .iter()
            .filter(|row| row.delivery == Delivery::Steer)
            .map(|row| &row.id),
    ) {
        if held.contains(id) {
            continue;
        }
        if let Some(item) = inbox.iter().find(|row| &row.id == id && row.is_message())
            && (item.delivery == Delivery::Queue
                || set_delivery(engine, lease, session, id, Delivery::Queue)
                    .await
                    .is_ok())
        {
            held.push(id.clone());
        }
    }
    for summary in inbox.iter().filter(|row| !row.is_message() && stopping) {
        let path = format!(
            "{}/inbox/{}",
            sessions::path(session),
            api::segment(&summary.id)
        );
        let _ = engine
            .call::<Value>(lease, Method::DELETE, &path, None)
            .await;
    }
    let queue = engine
        .queues
        .put(session, |queue| {
            let ids = &mut held;
            for id in &queue.held {
                if !ids.contains(id)
                    && (queue.contains(id)
                        || inbox.iter().any(|row| &row.id == id && row.is_message()))
                {
                    ids.push(id.clone());
                }
            }
            queue.paused |= inbox.iter().any(Inbox::is_message) || !queue.waiting.is_empty();
            queue.held = held;
        })
        .await
        .map_err(save_failed)?;
    if stopping {
        interrupt(engine, lease, session, false)
            .await
            .map_err(queue_failed)?;
    }
    Ok(queue.shown())
}

pub(crate) async fn advance(
    engine: &Engine,
    lease: &Lease,
    session: &str,
    id: &str,
    immediately: bool,
) -> Result<bool, Refusal> {
    let current = engine.queues.get(session).await.map_err(save_failed)?;
    let active = engine
        .call::<Value>(lease, Method::GET, "api/session/active", None)
        .await
        .map_err(queue_failed)?;
    let running = active
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row == session))
        || active.get(session).is_some_and(|value| !value.is_null());
    if current.contains(id) {
        let Some(mut prompt) = engine
            .queues
            .waiting_prompt(id)
            .await
            .map_err(save_failed)?
        else {
            return Ok(false);
        };
        if !reached(engine, lease, session, id)
            .await
            .map_err(queue_failed)?
        {
            prompt.delivery = Delivery::Steer;
            send::post(engine, lease, session, &prompt).await?;
        }
        engine
            .queues
            .clear(session, id)
            .await
            .map_err(save_failed)?;
        engine
            .queues
            .put(session, Queue::release)
            .await
            .map_err(save_failed)?;
        if running {
            interrupt(engine, lease, session, true)
                .await
                .map_err(queue_failed)?;
        }
        return Ok(true);
    }
    let inbox = inbox(engine, lease, session).await.map_err(queue_failed)?;
    let Some(item) = inbox.iter().find(|item| item.id == id && item.is_message()) else {
        return Ok(false);
    };
    let now = immediately || !running;
    if item.delivery == Delivery::Queue
        && let Err(error) = set_delivery(engine, lease, session, id, Delivery::Steer).await
    {
        if error.missing() {
            return Ok(false);
        }
        return Err(queue_failed(error));
    }
    engine
        .queues
        .put(session, |queue| {
            queue.held.retain(|held| held != id);
            queue.order.retain(|ordered| ordered != id);
            queue.came_held.retain(|held| held != id);
            queue.arriving.clear();
            if now {
                queue.release();
            }
        })
        .await
        .map_err(save_failed)?;
    if now && (running || item.delivery == Delivery::Steer) {
        interrupt(engine, lease, session, true)
            .await
            .map_err(queue_failed)?;
    }
    Ok(true)
}

async fn resume_queue(engine: &Engine, session: &str) -> Result<Value, Refusal> {
    let _turn = engine.turn(session).await;
    let lease = command_target(engine, session).await?;
    let saved = engine
        .queues
        .put(session, Queue::release)
        .await
        .map_err(save_failed)?;
    let inbox = inbox(engine, &lease, session).await.map_err(queue_failed)?;
    let order = saved.order(&inbox);
    if let Some(id) = order.first() {
        advance(engine, &lease, session, id, true).await?;
    }
    Ok(engine
        .queues
        .get(session)
        .await
        .map_err(save_failed)?
        .shown())
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

pub(crate) fn register(engine: &Engine, routes: &mut Registry) {
    let local = engine.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move {
            let session = sessions::id(&call)?;
            let wants = crate::query(&call.request.path, "items").as_deref() == Some("1");
            Ok(RouteReply::json::<200>(sessions::typed(
                snapshot(&engine, &session, wants).await?,
            )?))
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
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    let value = snapshot(&engine, &session, true).await.map_err(|_| io::Error::other("The queue could not be read."))?;
                    if value["items"].is_array() && previous.as_ref() != Some(&value) {
                        previous = Some(value.clone());
                        yield JsonLine::new(serde_json::from_value::<events::Status200>(value)?)?;
                    }
                    tokio::select! {
                        () = call.cancel.cancelled() => break,
                        result = updates.changed() => if result.is_err() { break; },
                        _ = tick.tick() => {},
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
                loop {
                    let index = engine.queues.index().await?;
                    let queues = index.iter().filter(|(_, state)| state.shown_in_overview())
                        .map(|(id, state)| (id, state.shown())).collect::<BTreeMap<_, _>>();
                    let value = json!({ "queues": queues });
                    if previous.as_ref() != Some(&value) {
                        previous = Some(value.clone());
                        yield JsonLine::new(serde_json::from_value::<overview::Status200>(value)?)?;
                    }
                    tokio::select! {
                        () = call.cancel.cancelled() => break,
                        result = updates.changed() => if result.is_err() { break; },
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
        async move { advance_reply(&engine, call, true).await }
    });
    let local = engine.clone();
    routes.register::<steer::Spec, _, _>(Access::Any, move |call| {
        let engine = local.clone();
        async move { advance_reply(&engine, call, false).await }
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
    let items = given
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Ok(RouteReply::json::<200>(sessions::typed(
        holding(engine, &session, items, stopping).await?,
    )?))
}

async fn advance_reply<R>(
    engine: &Engine,
    call: RouteCall<()>,
    immediately: bool,
) -> Result<RouteReply<R>, Refusal>
where
    R: kuvo_protocol::JsonStatus<200>,
    <R as kuvo_protocol::JsonStatus<200>>::Body: serde::de::DeserializeOwned,
{
    let session = sessions::id(&call)?;
    let id = queue_item(&call)?;
    let _turn = engine.turn(&session).await;
    let lease = command_target(engine, &session).await?;
    if !advance(engine, &lease, &session, &id, immediately).await? {
        return Err(not_waiting());
    }
    Ok(RouteReply::json::<200>(sessions::typed(
        engine
            .queues
            .get(&session)
            .await
            .map_err(save_failed)?
            .shown(),
    )?))
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
    let _turn = engine.turn(&session).await;
    let lease = command_target(engine, &session).await?;
    let inbox = inbox(engine, &lease, &session)
        .await
        .map_err(queue_failed)?;
    let saved = engine.queues.get(&session).await.map_err(save_failed)?;
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
        with_items(engine, &queue, &inbox).await?,
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
    disk::durable(spent_path(engine, session, id, token), ()).await
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
    let _turn = engine.turn(&session).await;
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
    let mut lease = engine.acquire(Demand::Passive).await;
    let items = match &lease {
        Some(lease) => inbox(engine, lease, &session).await.ok(),
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
                        Some(lease) => reached(engine, lease, &session, &id).await.unwrap_or(false),
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
            let path = format!("{}/inbox/{}", sessions::path(&session), api::segment(&id));
            if let Err(error) = engine
                .call::<Value>(&lease, Method::DELETE, &path, None)
                .await
                && !error.missing()
            {
                return Err(queue_failed(error));
            }
            if inbox(engine, &lease, &session)
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
            if transcript_has(engine, &lease, &session, &id)
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
        .shown();
    let mut removed = removal.removed;
    removed["attached"] = json!(sendable(
        removed["files"].as_array().map_or(&[], Vec::as_slice)
    ));
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
            .shown(),
    )?))
}

impl Engine {
    /// Keep the old engine's inboxes before a coordinated release switch.
    /// A failed inventory or transfer leaves the engine in place and can be retried.
    pub async fn keep_inboxes(&self) -> io::Result<usize> {
        let Some(lease) = self.acquire(Demand::Existing).await else {
            return Ok(0);
        };
        let ids = inbox_inventory(self, &lease).await?;
        let mut moved = 0;
        for session in ids {
            let _turn = self.turn(&session).await;
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
            if state.paused || state.waiting.is_empty() {
                continue;
            }
            let _turn = self.turn(&session).await;
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
    let rows = inbox(engine, lease, session)
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
    if inbox(engine, lease, session)
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
    let active = engine
        .call::<Value>(lease, Method::GET, "api/session/active", None)
        .await
        .map_err(io::Error::other)?;
    let mut steer = !(active
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row == session))
        || active.get(session).is_some_and(|value| !value.is_null()));
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
        let mut prompt = engine
            .queues
            .waiting_prompt(id)
            .await?
            .ok_or_else(|| io::Error::other("The waiting message's contents are missing."))?;
        if !reached(engine, lease, session, id)
            .await
            .map_err(io::Error::other)?
        {
            if steer {
                prompt.delivery = Delivery::Steer;
            }
            send::post(engine, lease, session, &prompt)
                .await
                .map_err(|_| io::Error::other("The engine didn't confirm the waiting message."))?;
        }
        engine.queues.clear(session, id).await?;
        steer = false;
        sent += 1;
    }
    Ok(sent)
}

pub(crate) fn engine_event(
    engine: &Engine,
    session: &str,
    kind: &str,
    data: &Value,
    stop: tokio_util::sync::CancellationToken,
) {
    if kind.starts_with("session.inbox.") {
        engine.queues.notify();
    }
    let engine = engine.clone();
    let session = session.to_owned();
    let at = disk::now();
    let operation = match kind {
        "session.execution.succeeded" | "session.idle" | "session.inbox.enqueued" => {
            QueueEvent::Drain
        }
        "session.execution.interrupted" => match data["reason"].as_str() {
            Some("superseded" | "inactivity" | "agent-exit") => QueueEvent::Drain,
            Some("restart") => return,
            _ => QueueEvent::Hold,
        },
        "session.execution.failed" => QueueEvent::Failed,
        "session.inbox.delivered" | "session.inbox.cancelled" => QueueEvent::Prune,
        _ => return,
    };
    tokio::spawn(async move {
        // OpenCode can publish a completion before it removes the active record.
        let update = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            react(&engine, &session, operation, at).await
        };
        tokio::select! {
            () = stop.cancelled() => {}
            result = update => if let Err(error) = result {
                eprintln!("Updating the engine's queue: {error}");
            }
        }
    });
}

enum QueueEvent {
    Drain,
    Hold,
    Failed,
    Prune,
}

async fn react(engine: &Engine, session: &str, operation: QueueEvent, at: u64) -> io::Result<()> {
    let _turn = engine.turn(session).await;
    let Some(lease) = engine.acquire(Demand::Passive).await else {
        return Ok(());
    };
    let saved = engine.queues.get(session).await?;
    if matches!(operation, QueueEvent::Prune) && saved.held.is_empty() {
        return Ok(());
    }
    if matches!(operation, QueueEvent::Drain) && (saved.paused || !saved.inbox_moved.is_empty()) {
        return Ok(());
    }
    if matches!(operation, QueueEvent::Hold | QueueEvent::Failed)
        && saved.resumed.is_some_and(|time| time >= at)
    {
        return Ok(());
    }
    let entries = inbox(engine, &lease, session)
        .await
        .map_err(io::Error::other)?;
    let waiting = entries
        .iter()
        .filter(|row| row.is_message())
        .collect::<Vec<_>>();
    match operation {
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
        QueueEvent::Hold | QueueEvent::Failed => {
            if matches!(operation, QueueEvent::Failed) {
                let path = format!("{}/message?limit=6&order=desc", sessions::path(session));
                let page = engine
                    .call::<Vec<Value>>(&lease, Method::GET, &path, None)
                    .await
                    .unwrap_or_default();
                if page
                    .iter()
                    .find(|row| row["type"] != "idle")
                    .is_some_and(|row| row["type"] == "compaction" && row["status"] == "failed")
                {
                    drop(_turn);
                    return Box::pin(react(engine, session, QueueEvent::Drain, at)).await;
                }
            }
            let mut held = saved.held;
            for item in waiting.iter().filter(|row| row.delivery == Delivery::Steer) {
                if set_delivery(engine, &lease, session, &item.id, Delivery::Queue)
                    .await
                    .is_ok()
                    && !held.contains(&item.id)
                {
                    held.push(item.id.clone());
                }
            }
            for summary in entries.iter().filter(|row| !row.is_message()) {
                let path = format!(
                    "{}/inbox/{}",
                    sessions::path(session),
                    api::segment(&summary.id)
                );
                let _ = engine
                    .call::<Value>(&lease, Method::DELETE, &path, None)
                    .await;
            }
            if !waiting.is_empty() || !saved.waiting.is_empty() {
                engine
                    .queues
                    .put(session, |state| {
                        state.paused = true;
                        state.held = held;
                    })
                    .await?;
            }
        }
        QueueEvent::Drain => {
            if waiting.is_empty() && saved.waiting.is_empty() {
                return Ok(());
            }
            let active = engine
                .call::<Value>(&lease, Method::GET, "api/session/active", None)
                .await
                .map_err(io::Error::other)?;
            if active
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row == session))
                || active.get(session).is_some_and(|value| !value.is_null())
            {
                return Ok(());
            }
            if let Some(id) = saved.order(&entries).first() {
                advance(engine, &lease, session, id, true)
                    .await
                    .map_err(|_| io::Error::other("The next waiting message could not be sent."))?;
            }
        }
    }
    Ok(())
}
