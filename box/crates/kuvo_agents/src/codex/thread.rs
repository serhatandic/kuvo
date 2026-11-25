//! A conversation's Codex thread: loaded in the account's app-server, its turns started and
//! steered, what Codex says made its steps, and what it asks put to you.

use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex as Plain,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Map, Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::Instant,
};

use crate::{
    hold::TurnNote,
    rpc::{Failure, RpcError},
    runtime::AdapterCall,
    session::{
        ActiveTurn, Answer, Delivery, EventEnvelope, Field, FieldKind, FormOption, Limit, Outcome,
        PermissionMode, Prompt, PromptFile, QuestionAsk, SessionContext, Steer, SteerReceipt,
        Thread, TurnError, TurnEvent, TurnInput, TurnKind, TurnResult, TurnSink, VendorState,
    },
    session_view::Compaction,
    told::instructions_note,
    tool_relay::{AgentTools, ToolRelay},
};

use super::{
    asks,
    projection::Lane,
    server::{Event, Route, Server, Shared, lock, stopped},
};

/// What Codex's models are told of asking you. They list request_user_input outside Plan too,
/// where it refuses them, and a model that reached for it said it couldn't ask.
pub(super) const ASKING_WORDS: &str = "Outside Plan mode, ask the user with request_user_input_async: request_user_input refuses there, which doesn't mean you can't ask. It returns at once, and the answer comes as a new user message, which also ends a sleep you're in.";

const LOAD: Duration = Duration::from_secs(60);
const STEER: Duration = Duration::from_secs(30);
const STOP: Duration = Duration::from_secs(10);
/// Turned down by its box this many times with no work in between: no further turn for it; and
/// this many in one turn: the turn is stopped.
const TURNED_DOWN_TURNS: u32 = 2;
const TURNED_DOWN_IN_TURN: u32 = 5;

static HOLDERS: AtomicU64 = AtomicU64::new(1);

type Reply<T> = oneshot::Sender<io::Result<T>>;

enum Command {
    Start(Box<TurnInput>, Arc<dyn TurnSink>, Reply<()>),
    Steer(String, Prompt, Reply<Steer>),
    Interrupt(String, Reply<()>),
    Resolve(SteerReceipt, Reply<Option<bool>>),
    Handover(Reply<()>),
    LetGo(TurnNote, Reply<()>),
    /// `signed_out`: its account signed out (a turn under way fails, as Codex stopped);
    /// otherwise removed, or the box stopping (it ends as stopped).
    Close(bool, Reply<()>),
}

