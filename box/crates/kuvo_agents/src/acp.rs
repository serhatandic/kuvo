//! Agents that speak ACP, the Agent Client Protocol (Grok's CLI, Antigravity's `agy`): one client
//! for all of them, each a flavour of its own. A process carries several conversations, each one
//! of its sessions, under one holder: conversations started the same way share it.

mod antigravity;
mod flavour;
mod grok;
mod local;
mod projection;
mod server;
mod thread;
mod tools;
mod wire;

use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use futures_util::future::{BoxFuture, join_all};
use kuvo_store::home::Home;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::{Account, AgentKind},
    catalog::Models,
    hold::{Controller, HoldKind, Record, TurnNote},
    program_updates::Definition,
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption},
    session::{SessionContext, Thread},
    sign_in::{Attempt, Request},
    tool_relay::ToolRelay,
};

pub use antigravity::forget_old_releases as forget_old_antigravity;
use flavour::Flavour;
pub use grok::{GrokSecret, grok_installed, grok_secret};
use server::Shared;
use thread::Conversation;

/// Makes each Grok or Antigravity account's agent.
#[derive(Clone)]
pub struct AcpFactory {
    kind: AgentKind,
    home: Home,
    holder_binary: PathBuf,
    relay: Option<ToolRelay>,
    feed: Option<crate::feed::Feed>,
}

impl AcpFactory {
    /// `kind`: Grok or Antigravity. `relay`: the resident's, for Kuvo Code's tools and the
    /// computer's MCP servers.
    pub fn new(
        kind: AgentKind,
        home: Home,
        holder_binary: PathBuf,
        relay: Option<ToolRelay>,
    ) -> Self {
        Self {
            kind,
            home,
            holder_binary,
            relay,
            feed: None,
        }
    }

    /// Where its accounts say their sign-in or models changed.
    pub fn with_feed(mut self, feed: crate::feed::Feed) -> Self {
        self.feed = Some(feed);
        self
    }
}

impl Factory for AcpFactory {
    fn create(&self, account: &Account, home: Option<PathBuf>) -> io::Result<Arc<dyn Agent>> {
        let provider = account.provider().to_string();
        let flavour: Arc<dyn Flavour> = match self.kind {
            AgentKind::Grok => Arc::new(grok::Grok::new(
                self.home.clone(),
                provider.clone(),
                home,
                self.feed.clone(),
            )),
            AgentKind::Antigravity => Arc::new(antigravity::Antigravity::new(
                self.home.clone(),
                provider.clone(),
                home,
                self.holder_binary.clone(),
                self.feed.clone(),
            )),
            other => {
                return Err(io::Error::other(format!(
                    "{} doesn't speak ACP here.",
                    other.display_name()
                )));
            }
        };
        Ok(Arc::new(Acp {
            shared: Shared::new(
                flavour,
                self.home.clone(),
                self.holder_binary.clone(),
                provider,
            ),
            relay: self.relay.clone(),
            threads: Mutex::default(),
        }))
    }
}

struct Acp {
    shared: Arc<Shared>,
    relay: Option<ToolRelay>,
    threads: Mutex<HashMap<String, Conversation>>,
}

impl Acp {
    fn flavour(&self) -> &dyn Flavour {
        self.shared.flavour.as_ref()
    }

    /// Its conversations let go of, and its processes stopped: opened again in new ones as
    /// they're next used.
    async fn stop_all(&self, signed_out: bool) {
        self.shared.halt();
        let threads: Vec<_> = self
            .threads
            .lock()
            .await
            .drain()
            .map(|(_, thread)| thread)
            .collect();
        join_all(threads.iter().map(|thread| thread.close(signed_out))).await;
        self.shared.shutdown().await;
    }

    async fn thread(&self, context: SessionContext) -> Conversation {
        let mut threads = self.threads.lock().await;
        threads
            .entry(context.session_id.clone())
            .or_insert_with(|| {
                Conversation::spawn(self.shared.clone(), self.relay.clone(), context)
            })
            .clone()
    }
}

impl Agent for Acp {
    fn probe_account(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        self.flavour().probe()
    }

