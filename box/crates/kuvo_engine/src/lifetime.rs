use kuvo_os::timer_fires::{self, Timer};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use kuvo_os::{
    platform::Environment,
    process::{ProcessIdentity, ProcessState},
};
use serde::Deserialize;
use tokio::{
    fs,
    sync::{mpsc, oneshot, watch},
    time::{self, Instant},
};

use crate::{
    launch::{self, Record, Worker},
    proxy::{Upstream, busy},
};

#[derive(Clone, Copy)]
pub struct Timing {
    pub idle: Duration,
    pub check: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(600),
            check: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Starting,
    Running {
        version: Option<String>,
        started: Option<SystemTime>,
    },
    Stopped,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) upstream: Upstream,
    pub(crate) context_windows: crate::context::Windows,
    pub(crate) catalogs: crate::models::CatalogCache,
    pub(crate) home: PathBuf,
    pub(crate) account_home: PathBuf,
    pub(crate) queues: crate::queue::QueueStore,
    pub(crate) attachment_expiry: crate::attachments::Expiry,
    pub(crate) tool_words: std::sync::Arc<crate::conversation::tool_words::ToolWords>,
    pub(crate) attachment_source: Option<std::sync::Arc<dyn crate::AttachmentSource>>,
    pub(crate) legacy_attachments: Option<PathBuf>,
    pub(crate) agent_models: Option<std::sync::Arc<dyn crate::AgentModelSource>>,
    pub(crate) agent_sessions: Option<std::sync::Arc<dyn crate::AgentSessionSource>>,
    pub(crate) model_stars: Option<std::sync::Arc<dyn crate::ModelStarSource>>,
    pub(crate) external_asks: Option<std::sync::Arc<dyn crate::ExternalAsks>>,
    pub(crate) ownership: Option<std::sync::Arc<dyn crate::SessionOwnership>>,
    pub(crate) peer_waiting:
        Option<tokio::sync::watch::Receiver<std::sync::Arc<Vec<crate::PeerWaiting>>>>,
    pub(crate) raw_prompts: crate::raw_prompts::RawPrompts,
    pub(crate) receipt_index: crate::receipt_index::ReceiptIndex,
    pub(crate) message_clock: std::sync::Arc<std::sync::atomic::AtomicU64>,
    turns: Turns,
    pub(crate) submission_turns: Turns,
    pub(crate) draft_turns: Turns,
    pub(crate) decision_writer: std::sync::Arc<tokio::sync::Semaphore>,
    pub(crate) heard: tokio::sync::broadcast::Sender<std::sync::Arc<EngineEvent>>,
    pub(crate) work_stops:
        std::sync::Arc<std::sync::OnceLock<std::sync::Arc<dyn crate::WorkStops>>>,

    pub(crate) version: String,
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<State>,
}

pub(crate) use crate::turns::Turns;

/// One of the engine's events: its type (`session.execution.succeeded`, `credential.updated`, ...)
/// and what it says.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct EngineEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, alias = "properties")]
    pub data: serde_json::Value,
    /// Where it happened, as the engine says (`directory`), for an event about a folder.
    #[serde(default)]
    pub location: serde_json::Value,
}

pub(crate) struct Lease {
    pub record: Record,
    commands: Option<mpsc::UnboundedSender<Command>>,
    started_for_install: Option<ProcessIdentity>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Release);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Demand {
    Start,
    Existing,
    Owned,
    Passive,
}

#[derive(Clone, Copy)]
pub(crate) enum Control {
    Start,
    Restart,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeaseUse {
    Resident,
    Install,
}

const INSTALL_ENGINE: &str = "run/ssh-engine.json";

enum Command {
    Acquire(Demand, LeaseUse, oneshot::Sender<Option<Lease>>),
    StopInstall(ProcessIdentity, oneshot::Sender<io::Result<()>>),
    Release,
    Control(Control),
    Drain(oneshot::Sender<io::Result<bool>>),
    ToolServer(serde_json::Value),
}

impl Engine {
    /// Install the resident's own tool server before the engine starts. The
    /// engine's other configuration remains untouched.
    pub fn set_own_tool_server(&self, server: serde_json::Value) {
        let _ = self.commands.send(Command::ToolServer(server));
    }

