//! Contract between the resident and its vendor adapters.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
};

use crate::{
    accounts::{Account, Accounts, AgentKind, Provider},
    hold::{Controller, Directory, Record, Replay, TurnNote},
    roster::{Factory, Roster},
    session_store::SessionStore,
};
use kuvo_store::home::Home;

pub type AdapterCall<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// An adapter decides whether it can reconstruct its protocol state before
/// taking ownership of a holder. The resident owns account selection and replay.
pub trait Adoption: Send + Sync {
    fn accepts(&self, record: &Record, turns: &BTreeMap<String, TurnNote>) -> bool;
    /// Take ownership of the controller and start reading the replay. Return
    /// after installing the reader so other holders can be adopted.
    fn attach<'a>(
        &'a self,
        account: &'a Account,
        record: Record,
        turns: BTreeMap<String, TurnNote>,
        controller: Controller,
    ) -> AdapterCall<'a, ()>;

    /// The resident won't take up this attached turn after all: its
    /// conversation is gone or has moved on, or `Thread::start` with
    /// `TurnKind::Recover` failed. Stop the held agent unless another turn
    /// taken up in it still runs.
    fn let_go<'a>(&'a self, _turn: &'a TurnNote) -> AdapterCall<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
pub struct Runtime {
    accounts: Accounts,
    holders: Directory,
    sessions: SessionStore,
}

#[derive(Debug)]
pub struct RecoveryFailure {
    pub holder: String,
    pub error: io::Error,
}

#[derive(Clone)]
pub struct AdoptedTurn {
    pub provider: Provider,
    pub note: TurnNote,
}

impl Runtime {
    pub fn for_home(home: &Home) -> Self {
        Self {
            accounts: Accounts::for_home(home),
            holders: Directory::for_home(home),
            sessions: SessionStore::new(home.root().join("agents/sessions")),
        }
    }

    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }

    pub fn holders(&self) -> &Directory {
        &self.holders
    }

    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }

    /// Initialize the adapters and restore their holders before publishing
    /// readiness. The caller supplies the compiled vendor factories.
    pub async fn activate(
        &self,
        factories: &HashMap<AgentKind, Arc<dyn Factory>>,
    ) -> io::Result<(Roster, Vec<RecoveryFailure>)> {
        let (roster, failures, _) = self.activate_with_turns(factories).await?;
        Ok((roster, failures))
    }

    pub(crate) async fn activate_with_turns(
        &self,
        factories: &HashMap<AgentKind, Arc<dyn Factory>>,
    ) -> io::Result<(Roster, Vec<RecoveryFailure>, Vec<AdoptedTurn>)> {
        let mut roster = Roster::default();
        roster.reconcile(&self.accounts, factories).await?;
        let (failures, turns) = self.recover_with_turns(&roster.recovery_adapters()).await?;
        Ok((roster, failures, turns))
    }

    /// The conversations whose turns go on in held agents: the next
    /// resident can take them up.
    pub async fn held_sessions(&self) -> io::Result<std::collections::BTreeSet<String>> {
        let mut held = std::collections::BTreeSet::new();
        for record in self.holders.records().await? {
            let Ok(turns) = record.turns() else { continue };
            held.extend(
                turns
                    .into_values()
                    .filter(|turn| turn.turn.is_some())
                    .map(|turn| turn.session),
            );
        }
        Ok(held)
    }

    /// Read only the saved session header. Model routing does not need to open
    /// the conversation or resume the agent to find its working directory.
    pub async fn session_folder(&self, id: &str) -> io::Result<Option<String>> {
        Ok(self
            .sessions
            .read(id)
            .await?
            .and_then(|session| session.directory().map(str::to_owned)))
    }

    /// Give each recoverable stream to its account's adapter. An unavailable
    /// adapter leaves the stream for the holder's ordinary orphan deadline.
    pub async fn recover(
        &self,
        adapters: &HashMap<Provider, Arc<dyn Adoption>>,
    ) -> io::Result<Vec<RecoveryFailure>> {
        let (failures, _) = self.recover_with_turns(adapters).await?;
        Ok(failures)
    }

    async fn recover_with_turns(
        &self,
        adapters: &HashMap<Provider, Arc<dyn Adoption>>,
    ) -> io::Result<(Vec<RecoveryFailure>, Vec<AdoptedTurn>)> {
        let accounts = self.accounts.all().await?;
        let mut failed = Vec::new();
        let mut adopted = Vec::new();
        for record in self.holders.records().await? {
            let Some(provider) = record
                .meta
                .get("provider")
                .and_then(serde_json::Value::as_str)
                .and_then(Provider::parse)
            else {
                continue;
            };
            let (Some(adapter), Some(account)) = (
                adapters.get(&provider),
                accounts
                    .iter()
                    .find(|account| account.provider() == provider),
            ) else {
                continue;
            };
            let holder = record.id.clone();
            match self.take_up(adapter.as_ref(), account, record).await {
                Ok(turns) => adopted.extend(turns.into_values().map(|note| AdoptedTurn {
                    provider: provider.clone(),
                    note,
                })),
                Err(error) => failed.push(RecoveryFailure { holder, error }),
            }
        }
        Ok((failed, adopted))
    }

    async fn take_up(
        &self,
        adapter: &dyn Adoption,
        account: &Account,
        record: Record,
    ) -> io::Result<BTreeMap<String, TurnNote>> {
        let turns = record.turns()?;
        if !adapter.accepts(&record, &turns) {
            return Ok(BTreeMap::new());
        }
        let replay = turns
            .values()
            .map(|turn| turn.from)
            .min()
            .map_or(Replay::Idle, Replay::Complete);
        let controller = self.holders.adopt(&record, replay).await?;
        adapter
            .attach(account, record, turns.clone(), controller)
            .await?;
        Ok(turns)
    }
}
