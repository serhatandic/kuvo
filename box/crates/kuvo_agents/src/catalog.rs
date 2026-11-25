//! Model and account snapshots attributed to their owning provider.

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::Account,
    roster::{Entry, Roster},
};
use futures_util::future::join_all;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Models {
    pub generation: Option<String>,
    pub complete: bool,
    pub models: Vec<Model>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub efforts: Vec<String>,
    pub speeds: Vec<Speed>,
    pub hidden: bool,
    pub retiring: bool,
    pub provisional: bool,
    /// How much it holds, in tokens, where its agent says (or its list does).
    pub context: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Speed {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

pub struct CatalogEntry {
    pub account: Account,
    pub status: Result<AccountStatus, CheckFailure>,
    pub models: Option<Models>,
    pub unavailable: bool,
}

#[derive(Default)]
pub(crate) struct ModelCache {
    epoch: u64,
    next: u64,
    applied: u64,
    last: Option<Models>,
    identity: Option<String>,
}

impl ModelCache {
    pub fn clear(&mut self) {
        *self = Self {
            epoch: self.epoch + 1,
            ..Self::default()
        };
    }
}

impl Roster {
    pub async fn catalog(&self, refresh: bool) -> Vec<CatalogEntry> {
        join_all(self.all().iter().map(|entry| entry.catalog(refresh))).await
    }
}

impl Entry {
    /// Whether its last list is every model it runs (one missing from it is gone).
    pub async fn lists_every_model(&self) -> bool {
        self.model_cache
            .lock()
            .await
            .last
            .as_ref()
            .is_some_and(|models| models.complete)
    }

    async fn catalog(&self, refresh: bool) -> CatalogEntry {
        loop {
            let (epoch, read) = {
                let mut cache = self.model_cache.lock().await;
                cache.next += 1;
                (cache.epoch, cache.next)
            };
            let status = self.status(refresh).await;
            let runnable = status
                .as_ref()
                .is_ok_and(|status| status.installed && status.signed_in);
            let answer = if runnable {
                Some(self.agent.models(refresh).await)
            } else {
                None
            };
            let mut cache = self.model_cache.lock().await;
            if cache.epoch != epoch {
                continue;
            }
            let unavailable = answer.as_ref().is_some_and(Result::is_err);
            if read >= cache.applied {
                cache.applied = read;
                if let Ok(status) = &status
                    && let Some(identity) = status.identity.as_ref().or(status
                        .label
                        .as_ref()
                        .filter(|_| !status.label_describes_method))
                    && cache.identity.as_ref() != Some(identity)
                {
                    cache.last = None;
                    cache.identity = Some(identity.clone());
                }
                match answer {
                    Some(Ok(models)) => cache.last = Some(models),
                    None => cache.last = None,
                    Some(Err(_)) => {}
                }
            }
            let mut models = runnable.then(|| cache.last.clone()).flatten();
            if unavailable && let Some(models) = &mut models {
                models.complete = false;
            }
            return CatalogEntry {
                account: self.account.clone(),
                status,
                models,
                unavailable,
            };
        }
    }
}
