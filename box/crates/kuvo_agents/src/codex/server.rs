//! The account's Codex app-server, shared by all its conversations: started the first time it's
//! needed and again after it stops, restarted once quiet when what it started with changed, and
//! stopped after a while with nothing on it. What it says goes to the thread it names.

use std::{
    collections::{BTreeSet, HashMap},
    io,
    sync::{
        Arc, Mutex as Plain,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, mpsc},
    time::Instant,
};

use crate::{
    hold::{Controller, Directory, Metadata},
    rpc::{Framing, Incoming, Options, Rpc, RpcError},
};

use super::{
    account::{Account, Heard},
    program::Program,
};

const IDLE: Duration = Duration::from_secs(15 * 60);
const INITIALIZE: Duration = Duration::from_secs(30);
/// How long a stopped app-server, or a start under way, is waited for to end.
const ENDS: Duration = Duration::from_secs(10);

/// An app-server, as it runs now.
pub(super) struct Server {
    pub id: u64,
    pub rpc: Rpc,
    /// Who it's signed in as: it reads its sign-in as it starts, and keeps it.
    pub signed_in_as: Plain<Option<String>>,
    /// The "Summarize long conversations at" it started with.
    compaction: String,
    /// The turns under way in it, by Codex thread, as its holder keeps them for a next resident.
    turns: Plain<serde_json::Map<String, Value>>,
    /// The conversations with commands left running in it, as its holder keeps them.
    background: Plain<serde_json::Map<String, Value>>,
    /// Taken up from its holder: what it says waits until the turns taken up in it are ready.
    waiting: Plain<std::collections::HashSet<String>>,
    gate: tokio::sync::watch::Sender<bool>,
}

impl Server {
    pub fn closed(&self) -> bool {
        self.rpc.is_closed()
    }

    /// Keeps `thread`'s turn (none: it ended) in its holder's record, for a resident taking it up.
    pub async fn note_turn(&self, thread: &str, note: Option<Value>) {
        let turns = {
            let mut turns = lock(&self.turns);
            match note {
                Some(note) => turns.insert(thread.to_owned(), note),
                None => turns.remove(thread),
            };
            turns.clone()
        };
        self.rpc
            .note(Metadata::from_iter([(
                "turns".into(),
                Value::Object(turns),
            )]))
            .await;
    }

    /// Keeps whether `session` has commands left running in it in its holder's record.
    pub async fn note_background(&self, session: &str, snapshot: Option<Value>) {
        let sessions = {
            let mut sessions = lock(&self.background);
            if sessions.get(session) == snapshot.as_ref() {
                return;
            }
            if let Some(snapshot) = snapshot {
                sessions.insert(session.to_owned(), snapshot);
            } else {
                sessions.remove(session);
            }
            sessions.clone()
        };
        let note = Metadata::from_iter([("backgroundSessions".into(), Value::Object(sessions))]);
        self.rpc.note(note).await;
    }

    /// Where the earliest turn under way in it began: what the next resident replays from.
    pub fn earliest_turn(&self) -> Option<u64> {
        lock(&self.turns)
            .values()
            .filter_map(|note| note["from"].as_u64())
            .min()
    }

    /// A turn taken up in it is ready for its lines (or let go): once all are, they come.
    pub fn ready(&self, thread: &str) {
        let mut waiting = lock(&self.waiting);
        waiting.remove(thread);
        if waiting.is_empty() {
            self.gate.send_replace(true);
        }
    }
}

/// What a thread hears from the app-server it's in.
pub(super) enum Event {
    Notification {
        server: u64,
        method: String,
        params: Value,
        sequence: u64,
    },
    Request {
        server: Arc<Server>,
        id: Value,
        method: String,
        params: Value,
    },
    /// The app-server stopped: its threads must be loaded again, and a turn on it is over.
    Stopped { server: u64 },
}

