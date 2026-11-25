//! Codex, driven through its app-server (`codex app-server`, JSON-RPC on its pipes): your own
//! `codex`, signed in with ChatGPT or an API key (what `codex login` keeps). One app-server serves
//! every conversation on an account, each one of its threads.

mod account;
mod asks;
mod citations;
mod program;
mod projection;
mod server;
mod thread;

use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use futures_util::future::{BoxFuture, join_all};
use kuvo_store::home::Home;
use tokio::sync::Mutex;

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::Account,
    catalog::Models,
    hold::{Controller, HoldKind, Record, TurnNote},
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption},
    session::{SessionContext, Thread},
    sign_in::{Attempt, Request},
    tool_relay::ToolRelay,
};

use server::Shared;

#[derive(Clone)]
pub struct CodexFactory {
    home: Home,
    holder_binary: PathBuf,
    relay: Option<ToolRelay>,
}

impl CodexFactory {
    /// `relay`: the resident's, for Kuvo Code's tools and the computer's MCP servers.
    pub fn new(home: Home, holder_binary: PathBuf, relay: Option<ToolRelay>) -> Self {
        Self {
            home,
            holder_binary,
            relay,
        }
    }
}

impl Factory for CodexFactory {
    fn create(&self, account: &Account, home: Option<PathBuf>) -> io::Result<Arc<dyn Agent>> {
        let program = program::Program::new(
            self.home.clone(),
            self.holder_binary.clone(),
            account.provider().to_string(),
            home,
        );
        Ok(Arc::new(Codex {
            shared: Shared::new(program)?,
            relay: self.relay.clone(),
            threads: Mutex::default(),
        }))
    }
}

struct Codex {
    shared: Arc<Shared>,
    relay: Option<ToolRelay>,
    threads: Mutex<HashMap<String, thread::Conversation>>,
}

impl Codex {
    /// Its conversations let go of, and their app-server stopped: they're loaded again in a new one
    /// as they're next used.
    async fn stop_all(&self, signed_out: bool) {
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
}

impl Agent for Codex {
    fn probe_account(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(self.shared.account.probe(&self.shared))
    }

    fn models(&self, refresh: bool) -> AdapterCall<'_, Models> {
        Box::pin(self.shared.account.models(&self.shared, refresh))
    }

    fn keep_account(&self) -> Duration {
        Duration::from_secs(30)
    }

    /// ChatGPT's device code works wherever its page is opened.
    fn remote_sign_in(&self) -> bool {
        true
    }

    fn compacts(&self) -> bool {
        true
    }

    fn sign_in(&self, request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        Box::pin(self.shared.account.sign_in(&self.shared, request))
    }

    fn sign_in_with_key(&self, key: String) -> AdapterCall<'_, ()> {
        Box::pin(self.shared.account.sign_in_with_key(&self.shared, key))
    }

    /// Its app-servers read the sign-in as they start: the next one, signed out. A turn running
    /// on it stops.
    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.shared.program.logout().await?;
            self.stop_all(true).await;
            self.shared.account.forget_models();
            Ok(())
        })
    }

    /// An added account signs out as it goes: Codex forgets the sign-in tied to its folder, the
    /// Keychain's or keyring's item too, before the folder goes.
    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.stop_all(false).await;
            if self.shared.program.added() {
                let _ = self.shared.program.logout().await;
            }
            Ok(())
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.stop_all(false).await;
            Ok(())
        })
    }

    /// The next resident goes on with its turns in the same held app-server: the holder keeps its
    /// lines from where the earliest began.
    fn prepare_replacement(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            if let Some(server) = self.shared.current().await
                && let Some(from) = server.earliest_turn()
                && !server.rpc.prepare(from).await
            {
                return Err(io::Error::other(
                    "Codex's holder couldn't keep its turns for the next resident.",
                ));
            }
            let threads: Vec<_> = self
                .threads
                .lock()
                .await
                .drain()
                .map(|(_, thread)| thread)
                .collect();
            join_all(threads.iter().map(thread::Conversation::handover)).await;
            self.shared.detach().await;
            Ok(())
        })
    }

    fn open_thread(&self, context: SessionContext) -> AdapterCall<'_, Arc<dyn Thread>> {
        Box::pin(async move {
            let mut threads = self.threads.lock().await;
            if let Some(thread) = threads.get(&context.session_id) {
                return Ok(Arc::new(thread.clone()) as Arc<dyn Thread>);
            }
            let session = context.session_id.clone();
            let thread =
                thread::Conversation::spawn(self.shared.clone(), self.relay.clone(), context);
            threads.insert(session, thread.clone());
            Ok(Arc::new(thread) as Arc<dyn Thread>)
        })
    }
}

impl Adoption for Codex {
    fn accepts(&self, record: &Record, turns: &BTreeMap<String, TurnNote>) -> bool {
        record.kind == HoldKind::JsonRpc
            && record.meta.get("kind").and_then(serde_json::Value::as_str) == Some("codex")
            && record
                .meta
                .get("provider")
                .and_then(serde_json::Value::as_str)
                == Some(&self.shared.program.provider)
            && record
                .meta
                .get("key")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .is_empty()
            && record.exited.is_none()
            && turns.values().all(|turn| turn.turn.is_some())
    }

    fn attach<'a>(
        &'a self,
        _: &'a Account,
        _record: Record,
        _turns: BTreeMap<String, TurnNote>,
        controller: Controller,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            self.shared.adopt(controller).await?;
            Ok(())
        })
    }

    /// A turn the resident won't take up: Codex is told to stop it, and the app-server's lines
    /// for the others come.
    fn let_go<'a>(&'a self, turn: &'a TurnNote) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            let thread = self.threads.lock().await.get(&turn.session).cloned();
            match thread {
                Some(thread) => thread.let_go(turn.clone()).await,
                None => self.shared.let_go(turn).await,
            }
        })
    }
}
