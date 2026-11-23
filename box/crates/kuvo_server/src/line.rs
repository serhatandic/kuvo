use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    future::pending,
    sync::Weak,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{
    StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE},
};
use http_body_util::BodyExt;
use kuvo_protocol::routes::{
    get_kuvo_line as get, post_kuvo_line_watch as watch,
    types::{
        GetKuvoLineStatus200Object as Greeting, GetKuvoLineStatus200Object2 as Data,
        GetKuvoLineStatus200Object3 as Refused, GetKuvoLineStatus200Object3Refused as Words,
        GetKuvoLineStatus200Object4 as Gap, GetKuvoLineStatus200Object5 as Heartbeat,
        GetKuvoLineStatus200ObjectHello as Hello,
    },
};
use serde_json::Value;
use tokio::{
    sync::{mpsc, oneshot},
    time::{self, Instant},
};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::{
    Access, Caller, DispatchRequest, Dispatcher, JsonLine, Refusal, Registry, Reply, RouteReply,
};

const LINE: &str = "kuvo/line";
const AGAIN: [u64; 4] = [250, 1_000, 2_000, 5_000];
type Watch = BTreeMap<String, String>;
type LineFrame = JsonLine<get::Spec, 200>;

#[derive(Clone, Copy)]
pub struct LineTiming {
    pub heartbeat: Duration,
    pub retention: Duration,
    pub replay_bytes: usize,
    pub events_quiet: Duration,
    pub once: Duration,
}

impl Default for LineTiming {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(5),
            retention: Duration::from_secs(120),
            replay_bytes: 4 * 1024 * 1024,
            events_quiet: Duration::from_secs(60),
            once: Duration::from_secs(10),
        }
    }
}

#[derive(Clone)]
pub(crate) struct LineHub {
    commands: mpsc::UnboundedSender<Command>,
    timing: LineTiming,
}

pub(crate) struct PendingHub {
    commands: mpsc::UnboundedReceiver<Command>,
    handles: mpsc::WeakUnboundedSender<Command>,
    timing: LineTiming,
}

struct Open {
    id: String,
    after: Option<u64>,
    once: bool,
    watch: Watch,
    request: DispatchRequest,
}

struct Attachment {
    id: String,
    fresh: bool,
    once: bool,
    empty: bool,
    after: u64,
    replay: Vec<Sent>,
    incoming: mpsc::Receiver<Sent>,
    replaced: CancellationToken,
    _drop: Detach,
}

struct Detach {
    commands: mpsc::WeakUnboundedSender<Command>,
    id: String,
    attachment: u64,
}

impl Drop for Detach {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.upgrade() {
            let _ = commands.send(Command::Detach {
                id: std::mem::take(&mut self.id),
                attachment: self.attachment,
            });
        }
    }
}

enum Command {
    Open(Open, oneshot::Sender<std::io::Result<Attachment>>),
    Watch {
        id: String,
        caller: Caller,
        watch: Watch,
        answer: oneshot::Sender<bool>,
    },
    Detach {
        id: String,
        attachment: u64,
    },
}

#[derive(Clone)]
struct Sent {
    n: u64,
    frame: LineFrame,
    last: bool,
}

struct Listener {
    id: u64,
    output: mpsc::Sender<Sent>,
    _stop: DropGuard,
}

struct Topic {
    route: String,
    id: u64,
    _stop: DropGuard,
}

struct Line {
    caller: Caller,
    request: DispatchRequest,
    n: u64,
    sent: VecDeque<Sent>,
    bytes: usize,
    topics: HashMap<String, Topic>,
    listener: Option<Listener>,
    expiry: Option<Instant>,
    once: Option<HashSet<String>>,
}

struct TopicEvent {
    line: String,
    name: String,
    id: u64,
    value: TopicValue,
}

enum TopicValue {
    State(Value),
    Gap,
    Refused(TopicRefusal),
}

#[derive(Clone, PartialEq)]
struct TopicRefusal {
    status: u16,
    message: String,
    said: Option<Value>,
}

struct Hub {
    lines: HashMap<String, Line>,
    serial: u64,
    dispatch: Weak<Registry>,
    topics: mpsc::Sender<TopicEvent>,
    commands: mpsc::WeakUnboundedSender<Command>,
    timing: LineTiming,
}

