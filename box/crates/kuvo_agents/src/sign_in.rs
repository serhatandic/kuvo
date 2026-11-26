//! Sign-in attempts are scoped to the initiating provider and live in the resident.

pub mod command;

use std::{collections::HashMap, io, sync::Arc, time::Duration};

use serde::Serialize;
use tokio::time::Instant;

use crate::{accounts::Provider, runtime::AdapterCall};

const LIFETIME: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Auto,
    Code,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub url: String,
    pub instructions: String,
    pub mode: Mode,
    pub accepts_code: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Status {
    Pending {
        #[serde(flatten)]
        page: Page,
    },
    Complete,
    Failed {
        message: String,
    },
    Expired,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub remote: bool,
    pub browser_return: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    #[serde(rename = "attemptID")]
    pub id: String,
    #[serde(flatten)]
    pub page: Page,
    pub time: Time,
}

#[derive(Serialize)]
pub struct Time {
    pub created: u64,
    pub expires: u64,
}

impl Time {
    pub(crate) fn now() -> Self {
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            created,
            expires: created.saturating_add(LIFETIME.as_millis() as u64),
        }
    }
}

pub trait Attempt: Send + Sync {
    /// A page may arrive after the program has been downloaded and launched.
    fn status(&self) -> Status;
    fn submit_code(&self, code: String) -> AdapterCall<'_, ()>;
    fn cancel(&self);
}

struct Pending {
    provider: Provider,
    deadline: Instant,
    attempt: Arc<dyn Attempt>,
}

#[derive(Default)]
pub struct SignIns {
    attempts: HashMap<String, Pending>,
}

impl SignIns {
    pub fn track(
        &mut self,
        provider: Provider,
        attempt: Arc<dyn Attempt>,
        now: Instant,
    ) -> io::Result<String> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let id = format!("att_{}", hex::encode(random));
        self.attempts.insert(
            id.clone(),
            Pending {
                provider,
                deadline: now + LIFETIME,
                attempt,
            },
        );
        Ok(id)
    }

    pub fn waiting(&self, now: Instant) -> bool {
        self.attempts.values().any(|entry| {
            now < entry.deadline && matches!(entry.attempt.status(), Status::Pending { .. })
        })
    }

    pub fn status(&mut self, provider: &Provider, id: &str, now: Instant) -> Option<Status> {
        let entry = self
            .attempts
            .get(id)
            .filter(|entry| entry.provider == *provider)?;
        if now >= entry.deadline {
            self.cancel(provider, id);
            return Some(Status::Expired);
        }
        let status = entry.attempt.status();
        if !matches!(status, Status::Pending { .. }) {
            self.attempts.remove(id);
        }
        Some(status)
    }

    /// Clone the adapter handle before awaiting the code submission so the
    /// resident can still answer status and cancellation requests.
    pub fn for_code(
        &self,
        provider: &Provider,
        id: &str,
        now: Instant,
    ) -> io::Result<Arc<dyn Attempt>> {
        let entry = self
            .attempts
            .get(id)
            .filter(|entry| entry.provider == *provider && now < entry.deadline)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "That sign-in is over. Start it again.",
                )
            })?;
        if let Status::Pending { page } = entry.attempt.status()
            && (page.mode == Mode::Code || page.accepts_code)
        {
            return Ok(entry.attempt.clone());
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "This sign-in finishes in the browser.",
        ))
    }

    pub fn cancel(&mut self, provider: &Provider, id: &str) -> bool {
        if self
            .attempts
            .get(id)
            .is_some_and(|entry| entry.provider == *provider)
            && let Some(entry) = self.attempts.remove(id)
        {
            entry.attempt.cancel();
            return true;
        }
        false
    }

    pub fn expire(&mut self, now: Instant) {
        self.attempts.retain(|_, entry| {
            if now < entry.deadline + Duration::from_secs(60) {
                return true;
            }
            entry.attempt.cancel();
            false
        });
    }

    pub fn next_expiry(&self) -> Option<Instant> {
        self.attempts.values().map(|entry| entry.deadline + Duration::from_secs(60)).min()
    }
}

impl Drop for SignIns {
    fn drop(&mut self) {
        for entry in self.attempts.values() {
            entry.attempt.cancel();
        }
    }
}
