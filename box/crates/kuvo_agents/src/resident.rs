//! The resident owns one roster and serializes changes to its accounts.

use std::{collections::HashMap, io, sync::Arc};
use tokio::{
    sync::{Mutex, mpsc, oneshot, watch},
    time::Instant,
};

use crate::{
    accounts::{Account, AgentKind, Provider},
    roster::{Factory, Roster},
    runtime::{RecoveryFailure, Runtime},
    sign_in::{Request, SignIns, Started, Status, Time},
};

type Answer<T> = oneshot::Sender<io::Result<T>>;

enum Command {
    Add(AgentKind, Option<String>, Answer<Account>),
    Rename(Provider, Option<String>, Answer<()>),
    Remove(Provider, bool, Answer<()>),
}

#[derive(Clone)]
pub struct ResidentAgents {
    commands: mpsc::Sender<Command>,
    roster: watch::Receiver<Arc<Roster>>,
    sign_ins: Arc<Mutex<SignIns>>,
}

impl ResidentAgents {
    pub async fn start(
        runtime: Runtime,
        factories: HashMap<AgentKind, Arc<dyn Factory>>,
    ) -> io::Result<(Self, Vec<RecoveryFailure>)> {
        let (roster, failures) = runtime.activate(&factories).await?;
        let (published, updates) = watch::channel(Arc::new(roster.clone()));
        let (commands, requests) = mpsc::channel(16);
        let sign_ins = Arc::new(Mutex::new(SignIns::default()));
        let expiry = Arc::downgrade(&sign_ins);
        tokio::spawn(async move {
            let mut clock = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                clock.tick().await;
                let Some(sign_ins) = expiry.upgrade() else {
                    return;
                };
                sign_ins.lock().await.expire(Instant::now());
            }
        });
        tokio::spawn(run(runtime, factories, roster, published, requests));
        Ok((
            Self {
                commands,
                roster: updates,
                sign_ins,
            },
            failures,
        ))
    }

    pub fn roster(&self) -> Arc<Roster> {
        self.roster.borrow().clone()
    }

    pub async fn start_sign_in(
        &self,
        provider: &Provider,
        mut request: Request,
    ) -> io::Result<Started> {
        let roster = self.roster();
        let entry = lookup(&roster, provider)?;
        if request.remote && !entry.agent.remote_sign_in() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Sign in on the computer it runs on: its page comes back there.",
            ));
        }
        request.browser_return &= request.remote;
        let attempt = entry.agent.sign_in(request).await?;
        let Status::Pending { page } = attempt.status() else {
            attempt.cancel();
            return Err(io::Error::other("The sign-in is over. Start it again."));
        };
        let mut attempts = self.sign_ins.lock().await;
        let now = Instant::now();
        attempts.expire(now);
        let id = match attempts.track(provider.clone(), attempt.clone(), now) {
            Ok(id) => id,
            Err(error) => {
                attempt.cancel();
                return Err(error);
            }
        };
        Ok(Started {
            id,
            page,
            time: Time::now(),
        })
    }

    pub async fn sign_in_status(&self, provider: &Provider, id: &str) -> io::Result<Status> {
        let roster = self.roster();
        let entry = lookup(&roster, provider)?;
        let status = self
            .sign_ins
            .lock()
            .await
            .status(provider, id, Instant::now())
            .ok_or_else(over)?;
        if status == Status::Complete {
            entry.credentials_changed().await;
        }
        Ok(status)
    }

    pub async fn submit_code(&self, provider: &Provider, id: &str, code: String) -> io::Result<()> {
        let attempt = self
            .sign_ins
            .lock()
            .await
            .for_code(provider, id, Instant::now())?;
        attempt.submit_code(code).await
    }

    pub async fn cancel_sign_in(&self, provider: &Provider, id: &str) -> io::Result<()> {
        if self.sign_ins.lock().await.cancel(provider, id) {
            Ok(())
        } else {
            Err(over())
        }
    }

    pub async fn sign_in_with_key(&self, provider: &Provider, key: String) -> io::Result<()> {
        let roster = self.roster();
        let entry = lookup(&roster, provider)?;
        entry.agent.sign_in_with_key(key.trim().into()).await?;
        entry.credentials_changed().await;
        Ok(())
    }

    pub async fn sign_out(&self, provider: &Provider) -> io::Result<()> {
        let roster = self.roster();
        let entry = lookup(&roster, provider)?;
        entry.agent.sign_out().await?;
        entry.credentials_changed().await;
        Ok(())
    }

    pub async fn prepare_replacement(&self) -> Vec<(Provider, io::Error)> {
        let roster = self.roster();
        futures_util::future::join_all(roster.all().iter().map(|entry| async move {
            entry
                .agent
                .prepare_replacement()
                .await
                .err()
                .map(|error| (entry.account.provider(), error))
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }

    pub async fn add(&self, kind: AgentKind, label: Option<String>) -> io::Result<Account> {
        self.ask(|reply| Command::Add(kind, label, reply)).await
    }

    pub async fn rename(&self, provider: Provider, label: Option<String>) -> io::Result<()> {
        self.ask(|reply| Command::Rename(provider, label, reply))
            .await
    }

    pub async fn remove(&self, provider: Provider, only_unsigned: bool) -> io::Result<()> {
        self.ask(|reply| Command::Remove(provider, only_unsigned, reply))
            .await
    }

    async fn ask<T>(&self, command: impl FnOnce(Answer<T>) -> Command) -> io::Result<T> {
        let (answer, reply) = oneshot::channel();
        let stopped =
            || io::Error::new(io::ErrorKind::BrokenPipe, "The coding agents are stopping.");
        self.commands
            .send(command(answer))
            .await
            .map_err(|_| stopped())?;
        reply.await.map_err(|_| stopped())?
    }
}

async fn run(
    runtime: Runtime,
    factories: HashMap<AgentKind, Arc<dyn Factory>>,
    mut roster: Roster,
    published: watch::Sender<Arc<Roster>>,
    mut commands: mpsc::Receiver<Command>,
) {
    let accounts = runtime.accounts();
    while let Some(command) = commands.recv().await {
        match command {
            Command::Add(kind, label, reply) => {
                let result = async {
                    if !factories.contains_key(&kind) {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "That coding agent isn't available on this computer.",
                        ));
                    }
                    let account = accounts.add(kind, label).await?;
                    let result = roster.reconcile(accounts, &factories).await;
                    if let Err(error) = result {
                        let _ = accounts.remove(account.provider()).await;
                        return Err(error);
                    }
                    published.send_replace(Arc::new(roster.clone()));
                    Ok(account)
                }
                .await;
                let _ = reply.send(result);
            }
            Command::Rename(provider, label, reply) => {
                let result = async {
                    added(&roster, &provider)?;
                    accounts.rename(provider, label).await?;
                    let result = roster.reconcile(accounts, &factories).await;
                    published.send_replace(Arc::new(roster.clone()));
                    result
                }
                .await;
                let _ = reply.send(result);
            }
            Command::Remove(provider, only_unsigned, reply) => {
                let result = async {
                    let entry = added(&roster, &provider)?;
                    if only_unsigned {
                        match entry.status(true).await {
                            Ok(status) if status.signed_in => return Ok(()),
                            Ok(_) => {}
                            Err(_) => {
                                return Err(io::Error::other(
                                    "The sign-in couldn't be checked just now.",
                                ));
                            }
                        }
                    }
                    let _ = entry.agent.remove_credentials().await;
                    entry.agent.shutdown().await?;
                    let removed = accounts.remove(provider).await;
                    let result = roster.reconcile(accounts, &factories).await;
                    published.send_replace(Arc::new(roster.clone()));
                    removed.and(result)
                }
                .await;
                let _ = reply.send(result);
            }
        }
    }
    // An ordinary resident replacement drops controller ownership. Vendor
    // handles must leave their recorded holders available for adoption.
}

fn added<'a>(roster: &'a Roster, provider: &Provider) -> io::Result<&'a crate::roster::Entry> {
    let entry = lookup(roster, provider)?;
    if entry.account.id == "default" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "That's the agent's own sign-in: sign out of it where you signed in.",
        ));
    }
    Ok(entry)
}

fn lookup<'a>(roster: &'a Roster, provider: &Provider) -> io::Result<&'a crate::roster::Entry> {
    roster
        .find(provider)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No such account."))
}

fn over() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "That sign-in is over. Start it again.",
    )
}
