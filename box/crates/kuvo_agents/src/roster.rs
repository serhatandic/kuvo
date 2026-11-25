//! One adapter instance per configured provider.

use crate::{
    account_check::{AccountStatus, CheckFailure, KnownAccount},
    accounts::{Account, Accounts, AgentKind, Provider},
    catalog::{ModelCache, Models},
    runtime::{AdapterCall, Adoption},
    session::{SessionContext, Thread},
    sign_in::{Attempt, Request},
};
use futures_util::future::BoxFuture;
use std::{collections::HashMap, io, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub trait Agent: Adoption {
    fn probe_account(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>>;
    fn models(&self, refresh: bool) -> AdapterCall<'_, Models>;

    /// Open or resume the conversation identified by this Kuvo session. After
    /// adoption this returns the thread associated with any recovered holder.
    fn open_thread(&self, _context: SessionContext) -> AdapterCall<'_, Arc<dyn Thread>> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This agent doesn't run sessions here.",
            ))
        })
    }

    /// Whether it summarizes its conversation when asked (not only by itself as it fills up).
    fn compacts(&self) -> bool {
        false
    }

    fn keep_account(&self) -> Duration {
        Duration::ZERO
    }

    fn remote_sign_in(&self) -> bool {
        false
    }

    fn sign_in(&self, _request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This agent signs in another way.",
            ))
        })
    }

    fn sign_in_with_key(&self, _key: String) -> AdapterCall<'_, ()> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This agent doesn't take a key here.",
            ))
        })
    }

    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This agent signs out another way.",
            ))
        })
    }

    /// Checkpoint the vendor state and reserve the replay range of live turns.
    /// Keep the holder alive when the resident lets go of its controller.
    fn prepare_replacement(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    /// Remove credentials the native agent keeps outside a managed account folder.
    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    /// Finish reading the current turn before returning from an intentional stop.
    /// Removal may call this again after a store operation is retried.
    fn shutdown(&self) -> AdapterCall<'_, ()>;

    /// Its program, and how it's updated (`program_updates`); none when it isn't the box's to.
    fn program(&self) -> Option<crate::program_updates::Definition> {
        None
    }

    /// Its program was updated: what runs the old one starts again once nothing uses it, and its
    /// models are asked anew.
    fn program_updated(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// Implemented once per vendor by rows 4.2–4.5. Create an independent
/// adapter for each account. A supplied home is its private managed folder;
/// with no supplied home, use the agent's native default sign-in. The
/// adapter owns protocol tasks and keeps recorded holders on resident exit.
pub trait Factory: Send + Sync {
    fn create(&self, account: &Account, home: Option<PathBuf>) -> io::Result<Arc<dyn Agent>>;
}

#[derive(Clone)]
pub struct Entry {
    pub account: Account,
    pub agent: Arc<dyn Agent>,
    known: Arc<KnownAccount>,
    pub(crate) model_cache: Arc<Mutex<ModelCache>>,
}

impl Entry {
    pub async fn status(&self, refresh: bool) -> Result<AccountStatus, CheckFailure> {
        self.known
            .read(refresh, || {
                let agent = self.agent.clone();
                async move { agent.probe_account().await }
            })
            .await
    }

    pub async fn credentials_changed(&self) {
        let mut models = self.model_cache.lock().await;
        self.known.reset().await;
        models.clear();
    }
}

#[derive(Clone, Default)]
pub struct Roster {
    entries: Vec<Entry>,
}

impl Roster {
    pub fn all(&self) -> &[Entry] {
        &self.entries
    }

    pub fn find(&self, provider: &Provider) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.account.provider() == *provider)
    }

    pub fn recovery_adapters(&self) -> HashMap<Provider, Arc<dyn Adoption>> {
        self.entries
            .iter()
            .map(|entry| {
                (
                    entry.account.provider(),
                    entry.agent.clone() as Arc<dyn Adoption>,
                )
            })
            .collect()
    }

    /// Reuse running instances when labels change. Only reconcile after the
    /// account index has been written successfully.
    pub async fn reconcile(
        &mut self,
        accounts: &Accounts,
        factories: &HashMap<AgentKind, Arc<dyn Factory>>,
    ) -> io::Result<()> {
        let mut next: Vec<Entry> = Vec::new();
        for account in accounts.all().await? {
            let Some(factory) = factories.get(&account.kind) else {
                continue;
            };
            if next
                .iter()
                .any(|entry| entry.account.provider() == account.provider())
            {
                continue;
            }
            if let Some(existing) = self.find(&account.provider()) {
                next.push(Entry {
                    account,
                    agent: existing.agent.clone(),
                    known: existing.known.clone(),
                    model_cache: existing.model_cache.clone(),
                });
                continue;
            }
            let agent = {
                match factory.create(&account, accounts.home(&account)) {
                    Ok(agent) => agent,
                    Err(error) => {
                        for pending in &next {
                            if self.find(&pending.account.provider()).is_none() {
                                let _ = pending.agent.shutdown().await;
                            }
                        }
                        return Err(error);
                    }
                }
            };
            next.push(Entry {
                account,
                known: Arc::new(KnownAccount::new(agent.keep_account())),
                agent,
                model_cache: Arc::new(Mutex::new(ModelCache::default())),
            });
        }

        let old = std::mem::replace(&mut self.entries, next);
        let mut failure = None;
        for entry in old {
            if self.find(&entry.account.provider()).is_none()
                && let Err(error) = entry.agent.shutdown().await
            {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