impl LineHub {
    pub(crate) fn channel(timing: LineTiming) -> (Self, PendingHub) {
        let (send, receive) = mpsc::unbounded_channel();
        let pending = PendingHub {
            commands: receive,
            handles: send.downgrade(),
            timing,
        };
        (
            Self {
                commands: send,
                timing,
            },
            pending,
        )
    }

    pub(crate) fn register(self, registry: &mut Registry) {
        let get = self.clone();
        registry.register::<get::Spec, _, _>(Access::Any, move |call| {
            let hub = get.clone();
            async move {
                let query = call
                    .request
                    .path
                    .split_once('?')
                    .map_or("", |(_, query)| query);
                let fields: Vec<_> = form_urlencoded::parse(query.as_bytes()).collect();
                let field = |name| {
                    fields
                        .iter()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value.as_ref())
                };
                let watch = field("watch")
                    .map_or_else(|| Some(Watch::new()), decode_watch)
                    .filter(valid_watch)
                    .ok_or_else(bad_watch)?;
                let once = field("once") == Some("1");
                let open = Open {
                    id: field("line").unwrap_or_default().to_owned(),
                    after: field("after").map_or(Some(0), cursor),
                    once,
                    watch,
                    request: call.request.topic(String::new()),
                };
                let (send, answer) = oneshot::channel();
                hub.commands
                    .send(Command::Open(open, send))
                    .map_err(|_| unavailable())?;
                let attached = answer
                    .await
                    .map_err(|_| unavailable())?
                    .map_err(|_| unavailable())?;
                let mut answer =
                    RouteReply::ndjson::<200>(stream(attached, hub.timing, call.cancel));
                answer
                    .headers_mut()
                    .insert(CACHE_CONTROL, "no-store".parse().unwrap());
                Ok(answer)
            }
        });
        registry.register_decoded::<watch::Spec, _, _, _>(
            Access::Any,
            |bytes| {
                // A valid watch without a known id asks the client to open a fresh line.
                let mut given: serde_json::Map<String, Value> =
                    serde_json::from_slice(bytes).map_err(|_| bad_watch())?;
                let watch = match given.remove("watch").ok_or_else(bad_watch)? {
                    Value::String(encoded) => decode_watch(&encoded),
                    value => serde_json::from_value(value).ok(),
                }
                .filter(valid_watch)
                .ok_or_else(bad_watch)?;
                Ok(watch::Request {
                    line: given
                        .get("line")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    watch,
                })
            },
            move |call| {
                let hub = self.clone();
                async move {
                    let (send, answer) = oneshot::channel();
                    let watch::Request { line, watch } = call.body;
                    hub.commands
                        .send(Command::Watch {
                            id: line.clone(),
                            caller: call.request.caller,
                            watch,
                            answer: send,
                        })
                        .map_err(|_| unavailable())?;
                    if answer.await.map_err(|_| unavailable())? {
                        Ok(RouteReply::json::<200>(watch::Status200 { line }))
                    } else {
                        Ok(RouteReply::json::<404>(watch::Status404 {
                            fresh: true,
                            message: "Open the line again.".into(),
                        }))
                    }
                }
            },
        );
    }
}

fn cursor(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(0);
    }
    if let Ok(number) = raw.parse::<u64>() {
        return Some(number);
    }
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(digits) = raw.to_ascii_lowercase().strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).ok();
        }
    }
    let number = raw.parse::<f64>().ok()?;
    (number >= 0.0 && number < u64::MAX as f64 && number.fract() == 0.0).then_some(number as u64)
}

fn decode_watch(raw: &str) -> Option<Watch> {
    let bytes = URL_SAFE_NO_PAD.decode(raw.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn valid_watch(watch: &Watch) -> bool {
    watch.iter().all(|(name, route)| {
        name.encode_utf16().count() <= 200
            && !route.is_empty()
            && !route.starts_with('/')
            && !route.starts_with(LINE)
    })
}

fn bad_watch() -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, "Name what this line follows.")
}

fn unavailable() -> Refusal {
    Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "Kuvo is restarting.")
}

