//! Claude Code's command line and stream-json protocol.

mod diagnostic;
mod grants;
mod model;
mod owned_paths;
mod permissions;
mod program;
mod projection;
mod results;
mod tasks;
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
    accounts::Account,
    catalog::Models,
    hold::{Controller, HoldKind, Record, TurnNote},
    program_updates::{Definition, Native},
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption},
    session::{SessionContext, Thread},
    sign_in::{Attempt, Request},
};

#[derive(Clone)]
pub struct ClaudeFactory {
    home: Home,
    holder_binary: PathBuf,
    feed: Option<crate::feed::Feed>,
}

impl ClaudeFactory {
    pub fn new(home: Home, holder_binary: PathBuf) -> Self {
        Self {
            home,
            holder_binary,
            feed: None,
        }
    }

    pub fn with_feed(mut self, feed: crate::feed::Feed) -> Self {
        self.feed = Some(feed);
        self
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
        let program = program::Program::new(
            self.home.clone(),
            self.holder_binary.clone(),
            account.id.clone(),
            home,
        );
        let models = model::Catalog::new(program.catalog_file(), self.feed.clone())?;
        Ok(Arc::new(Claude {
            program,
            provider: account.provider().to_string(),
            threads: Mutex::new(Threads::default()),
            grants: grants::Reader::default(),
            models,
        }))
    }
}

