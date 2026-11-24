//! Event recorder for tool calls, turns, browser downloads, and library handovers,
//! matching `box/src/link/artifacts/recorder.ts` 1:1.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use url::Url;

use super::{
    attachments::attachment_note,
    changes::unapply_exactly,
    kinds::{kind_of, strip_folder_prefix},
    paths::spelled_in,
    store::{ArtifactStore, Clock, GroupRef, Made, MadeWhere, PinBy, Trigger, Turn, Who, WhoKind},
};
use crate::library::store::{Handover, How};

#[derive(Clone, Debug)]
pub struct Event {
    pub event_type: String,
    pub data: Value,
    pub directory: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionDetails {
    pub parent_id: Option<String>,
    pub directory: Option<String>,
    pub metadata: Option<Value>,
}

pub type SessionFuture<'a> = Pin<Box<dyn Future<Output = Option<SessionDetails>> + Send + 'a>>;

pub trait SessionSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn session<'a>(&'a self, id: &'a str) -> SessionFuture<'a>;
    fn agent(&self, id: &str) -> Option<String>;
}

pub type TurnEndedHook = Arc<
    dyn Fn(TurnEnded) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

pub struct TurnEnded {
    pub bot: Option<String>,
    pub folder: Option<String>,
    pub since_ms: i64,
    pub made: Arc<dyn Fn(&str) -> Made + Send + Sync>,
}

pub type HandedHook = Arc<dyn Fn(Handover) + Send + Sync>;

#[derive(Clone, Default)]
struct Info {
    directory: Option<String>,
    parent: Option<String>,
    bot: Option<String>,
    seat: Option<(String, String)>, // (group, bot)
    hidden: bool,
}

#[derive(Clone, Default)]
struct Call {
    name: String,
    input: Value,
}

#[derive(Clone)]
struct LiveTurn {
    started_ms: i64,
    message: Option<String>,
    group: Option<String>,
    trigger: Option<Trigger>,
}

#[derive(Default)]
struct LiveState {
    turn: Option<LiveTurn>,
    calls: HashMap<String, Call>,
}

#[derive(Clone)]
struct RunningBot {
    key: String,
    made: Arc<dyn Fn(&str) -> Made + Send + Sync>,
}

struct RecorderState {
    live: HashMap<String, Arc<tokio::sync::Mutex<LiveState>>>,
    infos: HashMap<String, Info>,
    infos_order: Vec<String>,
    parts: HashMap<String, String>,
    parts_order: Vec<String>,
    wakes: HashSet<String>,
    wakes_order: Vec<String>,
    running: HashMap<String, RunningBot>,
}

impl RecorderState {
    fn new() -> Self {
        Self {
            live: HashMap::new(),
            infos: HashMap::new(),
            infos_order: Vec::new(),
            parts: HashMap::new(),
            parts_order: Vec::new(),
            wakes: HashSet::new(),
            wakes_order: Vec::new(),
            running: HashMap::new(),
        }
    }
}

const REMEMBERED: usize = 500;
const WAKE_MARKER: &str = "[Wake-up] ";

fn remember_map<V>(map: &mut HashMap<String, V>, order: &mut Vec<String>, key: String, value: V) {
    order.retain(|k| k != &key);
    order.push(key.clone());
    map.insert(key, value);
    while map.len() > REMEMBERED && !order.is_empty() {
        let old = order.remove(0);
        map.remove(&old);
    }
}

fn remember_set(set: &mut HashSet<String>, order: &mut Vec<String>, key: String) {
    order.retain(|k| k != &key);
    order.push(key.clone());
    set.insert(key);
    while set.len() > REMEMBERED && !order.is_empty() {
        let old = order.remove(0);
        set.remove(&old);
    }
}

fn is_relevant(event_type: &str) -> bool {
    matches!(
        event_type,
        "session.execution.started"
            | "session.inbox.enqueued"
            | "session.inbox.delivered"
            | "session.tool.input.started"
            | "session.tool.called"
            | "session.tool.success"
            | "session.tool.failed"
            | "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted"
    )
}

fn is_write_tool(name: &str) -> bool {
    matches!(name, "write" | "Write" | "create" | "write_file")
}