impl PendingHub {
    pub(crate) fn run(mut self, dispatch: Weak<Registry>) {
        let (topics, mut events) = mpsc::channel(128);
        let mut hub = Hub {
            lines: HashMap::new(),
            serial: 0,
            dispatch,
            topics,
            commands: self.handles,
            timing: self.timing,
        };
        tokio::spawn(async move {
            loop {
                let expiry = hub.lines.values().filter_map(|line| line.expiry).min();
                tokio::select! {
                    command = self.commands.recv() => match command {
                        Some(Command::Open(open, answer)) => {
                            let _ = answer.send(hub.open(open));
                        }
                        Some(Command::Watch { id, caller, watch, answer }) => {
                            let found = hub.lines.get_mut(&id).filter(|line| line.caller.same_owner(&caller) && line.expiry.is_none_or(|end| end > Instant::now()));
                            let changed = if let Some(line) = found {
                                hub.serial = set_topics(&hub.dispatch, &hub.topics, &id, line, watch, hub.timing, hub.serial);
                                true
                            } else { false };
                            let _ = answer.send(changed);
                        }
                        Some(Command::Detach { id, attachment }) => hub.detach(&id, attachment),
                        None => return,
                    },
                    Some(event) = events.recv() => hub.emit(event),
                    () = wait_until(expiry) => {
                        let now = Instant::now();
                        hub.lines.retain(|_, line| line.expiry.is_none_or(|expiry| expiry > now));
                    },
                }
            }
        });
    }
}

impl Hub {
    fn open(&mut self, open: Open) -> std::io::Result<Attachment> {
        let Open {
            id,
            after,
            once,
            watch,
            request,
        } = open;
        let resuming = !once
            && after.is_some_and(|after| {
                self.lines.get(&id).is_some_and(|line| {
                    line.once.is_none()
                        && line.expiry.is_none_or(|end| end > Instant::now())
                        && line.caller.same_owner(&request.caller)
                        && (after == line.n
                            || (after < line.n
                                && line.sent.front().is_some_and(|first| first.n <= after + 1)))
                })
            });
        let (id, after) = if resuming {
            (id, after.expect("resumable position"))
        } else {
            let id = URL_SAFE_NO_PAD.encode(crate::resident::random::<12>()?);
            self.lines.insert(
                id.clone(),
                Line {
                    caller: request.caller.clone(),
                    request: request.clone(),
                    n: 0,
                    sent: VecDeque::new(),
                    bytes: 0,
                    topics: HashMap::new(),
                    listener: None,
                    expiry: None,
                    once: once.then(|| watch.keys().cloned().collect()),
                },
            );
            (id, 0)
        };
        let line = self.lines.get_mut(&id).expect("open line");
        let empty = once && watch.is_empty();
        line.expiry = None;
        line.request = request;
        let replay = line
            .sent
            .iter()
            .filter(|sent| sent.n > after)
            .cloned()
            .collect();
        let (output, incoming) = mpsc::channel(if once { watch.len().max(1) } else { 64 });
        self.serial += 1;
        let attachment = self.serial;
        let replaced = CancellationToken::new();
        line.listener = Some(Listener {
            id: attachment,
            output,
            _stop: replaced.clone().drop_guard(),
        });
        self.serial = set_topics(
            &self.dispatch,
            &self.topics,
            &id,
            line,
            watch,
            self.timing,
            self.serial,
        );
        let drop = Detach {
            commands: self.commands.clone(),
            id: id.clone(),
            attachment,
        };
        Ok(Attachment {
            id,
            fresh: !resuming,
            once,
            empty,
            after,
            replay,
            incoming,
            replaced,
            _drop: drop,
        })
    }

    fn detach(&mut self, id: &str, attachment: u64) {
        let Some(line) = self.lines.get_mut(id) else {
            return;
        };
        if line
            .listener
            .as_ref()
            .is_some_and(|listener| listener.id != attachment)
        {
            return;
        }
        if line.once.is_some() {
            self.lines.remove(id);
        } else if line.listener.take().is_some() {
            line.expiry = Some(Instant::now() + self.timing.retention);
        }
    }

