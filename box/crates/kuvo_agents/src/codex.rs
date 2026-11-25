//! Codex, driven through its app-server (`codex app-server`, JSON-RPC on its pipes): your own
//! `codex`, signed in with ChatGPT or an API key (what `codex login` keeps). One app-server serves
//! every conversation on an account, each one of its threads.

mod account;
mod asks;
mod citations;
mod program;
mod projection;
mod server;
mod terminals;
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
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::Account,
    catalog::Models,
    hold::{Controller, HoldKind, Record, TurnNote},
    program_updates::{Definition, Native},
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption},
    session::{SessionContext, Thread},
    sign_in::{Attempt, Request},
    tool_relay::ToolRelay,
};

use server::Shared;

pub(crate) use asks::elicitation_field;

/// A conversation's replies kept from before Codex's directives in its words were read (an older
/// bridge kept them as words), and its title with one, as they read now: read once, as the
/// conversation is first needed (or as the resident starts, for one whose turn goes on). Whether
/// anything changed.
pub async fn read_kept_citations(
    store: &crate::session_store::SessionStore,
    id: &str,
) -> io::Result<bool> {
    let replies: Vec<_> = store
        .rows_with(id, ":codex-")
        .await?
        .iter()
        .filter_map(citations::kept_reply)
        .collect();
    let mut changed = !replies.is_empty();
    if changed {
        store.write_rows(id, replies).await?;
    }
    let Some(header) = store.read(id).await? else {
        return Ok(changed);
    };
    let fields = Value::Object(header.into_fields());
    let codex = fields["model"]["providerID"]
        .as_str()
        .and_then(crate::accounts::Provider::parse)
        .is_some_and(|provider| provider.kind == crate::accounts::AgentKind::Codex);
    if let Some(title) = fields["title"]
        .as_str()
        .filter(|title| codex && title.contains(":codex-"))
    {
        let plain = citations::plain(title);
        changed |= store
            .change_header(id, move |header| {
                header.insert("title".into(), plain.into());
            })
            .await?;
    }
    Ok(changed)
}

#[derive(Clone)]
pub struct CodexFactory {
    home: Home,
    holder_binary: PathBuf,
    relay: Option<ToolRelay>,
    feed: Option<crate::feed::Feed>,
}

impl CodexFactory {
    /// `relay`: the resident's, for Kuvo Code's tools and the computer's MCP servers.
    pub fn new(home: Home, holder_binary: PathBuf, relay: Option<ToolRelay>) -> Self {
        Self {
            home,
            holder_binary,
            relay,
            feed: None,
        }
    }

