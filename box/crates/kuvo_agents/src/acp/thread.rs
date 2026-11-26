//! A conversation's session in its agent: opened in the process its launch shares with others,
//! its turns prompted, what the agent says made the turn's events, and what it asks put to you.
//! One task owns it; what its process says comes to it as events.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as Plain,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use base64::Engine as _;
use serde_json::{Map, Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    background::{Helper, WorkOrder},
    hold::TurnNote,
    rpc::{Failure, RpcError},
    runtime::AdapterCall,
    session::{
        ActiveTurn, Answer, Decision, Delivery, EventEnvelope, Field, FieldKind, FormOption, Limit,
        Outcome, OwnTurns, PermissionAsk, PermissionMode, Preflight, Prompt, PromptFile,
        QuestionAsk, SessionContext, Thread, TurnError, TurnEvent, TurnInput, TurnKind, TurnResult,
        TurnSink, VendorState,
    },
    tool_relay::{AgentTools, ToolRelay},
};

use super::{
    flavour::{Extension, Mode, OwnAnswer, OwnRequest, Task},
    projection::{Context, Projection, Served},
    server::{Event, Exit, Process, Route, Shared, lock, stopped},
    tools::{self, Call, KnownTools},
    wire::{self, SessionUpdate, Setup},
};

/// How long opening a session, choosing its model, or a stop is waited for.
const OPEN: Duration = Duration::from_secs(120);
const STOP: Duration = Duration::from_secs(15);
/// A turn the agent began on its own that goes this long with nothing from it is over; one
/// turned away is let go after as long.
pub(super) const OWN_QUIET: Duration = Duration::from_secs(20);
/// Told to carry on after a refusal of Kuvo's own at most this many times a turn.
const CARRY_ON: u32 = 3;
/// An agent's words for a usage limit.
const LIMIT_WORDS: [&str; 7] = [
    "resource_exhausted",
    "usage limit reached",
    "usage limit exceeded",
    "quota exceeded",
    "exceeded your quota",
    "rate limit exceeded",
    "too many requests",
];

static HOLDERS: AtomicU64 = AtomicU64::new(1 << 40);

type Reply<T> = oneshot::Sender<io::Result<T>>;

enum Command {
    Start(Box<TurnInput>, Arc<dyn TurnSink>, Reply<()>),
    Interrupt(String, Reply<()>),
    RestoreIdle(Arc<Process>, Value, Reply<()>),
    Handover(Reply<()>),
    LetGo(TurnNote, Reply<()>),
    /// `signed_out`: its account signed out (a turn under way fails); otherwise removed, or the
    /// box stopping (it ends as stopped).
    Close(bool, Reply<()>),
}

#[derive(Clone)]
pub(super) struct Conversation {
    commands: mpsc::Sender<Command>,
    /// Where a turn handed to the next resident reports, kept open while this one is here.
    _handed: Arc<Plain<Vec<Arc<dyn TurnSink>>>>,
}

impl Conversation {
    pub fn spawn(shared: Arc<Shared>, relay: Option<ToolRelay>, context: SessionContext) -> Self {
        let (commands, receiver) = mpsc::channel(16);
        let (events_in, events) = mpsc::unbounded_channel();
        let busy = Arc::new(AtomicBool::new(false));
        let native = context
            .state
            .get("acpSession")
            .and_then(Value::as_str)
            .map(str::to_owned);
        shared.routes().insert(
            context.session_id.clone(),
            Route {
                native: None,
                process: None,
                busy: busy.clone(),
                events: events_in,
            },
        );
        let handed = Arc::new(Plain::new(Vec::new()));
        tokio::spawn(
            Actor {
                shared,
                relay,
                holder: HOLDERS.fetch_add(1, Ordering::Relaxed),
                session: context.session_id,
                directory: context.directory,
                state: context.state,
                background: context.background,
                own_turns: context.own_turns,
                native,
                process: None,
                opened_with: None,
                setup: Setup::default(),
                stale: false,
                busy,
                handed: handed.clone(),
                active: None,
                own: None,
                tasks: Vec::new(),
                asks: JoinSet::new(),
                known: KnownTools::default(),
                closing: false,
                deadline: None,
                titled: false,
            }
            .run(receiver, events),
        );
        Self {
            commands,
            _handed: handed,
        }
    }

    async fn send<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> io::Result<T> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        answer
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?
    }

    pub async fn handover(&self) -> io::Result<()> {
        self.send(Command::Handover).await
    }

    pub async fn restore_idle(&self, process: Arc<Process>, note: Value) -> io::Result<()> {
        self.send(|reply| Command::RestoreIdle(process, note, reply))
            .await
    }

    pub async fn let_go(&self, note: TurnNote) -> io::Result<()> {
        self.send(|reply| Command::LetGo(note, reply)).await
    }

    pub async fn close(&self, signed_out: bool) -> io::Result<()> {
        self.send(|reply| Command::Close(signed_out, reply)).await
    }
}

impl Thread for Conversation {
    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>> {
        Box::pin(async move {
            let id = input.id.clone();
            self.send(|reply| Command::Start(Box::new(input), sink, reply))
                .await?;
            Ok(Arc::new(Turn {
                id,
                conversation: self.clone(),
            }) as Arc<dyn ActiveTurn>)
        })
    }

    /// ACP has no way into a prompt under way: the resident cancels and resumes.
    fn takes_steers(&self) -> bool {
        false
    }
}

struct Turn {
    id: String,
    conversation: Conversation,
}

impl ActiveTurn for Turn {
    fn interrupt(&self) -> AdapterCall<'_, ()> {
        Box::pin(
            self.conversation
                .send(|reply| Command::Interrupt(self.id.clone(), reply)),
        )
    }
}

/// A turn under way.
struct Active {
    id: String,
    start_cancel: CancellationToken,
    sink: Arc<dyn TurnSink>,
    projection: Projection,
    process: Arc<Process>,
    model: String,
    variant: Option<String>,
    mode: Mode,
    compact: bool,
    /// The prompt under way (each "carry on" is another): an older one's answer isn't the end.
    prompt: u64,
    prompt_id: Option<String>,
    /// Its prompt's request under way (one taken up from its holder: the earlier resident's).
    request: Option<Value>,
    /// The instructions this message carries, kept as told once the agent took it.
    instructed: Option<String>,
    answered: bool,
    renewed: bool,
    /// The process it was renewed from: what that one says isn't the turn's end.
    replaced: Option<u64>,
    own: bool,
    stopping: bool,
    /// What Kuvo last refused it on its own (your settings, not you), since it was told.
    refused: Option<String>,
    carried_on: u32,
    /// An own turn's quiet end, or a stop's last wait.
    quiet: Option<Instant>,
    /// The agent's own record, looked at for this prompt's end: where it stood as it was sent,
    /// when, the answer seen last look, and when to look next.
    record: Option<Record>,
}

impl Active {
    /// `input`'s turn on `process`, nothing yet sent, told or answered.
    fn new(
        input: &TurnInput,
        sink: Arc<dyn TurnSink>,
        process: Arc<Process>,
        projection: Projection,
    ) -> Self {
        Self {
            id: input.id.clone(),
            start_cancel: input.start_cancel.clone(),
            sink,
            projection,
            process,
            model: input.model.id.clone(),
            variant: input.model.variant.clone(),
            mode: Actor::mode(input),
            compact: false,
            prompt: 0,
            prompt_id: None,
            request: None,
            instructed: None,
            answered: false,
            renewed: false,
            replaced: None,
            own: false,
            stopping: false,
            refused: None,
            carried_on: 0,
            quiet: None,
            record: None,
        }
    }
}

/// A turn that ends before its prompt is sent, failed with `error`'s words.
async fn unsent(sink: &dyn TurnSink, error: impl ToString) -> io::Result<()> {
    let outcome = Outcome::Failed(failure(error.to_string()));
    let result = TurnResult {
        outcome,
        took_prompt: false,
        missed: vec![],
    };
    sink.event(envelope(TurnEvent::Finished(result))).await
}