fn is_maker_tool(name: &str) -> bool {
    matches!(name, "generate_image" | "browser_screenshot")
}

fn is_read_tool(name: &str) -> bool {
    matches!(name, "read" | "Read" | "read_file" | "view_image")
}

#[derive(Clone)]
pub struct Recorder {
    store: ArtifactStore,
    bots: Option<PathBuf>,
    clock: Option<Clock>,
    turn_ended: Arc<Mutex<Option<TurnEndedHook>>>,
    handed: Option<HandedHook>,
    state: Arc<Mutex<RecorderState>>,
}

#[derive(Clone, Default)]
pub struct RecorderOptions {
    pub bots: Option<PathBuf>,
    pub clock: Option<Clock>,
    pub turn_ended: Option<TurnEndedHook>,
    pub handed: Option<HandedHook>,
}

struct Context {
    made: Arc<dyn Fn(&str) -> Made + Send + Sync>,
    bot: Option<String>,
    folder: Option<String>,
}

impl Recorder {
    pub fn new(store: ArtifactStore, options: RecorderOptions) -> Self {
        Self {
            store,
            bots: options.bots,
            clock: options.clock,
            turn_ended: Arc::new(Mutex::new(options.turn_ended)),
            handed: options.handed,
            state: Arc::new(Mutex::new(RecorderState::new())),
        }
    }

    pub fn set_turn_ended(&self, hook: TurnEndedHook) {
        *self.turn_ended.lock().unwrap() = Some(hook);
    }

    pub fn given(&self, prompt: &str, chat: &str) {
        let st = &mut *self.state.lock().unwrap();
        remember_map(&mut st.parts, &mut st.parts_order, prompt.to_owned(), chat.to_owned());
    }

    pub fn made_in_folder(&self, bot: &str) -> Option<Arc<dyn Fn(&str) -> Made + Send + Sync>> {
        self.state
            .lock()
            .unwrap()
            .running
            .get(bot)
            .map(|r| r.made.clone())
    }