    fn emit(&mut self, event: TopicEvent) {
        let Some(line) = self.lines.get_mut(&event.line) else {
            return;
        };
        if !line
            .topics
            .get(&event.name)
            .is_some_and(|topic| topic.id == event.id)
        {
            return;
        }
        if let Some(pending) = &mut line.once
            && !pending.remove(&event.name)
        {
            return;
        }
        line.n += 1;
        let n = line.n;
        let t = event.name.clone();
        let value = match event.value {
            TopicValue::State(d) => get::Status200::Object2(Data { t, n: n.into(), d }),
            TopicValue::Gap => get::Status200::Object4(Gap {
                t,
                n: n.into(),
                gap: true,
            }),
            TopicValue::Refused(refusal) => {
                if refusal.status < 500 {
                    line.topics.remove(&event.name);
                }
                let refused = Words {
                    status: refusal.status.into(),
                    message: refusal.message,
                    said: refusal.said,
                };
                get::Status200::Object3(Refused {
                    t,
                    n: n.into(),
                    refused,
                })
            }
        };
        let frame = LineFrame::new(value).expect("line frames contain only JSON values");
        let sent = Sent {
            n,
            frame,
            last: line.once.as_ref().is_some_and(HashSet::is_empty),
        };
        if line.once.is_none() {
            line.bytes += sent.frame.len();
            line.sent.push_back(sent.clone());
            while line.bytes > self.timing.replay_bytes && line.sent.len() > 1 {
                line.bytes -= line
                    .sent
                    .pop_front()
                    .expect("oldest replay entry")
                    .frame
                    .len();
            }
        }
        if let Some(listener) = &line.listener
            && listener.output.try_send(sent).is_err()
        {
            let attachment = listener.id;
            self.detach(&event.line, attachment);
        }
    }
}

fn stream(
    attached: Attachment,
    timing: LineTiming,
    cancel: CancellationToken,
) -> impl futures_core::Stream<Item = std::io::Result<LineFrame>> + Send + 'static {
    async_stream::stream! {
        let Attachment { id, fresh, once, empty, after, replay, mut incoming, replaced, _drop } = attached;
        let until = Instant::now() + timing.once;
        let hello = Hello { line: id, fresh, beat: (timing.heartbeat.as_millis() as u64).into() };
        yield LineFrame::new(get::Status200::Object(Greeting { hello }));
        if empty { return; }
        let mut n = after;
        for sent in replay {
            if replaced.is_cancelled() || cancel.is_cancelled() { return; }
            n = sent.n;
            yield Ok(sent.frame);
        }
        loop {
            let deadline = if once { until } else { Instant::now() + timing.heartbeat };
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = replaced.cancelled() => return,
                () = time::sleep_until(deadline) => {
                    if once { return; }
                    yield LineFrame::new(get::Status200::Object5(Heartbeat { beat: n.into() }));
                }
                sent = incoming.recv() => {
                    let Some(sent) = sent else { return };
                    n = sent.n;
                    yield Ok(sent.frame);
                    if sent.last { return; }
                }
            }
        }
    }
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => time::sleep_until(deadline).await,
        None => pending().await,
    }
}

fn set_topics(
    dispatch: &Weak<Registry>,
    events: &mpsc::Sender<TopicEvent>,
    id: &str,
    line: &mut Line,
    watch: Watch,
    timing: LineTiming,
    mut serial: u64,
) -> u64 {
    line.topics
        .retain(|name, topic| watch.get(name) == Some(&topic.route));
    for (name, route) in watch {
        if line.topics.contains_key(&name) {
            continue;
        }
        serial += 1;
        let stop = CancellationToken::new();
        let task = TopicTask {
            dispatch: dispatch.clone(),
            events: events.clone(),
            line: id.to_owned(),
            name: name.clone(),
            id: serial,
            request: line.request.topic(route.clone()),
            timing,
        };
        line.topics.insert(
            name,
            Topic {
                route,
                id: serial,
                _stop: stop.clone().drop_guard(),
            },
        );
        tokio::spawn(async move {
            tokio::select! {
                () = stop.cancelled() => {},
                () = task.run() => {},
            }
        });
    }
    serial
}

struct TopicTask {
    dispatch: Weak<Registry>,
    events: mpsc::Sender<TopicEvent>,
    line: String,
    name: String,
    id: u64,
    request: DispatchRequest,
    timing: LineTiming,
}

