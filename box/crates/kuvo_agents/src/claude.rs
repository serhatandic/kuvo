//! Claude Code's command line and stream-json protocol.

mod diagnostic;
mod model;
mod owned_paths;
mod permissions;
mod program;
mod projection;
mod thread;
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
};

#[derive(Clone)]
pub struct ClaudeFactory {
    home: Home,
    holder_binary: PathBuf,
}

impl ClaudeFactory {
    pub fn new(home: Home, holder_binary: PathBuf) -> Self {
        Self {
            home,
            holder_binary,
        }
    }
}

/// Older Macs ran Claude Console in Kuvo's own folder. Its native sign-in belongs to
/// that exact path, so register the folder without copying its contents.
pub async fn adopt_console_folder(home: &Home) -> io::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    const FOLDER: &str = "claude/console";
    let agents = home.root().join("agents");
    let directory = agents.join(FOLDER);
    let used = tokio::task::spawn_blocking(move || {
        if directory.ancestors().take(3).any(|folder| {
            !std::fs::symlink_metadata(folder).is_ok_and(|metadata| metadata.is_dir())
        }) {
            return false;
        }
        if directory.join(".claude.json").exists() || directory.join("projects").exists() {
            return true;
        }
        // Preserve any unrecognized files left by the old CLI.
        let _ = std::fs::remove_dir(directory);
        false
    })
    .await?;
    if used {
        crate::accounts::Accounts::for_home(home)
            .adopt_home(
                crate::accounts::AgentKind::Claude,
                FOLDER.into(),
                Some("Anthropic Console".into()),
            )
            .await?;
    }
    Ok(())
}

impl Factory for ClaudeFactory {
    fn create(&self, account: &Account, home: Option<PathBuf>) -> io::Result<Arc<dyn Agent>> {
        Ok(Arc::new(Claude {
            program: program::Program::new(
                self.home.clone(),
                self.holder_binary.clone(),
                account.id.clone(),
                home,
            ),
            provider: account.provider().to_string(),
            threads: Mutex::new(Threads::default()),
            models: model::Catalog::new()?,
        }))
    }
}

struct Claude {
    program: program::Program,
    models: model::Catalog,
    provider: String,
    threads: Mutex<Threads>,
}

#[derive(Default)]
struct Threads {
    open: HashMap<String, Arc<thread::Conversation>>,
    held: HashMap<String, thread::Recovery>,
}

impl Claude {
    async fn stop_threads(&self, shutdown: bool) -> io::Result<()> {
        let (open, held) = {
            let mut threads = self.threads.lock().await;
            let open: Vec<_> = if shutdown {
                threads.open.drain().map(|(_, thread)| thread).collect()
            } else {
                threads.open.values().cloned().collect()
            };
            (open, std::mem::take(&mut threads.held))
        };
        let mut failures = join_all(open.iter().map(|thread| thread.stop(shutdown)))
            .await
            .into_iter()
            .filter_map(Result::err);
        let held = join_all(held.into_values().map(thread::Recovery::stop)).await;
        failures
            .next()
            .or_else(|| held.into_iter().find_map(Result::err))
            .map_or(Ok(()), Err)
    }
}

impl Agent for Claude {
    fn probe_account(&self) -> BoxFuture<'_, Result<AccountStatus, CheckFailure>> {
        Box::pin(async {
            let (status, replaced) =
                self.program
                    .probe()
                    .await
                    .map_err(|error| match error.kind() {
                        io::ErrorKind::TimedOut => CheckFailure::TimedOut,
                        _ => CheckFailure::Failed(error.to_string()),
                    })?;
            if replaced {
                self.models.clear().await;
            }
            Ok(status)
        })
    }

    fn models(&self, refresh: bool) -> AdapterCall<'_, Models> {
        Box::pin(self.models.read(self.program.clone(), refresh))
    }

    fn keep_account(&self) -> Duration {
        Duration::from_secs(30)
    }

    fn remote_sign_in(&self) -> bool {
        true
    }

    fn sign_in(&self, request: Request) -> AdapterCall<'_, Arc<dyn Attempt>> {
        Box::pin(self.program.login(request, self.models.clone()))
    }

    fn sign_out(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.stop_threads(false).await?;
            self.program.logout().await?;
            self.models.clear().await;
            Ok(())
        })
    }

    fn remove_credentials(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.stop_threads(false).await?;
            self.program.logout().await?;
            self.models.clear().await;
            Ok(())
        })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(self.stop_threads(true))
    }

    fn prepare_replacement(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            let threads: Vec<_> = self.threads.lock().await.open.values().cloned().collect();
            join_all(threads.iter().map(|thread| thread.prepare()))
                .await
                .into_iter()
                .find_map(Result::err)
                .map_or(Ok(()), Err)
        })
    }

    fn open_thread(&self, context: SessionContext) -> AdapterCall<'_, Arc<dyn Thread>> {
        Box::pin(async move {
            let mut threads = self.threads.lock().await;
            if let Some(thread) = threads.open.get(&context.session_id) {
                return Ok(thread.clone() as Arc<dyn Thread>);
            }
            let session = context.session_id.clone();
            let recovery = threads.held.remove(&session);
            let thread = Arc::new(thread::Conversation::spawn(
                self.program.clone(),
                self.provider.clone(),
                context,
                recovery,
            ));
            threads.open.insert(session, thread.clone());
            Ok(thread as Arc<dyn Thread>)
        })
    }
}

impl Adoption for Claude {
    fn accepts(&self, record: &Record, turns: &BTreeMap<String, TurnNote>) -> bool {
        record.kind == HoldKind::ControlJson
            && record
                .meta
                .get("claudeAdapter")
                .and_then(serde_json::Value::as_u64)
                == Some(1)
            && record
                .meta
                .get("provider")
                .and_then(serde_json::Value::as_str)
                == Some(&self.provider)
            && record.meta.get("claudeReconnect") == Some(&serde_json::Value::Bool(true))
            && holder_session(record).is_some_and(|session| {
                (turns.is_empty() && record.exited.is_none())
                    || turns.len() == 1
                        && turns
                            .get("main")
                            .is_some_and(|note| note.session == session)
            })
    }

    fn attach<'a>(
        &'a self,
        _: &'a Account,
        record: Record,
        mut turns: BTreeMap<String, TurnNote>,
        controller: Controller,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            let session = holder_session(&record)
                .ok_or_else(|| io::Error::other("Claude Code holder has no saved session."))?;
            let note = turns.remove("main");
            let mut threads = self.threads.lock().await;
            if threads.open.contains_key(session) || threads.held.contains_key(session) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "This conversation already has a Claude Code holder.",
                ));
            }
            threads
                .held
                .insert(session.into(), thread::Recovery::new(controller, note));
            Ok(())
        })
    }
}

fn holder_session(record: &Record) -> Option<&str> {
    let id = record.meta.get("session")?.as_str()?;
    (id.starts_with("ses_")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(id)
}
