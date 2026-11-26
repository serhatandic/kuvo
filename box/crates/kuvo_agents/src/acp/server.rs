//! The account's agent processes, shared by its conversations: one per launch key (the same mode,
//! skills, program and sign-in), each under a holder, carrying every session opened in it. What a
//! process says goes to the conversation whose session it names. One that's outdated (a new
//! program, a stop it can't go on from) takes no new sessions and stops once nothing uses it; one
//! nothing has used for a while stops.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex as Plain,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use kuvo_store::home::Home;
use serde_json::{Map, Value, json};
use tokio::{
    sync::{Mutex, Notify, mpsc, watch},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    hold::{Controller, Directory, Metadata},
    rpc::{Framing, Incoming, Options, Rpc, RpcError},
};

use super::{
    flavour::{Extension, Flavour, Launch, Line},
    wire::{Capabilities, SessionUpdate, initialize},
};

const IDLE: Duration = Duration::from_secs(15 * 60);
pub(super) const INITIALIZE: Duration = Duration::from_secs(120);
pub(super) const AUTHENTICATE: Duration = Duration::from_secs(300);
/// How long a stopped process, or a start under way, is waited for to end.
const ENDS: Duration = Duration::from_secs(10);
/// A process that asks for a new sign-in sooner than this after it started has lost its sign-in.
pub(super) const SIGN_IN_GRACE: Duration = Duration::from_secs(60);
/// The most updates kept for sessions being opened, before they're known.
const STAGED: usize = 1024;

/// Why a process ended.
#[derive(Clone, Debug, Default)]
pub(super) struct Exit {
    /// What it last wrote on stderr, or what stopped it.
    pub said: String,
    /// It asked for a new sign-in.
    pub asks_sign_in: bool,
    /// Sooner than [`SIGN_IN_GRACE`] after it started.
    pub young: bool,
    /// It ended by itself (not stopped here, nor for a sign-in).
    pub by_itself: bool,
}

/// What made a process stop that the filter saw.
#[derive(Default)]
struct Stopping {
    asks_sign_in: bool,
}

/// An agent process, as it runs now.
pub(super) struct Process {
    pub id: u64,
    pub key: String,
    pub rpc: Rpc,
    capabilities: std::sync::OnceLock<Capabilities>,
    /// Who it was signed in as as it started.
    pub credential: Option<String>,
    /// Its own folder, removed as it ends.
    temporary: Option<PathBuf>,
    started: Instant,
    last_use: Plain<Instant>,
    idle_changes: watch::Sender<u64>,
    /// Takes no new sessions, and stops once nothing uses it.
    outdated: AtomicBool,
    stopping: Arc<Plain<Stopping>>,
    /// Its holder's record of turns under way by ACP session, and of each session open in it.
    turns: Plain<Map<String, Value>>,
    sessions: Plain<Map<String, Value>>,
    /// Sessions being opened in it: updates for one not yet known wait for it.
    opening: Plain<Staging>,
    /// Taken up from its holder: what it says waits until the turns taken up in it are ready.
    waiting: Plain<HashSet<String>>,
    gate: watch::Sender<bool>,
}

#[derive(Default)]
struct Staging {
    opening: usize,
    updates: VecDeque<(SessionUpdate, u64)>,
}

impl Process {
    pub fn closed(&self) -> bool {
        self.rpc.is_closed()
    }

    pub fn capabilities(&self) -> Capabilities {
        self.capabilities.get().cloned().unwrap_or_default()
    }

    /// Used now: it isn't idle.
    pub fn touch(&self) {
        *lock(&self.last_use) = Instant::now();
        self.idle_changes.send_modify(|change| *change = change.wrapping_add(1));
    }

    pub fn outdated(&self) -> bool {
        self.outdated.load(Ordering::Relaxed)
    }

    /// Takes no new sessions; stops once nothing uses it.
    pub fn retire(&self) {
        self.outdated.store(true, Ordering::Relaxed);
        self.idle_changes.send_modify(|change| *change = change.wrapping_add(1));
    }

