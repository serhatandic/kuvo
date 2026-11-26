//! A session actor owns the Claude process pipe and correlates control requests.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    future::pending,
    io,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use kuvo_os::platform::{Platform, env_value};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;

use crate::{
    hold::{
        Controller, ControllerInput, Directory, Exit, HoldKind, Limits, Metadata, Output, Start,
        TurnNote,
    },
    runtime::AdapterCall,
    session::{
        ActiveTurn, Delivery, EventEnvelope, ModelRef, Outcome, Position, Prompt, SessionContext,
        Steer, SteerReceipt, Thread, TurnError, TurnEvent, TurnInput, TurnKind, TurnResult,
        TurnSink, VendorState,
    },
    told::instructions_note,
};

use super::{
    diagnostic::Said,
    model::Catalog,
    owned_paths,
    permissions::Request,
    program::Program,
    projection::Projection,
    tasks::{self, Tasks},
    wire,
};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const MISSED_TIMEOUT: Duration = Duration::from_secs(5);

struct Pipe {
    input: ControllerInput,
    output: mpsc::Receiver<io::Result<Output>>,
    reader: JoinHandle<()>,
    sequence: u64,
    replay_through: Option<u64>,
    reconnect: Option<String>,
    can_reconnect: bool,
    needs_initialize: bool,
    exited: bool,
    compact_at: Option<u64>,
    said: Said,
    /// The sign-in it started under, as a fingerprint: what it hears of the plan's limits counts
    /// for that one alone. None when it wasn't known (or no one was signed in that way).
    sign_in: Option<String>,
    /// The update of Claude Code it started after (one taken up: none known).
    program: Option<u64>,
}

impl Pipe {
    fn new(controller: Controller, replay: bool) -> Self {
        let (send, output) = mpsc::channel(32);
        let mut reader = controller.output;
        let reader = tokio::spawn(async move {
            loop {
                let event = reader.next().await;
                let end = event.is_err();
                if send.send(event).await.is_err() || end {
                    return;
                }
            }
        });
        Self {
            input: controller.input,
            output,
            reader,
            sequence: controller.welcome.seq,
            replay_through: replay.then_some(controller.welcome.seq),
            reconnect: None,
            can_reconnect: controller.welcome.meta.get("claudeReconnect")
                == Some(&Value::Bool(true)),
            needs_initialize: false,
            exited: controller.welcome.exited.is_some(),
            compact_at: controller
                .welcome
                .meta
                .get("claudeCompactAt")
                .and_then(Value::as_u64),
            // (What it said before this resident was there, if it stopped that soon.)
            said: {
                let mut said = Said::default();
                said.add(&controller.welcome.stderr);
                said
            },
            sign_in: controller
                .welcome
                .meta
                .get("claudeSignIn")
                .and_then(Value::as_str)
                .map(str::to_owned),
            program: None,
        }
    }

    async fn next(&mut self) -> io::Result<Output> {
        self.output
            .recv()
            .await
            .unwrap_or_else(|| Err(io::Error::from(io::ErrorKind::UnexpectedEof)))
    }

    async fn send(&mut self, request: Value) -> io::Result<String> {
        let id = unique_id()?;
        wire::request(&mut self.input, &id, request).await?;
        Ok(id)
    }

    async fn call(&mut self, request: Value) -> io::Result<wire::ReplyBody> {
        let id = self.send(request).await?;
        timeout(CONTROL_TIMEOUT, async {
            loop {
                match self.next().await? {
                    Output::Line { sequence, line, .. } => {
                        self.sequence = sequence;
                        self.input.acknowledge(sequence).await?;
                        match serde_json::from_str(&line) {
                            Ok(wire::Frame::ControlResponse { response })
                                if response.request_id == id =>
                            {
                                return Ok(response.body);
                            }
                            Ok(wire::Frame::ControlRequest {
                                request_id,
                                request,
                            }) => {
                                let request = serde_json::from_value::<Request>(request)?;
                                let result = request.answer(None, request_id.clone(), false).await;
                                wire::reply(
                                    &mut self.input,
                                    &request_id,
                                    result.map_err(|_| "No turn is active."),
                                )
                                .await?;
                            }
                            _ => {}
                        }
                    }
                    Output::Stderr { text } => self.said.add(&text),
                    Output::Exited { exit } => {
                        self.exited = true;
                        return Err(io::Error::other(self.stopped(&exit)));
                    }
                    _ => return Err(io::Error::other("Claude Code stopped before answering.")),
                }
            }
        })
        .await?
    }

    /// How it stopped before answering, as Claude's SDK says it, with the last thing it said.
    fn stopped(&mut self, exit: &Exit) -> String {
        let how = match (exit.code, &exit.signal) {
            (Some(code), _) => format!("Claude Code process exited with code {code}"),
            (None, Some(signal)) => format!("Claude Code process terminated by signal {signal}"),
            (None, None) => "Claude Code stopped before answering.".to_owned(),
        };
        match self.said.last() {
            Some(said) => format!("{how}: {said}"),
            None => how,
        }
    }

    async fn stop(mut self) -> io::Result<()> {
        self.input.end().await?;
        let result = timeout(Duration::from_secs(3), async {
            while !self.exited && !matches!(self.next().await?, Output::Exited { .. }) {}
            Ok::<_, io::Error>(())
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            self.input.signal("SIGKILL").await?;
        }
        self.input.bye().await
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // The reader owns a socket half. Cancel it so detaching really disconnects.
        self.reader.abort();
    }
}

pub(super) struct Recovery {
    pipe: Pipe,
    note: Option<TurnNote>,
    instructions: Option<String>,
    tasks: Value,
}

impl Recovery {
    pub fn new(controller: Controller, note: Option<TurnNote>) -> Self {
        let tasks = controller
            .welcome
            .meta
            .get("claudeTasks")
            .cloned()
            .unwrap_or(Value::Null);
        let instructions = controller
            .welcome
            .meta
            .get("claudeInstructions")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut pipe = Pipe::new(controller, note.is_some());
        pipe.needs_initialize = true;
        Self {
            pipe,
            note,
            instructions,
            tasks,
        }
    }

    pub async fn stop(mut self) -> io::Result<()> {
        self.pipe.input.unpin().await?;
        self.pipe.stop().await
    }

