//! The resident's live coding-agent sessions: their turns, and the line of
//! messages waiting for each.

use crate::{
    accounts::Provider,
    inbox::{Delivery, InboxEvent, Input},
    live_asks::{Ask, AskKey, LiveAsks, Reply as AskReply},
    permissions::{
        Allowance, ConfigLocation, Policy, covers, remember,
        turn::{Settings, TurnRules},
    },
    resident::ResidentAgents,
    runtime::AdoptedTurn,
    session::{Decision, Outcome, PermissionMode, Prompt, Thread, TurnInput, TurnKind},
    session_store::SessionStore,
    turn_projection::{Projection, record_failed_start},
    turn_runtime::RunningTurn,
    turn_sink::ResidentSink,
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, ErrorKind},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinSet,
};

mod line;
mod update;

pub use update::{HandedTurn, Handover, Update};

type Reply<T> = oneshot::Sender<io::Result<T>>;

/// A summary asked for: under way now, or once the turn under way ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Compacting {
    Now,
    Waiting,
}

impl Compacting {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Now => "compacting",
            Self::Waiting => "waiting",
        }
    }
}

/// What became of a message taken back from the line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cancelled {
    Removed,
    /// It isn't waiting any more.
    Missing,
    /// It's being handed to the agent, or Kuvo is checking whether it took it.
    Busy,
}

enum Command {
    Start(Box<Start>),
    Find {
        session: String,
        reply: Reply<Option<Live>>,
    },
    /// Refuse new turns while the conversation is deleted, and hand back the
    /// turn to stop first.
    Retire {
        session: String,
        reply: Reply<Option<Live>>,
    },
    Release {
        session: String,
    },
    Line(line::Request),
    Revert {
        session: String,
        message: Option<String>,
        reply: Reply<Value>,
    },
    /// A message handed to a turn: what its agent said.
    Steered(Box<line::Steered>),
    StepBoundary {
        session: String,
        turn: String,
    },
    /// Uncertain handoffs looked at again.
    Reconciled {
        session: String,
        retry: bool,
        remaining: bool,
    },
    HandOver {
        held: BTreeSet<String>,
        reply: Reply<Handover>,
    },
}

struct Start {
    session: String,
    input: TurnInput,
    policy: Policy,
    note: Option<Map<String, Value>>,
    /// The message as it came from the line, its row's words: one whose agent
    /// can't start is still kept in the conversation, failed.
    shown: Option<Prompt>,
    reply: Option<Reply<RunningTurn>>,
}

#[derive(Clone)]
struct Live {
    turn: RunningTurn,
    id: String,
    rules: Arc<TurnRules>,
    thread: Arc<dyn Thread>,
    provider: String,
    /// A summary you asked for, not a turn of its own work.
    compacting: bool,
}

/// What a conversation is doing now.
#[derive(Default)]
enum Doing {
    #[default]
    Idle,
    /// Its agent is getting ready for message `id`. `stop` once asked to
    /// stop meanwhile.
    Starting {
        stop: bool,
        id: String,
    },
    Running(Live),
    Stopping(Live),
    Files,
}

impl Doing {
    fn live(&self) -> Option<&Live> {
        match self {
            Self::Running(live) | Self::Stopping(live) => Some(live),
            _ => None,
        }
    }
}

/// The resident's state of one conversation, beside what's saved of it.
#[derive(Default)]
struct Conversation {
    doing: Doing,
    /// A Send Now or Resume waiting for the conversation to be free.
    go: bool,
    /// Its agent's process ended under the last turn: what waits goes on, in
    /// order, once uncertain handoffs are settled.
    continue_after_exit: bool,
    /// Messages being handed to the running turn, and whether each was
    /// taken back meanwhile.
    handing: BTreeMap<String, Arc<AtomicBool>>,
    retired: bool,
    waiting_files: Vec<Command>,
    checking: bool,
    /// A next-step delivery may be waiting; idle step notifications do not read disk.
    step_waiting: bool,
    step_seen_while_starting: bool,
    /// Native steers whose confirmed refusal should use Send Now's fallback.
    immediate: BTreeSet<String>,
    /// The turn an earlier resident left under way, being taken up here.
    recovered: Option<AdoptedTurn>,
    /// A turn its agent began on its own, taken: it starts before what waits in line.
    own: Option<line::OwnStart>,
}

/// How long a turn asked to stop is waited for before its conversation goes
/// without it.
const STOP_GRACE: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct SessionRuns {
    stop_grace: Duration,
    asks: LiveAsks,
    store: SessionStore,
    commands: mpsc::Sender<Command>,
    running: watch::Receiver<Arc<BTreeSet<String>>>,
    parents: watch::Receiver<Arc<BTreeSet<String>>>,
    events: broadcast::Sender<InboxEvent>,
    restoring: Arc<update::Restorings>,
}

impl SessionRuns {
    pub fn start(
        agents: ResidentAgents,
        store: SessionStore,
        home: PathBuf,
        location: ConfigLocation,
    ) -> Self {
        Self::start_after(agents, store, home, location, Update::default())
    }

