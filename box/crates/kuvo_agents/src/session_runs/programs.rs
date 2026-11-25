//! A program being updated holds its turns, for every account with it: what's sent waits in line,
//! kept, and the turns already under way finish first. So does an installer an earlier resident
//! left running (its record, `agents/program-installing`).

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    sync::{Arc, Mutex as Plain},
    time::Duration,
};

use tokio::sync::oneshot;

use super::{Command, Doing, Service, State};
use kuvo_store::home::Home;

use crate::{
    accounts::Provider,
    hold::Directory,
    inbox::{Delivery, InboxEvent},
    program_updates::{Installation, installation},
};

fn lock<T>(plain: &Plain<T>) -> std::sync::MutexGuard<'_, T> {
    plain.lock().unwrap_or_else(|error| error.into_inner())
}

/// The program an account's agent runs (`codex`); none for the engine's models.
pub(super) fn program_of(provider: &str) -> Option<&'static str> {
    Provider::parse(provider).map(|parsed| parsed.kind.name())
}

#[derive(Clone)]
pub(super) struct Holds {
    agents: PathBuf,
    /// Agents kept running across residents: one with a turn under way is the program's work.
    holders: Option<Directory>,
    held: Arc<Plain<BTreeSet<String>>>,
    /// Turns past their program's wait, not yet running: they count as its work.
    starting: Arc<Plain<BTreeMap<String, String>>>,
}

impl Holds {
    pub fn new(home: &std::path::Path) -> Self {
        Self {
            agents: home.join("agents"),
            holders: Home::new(home.to_owned())
                .ok()
                .map(|home| Directory::for_home(&home)),
            held: Arc::default(),
            starting: Arc::default(),
        }
    }

    /// Whether `provider`'s program holds its turns now.
    pub fn holds(&self, provider: &str) -> bool {
        program_of(provider).is_some_and(|program| {
            lock(&self.held).contains(program)
                || installation(&self.agents, program) == Some(Installation::Running)
        })
    }

    /// At the turn's own start (Auto may have picked another program since it left the line):
    /// waits while its program is held (`wait`), then counts it as that program's work.
    pub async fn enter(&self, session: &str, provider: &str, wait: bool) {
        while wait && self.holds(provider) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(program) = program_of(provider) {
            lock(&self.starting).insert(session.to_owned(), program.to_owned());
        }
    }

    /// It runs (or never will): its conversation says so from here.
    pub fn entered(&self, session: &str) {
        lock(&self.starting).remove(session);
    }
}

impl super::SessionRuns {
    /// Holds `program`'s turns across its accounts, then waits for those under way to finish:
    /// the release lets them go. Past `deadline` (ms), it lets them go and fails.
    pub async fn hold_program(
        &self,
        program: &str,
        deadline: u64,
    ) -> io::Result<Box<dyn FnOnce() + Send>> {
        let holds = self.programs.clone();
        lock(&holds.held).insert(program.to_owned());
        let commands = self.commands.clone();
        let name = program.to_owned();
        let release = move || {
            lock(&holds.held).remove(&name);
            let _ = commands.try_send(Command::WakeHeld);
        };
        while crate::background::now() < deadline {
            let (reply, working) = oneshot::channel();
            let asked = Command::ProgramWorks {
                program: program.to_owned(),
                reply,
            };
            let works = match self.commands.send(asked).await {
                Ok(()) => working.await.unwrap_or(false),
                Err(_) => false,
            };
            let installing =
                installation(&self.programs.agents, program) == Some(Installation::Running);
            if !works && !installing {
                return Ok(Box::new(release));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        release();
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "The program's update wait expired. Try again after the running turns finish.",
        ))
    }
}

impl Service {
    /// Whether `program` has a turn under way here, or in an agent kept running for another
    /// resident.
    pub(super) async fn program_works(&self, state: &State, program: &str) -> bool {
        let ours = |provider: &str| program_of(provider) == Some(program);
        let running = state.conversations.values().any(|conversation| {
            matches!(&conversation.doing, Doing::Running(live) | Doing::Stopping(live) if ours(&live.provider))
        });
        let starting = lock(&self.programs.starting)
            .values()
            .any(|each| each == program);
        if running || starting {
            return true;
        }
        let Some(holders) = &self.programs.holders else {
            return false;
        };
        let Ok(records) = holders.records().await else {
            return false;
        };
        records.iter().any(|record| {
            record.exited.is_none()
                && record.meta.get("kind").and_then(|kind| kind.as_str()) == Some(program)
                && record
                    .meta
                    .get("turns")
                    .and_then(|turns| turns.as_object())
                    .is_some_and(|turns| !turns.is_empty())
        })
    }

    /// The next in line for `session` waits, kept, while its program updates. Whether it does.
    pub(super) async fn held_for_program(&self, session: &str, index: usize) -> bool {
        let Ok(Some(header)) = self.store.read(session).await else {
            return false;
        };
        let fields = header.into_fields();
        let provider = fields
            .get("model")
            .and_then(|model| model["providerID"].as_str())
            .unwrap_or_default();
        if !self.programs.holds(provider) {
            return false;
        }
        let changed = self
            .store
            .edit_line(session, move |line| match line.inputs.get_mut(index) {
                Some(input) => {
                    input.held = true;
                    let id = input.id.clone();
                    let moved = input.delivery != Delivery::Steer;
                    input.delivery = Delivery::Steer;
                    (moved.then_some(id), true)
                }
                None => (None, false),
            })
            .await;
        if let Ok(Some(Some(id))) = changed {
            let _ = self.events.send(InboxEvent::DeliveryChanged {
                session: session.to_owned(),
                id,
                delivery: Delivery::Steer,
            });
        }
        true
    }

    /// A program's update is over: what it held goes, unless the resident is making way.
    pub(super) async fn wake_held(&self, state: &mut State) {
        if state.closing {
            return;
        }
        let Ok(headers) = self.store.headers().await else {
            return;
        };
        for header in headers {
            let session = header.id().to_owned();
            let Ok(Some(line)) = self.store.line(&session).await else {
                continue;
            };
            if !line.iter().any(|input| input.held) {
                continue;
            }
            let released = self
                .store
                .edit_line(&session, |line| {
                    for input in &mut line.inputs {
                        input.held = false;
                    }
                    ((), true)
                })
                .await;
            if released.is_ok() {
                self.wake(state, &session).await;
            }
        }
    }
}

impl crate::program_updates::Hold for super::SessionRuns {
    fn hold<'a>(
        &'a self,
        program: &'a str,
        deadline: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<Box<dyn FnOnce() + Send>, String>> {
        Box::pin(async move {
            self.hold_program(program, deadline)
                .await
                .map_err(|error| error.to_string())
        })
    }
}
