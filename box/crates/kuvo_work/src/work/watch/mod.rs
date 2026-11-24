//! A conversation hears of the sessions it started (PROTOCOL.md, "Work"): as one finishes, fails or
//! asks for something, the box writes to it, as a wake-up does, so it never checks on them. Those
//! that come close together go as one message, kept in `run/work/wakes.json` until they're sent,
//! so none is lost to the box starting again; `run/work/told.json` says what each starter was
//! told. Each changes its bot's work, for clients to read again.

pub mod background;
mod catchup;
mod held;
pub mod lines;
pub mod services;
pub mod told;
pub mod turns;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use http::Method;
use kuvo_engine::{BotEngine, BoxMessage, EngineEvent};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{
    Work,
    unanswering::{ModelRef, error_of, hold_words},
};
use lines::{STOPPED_OPENING, wake_line_shown, wake_text};
use told::{ENGINE, Kept, Note, Tried, Waiting, now};
use turns::{BeganBy, LastReply};

/// How long the box gathers what's to tell a conversation before it writes.
pub const GATHER: Duration = Duration::from_secs(4);
const WAIT: Duration = Duration::from_secs(10);
// How long what a conversation would hear waits while it's archived.
const ARCHIVED_FOR: u64 = 7 * 86_400_000;
// How long what a conversation no one here runs now would hear is kept.
const NOTE_MEMORY: u64 = 30 * 86_400_000;
const EVENTS: &[&str] = &[
    "session.inbox.delivered",
    "session.execution.started",
    "session.execution.succeeded",
    "session.execution.failed",
    "session.execution.interrupted",
    "permission.asked",
    "form.created",
    "permission.replied",
    "form.replied",
];
static ADDRESS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\(address (\S+?)\)").expect("a pattern"));

fn address_in(line: &str) -> Option<&str> {
    Some(ADDRESS.captures(line)?.get(1)?.as_str())
}

/// Who started a session, as it's marked; none for one started by you.
pub fn started_by(session: &Value) -> Option<&str> {
    session["metadata"]["kuvo"]["startedBy"]
        .as_str()
        .filter(|by| !by.is_empty())
}

/// The bot a session works for.
pub fn works_for(session: &Value) -> Option<&str> {
    let by = &session["metadata"]["kuvo"]["by"];
    by["name"].is_string().then(|| by["bot"].as_str()).flatten()
}

fn wakes_of(session: &Value) -> Vec<&str> {
    let wake = session["metadata"]["kuvo"]["wake"]
        .as_array()
        .into_iter()
        .flatten();
    wake.filter_map(Value::as_str).collect()
}

/// Whether a session was put away, in any of the ways a client marks it.
pub fn is_archived(session: &Value) -> bool {
    let metadata = &session["metadata"];
    let time = &session["time"]["archived"];
    (!time.is_null() && time != false)
        || metadata["kuvo.archived"] == true
        || truthy(&metadata["kuvo"]["archived"])
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false)) && value != 0 && value != ""
}