struct Record {
    mark: i64,
    sent: u64,
    candidate: Option<i64>,
    next: Instant,
}

/// How often an agent's own record is looked at for a prompt's end.
const RECORD_LOOK: Duration = Duration::from_secs(1);

/// A turn the agent began on its own, until a turn here takes it, or it's turned away.
struct Own {
    updates: Vec<SessionUpdate>,
    turned_away: bool,
    ended: bool,
    started: u64,
    /// Let go of once quiet this long.
    forget_at: Option<Instant>,
}

/// What an ask came back with, for the agent and the turn.
enum Settled {
    Asked {
        process: Arc<Process>,
        id: Value,
        answer: Result<Value, RpcError>,
        after: After,
        cancellable: bool,
    },
    /// Whether the resident took a turn the agent began on its own.
    Offered(bool),
}

impl Settled {
    /// An answer that leaves nothing for the turn.
    fn plain(process: Arc<Process>, id: Value, answer: Value) -> Self {
        let after = After::default();
        Self::Asked {
            process,
            id,
            answer: Ok(answer),
            after,
            cancellable: false,
        }
    }
}

/// What a settled ask leaves for the turn.
#[derive(Default)]
struct After {
    /// A read Kuvo did for the agent.
    served: Option<(PathBuf, Served)>,
    /// A call it was told no to, and the words its step shows.
    declined: Option<(String, String)>,
    /// What Kuvo refused on its own.
    refused: Option<String>,
}

struct Actor {
    shared: Arc<Shared>,
    relay: Option<ToolRelay>,
    holder: u64,
    session: String,
    directory: PathBuf,
    state: VendorState,
    background: crate::background::Background,
    own_turns: OwnTurns,
    /// Its session in the agent.
    native: Option<String>,
    process: Option<Arc<Process>>,
    /// What the session was opened with: the process and its servers.
    opened_with: Option<(u64, String)>,
    setup: Setup,
    /// Its process can't go on for it (stopped mid-turn): the next turn starts another.
    stale: bool,
    busy: Arc<AtomicBool>,
    handed: Arc<Plain<Vec<Arc<dyn TurnSink>>>>,
    active: Option<Active>,
    own: Option<Own>,
    /// What it keeps going in the background, with when each started and its order.
    tasks: Vec<(Task, u64, WorkOrder)>,
    asks: JoinSet<Settled>,
    known: KnownTools,
    closing: bool,
    deadline: Option<Instant>,
    /// Its agent's title for it was taken (one that titles itself).
    titled: bool,
}

fn envelope(event: TurnEvent) -> EventEnvelope {
    EventEnvelope {
        position: None,
        delivery: Delivery::Live,
        event,
    }
}

fn failure(message: impl Into<String>) -> TurnError {
    TurnError {
        kind: "agent".into(),
        message: message.into(),
        status: None,
        limit: None,
    }
}

fn now() -> u64 {
    crate::background::now()
}

impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut events: mpsc::UnboundedReceiver<Event>,
    ) {
        while !self.closing {
            let deadline = self.next_deadline();
            let timer = async {
                match deadline {
                    Some(at) => timer_fires::sleep_until(Timer::AcpTurnDeadline, at).await,
                    None => std::future::pending().await,
                }
            };
            let result = tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
                Some(event) = events.recv() => self.event(event).await,
                Some(settled) = self.asks.join_next() => match settled {
                    Ok(settled) => self.settled(settled).await,
                    Err(_) => Ok(()),
                },
                () = timer => self.expired().await,
            };
            if let Err(error) = result {
                eprintln!(
                    "{} conversation {}: {error}",
                    self.shared.name(),
                    self.session
                );
            }
        }
        self.shared.routes().remove(&self.session);
        if let Some(process) = &self.process { process.touch(); } else { self.shared.note_activity(); }
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.active.as_ref().and_then(|active| active.quiet),
            self.active
                .as_ref()
                .and_then(|active| active.record.as_ref())
                .map(|record| record.next),
            self.own.as_ref().and_then(|own| own.forget_at),
            self.deadline,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn set_busy(&self) {
        let busy = self.active.is_some() || !self.tasks.is_empty();
        if self.busy.swap(busy, Ordering::Relaxed) != busy {
            if let Some(process) = &self.process { process.touch(); } else { self.shared.note_activity(); }
        }
    }

    async fn command(&mut self, command: Command) -> io::Result<()> {
        match command {
            Command::Start(input, sink, reply) => {
                let result = self.start(*input, sink).await;
                let _ = reply.send(result);
            }
            Command::Interrupt(turn, reply) => {
                let result = self.interrupt(&turn).await;
                let _ = reply.send(result);
            }
            Command::RestoreIdle(process, note, reply) => {
                self.restore_idle(process, note).await;
                let _ = reply.send(Ok(()));
            }
            Command::Handover(reply) => {
                // The next resident goes on with the turn: this one answers nothing more of it.
                if let Some(active) = self.active.take() {
                    lock(&self.handed).push(active.sink);
                }
                self.asks.abort_all();
                self.closing = true;
                let _ = reply.send(Ok(()));
            }
            Command::LetGo(note, reply) => {
                let result = self.shared.let_go(&note.session).await;
                let _ = reply.send(result);
            }
            Command::Close(signed_out, reply) => {
                self.closing = true;
                if self.active.is_some() {
                    self.cancel_native().await;
                    let outcome = if signed_out {
                        Outcome::Failed(failure(format!("{} stopped.", self.shared.name())))
                    } else {
                        Outcome::Interrupted
                    };
                    self.finish(outcome).await?;
                }
                if let (Some(process), Some(native)) = (self.live_process(), self.native.clone()) {
                    if process.capabilities().close {
                        let _ = process
                            .rpc
                            .request(
                                "session/close",
                                Some(json!({"sessionId": native})),
                                Some(STOP),
                            )
                            .await;
                    }
                    process.note_session(&native, None).await;
                }
                if let Some(relay) = &self.relay {
                    relay.drop_relayed(&self.session, self.holder);
                }
                self.background.set_tasks(&self.session, Vec::new());
                let _ = reply.send(Ok(()));
            }
        }
        Ok(())
    }

    fn live_process(&self) -> Option<Arc<Process>> {
        self.process.clone().filter(|process| !process.closed())
    }

    /// A conversation saved in plan mode goes on in the normal one: the plan setting stays saved,
    /// unread here.
    fn mode(input: &TurnInput) -> Mode {
        Mode {
            approve_all: input.permissions == PermissionMode::ApproveAll,
            read_only: false,
        }
    }

    /// The servers the session is given: Kuvo's tools, and the computer's MCP servers through the
    /// relay (for an agent that takes them over HTTP); and what tells them apart.
    async fn servers(&self, process: &Process) -> io::Result<(Vec<Value>, String)> {
        let Some(relay) = &self.relay else {
            return Ok((Vec::new(), String::new()));
        };
        let http = process.capabilities().http_servers;
        let own = self.shared.flavour.reads_own_servers();
        let headers = |list: Vec<(String, String)>| -> Vec<Value> {
            list.into_iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect()
        };
        let mut servers = Vec::new();
        let mut key = String::new();
        let tools = match own {
            true => relay.http_tools(&self.session, self.holder).await?,
            false => relay.agent_tools(&self.session, self.holder, false).await?,
        };
        if let Some(tools) = tools {
            match tools {
                AgentTools::Stdio {
                    key: tools,
                    command,
                    args,
                    env,
                    ..
                } => {
                    key = tools;
                    let env: Vec<Value> = env
                        .into_iter()
                        .map(|(name, value)| json!({"name": name.to_string_lossy(), "value": value.to_string_lossy()}))
                        .collect();
                    let args: Vec<String> = args
                        .iter()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect();
                    servers.push(json!({"name": crate::tool_relay::TOOL_SERVER_NAME, "command": command, "args": args, "env": env}));
                }
                AgentTools::Http {
                    key: tools,
                    url,
                    headers: list,
                    ..
                } if http => {
                    key = tools;
                    servers.push(json!({"type": "http", "name": crate::tool_relay::TOOL_SERVER_NAME, "url": url, "headers": headers(list)}));
                }
                AgentTools::Http { .. } => {}
            }
        }
        if http && !own {
            let found = relay.computer_servers(&self.directory).await;
            key.push('|');
            key.push_str(&relay.servers_key(&self.session, &found).await);
            for server in relay
                .relayed_servers(&self.session, self.holder, &self.directory, &found, true)
                .await?
            {
                servers.push(json!({"type": "http", "name": server.name, "url": server.url, "headers": [{"name": "Authorization", "value": server.authorization}]}));
            }
        }
        Ok((servers, key))
    }

    /// Its session open in a process its launch now shares: the one it's in, or (that one
    /// outdated, stale, or started another way) another, its conversation taken up there.
    /// Whether it started over (its conversation couldn't be taken up).
    async fn ensure_open(
        &mut self,
        model: &str,
        mode: &Mode,
        cancel: &CancellationToken,
    ) -> io::Result<bool> {
        let flavour = self.shared.flavour.clone();
        let key = {
            let (flavour, mode, directory, model) = (
                flavour.clone(),
                mode.clone(),
                self.directory.clone(),
                model.to_owned(),
            );
            tokio::task::spawn_blocking(move || flavour.launch_key(&mode, &directory, &model))
                .await
                .map_err(io::Error::other)??
        };
        let current = self
            .live_process()
            .filter(|process| process.key == key && !process.outdated() && !self.stale);
        let process = match current {
            Some(process) => process,
            None => {
                self.stale = false;
                let (flavour, mode, directory, model) = (
                    flavour.clone(),
                    mode.clone(),
                    self.directory.clone(),
                    model.to_owned(),
                );
                self.shared
                    .process(&key, move || flavour.launch(&mode, &directory, &model), cancel)
                    .await?
            }
        };
        let (servers, servers_key) = self.servers(&process).await?;
        if self.opened_with.as_ref() == Some(&(process.id, servers_key.clone())) {
            return Ok(false);
        }
        let started_over = match self.open_session(&process, servers.clone()).await {
            Err(error)
                if error.kind() == io::ErrorKind::PermissionDenied
                    && !flavour.authenticates_first() =>
            {
                self.shared.authenticate(&process).await?;
                self.open_session(&process, servers).await?
            }
            other => other?,
        };
        self.process = Some(process.clone());
        self.opened_with = Some((process.id, servers_key));
        let native = self.native.clone().unwrap_or_default();
        let mut note = json!({"session": self.session, "setup": self.setup.portable()});
        if !self.tasks.is_empty() {
            note["backgroundTasks"] = self.task_notes();
        }
        process.note_session(&native, Some(note)).await;
        flavour.note_setup(&self.setup, process.credential.as_deref());
        Ok(started_over)
    }

    /// Opens its session in `process`: takes up the one it had (resumed, or loaded with what it
    /// replays dropped), or a new one in its place.
    async fn open_session(
        &mut self,
        process: &Arc<Process>,
        servers: Vec<Value>,
    ) -> io::Result<bool> {
        let flavour = self.shared.flavour.clone();
        let capabilities = process.capabilities();
        let previous = self.native.clone();
        // (Its session's updates come from this process from now on, those said before it's known
        // too.)
        self.process = Some(process.clone());
        let cwd = self.directory.to_string_lossy().into_owned();
        let attempts: Vec<&str> = match &previous {
            Some(_) if flavour.resumes() && capabilities.resume => {
                vec!["session/resume", "session/load"]
            }
            Some(_) => vec!["session/load"],
            None => vec![],
        };
        for method in attempts {
            if method == "session/load" && !capabilities.load {
                continue;
            }
            process.opening();
            let params = json!({"sessionId": previous, "cwd": cwd, "mcpServers": servers});
            let answer = process.rpc.request(method, Some(params), Some(OPEN)).await;
            // (What a load replays is in the conversation already.)
            self.shared
                .opened(process, &self.session, previous.as_deref(), false);
            match answer {
                Ok(answer) => {
                    self.setup = Setup::read(&answer);
                    return Ok(false);
                }
                Err(Failure::Refused(error)) if error.code == -32000 && is_auth(&error) => {
                    return Err(auth_needed());
                }
                Err(Failure::Closed(_)) => {
                    return Err(self.shared.failed(process, Failure::Closed(String::new())));
                }
                Err(_) => {}
            }
        }
        process.opening();
        let answer = process
            .rpc
            .request(
                "session/new",
                Some(json!({"cwd": cwd, "mcpServers": servers})),
                Some(OPEN),
            )
            .await;
        let answer = match answer {
            Ok(answer) => answer,
            Err(Failure::Refused(error)) if error.code == -32000 => {
                self.shared.opened(process, &self.session, None, false);
                return Err(auth_needed());
            }
            Err(failed) => {
                self.shared.opened(process, &self.session, None, false);
                return Err(self.shared.failed(process, failed));
            }
        };
        let native = answer["sessionId"].as_str().map(str::to_owned);
        let early = self
            .shared
            .opened(process, &self.session, native.as_deref(), true);
        self.setup = Setup::read(&answer);
        let started_over = previous.is_some() && previous != native;
        self.native = native.clone();
        if started_over {
            // In place of one it had: its instructions go again.
            self.state.remove("acpInstructions");
            self.state.remove("acpInstructed");
        }
        self.state.insert("acpSession".into(), native.into());
        if let Some(active) = &self.active {
            let _ = active.sink.save_state(self.state.clone()).await;
        }
        for (update, _) in early {
            Box::pin(self.event(Event::Update {
                process: process.id,
                update,
            }))
            .await?;
        }
        Ok(started_over)
    }

    async fn start(&mut self, input: TurnInput, sink: Arc<dyn TurnSink>) -> io::Result<()> {
        match &input.kind {
            TurnKind::Recover { note } => {
                return self
                    .recover(input.id.clone(), note.clone(), &input, sink)
                    .await;
            }
            TurnKind::AgentStarted => return self.take_own(input, sink).await,
            TurnKind::Prompt(_) | TurnKind::Compact => {}
        }
        self.own = None;
        let mode = Self::mode(&input);
        let model = (self.shared.flavour).model_value(&input.model.provider, &input.model.id);
        if let Err(error) = self.ensure_open(&model, &mode, &input.start_cancel).await {
            if input.start_cancel.is_cancelled() {
                return Err(error);
            }
            return unsent(sink.as_ref(), error).await;
        }
        let process = self
            .process
            .clone()
            .ok_or_else(|| stopped(self.shared.name()))?;
        let native = self.native.clone().unwrap_or_default();
        let flavour = self.shared.flavour.clone();
        let credential = process.credential.clone();
        let _ = flavour
            .select_model(
                &process.rpc,
                &native,
                &model,
                input.model.variant.as_deref(),
                &mut self.setup,
                credential.as_deref(),
            )
            .await;
        if let Err(error) = flavour
            .select_mode(&process.rpc, &native, &mode, &mut self.setup)
            .await
        {
            return unsent(sink.as_ref(), error).await;
        }
        let compact = matches!(input.kind, TurnKind::Compact);
        let (content, instructed) = match &input.kind {
            TurnKind::Prompt(prompt) => self.prompt_content(prompt, &input.instructions).await,
            _ => (vec![json!({"type": "text", "text": "/compact"})], None),
        };
        sink.event(envelope(TurnEvent::StepStarted {
            model: Some(input.model.clone()),
        }))
        .await?;
        if compact {
            sink.event(envelope(TurnEvent::CompactionStarted)).await?;
        }
        process.touch();
        self.active = Some(Active {
            compact,
            instructed,
            ..Active::new(&input, sink, process, Projection::new(now(), compact))
        });
        self.set_busy();
        self.prompt(content).await
    }

    /// The prompt a message makes: its words (with the app's instructions when they're new or
    /// changed), its pictures, and the paths of its other files.
    async fn prompt_content(
        &mut self,
        prompt: &Prompt,
        instructions: &str,
    ) -> (Vec<Value>, Option<String>) {
        let mut content = Vec::new();
        let mut others = Vec::new();
        for file in &prompt.files {
            if let Some(data) = picture(file).await {
                content.push(json!({"type": "image", "data": data, "mimeType": file.mime}));
            } else if let Some(path) = &file.path {
                others.push(path.to_string_lossy().into_owned());
            }
        }
        let mut text = prompt.text.clone();
        let sent = match self.state.get("acpInstructions") {
            Some(Value::String(sent)) => Some(sent.clone()),
            _ if self.state.get("acpInstructed").is_some() => None,
            _ => Some(String::new()),
        };
        let mut instructed = None;
        if sent.as_deref() != Some(instructions)
            && (!instructions.is_empty() || sent.as_ref().is_some_and(|sent| !sent.is_empty()))
        {
            if !instructions.is_empty() {
                let changed = if sent.as_ref().is_some_and(|sent| !sent.is_empty()) {
                    "Your instructions from Kuvo Code changed. They're now:\n\n"
                } else {
                    ""
                };
                text = format!("{changed}{instructions}\n\n{text}");
            }
            instructed = Some(instructions.to_owned());
        }
        if !others.is_empty() {
            text.push_str("\n\nAttached files:\n");
            text.push_str(
                &others
                    .iter()
                    .map(|path| format!("- {path}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        content.insert(0, json!({"type": "text", "text": text}));
        (content, instructed)
    }

    /// Sends a prompt of the turn's: its answer ends the turn, unless a newer one went meanwhile.
    async fn prompt(&mut self, content: Vec<Value>) -> io::Result<()> {
        let flavour = self.shared.flavour.clone();
        let native = self.native.clone().unwrap_or_default();
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        active.prompt += 1;
        active.projection.said.clear();
        active.projection.said_length = 0;
        let meta = flavour.prompt_meta();
        active.prompt_id = meta
            .as_ref()
            .and_then(|meta| meta["promptId"].as_str())
            .map(str::to_owned);
        let mut params = json!({"sessionId": native, "prompt": content});
        if let Some(meta) = meta {
            params["_meta"] = meta;
        }
        if let Some(mark) = flavour.record_mark(&native) {
            active.record = Some(Record {
                mark,
                sent: now(),
                candidate: None,
                next: Instant::now() + RECORD_LOOK,
            });
        }
        let process = active.process.clone();
        let from = process
            .turn_note(&native)
            .and_then(|note| note["from"].as_u64())
            .filter(|_| active.prompt > 1)
            .unwrap_or_else(|| process.rpc.seen());
        let (number, compact) = (active.prompt, active.compact);
        active.request = None;
        match process.rpc.send("session/prompt", Some(params)).await {
            Ok(id) => {
                // (Its answer comes in order with what it says, as an event.)
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|active| active.prompt == number)
                {
                    active.request = Some(json!(id));
                }
                if !compact {
                    let note = json!({"session": self.session, "from": from, "request": id, "sentAt": now()});
                    process.note_turn(&native, Some(note)).await;
                }
                Ok(())
            }
            // (Gone before it could be sent: its stop may have come already.)
            Err(_) => {
                let words = format!("{} stopped.", self.shared.name());
                Box::pin(self.end(Outcome::Failed(failure(words)))).await
            }
        }
    }

    /// A turn an earlier resident left under way in this conversation's process, taken up as it
    /// is: what it said since comes again, and the answer to the prompt sent then ends it.
    async fn recover(
        &mut self,
        id: String,
        note: TurnNote,
        input: &TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> io::Result<()> {
        let native = self
            .native
            .clone()
            .ok_or_else(|| io::Error::other("This conversation has no session in its agent."))?;
        let process = self
            .shared
            .running()
            .await
            .into_iter()
            .find(|process| process.turn_note(&native).is_some())
            .ok_or_else(|| stopped(self.shared.name()))?;
        if let Some(saved) = process.session_note(&native) {
            self.setup = Setup::read(&saved["setup"]);
        }
        self.shared
            .opened(&process, &self.session, Some(&native), false);
        self.process = Some(process.clone());
        self.opened_with = None;
        sink.event(envelope(TurnEvent::StepStarted {
            model: Some(input.model.clone()),
        }))
        .await?;
        let request = note
            .request
            .as_ref()
            .map(|request| serde_json::to_value(request).unwrap_or(Value::Null));
        let projection = Projection::new(note.sent_at.unwrap_or_else(now), false);
        self.active = Some(Active {
            id,
            prompt: 1,
            request,
            ..Active::new(input, sink, process.clone(), projection)
        });
        if let (Some(active), Some(sent)) = (self.active.as_mut(), note.sent_at)
            && self.shared.flavour.record_mark(&native).is_some()
        {
            active.record = Some(Record {
                mark: -1,
                sent,
                candidate: None,
                next: Instant::now() + RECORD_LOOK,
            });
        }
        self.set_busy();
        process.ready(&native);
        Ok(())
    }

    /// A turn the agent began on its own, taken here with what it said so far.
    async fn take_own(&mut self, input: TurnInput, sink: Arc<dyn TurnSink>) -> io::Result<()> {
        let own = self.own.take();
        let process = self.live_process();
        let (Some(own), Some(process)) = (own.filter(|own| !own.turned_away), process) else {
            let done = TurnResult {
                outcome: Outcome::Succeeded,
                took_prompt: false,
                missed: vec![],
            };
            sink.event(envelope(TurnEvent::Finished(done))).await?;
            return Ok(());
        };
        sink.event(envelope(TurnEvent::StepStarted {
            model: Some(input.model.clone()),
        }))
        .await?;
        self.active = Some(Active {
            own: true,
            quiet: Some(Instant::now() + OWN_QUIET),
            ..Active::new(&input, sink, process, Projection::new(own.started, false))
        });
        self.set_busy();
        for update in own.updates {
            self.update(update).await?;
        }
        if own.ended {
            self.end(Outcome::Succeeded).await?;
        }
        Ok(())
    }

    async fn restore_idle(&mut self, process: Arc<Process>, note: Value) {
        let Some(native) = self.native.clone() else {
            return;
        };
        self.setup = Setup::read(&note["setup"]);
        self.shared
            .opened(&process, &self.session, Some(&native), false);
        self.process = Some(process.clone());
        self.tasks = note["backgroundTasks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|task| {
                let id = task["id"].as_str()?.to_owned();
                let started = task["started"].as_u64().unwrap_or_else(now);
                let order = serde_json::from_value(task["order"].clone())
                    .unwrap_or_else(|_| WorkOrder::next());
                let text = |key: &str| task[key].as_str().map(str::to_owned);
                Some((
                    Task {
                        id,
                        command: text("command"),
                        description: text("description"),
                        kind: text("kind"),
                        started: Some(started),
                        output_file: text("outputFile"),
                    },
                    started,
                    order,
                ))
            })
            .collect();
        self.publish_tasks();
        self.set_busy();
    }

    async fn event(&mut self, event: Event) -> io::Result<()> {
        match event {
            Event::Update { process, update } => {
                if self
                    .process
                    .as_ref()
                    .is_some_and(|current| current.id == process)
                {
                    self.update(update).await?;
                }
            }
            Event::Extension { extension, .. } => self.extension(extension).await?,
            Event::Request {
                process,
                id,
                method,
                params,
            } => self.request(process, id, method, params).await,
            Event::Answer {
                process,
                id,
                answer,
            } => {
                let ours = self.active.as_ref().is_some_and(|active| {
                    active.process.id == process && active.request.as_ref() == Some(&id)
                });
                if ours {
                    let answer = answer.map_err(|error| Failure::Refused(error));
                    self.prompt_answered(answer).await?;
                }
            }
            Event::Stopped { process, exit } => self.stopped(process, exit).await?,
        }
        Ok(())
    }

    /// An update of its session's, or a subagent's session.
    async fn update(&mut self, update: SessionUpdate) -> io::Result<()> {
        if update.replay {
            return Ok(());
        }
        // (What it offers changed: the next turn chooses from that.)
        match &update.update {
            crate::acp::wire::Update::ConfigOptions(options) => {
                self.setup.set("configOptions", options.clone())
            }
            crate::acp::wire::Update::Mode(mode) => self.setup.set("kuvoMode", mode.clone()),
            _ => {}
        }
        let ours = match (&update.session, &self.native) {
            (Some(session), Some(native)) => session == native,
            _ => true,
        };
        let Some(active) = self.active.as_mut() else {
            if ours && self.native.is_some() {
                self.own_update(update).await;
            }
            return Ok(());
        };
        if active.own {
            active.quiet = Some(Instant::now() + OWN_QUIET);
        }
        let events = if ours {
            let native = self.native.clone().unwrap_or_default();
            let flavour = self.shared.flavour.clone();
            let cx = Context {
                flavour: flavour.as_ref(),
                directory: &self.directory,
                model: &active.model,
                native: &native,
                known: &self.known,
            };
            active.projection.update(&update.update, &cx)
        } else {
            active
                .projection
                .child(update.session.as_deref().unwrap_or(""), &update.update)
        };
        self.emit(events).await
    }

    async fn emit(&mut self, events: Vec<TurnEvent>) -> io::Result<()> {
        let Some(sink) = self.active.as_ref().map(|active| active.sink.clone()) else {
            return Ok(());
        };
        for event in events {
            sink.event(envelope(event)).await?;
        }
        Ok(())
    }

    /// An update with no turn here: the start of one the agent began on its own asks the
    /// resident to take it.
    async fn own_update(&mut self, update: SessionUpdate) {
        if let Some(own) = &mut self.own {
            if own.turned_away {
                own.forget_at = Some(Instant::now() + OWN_QUIET);
            } else {
                own.updates.push(update);
            }
            return;
        }
        if !update.update.begins_work() {
            return;
        }
        self.own = Some(Own {
            updates: vec![update],
            turned_away: false,
            ended: false,
            started: now(),
            forget_at: None,
        });
        let (own_turns, session, provider) = (
            self.own_turns.clone(),
            self.session.clone(),
            self.shared.provider.clone(),
        );
        let text = format!("{} carried on by itself.", self.shared.name());
        self.asks.spawn(async move {
            Settled::Offered(own_turns.offer(&session, &provider, text).await)
        });
    }

    /// A turn it began on its own that isn't taken: it stops (nothing would show what it does).
    async fn turn_away(&mut self) {
        let Some(own) = self.own.as_mut().filter(|own| !own.turned_away) else {
            return;
        };
        own.turned_away = true;
        own.forget_at = Some(Instant::now() + OWN_QUIET);
        self.cancel_native().await;
    }

    async fn cancel_native(&self) {
        if let (Some(process), Some(native)) = (self.live_process(), &self.native) {
            process
                .rpc
                .notify("session/cancel", Some(json!({"sessionId": native})))
                .await;
        }
    }

    async fn extension(&mut self, extension: Extension) -> io::Result<()> {
        match extension {
            Extension::Background {
                session,
                tasks,
                replayed,
            } => {
                if replayed || session.is_some_and(|session| Some(&session) != self.native.as_ref())
                {
                    return Ok(());
                }
                let before = std::mem::take(&mut self.tasks);
                self.tasks = tasks
                    .into_iter()
                    .map(|task| {
                        let known = before.iter().find(|(old, _, _)| old.id == task.id);
                        let started = task
                            .started
                            .or(known.map(|(_, started, _)| *started))
                            .unwrap_or_else(now);
                        let order =
                            known.map_or_else(WorkOrder::next, |(_, _, order)| order.clone());
                        (task, started, order)
                    })
                    .collect();
                self.publish_tasks();
                self.set_busy();
                if let (Some(process), Some(native)) = (self.live_process(), self.native.clone()) {
                    let mut note = process
                        .session_note(&native)
                        .unwrap_or_else(|| json!({"session": self.session}));
                    note["backgroundTasks"] = self.task_notes();
                    process.note_session(&native, Some(note)).await;
                }
            }
            Extension::PromptComplete { own: true, .. } => {
                if self.active.as_ref().is_some_and(|active| active.own) {
                    self.end(Outcome::Succeeded).await?;
                } else if let Some(own) = &mut self.own {
                    if own.turned_away {
                        self.own = None;
                    } else {
                        own.ended = true;
                    }
                }
            }
            Extension::PromptComplete {
                own: false,
                stop,
                error,
                prompt,
            } => {
                let Some(active) = &self.active else {
                    return Ok(());
                };
                if active.own
                    || (prompt.is_some()
                        && active.prompt_id.is_some()
                        && prompt != active.prompt_id)
                {
                    return Ok(());
                }
                let name = self.shared.name();
                let outcome = match (error, stop.as_str()) {
                    (Some(error), "rate_limit") => self.limited(Some(error)),
                    (Some(error), _) => Outcome::Failed(failure(error)),
                    (None, "rate_limit") => self.limited(Some(format!(
                        "You've reached your {name} usage limit for now."
                    ))),
                    (None, "cancelled") => Outcome::Interrupted,
                    (None, _) => Outcome::Succeeded,
                };
                self.end(outcome).await?;
            }
        }
        Ok(())
    }

    fn task_notes(&self) -> Value {
        Value::Array(
            self.tasks
                .iter()
                .map(|(task, started, order)| {
                    json!({"id": task.id, "command": task.command, "description": task.description, "kind": task.kind, "started": started, "order": order, "outputFile": task.output_file})
                })
                .collect(),
        )
    }

    fn publish_tasks(&self) {
        let helpers = self
            .tasks
            .iter()
            .map(|(task, started, order)| Helper {
                id: task.id.clone(),
                since: *started,
                order: Some(order.clone()),
            })
            .collect();
        self.background.set_tasks(&self.session, helpers);
    }

    /// The agent's request, answered off this task so what it says meanwhile still comes.
    async fn request(&mut self, process: Arc<Process>, id: Value, method: String, params: Value) {
        let flavour = self.shared.flavour.clone();
        let name = self.shared.name();
        let sink = self.active.as_ref().map(|active| active.sink.clone());
        if let Some(own) = flavour.own_request(&method, &params) {
            let flavour = flavour.clone();
            self.asks.spawn(async move {
                let answer = match (&own, sink) {
                    (
                        OwnRequest::Questions {
                            tool_call,
                            questions,
                        },
                        Some(sink),
                    ) => {
                        let ask = QuestionAsk {
                            id: tool_call
                                .clone()
                                .unwrap_or_else(|| format!("acp-question-{id}")),
                            title: "Question".into(),
                            fields: questions.iter().map(question_field).collect(),
                            tool_call_id: tool_call.clone(),
                            metadata: Map::new(),
                        };
                        match sink.question(ask).await {
                            Ok(Answer::Submitted(values)) => OwnAnswer::Answered(values),
                            Ok(Answer::Dismissed { reason }) => OwnAnswer::Declined(reason),
                            Err(_) => OwnAnswer::Declined(None),
                        }
                    }
                    (OwnRequest::ExitPlan { plan }, Some(sink)) => {
                        if let Some(plan) = plan {
                            let _ = sink
                                .event(envelope(TurnEvent::TextDelta {
                                    kind: crate::session::TextKind::Text,
                                    block: None,
                                    delta: plan.clone(),
                                }))
                                .await;
                        }
                        OwnAnswer::Declined(None)
                    }
                    _ => OwnAnswer::Declined(None),
                };
                Settled::plain(process, id, flavour.answer_own(&own, answer))
            });
            return;
        }
        match method.as_str() {
            "session/request_permission" => self.permission(process, id, params),
            "fs/read_text_file" | "fs/write_text_file" if flavour.file_system() => {
                self.file(process, id, method, params)
            }
            "elicitation/create" => {
                self.asks.spawn(async move {
                    let answer = match sink {
                        Some(sink) => crate::codex::elicitation_form(
                            sink.as_ref(),
                            format!("acp-form-{}", now()),
                            name,
                            &params,
                        )
                        .await
                        .unwrap_or_else(|_| json!({"action": "decline"})),
                        None => json!({"action": "decline"}),
                    };
                    Settled::plain(process, id, answer)
                });
            }
            _ => {
                if let Some(active) = &self.active
                    && !method.starts_with("terminal/")
                {
                    let words =
                        format!("{name} asked Kuvo Code for something it can't do yet ({method}).");
                    let _ = active.sink.event(envelope(TurnEvent::Notice(words))).await;
                }
                process
                    .rpc
                    .respond(id, Err(RpcError::not_found(&method)))
                    .await;
            }
        }
    }

    /// A permission ask: shown on its step, your answer told back as the option it offered
    /// (never its "always", which Kuvo keeps). Antigravity's questions come this way too.
    fn permission(&mut self, process: Arc<Process>, id: Value, params: Value) {
        let options: Vec<(String, String, String)> = params["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| {
                let id = option["optionId"].as_str()?.to_owned();
                let name = option["name"].as_str().unwrap_or(&id).to_owned();
                Some((id, option["kind"].as_str().unwrap_or("").to_owned(), name))
            })
            .collect();
        let Some(active) = &self.active else {
            self.asks
                .spawn(async move { Settled::plain(process, id, cancelled()) });
            return;
        };
        let sink = active.sink.clone();
        let mut call: Call = params["toolCall"]["toolCallId"]
            .as_str()
            .and_then(|call| active.projection.call(call))
            .cloned()
            .unwrap_or_default();
        tools::merge(
            &mut call,
            params["toolCall"].as_object().unwrap_or(&Map::new()),
        );
        let call_id = tools::id(&call).to_owned();
        if tools::is_question(&call) {
            let ask = QuestionAsk {
                id: call_id.clone(),
                title: "Question".into(),
                fields: vec![Field {
                    key: "answer".into(),
                    title: Some("Question".into()),
                    description: Some(tools::title(&call).to_owned()),
                    required: true,
                    kind: FieldKind::String {
                        options: options
                            .iter()
                            .map(|(value, _, label)| FormOption {
                                value: value.clone(),
                                label: label.clone(),
                                description: None,
                            })
                            .collect(),
                        custom: false,
                        format: None,
                    },
                }],
                tool_call_id: Some(call_id.clone()),
                metadata: Map::new(),
            };
            self.asks.spawn(async move {
                let (answer, declined) = match sink.question(ask).await {
                    Ok(Answer::Submitted(values)) => {
                        match values.get("answer").and_then(Value::as_str) {
                            Some(chosen) => (
                                json!({"outcome": {"outcome": "selected", "optionId": chosen}}),
                                None,
                            ),
                            None => (cancelled(), None),
                        }
                    }
                    // (It can only hear cancelled: its step says what became of it.)
                    Ok(Answer::Dismissed { reason }) => {
                        (cancelled(), reason.map(|words| (call_id, words)))
                    }
                    Err(_) => (cancelled(), None),
                };
                Settled::Asked {
                    process,
                    id,
                    answer: Ok(answer),
                    after: After {
                        declined,
                        ..After::default()
                    },
                    cancellable: true,
                }
            });
            return;
        }
        let (name, _) = tools::kuvo_tool(&call, &self.known);
        let action = tools::permission_action(&call, &name);
        let resources = tools::resources(&call);
        let mut metadata = Map::new();
        let files: Vec<Value> = call
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "diff" && part["path"].is_string())
            .map(|part| {
                crate::file_change::file_change(
                    part["path"].as_str().unwrap_or(""),
                    part["oldText"].as_str(),
                    Some(part["newText"].as_str().unwrap_or("")),
                )
            })
            .collect();
        if !files.is_empty() {
            metadata.insert("files".into(), files.into());
        }
        let ask = PermissionAsk {
            id: call_id.clone(),
            tool_call_id: Some(call_id.clone()),
            unclassified: !tools::classified(&action),
            action: action.clone(),
            resources: resources.clone(),
            save: Vec::new(),
            message: Some(tools::title(&call).to_owned()).filter(|title| !title.is_empty()),
            metadata,
            sandboxed: None,
            already_weighed: false,
        };
        self.asks.spawn(async move {
            // Refused by your settings, not by you: told why, it carries on.
            let automatic = matches!(
                sink.preflight(ask.clone(), false).await,
                Ok(Preflight::Deny)
            );
            let decision = sink.permission(ask).await;
            let pick = |kinds: &[&str]| {
                kinds
                    .iter()
                    .find_map(|kind| options.iter().find(|(_, of, _)| of == kind))
                    .map(|(id, _, _)| id.clone())
            };
            let (option, after) = match decision {
                Ok(Decision::Reject) => {
                    let refused = automatic.then(|| tools::refused_what(&action, &resources));
                    (
                        pick(&["reject_once", "reject_always"]),
                        After {
                            declined: Some((call_id, "Declined.".into())),
                            refused,
                            ..After::default()
                        },
                    )
                }
                Ok(_) => (pick(&["allow_once", "allow_always"]), After::default()),
                Err(_) => (None, After::default()),
            };
            let answer = option.map_or_else(
                cancelled,
                |option| json!({"outcome": {"outcome": "selected", "optionId": option}}),
            );
            Settled::Asked {
                process,
                id,
                answer: Ok(answer),
                after,
                cancellable: true,
            }
        });
    }

    /// A file the agent reads or writes through Kuvo: as your rules say, asking when they ask.
    fn file(&mut self, process: Arc<Process>, id: Value, method: String, params: Value) {
        let path = self.directory.join(params["path"].as_str().unwrap_or(""));
        let sink = self.active.as_ref().map(|active| active.sink.clone());
        self.asks.spawn(async move {
            let not_allowed = || RpcError::new(-32000, "Kuvo Code's settings don't allow this (Settings › Permissions, or this session's rules).");
            let resources = vec![path.to_string_lossy().into_owned()];
            let ask = |action: &str| PermissionAsk {
                id: format!("acp-file-{id}"),
                tool_call_id: None,
                action: action.into(),
                resources: resources.clone(),
                save: Vec::new(),
                message: None,
                metadata: Map::new(),
                sandboxed: None,
                already_weighed: false,
                unclassified: false,
            };
            let mut after = After::default();
            let answer = if method == "fs/write_text_file" {
                match &sink {
                    Some(sink) if matches!(sink.preflight(ask("edit"), false).await, Ok(Preflight::Deny)) => Err(not_allowed()),
                    _ => write_file(&path, params["content"].as_str().unwrap_or("")).await.map(|()| json!({})).map_err(|error| RpcError::new(-32000, error.to_string())),
                }
            } else {
                let allowed = match &sink {
                    // (Between turns: nothing to ask in.)
                    None => Ok(()),
                    Some(sink) => match sink.preflight(ask("read"), false).await {
                        Ok(Preflight::Deny) => Err(not_allowed()),
                        Ok(Preflight::Ask) => match sink.permission(ask("read")).await {
                            Ok(Decision::Reject) | Err(_) => Err(RpcError::new(-32000, "Declined.")),
                            Ok(_) => Ok(()),
                        },
                        _ => Ok(()),
                    },
                };
                match allowed {
                    Err(error) => Err(error),
                    Ok(()) => match tokio::fs::read_to_string(&path).await {
                        Ok(text) => {
                            let text = lines(&text, params["line"].as_u64(), params["limit"].as_u64());
                            after.served = Some((path.clone(), Served::Text(text.clone())));
                            Ok(json!({"content": text}))
                        }
                        Err(error) => {
                            let words = if error.kind() == io::ErrorKind::NotFound { "There's no such file.".to_owned() } else { error.to_string() };
                            after.served = Some((path.clone(), Served::Error(words.clone())));
                            Err(RpcError::new(-32002, words))
                        }
                    },
                }
            };
            Settled::Asked { process, id, answer, after, cancellable: false }
        });
    }

    async fn settled(&mut self, settled: Settled) -> io::Result<()> {
        match settled {
            Settled::Asked {
                process,
                id,
                answer,
                after,
                cancellable,
            } => {
                let stopping = self.active.as_ref().is_some_and(|active| active.stopping);
                // Stopped meanwhile: it hears its ask was cancelled, not that you said no.
                let answer = if cancellable && stopping {
                    Ok(cancelled())
                } else {
                    answer
                };
                process.rpc.respond(id, answer).await;
                let native = self.native.clone().unwrap_or_default();
                let flavour = self.shared.flavour.clone();
                let mut events = Vec::new();
                if let Some(active) = self.active.as_mut() {
                    if let Some((call, words)) = after.declined {
                        active.projection.declined.insert(call, words);
                    }
                    if let Some(refused) = after.refused {
                        active.refused = Some(refused);
                    } else if after.served.is_none() {
                        active.refused = None;
                    }
                    if let Some((path, served)) = after.served {
                        let cx = Context {
                            flavour: flavour.as_ref(),
                            directory: &self.directory,
                            model: &active.model,
                            native: &native,
                            known: &self.known,
                        };
                        events = active.projection.served(&path, served, &cx);
                    }
                }
                self.emit(events).await?;
            }
            Settled::Offered(taken) => {
                if !taken {
                    self.turn_away().await;
                }
            }
        }
        Ok(())
    }

    /// The answer to the turn's prompt: its end.
    async fn prompt_answered(&mut self, answer: Result<Value, Failure>) -> io::Result<()> {
        let name = self.shared.name();
        let outcome = match answer {
            Ok(result) => {
                let usage = self.shared.flavour.answers_usage().then(|| wire::prompt_usage(&result)).flatten();
                if let Some(active) = &mut self.active {
                    active.answered = true;
                    active.projection.turn = usage;
                }
                match result["stopReason"].as_str() {
                    Some("rate_limit") => self.limited(Some(format!(
                        "You've reached your {name} usage limit for now."
                    ))),
                    Some("cancelled") => Outcome::Interrupted,
                    _ => Outcome::Succeeded,
                }
            }
            Err(Failure::Refused(error)) => {
                let words = error
                    .data
                    .as_ref()
                    .and_then(|data| data["message"].as_str())
                    .unwrap_or(&error.message)
                    .to_owned();
                let lower = words.to_ascii_lowercase();
                if error.code == -32003 || LIMIT_WORDS.iter().any(|limit| lower.contains(limit)) {
                    self.limited(Some(words))
                } else {
                    Outcome::Failed(failure(words))
                }
            }
            Err(Failure::TookTooLong(what)) => {
                Outcome::Failed(failure(format!("{what} took too long.")))
            }
            // (Gone: how it ended comes as it stops.)
            Err(Failure::Closed(_)) => return Ok(()),
        };
        self.end(outcome).await
    }

    /// A usage limit: the account's, or (Antigravity) the model's.
    fn limited(&self, words: Option<String>) -> Outcome {
        let name = self.shared.name();
        let scope = self.shared.flavour.limit_scope();
        let message = match scope {
            crate::session::LimitScope::Model => {
                format!("You've reached your {name} usage limit for this model.")
            }
            crate::session::LimitScope::Provider => {
                words.unwrap_or_else(|| format!("You've reached your {name} usage limit for now."))
            }
        };
        Outcome::Failed(TurnError {
            kind: "limit".into(),
            message,
            status: None,
            limit: Some(Limit {
                scope,
                resets_at: None,
            }),
        })
    }

    /// Its process stopped: a turn on it fails (or, its access ended partway, carries on in a new
    /// one); its session is opened again for the next.
    async fn stopped(&mut self, process: u64, exit: Exit) -> io::Result<()> {
        if self
            .process
            .as_ref()
            .is_some_and(|current| current.id == process)
        {
            self.process = None;
            self.opened_with = None;
            if !self.tasks.is_empty() && self.active.is_none() {
                eprintln!(
                    "{} stopped between turns with work going in {}.",
                    self.shared.name(),
                    self.session
                );
            }
            self.tasks.clear();
            self.publish_tasks();
            self.set_busy();
        }
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        if active.process.id != process {
            return Ok(());
        }
        if exit.asks_sign_in && !exit.young && !active.renewed {
            active.renewed = true;
            active.replaced = Some(process);
            return self.renew().await;
        }
        let said = if exit.said.trim().is_empty() {
            format!("{} stopped.", self.shared.name())
        } else {
            exit.said
        };
        let outcome = if exit.by_itself {
            Outcome::AgentExited(Some(failure(said)))
        } else {
            Outcome::Failed(failure(said))
        };
        self.end(outcome).await
    }

    /// Its access ran out partway and it asked for a new sign-in: started again, it renews it from
    /// the sign-in it keeps, takes up its conversation, and carries on.
    async fn renew(&mut self) -> io::Result<()> {
        let Some(active) = &self.active else {
            return Ok(());
        };
        let (model, variant, mode, compact, cancel) = (
            active.model.clone(),
            active.variant.clone(),
            active.mode.clone(),
            active.compact,
            active.start_cancel.clone(),
        );
        let name = self.shared.name();
        match self.ensure_open(&model, &mode, &cancel).await {
            Err(_) if cancel.is_cancelled() => return self.end(Outcome::Interrupted).await,
            Err(error) => return self.end(Outcome::Failed(failure(error.to_string()))).await,
            Ok(true) => return self.end(Outcome::Failed(failure(format!("{name} started again and couldn't take up this conversation. Send your message again.")))).await,
            Ok(false) => {}
        }
        let Some(process) = self.process.clone() else {
            return Ok(());
        };
        let native = self.native.clone().unwrap_or_default();
        let flavour = self.shared.flavour.clone();
        let _ = flavour
            .select_model(
                &process.rpc,
                &native,
                &model,
                variant.as_deref(),
                &mut self.setup,
                process.credential.as_deref(),
            )
            .await;
        if let Err(error) = flavour
            .select_mode(&process.rpc, &native, &mode, &mut self.setup)
            .await
        {
            return self.end(Outcome::Failed(failure(error.to_string()))).await;
        }
        if let Some(active) = self.active.as_mut() {
            active.process = process;
        }
        let text = if compact {
            "/compact"
        } else {
            "<system-reminder>Your connection was renewed partway through your work, and what was under way then was cut off. Carry on with the task from where you were.</system-reminder>"
        };
        self.prompt(vec![json!({"type": "text", "text": text})])
            .await
    }

    /// Stop: it's told to cancel; a turn that hasn't ended a while later ends as stopped.
    async fn interrupt(&mut self, turn: &str) -> io::Result<()> {
        let restarts = self.shared.flavour.restarts_after_stop();
        let Some(active) = self.active.as_mut().filter(|active| active.id == turn) else {
            return Ok(());
        };
        active.stopping = true;
        active.quiet = Some(Instant::now() + STOP);
        if restarts {
            // It can't go on after a stop: the next message starts another, and this one stops
            // once nothing else uses it.
            self.stale = true;
            active.process.retire();
        }
        self.cancel_native().await;
        Ok(())
    }

    async fn expired(&mut self) -> io::Result<()> {
        let now = Instant::now();
        if self
            .own
            .as_ref()
            .and_then(|own| own.forget_at)
            .is_some_and(|at| at <= now)
        {
            if self.own.as_ref().is_some_and(|own| own.turned_away) {
                self.own = None;
            } else if let Some(own) = &mut self.own {
                own.forget_at = None;
            }
        }
        if self
            .active
            .as_ref()
            .and_then(|active| active.record.as_ref())
            .is_some_and(|record| record.next <= now)
        {
            self.look_at_record().await?;
        }
        let Some(active) = &self.active else {
            self.deadline = None;
            return Ok(());
        };
        if active.quiet.is_some_and(|at| at <= now) {
            if active.stopping {
                // Still not over: stopped here, and its process (if nothing else is on it) too.
                self.stale = true;
                active.process.retire();
                self.end(Outcome::Interrupted).await?;
                self.shared.stop_quiet(false).await;
            } else {
                // An own turn gone quiet: over.
                self.end(Outcome::Succeeded).await?;
            }
        }
        Ok(())
    }

    /// The agent's own record says this prompt is answered, twice running, though ACP hasn't (its
    /// answer can go missing): the turn is over, and the next starts in another process.
    async fn look_at_record(&mut self) -> io::Result<()> {
        let flavour = self.shared.flavour.clone();
        let native = self.native.clone().unwrap_or_default();
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        let Some(record) = active.record.as_mut() else {
            return Ok(());
        };
        record.next = Instant::now() + RECORD_LOOK;
        if active.stopping {
            return Ok(());
        }
        let said = active.projection.said.clone();
        let candidate = flavour.record_answer(
            &native,
            record.mark,
            record.sent,
            &said,
            active.projection.said_length,
        );
        if candidate.is_none() || candidate != record.candidate {
            record.candidate = candidate;
            return Ok(());
        }
        active.answered = true;
        active.process.retire();
        self.stale = true;
        self.end(Outcome::Succeeded).await
    }

    /// It stopped because Kuvo refused a tool by your settings (it takes any no as yours): told
    /// why, it carries on. Not after a no of yours, Stop, or a few times this turn.
    async fn carry_on(&mut self) -> io::Result<bool> {
        let Some(active) = self.active.as_mut() else {
            return Ok(false);
        };
        let Some(refused) = active.refused.take() else {
            return Ok(false);
        };
        if active.own || active.stopping || active.carried_on >= CARRY_ON {
            return Ok(false);
        }
        active.carried_on += 1;
        let text = format!(
            "<system-reminder>Kuvo Code didn't allow {refused}: the user's settings (Settings › Permissions, the session's agent, or Plan mode) don't allow it here. The user didn't stop you. Carry on with the task without it, or say what you'd need.</system-reminder>"
        );
        Box::pin(self.prompt(vec![json!({"type": "text", "text": text})])).await?;
        Ok(true)
    }

    /// The turn ends as `outcome`.
    async fn end(&mut self, mut outcome: Outcome) -> io::Result<()> {
        let flavour = self.shared.flavour.clone();
        if let Some(active) = &self.active
            && matches!(outcome, Outcome::Succeeded)
            && !active.own
            && !active.projection.has_tools()
            && !active.projection.said.is_empty()
            && flavour.limit_reply(&active.projection.said)
        {
            // Done, having run nothing, with an answer that only says its plan is at its limit.
            outcome = self.limited(Some(active.projection.said.clone()));
        }
        if matches!(outcome, Outcome::Interrupted) && self.carry_on().await? {
            return Ok(());
        }
        let Some(mut active) = self.active.take() else {
            return Ok(());
        };
        let native = self.native.clone().unwrap_or_default();
        if !active.process.closed() {
            active.process.note_turn(&native, None).await;
            active.process.touch();
        }
        // The instructions went with a message it took.
        let failed = matches!(outcome, Outcome::Failed(_) | Outcome::AgentExited(_));
        if let Some(instructed) = active
            .instructed
            .take()
            .filter(|_| !failed || active.projection.heard)
        {
            self.state
                .insert("acpInstructions".into(), instructed.into());
            self.state.remove("acpInstructed");
            active.sink.save_state(self.state.clone()).await?;
        }
        let cx = Context {
            flavour: flavour.as_ref(),
            directory: &self.directory,
            model: &active.model,
            native: &native,
            known: &self.known,
        };
        let mut events = active.projection.end(&cx);
        if let Some(title) = self.own_title(&active.process).await {
            events.push(TurnEvent::Titled(title));
        }
        if active.compact {
            events.push(TurnEvent::CompactionEnded {
                succeeded: matches!(outcome, Outcome::Succeeded),
            });
        }
        let outcome = if active.stopping && !matches!(outcome, Outcome::Failed(_)) {
            Outcome::Interrupted
        } else {
            outcome
        };
        let took_prompt = active.projection.heard || active.answered;
        events.push(TurnEvent::Finished(TurnResult {
            outcome,
            took_prompt,
            missed: vec![],
        }));
        for event in events {
            active.sink.event(envelope(event)).await?;
        }
        self.set_busy();
        Ok(())
    }

    /// The title its agent gave it, as a turn ends, for one that titles itself: taken once.
    async fn own_title(&mut self, process: &Process) -> Option<String> {
        if self.titled || !self.shared.flavour.titles_itself() || process.closed() {
            return None;
        }
        let native = self.native.clone()?;
        let params = json!({"cwd": self.directory});
        let answer = process.rpc.request("session/list", Some(params), Some(STOP)).await.ok()?;
        let title = wire::listed_title(&answer, &native)?;
        self.titled = true;
        Some(title)
    }

    /// Stops what the agent does for this conversation now.
    async fn finish(&mut self, outcome: Outcome) -> io::Result<()> {
        if let Some(active) = &mut self.active {
            active.refused = None;
        }
        self.end(outcome).await
    }
}