    /// Keeps `session`'s turn (none: it ended) in its holder's record, for a resident taking it up.
    pub async fn note_turn(&self, session: &str, note: Option<Value>) {
        let turns = change(&self.turns, session, note);
        self.rpc
            .note(Metadata::from_iter([(
                "turns".into(),
                Value::Object(turns),
            )]))
            .await;
    }

    /// Keeps what `session` is in it (its conversation, what it offered, what it has going).
    pub async fn note_session(&self, session: &str, note: Option<Value>) {
        if lock(&self.sessions).get(session) == note.as_ref() {
            return;
        }
        let sessions = change(&self.sessions, session, note);
        self.rpc
            .note(Metadata::from_iter([(
                "sessions".into(),
                Value::Object(sessions),
            )]))
            .await;
    }

    pub fn session_note(&self, session: &str) -> Option<Value> {
        lock(&self.sessions).get(session).cloned()
    }

    pub fn turn_note(&self, session: &str) -> Option<Value> {
        lock(&self.turns).get(session).cloned()
    }

    /// Where the earliest turn under way in it began: what the next resident replays from.
    pub fn earliest_turn(&self) -> Option<u64> {
        lock(&self.turns)
            .values()
            .filter_map(|note| note["from"].as_u64())
            .min()
    }

    /// A turn taken up in it is ready for its lines (or let go): once all are, they come.
    pub fn ready(&self, session: &str) {
        let mut waiting = lock(&self.waiting);
        waiting.remove(session);
        if waiting.is_empty() {
            self.gate.send_replace(true);
        }
    }

    /// A session is being opened: updates for sessions not known yet wait, until `opened`.
    pub fn opening(&self) {
        lock(&self.opening).opening += 1;
    }

    pub fn young(&self) -> bool {
        self.started.elapsed() < SIGN_IN_GRACE
    }
}

fn change(map: &Plain<Map<String, Value>>, key: &str, value: Option<Value>) -> Map<String, Value> {
    let mut map = lock(map);
    match value {
        Some(value) => map.insert(key.to_owned(), value),
        None => map.remove(key),
    };
    map.clone()
}

/// What a conversation hears from the process it's in.
pub(super) enum Event {
    Update {
        process: u64,
        update: SessionUpdate,
    },
    Extension {
        extension: Extension,
    },
    Request {
        process: Arc<Process>,
        id: Value,
        method: String,
        params: Value,
    },
    /// An answer to a request an earlier resident sent: a prompt it took up ends with it.
    Answer {
        process: u64,
        id: Value,
        answer: Result<Value, RpcError>,
    },
    Stopped {
        process: u64,
        exit: Exit,
    },
}

/// A conversation, as what its processes say finds it.
pub(super) struct Route {
    /// Its session in the agent.
    pub native: Option<String>,
    pub process: Option<u64>,
    /// In a turn, or keeping work going: its process isn't to stop meanwhile.
    pub busy: Arc<AtomicBool>,
    pub events: mpsc::UnboundedSender<Event>,
}

#[derive(Default)]
struct Slots {
    processes: Vec<Arc<Process>>,
}

pub(super) struct Shared {
    pub flavour: Arc<dyn Flavour>,
    pub home: Home,
    pub holder_binary: PathBuf,
    pub provider: String,
    slots: Mutex<Slots>,
    /// One start at a time: the others wait for it, and may share it.
    starting: Mutex<()>,
    routes: Plain<HashMap<String, Route>>,
    next: AtomicU64,
    /// Shut down (or let go of) this many times: a start under way then fails, and stops its own.
    shutdowns: watch::Sender<u64>,
    idle_check: AtomicBool,
    idle_changes: watch::Sender<u64>,
}