    /// Start after an update: the turns the earlier resident's agents went
    /// on with are taken up before anything else starts.
    pub fn start_after(
        agents: ResidentAgents,
        store: SessionStore,
        home: PathBuf,
        location: ConfigLocation,
        update: Update,
    ) -> Self {
        Self::start_with_limits(
            agents,
            store,
            home,
            location,
            update,
            Arc::new(crate::turn_projection::Unheard),
        )
    }

    /// The resident's usage service receives the plan readings from every live turn.
    pub fn start_with_limits(
        agents: ResidentAgents,
        store: SessionStore,
        home: PathBuf,
        location: ConfigLocation,
        update: Update,
        limits: Arc<dyn crate::PlanLimits>,
    ) -> Self {
        let (commands, requests) = mpsc::channel(32);
        // Turns agents begin on their own come here, as line requests.
        let (offers, mut offered) = mpsc::unbounded_channel::<crate::session::OwnTurn>();
        let store = store.with_own_turns(crate::session::OwnTurns::new(offers));
        let line = commands.downgrade();
        tokio::spawn(async move {
            while let Some(offer) = offered.recv().await {
                let Some(commands) = line.upgrade() else {
                    return;
                };
                let (reply, answer) = oneshot::channel();
                let request = line::Request::OwnTurn {
                    session: offer.session,
                    provider: offer.provider,
                    text: offer.text,
                    reply,
                };
                let sent = commands.send(Command::Line(request)).await.is_ok();
                drop(commands);
                let taken = sent && answer.await.ok().and_then(Result::ok).unwrap_or(false);
                let _ = offer.taken.send(taken);
            }
        });
        let (identities, parent_watch) = watch::channel(Arc::new(BTreeSet::new()));
        let mut parents = parent_watch.clone();
        let (active, running) = watch::channel(Arc::new(BTreeSet::new()));
        let background = store.background().clone();
        let mut changes = background.changes();
        tokio::spawn(async move {
            loop {
                let mut all = (**parents.borrow_and_update()).clone();
                all.extend(background.child_ids());
                active.send_if_modified(|old| {
                    if **old == all { false } else { *old = Arc::new(all); true }
                });
                tokio::select! {
                    result = parents.changed() => if result.is_err() { break; },
                    result = changes.changed() => if result.is_err() { break; },
                    () = active.closed() => break,
                }
            }
        });
        let (events, _) = broadcast::channel(256);
        let asks = LiveAsks::start();
        let restoring = Arc::new(watch::channel(update::expected_asks(&update)).0);
        let service = Service {
            limits,
            restoring: restoring.clone(),
            agents,
            store: store.clone(),
            asks: asks.clone(),
            identities,
            settings: Settings { home, location },
            events: events.clone(),
            commands: commands.downgrade(),
        };
        tokio::spawn(service.run(requests, update));
        Self {
            stop_grace: STOP_GRACE,
            asks,
            store,
            commands,
            running,
            parents: parent_watch,
            events,
            restoring,
        }
    }

    /// Make way for the next resident: start nothing more, keep what waits
    /// behind turns that won't go on, and say what goes on. `held`: the
    /// conversations whose turns go on in held agents.
    pub async fn hand_over(&self, held: BTreeSet<String>) -> io::Result<Handover> {
        self.call(|reply| Command::HandOver { held, reply }).await
    }

    /// Once the approvals and questions a turn taken up after an update was
    /// waiting on are shown again (or have had their time): a client's read
    /// of them then shows them all.
    pub async fn asks_restored(&self, session: &str) {
        update::restored(&self.restoring, session).await;
    }

    /// Wait this long for a turn asked to stop, rather than the usual eight
    /// seconds.
    pub fn with_stop_grace(mut self, grace: Duration) -> Self {
        self.stop_grace = grace;
        self
    }

    pub fn asks(&self) -> &LiveAsks {
        &self.asks
    }

    /// The line's changes, as clients are told them.
    pub fn events(&self) -> broadcast::Receiver<InboxEvent> {
        self.events.subscribe()
    }

