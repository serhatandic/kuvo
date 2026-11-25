//! Wake-ups and watches (PROTOCOL.md, `kuvo/wakes`): a conversation's or a bot's, kept in its
//! folder as `kuvo-wake` keeps them, listed, added, held, cancelled and sent now from any client,
//! and sent at their time by the resident itself, with no client connected.
//!
//! The TypeScript box ran `kuvo-wake fire`, a shell script, every minute. The Rust resident runs
//! the same round in-process (`fire`), on the same files, so an agent's own `kuvo-wake add` keeps
//! working beside it, and native Windows needs no shell for its wake-ups.

pub mod check;
pub mod clock;
pub(crate) mod fire;
pub mod folder;
mod inbox;
mod routes;
pub mod verbs;

use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use kuvo_engine::Engine;
use kuvo_server::Registry;
use tokio::{sync::watch, task::JoinHandle};

pub use fire::{next_wake, now};
pub use folder::{Listing, Roots};

/// What these routes answer whole.
pub const FEATURES: &[&str] = &["wakes", "wakes.edit", "wakes.events"];

/// How often the resident looks whether a wake-up came due (one an agent's `kuvo-wake` wrote
/// beside the box included), and how often watches and held folders are looked at regardless.
const LOOK: Duration = Duration::from_secs(2);
const ROUND: Duration = Duration::from_secs(60);

/// This computer's wake-ups and watches.
#[derive(Clone)]
pub struct Wakes {
    inner: Arc<Inner>,
}

struct Inner {
    roots: Roots,
    engine: Engine,
    changes: watch::Sender<u64>,
    /// Wake-ups being sent with Wake Up Now, so a second tap doesn't send one twice.
    waking_now: Mutex<HashSet<String>>,
}

impl Wakes {
    pub fn new(home: &Path, engine: Engine) -> Self {
        Self {
            inner: Arc::new(Inner {
                roots: Roots::of(home),
                engine,
                changes: watch::channel(0).0,
                waking_now: Mutex::new(HashSet::new()),
            }),
        }
    }

    pub fn roots(&self) -> &Roots {
        &self.inner.roots
    }

    /// Tells those following wake-ups (and bots) that some changed here.
    pub fn changed(&self) {
        self.inner.changes.send_modify(|count| *count += 1);
    }

    /// A change made here, as it's made.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    pub fn register(&self, routes: &mut Registry) {
        routes::register(self.clone(), routes);
    }

    /// Sends this computer's wake-ups and watches at their time, until stopped.
    pub fn start(&self) -> Firing {
        let (stop, stopping) = watch::channel(false);
        let task = tokio::spawn(self.clone().fire(stopping));
        Firing { stop, task }
    }

    async fn fire(self, mut stopping: watch::Receiver<bool>) {
        let env = check::environment();
        let mut changes = self.changes();
        let mut last = (Instant::now(), fire::now());
        let mut watching: Option<JoinHandle<()>> = None;
        loop {
            let roots = self.inner.roots.clone();
            let now = fire::now();
            let due = tokio::task::spawn_blocking(move || next_wake(&roots))
                .await
                .ok()
                .flatten();
            // One still due after a round it was due before (it couldn't go) waits a minute.
            let due_now = due.is_some_and(|due| due <= now as f64 && due > last.1 as f64);
            if due_now || last.0.elapsed() >= ROUND {
                last = (Instant::now(), now);
                self.round(now, &env, &mut watching).await;
            }
            tokio::select! {
                _ = stopping.changed() => break,
                () = tokio::time::sleep(LOOK) => {}
                _ = changes.changed() => tokio::time::sleep(Duration::from_millis(200)).await,
            }
            if *stopping.borrow() {
                break;
            }
        }
        if let Some(watching) = watching {
            watching.abort();
        }
    }

    async fn round(
        &self,
        now: i64,
        env: &kuvo_os::platform::Environment,
        watching: &mut Option<JoinHandle<()>>,
    ) {
        let roots = self.inner.roots.clone();
        let (engine, agents) = tokio::task::spawn_blocking(move || {
            (fire::wanted(&roots, now), fire::agents_wanted(&roots, now))
        })
        .await
        .unwrap_or_default();
        // The engine started for its own conversations; the coding agents, in this box, reached
        // with it or alone.
        let engine = match engine {
            true => self.inner.engine.wake_engine(true).await,
            false => None,
        };
        let engine = engine.or_else(|| {
            agents
                .then(|| self.inner.engine.agents_wake_engine())
                .flatten()
        });
        let Some(engine) = engine else {
            return;
        };
        let engine = Arc::new(engine);
        fire::Round {
            roots: &self.inner.roots,
            reach: engine.as_ref(),
            now,
        }
        .wakes()
        .await;
        self.changed();
        if watching.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let (wakes, env) = (self.clone(), env.clone());
        let home = self.inner.roots.run.parent().map(Path::to_owned);
        *watching = Some(tokio::spawn(async move {
            let fence = home.as_deref().map_or(check::Fence::Off, check::Fence::of);
            fire::Round {
                roots: &wakes.inner.roots,
                reach: engine.as_ref(),
                now,
            }
            .watches(&fence, &env)
            .await;
            wakes.changed();
        }));
    }
}

/// The resident's wake-up rounds; `stop` lets a round already going finish first.
pub struct Firing {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Firing {
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}