impl Shared {
    pub fn new(
        flavour: Arc<dyn Flavour>,
        home: Home,
        holder_binary: PathBuf,
        provider: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            flavour,
            home,
            holder_binary,
            provider,
            slots: Mutex::default(),
            starting: Mutex::new(()),
            routes: Plain::default(),
            next: AtomicU64::new(0),
            shutdowns: watch::Sender::new(0),
            idle_check: AtomicBool::new(false),
            idle_changes: watch::Sender::new(0),
        })
    }

    pub fn name(&self) -> &'static str {
        self.flavour.name()
    }

    pub fn routes(&self) -> std::sync::MutexGuard<'_, HashMap<String, Route>> {
        lock(&self.routes)
    }

    pub(super) fn note_activity(&self) { self.idle_changes.send_modify(|change| *change = change.wrapping_add(1)); }

    /// Whether a turn or kept work uses `process`.
    fn busy(&self, process: u64) -> bool {
        self.routes()
            .values()
            .any(|route| route.process == Some(process) && route.busy.load(Ordering::Relaxed))
    }

    /// Its session `native` is known in `process` from now on, for `session`: what it said of it
    /// before it was known comes back (`keep`: as it came; otherwise, a load's replay, dropped).
    pub fn opened(
        &self,
        process: &Process,
        session: &str,
        native: Option<&str>,
        keep: bool,
    ) -> Vec<(SessionUpdate, u64)> {
        let mut routes = self.routes();
        if let Some(route) = routes.get_mut(session) {
            route.native = native.map(str::to_owned);
            route.process = Some(process.id);
        }
        let mut staging = lock(&process.opening);
        staging.opening = staging.opening.saturating_sub(1);
        let mut taken = Vec::new();
        if let Some(native) = native {
            let mut rest = VecDeque::new();
            for (update, sequence) in staging.updates.drain(..) {
                if update.session.as_deref() == Some(native) {
                    taken.push((update, sequence));
                } else {
                    rest.push_back((update, sequence));
                }
            }
            staging.updates = rest;
        }
        if staging.opening == 0 {
            staging.updates.clear();
        }
        drop(staging);
        drop(routes);
        process.touch();
        if keep { taken } else { Vec::new() }
    }

    /// The process conversations started with this launch key share: a live one that takes
    /// sessions, or one started now.
    pub async fn process(
        self: &Arc<Self>,
        key: &str,
        launch: impl FnOnce() -> io::Result<Launch> + Send + 'static,
        cancel: &CancellationToken,
    ) -> io::Result<Arc<Process>> {
        let shutdowns = *self.shutdowns.borrow();
        let _starting = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(stopped(self.name())),
            lock = self.starting.lock() => lock,
        };
        if *self.shutdowns.borrow() != shutdowns {
            return Err(stopped(self.name()));
        }
        {
            let mut slots = self.slots.lock().await;
            slots.processes.retain(|process| !process.closed());
            if let Some(process) = slots
                .processes
                .iter()
                .find(|process| process.key == key && !process.outdated())
            {
                process.touch();
                return Ok(process.clone());
            }
        }
        self.watch_idle();
        let launch = tokio::task::spawn_blocking(launch)
            .await
            .map_err(io::Error::other)??;
        let process = self.start(key, launch, shutdowns, cancel).await?;
        let mut slots = self.slots.lock().await;
        if *self.shutdowns.borrow() != shutdowns {
            drop(slots);
            process.rpc.stop().await;
            return Err(stopped(self.name()));
        }
        slots.processes.push(process.clone());
        process.touch();
        Ok(process)
    }

    /// The processes running now.
    pub async fn running(&self) -> Vec<Arc<Process>> {
        let slots = self.slots.lock().await;
        slots
            .processes
            .iter()
            .filter(|process| !process.closed())
            .cloned()
            .collect()
    }

    async fn start(
        self: &Arc<Self>,
        key: &str,
        launch: Launch,
        shutdowns: u64,
        cancel: &CancellationToken,
    ) -> io::Result<Arc<Process>> {
        let mut halted = self.shutdowns.subscribe();
        let Launch {
            mut start,
            temporary,
            credential,
        } = launch;
        let mut meta = json!({
            "kind": self.flavour.kind().name(), "provider": self.provider, "key": key,
            "turns": {}, "sessions": {}, "initialized": false,
        });
        if let Some(version) = self.flavour.version() {
            meta["version"] = version.into();
        }
        if let Some(temporary) = &temporary {
            meta["temporary"] = temporary.to_string_lossy().into_owned().into();
        }
        if let Some(credential) = &credential {
            meta["credential"] = credential.clone().into();
        }
        start
            .meta
            .extend(meta.as_object().cloned().unwrap_or_default());
        let mut staged = None;
        if let Some(definition) = self.flavour.program()
            && let Some(exe) = start.command.first().cloned()
            && !exe.starts_with(self.home.root())
        {
            let home_root = self.home.root().to_owned();
            let program = self.flavour.kind().name().to_owned();
            let npm_package = definition.npm_package;
            let prepared = tokio::task::spawn_blocking(move || {
                crate::program_updates::store::prepare(
                    &home_root,
                    &program,
                    &exe,
                    npm_package.as_deref(),
                )
            })
            .await
            .map_err(io::Error::other)??;
            if let Some(first) = start.command.first_mut() {
                *first = prepared.executable.clone();
            }
            if start
                .env
                .contains_key(std::ffi::OsStr::new("KUVO_AGY_PROGRAM"))
            {
                start.env.insert(
                    "KUVO_AGY_PROGRAM".into(),
                    prepared.executable.clone().into_os_string(),
                );
            }
            staged = Some(prepared);
        }
        let controller = match Directory::for_home(&self.home)
            .start_cancellable(&self.holder_binary, start, async {
                tokio::select! {
                    () = cancel.cancelled() => {},
                    _ = halted.wait_for(|now| *now != shutdowns) => {},
                }
            })
            .await
        {
            Ok(controller) => {
                drop(staged);
                controller
            }
            Err(error) => {
                drop(staged);
                remove(temporary);
                return Err(error);
            }
        };
        let process = self.serve(
            controller,
            key.to_owned(),
            credential,
            temporary,
            None,
            1 << 30,
            Map::new(),
            Map::new(),
        );
        let said = initialize(
            self.flavour.file_system(),
            self.flavour.initialize_meta(),
            self.flavour.client_capabilities(),
        );
        let initialize = process
            .rpc
            .request("initialize", Some(said), Some(INITIALIZE));
        let initialized = tokio::select! {
            initialized = initialize => initialized.map_err(|failure| self.failed(&process, failure)),
            _ = halted.wait_for(|now| *now != shutdowns) => Err(stopped(self.name())),
        };
        match initialized {
            Ok(init) => {
                let _ = process.capabilities.set(Capabilities::read(&init));
            }
            Err(error) => {
                process.rpc.stop().await;
                process.rpc.ended(ENDS).await;
                return Err(error);
            }
        }
        if self.flavour.authenticates_first()
            && let Err(error) = self.authenticate(&process).await
        {
            process.rpc.stop().await;
            return Err(error);
        }
        process
            .rpc
            .note(Metadata::from_iter([
                ("initialized".into(), Value::Bool(true)),
                ("capabilities".into(), process.capabilities().to_value()),
            ]))
            .await;
        Ok(process)
    }

    /// Asks it to sign in the way it does. Its refusal that says the sign-in is gone ends it here.
    pub async fn authenticate(&self, process: &Process) -> io::Result<()> {
        let Some(method) = self.flavour.auth_method(&process.capabilities()) else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} isn't signed in. Sign in again in Settings › Accounts.",
                    self.name()
                ),
            ));
        };
        match process
            .rpc
            .request(
                "authenticate",
                Some(json!({"methodId": method})),
                Some(AUTHENTICATE),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(crate::rpc::Failure::Refused(error)) if self.flavour.sign_in_refused(&error) => {
                self.flavour.sign_in_ended();
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "{} needs you to sign in again: Settings › Accounts.",
                        self.name()
                    ),
                ))
            }
            Err(failure) => Err(self.failed(process, failure)),
        }
    }

    /// Why a request found no answer, in the words that stopped it when a line did.
    pub fn failed(&self, process: &Process, failure: crate::rpc::Failure) -> io::Error {
        let stopping = lock(&process.stopping);
        if stopping.asks_sign_in {
            return io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} needs you to sign in again: Settings › Accounts.",
                    self.name()
                ),
            );
        }
        failure.into()
    }

    /// A process an earlier resident left held, this resident's own again as it is. One running an
    /// older staged version is marked outdated so its running turns finish while new turns launch
    /// the current version.
    pub async fn adopt(
        self: &Arc<Self>,
        controller: Controller,
        program_path: &std::path::Path,
    ) -> io::Result<Arc<Process>> {
        let meta = controller.welcome.meta.clone();
        let first = controller
            .welcome
            .generation
            .saturating_mul(1 << 24)
            .saturating_add(1 << 30);
        let key = meta
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let credential = meta
            .get("credential")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let capabilities = meta.get("capabilities").map(Capabilities::read);
        let temporary = meta
            .get("temporary")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let object = |name: &str| {
            meta.get(name)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        let process = self.serve(
            controller,
            key,
            credential,
            temporary,
            capabilities,
            first,
            object("turns"),
            object("sessions"),
        );
        if crate::program_updates::store::is_outdated_executable(
            self.home.root(),
            self.flavour.kind().name(),
            program_path,
        ) {
            process.retire();
        }
        self.slots.lock().await.processes.push(process.clone());
        self.watch_idle();
        self.note_activity();
        Ok(process)
    }

    /// One an earlier resident left as it started, never initialized: of no use.
    pub async fn discard(&self, controller: Controller) {
        let (rpc, _incoming) = Rpc::over(
            controller,
            Options {
                framing: Framing::JsonRpc2,
                first_id: 1,
                filter: None,
            },
        );
        rpc.stop().await;
        rpc.ended(ENDS).await;
    }

    #[allow(clippy::too_many_arguments)]
    fn serve(
        self: &Arc<Self>,
        controller: Controller,
        key: String,
        credential: Option<String>,
        temporary: Option<PathBuf>,
        capabilities: Option<Capabilities>,
        first_id: u64,
        turns: Map<String, Value>,
        sessions: Map<String, Value>,
    ) -> Arc<Process> {
        let stopping = Arc::new(Plain::new(Stopping::default()));
        let stop = Arc::new(Notify::new());
        let filter = {
            let flavour = self.flavour.clone();
            let stopping = stopping.clone();
            let stop = stop.clone();
            Box::new(move |line: &str| match flavour.line(line) {
                Line::Protocol => true,
                Line::AsksSignIn => {
                    lock(&stopping).asks_sign_in = true;
                    stop.notify_one();
                    false
                }
            })
        };
        let (rpc, mut incoming) = Rpc::over(
            controller,
            Options {
                framing: Framing::JsonRpc2,
                first_id,
                filter: Some(filter),
            },
        );
        let mut waiting: HashSet<String> = turns.keys().cloned().collect();
        if sessions.values().any(|session| {
            session["backgroundTasks"]
                .as_array()
                .is_some_and(|tasks| !tasks.is_empty())
        }) {
            waiting.insert("background".into());
        }
        let (gate, mut open) = watch::channel(waiting.is_empty());
        let process = Arc::new(Process {
            id: self.next.fetch_add(1, Ordering::Relaxed) + 1,
            key,
            rpc,
            capabilities: capabilities
                .map(std::sync::OnceLock::from)
                .unwrap_or_default(),
            credential,
            temporary,
            started: Instant::now(),
            last_use: Plain::new(Instant::now()),
            idle_changes: self.idle_changes.clone(),
            outdated: AtomicBool::new(false),
            stopping,
            turns: Plain::new(turns),
            sessions: Plain::new(sessions),
            opening: Plain::default(),
            waiting: Plain::new(waiting),
            gate,
        });
        let shared = Arc::downgrade(self);
        let own = Arc::downgrade(&process);
        tokio::spawn(async move {
            let _ =
                tokio::time::timeout(Duration::from_secs(60), open.wait_for(|open| *open)).await;
            loop {
                let message = tokio::select! {
                    message = incoming.recv() => message,
                    () = stop.notified() => {
                        if let Some(process) = own.upgrade() {
                            process.rpc.stop().await;
                        }
                        continue;
                    }
                };
                let (Some(message), Some(shared), Some(process)) =
                    (message, shared.upgrade(), own.upgrade())
                else {
                    return;
                };
                shared.dispatch(&process, message).await;
            }
        });
        process
    }

    /// Where an update or request for `native` goes in `process`: its conversation's events.
    fn route_to(
        &self,
        process: &Process,
        native: Option<&str>,
    ) -> Option<mpsc::UnboundedSender<Event>> {
        route_in(&self.routes(), process, native)
    }

    async fn dispatch(self: &Arc<Self>, process: &Arc<Process>, message: Incoming) {
        match message {
            Incoming::Notification {
                method,
                params,
                sequence,
            } if method == "session/update" => {
                let update = SessionUpdate::read(&params);
                // (Under the routes' lock: a session being opened is known, or what's said of it
                // waits for it.)
                let routes = self.routes();
                match route_in(&routes, process, update.session.as_deref()) {
                    Some(events) => {
                        let _ = events.send(Event::Update {
                            process: process.id,
                            update,
                        });
                    }
                    None => {
                        let mut staging = lock(&process.opening);
                        if staging.opening > 0 && staging.updates.len() < STAGED {
                            staging.updates.push_back((update, sequence));
                        }
                    }
                }
            }
            Incoming::Notification { method, params, .. } => {
                let Some(extension) = self.flavour.extension(&method, &params) else {
                    return;
                };
                let native = params["sessionId"].as_str();
                if let Some(events) = self.route_to(process, native) {
                    let _ = events.send(Event::Extension { extension });
                }
            }
            Incoming::Request {
                id, method, params, ..
            } => {
                let native = params["sessionId"]
                    .as_str()
                    .or_else(|| params["params"]["sessionId"].as_str());
                let unsent = match self.route_to(process, native) {
                    Some(events) => events
                        .send(Event::Request {
                            process: process.clone(),
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
                    process.rpc.respond(id, unrouted(&method)).await;
                }
            }
            Incoming::Answer { id, answer, .. } => {
                for route in self
                    .routes()
                    .values()
                    .filter(|route| route.process == Some(process.id))
                {
                    let _ = route.events.send(Event::Answer {
                        process: process.id,
                        id: id.into(),
                        answer: answer.clone(),
                    });
                }
            }
            Incoming::Stray { id, answer } => {
                for route in self
                    .routes()
                    .values()
                    .filter(|route| route.process == Some(process.id))
                {
                    let _ = route.events.send(Event::Answer {
                        process: process.id,
                        id: id.clone(),
                        answer: answer.clone(),
                    });
                }
            }
            Incoming::Exited { code, .. } => {
                self.slots
                    .lock()
                    .await
                    .processes
                    .retain(|running| running.id != process.id);
                self.note_activity();
                let stopping = lock(&process.stopping);
                let stderr = process.rpc.stderr();
                let lines: Vec<_> = stderr.trim().lines().collect();
                let said = lines[lines.len().saturating_sub(3)..].join("\n");
                let exit = Exit {
                    said: if stopping.asks_sign_in {
                        format!(
                            "{} needs you to sign in again: Settings › Accounts.",
                            self.name()
                        )
                    } else {
                        said
                    },
                    asks_sign_in: stopping.asks_sign_in,
                    young: process.young(),
                    by_itself: code != -1 && !stopping.asks_sign_in,
                };
                drop(stopping);
                if exit.asks_sign_in && exit.young {
                    self.flavour.sign_in_ended();
                }
                for route in self.routes().values() {
                    let _ = route.events.send(Event::Stopped {
                        process: process.id,
                        exit: exit.clone(),
                    });
                }
                remove(process.temporary.clone());
            }
        }
    }

    /// Stop idle and outdated processes at their real deadlines or on a route/turn change.
    fn watch_idle(self: &Arc<Self>) {
        if self.idle_check.swap(true, Ordering::Relaxed) { return; }
        let shared = Arc::downgrade(self);
        let mut changes = self.idle_changes.subscribe();
        tokio::spawn(async move {
            loop {
                let Some(here) = shared.upgrade() else { return; };
                here.stop_quiet(false).await;
                let next = {
                    let slots = here.slots.lock().await;
                    slots.processes.iter().filter(|process| !here.busy(process.id))
                        .map(|process| if process.outdated() { Instant::now() } else { *lock(&process.last_use) + IDLE }).min()
                };
                drop(here);
                tokio::select! {
                    changed = changes.changed() => if changed.is_err() { return; },
                    _ = async { match next {
                        Some(at) => timer_fires::sleep_until(Timer::AcpServerExpiry, at).await,
                        None => std::future::pending().await,
                    } } => {},
                }
            }
        });
    }

    /// Stops outdated processes nothing is busy on, and (`idle` or after a while) those no
    /// conversation is in.
    pub async fn stop_quiet(&self, idle: bool) {
        let mut slots = self.slots.lock().await;
        let mut stopping = Vec::new();
        for process in &slots.processes {
            if self.busy(process.id) {
                *lock(&process.last_use) = Instant::now();
            } else if process.outdated() || idle || lock(&process.last_use).elapsed() >= IDLE {
                stopping.push(process.clone());
            }
        }
        slots
            .processes
            .retain(|process| !stopping.iter().any(|stop| stop.id == process.id));
        drop(slots);
        for process in stopping {
            eprintln!(
                "Stopping {}: {}",
                self.name(),
                if process.outdated() {
                    "outdated"
                } else {
                    "idle"
                }
            );
            process.rpc.stop().await;
        }
    }

    /// Every process now: none takes new sessions, each stops once quiet.
    pub async fn retire_all(&self) {
        for process in self.running().await {
            process.retire();
        }
        self.stop_quiet(false).await;
    }

    /// A start under way fails, and stops what it started.
    pub fn halt(&self) {
        self.shutdowns.send_modify(|count| *count += 1);
    }

    /// Stops every process (and one starting: its start then fails).
    pub async fn shutdown(&self) {
        self.halt();
        let _ = tokio::time::timeout(ENDS, self.starting.lock()).await;
        let processes = std::mem::take(&mut self.slots.lock().await.processes);
        self.note_activity();
        for process in processes {
            eprintln!("Stopping {}: shutdown", self.name());
            process.rpc.stop().await;
            process.rpc.ended(ENDS).await;
        }
    }

    /// Lets go of every process for the next resident, as they are.
    pub async fn detach(&self) {
        self.halt();
        self.slots.lock().await.processes.clear();
        self.note_activity();
    }

    /// A turn taken up that no conversation here goes on with: cancelled in the agent, and the
    /// lines of the others come.
    pub async fn let_go(&self, session: &str) -> io::Result<()> {
        for process in self.running().await {
            let native = lock(&process.turns)
                .iter()
                .find(|(_, note)| note["session"] == session)
                .map(|(native, _)| native.clone());
            let Some(native) = native else { continue };
            process.note_turn(&native, None).await;
            process
                .rpc
                .notify("session/cancel", Some(json!({"sessionId": native})))
                .await;
            process.ready(&native);
        }
        Ok(())
    }
}

/// What the agent hears when no conversation takes its request.
fn unrouted(method: &str) -> Result<Value, RpcError> {
    match method {
        "session/request_permission" => Ok(json!({"outcome": {"outcome": "cancelled"}})),
        "elicitation/create" => Ok(json!({"action": "decline"})),
        _ => Err(RpcError::not_found(method)),
    }
}

/// The conversation `native` is in `process`, or (unnamed) the one conversation in it.
fn route_in(
    routes: &HashMap<String, Route>,
    process: &Process,
    native: Option<&str>,
) -> Option<mpsc::UnboundedSender<Event>> {
    let mut in_it = routes
        .values()
        .filter(|route| route.process == Some(process.id));
    match native {
        Some(native) => in_it
            .find(|route| route.native.as_deref() == Some(native))
            .map(|route| route.events.clone()),
        None => {
            let only = in_it.next()?;
            in_it.next().is_none().then(|| only.events.clone())
        }
    }
}

fn remove(folder: Option<PathBuf>) {
    if let Some(folder) = folder {
        let _ = std::fs::remove_dir_all(folder);
    }
}

pub(super) fn lock<T>(value: &Plain<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(super) fn stopped(name: &str) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, format!("{name} stopped."))
}