    pub fn new(home: PathBuf, env: Environment, timing: Timing) -> io::Result<Self> {
        crate::notes::clear_stale(&home);
        let account_home = kuvo_os::platform::Account::current()
            .and_then(|account| account.home_folder(kuvo_os::platform::Platform::current(), &env))
            .unwrap_or_default();
        let upstream = Upstream::new().map_err(io::Error::other)?;
        let tool_words = std::sync::Arc::new(
            crate::conversation::tool_words::ToolWords::new().map_err(io::Error::other)?,
        );
        let version = launch::pinned_version();
        let (commands, inbox) = mpsc::unbounded_channel();
        let initial = if home.join("run/stopped").exists() {
            State::Stopped
        } else {
            State::Idle
        };
        let (updates, state) = watch::channel(initial);
        tokio::spawn(
            Supervisor {
                home: home.clone(),
                version: version.clone(),
                env,
                timing,
                upstream: upstream.clone(),
                updates,
                commands: commands.downgrade(),
                worker: None,
                leases: 0,
                used: Instant::now(),
                activity: 0,
                recover: false,
                healthy: false,
            }
            .run(inbox),
        );
        Ok(Self {
            upstream,
            turns: Turns::new(),
            submission_turns: Turns::new(),
            draft_turns: Turns::new(),
            decision_writer: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            heard: tokio::sync::broadcast::channel(256).0,
            work_stops: std::sync::Arc::default(),
            context_windows: crate::context::Windows::open(&home),
            catalogs: crate::models::CatalogCache::new(),
            queues: crate::queue::QueueStore::open(&home),
            attachment_expiry: Default::default(),
            tool_words,
            attachment_source: None,
            legacy_attachments: None,
            agent_models: None,
            agent_sessions: None,
            model_stars: None,
            external_asks: None,
            ownership: None,
            peer_waiting: None,
            raw_prompts: crate::raw_prompts::RawPrompts::start(),
            receipt_index: crate::receipt_index::ReceiptIndex::start(&home),
            message_clock: Default::default(),
            home,
            account_home,
            version,
            commands,
            state,
        })
    }

    pub(crate) async fn turn(&self, session: &str) -> crate::turns::Turn {
        self.turns.acquire(session).await
    }

    /// The engine's events as its feed hears them, for those that follow what its sessions do
    /// (one that falls behind misses some, and hears that it did).
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<std::sync::Arc<EngineEvent>> {
        self.heard.subscribe()
    }

    /// The engine's state as it changes.
    pub fn states(&self) -> watch::Receiver<State> {
        self.state.clone()
    }

    pub async fn state(&self) -> State {
        if stopped(&self.home).await {
            State::Stopped
        } else {
            self.state.borrow().clone()
        }
    }

    pub(crate) async fn acquire(&self, demand: Demand) -> Option<Lease> {
        self.request_lease(demand, LeaseUse::Resident).await
    }

    async fn request_lease(&self, demand: Demand, purpose: LeaseUse) -> Option<Lease> {
        if demand != Demand::Start && *self.state.borrow() == State::Starting {
            return None;
        }
        let (answer, reply) = oneshot::channel();
        self.commands
            .send(Command::Acquire(demand, purpose, answer))
            .ok()?;
        reply.await.ok().flatten()
    }

    /// Hold OpenCode open while a staged installation imports sign-ins. A failed import
    /// stops the engine only when this call started and still owns the same process.
    pub async fn with_install_engine<T>(
        &self,
        import: impl Future<Output = io::Result<T>>,
    ) -> io::Result<T> {
        let lease = self
            .request_lease(Demand::Start, LeaseUse::Install)
            .await
            .ok_or_else(|| io::Error::other("The staged engine did not start."))?;
        let result = import.await;
        let started = lease.started_for_install.clone();
        drop(lease);
        if result.is_err()
            && let Some(started) = started
        {
            self.stop_staged_worker(started).await?;
        }
        result
    }