    pub async fn reply_permission(
        &self,
        key: AskKey,
        answer: Decision,
        cancelled: impl Fn() -> bool,
    ) -> io::Result<()> {
        if cancelled() {
            return Err(ErrorKind::Interrupted.into());
        }
        if answer == Decision::Reject {
            return self.asks.reply(key, AskReply::Permission(answer)).await;
        }
        let ask = {
            let pending = self.asks.pending();
            let current = pending.borrow();
            match current.get(&key) {
                Some(Ask::Permission(ask)) => ask.clone(),
                _ => return Err(ErrorKind::NotFound.into()),
            }
        };
        let live = self
            .find(key.session.clone())
            .await?
            .filter(|live| live.id == key.turn)
            .ok_or(ErrorKind::NotFound)?;
        live.rules.verify_reply(&ask).await?;
        if cancelled() {
            return Err(ErrorKind::Interrupted.into());
        }
        self.asks
            .reply(key.clone(), AskReply::Permission(answer))
            .await?;
        if answer != Decision::Always {
            return Ok(());
        }
        let saved = remember(&ask);
        live.rules.remembered.send_if_modified(|allowed| {
            if allowed.contains(&saved) {
                return false;
            }
            allowed.push(saved.clone());
            true
        });
        let others = {
            let watched = self.asks.pending();
            let pending = watched.borrow();
            pending
                .iter()
                .filter_map(|(other, ask)| match ask {
                    Ask::Permission(ask)
                        if other.session == key.session
                            && other.turn == key.turn
                            && covers(&saved, ask) =>
                    {
                        Some((other.clone(), ask.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        for (other, ask) in others {
            if live.rules.verify_reply(&ask).await.is_ok() {
                let _ = self
                    .asks
                    .reply(other, AskReply::Permission(Decision::Once))
                    .await;
            }
        }
        Ok(())
    }

    pub fn running(&self) -> Arc<BTreeSet<String>> {
        let parents = self.parents.borrow();
        let children = self.store.background().child_ids();
        if children.is_empty() { parents.clone() } else {
            let mut all = (**parents).clone();
            all.extend(children);
            Arc::new(all)
        }
    }

    pub fn changes(&self) -> watch::Receiver<Arc<BTreeSet<String>>> {
        let mut watch = self.running.clone();
        watch.mark_unchanged();
        watch
    }

    /// Start a turn directly, outside the line (a recovered turn).
    pub async fn run_turn(
        &self,
        session: String,
        input: TurnInput,
        policy: Policy,
    ) -> io::Result<RunningTurn> {
        self.call(|reply| {
            Command::Start(Box::new(Start {
                session,
                input,
                policy,
                note: None,
                shown: None,
                reply: Some(reply),
            }))
        })
        .await
    }

    /// Put a message in the conversation's line: into the turn under way
    /// where its agent takes one partway, now when nothing runs, or after.
    /// Sent twice, it goes once. `handoff` is the box's token for this
    /// attempt; one whose handoff was cancelled is refused.
    pub async fn prompt(
        &self,
        session: String,
        input: Input,
        handoff: Option<String>,
    ) -> io::Result<()> {
        self.line(|reply| line::Request::Prompt {
            session,
            input: Box::new(input),
            handoff,
            reply,
        })
        .await
    }

    /// Whether the conversation's transcript has this message.
    pub async fn has_message(&self, session: &str, id: &str) -> io::Result<bool> {
        self.store.has_message(session, id).await
    }

    /// The conversation's last `limit` rows, newest first.
    pub async fn recent(&self, session: &str, limit: u32) -> io::Result<Vec<Value>> {
        Ok(self
            .store
            .message_page(
                session,
                None,
                limit,
                crate::session_store::MessageOrder::NewestFirst,
            )
            .await?
            .map(|page| page.messages)
            .unwrap_or_default())
    }

    /// What waits in the conversation's line, as clients list it.
    pub async fn inbox(&self, session: &str) -> io::Result<Option<Vec<Input>>> {
        self.store.line(session).await
    }

    /// Steer (it goes next, and now if nothing runs) or queue a waiting
    /// message. `into`: steered into the turn under way, where its agent
    /// takes one partway.
    pub async fn set_delivery(
        &self,
        session: String,
        id: String,
        delivery: Delivery,
        into: bool,
    ) -> io::Result<()> {
        self.line(|reply| line::Request::SetDelivery {
            session,
            id,
            delivery,
            into,
            now: false,
            reply,
        })
        .await
    }

    /// Shared queue delivery: native steering, or interrupt/resume for Send Now.
    pub async fn advance_line(&self, session: String, id: String, now: bool) -> io::Result<()> {
        self.line(|reply| line::Request::SetDelivery {
            session,
            id,
            delivery: Delivery::Steer,
            into: true,
            now,
            reply,
        })
        .await
    }

    /// Take a waiting message back. `defer`: one being handed to the agent
    /// is taken back if the agent doesn't take it.
    pub async fn cancel_input(
        &self,
        session: String,
        id: String,
        defer: bool,
    ) -> io::Result<Cancelled> {
        self.line(|reply| line::Request::Cancel {
            session,
            id,
            defer,
            reply,
        })
        .await
    }

    /// Whether the conversation has this message, waiting or sent, written
    /// to disk before answering.
    pub async fn confirm_prompt(&self, session: String, id: String) -> io::Result<bool> {
        self.line(|reply| line::Request::Confirm { session, id, reply })
            .await
    }

    /// Keep a late arrival of this attempt from landing after a Delete. False
    /// when the conversation already has the message.
    pub async fn cancel_handoff(
        &self,
        session: String,
        id: String,
        token: String,
    ) -> io::Result<bool> {
        self.line(|reply| line::Request::CancelHandoff {
            session,
            id,
            token,
            reply,
        })
        .await
    }

    /// The conversation's mode from its next message on.
    /// The conversation's folder.
    pub async fn directory(&self, session: &str) -> Option<String> {
        let header = self.store.read(session).await.ok()??;
        header.directory().map(str::to_owned)
    }

    /// Puts the conversation on `model` from its next turn: one of an agent connected here.
    /// `None` when the conversation isn't here; otherwise the model it was on, when it changed.
    pub async fn select_model(
        &self,
        session: &str,
        model: Value,
        agents: &ResidentAgents,
    ) -> io::Result<Option<Option<Value>>> {
        let connected = model["providerID"]
            .as_str()
            .and_then(Provider::of_model)
            .is_some_and(|provider| agents.roster().find(&provider).is_some());
        self.store.select_model(session, model, connected).await
    }

    pub async fn select_agent(
        &self,
        session: &str,
        agent: &str,
    ) -> io::Result<Option<Option<Value>>> {
        self.store.select_agent(session, agent).await
    }

    /// Summarize the conversation with the agent that holds it: now, or once
    /// the turn under way ends. Refused, in a sentence, when it can't be.
    pub async fn compact(&self, session: String) -> io::Result<Compacting> {
        self.line(|reply| line::Request::Compact { session, reply })
            .await
    }

    pub async fn interrupt(&self, session: String) -> io::Result<bool> {
        self.interrupt_and(session, false).await
    }

    /// Stop the turn under way. `resume`: what you steered goes next (Send
    /// Now); with nothing running, it starts now.
    pub async fn interrupt_and(&self, session: String, resume: bool) -> io::Result<bool> {
        if self.store.background().stop_child(&session) { return Ok(true); }
        let stopping = self
            .line(|reply| line::Request::Interrupt {
                session,
                resume,
                reply,
            })
            .await?;
        match stopping {
            line::Stopping::Turn(turn) => {
                turn.interrupt().await?;
                Ok(true)
            }
            line::Stopping::Starting => Ok(true),
            line::Stopping::Nothing => Ok(false),
        }
    }

    pub async fn stage_revert(&self, session: String, message: String) -> io::Result<Value> {
        self.call(|reply| Command::Revert { session, message: Some(message), reply }).await
    }

    pub async fn clear_revert(&self, session: String) -> io::Result<()> {
        self.call(|reply| Command::Revert { session, message: None, reply }).await.map(|_| ())
    }

    /// Delete a conversation as the bridge does: no turn starts meanwhile, the
    /// running one stops first, and its subagents' conversations go with it
    /// unless a fork's steps still show them. False when it wasn't here.
    pub async fn delete(&self, session: String) -> io::Result<bool> {
        Box::pin(self.delete_retired(session)).await
    }

    async fn delete_retired(&self, session: String) -> io::Result<bool> {
        let mut changed = self.store.background().changes();
        if self.store.background().stop_child(&session) {
            let stopped = async {
                while self.store.background().child_ids().contains(&session) {
                    if changed.changed().await.is_err() { break; }
                }
            };
            let _ = tokio::time::timeout(self.stop_grace, stopped).await;
        }
        let live = self
            .call(|reply| Command::Retire {
                session: session.clone(),
                reply,
            })
            .await?;
        let deleted = async {
            if let Some(live) = live {
                // One that doesn't stop is given up on: what it says after
                // can't bring the conversation back.
                let stopping = async {
                    let _ = live.turn.interrupt().await;
                    let _ = live.turn.finished().await;
                };
                let _ = tokio::time::timeout(self.stop_grace, stopping).await;
            }
            let headers = self.store.headers().await?;
            let children = headers
                .iter()
                .filter(|header| header.parent() == Some(session.as_str()))
                .map(|header| header.id().to_owned())
                .collect::<Vec<_>>();
            for child in children {
                let mut shown = false;
                for other in &headers {
                    if other.id() != session
                        && other.id() != child
                        && self.store.shows_subagent(other.id(), &child).await?
                    {
                        shown = true;
                        break;
                    }
                }
                if !shown {
                    self.delete(child).await?;
                }
            }
            self.store.delete(&session).await
        }
        .await;
        let _ = self.commands.send(Command::Release { session }).await;
        deleted
    }

    async fn find(&self, session: String) -> io::Result<Option<Live>> {
        self.call(|reply| Command::Find { session, reply }).await
    }

    async fn line<T>(&self, request: impl FnOnce(Reply<T>) -> line::Request) -> io::Result<T> {
        self.call(|reply| Command::Line(request(reply))).await
    }

    async fn call<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> io::Result<T> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(command(reply))
            .await
            .map_err(|_| stopped())?;
        response.await.map_err(|_| stopped())?
    }
}

#[derive(Clone)]
struct Service {
    limits: Arc<dyn crate::PlanLimits>,
    agents: ResidentAgents,
    store: SessionStore,
    asks: LiveAsks,
    identities: watch::Sender<Arc<BTreeSet<String>>>,
    settings: Settings,
    events: broadcast::Sender<InboxEvent>,
    commands: mpsc::WeakSender<Command>,
    restoring: Arc<update::Restorings>,
}

type Prepared = (String, io::Result<Live>, Option<Reply<RunningTurn>>);
type KeptFiles = (String, bool, io::Result<Value>, Reply<Value>);

/// The actor's state: every conversation it knows of now, and the tasks it
/// waits on.
struct State {
    conversations: BTreeMap<String, Conversation>,
    approvals: BTreeMap<String, watch::Sender<Vec<Allowance>>>,
    preparations: JoinSet<Prepared>,
    files: JoinSet<KeptFiles>,
    handover_waiting: Vec<(BTreeSet<String>, Reply<Handover>)>,
    turns: JoinSet<(String, io::Result<Option<Outcome>>)>,
    /// Making way for the next resident: nothing more starts here.
    closing: bool,
}

impl State {
    fn conversation(&mut self, session: &str) -> &mut Conversation {
        self.conversations.entry(session.to_owned()).or_default()
    }
}

impl Service {
    async fn run(self, mut commands: mpsc::Receiver<Command>, update: Update) {
        let mut state = State {
            conversations: BTreeMap::new(),
            approvals: BTreeMap::new(),
            preparations: JoinSet::new(),
            files: JoinSet::new(),
            handover_waiting: Vec::new(),
            turns: JoinSet::new(),
            closing: false,
        };
        let resumed = self.take_up(&mut state, update).await;
        self.wake_saved(&mut state, &resumed).await;
        loop {
            tokio::select! {
                next = commands.recv() => {
                    let Some(command) = next else { break };
                    self.handle(&mut state, command).await;
                }
                prepared = state.preparations.join_next(), if !state.preparations.is_empty() => {
                    if let Some(Ok(prepared)) = prepared {
                        self.prepared(&mut state, prepared).await;
                    }
                }
                files = state.files.join_next(), if !state.files.is_empty() => {
                    if let Some(Ok((session, staged, result, reply))) = files {
                        self.files_ended(&mut state, session, staged, result, reply).await;
                    }
                }
                ended = state.turns.join_next(), if !state.turns.is_empty() => {
                    if let Some(Ok((session, result))) = ended {
                        self.turn_ended(&mut state, &session, result).await;
                    }
                }
            }
        }
    }

    async fn handle(&self, state: &mut State, command: Command) {
        match command {
            Command::Start(request) => {
                let Start {
                    session,
                    input,
                    policy,
                    note,
                    shown,
                    reply,
                } = *request;
                let closing = state.closing;
                let conversation = state.conversation(&session);
                let refused = if closing {
                    Some(stopped())
                } else if conversation.retired {
                    Some(io::Error::new(
                        ErrorKind::NotFound,
                        "That conversation is being deleted.",
                    ))
                } else if !matches!(conversation.doing, Doing::Idle) {
                    Some(io::Error::new(
                        ErrorKind::AlreadyExists,
                        "That conversation is already working.",
                    ))
                } else {
                    None
                };
                if let Some(error) = refused {
                    if let Some(reply) = reply {
                        let _ = reply.send(Err(error));
                    }
                    return;
                }
                conversation.doing = Doing::Starting {
                    stop: false,
                    id: input.id.clone(),
                };
                self.spawn_start(state, session, input, policy, note, shown, reply);
            }
            Command::Find { session, reply } => {
                let live = state
                    .conversations
                    .get(&session)
                    .and_then(|conversation| conversation.doing.live())
                    .cloned();
                let _ = reply.send(Ok(live));
            }
            Command::Retire { session, reply } => {
                if matches!(state.conversation(&session).doing, Doing::Files) {
                    state.conversation(&session).waiting_files.push(Command::Retire { session, reply });
                    return;
                }
                state.approvals.remove(&session);
                let conversation = state.conversation(&session);
                conversation.retired = true;
                let _ = reply.send(Ok(conversation.doing.live().cloned()));
            }
            Command::Release { session } => {
                if let Some(conversation) = state.conversations.get_mut(&session) {
                    conversation.retired = false;
                    if matches!(conversation.doing, Doing::Idle) {
                        state.conversations.remove(&session);
                    }
                }
            }
            Command::Revert { session, message, reply } => {
                let closing = state.closing;
                let conversation = state.conversation(&session);
                if closing || conversation.retired {
                    let _ = reply.send(Err(io::ErrorKind::NotFound.into()));
                } else if matches!(conversation.doing, Doing::Files)
                    || conversation.doing.live().is_some_and(|live| live.turn.is_done())
                {
                    conversation.waiting_files.push(Command::Revert { session, message, reply });
                } else if !matches!(conversation.doing, Doing::Idle) {
                    let _ = reply.send(Err(io::Error::new(ErrorKind::InvalidInput, "Stop the agent first.")));
                } else {
                    conversation.doing = Doing::Files;
                    let store = self.store.clone();
                    state.files.spawn(async move {
                        let staged = message.is_some();
                        let result = if let Some(message) = message {
                            store.stage_revert(&session, &message).await
                        } else {
                            store.clear_revert(&session).await.map(|_| serde_json::json!({}))
                        };
                        (session, staged, result, reply)
                    });
                }
            }
            Command::Line(request) => self.line_request(state, request).await,
            Command::Steered(steered) => self.steered(state, *steered).await,
            Command::StepBoundary { session, turn } => {
                self.step_boundary(state, &session, &turn).await
            }
            Command::Reconciled {
                session,
                retry,
                remaining,
            } => self.reconciled(state, &session, retry, remaining).await,
            Command::HandOver { held, reply } => {
                if state.files.is_empty() {
                    let _ = reply.send(Ok(self.hand_over(state, &held).await));
                } else {
                    // An Undo or Redo already changing files finishes before replacement.
                    state.closing = true;
                    state.handover_waiting.push((held, reply));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_start(
        &self,
        state: &mut State,
        session: String,
        input: TurnInput,
        policy: Policy,
        note: Option<Map<String, Value>>,
        shown: Option<Prompt>,
        reply: Option<Reply<RunningTurn>>,
    ) {
        let remembered = state
            .approvals
            .entry(session.clone())
            .or_insert_with(|| watch::channel(Vec::new()).0)
            .clone();
        let service = self.clone();
        state.preparations.spawn(async move {
            let outcome = service
                .begin(&session, input, policy, remembered, note, shown)
                .await;
            (session, outcome, reply)
        });
    }

    async fn prepared(&self, state: &mut State, prepared: Prepared) {
        let (session, mut outcome, reply) = prepared;
        let selected = match &state.conversation(&session).doing {
            Doing::Starting { id, .. } => Some(id.clone()),
            _ => None,
        };
        if outcome.is_err() && let Some(id) = selected {
            let written = self.store.has_message(&session, &id).await.unwrap_or(false);
            let _ = self.store.edit_line(&session, move |line| {
                if written { return ((), line.take(&id).is_some()); }
                if let Some(input) = line.inputs.iter_mut().find(|input| input.id == id) {
                    input.held = true;
                    return ((), true);
                }
                ((), false)
            }).await;
        }
        let conversation = state.conversation(&session);
        let stop = matches!(conversation.doing, Doing::Starting { stop: true, .. });
        if conversation.retired
            && let Ok(live) = &outcome
        {
            // Deleted while it started: stop it rather than run it.
            let turn = live.turn.clone();
            tokio::spawn(async move {
                let _ = turn.interrupt().await;
            });
            outcome = Err(io::Error::new(
                ErrorKind::NotFound,
                "That conversation is being deleted.",
            ));
        }
        match &outcome {
            Ok(live) => {
                let turn = live.turn.clone();
                conversation.doing = if stop {
                    let stopping = turn.clone();
                    tokio::spawn(async move {
                        let _ = stopping.interrupt().await;
                    });
                    Doing::Stopping(live.clone())
                } else {
                    Doing::Running(live.clone())
                };
                let ended = session.clone();
                state
                    .turns
                    .spawn(async move { (ended, turn.finished().await) });
                self.publish(state);
                let _ = self.events.send(InboxEvent::Started {
                    session: session.clone(),
                });
                self.started_delivery(state, &session).await;
            }
            Err(_) => {
                // A turn that never started ends as a failed one.
                conversation.doing = Doing::Idle;
                if let Some(recovered) = conversation.recovered.take() {
                    update::stopped_restoring(&self.restoring, &session);
                    self.let_go(recovered);
                }
                let conversation = state.conversation(&session);
                if !conversation.retired {
                    self.advance(state, &session, None).await;
                    self.emit_idle(state, &session);
                }
            }
        }
        if let Some(reply) = reply {
            let _ = reply.send(outcome.map(|live| live.turn));
        }
    }

    async fn files_ended(&self, state: &mut State, session: String, staged: bool, result: io::Result<Value>, reply: Reply<Value>) {
        let conversation = state.conversation(&session);
        conversation.doing = Doing::Idle;
        let waiting = std::mem::take(&mut conversation.waiting_files);
        if let Ok(value) = &result {
            let directory = self.store.read(&session).await.ok().flatten().and_then(|header| header.directory().map(str::to_owned));
            let (kind, data) = if staged {
                ("session.revert.staged", serde_json::json!({"sessionID":session,"revert":value}))
            } else {
                ("session.revert.cleared", serde_json::json!({"sessionID":session}))
            };
            self.agents.feed().emit_from(kind, data, directory);
        }
        let _ = reply.send(result);
        for command in waiting { self.handle(state, command).await; }
        if state.files.is_empty() {
            for (held, reply) in std::mem::take(&mut state.handover_waiting) {
                let _ = reply.send(Ok(self.hand_over(state, &held).await));
            }
        }
        self.advance(state, &session, None).await;
    }

    async fn turn_ended(
        &self,
        state: &mut State,
        session: &str,
        result: io::Result<Option<Outcome>>,
    ) {
        let conversation = state.conversation(session);
        let stopped = matches!(conversation.doing, Doing::Stopping(_));
        let resuming = stopped && conversation.go;
        conversation.doing = Doing::Idle;
        if let Some(recovered) = conversation.recovered.take() {
            update::stopped_restoring(&self.restoring, session);
            if result.is_err() {
                self.let_go(recovered);
            }
        }
        self.publish(state);
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                eprintln!("Kuvo could not finish coding-agent session {session}: {error}");
                None
            }
        };
        let _ = self.events.send(InboxEvent::Ended {
            session: session.to_owned(),
            outcome: outcome.clone(),
            stopped,
            resuming,
        });
        let exited = !stopped && matches!(outcome, Some(Outcome::AgentExited(_)));
        let succeeded = !stopped && matches!(outcome, Some(Outcome::Succeeded));
        let conversation = state.conversation(session);
        if exited {
            conversation.continue_after_exit = true;
        }
        if conversation.retired {
            return;
        }
        for command in std::mem::take(&mut state.conversation(session).waiting_files) {
            self.handle(state, command).await;
        }
        self.advance(state, session, Some(succeeded)).await;
        self.emit_idle(state, session);
    }

    fn emit_idle(&self, state: &mut State, session: &str) {
        if matches!(state.conversation(session).doing, Doing::Idle) {
            let _ = self.events.send(InboxEvent::Idle {
                session: session.to_owned(),
            });
        }
    }

    fn publish(&self, state: &State) {
        let running = state
            .conversations
            .iter()
            .filter(|(_, conversation)| conversation.doing.live().is_some())
            .map(|(session, _)| session.clone())
            .collect();
        self.identities.send_replace(Arc::new(running));
    }

    /// The agent's name, when its account is installed and signed out of: as it was last known,
    /// and asked anew (signed in a moment ago, in Terminal say, it goes). At most 10 s each.
    async fn signed_out(&self, provider: &str) -> Option<&'static str> {
        let parsed = Provider::of_model(provider)?;
        let roster = self.agents.roster();
        let entry = roster.find(&parsed)?.clone();
        let out = |status: Option<Result<crate::account_check::AccountStatus, _>>| matches!(status, Some(Ok(status)) if status.installed && !status.signed_in && !status.unchecked);
        let within = Duration::from_secs(10);
        if !out(tokio::time::timeout(within, entry.status(false)).await.ok()) {
            return None;
        }
        out(tokio::time::timeout(within, entry.status(true)).await.ok())
            .then(|| parsed.kind.display_name())
    }

    async fn thread(&self, session: &str, provider: &str) -> io::Result<Arc<dyn Thread>> {
        let parsed = Provider::of_model(provider).ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidInput, "Choose a coding-agent model.")
        })?;
        let roster = self.agents.roster();
        let entry = roster.find(&parsed).ok_or_else(|| {
            io::Error::new(ErrorKind::NotFound, "That coding-agent account isn't here.")
        })?;
        let context = self
            .store
            .context(session, provider)
            .await?
            .ok_or_else(|| {
                io::Error::new(ErrorKind::NotFound, "That coding-agent session isn't here.")
            })?;
        entry.agent.open_thread(context).await
    }

    async fn begin(
        &self,
        session: &str,
        mut input: TurnInput,
        mut policy: Policy,
        remembered: watch::Sender<Vec<Allowance>>,
        note: Option<Map<String, Value>>,
        shown: Option<Prompt>,
    ) -> io::Result<Live> {
        let agent = if input.plan { "plan" } else { "build" };
        let header = self.store.read(session).await?.ok_or(ErrorKind::NotFound)?;
        let directory = header.directory().map(PathBuf::from).ok_or(ErrorKind::NotFound)?;
        if matches!(&input.kind, TurnKind::Prompt(prompt) if !prompt.synthetic)
            && header.into_fields().get("revert").is_some_and(|value| !value.is_null())
        {
            // Native histories don't support removing an arbitrary suffix.
            // Close the native threads before starting a fresh one from the kept rows.
            let roster = self.agents.roster();
            for entry in roster.all() { entry.agent.close_thread(session).await?; }
            if let Some(cut) = self.store.commit_revert(session).await? {
                self.agents.feed().emit_from("session.revert.committed", serde_json::json!({"sessionID":session,"to":cut}), Some(directory.to_string_lossy().into_owned()));
            }
        }
        // Its account signed out of: said at once, rather than fail only after its agent started
        // and tried. (A turn taken up goes on where it is.)
        if !matches!(input.kind, TurnKind::Recover { .. })
            && let Some(name) = self.signed_out(&input.model.provider).await
        {
            // (Each computer signs in on its own: the one it runs on is named.)
            let computer = std::env::var("KUVO_MACHINE_NAME")
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "this machine".into());
            let message = format!(
                "{name} isn't signed in on {computer}, where this conversation runs: each machine signs in on its own."
            );
            if shown.is_some() {
                let error = crate::session::TurnError {
                    kind: "signin".into(),
                    message: message.clone(),
                    status: None,
                    limit: None,
                };
                let _ = crate::turn_projection::record_turned_away(
                    self.store.clone(),
                    session.into(),
                    agent.into(),
                    &input,
                    shown.as_ref(),
                    note,
                    error,
                )
                .await;
            }
            return Err(io::Error::new(ErrorKind::PermissionDenied, message));
        }
        let thread = match self.thread(session, &input.model.provider).await {
            Ok(thread) => thread,
            Err(error) => {
                if shown.is_some() || matches!(input.kind, TurnKind::Recover { .. }) {
                    let _ = record_failed_start(
                        self.store.clone(),
                        session.into(),
                        agent.into(),
                        &input,
                        shown.as_ref(),
                        note,
                        &error,
                    )
                    .await;
                }
                return Err(error);
            }
        };
        let cwd = directory.to_string_lossy().into_owned();
        policy.directory = directory.clone();
        policy.own_folders.extend([
            self.settings.home.join("agents"),
            self.settings.home.join("tmp"),
        ]);
        let policy = Arc::new(TurnRules {
            settings: self.settings.clone(),
            current: policy,
            plan: input.plan,
            thread: thread.clone(),
            remembered,
            store: self.store.clone(),
            session: session.into(),
        });
        // As the reference's sessions say (`approvesAll`): from its rules and Blocks.
        input.permissions = if policy.approves_all().await {
            PermissionMode::ApproveAll
        } else {
            PermissionMode::AskFirst
        };
        let writer = Projection::start(
            self.store.clone(),
            session.into(),
            agent.into(),
            &input,
            shown.as_ref(),
            note,
        )
        .await?
        .with_limits(self.limits.clone());
        // The user row now durably confirms the handoff. Until that write,
        // the selected input remains in the saved line. The agent hasn't
        // received the prompt, so the repository capture is still before it.
        if matches!(input.kind, TurnKind::Prompt(_) | TurnKind::Compact) {
            let id = input.id.clone();
            self.store.edit_line(session, move |line| {
                let removed = line.take(&id).is_some();
                ((), removed)
            }).await?;
        }
        let snapshot = match &input.kind {
            TurnKind::Prompt(prompt) if !prompt.synthetic => match self.store.snapshot_start(session, &input.id, &directory).await {
                Ok(pending) => pending,
                Err(error) => {
                    eprintln!("Kuvo could not note the repository snapshot: {error}");
                    false
                }
            },
            TurnKind::Recover { .. } => true,
            _ => false,
        };
        let writer = writer.with_snapshot(snapshot);
        let asks = self.asks.for_turn(session.into(), input.id.clone());
        let provider = input.model.provider.clone();
        let (sink, writes) = ResidentSink::channel(
            session.into(),
            provider.clone(),
            self.store.clone(),
            Some(self.agents.roster()),
            asks,
            Some(self.agents.feed().clone()),
            policy.clone(),
        );
        let id = input.id.clone();
        let compacting = matches!(input.kind, TurnKind::Compact);
        // Stop on a command it waits on stops the turn, as Stop does.
        let (commands, stopped) = (self.commands.clone(), session.to_owned());
        let stop: crate::foreground::StopTurn = Arc::new(move || {
            let (commands, session) = (commands.clone(), stopped.clone());
            Box::pin(async move {
                let Some(commands) = commands.upgrade() else {
                    return;
                };
                let (reply, stopping) = oneshot::channel();
                let request = line::Request::Interrupt {
                    session,
                    resume: false,
                    reply,
                };
                if commands.send(Command::Line(request)).await.is_ok()
                    && let Ok(Ok(line::Stopping::Turn(turn))) = stopping.await
                {
                    let _ = turn.interrupt().await;
                }
            })
        });
        let foreground = crate::foreground::Foreground::new(
            session,
            &cwd,
            self.store.background().clone(),
            stop,
        );
        let (commands, step_session, step_turn) =
            (self.commands.clone(), session.to_owned(), id.clone());
        let boundary = move || {
            let (commands, session, turn) =
                (commands.clone(), step_session.clone(), step_turn.clone());
            async move {
                if let Some(commands) = commands.upgrade() {
                    let _ = commands.send(Command::StepBoundary { session, turn }).await;
                }
            }
        };
        let turn = RunningTurn::launch(
            thread.clone(),
            input,
            sink,
            writes,
            writer,
            foreground,
            crate::turn_runtime::Moments::new(session, self.events.clone()),
            boundary,
        );
        Ok(Live {
            turn,
            id,
            rules: policy,
            thread,
            provider,
            compacting,
        })
    }
}

fn stopped() -> io::Error {
    io::Error::new(
        ErrorKind::BrokenPipe,
        "The resident's coding-agent sessions are stopping.",
    )
}