#[derive(Clone)]
pub(super) struct Conversation {
    commands: mpsc::Sender<Command>,
    /// The thread and the turn under way, for a steered message's receipt before it's sent.
    current: Arc<Plain<Option<(String, String)>>>,
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
            .get("codexThread")
            .and_then(Value::as_str)
            .map(str::to_owned);
        shared.routes().insert(
            context.session_id.clone(),
            Route {
                native: native.clone(),
                server: None,
                busy: busy.clone(),
                events: events_in,
            },
        );
        let current = Arc::new(Plain::new(None));
        let handed = Arc::new(Plain::new(Vec::new()));
        let conversation = Self {
            commands,
            current: current.clone(),
            _handed: handed.clone(),
        };
        tokio::spawn(
            Actor {
                shared,
                relay,
                holder: HOLDERS.fetch_add(1, Ordering::Relaxed),
                session: context.session_id,
                directory: context.directory,
                state: context.state,
                native,
                server: None,
                loaded: false,
                loaded_with: None,
                busy,
                current,
                handed,
                active: None,
                asks: HashMap::new(),
                replies: JoinSet::new(),
                cards: HashMap::new(),
                answers: Vec::new(),
                turned_down: 0,
                deadline: None,
                closing: false,
            }
            .run(receiver, events),
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

    pub async fn handover(&self) -> io::Result<()> {
        self.send(Command::Handover).await
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

    /// Codex keeps a steered message's client ID in its thread once it takes it.
    fn resolve_steer(&self, receipt: SteerReceipt) -> AdapterCall<'_, Option<bool>> {
        Box::pin(self.send(|reply| Command::Resolve(receipt, reply)))
    }
}

struct Turn {
    id: String,
    conversation: Conversation,
}

impl ActiveTurn for Turn {
    fn steer_receipt(&self, prompt: &Prompt) -> Option<SteerReceipt> {
        let (thread, turn) = lock(&self.conversation.current).clone()?;
        Some(SteerReceipt {
            thread,
            turn,
            message: prompt.id.clone(),
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

/// What the thread was loaded with: loaded again when it changes.
#[derive(Clone, PartialEq)]
struct Loaded {
    instructions: String,
    tools: Option<String>,
    servers: String,
    compaction: String,
}

/// A turn under way: one of yours, and the native turns it takes (your answers to its questions
/// start more).
struct Active {
    id: String,
    sink: Arc<dyn TurnSink>,
    lane: Lane,
    input: TurnStart,
    /// Codex's turn, once it says.
    turn: Option<String>,
    /// Taken up from its holder: the holder's last line before this turn began.
    from: Option<u64>,
    /// The native turn ended, and its questions are still open.
    between: bool,
    stopping: bool,
    stopped_asking: bool,
    stopped_for_limit: bool,
    problem: Option<String>,
    limit: Option<Limit>,
    took_prompt: bool,
    /// The instructions said ahead of its message, kept as told once it took it.
    told: Option<String>,
}

/// What a turn starts with, for the native turns your answers start too.
#[derive(Clone)]
struct TurnStart {
    model: String,
    effort: Option<String>,
    plan: bool,
    auto: bool,
    approve_all: bool,
    compact: bool,
}

struct Ask {
    stable: String,
    task: tokio::task::AbortHandle,
}

/// Its question's card, while it's up: a message you send meanwhile answers it instead.
struct Card {
    task: tokio::task::AbortHandle,
    stable: String,
}

/// What Codex hears of a question.
struct Said {
    text: String,
    files: Vec<PromptFile>,
    declined: bool,
    message: Option<String>,
}

enum Settled {
    /// An answer to Codex's request, by its app-server and ID.
    Request(u64, Value, Result<Value, RpcError>),
    /// A question's card: what you answered, or a dismissal.
    Card(String, io::Result<Answer>),
}

struct Actor {
    shared: Arc<Shared>,
    relay: Option<ToolRelay>,
    holder: u64,
    session: String,
    directory: PathBuf,
    state: VendorState,
    native: Option<String>,
    server: Option<Arc<Server>>,
    loaded: bool,
    loaded_with: Option<Loaded>,
    busy: Arc<AtomicBool>,
    current: Arc<Plain<Option<(String, String)>>>,
    handed: Arc<Plain<Vec<Arc<dyn TurnSink>>>>,
    active: Option<Active>,
    /// Codex's asks under way, by app-server and ID.
    asks: HashMap<(u64, String), Ask>,
    replies: JoinSet<Settled>,
    cards: HashMap<String, Card>,
    /// Answers no native turn took as they came: the next starts with them.
    answers: Vec<Said>,
    turned_down: u32,
    deadline: Option<Instant>,
    closing: bool,
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

impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut events: mpsc::UnboundedReceiver<Event>,
    ) {
        while !self.closing {
            let timer = async {
                match self.deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let result = tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
                Some(event) = events.recv() => self.event(event).await,
                Some(settled) = self.replies.join_next() => match settled {
                    Ok(settled) => self.settled(settled).await,
                    Err(_) => Ok(()),
                },
                () = timer => self.expired().await,
            };
            if let Err(error) = result {
                eprintln!("Codex conversation {}: {error}", self.session);
            }
        }
        self.shared.routes().remove(&self.session);
    }

    fn route(&self) {
        let mut routes = self.shared.routes();
        if let Some(route) = routes.get_mut(&self.session) {
            route.native = self.native.clone();
            route.server = self.server.as_ref().map(|server| server.id);
        }
    }

    async fn command(&mut self, command: Command) -> io::Result<()> {
        match command {
            Command::Start(input, sink, reply) => {
                let result = self.start(*input, sink).await;
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
            Command::Handover(reply) => {
                // The next resident goes on with the turn: this one answers nothing more of it.
                if let Some(active) = self.active.take() {
                    lock(&self.handed).push(active.sink);
                }
                for (_, ask) in self.asks.drain() {
                    ask.task.abort();
                }
                self.closing = true;
                let _ = reply.send(Ok(()));
            }
            Command::LetGo(note, reply) => {
                let result = self.let_go(note).await;
                let _ = reply.send(result);
            }
            Command::Close(signed_out, reply) => {
                // Its account signed out (Codex stopped under it), or removed, or the box
                // stopping (the turn ends as stopped, without Stop).
                if self.active.is_some() {
                    self.stop_native().await?;
                    let outcome = if signed_out {
                        Outcome::Failed(failure("Codex stopped."))
                    } else {
                        Outcome::Interrupted
                    };
                    self.finish(outcome).await?;
                }
                if let Some(server) = self.server.clone().filter(|server| !server.closed())
                    && let Some(native) = self.native.clone().filter(|_| self.loaded)
                {
                    let _ = server
                        .rpc
                        .request(
                            "thread/unsubscribe",
                            Some(json!({ "threadId": native })),
                            Some(STOP),
                        )
                        .await;
                }
                if let Some(relay) = &self.relay {
                    relay.drop_relayed(&self.session, self.holder);
                }
                self.closing = true;
                let _ = reply.send(Ok(()));
            }
        }
        Ok(())
    }

    /// Loaded in the app-server now, with what it's given now: again when that changed.
    async fn ensure_thread(
        &mut self,
        model: &str,
        instructions: &str,
        sink: &Arc<dyn TurnSink>,
    ) -> io::Result<Arc<Server>> {
        let server = self.shared.server().await?;
        if self.server.as_ref().is_none_or(|old| old.id != server.id) {
            self.server = Some(server.clone());
            self.loaded = false;
        }
        self.route();
        let instructions = [instructions, ASKING_WORDS]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let compaction = Compaction::read(self.shared.program.home.root());
        let mut config = Map::new();
        // Codex caps this at 90% of the model window. Without it, Codex chooses its limit.
        if let Compaction::At(tokens) = compaction {
            config.insert("model_auto_compact_token_limit".into(), json!(tokens));
        }
        config.insert(
            "shell_environment_policy.set.OPENCODE_SESSION_ID".into(),
            json!(self.session),
        );
        let mut tools_key = None;
        let mut servers_key = String::new();
        if let Some(relay) = &self.relay {
            // Kuvo Code's own tools, as the engine's models have them (for this thread alone).
            if let Some(tools) = relay.agent_tools(&self.session, self.holder, false).await? {
                let entry = match tools {
                    AgentTools::Http {
                        key,
                        url,
                        headers,
                        timeout,
                    } => {
                        tools_key = Some(key);
                        json!({ "url": url, "http_headers": headers.into_iter().map(|(name, value)| (name, json!(value))).collect::<Map<_, _>>(), "tool_timeout_sec": timeout.as_secs_f64().ceil() as u64 })
                    }
                    AgentTools::Stdio {
                        key,
                        command,
                        args,
                        env,
                        timeout,
                    } => {
                        tools_key = Some(key);
                        let env: Map<String, Value> = env
                            .into_iter()
                            .map(|(name, value)| {
                                (
                                    name.to_string_lossy().into_owned(),
                                    json!(value.to_string_lossy()),
                                )
                            })
                            .collect();
                        let args: Vec<String> = args
                            .iter()
                            .map(|arg| arg.to_string_lossy().into_owned())
                            .collect();
                        json!({ "command": command, "args": args, "env": env, "tool_timeout_sec": timeout.as_secs_f64().ceil() as u64 })
                    }
                };
                config.insert(
                    format!("mcp_servers.{}", super::projection::TOOLS_SERVER),
                    entry,
                );
            }
            // The computer's MCP servers, through the relay, which keeps their keys.
            let found = relay.computer_servers(&self.directory).await;
            servers_key = relay.servers_key(&self.session, &found).await;
            for server in relay
                .relayed_servers(&self.session, self.holder, &self.directory, &found, false)
                .await?
            {
                config.insert(
                    format!("mcp_servers.{}", server.name),
                    json!({
                        "url": server.url, "http_headers": {"Authorization": server.authorization},
                        "tool_timeout_sec": server.timeout.as_secs_f64().ceil() as u64, "startup_timeout_sec": 120,
                    }),
                );
            }
        }
        let loaded = Loaded {
            instructions: instructions.clone(),
            tools: tools_key,
            servers: servers_key,
            compaction: compaction.id(),
        };
        if self.native.is_some() && self.loaded && self.loaded_with.as_ref() == Some(&loaded) {
            return Ok(server);
        }
        let common = json!({
            "cwd": self.directory, "model": model, "developerInstructions": instructions, "config": config,
        });
        if let Some(native) = self.native.clone() {
            let mut resume = common.clone();
            resume["threadId"] = json!(native);
            resume["excludeTurns"] = json!(true);
            match server
                .rpc
                .request("thread/resume", Some(resume.clone()), Some(LOAD))
                .await
            {
                Ok(_) => {
                    self.loaded = true;
                    self.loaded_with = Some(loaded);
                    return Ok(server);
                }
                Err(Failure::Refused(error))
                    if error.message.to_lowercase().contains("archived")
                        || error.message.to_lowercase().contains("unarchive") =>
                {
                    let _ = server
                        .rpc
                        .request(
                            "thread/unarchive",
                            Some(json!({ "threadId": native })),
                            Some(LOAD),
                        )
                        .await;
                    server
                        .rpc
                        .request("thread/resume", Some(resume), Some(LOAD))
                        .await?;
                    self.loaded = true;
                    self.loaded_with = Some(loaded);
                    return Ok(server);
                }
                // Gone (deleted, or another Codex's): a new thread, without what came before.
                Err(Failure::Refused(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let started = server
            .rpc
            .request("thread/start", Some(common), Some(LOAD))
            .await?;
        let id = started["thread"]["id"]
            .as_str()
            .ok_or_else(|| io::Error::other("Codex didn't start a thread."))?
            .to_owned();
        self.native = Some(id.clone());
        self.route();
        self.state.insert("codexThread".into(), json!(id));
        // A new thread has the instructions it's given now.
        self.state.insert("codexTold".into(), json!(instructions));
        sink.save_state(self.state.clone()).await?;
        self.loaded = true;
        self.loaded_with = Some(loaded);
        Ok(server)
    }

    async fn start(&mut self, input: TurnInput, sink: Arc<dyn TurnSink>) -> io::Result<()> {
        if self.active.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Codex is already answering.",
            ));
        }
        let start = TurnStart {
            model: input.model.id.clone(),
            effort: input.model.variant.clone(),
            plan: input.plan,
            auto: input.auto,
            approve_all: input.permissions == PermissionMode::ApproveAll,
            compact: matches!(input.kind, TurnKind::Compact),
        };
        if let TurnKind::Recover { note } = input.kind {
            return self.recover(input.id, note, start, sink).await;
        }
        let prompt = match input.kind {
            TurnKind::Prompt(prompt) => Some(prompt),
            TurnKind::Compact => None,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "This Codex turn requires a prompt.",
                ));
            }
        };
        self.turned_down = 0;
        self.answers.clear();
        self.busy.store(true, Ordering::Relaxed);
        let result = self
            .begin(input.id, prompt, start, input.instructions, sink)
            .await;
        if result.is_err() {
            self.busy.store(false, Ordering::Relaxed);
        }
        result
    }

    async fn begin(
        &mut self,
        id: String,
        prompt: Option<Prompt>,
        start: TurnStart,
        instructions: String,
        sink: Arc<dyn TurnSink>,
    ) -> io::Result<()> {
        self.ensure_thread(&start.model, &instructions, &sink)
            .await?;
        let mut active = Active {
            id: id.clone(),
            sink,
            lane: Lane::new(&id, &start.model, start.compact),
            input: start,
            turn: None,
            from: None,
            between: false,
            stopping: false,
            stopped_asking: false,
            stopped_for_limit: false,
            problem: None,
            limit: None,
            took_prompt: false,
            told: None,
        };
        let message = prompt.map(|mut prompt| {
            // A thread it goes on with keeps the instructions it started with: what changed is
            // said ahead of the message, once.
            let loaded = self
                .loaded_with
                .as_ref()
                .map(|loaded| loaded.instructions.clone())
                .unwrap_or_default();
            if let Some(note) =
                instructions_note(self.state.get("codexTold").and_then(Value::as_str), &loaded)
            {
                prompt.text = format!("{note}\n\n{}", prompt.text);
                active.told = Some(loaded);
            }
            prompt
        });
        self.active = Some(active);
        self.native_turn(message.map(|prompt| (prompt.text, prompt.files, Some(prompt.id))))
            .await
    }

    /// A native turn of the turn under way: with your message, an answer, or a summary.
    async fn native_turn(
        &mut self,
        message: Option<(String, Vec<PromptFile>, Option<String>)>,
    ) -> io::Result<()> {
        let server = self.server.clone().ok_or_else(stopped)?;
        let native = self.native.clone().ok_or_else(stopped)?;
        let active = self.active.as_mut().ok_or_else(stopped)?;
        active.turn = None;
        active.between = false;
        let start = active.input.clone();
        let request = match message {
            None => ("thread/compact/start", json!({ "threadId": native })),
            Some((text, files, message)) => {
                active.lane.start_step();
                let mut params = json!({
                    "threadId": native, "input": codex_input(&text, &files), "cwd": self.directory,
                    "model": start.model, "summary": "detailed", "approvalsReviewer": "user",
                    "collaborationMode": {"mode": if start.plan { "plan" } else { "default" }, "settings": {
                        "model": start.model, "reasoning_effort": start.effort, "developer_instructions": null,
                    }},
                });
                if let Some(message) = message {
                    params["clientUserMessageId"] = json!(message);
                }
                let (approval, sandbox) = policy(start.plan, start.approve_all);
                params["approvalPolicy"] = json!(approval);
                params["sandboxPolicy"] = sandbox;
                if let Some(effort) = &start.effort {
                    params["effort"] = json!(effort);
                }
                if let Some(tier) = self.service_tier(&start.model).await {
                    params["serviceTier"] = tier;
                }
                ("turn/start", params)
            }
        };
        let compact = request.0 == "thread/compact/start";
        let events = self
            .active
            .as_mut()
            .map(|active| active.lane.take())
            .unwrap_or_default();
        self.emit_all(events).await?;
        // Under its holder: where this turn's lines begin, for a resident taking it up.
        let from = server.rpc.seen();
        if !compact {
            let id = self
                .active
                .as_ref()
                .map(|active| active.id.clone())
                .unwrap_or_default();
            server
                .note_turn(
                    &native,
                    Some(json!({ "session": self.session, "from": from, "turn": id })),
                )
                .await;
        }
        let answer = server
            .rpc
            .request(request.0, Some(request.1), Some(LOAD))
            .await;
        match answer {
            Ok(result) => {
                if let Some(turn) = result["turn"]["id"].as_str()
                    && let Some(active) = self.active.as_mut()
                {
                    active.turn.get_or_insert_with(|| turn.to_owned());
                    active.took_prompt = true;
                    let turn = active.turn.clone().unwrap_or_default();
                    *lock(&self.current) = Some((native.clone(), turn.clone()));
                    if !compact {
                        let note = json!({ "session": self.session, "from": from, "turn": active.id, "state": {"codexTurn": turn} });
                        server.note_turn(&native, Some(note)).await;
                    }
                }
                Ok(())
            }
            Err(error) => {
                if !compact {
                    server.note_turn(&native, None).await;
                }
                let first = self
                    .active
                    .as_ref()
                    .is_some_and(|active| !active.took_prompt);
                if first {
                    self.active = None;
                    return Err(error.into());
                }
                self.finish(Outcome::Failed(failure(error.to_string())))
                    .await
            }
        }
    }

    /// The speed you picked for this account, as each turn starts: that speed where the model
    /// lists it, null for Standard, nothing where none was picked or Codex can't say.
    async fn service_tier(&self, model: &str) -> Option<Value> {
        let file = self.shared.program.home.root().join("box/models.json");
        let saved: Value = serde_json::from_slice(&tokio::fs::read(file).await.ok()?).ok()?;
        let picked = saved["speeds"]
            .as_object()?
            .get(&self.shared.program.provider)?
            .clone();
        let Some(picked) = picked.as_str() else {
            return picked.is_null().then_some(Value::Null);
        };
        let models = self.shared.account.models(&self.shared, false).await.ok()?;
        let listed = models.models.iter().find(|each| each.id == model)?;
        Some(if listed.speeds.iter().any(|speed| speed.id == picked) {
            json!(picked)
        } else {
            Value::Null
        })
    }

    /// A turn an earlier resident left under way in its held app-server: taken up as it is,
    /// sending nothing; what Codex said since it began comes again.
    async fn recover(
        &mut self,
        id: String,
        note: TurnNote,
        start: TurnStart,
        sink: Arc<dyn TurnSink>,
    ) -> io::Result<()> {
        let server = self.shared.current().await.ok_or_else(stopped)?;
        let native = self
            .native
            .clone()
            .ok_or_else(|| io::Error::other("This conversation has no Codex thread."))?;
        self.server = Some(server.clone());
        self.loaded = true;
        self.route();
        self.busy.store(true, Ordering::Relaxed);
        let turn = note
            .state
            .as_ref()
            .and_then(|state| state["codexTurn"].as_str())
            .map(str::to_owned);
        if let Some(turn) = &turn {
            *lock(&self.current) = Some((native.clone(), turn.clone()));
        }
        let mut lane = Lane::new(&id, &start.model, false);
        lane.start_step();
        let events = lane.take();
        self.active = Some(Active {
            id,
            sink,
            lane,
            input: start,
            turn,
            from: Some(note.from),
            between: false,
            stopping: false,
            stopped_asking: false,
            stopped_for_limit: false,
            problem: None,
            limit: None,
            took_prompt: true,
            told: None,
        });
        self.emit_all(events).await?;
        server.ready(&native);
        Ok(())
    }

    async fn let_go(&mut self, note: TurnNote) -> io::Result<()> {
        let Some(server) = self.shared.current().await else {
            return Ok(());
        };
        let Some(native) = self.native.clone() else {
            return Ok(());
        };
        server.note_turn(&native, None).await;
        if let Some(turn) = note
            .state
            .as_ref()
            .and_then(|state| state["codexTurn"].as_str())
        {
            let _ = server
                .rpc
                .request(
                    "turn/interrupt",
                    Some(json!({ "threadId": native, "turnId": turn })),
                    Some(STEER),
                )
                .await;
        }
        server.ready(&native);
        Ok(())
    }

    async fn emit_all(&mut self, events: Vec<TurnEvent>) -> io::Result<()> {
        let Some(sink) = self.active.as_ref().map(|active| active.sink.clone()) else {
            return Ok(());
        };
        for event in events {
            sink.event(envelope(event)).await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> io::Result<()> {
        let events = self
            .active
            .as_mut()
            .map(|active| active.lane.take())
            .unwrap_or_default();
        self.emit_all(events).await
    }

    async fn event(&mut self, event: Event) -> io::Result<()> {
        match event {
            Event::Stopped { server } => {
                if self
                    .server
                    .as_ref()
                    .is_none_or(|current| current.id != server)
                {
                    return Ok(());
                }
                self.loaded = false;
                self.server = None;
                self.route();
                if self.active.as_ref().is_some_and(|active| active.stopping) {
                    return self.finish(Outcome::Interrupted).await;
                }
                if let Some(active) = &self.active {
                    let why = active
                        .problem
                        .clone()
                        .unwrap_or_else(|| "Codex stopped.".into());
                    return self.finish(Outcome::AgentExited(Some(failure(why)))).await;
                }
                Ok(())
            }
            Event::Request {
                server,
                id,
                method,
                params,
            } => self.request(server, id, method, params).await,
            Event::Notification {
                server,
                method,
                params,
                sequence,
            } => {
                if self
                    .server
                    .as_ref()
                    .is_some_and(|current| current.id != server)
                    && method != "serverRequest/resolved"
                {
                    return Ok(());
                }
                self.notification(&method, &params, sequence).await
            }
        }
    }

    async fn request(
        &mut self,
        server: Arc<Server>,
        id: Value,
        method: String,
        params: Value,
    ) -> io::Result<()> {
        let key = (server.id, id.to_string());
        // (Asked again under the same ID, as it's taken up from its holder: the one ask.)
        if self.asks.contains_key(&key) {
            return Ok(());
        }
        let Some(active) = self.active.as_ref().filter(|active| !active.stopping) else {
            server.rpc.respond(id, asks::unasked(&method)).await;
            return Ok(());
        };
        let item = params["itemId"].as_str().unwrap_or_default();
        let changes = active.lane.changes.get(item).cloned().unwrap_or_default();
        let sink = active.sink.clone();
        let planning = active.input.plan;
        // Stable across residents: Codex's own ID for the ask, on its thread.
        let stable = format!(
            "codex-{}-{}",
            self.native.as_deref().unwrap_or("thread"),
            id.to_string().trim_matches('"')
        );
        let task = self.replies.spawn({
            let stable = stable.clone();
            let server = server.id;
            async move {
                let answer = asks::answer(sink, stable, &method, params, changes, planning).await;
                Settled::Request(server, id, answer)
            }
        });
        self.asks.insert(key, Ask { stable, task });
        Ok(())
    }

    async fn settled(&mut self, settled: Settled) -> io::Result<()> {
        match settled {
            Settled::Request(server, id, answer) => {
                if self.asks.remove(&(server, id.to_string())).is_none() {
                    return Ok(());
                }
                // The resident going away: left unanswered for the next one, which asks again.
                if matches!(&answer, Err(error) if error.code == -32603 && self.closing) {
                    return Ok(());
                }
                if let Some(current) = self.server.clone().filter(|current| current.id == server) {
                    current.rpc.respond(id, answer).await;
                }
                Ok(())
            }
            Settled::Card(stable, answer) => {
                if self.cards.remove(&stable).is_none() {
                    return Ok(());
                }
                let said = match answer {
                    Ok(Answer::Submitted(values)) => Some(Said {
                        text: field_text(values.get("text")),
                        files: Vec::new(),
                        declined: false,
                        message: None,
                    }),
                    Ok(Answer::Dismissed {
                        reason: Some(words),
                    }) => Some(Said {
                        text: words,
                        files: Vec::new(),
                        declined: true,
                        message: None,
                    }),
                    _ => None,
                };
                self.deliver(said).await
            }
        }
    }

    /// What Codex hears of a question settled: into the turn under way, or kept for the next.
    async fn deliver(&mut self, said: Option<Said>) -> io::Result<()> {
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        if active.stopping {
            return Ok(());
        }
        // Turned down by its box and asking again and again, with no work in between: no more.
        if said.as_ref().is_some_and(|said| said.declined) {
            self.turned_down += 1;
            let limit = if active.between {
                TURNED_DOWN_TURNS
            } else {
                TURNED_DOWN_IN_TURN
            };
            if self.turned_down > limit {
                active.stopped_asking = true;
                self.answers.clear();
                if active.between {
                    return self.settle_between().await;
                }
                return self.stop_native().await;
            }
        }
        // Dismissed: a turn at work hears so rather than wait on it; one that's over just ends.
        let dismissed = said.is_none();
        let said = match said {
            Some(said) => said,
            None if active.between => return self.settle_between().await,
            None => Said {
                text: "The user dismissed this question without answering it. Don't wait for an answer.".into(),
                files: Vec::new(),
                declined: false,
                message: None,
            },
        };
        if !active.between && active.turn.is_some() && self.steer_in(&said.text, &said.files).await
        {
            return Ok(());
        }
        // (A dismissal its turn didn't take isn't kept: there's nothing to tell a next turn.)
        if !dismissed {
            self.answers.push(said);
        }
        if self.active.as_ref().is_some_and(|active| active.between) {
            return self.settle_between().await;
        }
        Ok(())
    }

    /// `text` into the native turn under way. Whether Codex took it.
    async fn steer_in(&mut self, text: &str, files: &[PromptFile]) -> bool {
        let (Some(server), Some(native), Some(turn)) = (
            self.server.clone(),
            self.native.clone(),
            self.active.as_ref().and_then(|active| active.turn.clone()),
        ) else {
            return false;
        };
        server
            .rpc
            .request(
                "turn/steer",
                Some(json!({ "threadId": native, "input": codex_input(text, files), "expectedTurnId": turn })),
                Some(STEER),
            )
            .await
            .is_ok()
    }

    /// Between native turns with questions open: once they're settled, an answer no turn took
    /// starts the next, and with none the turn is over.
    async fn settle_between(&mut self) -> io::Result<()> {
        if !self.cards.is_empty() {
            return Ok(());
        }
        let stopped = self
            .active
            .as_ref()
            .is_some_and(|active| active.stopping || active.stopped_asking);
        if self.answers.is_empty() || stopped {
            return self.finish(Outcome::Succeeded).await;
        }
        let said = std::mem::take(&mut self.answers);
        let message = if said.len() == 1 {
            said[0].message.clone()
        } else {
            None
        };
        let text = said
            .iter()
            .map(|each| each.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let files = said.into_iter().flat_map(|each| each.files).collect();
        self.native_turn(Some((text, files, message))).await
    }

    async fn notification(
        &mut self,
        method: &str,
        params: &Value,
        sequence: u64,
    ) -> io::Result<()> {
        if method == "serverRequest/resolved" {
            // Codex settled an ask itself: its card goes.
            let id = params["requestId"].to_string();
            let found: Vec<_> = self
                .asks
                .keys()
                .filter(|(_, ask)| *ask == id)
                .cloned()
                .collect();
            for key in found {
                if let Some(ask) = self.asks.remove(&key) {
                    ask.task.abort();
                    if let Some(active) = &self.active {
                        active.sink.cancel_ask(&ask.stable).await?;
                    }
                }
            }
            return Ok(());
        }
        if method == "thread/closed" {
            self.loaded = false;
            return Ok(());
        }
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        // Taken up from its holder: what came before its own turn began isn't this turn's.
        if active.from.is_some_and(|from| sequence <= from) {
            return Ok(());
        }
        let turn = params["turnId"]
            .as_str()
            .or_else(|| params["turn"]["id"].as_str());
        if let (Some(mine), Some(turn)) = (&active.turn, turn)
            && mine != turn
        {
            return Ok(());
        }
        if active.between && method != "turn/started" {
            return Ok(());
        }
        // A question for you, its words shown as they are: asked now, whatever it does next.
        if method == "item/completed" && params["item"]["type"] == "agentMessage" {
            let titles: Vec<(String, Vec<String>)> = params["item"]["questions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|question| {
                    let title = question["title"]
                        .as_str()
                        .filter(|title| !title.is_empty())?;
                    let options = question["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                    Some((title.to_owned(), options))
                })
                .collect();
            if !titles.is_empty() {
                let item = params["item"]["id"]
                    .as_str()
                    .unwrap_or("question")
                    .to_owned();
                self.ask(item, titles);
            }
        }
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        if matches!(method, "item/started" | "item/completed") {
            let kind = params["item"]["type"].as_str().unwrap_or_default();
            if method == "item/started"
                && matches!(
                    kind,
                    "commandExecution"
                        | "fileChange"
                        | "mcpToolCall"
                        | "dynamicToolCall"
                        | "webSearch"
                        | "imageGeneration"
                        | "imageView"
                        | "collabAgentToolCall"
                )
            {
                self.turned_down = 0;
            }
        }
        match method {
            "turn/started" => {
                if let Some(turn) = params["turn"]["id"].as_str() {
                    active.turn.get_or_insert_with(|| turn.to_owned());
                    if let Some(native) = self.native.clone() {
                        *lock(&self.current) = Some((native, turn.to_owned()));
                    }
                }
                active.between = false;
                Ok(())
            }
            "thread/tokenUsage/updated" => {
                let window = params["tokenUsage"]["modelContextWindow"].as_u64();
                self.shared.account.note_context(&active.lane.model, window);
                active.lane.notification(method, params);
                self.flush().await
            }
            "error" => {
                let error = &params["error"];
                let retrying = params["willRetry"] == true;
                let limit = self.shared.account.limit_of(error, &active.input.model);
                let message = error["message"].as_str().filter(|text| !text.is_empty());
                if let Some(limit) = limit {
                    if retrying {
                        // On Auto, the next model goes on rather than waiting for Codex to try again.
                        if active.input.auto {
                            active.limit = Some(limit);
                            active.problem =
                                Some(message.unwrap_or("Codex is rate limited for now.").into());
                            active.stopped_for_limit = true;
                            return self.stop_native().await;
                        }
                        return Ok(());
                    }
                    active.limit = Some(limit);
                    active.problem = Some(
                        message
                            .unwrap_or("You've reached your Codex usage limit for now.")
                            .into(),
                    );
                    return Ok(());
                }
                if !retrying {
                    active.problem = error["additionalDetails"]
                        .as_str()
                        .or(message)
                        .map(str::to_owned);
                } else {
                    let said = message.unwrap_or("the connection dropped");
                    active.lane.notice_once(
                        "retry",
                        format!("Codex hit a problem and is trying again: {said}."),
                    );
                }
                self.flush().await
            }
            "turn/completed" => self.turn_completed(params).await,
            "account/rateLimits/updated" => {
                // Where the plan's limits stand, for the account the turn runs on, under the
                // sign-in it was heard under.
                let windows = super::account::windows(&params["rateLimits"]);
                let sign_in = self
                    .shared
                    .program
                    .credentials()
                    .await
                    .map(|found| super::program::fingerprint(&found.token));
                if windows.is_empty() {
                    return Ok(());
                }
                let event =
                    TurnEvent::PlanLimit(crate::session::PlanLimitReading { windows, sign_in });
                self.emit_all(vec![event]).await
            }
            _ => {
                active.lane.notification(method, params);
                self.flush().await
            }
        }
    }

    async fn turn_completed(&mut self, params: &Value) -> io::Result<()> {
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        active.lane.close_plan();
        active.lane.end_step(None);
        if let (Some(server), Some(native)) = (&self.server, &self.native) {
            server.note_turn(native, None).await;
        }
        let status = params["turn"]["status"].as_str().unwrap_or_default();
        let error = &params["turn"]["error"];
        // A limit only when the turn didn't go through: one it got past on a retry doesn't count.
        let limit = match status {
            "failed" => active
                .limit
                .clone()
                .or_else(|| self.shared.account.limit_of(error, &active.input.model)),
            "interrupted" if active.stopped_for_limit => active.limit.clone(),
            _ => None,
        };
        let message = error["message"].as_str().map(str::to_owned);
        let outcome = if let Some(limit) = limit.filter(|_| !active.stopping) {
            let message = active
                .problem
                .clone()
                .or(message)
                .unwrap_or_else(|| "You've reached your Codex usage limit for now.".into());
            Outcome::Failed(TurnError {
                kind: "limit".into(),
                message,
                status: None,
                limit: Some(limit),
            })
        } else if active.stopped_asking && !active.stopping {
            Outcome::Succeeded
        } else if active.stopping || status == "interrupted" {
            Outcome::Interrupted
        } else if status == "failed" {
            let message = message
                .or_else(|| active.problem.clone())
                .unwrap_or_else(|| "Codex stopped with an error.".into());
            Outcome::Failed(failure(message))
        } else {
            Outcome::Succeeded
        };
        // Its questions outlive the native turn that asked them: one still open is waited for, and
        // an answer no turn took starts the next.
        if matches!(outcome, Outcome::Succeeded)
            && !active.stopped_asking
            && (!self.cards.is_empty() || !self.answers.is_empty())
        {
            active.between = true;
            active.turn = None;
            *lock(&self.current) = None;
            self.flush().await?;
            return self.settle_between().await;
        }
        self.finish(outcome).await
    }

    async fn finish(&mut self, outcome: Outcome) -> io::Result<()> {
        self.deadline = None;
        for (_, card) in self.cards.drain() {
            card.task.abort();
            if let Some(active) = &self.active {
                let _ = active.sink.cancel_ask(&card.stable).await;
            }
        }
        for (_, ask) in self.asks.drain() {
            ask.task.abort();
            if let Some(active) = &self.active {
                let _ = active.sink.cancel_ask(&ask.stable).await;
            }
        }
        self.answers.clear();
        *lock(&self.current) = None;
        self.busy.store(false, Ordering::Relaxed);
        let Some(mut active) = self.active.take() else {
            return Ok(());
        };
        if let (Some(server), Some(native)) = (&self.server, &self.native)
            && !server.closed()
        {
            server.note_turn(native, None).await;
        }
        let mut events = active.lane.take();
        // What changed in its instructions was said with its message: told, once it took it.
        if let Some(told) = active.told.take()
            && (active.took_prompt || !matches!(outcome, Outcome::Failed(_)))
        {
            self.state.insert("codexTold".into(), json!(told));
            active.sink.save_state(self.state.clone()).await?;
        }
        events.push(TurnEvent::Finished(TurnResult {
            outcome,
            took_prompt: active.took_prompt,
            missed: Vec::new(),
        }));
        for event in events {
            active.sink.event(envelope(event)).await?;
        }
        Ok(())
    }

    /// A question Codex asked (its request_user_input_async), put to you as it's asked: its tool
    /// returns at once, and the answer goes into its turn, or starts the next.
    fn ask(&mut self, item: String, questions: Vec<(String, Vec<String>)>) {
        let Some(active) = &self.active else { return };
        let sink = active.sink.clone();
        let stable = format!("codex-question-{item}");
        if self.cards.contains_key(&stable) {
            return;
        }
        let fields = questions
            .iter()
            .enumerate()
            .map(|(index, (title, options))| Field {
                key: format!("q{index}"),
                title: Some("Question".into()),
                description: Some(title.clone()),
                required: true,
                kind: FieldKind::String {
                    options: options
                        .iter()
                        .map(|option| FormOption {
                            value: option.clone(),
                            label: option.clone(),
                            description: None,
                        })
                        .collect(),
                    custom: true,
                    format: None,
                },
            })
            .collect();
        let ask = QuestionAsk {
            id: stable.clone(),
            title: if questions.len() == 1 {
                "Question".into()
            } else {
                "Questions".into()
            },
            fields,
            tool_call_id: None,
            metadata: Map::new(),
        };
        let titles: Vec<String> = questions.into_iter().map(|(title, _)| title).collect();
        let task = self.replies.spawn({
            let stable = stable.clone();
            async move {
                let answer = sink.question(ask).await.map(|answer| match answer {
                    Answer::Submitted(values) if titles.len() > 1 => {
                        let mut joined = Map::new();
                        let text = titles
                            .iter()
                            .enumerate()
                            .map(|(index, title)| {
                                format!("{title}\n{}", field_text(values.get(&format!("q{index}"))))
                            })
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        joined.insert("text".into(), json!(text));
                        Answer::Submitted(joined)
                    }
                    Answer::Submitted(values) => {
                        let mut joined = Map::new();
                        joined.insert("text".into(), json!(field_text(values.get("q0"))));
                        Answer::Submitted(joined)
                    }
                    other => other,
                });
                Settled::Card(stable, answer)
            }
        });
        self.cards.insert(
            stable.clone(),
            Card {
                task: task.clone(),
                stable,
            },
        );
    }

    /// Your message, into the turn under way (Codex's turn/steer): it answers its questions too.
    async fn steer(&mut self, turn: &str, prompt: Prompt) -> io::Result<Steer> {
        let Some(active) = self.active.as_ref().filter(|active| active.id == turn) else {
            return Ok(Steer::Declined);
        };
        if active.stopping {
            return Ok(Steer::Declined);
        }
        let answers_cards = !prompt.synthetic && !self.cards.is_empty();
        // Between native turns, waiting on its questions: your message is the answer, and starts
        // the next.
        if active.between || active.turn.is_none() {
            if !answers_cards {
                return Ok(Steer::Declined);
            }
            self.close_cards().await;
            self.answers.push(Said {
                text: prompt.text,
                files: prompt.files,
                declined: false,
                message: Some(prompt.id),
            });
            if self.active.as_ref().is_some_and(|active| active.between) {
                self.settle_between().await?;
            }
            return Ok(Steer::Accepted);
        }
        let (Some(server), Some(native), Some(codex_turn)) = (
            self.server.clone(),
            self.native.clone(),
            active.turn.clone(),
        ) else {
            return Ok(Steer::Declined);
        };
        let receipt = SteerReceipt {
            thread: native.clone(),
            turn: codex_turn.clone(),
            message: prompt.id.clone(),
        };
        let params = json!({
            "threadId": native, "expectedTurnId": codex_turn, "clientUserMessageId": prompt.id,
            "input": codex_input(&prompt.text, &prompt.files),
        });
        match server
            .rpc
            .request("turn/steer", Some(params), Some(STEER))
            .await
        {
            Ok(_) => {
                if answers_cards {
                    self.close_cards().await;
                }
                Ok(Steer::Accepted)
            }
            // Request validation happens before it runs: refused, it didn't take it.
            Err(Failure::Refused(error)) if (-32602..=-32600).contains(&error.code) => {
                Ok(Steer::Declined)
            }
            Err(_) => Ok(Steer::InDoubt(receipt)),
        }
    }

    /// A message you typed answers its open questions: their cards go, and it hears your words.
    async fn close_cards(&mut self) {
        for (_, card) in self.cards.drain() {
            card.task.abort();
            if let Some(active) = &self.active {
                let _ = active.sink.cancel_ask(&card.stable).await;
            }
        }
    }

    /// Whether Codex took a steered message: its thread keeps the message's client ID.
    async fn resolve(&mut self, receipt: SteerReceipt) -> io::Result<Option<bool>> {
        let server = self.shared.server().await?;
        let answer = server
            .rpc
            .request(
                "thread/read",
                Some(json!({ "threadId": receipt.thread, "includeTurns": true })),
                Some(STOP),
            )
            .await?;
        let thread = &answer["thread"];
        if thread["id"].as_str() != Some(&receipt.thread) {
            return Ok(None);
        }
        let Some(turn) = thread["turns"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|turn| turn["id"].as_str() == Some(&receipt.turn))
        else {
            return Ok(None);
        };
        let Some(items) = turn["items"].as_array() else {
            return Ok(None);
        };
        let messages: Vec<&Value> = items
            .iter()
            .filter(|item| item["type"] == "userMessage")
            .collect();
        if messages
            .iter()
            .any(|item| item["clientId"].as_str() == Some(&receipt.message))
        {
            return Ok(Some(true));
        }
        // The tagged opener says this history keeps client IDs: an older Codex may take the ID and
        // ignore it, so its absence there proves nothing.
        let full = turn["itemsView"].is_null() || turn["itemsView"] == "full";
        let ended = matches!(
            turn["status"].as_str(),
            Some("completed" | "interrupted" | "failed")
        );
        if messages
            .first()
            .is_some_and(|first| first["clientId"].is_string())
            && full
            && ended
        {
            return Ok(Some(false));
        }
        Ok(None)
    }

    async fn stop_native(&mut self) -> io::Result<()> {
        let (Some(server), Some(native)) = (self.server.clone(), self.native.clone()) else {
            return Ok(());
        };
        if let Some(turn) = self.active.as_ref().and_then(|active| active.turn.clone()) {
            let _ = server
                .rpc
                .request(
                    "turn/interrupt",
                    Some(json!({ "threadId": native, "turnId": turn })),
                    Some(STOP),
                )
                .await;
        }
        Ok(())
    }

    async fn interrupt(&mut self, turn: &str) -> io::Result<()> {
        let Some(active) = self
            .active
            .as_mut()
            .filter(|active| active.id == turn && !active.stopping)
        else {
            return Ok(());
        };
        active.stopping = true;
        self.deadline = Some(Instant::now() + STOP);
        // Its questions go, and nothing more is sent.
        self.close_cards().await;
        self.answers.clear();
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.between || active.turn.is_none() && active.took_prompt)
        {
            return self.finish(Outcome::Interrupted).await;
        }
        self.stop_native().await
    }

    async fn expired(&mut self) -> io::Result<()> {
        self.deadline = None;
        if self.active.as_ref().is_some_and(|active| active.stopping) {
            return self.finish(Outcome::Interrupted).await;
        }
        Ok(())
    }
}

fn field_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map_or_else(|| item.to_string(), str::to_owned)
            })
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The approval policy and sandbox a turn starts with: Approve All asks so your rules can answer
/// what Codex's own rules ask about, Ask First and Plan ask before all but a look.
fn policy(plan: bool, approve_all: bool) -> (&'static str, Value) {
    if plan {
        return (
            "untrusted",
            json!({ "type": "readOnly", "networkAccess": true }),
        );
    }
    if approve_all {
        return ("on-request", json!({ "type": "dangerFullAccess" }));
    }
    // Ask First: what it runs can write in the project and where Kuvo Code keeps wake-ups.
    let roots: Vec<String> = ["KUVO_WAKES", "KUVO_BOTS"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok().filter(|root| !root.is_empty()))
        .collect();
    (
        "untrusted",
        json!({ "type": "workspaceWrite", "writableRoots": roots, "networkAccess": true }),
    )
}

/// Your message as Codex takes it: its words, its pictures, and the paths of other files.
fn codex_input(text: &str, files: &[PromptFile]) -> Vec<Value> {
    let mut words = text.to_owned();
    let mut items = Vec::new();
    let mut others = Vec::new();
    for file in files {
        let image = file.mime.starts_with("image/");
        match (&file.path, &file.data) {
            (Some(path), _) if image => items.push(json!({ "type": "localImage", "path": path })),
            (None, Some(data)) if image => items.push(
                json!({ "type": "image", "url": format!("data:{};base64,{data}", file.mime) }),
            ),
            (Some(path), _) => others.push(path.to_string_lossy().into_owned()),
            _ => {}
        }
    }
    if !others.is_empty() {
        words.push_str("\n\nAttached files:\n");
        words.push_str(
            &others
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    std::iter::once(json!({ "type": "text", "text": words, "text_elements": [] }))
        .chain(items)
        .collect()
}