fn cancelled() -> Value {
    json!({"outcome": {"outcome": "cancelled"}})
}

fn is_auth(error: &RpcError) -> bool {
    let words = error.message.to_ascii_lowercase();
    words.contains("auth") || words.contains("sign in") || words.contains("login")
}

fn auth_needed() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "Authentication required")
}

/// A picture of the message's, as base64.
async fn picture(file: &PromptFile) -> Option<String> {
    if !file.mime.starts_with("image/") {
        return None;
    }
    if let Some(data) = &file.data {
        return Some(data.clone());
    }
    let bytes = tokio::fs::read(file.path.as_ref()?).await.ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The lines asked for: from `line` (counted from 1), at most `limit`.
fn lines(text: &str, line: Option<u64>, limit: Option<u64>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_owned();
    }
    let start = line.unwrap_or(1).saturating_sub(1) as usize;
    let taken = text.split('\n').skip(start);
    match limit {
        Some(limit) => taken.take(limit as usize).collect::<Vec<_>>().join("\n"),
        None => taken.collect::<Vec<_>>().join("\n"),
    }
}

async fn write_file(path: &Path, content: &str) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        tokio::fs::create_dir_all(folder).await?;
    }
    tokio::fs::write(path, content).await
}

/// A question of the agent's own, as a card's field.
fn question_field(question: &super::flavour::Question) -> Field {
    let options = question
        .options
        .iter()
        .map(|(label, description)| FormOption {
            value: label.clone(),
            label: label.clone(),
            description: description.clone(),
        })
        .collect();
    Field {
        key: question.id.clone(),
        title: Some(question.header.clone().unwrap_or_else(|| "Question".into())),
        description: Some(question.question.clone()),
        required: true,
        kind: if question.multiple {
            FieldKind::MultiSelect {
                options,
                custom: true,
            }
        } else {
            FieldKind::String {
                options,
                custom: true,
                format: None,
            }
        },
    }
}
