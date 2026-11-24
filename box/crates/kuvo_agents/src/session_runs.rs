//! The resident's live coding-agent sessions.

use crate::{
    accounts::Provider,
    live_asks::{Ask, AskKey, LiveAsks, Reply as AskReply},
    permissions::{
        Allowance, ConfigLocation, Policy, covers, remember,
        turn::{Settings, TurnRules},
    },
    resident::ResidentAgents,
    session::{Decision, Prompt, Steer, TurnInput},
    session_store::SessionStore,
    turn_projection::{Projection, user},
    turn_runtime::RunningTurn,
    turn_sink::ResidentSink,
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, ErrorKind},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};

type Reply<T> = oneshot::Sender<io::Result<T>>;

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
}

struct Start {
    session: String,
    input: TurnInput,
    policy: Policy,
    note: Option<Map<String, Value>>,
    reply: Reply<RunningTurn>,
}

#[derive(Clone)]
struct Live {
    turn: RunningTurn,
    id: String,
    rules: Arc<TurnRules>,
}

#[derive(Clone)]
pub struct SessionRuns {
    asks: LiveAsks,
    store: SessionStore,
    commands: mpsc::Sender<Command>,
    running: watch::Receiver<Arc<BTreeSet<String>>>,
}

impl SessionRuns {
    pub fn start(
        agents: ResidentAgents,
        store: SessionStore,
        home: PathBuf,
        location: ConfigLocation,
    ) -> Self {
        let (commands, requests) = mpsc::channel(32);
        let (identities, running) = watch::channel(Arc::new(BTreeSet::new()));
        let asks = LiveAsks::start();
        let service = Service {
            agents,
            store: store.clone(),
            asks: asks.clone(),
            identities,
            settings: Settings { home, location },
        };
        tokio::spawn(service.run(requests));
        Self {
            asks,
            store,
            commands,
            running,
        }
    }

    pub fn asks(&self) -> &LiveAsks {
        &self.asks
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
        self.running.borrow().clone()
    }

    pub fn changes(&self) -> watch::Receiver<Arc<BTreeSet<String>>> {
        let mut watch = self.running.clone();
        watch.mark_unchanged();
        watch
    }

    pub async fn run_turn(
        &self,
        session: String,
        input: TurnInput,
        policy: Policy,
    ) -> io::Result<RunningTurn> {
        self.run_turn_noted(session, input, policy, None).await
    }

    /// Start a turn whose message carries a client's notes (its `metadata`),
    /// kept on the conversation's row for it.
    pub async fn run_turn_noted(
        &self,
        session: String,
        input: TurnInput,
        policy: Policy,
        note: Option<Map<String, Value>>,
    ) -> io::Result<RunningTurn> {
        self.call(|reply| {
            Command::Start(Box::new(Start {
                session,
                input,
                policy,
                note,
                reply,
            }))
        })
        .await
    }

    pub async fn interrupt(&self, session: String) -> io::Result<bool> {
        let Some(run) = self.find(session).await? else {
            return Ok(false);
        };
        run.turn.interrupt().await?;
        Ok(true)
    }

    pub async fn steer(&self, session: String, prompt: Prompt) -> io::Result<Steer> {
        let Some(run) = self.find(session.clone()).await? else {
            return Ok(Steer::Declined);
        };
        let reply = run.turn.steer(prompt.clone()).await?;
        if reply == Steer::Accepted
            && !self
                .store
                .write_messages(&session, vec![user(&prompt)])
                .await?
        {
            return Err(io::Error::new(
                ErrorKind::NotFound,
                "That coding-agent session isn't here.",
            ));
        }
        Ok(reply)
    }

    /// Delete a conversation as the bridge does: no turn starts meanwhile, the
    /// running one stops first, and its subagents' conversations go with it
    /// unless a fork's steps still show them. False when it wasn't here.
    pub async fn delete(&self, session: String) -> io::Result<bool> {
        Box::pin(self.delete_retired(session)).await
    }