    /// Resume cleanup after the guarded install command that started OpenCode exited.
    pub async fn stop_install_engine(&self) -> io::Result<()> {
        let bytes = match fs::read(self.home.join(INSTALL_ENGINE)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return match fs::symlink_metadata(self.home.join("run/engine.json")).await {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                    Ok(_) => Err(io::Error::other(
                        "There is no receipt for the running engine.",
                    )),
                };
            }
            Err(error) => return Err(error),
        };
        self.stop_staged_worker(serde_json::from_slice(&bytes)?)
            .await
    }

    /// Once enrollment commits, the resident owns this engine as usual.
    pub fn finish_install_engine(home: &Path) -> io::Result<()> {
        kuvo_store::files::remove_file_synced(&home.join(INSTALL_ENGINE))
    }

    async fn stop_staged_worker(&self, identity: ProcessIdentity) -> io::Result<()> {
        let (reply, result) = oneshot::channel();
        let stopped = || io::Error::other("The staged engine coordinator stopped.");
        self.commands
            .send(Command::StopInstall(identity, reply))
            .map_err(|_| stopped())?;
        result.await.map_err(|_| stopped())?
    }

    pub(crate) async fn quiet_for_release(&self) -> io::Result<bool> {
        // A borrowed engine may answer the same endpoints but its inbox is
        // not ours to move. Probe a verified worker without blocking the actor.
        let Some(lease) = self.acquire(Demand::Owned).await else {
            return Ok(!fs::try_exists(self.home.join("run/engine.json")).await?);
        };
        Ok(!browser_asks(&self.home).await && !busy(&self.upstream, &lease.record).await)
    }

    pub(crate) async fn stop_for_release(&self) -> io::Result<bool> {
        let (answer, reply) = oneshot::channel();
        self.commands
            .send(Command::Drain(answer))
            .map_err(|_| io::Error::other("The engine supervisor stopped."))?;
        reply.await.map_err(io::Error::other)?
    }

    pub(crate) async fn control(&self, action: Control) -> Result<(), &'static str> {
        if !launch::executable(&self.home, &self.version).is_file() {
            return Err("Kuvo Code hasn't set this computer up yet.");
        }
        if starting_fenced(&self.home).await {
            return Err("This computer is updating. Its engine starts again once that's done.");
        }
        self.commands
            .send(Command::Control(action))
            .map_err(|_| "It couldn't be started.")
    }

    pub(crate) async fn stopped_intentionally(&self) -> bool {
        exists(&self.home, "stopped").await
    }

    pub(crate) async fn retry_minutes(&self) -> Option<u64> {
        let seconds = fs::read_to_string(self.home.join("run/engine.retry"))
            .await
            .ok()?;
        let retry: u64 = seconds.trim().parse().ok()?;
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let remaining = retry.saturating_sub(now);
        (remaining > 30).then(|| remaining.div_ceil(60))
    }

    pub(crate) async fn is_mac_box(&self) -> bool {
        fs::read_to_string(self.home.join("box/kind"))
            .await
            .is_ok_and(|kind| kind.trim() == "mac")
    }
}

struct Supervisor {
    home: PathBuf,
    version: String,
    env: Environment,
    timing: Timing,
    upstream: Upstream,
    updates: watch::Sender<State>,
    commands: mpsc::WeakUnboundedSender<Command>,
    worker: Option<Worker>,
    leases: usize,
    used: Instant,
    activity: u64,
    recover: bool,
    healthy: bool,
}