/// A conversation's thread, as the app-server's words find it.
pub(super) struct Route {
    pub native: Option<String>,
    pub server: Option<u64>,
    /// In a turn, or keeping work going: its app-server isn't to stop meanwhile.
    pub busy: Arc<AtomicBool>,
    pub events: mpsc::UnboundedSender<Event>,
}

#[derive(Default)]
struct Slot {
    server: Option<Arc<Server>>,
    /// Stopped this many times: one stopped while starting isn't kept.
    stops: u64,
    /// The one still running with what changed since it started.
    outdated: Option<(u64, BTreeSet<&'static str>)>,
    last_use: Option<Instant>,
}

pub(super) struct Shared {
    pub program: Program,
    slot: Mutex<Slot>,
    /// Takes the slot's lock to start: one start at a time, the others wait for it.
    starting: Mutex<()>,
    pub routes: Plain<HashMap<String, Route>>,
    pub account: Account,
    next: AtomicU64,
    /// Shut down (or let go of) this many times: a start under way then fails, and stops its own.
    shutdowns: tokio::sync::watch::Sender<u64>,
    idle_check: AtomicBool,
}

impl Shared {
    pub fn new(program: Program) -> io::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            program,
            slot: Mutex::default(),
            starting: Mutex::new(()),
            routes: Plain::default(),
            account: Account::new()?,
            next: AtomicU64::new(0),
            shutdowns: tokio::sync::watch::Sender::new(0),
            idle_check: AtomicBool::new(false),
        }))
    }

    pub fn routes(&self) -> std::sync::MutexGuard<'_, HashMap<String, Route>> {
        self.routes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Whether something uses `server`: a turn, work kept going, a sign-in.
    fn busy(&self, server: u64) -> bool {
        self.account.signing_in()
            || self
                .routes()
                .values()
                .any(|route| route.server == Some(server) && route.busy.load(Ordering::Relaxed))
    }

    /// The app-server, started the first time it's needed and again after it stops.
    pub async fn server(self: &Arc<Self>) -> io::Result<Arc<Server>> {
        let shutdowns = *self.shutdowns.borrow();
        let _starting = self.starting.lock().await;
        if *self.shutdowns.borrow() != shutdowns {
            return Err(stopped());
        }
        let compaction = self.program.compaction();
        let identity = self.program.identity().await;
        let stops = {
            let mut slot = self.slot.lock().await;
            slot.last_use = Some(Instant::now());
            if let Some(server) = slot.server.clone().filter(|server| !server.closed()) {
                if server.compaction != compaction {
                    mark(&mut slot, server.id, "compaction");
                }
                if *lock(&server.signed_in_as) != identity {
                    mark(&mut slot, server.id, "account");
                }
            }
            drop(slot);
            self.restart_if_quiet(Some(&compaction), identity.as_ref())
                .await;
            let slot = self.slot.lock().await;
            if let Some(server) = slot.server.clone().filter(|server| !server.closed()) {
                return Ok(server);
            }
            slot.stops
        };
        self.watch_idle();
        let server = self.start(&compaction, identity, shutdowns).await?;
        let mut slot = self.slot.lock().await;
        if slot.stops != stops || *self.shutdowns.borrow() != shutdowns {
            server.rpc.stop().await;
            return Err(stopped());
        }
        slot.server = Some(server.clone());
        Ok(server)
    }

    /// The app-server now, if one runs.
    pub async fn current(&self) -> Option<Arc<Server>> {
        self.slot
            .lock()
            .await
            .server
            .clone()
            .filter(|server| !server.closed())
    }

    async fn start(
        self: &Arc<Self>,
        compaction: &str,
        identity: Option<String>,
        shutdowns: u64,
    ) -> io::Result<Arc<Server>> {
        let mut halted = self.shutdowns.subscribe();
        let program = self.program.clone();
        let launch = {
            let compaction = compaction.to_owned();
            tokio::task::spawn_blocking(move || program.launch(&compaction)).await??
        };
        let controller = Directory::for_home(&self.program.home)
            .start(&self.program.holder_binary, launch)
            .await?;
        let server = self.serve(
            controller,
            compaction.to_owned(),
            identity,
            1,
            Default::default(),
        );
        let initialize = server.rpc.request(
            "initialize",
            Some(json!({
                "clientInfo": {"name":"kuvo-code","title":"Kuvo Code","version":"1"},
                "capabilities": {"experimentalApi":true,"optOutNotificationMethods":["turn/diff/updated"]},
            })),
            Some(INITIALIZE),
        );
        // Stopped while it starts: it isn't left running, nor kept for a next resident.
        let initialized = tokio::select! {
            initialized = initialize => initialized.map_err(io::Error::from),
            _ = halted.wait_for(|now| *now != shutdowns) => Err(stopped()),
        };
        if let Err(error) = initialized {
            // One that won't start isn't left running (the next ask starts another), nor its
            // holder: it's gone before this says so.
            server.rpc.stop().await;
            server.rpc.ended(ENDS).await;
            return Err(error);
        }
        server.rpc.notify("initialized", None).await;
        server
            .rpc
            .note(Metadata::from_iter([(
                "initialized".into(),
                Value::Bool(true),
            )]))
            .await;
        Ok(server)
    }

    /// An app-server an earlier resident left as it was starting, never initialized: of no use (Codex
    /// refuses a second time), so it's stopped, its holder with it.
    pub async fn discard(&self, controller: Controller) {
        let options = Options {
            framing: Framing::Bare,
            first_id: 1,
            filter: None,
        };
        let (rpc, _incoming) = Rpc::over(controller, options);
        eprintln!("Stopping Codex app-server: never initialized");
        rpc.stop().await;
        rpc.ended(ENDS).await;
    }

    /// An app-server an earlier resident left held, this resident's own again as it is
    /// (initialized already: Codex refuses a second time).
    pub async fn adopt(self: &Arc<Self>, controller: Controller) -> io::Result<Arc<Server>> {
        let meta = controller.welcome.meta.clone();
        // (Its own IDs past those any earlier resident sent it.)
        let first = controller
            .welcome
            .generation
            .saturating_mul(1_000_000_000)
            .max(1);
        let compaction = meta
            .get("compaction")
            .and_then(Value::as_str)
            .unwrap_or("400000")
            .to_owned();
        let identity = self.program.identity().await;
        let turns = meta
            .get("turns")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let server = self.serve(controller, compaction, identity, first, turns);
        let mut slot = self.slot.lock().await;
        if slot
            .server
            .as_ref()
            .is_some_and(|running| !running.closed())
        {
            drop(slot);
            server.rpc.stop().await;
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Codex has an app-server running already.",
            ));
        }
        slot.server = Some(server.clone());
        slot.last_use = Some(Instant::now());
        drop(slot);
        self.watch_idle();
        Ok(server)
    }

    fn serve(
        self: &Arc<Self>,
        controller: Controller,
        compaction: String,
        identity: Option<String>,
        first_id: u64,
        turns: serde_json::Map<String, Value>,
    ) -> Arc<Server> {
        let background = controller
            .welcome
            .meta
            .get("backgroundSessions")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let (rpc, mut incoming) = Rpc::over(
            controller,
            Options {
                framing: Framing::Bare,
                first_id,
                filter: None,
            },
        );
        let mut waiting: std::collections::HashSet<String> = turns.keys().cloned().collect();
        if !background.is_empty() {
            waiting.insert("background".into());
        }
        let (gate, mut open) = tokio::sync::watch::channel(waiting.is_empty());
        let server = Arc::new(Server {
            id: self.next.fetch_add(1, Ordering::Relaxed) + 1,
            rpc,
            signed_in_as: Plain::new(identity),
            compaction,
            turns: Plain::new(turns),
            background: Plain::new(background),
            waiting: Plain::new(waiting),
            gate,
        });
        let shared = Arc::downgrade(self);
        let own = server.clone();
        tokio::spawn(async move {
            // (Taken up: its turns' threads are ready for what it says first.)
            let _ =
                tokio::time::timeout(Duration::from_secs(60), open.wait_for(|open| *open)).await;
            while let Some(message) = incoming.recv().await {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                shared.dispatch(&own, message).await;
            }
        });
        server
    }

    async fn dispatch(self: &Arc<Self>, server: &Arc<Server>, message: Incoming) {
        match message {
            Incoming::Notification {
                method,
                params,
                sequence,
            } => {
                let heard = self.account.heard(self, server, &method, &params).await;
                if matches!(heard, Heard::Taken) {
                    return;
                }
                let thread = params["threadId"].as_str();
                // For whoever's at work: each conversation with a turn under way hears it.
                let everyone = matches!(heard, Heard::Everyone)
                    || thread.is_none()
                        && matches!(
                            method.as_str(),
                            "warning"
                                | "deprecationNotice"
                                | "configWarning"
                                | "mcpServer/startupStatus/updated"
                                | "serverRequest/resolved"
                        );
                let routes = self.routes();
                for route in routes.values() {
                    if everyone || (thread.is_some() && route.native.as_deref() == thread) {
                        let _ = route.events.send(Event::Notification {
                            server: server.id,
                            method: method.clone(),
                            params: params.clone(),
                            sequence,
                        });
                    }
                }
            }
            Incoming::Request {
                id, method, params, ..
            } => {
                // A tool of the app's: it gives Codex none.
                if method == "item/tool/call" {
                    let answer = json!({"success":false,"contentItems":[{"type":"inputText","text":"Kuvo Code has no such tool."}]});
                    return server.rpc.respond(id, Ok(answer)).await;
                }
                let thread = params["threadId"]
                    .as_str()
                    .or_else(|| params["conversationId"].as_str())
                    .map(str::to_owned);
                let route = thread.and_then(|thread| {
                    self.routes()
                        .values()
                        .find(|route| route.native.as_deref() == Some(&thread))
                        .map(|route| route.events.clone())
                });
                let unsent = match route {
                    Some(events) => events
                        .send(Event::Request {
                            server: server.clone(),
                            id,
                            method,
                            params,
                        })
                        .err()
                        .and_then(|failed| match failed.0 {
                            Event::Request { id, method, .. } => Some((id, method)),
                            _ => None,
                        }),
                    None => Some((id, method)),
                };
                if let Some((id, method)) = unsent {
                    let answer = if method == "mcpServer/elicitation/request" {
                        Ok(json!({"action":"decline"}))
                    } else {
                        Err(RpcError::new(-32601, format!("No session for {method}.")))
                    };
                    server.rpc.respond(id, answer).await;
                }
            }
            Incoming::Stray { .. } => {}
            Incoming::Exited { .. } => {
                {
                    let mut slot = self.slot.lock().await;
                    if slot
                        .server
                        .as_ref()
                        .is_some_and(|current| current.id == server.id)
                    {
                        slot.server = None;
                    }
                }
                self.account.server_stopped(server.id);
                for route in self.routes().values() {
                    let _ = route.events.send(Event::Stopped { server: server.id });
                }
            }
        }
    }

    /// Restarts an outdated app-server when nothing uses it.
    async fn restart_if_quiet(&self, compaction: Option<&str>, identity: Option<&String>) {
        let mut slot = self.slot.lock().await;
        let Some((id, mut changed)) = slot.outdated.take() else {
            return;
        };
        let Some(server) = slot
            .server
            .clone()
            .filter(|server| server.id == id && !server.closed())
        else {
            return;
        };
        if compaction.is_some_and(|now| now == server.compaction) {
            changed.remove("compaction");
        }
        if identity.is_some_and(|now| lock(&server.signed_in_as).as_ref() == Some(now)) {
            changed.remove("account");
        }
        if changed.is_empty() {
            return;
        }
        if self.busy(id) {
            slot.outdated = Some((id, changed));
            return;
        }
        eprintln!("Stopping Codex app-server: launch settings changed ({changed:?})");
        slot.stops += 1;
        slot.server = None;
        drop(slot);
        // What the old one listed meanwhile is asked anew.
        self.account.forget_models();
        server.rpc.stop().await;
        self.stopped(id);
    }

    /// Stopped when nothing has used it for a while, checked every minute.
    fn watch_idle(self: &Arc<Self>) {
        if self.idle_check.swap(true, Ordering::Relaxed) {
            return;
        }
        let shared = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                shared.restart_if_quiet(None, None).await;
                let mut slot = shared.slot.lock().await;
                let Some(server) = slot.server.clone() else {
                    continue;
                };
                let on_it = shared
                    .routes()
                    .values()
                    .any(|route| route.server == Some(server.id));
                if shared.busy(server.id) || on_it {
                    slot.last_use = Some(Instant::now());
                    continue;
                }
                if slot.last_use.is_some_and(|last| last.elapsed() >= IDLE) {
                    eprintln!("Stopping Codex app-server: idle");
                    slot.stops += 1;
                    slot.server = None;
                    drop(slot);
                    server.rpc.stop().await;
                    shared.stopped(server.id);
                }
            }
        });
    }

    /// Its threads are told it stopped on purpose (they may be on a newer one by now).
    fn stopped(&self, server: u64) {
        for route in self.routes().values() {
            let _ = route.events.send(Event::Stopped { server });
        }
    }

    /// A start under way fails, and stops what it started.
    pub fn halt(&self) {
        self.shutdowns.send_modify(|count| *count += 1);
    }

    /// Stops the app-server (and one starting: its start then fails).
    pub async fn shutdown(&self) {
        self.halt();
        // A start under way ends first, and stops what it started: a holder isn't left starting
        // as the resident goes.
        let _ = tokio::time::timeout(ENDS, self.starting.lock()).await;
        let server = {
            let mut slot = self.slot.lock().await;
            slot.stops += 1;
            slot.server.take()
        };
        if let Some(server) = server {
            eprintln!("Stopping Codex app-server: shutdown");
            server.rpc.stop().await;
            server.rpc.ended(ENDS).await;
            self.stopped(server.id);
        }
    }

    /// A turn taken up that no conversation here goes on with: stopped in Codex, and the lines of
    /// the others come.
    pub async fn let_go(&self, turn: &crate::hold::TurnNote) -> io::Result<()> {
        let Some(server) = self.current().await else {
            return Ok(());
        };
        let thread = lock(&server.turns)
            .iter()
            .find(|(_, note)| note["session"] == turn.session.as_str())
            .map(|(thread, _)| thread.clone());
        let Some(thread) = thread else { return Ok(()) };
        server.note_turn(&thread, None).await;
        if let Some(codex) = turn
            .state
            .as_ref()
            .and_then(|state| state["codexTurn"].as_str())
        {
            let _ = server
                .rpc
                .request(
                    "turn/interrupt",
                    Some(json!({ "threadId": thread, "turnId": codex })),
                    Some(Duration::from_secs(30)),
                )
                .await;
        }
        server.ready(&thread);
        Ok(())
    }

    /// Lets go of the app-server for the next resident, as it is.
    pub async fn detach(&self) {
        self.shutdowns.send_modify(|count| *count += 1);
        self.slot.lock().await.server.take();
    }
}

fn mark(slot: &mut Slot, server: u64, field: &'static str) {
    match &mut slot.outdated {
        Some((id, changed)) if *id == server => {
            changed.insert(field);
        }
        _ => slot.outdated = Some((server, BTreeSet::from([field]))),
    }
}

pub(super) fn lock<T>(value: &Plain<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(super) fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "Codex stopped.")
}