impl TopicTask {
    async fn emit(&self, value: TopicValue) {
        let _ = self
            .events
            .send(TopicEvent {
                line: self.line.clone(),
                name: self.name.clone(),
                id: self.id,
                value,
            })
            .await;
    }

    async fn run(&self) {
        let mut tried = false;
        let mut tries = 0;
        let mut last_state = Vec::new();
        let mut last_refusal = None;
        loop {
            let Some(registry) = self.dispatch.upgrade() else {
                return;
            };
            let cancel = CancellationToken::new();
            let cancel_opening = cancel.clone().drop_guard();
            let reply = Dispatcher::from_registry(registry)
                .dispatch(self.request.clone(), cancel)
                .await;
            if reply.status.is_success() {
                let path = self.request.path.split('?').next().unwrap_or_default();
                let events = matches!(path, "api/event" | "kuvo/agents/api/event")
                    || reply
                        .headers
                        .get(CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|ty| ty.contains("event-stream"));
                if events && tried {
                    self.emit(TopicValue::Gap).await;
                }
                tries = 0;
                last_refusal = None;
                self.read(reply, events, &mut last_state).await;
            } else {
                let status = reply.status;
                let refusal = read_refusal(reply).await;
                if last_refusal.as_ref() != Some(&refusal) {
                    last_refusal = Some(refusal.clone());
                    self.emit(TopicValue::Refused(refusal)).await;
                }
                if status.as_u16() < 500 {
                    return;
                }
            }
            drop(cancel_opening);
            tried = true;
            time::sleep(Duration::from_millis(AGAIN[tries])).await;
            tries = (tries + 1).min(AGAIN.len() - 1);
        }
    }

    async fn read(&self, mut reply: Reply, events: bool, last_state: &mut Vec<u8>) {
        let mut decoder = TopicDecoder {
            events,
            data: Vec::new(),
            last_state,
        };
        let mut pending = Vec::new();
        loop {
            let next = if events {
                match time::timeout(self.timing.events_quiet, reply.body.frame()).await {
                    Ok(next) => next,
                    Err(_) => break,
                }
            } else {
                reply.body.frame().await
            };
            let Some(Ok(frame)) = next else { break };
            let Ok(bytes) = frame.into_data() else {
                continue;
            };
            let mut lines = bytes.split(|byte| *byte == b'\n').peekable();
            while let Some(part) = lines.next() {
                pending.extend_from_slice(part);
                if lines.peek().is_some() {
                    self.decode(&mut decoder, &pending).await;
                    pending.clear();
                }
            }
        }
        if !pending.is_empty() {
            self.decode(&mut decoder, &pending).await;
        }
        if events {
            self.decode(&mut decoder, &[]).await;
        }
    }

    async fn decode(&self, decoder: &mut TopicDecoder<'_>, line: &[u8]) {
        if let Some(value) = decoder.read(line.strip_suffix(b"\r").unwrap_or(line)) {
            self.emit(TopicValue::State(value)).await;
        }
    }
}

struct TopicDecoder<'a> {
    events: bool,
    data: Vec<u8>,
    last_state: &'a mut Vec<u8>,
}

impl TopicDecoder<'_> {
    fn read(&mut self, line: &[u8]) -> Option<Value> {
        if self.events {
            if line.is_empty() {
                return serde_json::from_slice(&std::mem::take(&mut self.data)).ok();
            }
            if let Some(data) = line.strip_prefix(b"data:") {
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data
                    .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
            }
            None
        } else if line == self.last_state.as_slice() {
            None
        } else {
            let value = serde_json::from_slice(line).ok()?;
            self.last_state.clear();
            self.last_state.extend_from_slice(line);
            Some(value)
        }
    }
}

async fn read_refusal(reply: Reply) -> TopicRefusal {
    let status = reply.status.as_u16();
    let raw = reply
        .body
        .collect()
        .await
        .map(|body| body.to_bytes())
        .unwrap_or_default();
    let said = serde_json::from_slice::<Value>(&raw)
        .ok()
        .filter(Value::is_object);
    let message = said
        .as_ref()
        .and_then(|said| said.get("message"))
        .and_then(Value::as_str)
        .map_or_else(
            || String::from_utf8_lossy(&raw).chars().take(500).collect(),
            str::to_owned,
        );
    TopicRefusal {
        status,
        message,
        said,
    }
}