impl Supervisor {
    fn own_tool_server(&mut self, server: serde_json::Value) {
        let mut config = match self.env.get(std::ffi::OsStr::new("OPENCODE_CONFIG_CONTENT")) {
            Some(raw) if !raw.is_empty() => match raw.to_str().and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok()) {
                Some(config) => config,
                None => return,
            },
            _ => serde_json::json!({}),
        };
        if !config.is_object() {
            return;
        }
        if !config["mcp"].is_object() {
            config["mcp"] = serde_json::json!({});
        }
        if !config["mcp"]["servers"].is_object() {
            config["mcp"]["servers"] = serde_json::json!({});
        }
        config["mcp"]["servers"]["kuvo-browser"] = server;
        self.env
            .insert("OPENCODE_CONFIG_CONTENT".into(), config.to_string().into());
    }

    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Command>) {
        if let Err(error) = self.load().await {
            eprintln!("Reading the engine record: {error}");
        }
        let run = self.home.join("run");
        let mut files = kuvo_os::watch::FileSet::new(kuvo_os::watch::WatchHub::default());
        files.set_targets(["off", "paused", "stopped", "fleet-engine-hold.json", "previous",
            "engine-swap.json", "update.json", ".updating", ".updating/pid"].into_iter()
            .map(|name| run.join(name)).collect(), Vec::new());
        let mut probing = tokio::task::JoinSet::new();
        let mut watched = None;
        let mut exited = None;
        let mut retry = None;
        let mut fallback = Instant::now() + self.timing.check;
        loop {
            let current = self.worker.as_ref().map(|worker| worker.identity().clone());
            if current != watched {
                exited = current.as_ref().and_then(|id| id.watch_exit().ok());
                watched = current;
                fallback = Instant::now() + self.timing.check;
                retry = None;
            }
            let deadline = self.next_check(!probing.is_empty(), exited.is_some(), retry, fallback).await;
            let look = tokio::select! {
                biased;
                command = inbox.recv() => match command {
                    Some(Command::Acquire(demand, purpose, reply)) if !reply.is_closed() => {
                        self.acquire(demand, purpose, reply).await;
                        false
                    }
                    Some(Command::StopInstall(identity, reply)) => {
                        let _ = reply.send(self.stop_install(identity).await);
                        false
                    }
                    Some(Command::Release) => {
                        self.leases -= 1;
                        self.touch();
                        false
                    }
                    Some(Command::Control(action)) => {
                        if let Err(error) = self.control(action).await {
                            eprintln!("Starting the engine on request: {error}");
                        }
                        false
                    }
                    Some(Command::Drain(reply)) => {
                        let _ = reply.send(self.drain().await);
                        false
                    }
                    Some(Command::ToolServer(server)) => {
                        self.own_tool_server(server);
                        false
                    }
                    Some(Command::Acquire(..)) => false,
                    None => break,
                },
                Some(result) = probing.join_next(), if !probing.is_empty() => {
                    if let Ok(probe) = result {
                        if let Err(error) = self.park_if_quiet(probe).await {
                            eprintln!("Parking the engine: {error}");
                            retry = Some(Instant::now() + self.timing.check);
                        }
                    }
                    false
                },
                _ = wait_check(deadline) => true,
                _ = files.changed() => true,
                _ = wait_exit(&mut exited) => { exited = None; true },
            };
            if look {
                retry = None;
                fallback = Instant::now() + self.timing.check;
                if let Err(error) = self.look(&mut probing).await {
                    eprintln!("Checking the engine: {error}");
                    retry = Some(Instant::now() + self.timing.check);
                }
            }
        }
    }

    async fn next_check(&self, probing: bool, watching_exit: bool, retry: Option<Instant>, fallback: Instant) -> Option<Instant> {
        // In a never-used engine there is no process, recovery or work to check.
        if self.worker.is_none() && !self.recover { return None; }
        let now = Instant::now();
        let not_before = retry.unwrap_or(now);
        if stopped(&self.home).await {
            // With no process-watch support, still reap an actual process.
            return (self.worker.is_some() && !watching_exit).then_some(fallback);
        }
        if self.worker.is_none() || !self.healthy || !watching_exit {
            return Some(fallback.max(not_before));
        }
        if probing || self.leases != 0 || !self.worker.as_ref().is_some_and(Worker::owned) {
            return None;
        }
        let idle_at = self.used + self.timing.idle;
        if idle_at > now { return Some(idle_at.max(not_before)); }
        // A live update or swap can disappear without cleaning its lock if killed. This
        // is an outstanding park/recovery, not a periodic check of an idle box.
        if crate::release::holding(&self.home.join("run")) || switching(&self.home).await || updating(&self.home).await {
            return Some(fallback.max(not_before));
        }
        Some(not_before)
    }

    async fn load(&mut self) -> io::Result<()> {
        let home = self.home.clone();
        self.worker = tokio::task::spawn_blocking(move || Worker::load(&home)).await??;
        if self.worker.is_some() {
            self.recover = true;
            if let Err(error) = self.ready().await {
                // A verified process that hasn't answered yet still owns the data.
                eprintln!("Checking the adopted engine: {error}");
            }
        } else {
            self.publish().await;
        }
        Ok(())
    }

    async fn acquire(
        &mut self,
        demand: Demand,
        purpose: LeaseUse,
        reply: oneshot::Sender<Option<Lease>>,
    ) {
        let mut started_for_install = None;
        let result = async {
            self.reap().await?;
            if self.worker.is_none()
                && demand == Demand::Start
                && !stopped(&self.home).await
                && !starting_fenced(&self.home).await
            {
                started_for_install = self.start(purpose).await?;
            } else if self.worker.is_some() && !self.healthy && demand == Demand::Start {
                self.ready().await?;
            }
            Ok::<_, io::Error>(())
        }
        .await;
        if let Err(ref error) = result {
            eprintln!("Preparing the engine: {error}");
        }
        let lease = self
            .worker
            .as_ref()
            .filter(|worker| {
                self.healthy && result.is_ok() && (demand != Demand::Owned || worker.owned())
            })
            .map(|worker| {
                let commands = if demand != Demand::Passive {
                    self.commands.upgrade()
                } else {
                    None
                };
                Lease {
                    record: worker.record.clone(),
                    commands,
                    started_for_install,
                }
            });
        if lease.as_ref().is_some_and(|lease| lease.commands.is_some()) {
            self.leases += 1;
            self.touch();
        }
        let _ = reply.send(lease);
    }

    fn touch(&mut self) {
        self.used = Instant::now();
        self.activity = self.activity.wrapping_add(1);
    }

    async fn start(&mut self, purpose: LeaseUse) -> io::Result<Option<ProcessIdentity>> {
        let _ = self.updates.send(State::Starting);
        kuvo_os::process::wait_for_background_memory().await;
        let home = self.home.clone();
        let version = self.version.clone();
        let env = self.env.clone();
        let start = tokio::task::spawn_blocking(move || {
            // Recheck discovery under the resident's home claim before spawning.
            if let Some(worker) = Worker::load(&home)? {
                return Ok(worker);
            }
            Worker::launch(&home, &version, env)
        })
        .await?;
        let staged = match start {
            Ok(worker) => {
                let staged = (purpose == LeaseUse::Install && worker.spawned())
                    .then(|| worker.record.identity())
                    .flatten();
                self.worker = Some(worker);
                if let Some(identity) = &staged
                    && let Err(error) = kuvo_store::files::write_atomic(
                        &self.home.join(INSTALL_ENGINE),
                        &serde_json::to_vec(identity)?,
                    )
                {
                    self.stop_install(identity.clone()).await?;
                    return Err(error);
                }
                staged
            }
            Err(error) => {
                self.publish().await;
                return Err(error);
            }
        };
        match self.ready().await {
            Ok(()) => {
                self.recover = true;
                self.touch();
                Ok(staged)
            }
            Err(error) => {
                if let Some(identity) = staged {
                    self.stop_install(identity).await?;
                }
                self.publish().await;
                Err(error)
            }
        }
    }

    async fn ready(&mut self) -> io::Result<()> {
        let worker = self.worker.as_ref().expect("worker present");
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if !worker.alive()? {
                self.reap().await?;
                return Err(io::Error::other("OpenCode exited before it answered"));
            }
            if self.upstream.healthy(&worker.record).await && worker.alive()? {
                self.healthy = true;
                self.publish().await;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(
                    "OpenCode didn't answer its authenticated health check",
                ));
            }
            time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn publish(&self) {
        let state = if stopped(&self.home).await {
            State::Stopped
        } else if let Some(worker) = self.worker.as_ref().filter(|_| self.healthy) {
            State::Running {
                version: worker.record.version.clone(),
                started: worker.started,
            }
        } else {
            State::Idle
        };
        self.updates.send_if_modified(|previous| {
            if *previous == state {
                return false;
            }
            *previous = state;
            true
        });
    }

    async fn reap(&mut self) -> io::Result<()> {
        if let Some(worker) = &self.worker
            && !worker.alive()?
        {
            self.stop().await?;
        }
        Ok(())
    }

    async fn stop(&mut self) -> io::Result<()> {
        self.healthy = false;
        let Some(mut worker) = self.worker.take() else {
            return Ok(());
        };
        let (worker, result) = tokio::task::spawn_blocking(move || {
            let result = worker.stop();
            (worker, result)
        })
        .await?;
        if let Err(error) = result {
            self.worker = Some(worker);
            return Err(error);
        }
        let path = self.home.join("run/engine.json");
        match fs::remove_file(path).await {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        self.publish().await;
        Ok(())
    }

    async fn stop_install(&mut self, identity: ProcessIdentity) -> io::Result<()> {
        match &self.worker {
            Some(worker)
                if self.leases == 0
                    && worker.owned()
                    && worker.record.identity().as_ref() == Some(&identity) =>
            {
                self.stop().await?;
            }
            None if identity.state() == ProcessState::Stale => (),
            _ => {
                return Err(io::Error::other(
                    "The engine is no longer owned by this installation attempt.",
                ));
            }
        }
        match fs::remove_file(self.home.join(INSTALL_ENGINE)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn drain(&mut self) -> io::Result<bool> {
        self.reap().await?;
        let Some(worker) = &self.worker else {
            return Ok(!fs::try_exists(self.home.join("run/engine.json")).await?);
        };
        if !worker.owned()
            || !self.healthy
            || self.leases != 0
            || browser_asks(&self.home).await
            || busy(&self.upstream, &worker.record).await
        {
            return Ok(false);
        }
        self.recover = false;
        self.stop().await?;
        Ok(true)
    }

    async fn control(&mut self, action: Control) -> io::Result<()> {
        if starting_fenced(&self.home).await
            || exists(&self.home, "off").await
            || exists(&self.home, "paused").await
        {
            return Ok(());
        }
        match fs::remove_file(self.home.join("run/stopped")).await {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        self.reap().await?;
        if matches!(action, Control::Restart) {
            self.stop().await?;
        }
        if self.worker.is_none() {
            self.start(LeaseUse::Resident).await?;
        }
        self.publish().await;
        Ok(())
    }

    async fn look(&mut self, probing: &mut tokio::task::JoinSet<IdleProbe>) -> io::Result<()> {
        self.reap().await?;
        self.publish().await;
        if stopped(&self.home).await {
            return Ok(());
        }
        let Some(worker) = &self.worker else {
            if self.recover && !starting_fenced(&self.home).await {
                self.start(LeaseUse::Resident).await?;
            }
            return Ok(());
        };
        if !self.healthy && self.upstream.healthy(&worker.record).await {
            self.healthy = true;
            self.recover = true;
            self.publish().await;
        }
        if probing.is_empty()
            && worker.owned()
            && self.healthy
            && self.idle()
            && !crate::release::holding(&self.home.join("run"))
            && !switching(&self.home).await
            && !updating(&self.home).await
        {
            let home = self.home.clone();
            let upstream = self.upstream.clone();
            let record = worker.record.clone();
            let activity = self.activity;
            probing.spawn(async move {
                IdleProbe {
                    pid: record.pid,
                    activity,
                    busy: browser_asks(&home).await || busy(&upstream, &record).await,
                }
            });
        }
        Ok(())
    }

    fn idle(&self) -> bool {
        self.leases == 0 && self.used.elapsed() >= self.timing.idle
    }

    async fn park_if_quiet(&mut self, probe: IdleProbe) -> io::Result<()> {
        if probe.activity != self.activity
            || !self.idle()
            || !self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.record.pid == probe.pid)
        {
            return Ok(());
        }
        if probe.busy {
            self.touch();
        } else if !stopped(&self.home).await
            && !crate::release::holding(&self.home.join("run"))
            && !switching(&self.home).await
            && !updating(&self.home).await
        {
            self.recover = false;
            self.stop().await?;
        }
        Ok(())
    }
}

async fn wait_check(at: Option<Instant>) {
    match at {
        Some(at) => timer_fires::sleep_until(Timer::EngineLifetime, at).await,
        None => std::future::pending().await,
    }
}

async fn wait_exit(exit: &mut Option<oneshot::Receiver<io::Result<()>>>) {
    match exit {
        Some(exit) => { let _ = exit.await; }
        None => std::future::pending().await,
    }
}

struct IdleProbe {
    pid: u32,
    activity: u64,
    busy: bool,
}

async fn exists(home: &Path, mark: &str) -> bool {
    fs::try_exists(home.join("run").join(mark))
        .await
        .unwrap_or(true)
}

async fn stopped(home: &Path) -> bool {
    exists(home, "off").await || exists(home, "paused").await || exists(home, "stopped").await
}

async fn starting_fenced(home: &Path) -> bool {
    if crate::release::holding(&home.join("run")) || switching(home).await {
        return true;
    }
    if !updating(home).await {
        return false;
    }
    #[derive(Deserialize)]
    struct Update {
        running: Option<String>,
    }
    let stage = read::<Update>(home, "update.json")
        .await
        .and_then(|update| update.running);
    !matches!(
        stage.as_deref(),
        Some("checking" | "downloading" | "downloading the engine" | "checking it")
    )
}

async fn switching(home: &Path) -> bool {
    #[derive(Deserialize)]
    struct Swap {
        pid: u32,
    }
    exists(home, "previous").await
        || read::<Swap>(home, "engine-swap.json")
            .await
            .is_some_and(|swap| pid_live(swap.pid))
}

async fn updating(home: &Path) -> bool {
    let lock = home.join("run/.updating");
    if let Ok(pid) = fs::read_to_string(lock.join("pid")).await
        && let Ok(pid) = pid.trim().parse()
    {
        return pid_live(pid);
    }
    fs::metadata(lock)
        .await
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(60))
}

async fn browser_asks(home: &Path) -> bool {
    #[derive(Deserialize)]
    struct Asks {
        asks: Vec<Ask>,
    }
    #[derive(Deserialize)]
    struct Ask {
        id: String,
        kind: String,
    }
    if !read::<Asks>(home, "browser-asks.json")
        .await
        .is_some_and(|file| {
            file.asks.iter().any(|ask| {
                !ask.id.is_empty() && matches!(ask.kind.as_str(), "signIn" | "code" | "action")
            })
        })
    {
        return false;
    }
    fs::read_to_string(home.join("run/browser.pid"))
        .await
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
        .is_some_and(pid_live)
}

fn pid_live(pid: u32) -> bool {
    !matches!(ProcessIdentity::read(pid), Ok(None))
}

async fn read<T: serde::de::DeserializeOwned>(home: &Path, name: &str) -> Option<T> {
    fs::read(home.join("run").join(name))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}
