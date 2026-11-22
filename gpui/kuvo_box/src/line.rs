//! The one line to a box (PROTOCOL.md, "The line"; docs/architecture/message-delivery.md, rule 2):
//! one stream per computer that carries everything this client follows there. Each follow is a
//! topic on it, named by the route the box already follows. The box pushes each change, numbered,
//! with a heartbeat. After a drop the line opens again from its last number, and the box sends what
//! it missed. Where the box can't (it started again), every topic starts over from its state now.
//!
//! A drop is never handed to what follows: each keeps the last state it received. Silence on the
//! line is what says a computer is away ([`Line::silence`]), never one slow or failed request.
//!
//! No GPUI here: a thread of its own reads the line, and hands each topic what it says.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

use crate::error::Refusal;
use crate::lines;
use crate::transport::{Closer, Request, Transport};

/// How long the line waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineTiming {
    /// With nothing heard for this long (the box beats every 5 s), the line counts as dropped
    /// and is opened again.
    pub quiet: Duration,
    /// After a failed opening, before trying again; doubled each time, up to `most`.
    pub retry_after: Duration,
    pub most: Duration,
    /// Over a transport that can't keep a stream, how often every topic is asked again.
    pub once_again: Duration,
    /// How long the line stays open after its last topic goes, for one that comes right after.
    pub linger: Duration,
}

impl Default for LineTiming {
    fn default() -> LineTiming {
        LineTiming { quiet: Duration::from_secs(15), retry_after: Duration::from_millis(500), most: Duration::from_secs(10), once_again: Duration::from_secs(5), linger: Duration::from_secs(2) }
    }
}

/// What a topic hears.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Said {
    /// What the route said: a state, or an event.
    Data(Value),
    /// Its events may have a gap (their source dropped and came back): read what they're of again.
    Gap,
    /// The line started over: what comes next is the state now. What was missed isn't sent again.
    Fresh,
    /// The box turned the topic down.
    Refused(Refusal),
}

type Sink = Arc<dyn Fn(Said) + Send + Sync>;

struct Topic {
    route: String,
    sink: Sink,
}

#[derive(Default)]
struct State {
    topics: BTreeMap<String, Topic>,
    next: u64,
    /// Changed each time the topics do, and the change the box has.
    version: u64,
    told: u64,
    /// The line the box gave, and the last number heard on it.
    line: Option<String>,
    after: u64,
    running: bool,
    posting: bool,
    /// What closes the stream open now.
    closer: Option<Closer>,
    /// When something last came on the line, and since when it's been tried.
    heard: Option<Instant>,
    since: Option<Instant>,
    /// Since when it has had no topics.
    empty: Option<Instant>,
}

/// The line to one box. Every client of the same transport shares it ([`Line::of`]).
pub struct Line {
    transport: Arc<dyn Transport>,
    timing: Mutex<LineTiming>,
    state: Mutex<State>,
    woken: Condvar,
}

/// A topic followed on a line. Dropping it ends it.
pub(crate) struct Watch {
    line: Arc<Line>,
    name: String,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.line.unwatch(&self.name);
    }
}

type Lines = Mutex<Vec<(usize, Weak<Line>)>>;

fn lines() -> &'static Lines {
    static LINES: std::sync::OnceLock<Lines> = std::sync::OnceLock::new();
    LINES.get_or_init(Lines::default)
}

impl Line {
    /// The line to the box `transport` reaches: one for each, however many clients hold it.
    pub fn of(transport: &Arc<dyn Transport>) -> Arc<Line> {
        let key = Arc::as_ptr(transport) as *const () as usize;
        let mut all = lines().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        all.retain(|(_, line)| line.strong_count() > 0);
        if let Some(line) = all.iter().find(|(at, _)| *at == key).and_then(|(_, line)| line.upgrade())
            && Arc::ptr_eq(&line.transport, transport)
        {
            return line;
        }
        let line = Arc::new(Line { transport: transport.clone(), timing: Mutex::new(LineTiming::default()), state: Mutex::default(), woken: Condvar::new() });
        all.push((key, Arc::downgrade(&line)));
        line
    }

    /// Waits as `timing` says from now on (a test's own).
    pub fn set_timing(&self, timing: LineTiming) {
        *self.timing.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = timing;
        self.woken.notify_all();
    }