    /// Whether it was left under way in `note`'s turn.
    pub fn holds(&self, note: &TurnNote) -> bool {
        self.note
            .as_ref()
            .is_some_and(|held| held.session == note.session && held.turn == note.turn)
    }
}

#[derive(Clone)]
pub(super) struct Conversation {
    commands: mpsc::Sender<Command>,
    /// Stop/prepare can withdraw a start even while its actor waits for memory.
    start_stop: watch::Sender<()>,
    session: String,
    directory: PathBuf,
    config: Option<PathBuf>,
    /// Where a turn handed to the next resident reports, kept open while this one is here: it
    /// isn't this resident's to end.
    _handed: Handed,
}

type Handed = Arc<std::sync::Mutex<Option<Arc<dyn TurnSink>>>>;

type Reply<T> = oneshot::Sender<io::Result<T>>;

/// Its tasks' control requests, through the conversation's actor while it's there.
struct Calls(mpsc::WeakSender<Command>);

impl tasks::Calls for Calls {
    fn call(&self, request: Value) -> futures_util::future::BoxFuture<'_, io::Result<Value>> {
        Box::pin(async move {
            let commands = self.0.upgrade().ok_or_else(stopped)?;
            let (reply, answer) = oneshot::channel();
            commands
                .send(Command::TaskCall(request, reply))
                .await
                .map_err(|_| stopped())?;
            timeout(CONTROL_TIMEOUT, answer)
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
                .map_err(|_| stopped())?
        })
    }
}

enum Command {
    Start(Box<TurnInput>, Arc<dyn TurnSink>, watch::Receiver<()>, Reply<()>),
    Steer(String, Prompt, Reply<Steer>),
    Interrupt(String, Reply<()>),
    Resolve(SteerReceipt, Reply<Option<bool>>),
    Prepare(Reply<()>),
    Stop(bool, Reply<()>),
    LetGo(TurnNote, Reply<()>),
    /// A control request about its tasks (their output, Stop): its answer's `response`.
    TaskCall(Value, Reply<Value>),
}

impl Conversation {
    pub fn spawn(
        program: Program,
        models: Catalog,
        provider: String,
        context: SessionContext,
        recovered: Option<Recovery>,
    ) -> Self {
        let (commands, receiver) = mpsc::channel(16);
        let handed = Handed::default();
        let conversation = Self {
            commands,
            start_stop: watch::channel(()).0,
            session: context.session_id.clone(),
            directory: context.directory.clone(),
            config: program.config_home(&context.directory),
            _handed: handed.clone(),
        };
        let tasks = Tasks::new(
            &context.session_id,
            &context.directory.to_string_lossy(),
            context.background.clone(),
            Arc::new(Calls(conversation.commands.downgrade())),
        );
        let (pipe, recovery, instructions) = recovered.map_or((None, None, None), |old| {
            tasks.restore(old.tasks);
            (Some(old.pipe), old.note, old.instructions)
        });
        tokio::spawn(
            Actor {
                program,
                models,
                provider,
                directory: context.directory,
                session: context.session_id,
                state: context.state,
                pipe,
                recovery,
                running: None,
                replay_sink: None,
                asks: HashMap::new(),
                answered: HashSet::new(),
                replies: JoinSet::new(),
                prepare: None,
                deadline: None,
                instructions,
                configured: None,
                closing: false,
                noticed: HashSet::new(),
                handed,
                tasks,
                pending: HashMap::new(),
            }
            .run(receiver),
        );
        conversation
    }

    async fn send<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> io::Result<T> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| stopped())?;
        answer.await.map_err(|_| stopped())?
    }

    pub async fn prepare(&self) -> io::Result<()> {
        self.start_stop.send_replace(());
        self.send(Command::Prepare).await
    }

    pub async fn stop(&self, shutdown: bool) -> io::Result<()> {
        self.start_stop.send_replace(());
        self.send(|reply| Command::Stop(shutdown, reply)).await
    }

    /// The turn it was left under way in won't go on here: that Claude Code goes, and the next
    /// message starts one that resumes the conversation.
    pub async fn let_go(&self, note: TurnNote) -> io::Result<()> {
        self.send(|reply| Command::LetGo(note, reply)).await
    }
}

impl Thread for Conversation {
    fn owns_path(&self, path: &Path) -> bool {
        self.config
            .as_deref()
            .is_some_and(|config| owned_paths::owns_path(config, &self.directory, path))
    }

    fn start(
        &self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
    ) -> AdapterCall<'_, Arc<dyn ActiveTurn>> {
        Box::pin(async move {
            let id = input.id.clone();
            let stop = self.start_stop.subscribe();
            self.send(|reply| Command::Start(Box::new(input), sink, stop, reply))
                .await?;
            Ok(Arc::new(Turn {
                id,
                conversation: self.clone(),
            }) as Arc<dyn ActiveTurn>)
        })
    }

    /// Claude Code keeps the message's own UUID in its transcript once it takes it.
    fn resolve_steer(&self, receipt: SteerReceipt) -> AdapterCall<'_, Option<bool>> {
        Box::pin(self.send(|reply| Command::Resolve(receipt, reply)))
    }
}

struct Turn {
    id: String,
    conversation: Conversation,
}

impl ActiveTurn for Turn {
    /// The UUID Claude Code acknowledges it by, derived from the message before it's sent.
    fn steer_receipt(&self, prompt: &Prompt) -> Option<SteerReceipt> {
        let session = &self.conversation.session;
        Some(SteerReceipt {
            thread: session.clone(),
            turn: self.id.clone(),
            message: prompt_id(session, prompt.id.clone()),
        })
    }

    fn steer(&self, prompt: Prompt) -> AdapterCall<'_, Steer> {
        Box::pin(
            self.conversation
                .send(|reply| Command::Steer(self.id.clone(), prompt, reply)),
        )
    }

    fn interrupt(&self) -> AdapterCall<'_, ()> {
        Box::pin(
            self.conversation
                .send(|reply| Command::Interrupt(self.id.clone(), reply)),
        )
    }
}

struct Actor {
    program: Program,
    /// The account's models, for how much each holds.
    models: Catalog,
    provider: String,
    directory: PathBuf,
    session: String,
    state: VendorState,
    pipe: Option<Pipe>,
    recovery: Option<TurnNote>,
    running: Option<Running>,
    replay_sink: Option<Arc<dyn TurnSink>>,
    asks: HashMap<String, Ask>,
    answered: HashSet<String>,
    replies: JoinSet<(String, io::Result<Value>)>,
    prepare: Option<Reply<()>>,
    deadline: Option<Instant>,
    instructions: Option<String>,
    configured: Option<(ModelRef, bool)>,
    closing: bool,
    /// What this Claude Code has said once already (a retry, a limit's warning).
    noticed: HashSet<String>,
    handed: Handed,
    /// What it keeps going besides its turn.
    tasks: Tasks,
    /// Its control requests about tasks, waiting on their answers.
    pending: HashMap<String, Reply<Value>>,
}