    fn models(&self, refresh: bool) -> AdapterCall<'_, Models> {
        self.flavour().models(refresh)
    }

    fn keep_account(&self) -> Duration {
        Duration::from_secs(30)
    }

    /// Its page's code is pasted back here, or its device code entered, from any device.
    fn remote_sign_in(&self) -> bool {
        true
    }

    /// Grok summarizes when asked; Antigravity can't be.
    fn compacts(&self) -> bool {
        self.flavour().kind() == AgentKind::Grok
    }

    fn sign_in(&self, request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        self.flavour().sign_in(request)
    }

    /// Its processes keep the sign-in they started with: they stop, and a turn on one fails.
    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            let result = self.flavour().sign_out().await;
            self.stop_all(true).await;
            result
        })
    }

    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.stop_all(false).await;
            self.flavour().remove_credentials().await
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.stop_all(false).await;
            Ok(())
        })
    }

    fn program(&self) -> Option<Definition> {
        self.flavour().program()
    }

    /// Processes on the old program take no new conversations, and stop once quiet.
    fn program_updated(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.shared.retire_all().await;
            Ok(())
        })
    }

    /// The next resident goes on with the turns in the same held processes: each holder keeps
    /// its lines from where its earliest turn began.
    fn prepare_replacement(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            for process in self.shared.running().await {
                if let Some(from) = process.earliest_turn()
                    && !process.rpc.prepare(from).await
                {
                    return Err(io::Error::other(format!(
                        "{}'s holder couldn't keep its turns for the next resident.",
                        self.shared.name()
                    )));
                }
            }
            let threads: Vec<_> = self
                .threads
                .lock()
                .await
                .drain()
                .map(|(_, thread)| thread)
                .collect();
            join_all(threads.iter().map(Conversation::handover)).await;
            self.shared.detach().await;
            Ok(())
        })
    }

    fn open_thread(&self, context: SessionContext) -> AdapterCall<'_, Arc<dyn Thread>> {
        Box::pin(async move { Ok(Arc::new(self.thread(context).await) as Arc<dyn Thread>) })
    }
}

impl Adoption for Acp {
    fn accepts(&self, record: &Record, _turns: &BTreeMap<String, TurnNote>) -> bool {
        let meta = |key: &str| record.meta.get(key).and_then(Value::as_str);
        record.kind == HoldKind::JsonRpc
            && meta("kind") == Some(self.flavour().kind().name())
            && meta("provider") == Some(&self.shared.provider)
            && record.exited.is_none()
    }

    fn attach<'a>(
        &'a self,
        _: &'a Account,
        record: Record,
        _turns: BTreeMap<String, TurnNote>,
        controller: Controller,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            if record.meta.get("initialized") != Some(&Value::Bool(true)) {
                self.shared.discard(controller).await;
                return Ok(());
            }
            self.shared.adopt(controller).await?;
            Ok(())
        })
    }

    /// Conversations open in a process taken up, with no turn under way: theirs again as they
    /// are, with what they keep going.
    fn restore_idle<'a>(
        &'a self,
        store: &'a crate::session_store::SessionStore,
        record: &'a Record,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            let key = record.meta.get("key").and_then(Value::as_str).unwrap_or("");
            let process = self
                .shared
                .running()
                .await
                .into_iter()
                .find(|process| process.key == key);
            let (Some(process), Some(sessions)) = (
                process,
                record.meta.get("sessions").and_then(Value::as_object),
            ) else {
                return Ok(());
            };
            let turns = record.turns().unwrap_or_default();
            for (native, note) in sessions {
                let Some(session) = note["session"].as_str() else {
                    continue;
                };
                if turns.contains_key(native) {
                    continue;
                }
                let Some(context) = store.context(session, &self.shared.provider).await? else {
                    continue;
                };
                if context.state.get("acpSession").and_then(Value::as_str) != Some(native) {
                    continue;
                }
                self.thread(context)
                    .await
                    .restore_idle(process.clone(), note.clone())
                    .await?;
            }
            process.ready("background");
            Ok(())
        })
    }

    /// A turn the resident won't take up: cancelled in the agent, and the process's lines for the
    /// others come.
    fn let_go<'a>(&'a self, turn: &'a TurnNote) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            let thread = self.threads.lock().await.get(&turn.session).cloned();
            match thread {
                Some(thread) => thread.let_go(turn.clone()).await,
                None => self.shared.let_go(&turn.session).await,
            }
        })
    }
}