    pub async fn heard(&self, source: &dyn SessionSource, event: Event) {
        let Some(id) = event.data.get("sessionID").and_then(Value::as_str) else {
            return;
        };
        if !is_relevant(&event.event_type) {
            return;
        }
        let id = id.to_owned();
        let key = format!("{}\0{id}", source.name());
        let live_mutex = {
            let mut st = self.state.lock().unwrap();
            st.live
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(LiveState::default())))
                .clone()
        };
        let mut live = live_mutex.lock().await;
        self.handle(source, &id, &key, &mut live, &event).await;
        let idle = live.turn.is_none() && live.calls.is_empty();
        drop(live);
        if idle {
            let mut st = self.state.lock().unwrap();
            if st
                .live
                .get(&key)
                .is_some_and(|entry| Arc::ptr_eq(entry, &live_mutex) && Arc::strong_count(entry) == 2)
            {
                st.live.remove(&key);
            }
        }
    }

    pub async fn noted(
        &self,
        sources: &[Arc<dyn SessionSource>],
        id: &str,
        path: &str,
        via: &str,
    ) -> bool {
        for source in sources {
            if source.session(id).await.is_none() {
                continue;
            }
            let key = format!("{}\0{id}", source.name());
            let live_mutex = self.state.lock().unwrap().live.get(&key).cloned();
            let turn = match live_mutex {
                Some(m) => m.lock().await.turn.clone(),
                None => None,
            };
            if let Some(ctx) = self.context(source.as_ref(), id, turn.as_ref(), None).await {
                let mut made = (ctx.made)(via);
                if via == "download" {
                    made.download = true;
                }
                self.store.change(path, made).await;
            }
            return true;
        }
        false
    }

    async fn handle(
        &self,
        source: &dyn SessionSource,
        id: &str,
        key: &str,
        live: &mut LiveState,
        event: &Event,
    ) {
        let data = &event.data;
        match event.event_type.as_str() {
            "session.execution.started" => {
                let started_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                live.turn = Some(LiveTurn {
                    started_ms,
                    message: None,
                    group: None,
                    trigger: None,
                });
                live.calls.clear();
                self.state.lock().unwrap().infos.remove(key);
                if let Some(ctx) = self
                    .context(source, id, live.turn.as_ref(), event.directory.as_deref())
                    .await
                    && let Some(bot) = ctx.bot
                {
                    self.state.lock().unwrap().running.insert(
                        bot,
                        RunningBot {
                            key: key.to_owned(),
                            made: ctx.made,
                        },
                    );
                }
            }
            "session.inbox.enqueued" => {
                let inbox_id = data.get("inboxID").and_then(Value::as_str);
                let item = data.get("item");
                let payload = item.and_then(|i| i.get("payload"));
                if let (Some(inbox_id), Some(text)) =
                    (inbox_id, payload.and_then(|p| p.get("text")).and_then(Value::as_str))
                    && text.starts_with(WAKE_MARKER)
                {
                    let st = &mut *self.state.lock().unwrap();
                    remember_set(&mut st.wakes, &mut st.wakes_order, inbox_id.to_owned());
                }
                if item.and_then(|i| i.get("type")).and_then(Value::as_str) == Some("user")
                    && self.handed.is_some()
                    && let Some(payload) = payload
                {
                    let attached = attached_paths(payload);
                    if !attached.is_empty()
                        && let Some(ctx) = self
                            .context(source, id, live.turn.as_ref(), event.directory.as_deref())
                            .await
                    {
                        for path in attached {
                            self.emit_handed(
                                &(ctx.made)("attach"),
                                &path,
                                How::Attached,
                                inbox_id,
                                None,
                            );
                        }
                    }
                }
            }
            "session.inbox.delivered" => {
                if let Some(turn) = &mut live.turn
                    && turn.message.is_none()
                    && let Some(inbox_id) = data.get("inboxID").and_then(Value::as_str)
                {
                    turn.message = Some(inbox_id.to_owned());
                    let mut st = self.state.lock().unwrap();
                    if let Some(group) = st.parts.remove(inbox_id) {
                        turn.group = Some(group);
                    }
                    if st.wakes.remove(inbox_id) {
                        turn.trigger = Some(Trigger::Wake);
                    }
                }
            }
            "session.tool.input.started" => {
                if let Some(call_id) = data.get("id").and_then(Value::as_str) {
                    let name = data
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let entry = live.calls.entry(call_id.to_owned()).or_default();
                    entry.name = name;
                }
            }
            "session.tool.called" => {
                if let Some(call_id) = data.get("id").and_then(Value::as_str) {
                    let input = data.get("input").cloned().unwrap_or(Value::Object(Default::default()));
                    let entry = live.calls.entry(call_id.to_owned()).or_default();
                    entry.input = input;
                }
            }
            "session.tool.success" => {
                let call_id = data.get("id").and_then(Value::as_str);
                let call = call_id
                    .and_then(|cid| live.calls.remove(cid))
                    .unwrap_or_default();
                let msg = data.get("assistantMessageID").and_then(Value::as_str);
                self.succeeded(
                    source,
                    id,
                    live.turn.as_ref(),
                    &call,
                    data.get("metadata"),
                    data.get("content"),
                    event.directory.as_deref(),
                    msg,
                    call_id,
                )
                .await;
            }
            "session.tool.failed" => {
                if let Some(call_id) = data.get("id").and_then(Value::as_str) {
                    live.calls.remove(call_id);
                }
            }
            "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted" => {
                let turn = live.turn.clone();
                let hook = self.turn_ended.lock().unwrap().clone();
                if let (Some(turn), Some(hook)) = (turn, hook) {
                    let info = self.info(source, id).await;
                    if info.parent.is_none()
                        && let Some(ctx) = self
                            .context(source, id, Some(&turn), event.directory.as_deref())
                            .await
                    {
                        hook(TurnEnded {
                            bot: ctx.bot,
                            folder: ctx.folder,
                            since_ms: turn.started_ms,
                            made: ctx.made,
                        })
                        .await;
                    }
                }
                live.turn = None;
                live.calls.clear();
                self.state
                    .lock()
                    .unwrap()
                    .running
                    .retain(|_, running| running.key != key);
            }
            _ => {}
        }
    }

    async fn context(
        &self,
        source: &dyn SessionSource,
        id: &str,
        own_turn: Option<&LiveTurn>,
        directory: Option<&str>,
    ) -> Option<Context> {
        let info = self.info(source, id).await;
        if info.hidden {
            return None;
        }
        let (root_id, root_info) = self.root(source, id, info.clone()).await;
        if root_info.hidden {
            return None;
        }
        let turn = if root_id != id {
            let root_key = format!("{}\0{root_id}", source.name());
            let root_live = self.state.lock().unwrap().live.get(&root_key).cloned();
            match root_live {
                Some(m) => m.lock().await.turn.clone().or_else(|| own_turn.cloned()),
                None => own_turn.cloned(),
            }
        } else {
            own_turn.cloned()
        };
        let folder = info
            .directory
            .clone()
            .or_else(|| directory.map(str::to_owned))
            .or_else(|| root_info.directory.clone());
        let bot = root_info
            .seat
            .as_ref()
            .map(|(_, b)| b.clone())
            .or_else(|| root_info.bot.clone());
        let project = folder
            .as_deref()
            .filter(|f| !self.in_bots(f))
            .map(str::to_owned);
        let agent = source.agent(id);
        let session_id = id.to_owned();
        let root_id_for_made = root_id.clone();
        let seat_group = root_info.seat.as_ref().map(|(g, _)| g.clone());
        let is_seat = root_info.seat.is_some();
        let bot_for_made = bot.clone();
        let project_for_made = project.clone();
        let clock = self.clock.clone();

        let made: Arc<dyn Fn(&str) -> Made + Send + Sync> = Arc::new(move |via: &str| {
            let group = seat_group
                .clone()
                .or_else(|| turn.as_ref().and_then(|t| t.group.clone()));
            let trigger = if group.is_some() {
                Some(Trigger::Group)
            } else {
                turn.as_ref().and_then(|t| t.trigger)
            };
            Made {
                by: Who {
                    kind: WhoKind::Agent,
                    session: Some(session_id.clone()),
                    bot: bot_for_made.clone(),
                    agent: agent.clone(),
                },
                turn: turn.as_ref().map(|t| Turn {
                    session: root_id_for_made.clone(),
                    message: t.message.clone(),
                }),
                group: group.as_ref().map(|g| GroupRef {
                    id: g.clone(),
                    message: None,
                }),
                trigger,
                via: via.to_owned(),
                where_: MadeWhere {
                    bot: bot_for_made.clone(),
                    project: project_for_made.clone(),
                    group,
                    session: if is_seat {
                        None
                    } else {
                        Some(root_id_for_made.clone())
                    },
                },
                download: false,
                patch: None,
                separate: false,
                keep: false,
                at: clock.as_ref().map(|c| c()),
            }
        });

        Some(Context { made, bot, folder })
    }

    #[allow(clippy::too_many_arguments)]
    async fn succeeded(
        &self,
        source: &dyn SessionSource,
        id: &str,
        turn: Option<&LiveTurn>,
        call: &Call,
        metadata: Option<&Value>,
        content: Option<&Value>,
        directory: Option<&str>,
        message_id: Option<&str>,
        part_id: Option<&str>,
    ) {
        let Some(ctx) = self.context(source, id, turn, directory).await else {
            return;
        };
        let folder = ctx.folder.as_deref();
        let bots_ref = self.bots.as_deref();
        let absolute = |raw: &str| -> String {
            let path = Path::new(raw);
            let joined = if path.is_absolute() {
                path.to_path_buf()
            } else {
                Path::new(folder.unwrap_or("/")).join(path)
            };
            spelled_in(&joined, &[folder.map(Path::new), bots_ref])
                .to_string_lossy()
                .into_owned()
        };
        let name = tool_name(&call.name);
        let input = &call.input;

        let files = metadata
            .and_then(|m| m.get("files"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for file in &files {
            let Some(raw_file) = file.get("file").and_then(Value::as_str) else {
                continue;
            };
            let path = absolute(raw_file);
            let status = file.get("status").and_then(Value::as_str);
            let patch = file.get("patch").and_then(Value::as_str);
            if self.store.at(&path).is_none()
                && status != Some("added")
                && let Some(patch) = patch
            {
                self.seed_before(&path, patch, status == Some("deleted")).await;
            }
            let mut made = (ctx.made)(&name);
            made.patch = patch.map(str::to_owned);
            self.store.change(&path, made).await;
        }

        let written = input
            .get("path")
            .or_else(|| input.get("filePath"))
            .or_else(|| input.get("file_path"))
            .and_then(Value::as_str);
        if files.is_empty() && is_write_tool(&name) && let Some(written) = written {
            self.store.change(&absolute(written), (ctx.made)(&name)).await;
        }

        let show_screenshot = input.get("show").and_then(Value::as_bool).unwrap_or(false);
        if is_maker_tool(&name) && (name != "browser_screenshot" || show_screenshot) {
            if let Some(parts) = content.and_then(Value::as_array) {
                for part in parts {
                    if part.get("type").and_then(Value::as_str) != Some("file") {
                        continue;
                    }
                    let Some(uri) = part.get("uri").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(file_path) = file_uri_to_path(uri) else {
                        continue;
                    };
                    let path = absolute(&file_path);
                    let made = (ctx.made)(&name);
                    self.store.change(&path, made.clone()).await;
                    if name == "generate_image" {
                        self.emit_handed(&made, &path, How::Generated, message_id, part_id);
                    }
                }
            }
        }

        if is_read_tool(&name) {
            for path in pictures(content, input, &absolute) {
                self.store.change(&path, (ctx.made)("read")).await;
            }
        }

        if (name == "show_file" || name == "show_in_panel")
            && let Some(raw_path) = input.get("path").and_then(Value::as_str)
        {
            self.store.shown(&absolute(raw_path), (ctx.made)(&name)).await;
        }
        if name == "show_file"
            && let Some(raw_path) = input.get("path").and_then(Value::as_str)
        {
            let made = (ctx.made)(&name);
            self.emit_handed(&made, &absolute(raw_path), How::Shown, message_id, part_id);
        }
        if name == "mark_final"
            && let Some(raw_path) = input.get("path").and_then(Value::as_str)
        {
            let path = absolute(raw_path);
            let artifact = match self.store.change(&path, (ctx.made)(&name)).await {
                Some(a) => Some(a),
                None => self.store.at(&path),
            };
            if let Some(artifact) = artifact {
                let note = input
                    .get("note")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                self.store.pin(&artifact.id, Some((PinBy::Bot, note))).await;
            }
        }
    }

    fn emit_handed(
        &self,
        made: &Made,
        path: &str,
        how: How,
        message_id: Option<&str>,
        part_id: Option<&str>,
    ) {
        let Some(hook) = &self.handed else {
            return;
        };
        let own = how == How::Attached || made.by.session == made.where_.session;
        let message = if own {
            message_id.map(str::to_owned)
        } else {
            made.turn.as_ref().and_then(|t| t.message.clone())
        };
        let part = if own {
            part_id.map(str::to_owned)
        } else {
            None
        };
        hook(Handover {
            path: path.to_owned(),
            how,
            bot: made.where_.bot.clone(),
            session: made.where_.session.clone(),
            group: made.where_.group.clone(),
            message,
            part,
            at: made.at,
        });
    }

    async fn seed_before(&self, path: &str, patch: &str, deleted: bool) {
        let now = if deleted {
            String::new()
        } else {
            match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(_) => return,
            }
        };
        if let Some(before) = unapply_exactly(patch, &now)
            && before != now
        {
            self.store.before(path, before.into_bytes(), None, None).await;
        }
    }

    async fn root(
        &self,
        source: &dyn SessionSource,
        id: &str,
        info: Info,
    ) -> (String, Info) {
        let mut current_id = id.to_owned();
        let mut current_info = info;
        for _ in 0..8 {
            let Some(parent) = current_info.parent.clone() else {
                break;
            };
            let parent_info = self.info(source, &parent).await;
            current_id = parent;
            current_info = parent_info;
        }
        (current_id, current_info)
    }

    async fn info(&self, source: &dyn SessionSource, id: &str) -> Info {
        let key = format!("{}\0{id}", source.name());
        if let Some(known) = self.state.lock().unwrap().infos.get(&key).cloned() {
            return known;
        }
        let Some(session) = source.session(id).await else {
            return Info::default();
        };
        let kuvo = session.metadata.as_ref().and_then(|m| m.get("kuvo"));
        let seat = kuvo.and_then(|k| k.get("seat")).and_then(|s| {
            let group = s.get("group").and_then(Value::as_str)?;
            let bot = s.get("bot").and_then(Value::as_str)?;
            Some((group.to_owned(), bot.to_owned()))
        });
        let bot = kuvo
            .and_then(|k| k.get("bot"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| self.bot_of(id, session.directory.as_deref()));
        let hidden = source.name() == "engine"
            && kuvo.and_then(|k| k.get("engineFor")).is_some_and(|v| !v.is_null() && v != &Value::Bool(false));
        let info = Info {
            directory: session.directory,
            parent: session.parent_id,
            bot,
            seat,
            hidden,
        };
        let st = &mut *self.state.lock().unwrap();
        remember_map(&mut st.infos, &mut st.infos_order, key, info.clone());
        info
    }

    fn in_bots(&self, folder: &str) -> bool {
        let Some(bots) = &self.bots else {
            return false;
        };
        let bots_str = bots.to_string_lossy();
        folder == bots_str.as_ref() || strip_folder_prefix(folder, &bots_str).is_some()
    }

    fn bot_of(&self, id: &str, directory: Option<&str>) -> Option<String> {
        let bots = self.bots.as_ref()?;
        let bots_str = bots.to_string_lossy();
        if let Some(dir) = directory
            && let Some(rel) = strip_folder_prefix(dir, &bots_str)
        {
            let first = rel.split(std::path::MAIN_SEPARATOR).next().unwrap_or("");
            if first.starts_with("bot_") {
                return Some(first.to_owned());
            }
        }
        let entries = std::fs::read_dir(bots).ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("bot_") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(bots.join(&name).join("session"))
                && text.trim() == id
            {
                return Some(name);
            }
        }
        None
    }
}