struct Ask {
    stable: String,
    task: tokio::task::AbortHandle,
    /// What Claude Code is told if the turn is stopped while it asks.
    stopped: Option<Value>,
}

struct Running {
    id: String,
    model: ModelRef,
    plan: bool,
    sink: Arc<dyn TurnSink>,
    projection: Projection,
    note: TurnNote,
    steered: Vec<Steered>,
    explicit_compaction: bool,
    took_prompt: bool,
    stopping: bool,
    /// The instructions said ahead of its message, kept as told once it took it.
    told: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Steered {
    native: String,
    kuvo: String,
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        if self.recovery.is_none()
            && self.tasks.keeps_working()
            && let Some(pipe) = &mut self.pipe
            && pipe.needs_initialize
            && let Err(error) =
                reinitialize(pipe, self.instructions.as_deref().unwrap_or_default()).await
        {
            eprintln!("Claude Code could not reconnect: {error}");
        }
        while !self.closing {
            let read = async {
                match &mut self.pipe {
                    Some(pipe) => pipe.next().await,
                    None => pending().await,
                }
            };
            let timer = async {
                match self.deadline {
                    Some(at) => timer_fires::sleep_until(Timer::AgentTurnDeadline, at).await,
                    None => pending().await,
                }
            };
            let output = tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
                frame = read, if self.recovery.is_none() => match frame {
                    Ok(frame) => self.output(frame).await,
                    Err(error) => Err(error),
                },
                Some(reply) = self.replies.join_next() => match reply {
                    Ok((id, body)) => self.answer(id, body).await,
                    Err(_) => Ok(()),
                },
                () = timer => self.expired().await,
            };
            if output.is_err() {
                let _ = self.ended().await;
            }
        }
    }

    async fn command(&mut self, command: Command) -> io::Result<()> {
        match command {
            Command::Start(input, sink, stop, reply) => {
                let result = self.start(*input, sink, stop).await;
                let _ = reply.send(result);
            }
            Command::Steer(turn, prompt, reply) => {
                let result = self.steer(&turn, prompt).await;
                let _ = reply.send(result);
            }
            Command::Interrupt(turn, reply) => {
                let result = self.interrupt(&turn).await;
                let _ = reply.send(result);
            }
            Command::Resolve(receipt, reply) => {
                let _ = reply.send(self.resolve(receipt).await);
            }
            Command::TaskCall(request, reply) => match &mut self.pipe {
                Some(pipe) => {
                    let id = unique_id()?;
                    match wire::request(&mut pipe.input, &id, request).await {
                        Ok(()) => {
                            self.pending.insert(id, reply);
                        }
                        Err(error) => {
                            let _ = reply.send(Err(error));
                        }
                    }
                }
                None => {
                    let _ = reply.send(Err(stopped()));
                }
            },
            Command::LetGo(note, reply) => {
                let held = self
                    .recovery
                    .as_ref()
                    .is_some_and(|saved| saved.session == note.session && saved.turn == note.turn);
                if held {
                    self.recovery = None;
                    self.replay_sink = None;
                    if let Some(mut pipe) = self.pipe.take() {
                        let _ = pipe.input.unpin().await;
                        let _ = pipe.stop().await;
                        self.tasks.gone();
                    }
                }
                let _ = reply.send(Ok(()));
            }
            Command::Prepare(reply) => {
                if self.tasks.keeps_working()
                    && let Some(pipe) = &mut self.pipe
                {
                    pipe.input
                        .note(Metadata::from_iter([(
                            "claudeTasks".into(),
                            self.tasks.snapshot(),
                        )]))
                        .await?;
                }
                if let Some(pipe) = &mut self.pipe
                    && let Some(turn) = &self.running
                {
                    if !pipe.can_reconnect {
                        let _ = reply.send(Err(io::Error::other(
                            "This Claude Code cannot reconnect to an open turn.",
                        )));
                    } else if let Err(error) = pipe.input.prepare(turn.note.from).await {
                        let _ = reply.send(Err(error));
                    } else {
                        self.prepare = Some(reply);
                    }
                } else {
                    if let Some(pipe) = self.pipe.take()
                        && !pipe.can_reconnect
                    {
                        let _ = pipe.stop().await;
                        self.tasks.gone();
                    }
                    self.closing = true;
                    let _ = reply.send(Ok(()));
                }
            }
            Command::Stop(shutdown, reply) => {
                let result = match self.pipe.take() {
                    Some(pipe) => pipe.stop().await,
                    None => Ok(()),
                };
                let _ = self.ended().await;
                self.closing = shutdown;
                let _ = reply.send(result);
            }
        }
        Ok(())
    }

    async fn start(
        &mut self,
        input: TurnInput,
        sink: Arc<dyn TurnSink>,
        mut stop: watch::Receiver<()>,
    ) -> io::Result<()> {
        if self.running.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Claude Code is already answering.",
            ));
        }
        let id = input.id;
        let model = input.model;
        let plan = input.plan;
        let explicit_compaction = match &input.kind {
            TurnKind::Recover { note } => note
                .state
                .as_ref()
                .is_some_and(|state| state["compact"] == true),
            TurnKind::Compact => true,
            _ => false,
        };
        let projection = Projection::new(explicit_compaction, self.program.uses_key().await);
        if let TurnKind::Recover { note } = input.kind {
            let saved = self.recovery.take().ok_or_else(stopped)?;
            if saved.session != note.session || saved.turn != note.turn {
                self.recovery = Some(saved);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Claude Code holder belongs to another turn.",
                ));
            }
            let steered = note
                .state
                .as_ref()
                .and_then(|state| state.get("steered"))
                .and_then(|steered| serde_json::from_value(steered.clone()).ok())
                .unwrap_or_default();
            let pipe = self.pipe.as_mut().ok_or_else(stopped)?;
            if pipe
                .replay_through
                .is_some_and(|through| through <= note.from)
            {
                pipe.replay_through = None;
                sink.event(envelope(None, Delivery::Replay, TurnEvent::ReplayCaughtUp))
                    .await?;
                pipe.input.unpin().await?;
            }
            let opened_for = self
                .instructions
                .get_or_insert_with(|| input.instructions.clone());
            self.configured = Some((model.clone(), plan));
            // No prompt and no model reset. Reattach the callbacks to the open turn.
            if !pipe.exited
                && let Err(error) = reinitialize(pipe, opened_for).await
            {
                self.recovery = Some(saved);
                return Err(error);
            }
            self.replay_sink = pipe.replay_through.map(|_| sink.clone());
            self.running = Some(Running {
                id,
                model,
                plan,
                sink,
                projection,
                note: saved,
                steered,
                explicit_compaction,
                took_prompt: false,
                stopping: false,
                told: None,
            });
            return Ok(());
        }

        let prompt = match input.kind {
            TurnKind::Prompt(prompt) => Some(prompt),
            TurnKind::Compact => None,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "This Claude Code turn requires a prompt.",
                ));
            }
        };
        if self.recovery.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Claude Code has a recovered turn.",
            ));
        }
        let state_before = self.state.clone();
        // Discover the account's models when its first real turn needs them, including context
        // limits for the compaction decision. The picker alone never starts this probe.
        tokio::select! {
            biased;
            () = start_cancelled(&input.start_cancel, &mut stop) => return Err(stopped()),
            _ = self.models.refresh(self.program.clone()) => {},
        }
        let compact_at = tokio::select! {
            biased;
            () = start_cancelled(&input.start_cancel, &mut stop) => return Err(stopped()),
            compact_at = self.compact_at(&model.id) => compact_at?,
        };
        let instructions_changed = self
            .instructions
            .as_ref()
            .is_some_and(|old| *old != input.instructions);
        // One started under another sign-in, or one taken up whose sign-in isn't known: what it
        // hears of the plan's limits would count for the wrong account (or none), so it starts
        // again, going on with its conversation.
        let sign_in = self.program.sign_in_mark().await;
        if self.pipe.as_ref().is_some_and(|pipe| {
            pipe.compact_at != compact_at
                || instructions_changed
                || pipe.sign_in != sign_in
                || pipe.program != Some(self.program.updates())
        }) && !self.tasks.keeps_working()
            && let Some(pipe) = self.pipe.take()
        {
            let _ = pipe.stop().await;
            self.tasks.gone();
        }
        if self.pipe.is_none() {
            self.pipe = Some(
                self.launch(&input.instructions, compact_at, plan, &input.start_cancel, &mut stop)
                    .await?,
            );
            self.instructions = Some(input.instructions.clone());
            self.configured = None;
        }
        let pipe = self.pipe.as_mut().expect("Claude process created above");
        if pipe.needs_initialize {
            let answer = pipe.call(initialize(&input.instructions)).await?;
            if !matches!(answer, wire::ReplyBody::Success { .. }) {
                return Err(io::Error::other("Claude Code didn't reconnect."));
            }
            pipe.needs_initialize = false;
        }
        if self
            .configured
            .as_ref()
            .is_none_or(|config| config.0 != model || config.1 != plan)
        {
            if let Err(error) = configure(pipe, &model, plan).await {
                if let Some(pipe) = self.pipe.take() {
                    let _ = pipe.stop().await;
                    self.tasks.gone();
                }
                return Err(error);
            }
            self.configured = Some((model.clone(), plan));
        }
        // A conversation it goes on with keeps the instructions it started with (until it's
        // summarized): what changed since is said ahead of the message, once. Not ahead of a
        // command (its /compact, a skill's), which goes as typed: the next message says it.
        let mut told = None;
        let message = match prompt {
            Some(mut prompt) => {
                if !prompt.text.trim_start().starts_with('/')
                    && let Some(said) = instructions_note(
                        self.state.get("claudeTold").and_then(Value::as_str),
                        &input.instructions,
                    )
                {
                    prompt.text = format!("{said}\n\n{}", prompt.text);
                    told = Some(input.instructions.clone());
                }
                message(&self.session, prompt, false).await?
            }
            None => user_frame(
                unique_id()?,
                vec![json!({ "type":"text", "text":"/compact" })],
                false,
            ),
        };
        if self.state != state_before {
            sink.save_state(self.state.clone()).await?;
        }
        let pipe = self.pipe.as_mut().expect("Claude process created above");
        let note = TurnNote {
            session: self.session.clone(),
            from: pipe.sequence,
            turn: Some(id.clone()),
            request: None,
            sent_at: None,
            state: explicit_compaction.then(|| json!({"compact": true})),
        };
        pipe.input.note(turn_note(Some(&note))?).await?;
        self.answered.clear();
        self.running = Some(Running {
            id,
            model,
            plan,
            sink,
            projection,
            note,
            steered: Vec::new(),
            explicit_compaction,
            took_prompt: false,
            stopping: false,
            told,
        });
        if let Err(error) = pipe.input.write(message.to_string()).await {
            self.running.take();
            self.pipe.take();
            return Err(error);
        }
        Ok(())
    }

    async fn compact_at(&mut self, model: &str) -> io::Result<Option<u64>> {
        let setting = self
            .program
            .home
            .settings()
            .join("conversation-compaction.json");
        let choice = tokio::task::spawn_blocking(move || {
            fs::read(setting)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok())
        })
        .await?;
        // How much the model holds: as this conversation's turns said, or as the account's list does.
        let heard = self
            .state
            .get("claudeWindow")
            .filter(|window| window["model"] == model)
            .and_then(|window| window["size"].as_u64());
        let window = match heard {
            Some(size) => Some(size),
            None => match self.models.clone().heard_window(model).await {
                Some(size) => Some(size),
                None => self.models.clone().window(model).await,
            },
        };
        Ok(match choice.as_deref() {
            Some("200000") => Some(200_000),
            Some("600000") => Some(600_000),
            Some("never") if window.is_some_and(|size| size >= 900_000) => None,
            _ => Some(400_000),
        })
    }

    async fn launch(
        &mut self,
        instructions: &str,
        compact_at: Option<u64>,
        plan: bool,
        cancel: &CancellationToken,
        stop: &mut watch::Receiver<()>,
    ) -> io::Result<Pipe> {
        let updates = self.program.updates();
        let program = self.program.clone();
        let session = self.session.clone();
        let directory = self.directory.clone();
        let provider = self.provider.clone();
        let native = self
            .state
            .get("claudeSession")
            .and_then(Value::as_str)
            .filter(|id| is_uuid(id))
            .map(str::to_owned);
        let resume =
            native.is_some() && self.state.get("claudeStarted") == Some(&Value::Bool(true));
        // A new conversation counts its cost from nothing, and has the instructions it's given now.
        if !resume {
            self.state.remove("claudeCost");
            self.state
                .insert("claudeTold".into(), Value::String(instructions.to_owned()));
        }
        let native = native.map_or_else(unique_id, Ok)?;
        let sign_in = self.program.sign_in_mark().await;
        let start = tokio::task::spawn_blocking(move || {
            turn_launch(
                program, session, directory, provider, native, resume, compact_at, plan,
            )
        })
        .await??;
        let controller = Directory::for_home(&self.program.home)
            .start_cancellable(&self.program.holder_binary, start, start_cancelled(cancel, stop))
            .await?;
        let mut pipe = Pipe::new(controller, false);
        pipe.sign_in = sign_in;
        pipe.program = Some(updates);
        match pipe.call(initialize(instructions)).await {
            Ok(wire::ReplyBody::Success {
                pending_permission_requests,
                ..
            }) => {
                pipe.can_reconnect = pending_permission_requests.is_some();
                pipe.input
                    .note(Metadata::from_iter([
                        ("claudeReconnect".into(), Value::Bool(pipe.can_reconnect)),
                        (
                            "claudeInstructions".into(),
                            Value::String(instructions.to_owned()),
                        ),
                        (
                            "claudeSignIn".into(),
                            pipe.sign_in.clone().map_or(Value::Null, Value::String),
                        ),
                    ]))
                    .await?;
                Ok(pipe)
            }
            other => {
                let _ = pipe.stop().await;
                other.and(Err(io::Error::other("Claude Code didn't initialize.")))
            }
        }
    }

    async fn steer(&mut self, turn: &str, prompt: Prompt) -> io::Result<Steer> {
        let Some(active) = &mut self.running else {
            return Ok(Steer::Declined);
        };
        if active.id != turn || active.stopping {
            return Ok(Steer::Declined);
        }
        let pipe = self.pipe.as_mut().ok_or_else(stopped)?;
        let id = prompt.id.clone();
        let frame = message(&self.session, prompt, true).await?;
        let native = frame["uuid"].as_str().expect("prompt UUID");
        if active.steered.iter().any(|sent| sent.native == native) {
            return Ok(Steer::Accepted);
        }
        active.steered.push(Steered {
            native: native.into(),
            kuvo: id.clone(),
        });
        active.note.state =
            Some(json!({ "steered":active.steered, "compact":active.explicit_compaction }));
        pipe.input.note(turn_note(Some(&active.note))?).await?;
        if pipe.input.write(frame.to_string()).await.is_err() {
            return Ok(Steer::InDoubt(SteerReceipt {
                thread: self.session.clone(),
                turn: turn.into(),
                message: native.into(),
            }));
        }
        Ok(Steer::Accepted)
    }

    /// Whether Claude Code took a steered message: unknown while a turn that was handed it
    /// hasn't said, then whether its transcript has it.
    async fn resolve(&mut self, receipt: SteerReceipt) -> io::Result<Option<bool>> {
        if receipt.thread != self.session {
            return Ok(None);
        }
        let handed =
            |steered: &[Steered]| steered.iter().any(|sent| sent.native == receipt.message);
        let recovered = self.recovery.as_ref().is_some_and(|note| {
            note.state
                .as_ref()
                .and_then(|state| state.get("steered"))
                .and_then(|steered| serde_json::from_value::<Vec<Steered>>(steered.clone()).ok())
                .is_some_and(|steered| handed(&steered))
        });
        if recovered
            || self
                .running
                .as_ref()
                .is_some_and(|turn| handed(&turn.steered))
        {
            return Ok(None);
        }
        let Some(native) = self
            .state
            .get("claudeSession")
            .and_then(Value::as_str)
            .filter(|id| is_uuid(id))
            .map(str::to_owned)
        else {
            return Ok(Some(false));
        };
        let Some(config) = self.program.config_home(&self.directory) else {
            return Ok(None);
        };
        let found =
            tokio::task::spawn_blocking(move || transcript_has(&config, &native, &receipt.message))
                .await?;
        Ok(found.ok())
    }

    async fn interrupt(&mut self, turn: &str) -> io::Result<()> {
        if let Some(active) = &mut self.running
            && active.id == turn
            && !active.stopping
        {
            active.stopping = true;
            self.deadline = Some(Instant::now() + MISSED_TIMEOUT);
            if let Some(pipe) = &mut self.pipe {
                pipe.send(json!({ "subtype":"interrupt" })).await?;
            }
            // What it asks goes, and it's told no.
            for (id, ask) in self.asks.drain().collect::<Vec<_>>() {
                ask.task.abort();
                self.answered.insert(id.clone());
                if let Some(turn) = &self.running {
                    turn.sink.cancel_ask(&ask.stable).await?;
                }
                if let (Some(body), Some(pipe)) = (ask.stopped, &mut self.pipe) {
                    wire::reply(&mut pipe.input, &id, Ok::<_, &str>(body)).await?;
                }
            }
        }
        Ok(())
    }

    async fn output(&mut self, output: Output) -> io::Result<()> {
        match output {
            Output::Line { sequence, line, .. } => self.line(sequence, &line).await,
            Output::Stderr { text } => {
                if let Some(pipe) = &mut self.pipe {
                    pipe.said.add(&text);
                }
                Ok(())
            }
            Output::Exited { .. } => {
                self.replay_caught_up().await?;
                if let Some(pipe) = &mut self.pipe {
                    let _ = pipe.input.bye().await;
                }
                self.ended().await
            }
            Output::Prepared { prepared } => {
                if let Some(reply) = self.prepare.take() {
                    if prepared {
                        self.pipe.take();
                        if let Some(turn) = self.running.take() {
                            *self
                                .handed
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()) = Some(turn.sink);
                        }
                        self.closing = true;
                        let _ = reply.send(Ok(()));
                    } else {
                        let _ = reply.send(Err(io::Error::other(
                            "Claude Code's turn no longer fits in the holder journal.",
                        )));
                    }
                }
                Ok(())
            }
            _ => self.ended().await,
        }
    }

    async fn line(&mut self, sequence: u64, line: &str) -> io::Result<()> {
        let pipe = self.pipe.as_mut().ok_or_else(stopped)?;
        pipe.sequence = pipe.sequence.max(sequence);
        let replay = pipe
            .replay_through
            .is_some_and(|through| sequence <= through);
        if pipe.replay_through.is_some() && !replay {
            pipe.replay_through = None;
            pipe.input.unpin().await?;
            self.replay_caught_up().await?;
        }
        match serde_json::from_str::<wire::Frame>(line) {
            Ok(wire::Frame::ControlRequest {
                request_id,
                request,
            }) => self.ask(request_id, request).await?,
            Ok(wire::Frame::ControlCancelRequest { request_id }) => {
                self.cancel(&request_id).await?
            }
            Ok(wire::Frame::ControlResponse { response }) => {
                if let Some(reply) = self.pending.remove(&response.request_id) {
                    let _ = reply.send(match response.body {
                        wire::ReplyBody::Success { response, .. } => Ok(response),
                        wire::ReplyBody::Error => {
                            Err(io::Error::other("Claude Code didn't do that."))
                        }
                    });
                } else if self.pipe.as_ref().and_then(|pipe| pipe.reconnect.as_ref())
                    == Some(&response.request_id)
                {
                    self.reconnected(response.body).await?;
                }
            }
            Ok(wire::Frame::Message) => {
                if let Some(pipe) = &mut self.pipe {
                    pipe.said.forget();
                }
                let message: Value = serde_json::from_str(line)?;
                if self.tasks.frame(&message)
                    && let Some(pipe) = &mut self.pipe
                {
                    pipe.input
                        .note(Metadata::from_iter([(
                            "claudeTasks".into(),
                            self.tasks.snapshot(),
                        )]))
                        .await?;
                }
                self.message(sequence, replay, &message).await?;
            }
            Err(_) => {}
        }
        if let Some(pipe) = &mut self.pipe {
            pipe.input.acknowledge(sequence).await?;
        }
        Ok(())
    }

    async fn replay_caught_up(&mut self) -> io::Result<()> {
        if let Some(sink) = self.replay_sink.take() {
            sink.event(envelope(None, Delivery::Replay, TurnEvent::ReplayCaughtUp))
                .await?;
        }
        Ok(())
    }

    async fn reconnected(&mut self, body: wire::ReplyBody) -> io::Result<()> {
        let pipe = self.pipe.as_mut().ok_or_else(stopped)?;
        pipe.reconnect.take();
        let wire::ReplyBody::Success {
            pending_permission_requests: Some(asks),
            ..
        } = body
        else {
            let pipe = self.pipe.take().expect("reconnecting process");
            let _ = pipe.stop().await;
            self.tasks.gone();
            return Err(io::Error::other(
                "Claude Code couldn't recover its open tool requests.",
            ));
        };
        pipe.can_reconnect = true;
        pipe.needs_initialize = false;
        for frame in asks {
            if let wire::Frame::ControlRequest {
                request_id,
                request,
            } = frame
            {
                self.ask(request_id, request).await?;
            }
        }
        Ok(())
    }

    async fn message(&mut self, sequence: u64, replay: bool, message: &Value) -> io::Result<()> {
        // Where the plan's limits stand counts for the sign-in this Claude Code started under, while
        // it's still the one signed in (one from before a sign-in changed counts for no one).
        let heard_under = if message["type"] == "rate_limit_event" && self.running.is_some() {
            let started = self.pipe.as_ref().and_then(|pipe| pipe.sign_in.clone());
            let now = self.program.sign_in_mark().await;
            Some((started == now).then_some(started))
        } else {
            None
        };
        let Some(turn) = &mut self.running else {
            return Ok(());
        };
        let delivery = if replay {
            Delivery::Replay
        } else {
            Delivery::Live
        };
        let prior_state = self.state.clone();
        let events =
            turn.projection
                .frame(message, &turn.model, &mut self.state, &mut self.noticed);
        // A new conversation (its first, its /clear) or one just summarized has the
        // instructions it was started with.
        let main = !message["parent_tool_use_id"].is_string();
        let renewed = match (message["type"].as_str(), message["subtype"].as_str()) {
            (Some("system"), Some("init")) => {
                prior_state.get("claudeStarted") != Some(&Value::Bool(true))
                    || prior_state.get("claudeSession") != message.get("session_id")
            }
            (Some("conversation_reset"), _) => true,
            (Some("system"), Some("compact_boundary")) => main,
            _ => false,
        };
        if renewed && let Some(opened) = &self.instructions {
            self.state
                .insert("claudeTold".into(), Value::String(opened.clone()));
        }
        if self.state != prior_state {
            turn.sink.save_state(self.state.clone()).await?;
        }
        let mut events = events;
        // How much the model holds, as Claude Code said: a turn's result says (and it's known for
        // every conversation from then on); a reading before that, as it said last.
        let model = turn
            .projection
            .context_model()
            .unwrap_or(&turn.model.id)
            .to_owned();
        for event in &mut events {
            match event {
                TurnEvent::Context {
                    window: Some(size), ..
                } if message["type"] == "result" => {
                    self.models.note(&model, *size).await;
                    self.models.note(&turn.model.id, *size).await;
                }
                TurnEvent::Context { window, .. } if window.is_none() => {
                    *window = self.models.heard_window(&model).await;
                }
                _ => {}
            }
        }
        events.retain_mut(|event| match (event, &heard_under) {
            (TurnEvent::PlanLimit(reading), Some(Some(sign_in))) => {
                reading.sign_in = sign_in.clone();
                true
            }
            (TurnEvent::PlanLimit(_), _) => false,
            _ => true,
        });
        let count = events.len() as u32;
        for (ordinal, event) in events.into_iter().enumerate() {
            turn.sink
                .event(envelope(
                    Some(Position {
                        sequence,
                        ordinal: ordinal as u32,
                    }),
                    delivery,
                    event,
                ))
                .await?;
        }
        if message["parent_tool_use_id"].is_string() {
            return Ok(());
        }
        let acknowledged = message["user_message_uuids"].as_array();
        if let Some(uuids) = acknowledged {
            turn.steered
                .retain(|sent| !uuids.iter().any(|uuid| uuid.as_str() == Some(&sent.native)));
        }
        if message["type"] == "assistant" || message["type"] == "stream_event" {
            turn.took_prompt = true;
        }
        if message["type"] != "result" {
            if self.deadline.is_some() && !turn.stopping {
                self.deadline = Some(Instant::now() + MISSED_TIMEOUT);
            }
            return Ok(());
        }
        let failure = turn.projection.failure(message);
        if failure.is_none() && !turn.stopping && acknowledged.is_some() && !turn.steered.is_empty()
        {
            self.deadline = Some(Instant::now() + MISSED_TIMEOUT);
            return Ok(());
        }
        if acknowledged.is_none() {
            turn.steered.clear();
        }
        let outcome = if turn.stopping {
            Outcome::Interrupted
        } else {
            failure.map_or(Outcome::Succeeded, Outcome::Failed)
        };
        turn.took_prompt = true;
        self.finish(
            outcome,
            Some(Position {
                sequence,
                ordinal: count,
            }),
            delivery,
        )
        .await
    }

    async fn ask(&mut self, request_id: String, data: Value) -> io::Result<()> {
        if self.asks.contains_key(&request_id) || self.answered.contains(&request_id) {
            return Ok(());
        }
        let request = serde_json::from_value::<Request>(data)?;
        let stable = request.stable_id(&request_id).to_owned();
        let stopped = request.stopped();
        let (sink, plan) = self
            .running
            .as_ref()
            .filter(|turn| !turn.stopping)
            .map_or((None, false), |turn| (Some(turn.sink.clone()), turn.plan));
        let task = self.replies.spawn({
            let request_id = request_id.clone();
            let stable = stable.clone();
            async move { (request_id, request.answer(sink, stable, plan).await) }
        });
        self.asks.insert(
            request_id,
            Ask {
                stable,
                task,
                stopped,
            },
        );
        Ok(())
    }

    async fn cancel(&mut self, id: &str) -> io::Result<()> {
        self.answered.insert(id.to_owned());
        if let Some(ask) = self.asks.remove(id) {
            ask.task.abort();
            if let Some(turn) = &self.running {
                turn.sink.cancel_ask(&ask.stable).await?;
            }
        }
        Ok(())
    }

    async fn cancel_all(&mut self) -> io::Result<()> {
        for id in self.asks.keys().cloned().collect::<Vec<_>>() {
            self.cancel(&id).await?;
        }
        Ok(())
    }

    async fn answer(&mut self, id: String, body: io::Result<Value>) -> io::Result<()> {
        if self.asks.remove(&id).is_some()
            && let Some(pipe) = &mut self.pipe
        {
            wire::reply(
                &mut pipe.input,
                &id,
                body.map_err(|_| "Kuvo could not complete this tool request."),
            )
            .await?;
            self.answered.insert(id);
        }
        Ok(())
    }

    async fn finish(
        &mut self,
        outcome: Outcome,
        position: Option<Position>,
        delivery: Delivery,
    ) -> io::Result<()> {
        self.cancel_all().await?;
        self.deadline = None;
        let Some(turn) = self.running.take() else {
            return Ok(());
        };
        let missed: Vec<_> = turn.steered.into_iter().map(|sent| sent.kuvo).collect();
        // Claude turned its sign-in away: the next message starts a Claude Code that reads the
        // sign-in as it is then.
        let turned_away = matches!(&outcome, Outcome::Failed(error) if error.kind == "signin");
        if turned_away {
            let program = self.program.clone();
            tokio::task::spawn_blocking(move || program.turn_away()).await?;
        }
        let keep = missed.is_empty() && !turned_away;
        // What changed in its instructions was said with its message: told, once it took it.
        if let Some(told) = &turn.told
            && (turn.took_prompt
                || !matches!(outcome, Outcome::Failed(_) | Outcome::AgentExited(_)))
        {
            self.state
                .insert("claudeTold".into(), Value::String(told.clone()));
            turn.sink.save_state(self.state.clone()).await?;
        }
        turn.sink
            .event(envelope(
                position,
                delivery,
                TurnEvent::Finished(TurnResult {
                    outcome,
                    took_prompt: turn.took_prompt,
                    missed,
                }),
            ))
            .await?;
        if let Some(pipe) = &mut self.pipe {
            pipe.input.note(turn_note(None)?).await?;
        }
        if !keep && let Some(pipe) = self.pipe.take() {
            let _ = pipe.stop().await;
            self.tasks.gone();
        }
        Ok(())
    }

    async fn ended(&mut self) -> io::Result<()> {
        // What it left running went with it.
        self.tasks.gone();
        self.pending.clear();
        self.replay_caught_up().await?;
        let last = self.pipe.take().and_then(|pipe| pipe.said.last());
        let outcome = if self.running.as_ref().is_some_and(|turn| turn.stopping) {
            Outcome::Interrupted
        } else {
            Outcome::AgentExited(last.map(|line| TurnError {
                kind: "agent".into(),
                message: format!("Claude Code stopped: {line}"),
                status: None,
                limit: None,
            }))
        };
        if let Some(reply) = self.prepare.take() {
            let _ = reply.send(Err(stopped()));
        }
        self.finish(outcome, None, Delivery::Live).await
    }

    async fn expired(&mut self) -> io::Result<()> {
        let stopped = self.running.as_ref().is_some_and(|turn| turn.stopping);
        if stopped && let Some(pipe) = self.pipe.take() {
            let _ = pipe.stop().await;
            self.tasks.gone();
        }
        self.finish(
            if stopped {
                Outcome::Interrupted
            } else {
                Outcome::Succeeded
            },
            None,
            Delivery::Live,
        )
        .await
    }
}