    fn timing(&self) -> LineTiming {
        *self.timing.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How long the line has been silent while it's wanted: since it last heard anything, or, never
    /// heard yet, since it was first tried. None while nothing's followed.
    pub fn silence(&self) -> Option<Duration> {
        let state = self.state();
        if !state.running {
            return None;
        }
        state.heard.or(state.since).map(|at| at.elapsed())
    }

    /// Whether the line has been silent for `long` or more while it's wanted.
    pub fn silent_for(&self, long: Duration) -> bool {
        self.silence().is_some_and(|silence| silence >= long)
    }

    /// Whether it's been heard within `recent`.
    pub fn heard_within(&self, recent: Duration) -> bool {
        let state = self.state();
        state.running && state.heard.is_some_and(|heard| heard.elapsed() < recent)
    }

    /// Follows `route` on the line: `sink` hears each thing it says, on the line's own thread, until
    /// the [`Watch`] goes.
    pub(crate) fn watch(self: &Arc<Self>, route: String, sink: impl Fn(Said) + Send + Sync + 'static) -> Watch {
        let (name, start) = {
            let mut state = self.state();
            state.next += 1;
            let name = state.next.to_string();
            state.topics.insert(name.clone(), Topic { route, sink: Arc::new(sink) });
            state.version += 1;
            state.empty = None;
            let start = !state.running;
            if start {
                state.running = true;
                state.since = Some(Instant::now());
                state.heard = None;
            }
            (name, start)
        };
        if start {
            let line = self.clone();
            if thread::Builder::new().name("kuvo-box line".into()).spawn(move || line.run()).is_err() {
                self.state().running = false;
            }
        } else {
            self.tell();
        }
        self.woken.notify_all();
        Watch { line: self.clone(), name }
    }

    fn unwatch(self: &Arc<Self>, name: &str) {
        let mut state = self.state();
        if state.topics.remove(name).is_some() {
            state.version += 1;
            if state.topics.is_empty() {
                state.empty = Some(Instant::now());
            }
        }
        drop(state);
        self.woken.notify_all();
        self.tell();
    }

    /// The topics as the box is told them: `{name: route}`.
    fn topics(state: &State) -> Value {
        Value::Object(state.topics.iter().map(|(name, topic)| (name.clone(), Value::String(topic.route.clone()))).collect())
    }

    /// Tells the box the topics as they are now, on a line it has open (`kuvo/line/watch`): one
    /// request at a time, the latest. One it doesn't take opens the line again, with them.
    fn tell(self: &Arc<Self>) {
        let mut state = self.state();
        if state.posting || state.line.is_none() || state.closer.is_none() || state.told == state.version || !self.transport.streams() {
            return;
        }
        state.posting = true;
        drop(state);
        let transport = self.transport.clone();
        let line = self.clone();
        let _ = thread::Builder::new().name("kuvo-box line watch".into()).spawn(move || {
            loop {
                let (id, watch, version) = {
                    let mut state = line.state();
                    match state.line.clone() {
                        Some(id) if state.told != state.version && state.closer.is_some() => (id, Self::topics(&state), state.version),
                        _ => {
                            state.posting = false;
                            return;
                        }
                    }
                };
                let taken = transport.send(&Request::post("kuvo/line/watch", json!({ "line": id, "watch": watch }))).is_ok_and(|answer| answer.is_success());
                let mut state = line.state();
                if taken {
                    if state.line.as_deref() == Some(id.as_str()) {
                        state.told = state.told.max(version);
                    }
                } else {
                    // Opened again, with every topic in its request.
                    state.posting = false;
                    if let Some(closer) = state.closer.take() {
                        drop(state);
                        closer.close();
                    }
                    return;
                }
            }
        });
    }

    fn heard(&self) {
        self.state().heard = Some(Instant::now());
    }

    fn sink(&self, name: &str) -> Option<Sink> {
        self.state().topics.get(name).map(|topic| topic.sink.clone())
    }

    fn sinks(&self) -> Vec<Sink> {
        self.state().topics.values().map(|topic| topic.sink.clone()).collect()
    }

    /// Whether it's ended: no topics for a moment. Ends it then.
    fn ended(&self) -> bool {
        let linger = self.timing().linger;
        let mut state = self.state();
        if state.topics.is_empty() && state.empty.is_none_or(|empty| empty.elapsed() >= linger) {
            state.running = false;
            state.line = None;
            state.after = 0;
            state.closer = None;
            return true;
        }
        false
    }

    /// Waits `time`, or until a topic comes back (for an empty line).
    fn wait(&self, time: Duration) {
        let state = self.state();
        let _ = self.woken.wait_timeout(state, time);
    }

    /// Reads the line, opening it again as it drops, until it's ended.
    fn run(self: Arc<Self>) {
        let mut retry = self.timing().retry_after;
        // The watchdog: a line silent too long is closed, so it's opened again.
        let watchdog = Arc::downgrade(&self);
        let _ = thread::Builder::new().name("kuvo-box line quiet".into()).spawn(move || {
            while let Some(line) = watchdog.upgrade() {
                let quiet = line.timing().quiet;
                let mut state = line.state();
                if !state.running {
                    return;
                }
                let last = state.heard.into_iter().chain(state.since).max();
                if state.closer.is_some() && last.is_some_and(|last| last.elapsed() >= quiet) {
                    let closer = state.closer.take();
                    // Counted from now: it's opened again before it's called quiet again.
                    state.since = Some(Instant::now());
                    drop(state);
                    if let Some(closer) = closer {
                        closer.close();
                    }
                    continue;
                }
                let _ = line.woken.wait_timeout(state, (quiet / 4).max(Duration::from_millis(5)));
            }
        });
        loop {
            if self.ended() {
                self.woken.notify_all();
                return;
            }
            let streams = self.transport.streams();
            let (path, resumed, version) = {
                let mut state = self.state();
                let watch = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Self::topics(&state).to_string());
                let mut path = format!("kuvo/line?watch={watch}");
                if !streams {
                    path.push_str("&once=1");
                } else if let Some(line) = &state.line {
                    path.push_str(&format!("&line={}&after={}", crate::paths::query_value(line), state.after));
                }
                state.since = Some(Instant::now());
                (path, state.line.is_some(), state.version)
            };
            let opened = self.transport.open(&Request::get(path), &|closer| {
                let mut state = self.state();
                if !state.running {
                    return false;
                }
                state.closer = Some(closer);
                true
            });
            let mut heard_any = false;
            match opened {
                Ok(mut stream) if (200..300).contains(&stream.status) => {
                    loop {
                        let text = match lines::next_line(&mut *stream.reader) {
                            Ok(Some(text)) => text,
                            Ok(None) | Err(_) => break,
                        };
                        let Ok(said) = serde_json::from_str::<Value>(&text) else { continue };
                        self.heard();
                        heard_any = true;
                        retry = self.timing().retry_after;
                        self.take(&said, resumed, version, streams);
                        if self.state().topics.is_empty() && self.ended_soon() {
                            stream.closer.close();
                            break;
                        }
                    }
                }
                Ok(mut stream) => {
                    // Turned down whole (a box from before the line, or one not ready yet): tried again.
                    let mut body = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut stream.reader, &mut body);
                    let refusal = Refusal::from_answer(stream.status, &body);
                    if stream.status == 404 || stream.status == 405 {
                        for sink in self.sinks() {
                            sink(Said::Refused(refusal.clone()));
                        }
                    }
                }
                Err(_) => {}
            }
            self.state().closer = None;
            if !streams {
                self.state().line = None;
                self.wait(self.timing().once_again);
                continue;
            }
            // Dropped after it had come through: opened again at once, from where it was.
            if !heard_any {
                let timing = self.timing();
                self.wait(retry);
                retry = (retry * 2).min(timing.most).max(timing.retry_after);
            }
        }
    }

    /// Whether the topics have been gone long enough to end the line.
    fn ended_soon(&self) -> bool {
        let linger = self.timing().linger;
        self.state().empty.is_some_and(|empty| empty.elapsed() >= linger)
    }

    /// Takes one thing the line said.
    fn take(self: &Arc<Self>, said: &Value, resumed: bool, version: u64, streams: bool) {
        if let Some(hello) = said.get("hello") {
            let fresh = hello.get("fresh").and_then(Value::as_bool).unwrap_or(true);
            {
                let mut state = self.state();
                state.line = streams.then(|| hello.get("line").and_then(Value::as_str).map(str::to_owned)).flatten();
                if fresh {
                    state.after = 0;
                }
                state.told = version;
            }
            // Started over where it was resumed: what was missed isn't coming.
            if fresh && resumed {
                for sink in self.sinks() {
                    sink(Said::Fresh);
                }
            }
            self.tell();
            return;
        }
        if let Some(n) = said.get("n").and_then(Value::as_u64) {
            let mut state = self.state();
            state.after = state.after.max(n);
        }
        let Some(name) = said.get("t").and_then(Value::as_str) else { return };
        let Some(sink) = self.sink(name) else { return };
        if let Some(data) = said.get("d") {
            sink(Said::Data(data.clone()));
        } else if said.get("gap").and_then(Value::as_bool) == Some(true) {
            sink(Said::Gap);
        } else if let Some(refused) = said.get("refused") {
            let status = refused.get("status").and_then(Value::as_u64).and_then(|status| u16::try_from(status).ok()).unwrap_or(500);
            let body = refused.get("said").map(Value::to_string).unwrap_or_else(|| json!({ "message": refused.get("message").cloned().unwrap_or_default() }).to_string());
            sink(Said::Refused(Refusal::from_answer(status, body.as_bytes())));
        }
    }
}