fn encoded(id: &str) -> String {
    percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Marks the watcher over as its task ends: what it was still to look at later isn't.
struct Over(Arc<Inner>);

impl Drop for Over {
    fn drop(&mut self) {
        self.0.over.store(true, Ordering::Relaxed);
    }
}

/// The work watcher: what's gathered for each conversation, sent one message at a time.
#[derive(Clone)]
pub struct Watcher {
    inner: Arc<Inner>,
}

struct Inner {
    work: Work,
    kept: Kept,
    due: Mutex<HashSet<String>>,
    sending: Mutex<HashSet<String>>,
    failures: Mutex<HashMap<String, u32>>,
    background: background::EngineBackground,
    // Finishes held for a starter's own wait, those looked at while they wait, and those settling.
    holding: Mutex<HashSet<String>>,
    looking: Mutex<HashSet<String>>,
    settling: Mutex<HashSet<String>>,
    // A bot's sessions nobody is told of, shown to their bot once what they left running is over.
    mark_later: Mutex<HashMap<String, String>>,
    // Whether what happened while the box was down has been looked at: until then, what it hears
    // doesn't move when it last heard of everything.
    caught: AtomicBool,
    over: AtomicBool,
}

impl Inner {
    fn caught(&self) {
        self.caught.store(true, Ordering::Relaxed);
    }

    fn over(&self) -> bool {
        self.over.load(Ordering::Relaxed)
    }
}

impl Work {
    /// Lines for conversations from outside the watcher (a stop), each kept until it's sent with
    /// what else waits for it, as one message.
    pub fn tell_starters(&self, told: &[(String, String)]) {
        if told.is_empty() {
            return;
        }
        for starter in Kept::new(&self.home.join("run")).add_lines(told) {
            let _ = self.wakes.send(starter);
        }
    }

    /// What `starter` was still to hear of its sessions, dropped (Stop All).
    pub fn forget_wakes(&self, starter: &str) {
        Kept::new(&self.home.join("run")).forget_wakes(starter);
    }
}

impl Watcher {
    pub fn new(work: Work) -> Self {
        let kept = Kept::new(&work.home.join("run"));
        let inner = Arc::new_cyclic(|this: &std::sync::Weak<Inner>| {
            let this = this.clone();
            // (What runs stopping counting at its limit is looked at as an end is.)
            let ran_out = Arc::new(move || {
                if let Some(inner) = this.upgrade() {
                    Watcher { inner }.background_ended();
                }
            });
            Inner {
                background: background::EngineBackground::new(work.clone(), ran_out),
                work,
                kept,
                due: Mutex::default(),
                sending: Mutex::default(),
                failures: Mutex::default(),
                holding: Mutex::default(),
                looking: Mutex::default(),
                settling: Mutex::default(),
                mark_later: Mutex::default(),
                caught: AtomicBool::new(false),
                over: AtomicBool::new(false),
            }
        });
        Self { inner }
    }

    fn work(&self) -> &Work {
        &self.inner.work
    }

    fn kept(&self) -> &Kept {
        &self.inner.kept
    }

    /// Follows the engine's events, and sends what's added from outside; what a box left waiting as
    /// it stopped is sent now.
    pub fn start(self, mut added: mpsc::UnboundedReceiver<String>) -> tokio::task::JoinHandle<()> {
        let engine = self.work().list.engine().clone();
        let (mut events, mut states) = (engine.events(), engine.states());
        for starter in self.kept().waiting().keys() {
            self.soon(starter, GATHER);
        }
        let before = self.kept().told();
        // A box with nothing told yet has heard of everything until now.
        if before.is_none() {
            self.kept().change_told(|_| {}, Some(ENGINE), now());
        }
        self.catch_up(before);
        tokio::spawn(async move {
            let _over = Over(self.inner.clone());
            loop {
                tokio::select! {
                    starter = added.recv() => match starter {
                        Some(starter) => self.soon(&starter, GATHER),
                        None => return,
                    },
                    changed = states.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        // The engine stopped: what ran there went with it; started, looked at again.
                        let running = matches!(*states.borrow_and_update(), kuvo_engine::State::Running { .. });
                        let background = self.inner.background.clone();
                        if running {
                            tokio::spawn(async move { background.start().await });
                        } else {
                            background.gone();
                        }
                    },
                    event = events.recv() => match event {
                        Ok(event) => {
                            let watcher = self.clone();
                            tokio::spawn(async move {
                                let (heard, noted) = tokio::join!(watcher.heard(event.clone()), watcher.inner.background.noted(&event));
                                let () = heard;
                                if noted {
                                    watcher.background_ended();
                                }
                            });
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => return,
                    },
                }
            }
        })
    }

    /// What's told `starter` goes in a moment, with what else comes meanwhile.
    fn soon(&self, starter: &str, after: Duration) {
        if !self
            .inner
            .due
            .lock()
            .expect("due")
            .insert(starter.to_owned())
        {
            return;
        }
        let (watcher, starter) = (self.clone(), starter.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            watcher.send(&starter).await;
        });
    }

    async fn send(&self, starter: &str) {
        self.inner.due.lock().expect("due").remove(starter);
        // (One at a time to each: what one puts back goes first.)
        if !self
            .inner
            .sending
            .lock()
            .expect("sending")
            .insert(starter.to_owned())
        {
            return self.soon(starter, GATHER);
        }
        self.send_now(starter).await;
        self.inner.sending.lock().expect("sending").remove(starter);
    }

    fn put_back(&self, starter: &str, waiting: Waiting) {
        self.kept().change_waiting(|all| {
            let now: Option<Waiting> = all
                .get(starter)
                .and_then(|now| serde_json::from_value(now.clone()).ok());
            let mut lines = waiting.lines;
            lines.extend(
                now.as_ref()
                    .map(|now| now.lines.clone())
                    .unwrap_or_default(),
            );
            let asks = waiting.asks || now.is_some_and(|now| now.asks);
            let back = Waiting {
                lines,
                asks,
                since: waiting.since,
                tried: waiting.tried,
            };
            all.insert(starter.to_owned(), json!(back));
        });
    }

    async fn send_now(&self, starter: &str) {
        let taken = self.kept().change_waiting(|all| all.shift_remove(starter));
        let Some(waiting) = taken.and_then(|taken| serde_json::from_value::<Waiting>(taken).ok())
        else {
            return;
        };
        // What went once already, its answer lost: again alone, under the same id, and what came
        // after it next. One stopped since it was gathered: nothing to tell of it.
        let tried = waiting
            .tried
            .clone()
            .filter(|tried| tried.lines > 0 && tried.lines <= waiting.lines.len());
        let stopped = self.kept().stopped();
        let unstopped = |lines: &[String]| -> Vec<String> {
            let shown = |line: &&String| {
                line.starts_with(STOPPED_OPENING)
                    || address_in(line).is_none_or(|id| !stopped.contains_key(id))
            };
            lines.iter().filter(shown).cloned().collect()
        };
        let split = tried
            .as_ref()
            .map_or(waiting.lines.len(), |tried| tried.lines);
        let first = unstopped(&waiting.lines[..split]);
        let after = unstopped(&waiting.lines[split..]);
        let again = tried.filter(|_| !first.is_empty());
        let (lines, rest) = match again {
            Some(_) => (first, after),
            None => ([first, after].concat(), Vec::new()),
        };
        if lines.is_empty() {
            return;
        }
        let is_ask = |line: &String| line.starts_with("[Session asks]");
        let asks = match (&again, &waiting.tried) {
            (Some(again), _) => again.asks,
            (None, Some(_)) => lines.iter().any(is_ask),
            (None, None) => waiting.asks,
        };
        let rest_asks = rest.iter().any(is_ask);
        let whole = [lines.clone(), rest.clone()].concat();
        let engine = self.work().list.engine().wake_engine(false).await;
        let id = match (&again, &engine) {
            (Some(again), _) => Some(again.id.clone()),
            (None, Some(engine)) => engine.message_id().ok(),
            (None, None) => None,
        };
        // Not the engine's (a conversation the coding agents run, which they tell, or gone), or not
        // to be reached now (the engine starting again, or moving to or from another computer):
        // kept, as long as a note would be, and tried again a little later each time.
        let place = match &engine {
            Some(engine) => engine.session(starter).await.ok(),
            None => None,
        };
        let since = now().saturating_sub(waiting.since);
        if matches!(place, Some(None)) && since > NOTE_MEMORY {
            self.inner
                .failures
                .lock()
                .expect("failures")
                .remove(starter);
            return;
        }
        let archived = place
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(is_archived);
        if archived && now().saturating_sub(waiting.since) > ARCHIVED_FOR {
            return;
        }
        let mut posted = false;
        let sent = match (engine, id, place.flatten()) {
            (Some(engine), Some(id), Some(_)) if !archived => {
                posted = true;
                let line = wake_line_shown(&lines);
                let note = json!({ "kuvo": { "note": { "from": "Its sessions", "line": line } } });
                let message = BoxMessage {
                    id: id.clone(),
                    text: wake_text(&lines),
                    metadata: Some(note),
                    from: "Its sessions",
                    words: &line,
                    steer: asks,
                };
                let told = engine.tell(starter, message).await;
                told.map(drop).map_err(|_| Some(id))
            }
            (_, id, _) => Err(id),
        };
        match sent {
            Ok(()) => {}
            Err(id) => {
                // (Sent, its answer lost, it may have arrived: kept with its id, to go again as it was.)
                let tried = id.filter(|_| posted || again.is_some()).map(|id| Tried {
                    id,
                    lines: lines.len(),
                    asks,
                });
                let back = Waiting {
                    lines: whole,
                    asks: asks || rest_asks,
                    since: waiting.since,
                    tried,
                };
                self.put_back(starter, back);
                let tries = {
                    let mut failures = self.inner.failures.lock().expect("failures");
                    let tries = failures.entry(starter.to_owned()).or_default();
                    *tries += 1;
                    *tries
                };
                let after = GATHER
                    .saturating_mul(2u32.saturating_pow(tries))
                    .min(Duration::from_secs(300));
                return self.soon(starter, after);
            }
        }
        self.inner
            .failures
            .lock()
            .expect("failures")
            .remove(starter);
        if rest.is_empty() {
            return;
        }
        // (Ahead of what came meanwhile.)
        self.put_back(
            starter,
            Waiting {
                lines: rest,
                asks: rest_asks,
                since: waiting.since,
                tried: None,
            },
        );
        self.soon(starter, GATHER);
    }

    /// A line for `starter`, kept until it's sent.
    fn tell(&self, starter: &str, line: String, asks: bool) {
        self.kept().change_waiting(|all| {
            let kept = all
                .get(starter)
                .and_then(|kept| serde_json::from_value(kept.clone()).ok());
            let mut kept: Waiting = kept.unwrap_or(Waiting {
                since: now(),
                ..Waiting::default()
            });
            kept.lines.push(line);
            kept.asks |= asks;
            all.insert(starter.to_owned(), json!(kept));
        });
        self.soon(starter, GATHER);
    }

    fn note_told(&self, session: &str, note: Note) {
        let caught = self.inner.caught.load(Ordering::Relaxed) && !self.inner.over();
        self.kept()
            .note_told(session, note, caught.then_some(ENGINE));
    }

    /// Whether a session stopped hasn't been set going since, so nothing of it is news. What it
    /// left running ending, or the box's own word, doesn't set it going; anything else written
    /// since does, and it's noted that it isn't stopped any more.
    fn stays_stopped(&self, session: &str, last: Option<&LastReply>) -> bool {
        let Some(at) = self.kept().stopped().get(session).and_then(Value::as_u64) else {
            return false;
        };
        let Some(last) = last else {
            return true;
        };
        if matches!(last.began.kind, BeganBy::Left | BeganBy::Word)
            || last.began.at.unwrap_or(0) <= at
        {
            return true;
        }
        self.kept().forget_stopped(session);
        false
    }

    async fn session(engine: &BotEngine, id: &str) -> Option<Value> {
        let read = engine
            .call(
                Method::GET,
                &format!("api/session/{}", encoded(id)),
                None,
                WAIT,
            )
            .await;
        read.ok().filter(|session| session["id"].is_string())
    }

    async fn at_work(engine: &BotEngine, id: &str) -> bool {
        let active = engine
            .call(Method::GET, "api/session/active", None, WAIT)
            .await
            .unwrap_or_default();
        match active {
            Value::Array(ids) => ids.iter().any(|each| each == id),
            Value::Object(ids) => ids.contains_key(id),
            _ => false,
        }
    }

    /// What an event changes for the sessions' starters.
    async fn heard(&self, event: Arc<EngineEvent>) {
        let data = &event.data;
        let Some(id) = data["sessionID"]
            .as_str()
            .or(data["form"]["sessionID"].as_str())
        else {
            return;
        };
        // (What an MCP server asks is filed under no session: Autopilot's to decline.)
        if !EVENTS.contains(&event.kind.as_str()) || id == "global" {
            return;
        }
        let Some(engine) = self.work().list.engine().running_bot_engine().await else {
            return;
        };
        // A subagent's ask is its conversation's.
        let mut session = Self::session(&engine, id).await;
        for _ in 0..5 {
            let Some(parent) = session
                .as_ref()
                .and_then(|session| session["parentID"].as_str().map(str::to_owned))
            else {
                break;
            };
            session = Self::session(&engine, &parent).await;
        }
        let Some(session) = session else {
            return;
        };
        let kind = event.kind.as_str();
        let own = session["id"] == id;
        if !own && (kind.starts_with("session.execution") || kind == "session.inbox.delivered") {
            return;
        }
        let session_id = session["id"].as_str().unwrap_or_default().to_owned();
        let (starter, wakes) = (started_by(&session).map(str::to_owned), wakes_of(&session));
        let stopped = self.kept().stopped().contains_key(&session_id);
        if kind == "session.inbox.delivered" {
            return self
                .written_to(&engine, &session, starter.is_some(), stopped)
                .await;
        }
        let bot = works_for(&session).map(str::to_owned);
        if let Some(bot) = &bot {
            self.work().records.note_changed(bot);
        }
        let ended = kind == "session.execution.succeeded" || kind == "session.execution.failed";
        // Only a summary it was asked for: not news, and not its work finishing. One that failed
        // says only that, whichever way its run ended.
        if ended {
            let rows = turns::newest(&engine, &session_id, 6)
                .await
                .unwrap_or_default();
            if let Some(summary) = turns::summary_row_at_end(&rows) {
                if summary["status"] == "failed" {
                    if let Some(starter) = starter.as_deref().filter(|_| {
                        wakes.contains(&"finished") && !is_archived(&session) && !stopped
                    }) {
                        let error = error_of(&data["error"], None)
                            .or_else(|| error_of(&summary["error"], None));
                        let line = self.work().not_summarized_line(
                            &session,
                            error.as_ref().map(|error| error.message.as_str()),
                        );
                        self.tell(starter, line, false);
                    }
                    let _ = self.settle(&session_id).await;
                    return;
                }
                if summary["status"] == "completed" && kind == "session.execution.succeeded" {
                    let _ = self.settle(&session_id).await;
                    return;
                }
            }
        }
        let wanted = if ended {
            "finished"
        } else if kind.ends_with(".asked") || kind == "form.created" {
            "asks"
        } else {
            ""
        };
        if kind == "session.execution.succeeded"
            && (starter.is_none() || !wakes.contains(&"finished"))
        {
            // Quiet to its starter, but still its bot's to see, once it's really done.
            if let Some(bot) = bot {
                self.finished_quietly(&session, &bot).await;
            }
            return;
        }
        let Some(starter) = starter.filter(|_| !wanted.is_empty() && wakes.contains(&wanted))
        else {
            return;
        };
        match kind {
            "session.execution.succeeded" => self.finish(&engine, &session, &starter, None).await,
            "session.execution.failed" => {
                self.failed(&engine, &session, &starter, &data["error"])
                    .await
            }
            _ => {
                // From one stopped or put away, nothing: what it asks went with its stop.
                if is_archived(&session) || stopped {
                    return;
                }
                let (line, ask) = match kind {
                    "form.created" => {
                        let form = if data["form"].is_object() {
                            &data["form"]
                        } else {
                            data
                        };
                        (self.work().form_line(&session, form), form["id"].as_str())
                    }
                    _ => (
                        self.work().permission_line(&session, data),
                        data["id"].as_str(),
                    ),
                };
                self.tell(&starter, line, true);
                self.note_told(
                    &session_id,
                    Note {
                        ask: ask.map(str::to_owned),
                        ..Note::default()
                    },
                );
            }
        }
    }

    /// Something written to a session of someone's that was stopped or put away, in its
    /// conversation by now. What it left running ending stops there, as that began no turn anyone
    /// wanted; anything else goes on, and a stopped one is no longer stopped.
    async fn written_to(&self, engine: &BotEngine, session: &Value, started: bool, stopped: bool) {
        if !started || (!stopped && !is_archived(session)) {
            return;
        }
        let id = session["id"].as_str().unwrap_or_default();
        let Some(began) = turns::last_reply(engine, id)
            .await
            .filter(|began| began.under_way)
        else {
            return;
        };
        if stopped && !self.stays_stopped(id, Some(&began)) {
            return;
        }
        if began.began.kind == BeganBy::Left || (stopped && began.began.kind == BeganBy::Word) {
            let _ = engine
                .call(
                    Method::POST,
                    &format!("api/session/{}/interrupt", encoded(id)),
                    None,
                    WAIT,
                )
                .await;
        }
    }

    /// A turn of a session that started by someone's asking ended well. Told to who started it, if
    /// it's news and nothing it left running still goes on; while that does, held, and looked at as
    /// it ends. `known`: its last reply, if it's read.
    async fn finish(
        &self,
        engine: &BotEngine,
        session: &Value,
        starter: &str,
        known: Option<LastReply>,
    ) {
        let id = session["id"].as_str().unwrap_or_default();
        let at = now();
        // (As its engine says it last changed, before its reply is read: what it wrote by then.)
        let upto = session["time"]["updated"].as_f64().map(|at| at as u64);
        let last = match known {
            Some(known) => Some(known),
            None => turns::last_reply(engine, id).await,
        };
        let reply = last.as_ref().and_then(|last| last.id.clone());
        let only = |reply| Note {
            reply,
            ..Note::default()
        };
        if is_archived(session) || self.stays_stopped(id, last.as_ref()) {
            self.kept().let_go_of(id);
            return self.note_told(id, only(reply));
        }
        // Its reply unread (the engine slow), and at work again by now: told as that turn ends.
        if last.is_none() && Self::at_work(engine, id).await {
            return;
        }
        let still = self
            .inner
            .background
            .of(id, lines::folder_of(session))
            .await;
        let waited = self.kept().waited(id);
        // A turn the user had with it is theirs: nothing to tell, unless a turn its starter asked
        // for waits on what it left running. That waits on as it did, and then its starter hears
        // it's done, though not the user's answer as its report.
        let yours = last
            .as_ref()
            .is_some_and(|last| last.began.kind == BeganBy::You);
        if yours && waited.is_none() {
            return self.note_told(id, only(reply));
        }
        // (Someone's question isn't held up by what was running before they asked: a dev server.)
        let began = last
            .as_ref()
            .map(|last| &last.began)
            .filter(|began| began.kind == BeganBy::Asked);
        let (asked, order) = match (&waited, yours) {
            (Some(waited), true) => (waited.asked, waited.asked_order.clone()),
            _ => (
                began.and_then(|began| began.at),
                began.and_then(|began| began.order.clone()),
            ),
        };
        if held::holds(still.as_ref(), asked, order.as_ref()) {
            let since = waited.as_ref().map_or(at, |waited| waited.at);
            return self.wait_for(id, since, asked, order);
        }
        let was = self.kept().session_told(id);
        self.kept().let_go_of(id);
        if let Some(last) = last
            .as_ref()
            .filter(|last| !yours && !turns::is_news(&was, last))
        {
            return self.note_told(id, only(last.id.clone()));
        }
        let line = if yours {
            self.work().done_since_line(session)
        } else {
            let rows = turns::newest(engine, id, 6).await.unwrap_or_default();
            let serving = still
                .as_ref()
                .map(background::Running::services_words)
                .unwrap_or_default();
            self.work()
                .finished_line(session, turns::reply_in(&rows), &serving)
        };
        let seen = upto.max(last.as_ref().and_then(|last| last.at));
        // Told meanwhile by another look at the same finish.
        let told = self.kept().session_told(id);
        if reply.is_some() && told.reply == reply {
            return self.note_told(id, only(reply));
        }
        if (reply.is_none() || told.unread)
            && seen.is_some_and(|seen| told.upto.is_some_and(|upto| seen <= upto))
        {
            if reply.is_some() {
                self.note_told(id, only(reply));
            }
            return;
        }
        self.tell(starter, line, false);
        // One of a bot's sessions done: for clients to show it seeing that, once, as it's told.
        if let Some(bot) = works_for(session) {
            self.work().records.note_oversight(bot, id, "finished");
        }
        let said = last.as_ref().map(|last| {
            if last.text.is_empty() {
                String::new()
            } else {
                turns::hashed(&last.text)
            }
        });
        let early = Some(still.is_some() || waited.is_some());
        let unread = reply.is_none();
        self.note_told(
            id,
            Note {
                reply,
                at: Some(at),
                said,
                early,
                upto: seen,
                unread,
                ..Note::default()
            },
        );
    }

    /// A turn that failed: why, told to who started it, once; one at its account or its limit holds
    /// that model back.
    async fn failed(&self, engine: &BotEngine, session: &Value, starter: &str, error: &Value) {
        let id = session["id"].as_str().unwrap_or_default();
        let at = now();
        let last = turns::last_reply(engine, id).await;
        let (reply, turn) = (
            last.as_ref().and_then(|last| last.id.clone()),
            last.as_ref().and_then(|last| last.turn.clone()),
        );
        let noted = |reply, turn| Note {
            reply,
            turn,
            ..Note::default()
        };
        if is_archived(session) || self.stays_stopped(id, last.as_ref()) {
            self.kept().let_go_of(id);
            return self.note_told(id, noted(reply, turn));
        }
        // (One the user started, writing to it: theirs to see, as its answer would be.)
        if last
            .as_ref()
            .is_some_and(|last| last.began.kind == BeganBy::You)
        {
            return self.note_told(id, noted(reply, turn));
        }
        // The same failure again, in a turn no one asked for (a timer's, at a limit): told once.
        let error = error_of(error, None);
        let failure = error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_default();
        let unasked = last
            .as_ref()
            .is_some_and(|last| last.began.kind != BeganBy::Asked);
        if !failure.is_empty()
            && unasked
            && self.kept().session_told(id).failure.as_deref() == Some(failure.as_str())
        {
            return self.note_told(id, noted(reply, turn));
        }
        let line = self.failed_line_of(engine, session, error).await;
        self.kept().let_go_of(id);
        self.tell(starter, line, false);
        self.note_told(
            id,
            Note {
                reply,
                at: Some(at),
                turn,
                failure: Some(failure),
                ..Note::default()
            },
        );
    }

    /// A failed session's line: why, in its engine's words (the event's own, else read from its
    /// conversation); one at its account or its limit holds that model back, and says so.
    async fn failed_line_of(
        &self,
        engine: &BotEngine,
        session: &Value,
        error: Option<super::unanswering::TurnError>,
    ) -> String {
        let id = session["id"].as_str().unwrap_or_default();
        let error = match error {
            Some(error) => Some(error),
            None => {
                let rows = turns::newest(engine, id, 5).await.unwrap_or_default();
                let end = turns::last_turn_end(&rows).filter(|end| end["outcome"] == "failed");
                end.map(|end| {
                    let at = end["time"]["created"].as_f64().map(|at| at as u64);
                    let none = super::unanswering::TurnError {
                        kind: String::new(),
                        message: String::new(),
                        at: None,
                        limit: None,
                    };
                    error_of(&end["error"], at).unwrap_or(none)
                })
            }
        };
        let model = &session["model"];
        let held = match (&error, model["providerID"].as_str(), model["id"].as_str()) {
            (Some(error), Some(provider), Some(model)) => {
                let model = ModelRef::new(provider, model);
                self.work()
                    .unanswering
                    .failed_on(engine, &model, error)
                    .await
            }
            _ => None,
        };
        let held = held.map(|held| hold_words(&held, &self.work().home));
        let why = error.as_ref().map(|error| error.message.as_str());
        self.work().failed_line(session, why, held)
    }
}