    /// Where its accounts say their sign-in changed.
    pub fn with_feed(mut self, feed: crate::feed::Feed) -> Self {
        self.feed = Some(feed);
        self
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
        let shared = Shared::new(program)?;
        if let Some(feed) = &self.feed {
            shared
                .account
                .tell_on(feed.clone(), account.provider().to_string());
        }
        Ok(Arc::new(Codex {
            shared,
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
        // (A start under way fails first: a thread waiting on it is closed next.)
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
}

impl Agent for Codex {
    fn plan_sign_in(&self) -> BoxFuture<'_, Option<(String, String)>> {
        Box::pin(async {
            let credentials = self.shared.program.credentials().await?;
            let mark = program::fingerprint(&credentials.token);
            let identity =
                program::chatgpt_identity(&credentials.token, credentials.account.as_deref())?;
            Some((mark, identity))
        })
    }

    fn read_plan_limits(
        &self,
        explicit: bool,
    ) -> AdapterCall<'_, Option<crate::session::DirectPlanReading>> {
        Box::pin(async move {
            let Some((_, identity)) = self.plan_sign_in().await else {
                return Ok(None);
            };
            let server = if explicit {
                self.shared.server().await?
            } else if let Some(server) = self.shared.current().await {
                server
            } else {
                return Ok(None);
            };
            if server
                .signed_in_as
                .lock()
                .ok()
                .and_then(|id| id.clone())
                .as_deref()
                != Some(identity.as_str())
            {
                return Ok(None);
            }
            let value = server
                .rpc
                .request(
                    "account/rateLimits/read",
                    Some(serde_json::json!({})),
                    Some(Duration::from_secs(20)),
                )
                .await
                .map_err(io::Error::other)?;
            let Some((mark, current)) = self.plan_sign_in().await else {
                return Ok(None);
            };
            if current != identity {
                return Ok(None);
            }
            let mut windows = vec![];
            if let Some(by_id) = value["rateLimitsByLimitId"].as_object() {
                for (id, snapshot) in by_id {
                    let mut snapshot = snapshot.clone();
                    if !snapshot.is_object() {
                        continue;
                    }
                    snapshot["limitId"] = serde_json::json!(id);
                    windows.extend(account::windows(&snapshot));
                }
                if !by_id.contains_key("codex") {
                    windows.extend(account::windows(&value["rateLimits"]));
                }
            } else {
                windows.extend(account::windows(&value["rateLimits"]));
            }
            let mut direct =
                crate::session::DirectPlanReading::windows(crate::session::PlanLimitReading {
                    windows,
                    sign_in: Some(mark),
                });
            let main = value["rateLimitsByLimitId"]
                .get("codex")
                .unwrap_or(&value["rateLimits"]);
            direct.plan = main["planType"].as_str().map(account::plan_name);
            if main["credits"]["hasCredits"] == true {
                let balance = &main["credits"]["balance"];
                direct.credits = balance
                    .as_f64()
                    .or_else(|| balance.as_str()?.parse().ok())
                    .filter(|n| n.is_finite())
                    .map(|n| n.max(0.0).floor());
            }
            if let Some(count) = value["rateLimitResetCredits"]["availableCount"].as_u64() {
                let now = jiff::Timestamp::now().as_millisecond().max(0) as u64;
                let mut targets = value["rateLimitResetCredits"]["credits"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|credit| {
                        if credit["status"].as_str().is_some_and(|s| s != "available") {
                            return None;
                        }
                        let id = credit["id"].as_str()?.to_owned();
                        if id.is_empty()
                            || id.len() > 100
                            || !id
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                        {
                            return None;
                        }
                        let expires_at =
                            credit["expiresAt"].as_u64().map(|n| n.saturating_mul(1000));
                        if expires_at.is_some_and(|at| at <= now) {
                            return None;
                        }
                        Some(crate::session::ResetTarget { id, expires_at })
                    })
                    .collect::<Vec<_>>();
                targets.sort_by_key(|t| t.expires_at.unwrap_or(u64::MAX));
                direct.resets = Some(crate::session::ResetInventory {
                    count: count.min(u64::from(u32::MAX)) as u32,
                    targets,
                    refills: vec![],
                });
            }
            Ok(Some(direct))
        })
    }

    fn consume_plan_reset<'a>(
        &'a self,
        identity: &'a str,
        request: &'a str,
        target: &'a str,
    ) -> AdapterCall<'a, crate::session::ResetOutcome> {
        Box::pin(async move {
            use crate::session::ResetOutcome;
            let server = match self.shared.server().await {
                Ok(server) => server,
                Err(_) => return Ok(ResetOutcome::Failed),
            };
            if server
                .signed_in_as
                .lock()
                .ok()
                .and_then(|id| id.clone())
                .as_deref()
                != Some(identity)
            {
                return Ok(ResetOutcome::NotNow);
            }
            // The server may have kept the previous login in memory. Ask it, not just
            // the credential file, before a request can spend an account's credit.
            let maker = server
                .rpc
                .request(
                    "account/rateLimits/read",
                    Some(serde_json::json!({"excludeResetCreditDetails":true})),
                    Some(Duration::from_secs(20)),
                )
                .await;
            let Some(maker) = maker
                .ok()
                .and_then(|m| m["accountId"].as_str().map(str::to_ascii_lowercase))
            else {
                return Ok(ResetOutcome::NotNow);
            };
            let expected = identity
                .strip_prefix("chatgpt:")
                .and_then(|id| id.split('/').next());
            if expected != Some(maker.as_str())
                || self
                    .plan_sign_in()
                    .await
                    .is_none_or(|(_, raw)| raw != identity)
            {
                return Ok(ResetOutcome::NoReset);
            }
            let answer = server
                .rpc
                .request(
                    "account/rateLimitResetCredit/consume",
                    Some(serde_json::json!({"idempotencyKey":request,"creditId":target})),
                    Some(Duration::from_secs(20)),
                )
                .await;
            Ok(
                match answer.as_ref().ok().and_then(|a| a["outcome"].as_str()) {
                    Some("reset") => ResetOutcome::Reset,
                    Some("alreadyRedeemed") => ResetOutcome::AlreadyRedeemed,
                    Some("nothingToReset") => ResetOutcome::NothingToReset,
                    Some("noCredit") => ResetOutcome::NoReset,
                    _ => ResetOutcome::Unknown,
                },
            )
        })
    }

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
            self.shared.account.clear_catalog(&self.shared).await;
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
            self.shared.account.clear_catalog(&self.shared).await;
            Ok(())
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.stop_all(false).await;
            Ok(())
        })
    }

    /// Updated by its own updater (`codex update`) where its installer put it, else by npm's.
    fn program(&self) -> Option<Definition> {
        let env = self.shared.program.environment();
        Some(Definition {
            name: "Codex".into(),
            executable: self.shared.program.executable(&env)?.path().to_owned(),
            npm_package: Some("@openai/codex".into()),
            native: Some(Native {
                args: vec!["update".into()],
                markers: vec!["/.codex/".into()],
                dist_tag: None,
            }),
        })
    }

    fn program_updated(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.shared.program_updated().await;
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
        record: Record,
        _turns: BTreeMap<String, TurnNote>,
        controller: Controller,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            if record.meta.get("initialized") == Some(&Value::Bool(false)) {
                self.shared.discard(controller).await;
                return Ok(());
            }
            self.shared.adopt(controller).await?;
            Ok(())
        })
    }

    fn restore_idle<'a>(
        &'a self,
        store: &'a crate::session_store::SessionStore,
        record: &'a Record,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            let Some(saved) = record
                .meta
                .get("backgroundSessions")
                .and_then(Value::as_object)
            else {
                return Ok(());
            };
            let result = async {
                let active = record.turns()?;
                for (session, snapshot) in saved {
                    if active.values().any(|turn| &turn.session == session) {
                        continue;
                    }
                    let Some(context) = store
                        .context(session, &self.shared.program.provider)
                        .await?
                    else {
                        continue;
                    };
                    let thread = {
                        let mut threads = self.threads.lock().await;
                        threads
                            .entry(session.clone())
                            .or_insert_with(|| {
                                thread::Conversation::spawn(
                                    self.shared.clone(),
                                    self.relay.clone(),
                                    context,
                                )
                            })
                            .clone()
                    };
                    thread.restore_idle(snapshot.clone()).await?;
                }
                Ok(())
            }
            .await;
            if let Some(server) = self.shared.current().await {
                server.ready("background");
            }
            result
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