    async fn delete_retired(&self, session: String) -> io::Result<bool> {
        let live = self
            .call(|reply| Command::Retire {
                session: session.clone(),
                reply,
            })
            .await?;
        let deleted = async {
            if let Some(live) = live {
                // A turn that can't be interrupted still ends when its record goes.
                let _ = live.turn.interrupt().await;
                let _ = live.turn.finished().await;
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
        let _ = self
            .commands
            .send(Command::Release { session })
            .await;
        deleted
    }

    async fn find(&self, session: String) -> io::Result<Option<Live>> {
        self.call(|reply| Command::Find { session, reply }).await
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
    agents: ResidentAgents,
    store: SessionStore,
    asks: LiveAsks,
    identities: watch::Sender<Arc<BTreeSet<String>>>,
    settings: Settings,
}

impl Service {
    async fn run(self, mut commands: mpsc::Receiver<Command>) {
        let mut sessions = BTreeMap::new();
        let mut starting = BTreeSet::new();
        let mut preparations = JoinSet::new();
        let mut by_session = JoinSet::new();
        let mut approvals = BTreeMap::<String, watch::Sender<Vec<Allowance>>>::new();
        let mut retired = BTreeSet::new();
        loop {
            tokio::select! {
                next = commands.recv() => {
                    match next {
                        Some(Command::Start(request)) => {
                            let Start { session, input, policy, note, reply } = *request;
                            if retired.contains(&session) {
                                let _ = reply.send(Err(io::Error::new(
                                    ErrorKind::NotFound, "That conversation is being deleted.",
                                )));
                                continue;
                            }
                            if sessions.contains_key(&session) || !starting.insert(session.clone()) {
                                let _ = reply.send(Err(io::Error::new(
                                    ErrorKind::AlreadyExists, "That conversation is already working.",
                                )));
                                continue;
                            }
                            let remembered = approvals.entry(session.clone())
                                .or_insert_with(|| watch::channel(Vec::new()).0).clone();
                            let service = self.clone();
                            preparations.spawn(async move {
                                let outcome = service.begin(&session, input, policy, remembered, note).await;
                                (session, outcome, reply)
                            });
                        }
                        Some(Command::Find { session, reply }) => {
                            let _ = reply.send(Ok(sessions.get(&session).cloned()));
                        }
                        Some(Command::Retire { session, reply }) => {
                            retired.insert(session.clone());
                            approvals.remove(&session);
                            let _ = reply.send(Ok(sessions.get(&session).cloned()));
                        }
                        Some(Command::Release { session }) => {
                            retired.remove(&session);
                        }
                        None => break,
                    }
                }
                prepared = preparations.join_next(), if !preparations.is_empty() => {
                    if let Some(Ok((session, mut outcome, reply))) = prepared {
                        starting.remove(&session);
                        if retired.contains(&session)
                            && let Ok(live) = &outcome
                        {
                            // Deleted while it started: stop it rather than run it.
                            let turn = live.turn.clone();
                            tokio::spawn(async move { let _ = turn.interrupt().await; });
                            outcome = Err(io::Error::new(
                                ErrorKind::NotFound, "That conversation is being deleted.",
                            ));
                        }
                        if let Ok(turn) = &outcome {
                            sessions.insert(session.clone(), turn.clone());
                            self.publish(&sessions);
                            let turn = turn.turn.clone();
                            by_session.spawn(async move { (session, turn.finished().await) });
                        }
                            let _ = reply.send(outcome.map(|live| live.turn));
                    }
                }
                completed = by_session.join_next(), if !by_session.is_empty() => {
                    if let Some(Ok((session, result))) = completed {
                        sessions.remove(&session);
                        self.publish(&sessions);
                        if let Err(error) = result {
                            eprintln!("Kuvo could not finish coding-agent session {session}: {error}");
                        }
                    }
                }
            }
        }
    }

    fn publish(&self, runs: &BTreeMap<String, Live>) {
        self.identities
            .send_replace(Arc::new(runs.keys().cloned().collect()));
    }

    async fn begin(
        &self,
        session: &str,
        input: TurnInput,
        mut policy: Policy,
        remembered: watch::Sender<Vec<Allowance>>,
        note: Option<Map<String, Value>>,
    ) -> io::Result<Live> {
        let provider = Provider::parse(&input.model.provider).ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidInput, "Choose a coding-agent model.")
        })?;
        let roster = self.agents.roster();
        let entry = roster.find(&provider).ok_or_else(|| {
            io::Error::new(ErrorKind::NotFound, "That coding-agent account isn't here.")
        })?;
        let context = self.store.context(session).await?.ok_or_else(|| {
            io::Error::new(ErrorKind::NotFound, "That coding-agent session isn't here.")
        })?;
        policy.directory = context.directory.clone();
        let thread = entry.agent.open_thread(context).await?;
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
        let agent = if input.plan { "plan" } else { "build" };
        let writer =
            Projection::start(self.store.clone(), session.into(), agent.into(), &input, note)
                .await?;
        let asks = self.asks.for_turn(session.into(), input.id.clone());
        let (sink, writes) =
            ResidentSink::channel(session.into(), self.store.clone(), asks, policy.clone());
        let id = input.id.clone();
        let turn = RunningTurn::launch(thread, input, sink, writes, writer);
        Ok(Live {
            turn,
            id,
            rules: policy,
        })
    }
}

fn stopped() -> io::Error {
    io::Error::new(
        ErrorKind::BrokenPipe,
        "The resident's coding-agent sessions are stopping.",
    )
}