/// The line for a box that answers each followed route on its own, in this process (a stand-in:
/// the app's demo copy, its shots, tests). It does what the box's `kuvo/line` does, for its routes:
/// opens each topic's route, and passes on what it says, numbered. It never resumes: a line it's
/// asked to resume starts over. A real box answers `kuvo/line` itself (`box/src/link/server/line.ts`).
pub mod by_routes {
    use std::collections::HashMap;
    use std::io::{BufReader, Read};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use base64::Engine;
    use serde_json::{Value, json};

    use crate::error::Result;
    use crate::lines;
    use crate::transport::{Closer, Request, Response, Stream};

    /// How a stand-in opens one of its own followed routes.
    pub type Open = Arc<dyn Fn(&Request, &dyn Fn(Closer) -> bool) -> Result<Stream> + Send + Sync>;

    struct Served {
        open: Open,
        out: Sender<Vec<u8>>,
        n: Arc<AtomicU64>,
        /// Each topic's route and what ends it.
        topics: HashMap<String, (String, Closer)>,
        ended: Arc<std::sync::atomic::AtomicBool>,
    }

    fn served() -> &'static Mutex<HashMap<String, Served>> {
        static SERVED: std::sync::OnceLock<Mutex<HashMap<String, Served>>> = std::sync::OnceLock::new();
        SERVED.get_or_init(Mutex::default)
    }

    fn watch_of(value: &Value) -> Vec<(String, String)> {
        value.as_object().map(|topics| topics.iter().filter_map(|(name, route)| Some((name.clone(), route.as_str()?.to_owned()))).collect()).unwrap_or_default()
    }

    fn query<'a>(path: &'a str, name: &str) -> Option<&'a str> {
        path.split_once('?')?.1.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    }

    fn send_line(out: &Sender<Vec<u8>>, n: &AtomicU64, mut value: Value) {
        value["n"] = json!(n.fetch_add(1, Ordering::SeqCst) + 1);
        let _ = out.send(format!("{value}\n").into_bytes());
    }

    /// Starts topic `name`, reading `route` on a thread of its own.
    fn start(served: &mut Served, name: String, route: String) {
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let kept: Arc<Mutex<Option<Closer>>> = Arc::default();
        let closer = {
            let (ended, kept) = (ended.clone(), kept.clone());
            Closer::new(move || {
                ended.store(true, Ordering::SeqCst);
                if let Some(closer) = kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
                    closer.close();
                }
            })
        };
        served.topics.insert(name.clone(), (route.clone(), closer));
        let (open, out, n, all) = (served.open.clone(), served.out.clone(), served.n.clone(), served.ended.clone());
        let _ = thread::Builder::new().name("stand-in line topic".into()).spawn(move || {
            let gone = || ended.load(Ordering::SeqCst) || all.load(Ordering::SeqCst);
            let hold = |closer: Closer| {
                *kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(closer);
                !gone()
            };
            // As the box runs a topic: one that ends is opened again (an event stream saying there may
            // be a gap), one turned down is said once, and ends unless it's only not ready (5xx).
            let mut attempt = 0;
            let mut refused_last: Option<String> = None;
            while !gone() {
                match open(&Request::get(route.clone()), &hold) {
                    Ok(mut stream) if (200..300).contains(&stream.status) => {
                        refused_last = None;
                        let events = stream.content_type.as_deref().is_some_and(|kind| kind.contains("event-stream")) || matches!(route.split('?').next(), Some("api/event" | "kuvo/agents/api/event"));
                        if events && attempt > 0 && !gone() {
                            send_line(&out, &n, json!({ "t": name, "gap": true }));
                        }
                        loop {
                            let read = if events { lines::next_event(&mut *stream.reader) } else { lines::next_line(&mut *stream.reader) };
                            let Ok(Some(text)) = read else { break };
                            if gone() {
                                return;
                            }
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                send_line(&out, &n, json!({ "t": name, "d": value }));
                            }
                        }
                    }
                    Ok(mut stream) => {
                        let mut body = Vec::new();
                        let _ = stream.reader.read_to_end(&mut body);
                        let said: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                        let message = said.get("message").and_then(Value::as_str).unwrap_or_default().to_owned();
                        let refused = json!({ "status": stream.status, "message": message, "said": said });
                        if !gone() && refused_last.as_deref() != Some(refused.to_string().as_str()) {
                            send_line(&out, &n, json!({ "t": name, "refused": refused }));
                        }
                        refused_last = Some(refused.to_string());
                        if stream.status < 500 {
                            return;
                        }
                    }
                    Err(_) => {}
                }
                attempt += 1;
                if gone() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    }

    fn set(served: &mut Served, watch: Vec<(String, String)>) {
        let wanted: HashMap<String, String> = watch.into_iter().collect();
        served.topics.retain(|name, (route, closer)| {
            let keep = wanted.get(name) == Some(route);
            if !keep {
                closer.close();
            }
            keep
        });
        for (name, route) in wanted {
            if !served.topics.contains_key(&name) {
                start(served, name, route);
            }
        }
    }

    struct Pipe(Receiver<Vec<u8>>, Vec<u8>);

    impl Read for Pipe {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(bytes) => self.1 = bytes,
                    Err(_) => return Ok(0),
                }
            }
            let count = buffer.len().min(self.1.len());
            buffer[..count].copy_from_slice(&self.1[..count]);
            self.1.drain(..count);
            Ok(count)
        }
    }

    /// `GET kuvo/line`, answered from the stand-in's own routes (`open`): none for any other request.
    pub fn open(request: &Request, hold: &dyn Fn(Closer) -> bool, open: Open) -> Option<Result<Stream>> {
        if request.route() != "kuvo/line" {
            return None;
        }
        let watch = query(&request.path, "watch")
            .and_then(|given| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(given).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .map(|value| watch_of(&value))
            .unwrap_or_default();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = format!("stand-in-{}", NEXT.fetch_add(1, Ordering::SeqCst));
        let (out, receiver) = channel();
        let _ = out.send(format!("{}\n", json!({ "hello": { "line": id, "fresh": true, "beat": 5000 } })).into_bytes());
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let closer = {
            let (id, ended) = (id.clone(), ended.clone());
            Closer::new(move || {
                ended.store(true, Ordering::SeqCst);
                if let Some(served) = served().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&id) {
                    for (_, (_, closer)) in served.topics {
                        closer.close();
                    }
                    // (Its pipe ends as the last sender goes.)
                    drop(served.out);
                }
            })
        };
        if !hold(closer.clone()) {
            closer.close();
            return Some(Err(crate::Error::Unreachable("The connection closed.".into())));
        }
        // A heartbeat, as a box's line beats while it's quiet.
        {
            let (out, ended) = (out.clone(), ended.clone());
            let _ = thread::Builder::new().name("stand-in line beat".into()).spawn(move || {
                while !ended.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_secs(5));
                    if out.send(format!("{}\n", json!({ "beat": 0 })).into_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
        let mut line = Served { open, out, n: Arc::new(AtomicU64::new(0)), topics: HashMap::new(), ended };
        set(&mut line, watch);
        served().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(id, line);
        Some(Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(BufReader::new(Pipe(receiver, Vec::new()))), closer }))
    }

    /// `POST kuvo/line/watch`, for a line [`open`] answered: none for any other request.
    pub fn send(request: &Request) -> Option<Result<Response>> {
        if request.route() != "kuvo/line/watch" {
            return None;
        }
        let body = request.body.clone().unwrap_or(Value::Null);
        let id = body.get("line").and_then(Value::as_str).unwrap_or_default().to_owned();
        let mut all = served().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let answer = |status: u16, value: Value| Some(Ok(Response { status, content_type: Some("application/json".into()), body: value.to_string().into_bytes() }));
        let Some(line) = all.get_mut(&id) else { return answer(404, json!({ "fresh": true, "message": "Open the line again." })) };
        set(line, watch_of(&body["watch"]));
        answer(200, json!({ "line": id }))
    }
}
