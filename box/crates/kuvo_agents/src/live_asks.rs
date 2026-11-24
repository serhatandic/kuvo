//! Ask ownership lives with the resident; vendors keep waiting asynchronously.

use crate::session::{Answer, Decision, PermissionAsk, QuestionAsk};
use std::{
    collections::BTreeMap,
    io::{self, ErrorKind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AskKey {
    pub session: String,
    pub turn: String,
    pub id: String,
}

impl AskKey {
    /// The ID shown to clients includes the Kuvo turn. A delayed answer for
    /// one turn cannot approve an identically named native request in another.
    pub fn client_id(&self, question: bool) -> String {
        let prefix = if question { "frm" } else { "per" };
        format!(
            "{prefix}_{}_{}",
            hex::encode(&self.turn),
            hex::encode(&self.id)
        )
    }
}

#[derive(Clone, Debug)]
pub enum Ask {
    Permission(PermissionAsk),
    Question(QuestionAsk),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Permission(Decision),
    Question(Answer),
}

type ResultSender<T> = oneshot::Sender<io::Result<T>>;

enum Command {
    Ask(AskKey, Ask, Arc<AtomicBool>, ResultSender<Reply>),
    Answer(AskKey, Reply, ResultSender<()>),
    Cancel(AskKey, ResultSender<()>),
    End(String, String, ResultSender<()>),
    Keep(ResultSender<()>),
}

struct Pending {
    ask: Ask,
    answer: ResultSender<Reply>,
}

impl Pending {
    fn cancel(self) {
        let answer = match self.ask {
            Ask::Permission(_) => Reply::Permission(Decision::Reject),
            Ask::Question(_) => Reply::Question(Answer::Dismissed { reason: None }),
        };
        let _ = self.answer.send(Ok(answer));
    }
}

#[derive(Clone)]
pub struct LiveAsks {
    commands: mpsc::Sender<Command>,
    pending: watch::Receiver<Arc<BTreeMap<AskKey, Ask>>>,
}

impl LiveAsks {
    pub fn start() -> Self {
        let (commands, requests) = mpsc::channel(32);
        let (published, pending) = watch::channel(Arc::new(BTreeMap::new()));
        tokio::spawn(run(requests, published));
        Self { commands, pending }
    }

    pub fn pending(&self) -> watch::Receiver<Arc<BTreeMap<AskKey, Ask>>> {
        let mut changes = self.pending.clone();
        changes.mark_unchanged();
        changes
    }

    pub fn for_turn(&self, session: String, turn: String) -> TurnAsks {
        TurnAsks {
            book: self.clone(),
            session,
            turn,
            ended: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn reply(&self, key: AskKey, answer: Reply) -> io::Result<()> {
        self.call(|reply| Command::Answer(key, answer, reply)).await
    }

    /// The resident makes way for the next, which asks again: from now on
    /// nothing waiting is answered here, not even turned down as the
    /// resident goes, so the agent hears only the next resident's answer.
    pub async fn keep_unanswered(&self) -> io::Result<()> {
        self.call(Command::Keep).await
    }

    async fn call<T>(&self, command: impl FnOnce(ResultSender<T>) -> Command) -> io::Result<T> {
        let (answer, receive) = oneshot::channel();
        self.commands
            .send(command(answer))
            .await
            .map_err(|_| stopped())?;
        receive.await.map_err(|_| stopped())?
    }
}

/// One adapter's ask handle for an active turn. The resident explicitly
/// retires this handle before releasing session ownership.
#[derive(Clone)]
pub struct TurnAsks {
    book: LiveAsks,
    session: String,
    turn: String,
    ended: Arc<AtomicBool>,
}

impl TurnAsks {
    pub(crate) fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    fn key(&self, id: &str) -> AskKey {
        AskKey {
            session: self.session.clone(),
            turn: self.turn.clone(),
            id: id.to_owned(),
        }
    }

    pub async fn permission(&self, ask: PermissionAsk) -> io::Result<Decision> {
        let key = self.key(&ask.id);
        match self
            .book
            .call(|answer| Command::Ask(key, Ask::Permission(ask), self.ended.clone(), answer))
            .await?
        {
            Reply::Permission(decision) => Ok(decision),
            Reply::Question(_) => Err(io::Error::other("Expected a permission decision.")),
        }
    }

    pub async fn question(&self, ask: QuestionAsk) -> io::Result<Answer> {
        let key = self.key(&ask.id);
        match self
            .book
            .call(|answer| Command::Ask(key, Ask::Question(ask), self.ended.clone(), answer))
            .await?
        {
            Reply::Question(answer) => Ok(answer),
            Reply::Permission(_) => Err(io::Error::other("Expected a question answer.")),
        }
    }

    pub async fn cancel(&self, id: &str) -> io::Result<()> {
        self.book
            .call(|answer| Command::Cancel(self.key(id), answer))
            .await
    }

    pub async fn end(&self) -> io::Result<()> {
        self.ended.store(true, Ordering::Release);
        self.book
            .call(|reply| Command::End(self.session.clone(), self.turn.clone(), reply))
            .await
    }
}

async fn run(
    mut commands: mpsc::Receiver<Command>,
    published: watch::Sender<Arc<BTreeMap<AskKey, Ask>>>,
) {
    let mut pending = BTreeMap::<AskKey, Pending>::new();
    let mut kept = false;
    while let Some(command) = commands.recv().await {
        let command = match command {
            Command::Keep(result) => {
                kept = true;
                let _ = result.send(Ok(()));
                continue;
            }
            Command::Answer(_, _, result) if kept => {
                let _ = result.send(Err(stopped()));
                continue;
            }
            Command::Cancel(_, result) | Command::End(_, _, result) if kept => {
                let _ = result.send(Ok(()));
                continue;
            }
            command => command,
        };
        let reply = match command {
            Command::Ask(key, ask, ended, answer) => {
                if answer.is_closed() {
                    continue;
                }
                if ended.load(Ordering::Acquire) {
                    Pending { ask, answer }.cancel();
                    continue;
                }
                if pending
                    .get(&key)
                    .is_some_and(|entry| !entry.answer.is_closed())
                {
                    let _ = answer.send(Err(io::Error::new(
                        ErrorKind::AlreadyExists,
                        "This agent is already waiting for that answer.",
                    )));
                    continue;
                }
                pending.insert(key, Pending { ask, answer });
                None
            }
            Command::Answer(key, reply, result) => {
                let found = pending.get(&key);
                let outcome = match found {
                    Some(Pending {
                        ask: Ask::Permission(_),
                        ..
                    }) if matches!(reply, Reply::Permission(_)) => {
                        deliver(pending.remove(&key), reply)
                    }
                    Some(Pending {
                        ask: Ask::Question(_),
                        ..
                    }) if matches!(reply, Reply::Question(_)) => {
                        deliver(pending.remove(&key), reply)
                    }
                    Some(_) => Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        "Answer the kind of question the agent asked.",
                    )),
                    None => Err(gone()),
                };
                Some((result, outcome))
            }
            Command::Cancel(key, result) => {
                if let Some(ask) = pending.remove(&key) {
                    ask.cancel();
                }
                Some((result, Ok(())))
            }
            Command::Keep(_) => unreachable!("handled above"),
            Command::End(session, turn, result) => {
                pending = pending
                    .into_iter()
                    .filter_map(|(key, ask)| {
                        if key.session != session || key.turn != turn {
                            return Some((key, ask));
                        }
                        ask.cancel();
                        None
                    })
                    .collect();
                Some((result, Ok(())))
            }
        };
        pending.retain(|_, entry| !entry.answer.is_closed());
        published.send_replace(Arc::new(
            pending
                .iter()
                .map(|(key, entry)| (key.clone(), entry.ask.clone()))
                .collect(),
        ));
        if let Some((sender, reply)) = reply {
            let _ = sender.send(reply);
        }
    }
    if kept {
        // (Never answered: the process ends with them waiting.)
        std::mem::forget(pending);
        return;
    }
    for (_, ask) in pending {
        ask.cancel();
    }
}

fn deliver(pending: Option<Pending>, reply: Reply) -> io::Result<()> {
    pending
        .ok_or_else(gone)?
        .answer
        .send(Ok(reply))
        .map_err(|_| gone())
}

fn gone() -> io::Error {
    io::Error::new(
        ErrorKind::NotFound,
        "The agent is no longer waiting for that answer.",
    )
}

fn stopped() -> io::Error {
    io::Error::new(ErrorKind::BrokenPipe, "The coding agents are stopping.")
}