pub fn attached_paths(payload: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Some(files) = payload.get("files").and_then(Value::as_array) {
        for file in files {
            let address = file
                .pointer("/source/uri")
                .or_else(|| file.get("uri"))
                .or_else(|| file.get("url"))
                .and_then(Value::as_str);
            if let Some(addr) = address
                && let Some(path) = file_uri_to_path(addr)
                && seen.insert(path.clone())
            {
                paths.push(path);
            }
        }
    }
    if let Some(text) = payload.get("text").and_then(Value::as_str) {
        for reference in attachment_note(text) {
            if !reference.stayed && seen.insert(reference.path.clone()) {
                paths.push(reference.path);
            }
        }
    }
    paths
}

fn file_uri_to_path(uri: &str) -> Option<String> {
    if !uri.starts_with("file:") {
        return None;
    }
    Url::parse(uri)
        .ok()?
        .to_file_path()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

fn pictures(content: Option<&Value>, input: &Value, absolute: &impl Fn(&str) -> String) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let read = ["path", "filePath", "file_path", "absolute_path"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()));
    let Some(parts) = content.and_then(Value::as_array) else {
        return found;
    };
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("file") {
            continue;
        }
        let Some(uri) = part.get("uri").and_then(Value::as_str) else {
            continue;
        };
        let mime = part
            .get("mime")
            .and_then(Value::as_str)
            .or_else(|| {
                uri.strip_prefix("data:")
                    .and_then(|rest| rest.split([';', ',']).next())
            })
            .unwrap_or("");
        if !mime.starts_with("image/") {
            continue;
        }
        let path = if uri.starts_with("file:") {
            file_uri_to_path(uri)
        } else if uri.starts_with("data:") {
            read.map(str::to_owned)
        } else {
            None
        };
        if let Some(path) = path
            && kind_of(&path, None, false) == super::kinds::Kind::Image
        {
            let abs = absolute(&path);
            if seen.insert(abs.clone()) {
                found.push(abs);
            }
        }
    }
    found
}

pub fn tool_name(name: &str) -> String {
    for own in [
        "show_file",
        "show_in_panel",
        "mark_final",
        "generate_image",
        "browser_screenshot",
    ] {
        if name == own
            || name.ends_with(&format!("_{own}"))
            || name.ends_with(&format!("__{own}"))
        {
            return own.to_owned();
        }
    }
    name.to_owned()
}
