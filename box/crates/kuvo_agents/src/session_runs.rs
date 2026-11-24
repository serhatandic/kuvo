//! The resident's live coding-agent sessions.

use crate::{
    accounts::Provider,
    live_asks::LiveAsks,
    permissions::{
        ConfigLocation, Policy,
        turn::{Settings, TurnRules},
    },
    resident::ResidentAgents,
    session::{Prompt, Steer, TurnInput},
    session_store::SessionStore,
    turn_projection::{Projection, user},
    turn_runtime::RunningTurn,
    turn_sink::ResidentSink,
};
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
        reply: Reply<Option<RunningTurn>>,
    },
}

struct Start {
    session: String,
    input: TurnInput,
    policy: Policy,
    reply: Reply<RunningTurn>,
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
        self.call(|reply| Command::Start(Box::new(Start {
            session,
            input,
            policy,
            reply,
        })))
        .await
    }

    pub async fn interrupt(&self, session: String) -> io::Result<bool> {
        let Some(run) = self.find(session).await? else {
            return Ok(false);
        };
        run.interrupt().await?;
        Ok(true)
    }

    pub async fn steer(&self, session: String, prompt: Prompt) -> io::Result<Steer> {
        let Some(run) = self.find(session.clone()).await? else {
            return Ok(Steer::Declined);
        };
        let reply = run.steer(prompt.clone()).await?;
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

    async fn find(&self, session: String) -> io::Result<Option<RunningTurn>> {
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
        loop {
            tokio::select! {
                next = commands.recv() => {
                    match next {
                        Some(Command::Start(request)) => {
                            let Start { session, input, policy, reply } = *request;
                            if sessions.contains_key(&session) || !starting.insert(session.clone()) {
                                let _ = reply.send(Err(io::Error::new(
                                    ErrorKind::AlreadyExists, "That conversation is already working.",
                                )));
                                continue;
                            }
                            let service = self.clone();
                            preparations.spawn(async move {
                                let outcome = service.begin(&session, input, policy).await;
                                (session, outcome, reply)
                            });
                        }
                        Some(Command::Find { session, reply }) => {
                            let _ = reply.send(Ok(sessions.get(&session).cloned()));
                        }
                        None => break,
                    }
                }
                prepared = preparations.join_next(), if !preparations.is_empty() => {
                    if let Some(Ok((session, outcome, reply))) = prepared {
                        starting.remove(&session);
                        if let Ok(turn) = &outcome {
                            sessions.insert(session.clone(), turn.clone());
                            self.publish(&sessions);
                            let turn = turn.clone();
                            by_session.spawn(async move { (session, turn.finished().await) });
                        }
                        let _ = reply.send(outcome);
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

    fn publish(&self, runs: &BTreeMap<String, RunningTurn>) {
        self.identities
            .send_replace(Arc::new(runs.keys().cloned().collect()));
    }

    async fn begin(
        &self,
        session: &str,
        input: TurnInput,
        mut policy: Policy,
    ) -> io::Result<RunningTurn> {
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
        });
        let agent = if input.plan { "plan" } else { "build" };
        let writer =
            Projection::start(self.store.clone(), session.into(), agent.into(), &input).await?;
        let asks = self.asks.for_turn(session.into(), input.id.clone());
        let (sink, writes) =
            ResidentSink::channel(session.into(), self.store.clone(), asks, policy);
        Ok(RunningTurn::launch(thread, input, sink, writes, writer))
    }
}

fn stopped() -> io::Error {
    io::Error::new(
        ErrorKind::BrokenPipe,
        "The resident's coding-agent sessions are stopping.",
    )
}
