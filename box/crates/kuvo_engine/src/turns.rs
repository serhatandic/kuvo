//! Give each record its own FIFO turn. Dropping a turn releases it.

use std::collections::{HashMap, VecDeque, hash_map::Entry};

use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub(crate) struct Turns(mpsc::UnboundedSender<Command>);

pub(crate) struct Turn {
    session: String,
    release: Option<mpsc::UnboundedSender<Command>>,
}

type Waiting = oneshot::Sender<Turn>;

enum Command {
    Acquire(String, Waiting),
    Release(String),
}

impl Turns {
    pub(crate) fn new() -> Self {
        let (send, mut commands) = mpsc::unbounded_channel();
        let release = send.downgrade();
        tokio::spawn(async move {
            let mut held = HashMap::<String, VecDeque<Waiting>>::new();
            while let Some(command) = commands.recv().await {
                match command {
                    Command::Acquire(session, waiting) => match held.entry(session) {
                        Entry::Occupied(mut entry) => entry.get_mut().push_back(waiting),
                        Entry::Vacant(entry) => {
                            if give(&release, entry.key(), waiting) {
                                entry.insert(VecDeque::new());
                            }
                        }
                    },
                    Command::Release(session) => {
                        if let Entry::Occupied(mut entry) = held.entry(session) {
                            let mut granted = false;
                            while let Some(waiter) = entry.get_mut().pop_front() {
                                if give(&release, entry.key(), waiter) {
                                    granted = true;
                                    break;
                                }
                            }
                            if !granted {
                                entry.remove();
                            }
                        }
                    }
                }
            }
        });
        Self(send)
    }

    pub(crate) async fn acquire(&self, session: &str) -> Turn {
        let (give, take) = oneshot::channel();
        self.0
            .send(Command::Acquire(session.to_owned(), give))
            .expect("the engine's turn coordinator lives with its callers");
        take.await
            .expect("the engine's turn coordinator gives each turn")
    }
}

fn give(release: &mpsc::WeakUnboundedSender<Command>, session: &str, waiting: Waiting) -> bool {
    let Some(release) = release.upgrade() else {
        return false;
    };
    let permit = Turn {
        session: session.to_owned(),
        release: Some(release),
    };
    match waiting.send(permit) {
        Ok(()) => true,
        Err(mut left) => {
            // This caller left before it acquired the turn.
            left.release = None;
            false
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(Command::Release(std::mem::take(&mut self.session)));
        }
    }
}
