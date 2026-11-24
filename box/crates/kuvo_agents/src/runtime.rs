//! Contract between the resident and its vendor adapters.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    io,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
};

use crate::{
    accounts::{Account, Accounts, AgentKind, Provider},
    hold::{Controller, Directory, Record, Replay, TurnNote},
    roster::{Factory, Roster},
};
use kuvo_store::{format::ObjectFormat, home::Home};

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
}

#[derive(Clone)]
pub struct Runtime {
    accounts: Accounts,
    holders: Directory,
    sessions: PathBuf,
}

#[derive(Debug)]
pub struct RecoveryFailure {
    pub holder: String,
    pub error: io::Error,
}

impl Runtime {
    pub fn for_home(home: &Home) -> Self {
        Self {
            accounts: Accounts::for_home(home),
            holders: Directory::for_home(home),
            sessions: home.root().join("agents/sessions"),
        }
    }

    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }

    pub fn holders(&self) -> &Directory {
        &self.holders
    }

    /// Initialize the adapters and restore their holders before publishing
    /// readiness. The caller supplies the compiled vendor factories.
    pub async fn activate(
        &self,
        factories: &HashMap<AgentKind, Arc<dyn Factory>>,
    ) -> io::Result<(Roster, Vec<RecoveryFailure>)> {
        let mut roster = Roster::default();
        roster.reconcile(&self.accounts, factories).await?;
        let failures = self.recover(&roster.recovery_adapters()).await?;
        Ok((roster, failures))
    }

    /// Read only the saved session header. Model routing does not need to open
    /// the conversation or resume the agent to find its working directory.
    pub async fn session_folder(&self, id: &str) -> io::Result<Option<String>> {
        if !id.starts_with("ses_")
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Ok(None);
        }
        let path = self.sessions.join(id).join("session.json");
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let format = ObjectFormat::additive("Coding-agent session");
            let saved = format
                .read(&path)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let Some(saved) = saved else { return Ok(None) };
            if saved.fields.get("id").and_then(serde_json::Value::as_str) != Some(id.as_str()) {
                return Ok(None);
            }
            Ok(saved
                .fields
                .get("location")
                .and_then(|location| location.get("directory"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned))
        })
        .await?
    }

    /// Give each recoverable stream to its account's adapter. An unavailable
    /// adapter leaves the stream for the holder's ordinary orphan deadline.
    pub async fn recover(
        &self,
        adapters: &HashMap<Provider, Arc<dyn Adoption>>,
    ) -> io::Result<Vec<RecoveryFailure>> {
        let accounts = self.accounts.all().await?;
        let mut failed = Vec::new();
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
            if let Err(error) = self.take_up(adapter.as_ref(), account, record).await {
                failed.push(RecoveryFailure { holder, error });
            }
        }
        Ok(failed)
    }

    async fn take_up(
        &self,
        adapter: &dyn Adoption,
        account: &Account,
        record: Record,
    ) -> io::Result<()> {
        let turns = record.turns()?;
        if !adapter.accepts(&record, &turns) {
            return Ok(());
        }
        let replay = turns
            .values()
            .map(|turn| turn.from)
            .min()
            .map_or(Replay::Idle, Replay::Complete);
        let controller = self.holders.adopt(&record, replay).await?;
        adapter.attach(account, record, turns, controller).await
    }
}