/// Claude Code keeps a conversation as `projects/<folder>/<id>.jsonl`; it finds one by its ID in
/// any project folder, and so does this.
fn transcript_has(config: &Path, native: &str, message: &str) -> io::Result<bool> {
    let projects = match fs::read_dir(config.join("projects")) {
        Ok(projects) => projects,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for project in projects {
        let file = match fs::File::open(project?.path().join(format!("{native}.jsonl"))) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for line in BufReader::new(file).lines() {
            let line = line?;
            if !line.contains(message) {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<Value>(&line)
                && entry["type"] == "user"
                && entry["uuid"] == message
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn turn_note(note: Option<&TurnNote>) -> io::Result<Metadata> {
    let turns: BTreeMap<_, _> = note.map(|note| ("main", note)).into_iter().collect();
    Ok(Metadata::from_iter([(
        "turns".into(),
        serde_json::to_value(turns)?,
    )]))
}

#[allow(clippy::too_many_arguments)]
fn turn_launch(
    program: Program,
    session: String,
    directory: PathBuf,
    provider: String,
    native: String,
    resume: bool,
    compact_at: Option<u64>,
    plan: bool,
) -> io::Result<Start> {
    let mut env = program.environment(true);
    let platform = Platform::current();
    let fence = env_value(platform, &env, "KUVO_FENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| program.home.run().join("fence"));
    if !platform.is_windows_host()
        && env_value(platform, &env, "KUVO_SANDBOX").and_then(|value| value.to_str()) != Some("off")
        && (fence.join("on").exists() || fence.is_dir() && !fence.join("decided").exists())
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Claude Code is waiting for Kuvo's sandbox.",
        ));
    }
    let executable = program
        .executable(&env)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Claude Code isn't installed."))?;
    let mut command = vec![executable.path().to_owned()];
    command.extend(
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
            "--include-partial-messages",
            "--permission-prompt-tool=stdio",
            // The mode it starts in; a later one is set over its pipe.
            if plan {
                "--permission-mode=plan"
            } else {
                "--permission-mode=default"
            },
            "--strict-mcp-config",
            if resume { "--resume" } else { "--session-id" },
            &native,
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    env.retain(|name, _| !name.eq_ignore_ascii_case("NODE_OPTIONS"));
    env.entry("CLAUDE_CODE_ENTRYPOINT".into())
        .or_insert_with(|| "sdk-ts".into());
    env.insert("CLAUDE_CODE_SDK_READS_SESSION_STATE".into(), "1".into());
    env.insert("OPENCODE_SESSION_ID".into(), session.clone().into());
    let compact = compact_at.map(|tokens| tokens + 33_000);
    env.retain(|name, _| {
        !name.eq_ignore_ascii_case("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
            && !name.eq_ignore_ascii_case("DISABLE_AUTO_COMPACT")
    });
    if let Some(window) = compact {
        env.insert(
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(),
            window.to_string().into(),
        );
    }
    env.insert(
        "DISABLE_AUTO_COMPACT".into(),
        if compact.is_some() { "0" } else { "1" }.into(),
    );
    let meta = json!({ "provider": provider, "session": session, "claudeAdapter":1, "claudeCompactAt":compact_at });
    Ok(Start {
        command,
        cwd: Some(directory),
        env,
        kind: HoldKind::ControlJson,
        meta: meta.as_object().expect("metadata object").clone(),
        portable_requests: None,
        limits: Limits::default(),
    })
}

fn initialize(instructions: &str) -> Value {
    json!({
        "subtype":"initialize",
        "appendSystemPrompt":instructions,
        "hooks": {"PreToolUse":[{"hookCallbackIds":["kuvo-pre-tool-use"]}]},
    })
}

async fn reinitialize(pipe: &mut Pipe, instructions: &str) -> io::Result<()> {
    pipe.reconnect = Some(pipe.send(initialize(instructions)).await?);
    Ok(())
}

async fn configure(pipe: &mut Pipe, model: &ModelRef, plan: bool) -> io::Result<()> {
    let selected = if model.id == "default" {
        json!({ "subtype":"set_model" })
    } else {
        json!({ "subtype":"set_model", "model":model.id })
    };
    for request in [
        selected,
        json!({ "subtype":"set_permission_mode", "mode":if plan { "plan" } else { "default" } }),
    ] {
        if matches!(pipe.call(request).await?, wire::ReplyBody::Error) {
            return Err(io::Error::other(
                "Claude Code refused its model or permission mode.",
            ));
        }
    }
    let _ = pipe
        .call(json!({ "subtype":"apply_flag_settings", "settings":{"effortLevel":model.variant} }))
        .await?;
    Ok(())
}

fn envelope(position: Option<Position>, delivery: Delivery, event: TurnEvent) -> EventEnvelope {
    EventEnvelope {
        position,
        delivery,
        event,
    }
}

async fn start_cancelled(cancel: &CancellationToken, stop: &mut watch::Receiver<()>) {
    tokio::select! {
        () = cancel.cancelled() => {},
        _ = stop.changed() => {},
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "Claude Code has stopped.")
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(byte, value)| {
            if matches!(byte, 8 | 13 | 18 | 23) {
                value == b'-'
            } else {
                value.is_ascii_hexdigit()
            }
        })
}

fn unique_id() -> io::Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(uuid(bytes, 4))
}

fn uuid(mut bytes: [u8; 16], version: u8) -> String {
    bytes[6] = (bytes[6] & 15) | (version << 4);
    bytes[8] = (bytes[8] & 63) | 128;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

fn prompt_id(session: &str, id: String) -> String {
    if is_uuid(&id) {
        return id;
    }
    let mut digest = Sha256::new();
    digest.update(b"kuvo-claude-prompt-v1\0");
    digest.update(session.as_bytes());
    digest.update(b"\0");
    digest.update(id.as_bytes());
    let hash = digest.finalize();
    uuid(hash[..16].try_into().expect("16 hash bytes"), 8)
}

async fn message(session: &str, mut prompt: Prompt, next: bool) -> io::Result<Value> {
    let id = prompt_id(session, std::mem::take(&mut prompt.id));
    let content = tokio::task::spawn_blocking(move || prompt_content(prompt)).await?;
    Ok(user_frame(id, content, next))
}

fn prompt_content(prompt: Prompt) -> Vec<Value> {
    let mut content = Vec::new();
    let mut text = prompt.text;
    let mut paths = Vec::new();
    for file in prompt.files {
        let image = matches!(
            file.mime.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        );
        let data = if image {
            file.data
                .or_else(|| file.path.as_deref().and_then(image_bytes))
        } else {
            None
        };
        if let Some(data) = data.filter(|data| !data.is_empty()) {
            content.push(json!({ "type":"image", "source":{ "type":"base64", "media_type":file.mime, "data":data } }));
        } else if let Some(path) = file.path.as_deref().and_then(Path::to_str) {
            paths.push(format!("- {path}"));
        }
    }
    if !paths.is_empty() {
        text.push_str("\n\nAttached files:\n");
        text.push_str(&paths.join("\n"));
    }
    content.push(json!({ "type":"text", "text":text }));
    content
}

fn image_bytes(path: &Path) -> Option<String> {
    const LIMIT: u64 = 24_000_000;
    let stat = fs::metadata(path).ok()?;
    if !stat.is_file() || stat.len() > LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= LIMIT).then(|| base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn user_frame(id: String, content: Vec<Value>, next: bool) -> Value {
    let mut message = json!({
        "type":"user", "uuid":id, "parent_tool_use_id":null, "message":{"role":"user", "content":content},
    });
    if next {
        message["priority"] = "next".into();
    }
    message
}