/// The channel Claude Code's own updater follows, as its settings say.
fn update_channel(env: &kuvo_os::platform::Environment) -> &'static str {
    use kuvo_os::platform::{Platform, env_value};
    let platform = Platform::current();
    let user = if platform == Platform::Windows {
        "USERPROFILE"
    } else {
        "HOME"
    };
    let folder = env_value(platform, env, "CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| env_value(platform, env, user).map(|home| PathBuf::from(home).join(".claude")));
    let stable = folder
        .and_then(|folder| std::fs::read(folder.join("settings.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|settings| settings["autoUpdatesChannel"] == "stable");
    if stable { "stable" } else { "latest" }
}

struct Claude {
    program: program::Program,
    models: model::Catalog,
    provider: String,
    threads: Mutex<Threads>,
    grants: grants::Reader,
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
    fn plan_sign_in(&self) -> BoxFuture<'_, Option<(String, String)>> {
        Box::pin(self.program.plan_sign_in())
    }

    fn read_plan_limits(
        &self,
        explicit: bool,
    ) -> AdapterCall<'_, Option<crate::session::DirectPlanReading>> {
        Box::pin(async move {
            let Some((before, identity)) = self.plan_sign_in().await else {
                return Ok(None);
            };
            // No passive plan read starts a Claude process. File-owned Linux
            // grants can be refreshed at their own hourly maker limit.
            if !explicit {
                let Some(resets) = self.grants.limits(&self.program).await else {
                    return Ok(None);
                };
                if self
                    .plan_sign_in()
                    .await
                    .as_ref()
                    .is_none_or(|(mark, who)| mark != &before || who != &identity)
                {
                    return Ok(None);
                }
                let mut direct =
                    crate::session::DirectPlanReading::windows(crate::session::PlanLimitReading {
                        windows: vec![],
                        sign_in: Some(before),
                    });
                direct.only_resets = true;
                direct.resets = Some(resets);
                return Ok(Some(direct));
            }
            let result = model::usage(self.program.clone()).await;
            let Some((mark, current)) = self.plan_sign_in().await else {
                return Ok(None);
            };
            if identity != current {
                return Ok(None);
            }
            use crate::session::{
                DirectPlanReading, DirectPlanStatus, LimitWindow, PlanLimitReading, PlanSpend,
            };
            // A failed read is still an observation of this confirmed account.
            // The previous windows and extras remain useful until a fresh answer.
            let answer = match result {
                Ok(answer) => answer,
                Err(error) => {
                    eprintln!("Could not refresh Claude Code limits: {error}");
                    let mut direct = DirectPlanReading::windows(PlanLimitReading {
                        windows: vec![],
                        sign_in: Some(mark),
                    });
                    direct.status = DirectPlanStatus::Failed;
                    return Ok(Some(direct));
                }
            };
            let rate = &answer["rate_limits"];
            let mut windows = vec![];
            let timestamp = |value: &Value| {
                value
                    .as_str()
                    .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                    .and_then(|at| at.as_millisecond().try_into().ok())
            };
            let number = |value: &Value| {
                value
                    .as_f64()
                    .or_else(|| value.as_str()?.parse::<f64>().ok())
                    .filter(|v| v.is_finite())
            };
            let mut add = |id: &str, v: &Value, minutes: u32, model: Option<String>| {
                if let Some(used) = number(&v["utilization"]) {
                    if windows.iter().any(|w: &LimitWindow| w.id == id) {
                        return;
                    }
                    windows.push(LimitWindow {
                        id: id.into(),
                        used: used / 100.0,
                        minutes: Some(minutes),
                        resets_at: timestamp(&v["resets_at"]),
                        model,
                    });
                }
            };
            add("five_hour", &rate["five_hour"], 300, None);
            add("seven_day", &rate["seven_day"], 10080, None);
            add(
                "seven_day_sonnet",
                &rate["seven_day_sonnet"],
                10080,
                Some("Sonnet".into()),
            );
            add(
                "seven_day_opus",
                &rate["seven_day_opus"],
                10080,
                Some("Opus".into()),
            );
            for scope in rate["model_scoped"].as_array().into_iter().flatten() {
                if let Some(model) = scope["display_name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                {
                    let id = format!(
                        "seven_day_{}",
                        model
                            .to_ascii_lowercase()
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                            .collect::<String>()
                    );
                    add(&id, scope, 10080, Some(model.into()));
                }
            }
            let mut direct = DirectPlanReading::windows(PlanLimitReading {
                windows,
                sign_in: Some(mark),
            });
            if answer["rate_limits_available"] == false {
                direct.status = DirectPlanStatus::NoPlan;
            }
            direct.plan = answer["subscription_type"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let mut chars = s.chars();
                    format!(
                        "{}{}",
                        chars.next().unwrap().to_uppercase(),
                        chars.as_str().to_lowercase()
                    )
                });
            let extra = &rate["extra_usage"];
            if extra["is_enabled"] == true
                && let Some(used) = number(&extra["used_credits"])
            {
                direct.extra_usage = Some(PlanSpend {
                    amount: used.max(0.0) / 100.0,
                    limit: number(&extra["monthly_limit"])
                        .filter(|n| *n > 0.0)
                        .map(|n| n / 100.0),
                    resets_at: timestamp(&extra["resets_at"]),
                });
            }
            direct.resets = self.grants.limits(&self.program).await;
            if direct.resets.is_none() && cfg!(target_os = "macos") {
                direct.reset_unreadable=Some("Claude Code keeps this Mac's sign-in in the Keychain, so its resets show only from a Linux computer of yours signed in to Claude Code with the same account.".into());
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
            Ok(self
                .grants
                .consume(&self.program, identity, request, target)
                .await)
        })
    }
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

    fn compacts(&self) -> bool {
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

    /// Its own installer puts it in `~/.local/share/claude` (or `~/.claude/local`): `claude
    /// update`, on the channel its settings follow. Otherwise npm's.
    fn program(&self) -> Option<Definition> {
        let env = self.program.environment(false);
        Some(Definition {
            name: "Claude Code".into(),
            executable: self.program.executable(&env)?.path().to_owned(),
            npm_package: Some("@anthropic-ai/claude-code".into()),
            native: Some(Native {
                args: vec!["update".into()],
                markers: vec!["/.local/share/claude/".into(), "/.claude/local/".into()],
                dist_tag: Some(update_channel(&env).into()),
            }),
        })
    }

    /// Its models are asked of the new Claude Code; each conversation's starts again at its
    /// next message.
    fn program_updated(&self) -> AdapterCall<'_, ()> {
        Box::pin(async {
            self.program.updated();
            self.models.clear().await;
            Ok(())
        })
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
                self.models.clone(),
                self.provider.clone(),
                context,
                recovery,
            ));
            threads.open.insert(session, thread.clone());
            Ok(thread as Arc<dyn Thread>)
        })
    }

    fn close_thread(&self, session: &str) -> AdapterCall<'_, ()> {
        let session = session.to_owned();
        Box::pin(async move {
            let (open, held) = {
                let mut threads = self.threads.lock().await;
                (threads.open.remove(&session), threads.held.remove(&session))
            };
            if let Some(held) = held {
                held.stop().await?;
            }
            match open {
                Some(thread) => thread.stop(false).await,
                None => Ok(()),
            }
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

    fn restore_idle<'a>(
        &'a self,
        store: &'a crate::session_store::SessionStore,
        record: &'a Record,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            if record
                .meta
                .get("claudeTasks")
                .is_none_or(serde_json::Value::is_null)
                || !record.turns()?.is_empty()
            {
                return Ok(());
            }
            if let Some(session) = holder_session(record)
                && let Some(context) = store.context(session, &self.provider).await?
            {
                self.open_thread(context).await?;
            }
            Ok(())
        })
    }

    /// A turn the resident won't take up (its end was written as the last resident went): the
    /// Claude Code left in it goes, so the conversation's next message starts one that resumes it.
    fn let_go<'a>(&'a self, turn: &'a TurnNote) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            let (held, open) = {
                let mut threads = self.threads.lock().await;
                let held = threads
                    .held
                    .get(&turn.session)
                    .is_some_and(|held| held.holds(turn))
                    .then(|| threads.held.remove(&turn.session))
                    .flatten();
                (held, threads.open.get(&turn.session).cloned())
            };
            if let Some(held) = held {
                return held.stop().await;
            }
            if let Some(thread) = open {
                thread.let_go(turn.clone()).await?;
            }
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
