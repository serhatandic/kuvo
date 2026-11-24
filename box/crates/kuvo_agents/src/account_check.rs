//! A failed agent probe does not invalidate a previously seen sign-in.

use std::{future::Future, time::Duration};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use tokio::{sync::Mutex, time::Instant};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountStatus {
    pub installed: bool,
    pub signed_in: bool,
    pub label: Option<String>,
    pub identity: Option<String>,
    pub label_describes_method: bool,
    pub message: Option<String>,
    pub ended: bool,
    pub unchecked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckFailure {
    TimedOut,
    Failed(String),
}

type Answer = Result<AccountStatus, CheckFailure>;
type Probe = Shared<BoxFuture<'static, Answer>>;

pub struct KnownAccount {
    keep: Duration,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    generation: u64,
    serial: u64,
    active: Option<Probe>,
    known: Option<(Instant, AccountStatus)>,
    failure: Option<(Instant, CheckFailure)>,
}

impl KnownAccount {
    pub fn new(keep: Duration) -> Self {
        Self {
            keep,
            state: Mutex::new(State::default()),
        }
    }

    pub async fn read<F, Fut>(&self, fresh: bool, check: F) -> Answer
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Answer> + Send + 'static,
    {
        loop {
            let (generation, serial, probe) = {
                let mut state = self.state.lock().await;
                if !fresh
                    && state.failure.is_none()
                    && let Some((at, known)) = &state.known
                    && at.elapsed() < self.keep
                {
                    return Ok(known.clone());
                }
                if !fresh
                    && let Some((at, failure)) = &state.failure
                    && at.elapsed() < Duration::from_secs(30)
                {
                    return unchecked(state.known.as_ref(), failure);
                }
                if state.active.is_none() {
                    state.serial += 1;
                    state.active = Some(check().boxed().shared());
                }
                (
                    state.generation,
                    state.serial,
                    state.active.clone().expect("active probe"),
                )
            };
            let result = probe.await;
            let mut state = self.state.lock().await;
            if generation != state.generation {
                continue;
            }
            if serial == state.serial && state.active.take().is_some() {
                match &result {
                    Ok(account) => {
                        state.known = Some((Instant::now(), account.clone()));
                        state.failure = None;
                    }
                    Err(failure) => state.failure = Some((Instant::now(), failure.clone())),
                }
            }
            return match result {
                Ok(account) => Ok(account),
                Err(failure) => unchecked(state.known.as_ref(), &failure),
            };
        }
    }

    /// Keep the last answer while asking again after a program update.
    pub async fn stale(&self) {
        let mut state = self.state.lock().await;
        if let Some((at, _)) = &mut state.known {
            *at -= self.keep;
        }
    }

    /// A sign-in change invalidates any probe started with the old credential.
    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        *state = State {
            generation: state.generation + 1,
            ..State::default()
        };
    }
}

fn unchecked(known: Option<&(Instant, AccountStatus)>, failure: &CheckFailure) -> Answer {
    if let Some((_, account)) = known {
        return Ok(AccountStatus {
            unchecked: true,
            ..account.clone()
        });
    }
    if *failure == CheckFailure::TimedOut {
        return Ok(AccountStatus {
            installed: true,
            signed_in: true,
            unchecked: true,
            label: None,
            identity: None,
            label_describes_method: false,
            message: None,
            ended: false,
        });
    }
    Err(failure.clone())
}
