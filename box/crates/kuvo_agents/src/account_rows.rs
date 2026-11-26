//! Settings › Accounts' rows (`kuvo/agents/api/integration`): each coding-agent account here, with
//! its sign-in as last said, its program's version and the newest, how its update stands and
//! whether Update All updates it, and its ways in. The box's to say, so every client shows the
//! same. Its program's update notices are decided as its program is read.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as Plain},
    time::Duration,
};

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    account_check::{AccountStatus, CheckFailure},
    accounts::{AgentKind, Provider},
    feed::Feed,
    program_updates::{
        Accounts, Changed, InstallResult, Maintenance, Program, ProgramUpdates, UpdateResult,
        install,
        notices::{ProgramNow, Served, UpdateNotices, agents_update_themselves},
        store,
    },
    resident::ResidentAgents,
    roster::Entry,
};

/// A way to sign in to an account.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Method {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<&'static str>,
    pub label: &'static str,
}

const fn oauth(id: &'static str, label: &'static str) -> Method {
    Method {
        kind: "oauth",
        id: Some(id),
        label,
    }
}

const fn key(label: &'static str) -> Method {
    Method {
        kind: "key",
        id: None,
        label,
    }
}

/// What every account of an agent shows, whatever runs it here.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub methods: Vec<Method>,
    /// Its maker's installer, as you'd run it in Terminal.
    pub install: Option<&'static str>,
    pub signs_out: bool,
    /// Signing out signs it out in Terminal too (its own sign-in is yours).
    pub sign_out_shared: bool,
    /// Signing out stops a turn running on it.
    pub sign_out_stops: bool,
    /// Its sign-in can finish on another device.
    pub remote_sign_in: bool,
    /// Its list is every model it runs once it's listed them (otherwise always).
    pub lists_when_listed: bool,
    /// Kuvo Code reads its program's version and updates it.
    pub program: bool,
}

/// An agent's facts, for its own sign-in (`own`) or an account added to it.
pub fn facts(kind: AgentKind, own: bool) -> Facts {
    let facts = |methods, install, signs_out, shared, stops, listed| Facts {
        methods,
        install,
        signs_out,
        sign_out_shared: shared,
        sign_out_stops: stops,
        remote_sign_in: true,
        lists_when_listed: listed,
        program: !matches!(kind, AgentKind::Cursor | AgentKind::OpenCode),
    };
    match kind {
        AgentKind::Claude => facts(
            vec![
                oauth("claudeai", "Sign in with your Claude plan"),
                oauth("console", "Sign in with Anthropic Console"),
            ],
            Some("curl -fsSL https://claude.ai/install.sh | bash"),
            own,
            own,
            false,
            true,
        ),
        AgentKind::Codex => facts(
            vec![
                oauth("chatgpt", "Sign in with ChatGPT"),
                key("Use an OpenAI API key"),
            ],
            Some("curl -fsSL https://chatgpt.com/codex/install.sh | sh"),
            own,
            own,
            true,
            true,
        ),
        AgentKind::Cursor => facts(
            vec![
                oauth("browser", "Sign in with Cursor"),
                key("Use a Cursor API key"),
            ],
            None,
            true,
            false,
            false,
            false,
        ),
        AgentKind::Grok => facts(
            vec![oauth("grok", "Sign in with Grok")],
            Some("curl -fsSL https://x.ai/cli/install.sh | bash"),
            own,
            own,
            false,
            false,
        ),
        AgentKind::Antigravity => facts(
            vec![oauth("oauth-personal", "Sign in with Google")],
            Some("curl -fsSL https://antigravity.google/cli/install.sh | bash"),
            true,
            false,
            false,
            true,
        ),
        // Its sign-ins are its providers', in their own rows.
        AgentKind::OpenCode => facts(Vec::new(), None, false, false, false, false),
    }
}

/// An account as it was last said: a change means other models.
#[derive(Clone, Default)]
struct Seen {
    label: Option<String>,
}

/// How a row's account read: as its agent said, or why it couldn't.
struct Account {
    installed: bool,
    signed_in: bool,
    label: Option<String>,
    what: bool,
    message: Option<String>,
    ended: bool,
    unchecked: bool,
}

impl From<Result<AccountStatus, CheckFailure>> for Account {
    fn from(read: Result<AccountStatus, CheckFailure>) -> Self {
        match read {
            Ok(status) => Self {
                installed: status.installed,
                signed_in: status.signed_in,
                label: status.label,
                what: status.label_describes_method,
                message: status.message,
                ended: status.ended,
                unchecked: status.unchecked,
            },
            // A check that fails is its program's: it's there, and says why.
            Err(CheckFailure::Failed(message)) => Self {
                installed: true,
                signed_in: false,
                label: None,
                what: false,
                message: Some(message),
                ended: false,
                unchecked: false,
            },
            Err(CheckFailure::TimedOut) => Self {
                installed: true,
                signed_in: false,
                label: None,
                what: false,
                message: None,
                ended: false,
                unchecked: true,
            },
        }
    }
}

/// Why an update or install was refused.
pub struct Refused {
    pub status: u16,
    pub message: String,
}

/// An object without its fields that say nothing.
fn without_nones(mut value: Value) -> Value {
    if let Value::Object(fields) = &mut value {
        fields.retain(|_, field| !field.is_null());
    }
    value
}

fn lock<T>(plain: &Plain<T>) -> std::sync::MutexGuard<'_, T> {
    plain.lock().unwrap_or_else(|error| error.into_inner())
}

struct Inner {
    agents: ResidentAgents,
    updates: ProgramUpdates,
    notices: UpdateNotices,
    home: PathBuf,
    temporary: PathBuf,
    /// Each program's version and the newest, as last known, by program.
    maintained: Plain<HashMap<String, Maintenance>>,
    seen: Plain<HashMap<String, Seen>>,
}

#[derive(Clone)]
pub struct AccountRows(Arc<Inner>);

fn accounts_of(agents: &ResidentAgents) -> Vec<Arc<dyn Program>> {
    agents
        .roster()
        .all()
        .iter()
        .map(|entry| Arc::new(entry.clone()) as Arc<dyn Program>)
        .collect()
}

/// Every account with `program` told it changed: each one's row says how it stands.
fn tell(feed: &Feed, agents: &ResidentAgents, program: &str) {
    for entry in agents.roster().all() {
        if entry.account.kind.name() == program {
            feed.emit(
                "integration.updated",
                json!({ "integrationID": entry.account.provider().to_string() }),
            );
        }
    }
}

impl AccountRows {
    /// The rows of `agents`' accounts, for a box at `home`.
    pub fn new(home: PathBuf, agents: ResidentAgents) -> Self {
        let listed = agents.clone();
        let accounts: Accounts = Arc::new(move || accounts_of(&listed));
        // (Ended, what the program is now is noted before anyone's told; then that its models
        // changed.)
        let cell: Arc<std::sync::OnceLock<std::sync::Weak<Inner>>> = Arc::default();
        let (feed, told, noting) = (agents.feed().clone(), agents.clone(), cell.clone());
        let changed: Changed = Arc::new(move |program: &str, ended: Option<&UpdateResult>| {
            if let Some(result) = ended
                && let Some(inner) = noting.get().and_then(std::sync::Weak::upgrade)
            {
                AccountRows(inner).ended(program, result.updated);
            }
            tell(&feed, &told, program);
        });
        let updates = ProgramUpdates::at(&home, accounts, changed);
        let rows = Self(Arc::new(Inner {
            notices: UpdateNotices::new(&home.join("agents")),
            temporary: home.join("tmp"),
            agents,
            updates,
            home,
            maintained: Plain::default(),
            seen: Plain::default(),
        }));
        let _ = cell.set(Arc::downgrade(&rows.0));
        rows.check_programs();
        rows
    }

    /// What the agents tell their clients.
    pub fn feed(&self) -> &Feed {
        self.0.agents.feed()
    }

    /// Each program’s newest looked up daily, or when the agents settings are opened.
    /// No version checks at resident startup.
    fn check_programs(&self) {
        let rows = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            loop {
                timer_fires::sleep(Timer::AgentProgramCheck, Duration::from_secs(24 * 60 * 60)).await;
                let Some(inner) = rows.upgrade() else { return };
                let rows = AccountRows(inner);
                let mut programs: Vec<Entry> = Vec::new();
                for entry in rows.0.agents.roster().all() {
                    if !programs
                        .iter()
                        .any(|each| each.account.kind == entry.account.kind)
                    {
                        programs.push(entry.clone());
                    }
                }
                for entry in programs {
                    let Some(definition) = entry.agent.program() else {
                        continue;
                    };
                    let value = rows.0.updates.maintenance(&definition, false).await;
                    if rows.noted(&entry, value) {
                        for each in rows.0.agents.roster().all() {
                            if each.account.kind == entry.account.kind {
                                rows.feed().emit(
                                    "integration.updated",
                                    json!({ "integrationID": each.account.provider().to_string(), "programMaintenance": true }),
                                );
                            }
                        }
                    }
                }
                drop(rows);
            }
        });
    }

    /// What `entry`'s program is now, as just read: its notices decided, and whether that's
    /// news. A version that moved on (updated in Terminal) runs anew in each account with it.
    fn noted(&self, entry: &Entry, value: Maintenance) -> bool {
        let kind = entry.account.kind;
        let program = kind.name();
        let roster = self.0.agents.roster();
        let own = roster
            .all()
            .iter()
            .find(|each| each.account.kind == kind && each.account.id == "default")
            .unwrap_or(entry);
        let decided = self.0.notices.note(
            ProgramNow {
                program: program.into(),
                name: kind.display_name().into(),
                provider: own.account.provider().to_string(),
                maintenance: value.clone(),
                updates: own.agent.program().is_some() && value.command.is_some(),
            },
            agents_update_themselves(&self.0.home),
        );
        if let Some(notice) = decided.told {
            self.feed()
                .emit("kuvo.updates.notice", json!({ "notice": notice }));
        }
        if decided.changed {
            self.feed().emit("kuvo.updates.notices.updated", json!({}));
        }
        let before = lock(&self.0.maintained).insert(program.into(), value.clone());
        if let Some(before) = &before
            && before.installed != value.installed
        {
            let definition = own.agent.program();
            let home = self.0.home.clone();
            let name = program.to_owned();
            let matching: Vec<_> = roster
                .all()
                .iter()
                .filter(|each| each.account.kind == kind)
                .cloned()
                .collect();
            tokio::spawn(async move {
                if let Some(definition) = definition {
                    let _ = tokio::task::spawn_blocking(move || {
                        store::stage_current(
                            &home,
                            &name,
                            &definition.executable,
                            definition.npm_package.as_deref(),
                        )
                    })
                    .await;
                }
                for each in matching {
                    Program::updated(&each).await;
                }
            });
            self.feed()
                .emit("model.updated", json!({ "program": program }));
        }
        before.as_ref() != Some(&value)
    }

    /// An update of `program` ended: what it is now is noted (its accounts run it anew if its
    /// version moved), and, updated, that its models changed.
    fn ended(&self, program: &str, updated: bool) {
        let roster = self.0.agents.roster();
        let Some(entry) = roster
            .all()
            .iter()
            .find(|each| each.account.kind.name() == program)
            .cloned()
        else {
            return;
        };
        let (rows, program) = (self.clone(), program.to_owned());
        tokio::spawn(async move {
            // (Read afresh by the update: what its accounts list from here.)
            if let Some(definition) = entry.agent.program() {
                let value = rows.0.updates.maintenance(&definition, false).await;
                rows.noted(&entry, value);
            }
            if updated {
                rows.feed()
                    .emit("model.updated", json!({ "program": program }));
            }
        });
    }

    /// Its version and the newest, if they come soon (checking may ask Homebrew or npm): what was
    /// known before otherwise, and its row is said again once they come.
    async fn maintained_soon(&self, entry: &Entry, fresh: bool) -> Option<Maintenance> {
        let definition = entry.agent.program()?;
        let (rows, account) = (self.clone(), entry.clone());
        let answered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let late = answered.clone();
        let asked = tokio::spawn(async move {
            let value = rows.0.updates.maintenance(&definition, fresh).await;
            if rows.noted(&account, value.clone()) && late.load(std::sync::atomic::Ordering::SeqCst)
            {
                rows.feed().emit(
                    "integration.updated",
                    json!({ "integrationID": account.account.provider().to_string() }),
                );
            }
            value
        });
        let value = match tokio::time::timeout(Duration::from_secs(2), asked).await {
            Ok(Ok(value)) => Some(value),
            _ => lock(&self.0.maintained)
                .get(entry.account.kind.name())
                .cloned(),
        };
        answered.store(true, std::sync::atomic::Ordering::SeqCst);
        value
    }

    /// Every account's row (`fresh`: asked anew).
    pub async fn list(&self, fresh: bool) -> Vec<Value> {
        let roster = self.0.agents.roster();
        let listed = roster.all().iter().filter(|entry| entry.account.kind.in_accounts());
        futures_util::future::join_all(listed.map(|entry| self.row(entry, fresh))).await
    }

    /// One account's row, asked anew.
    pub async fn one(&self, provider: &Provider) -> Option<Value> {
        let roster = self.0.agents.roster();
        let entry = roster.find(provider)?;
        provider.kind.in_accounts().then_some(())?;
        Some(self.row(entry, true).await)
    }

    async fn row(&self, entry: &Entry, fresh: bool) -> Value {
        let (account, maintained) =
            tokio::join!(entry.status(fresh), self.maintained_soon(entry, fresh));
        let account = Account::from(account);
        let kind = entry.account.kind;
        let provider = entry.account.provider().to_string();
        let own = entry.account.id == "default";
        let facts = facts(kind, own);
        // Who's signed in, as last said (one not said yet, or not now, is no other account).
        let label = {
            let mut seen = lock(&self.0.seen);
            let before = seen.entry(provider.clone()).or_default();
            let label = account
                .signed_in
                .then(|| account.label.clone().or_else(|| before.label.clone()))
                .flatten();
            before.label = label.clone();
            label
        };
        let program = kind.name();
        let updates = own && facts.program;
        let outdated = own && maintained.as_ref().is_some_and(Maintenance::is_outdated);
        let command = maintained
            .as_ref()
            .and_then(|value| value.command.as_ref())
            .is_some();
        let lists_every_model = !facts.lists_when_listed || entry.lists_every_model().await;
        let connections = if account.signed_in {
            let message = match (&label, account.unchecked) {
                (Some(label), _) => format!("Signed in as {label}"),
                (None, true) => "Couldn't check the sign-in just now".into(),
                (None, false) => "Signed in".into(),
            };
            let mut connection = json!({ "type": "agent", "label": label, "method": "oauth", "status": { "status": "connected", "message": message } });
            if account.what && label.is_some() {
                connection["what"] = json!(true);
            }
            vec![without_nones(connection)]
        } else {
            Vec::new()
        };
        let mut row = json!({
            "id": provider,
            "name": kind.display_name(),
            "metadata": {
                "source": "agent",
                "agent": program,
                "addedAccounts": true,
                "accountID": entry.account.id,
                "accountLabel": entry.account.label,
                "installed": account.installed,
                "message": account.message,
                "signInEnded": account.ended,
                "unchecked": account.unchecked,
                "account": label,
                "install": facts.install,
                "signsOut": facts.signs_out,
                "signOutShared": facts.signs_out && facts.sign_out_shared,
                "signOutStops": facts.signs_out && facts.sign_out_stops,
                "version": maintained.as_ref().and_then(|value| value.installed.clone()),
                "latestVersion": own.then(|| maintained.as_ref().and_then(|value| value.latest.clone())).flatten(),
                "outdated": outdated,
                "updateCommand": own.then(|| maintained.as_ref().and_then(|value| value.shown.clone())).flatten(),
                "updates": updates && command,
                "programUpdate": own.then(|| self.0.updates.update_of(program, maintained.as_ref().and_then(|value| value.installed.as_deref()))).flatten(),
                "updateAll": updates && command && outdated && !self.0.updates.is_updating(program),
                "remoteSignIn": facts.remote_sign_in,
                "modelsComplete": lists_every_model,
            },
            "methods": facts.methods,
            "connections": connections,
        });
        // (What it doesn't know it leaves out, as the reference does.)
        row["metadata"] = without_nones(row["metadata"].take());
        row
    }

    /// Update All: the programs it started.
    pub async fn update_all(&self) -> Vec<String> {
        self.0.updates.update_all().await
    }

    /// That account's program updated, answered as it starts.
    pub fn update(&self, provider: &Provider) -> Result<Vec<String>, Refused> {
        let roster = self.0.agents.roster();
        let entry = roster.find(provider).ok_or_else(|| Refused {
            status: 404,
            message: format!("No agent {provider}."),
        })?;
        if !facts(entry.account.kind, true).program {
            return Err(Refused {
                status: 400,
                message: format!(
                    "Update {} the way you installed it.",
                    entry.account.kind.display_name()
                ),
            });
        }
        drop(self.0.updates.update(Arc::new(entry.clone())));
        Ok(vec![entry.account.kind.name().into()])
    }

    /// How that account's program's update went, seen (`at`: that one), for every client.
    pub fn forget(&self, provider: &Provider, at: Option<u64>) -> Result<(), Refused> {
        let roster = self.0.agents.roster();
        let entry = roster.find(provider).ok_or_else(|| Refused {
            status: 404,
            message: format!("No agent {provider}."),
        })?;
        let program = entry.account.kind.name();
        if self.0.updates.forget(program, at) {
            tell(self.feed(), &self.0.agents, program);
        }
        Ok(())
    }

    /// The update notices served now, newest first.
    pub fn notices(&self) -> Vec<Served> {
        self.0.notices.list()
    }

    /// One seen, for every client.
    pub fn seen(&self, id: &str) {
        if self.0.notices.seen(id) {
            self.feed().emit("kuvo.updates.notices.updated", json!({}));
        }
    }

    /// Its program installed with its maker's installer; then what it lists.
    pub async fn install(&self, provider: &Provider) -> Result<InstallResult, Refused> {
        let roster = self.0.agents.roster();
        let entry = roster.find(provider).ok_or_else(|| Refused {
            status: 404,
            message: format!("No agent {provider}."),
        })?;
        let kind = entry.account.kind;
        let Some(command) = facts(kind, true)
            .install
            .filter(|_| facts(kind, true).program)
        else {
            return Err(Refused {
                status: 400,
                message: format!("Install {} the way its maker says.", kind.display_name()),
            });
        };
        let agent = entry.agent.clone();
        let result = install(kind.display_name(), command, &self.0.temporary, || {
            agent.program().map(|definition| definition.executable)
        })
        .await;
        if result.installed {
            // How an update went before is no longer news.
            self.0.updates.forget(kind.name(), None);
            for each in self
                .0
                .agents
                .roster()
                .all()
                .iter()
                .filter(|each| each.account.kind == kind)
            {
                Program::updated(each).await;
            }
            self.feed().emit(
                "integration.updated",
                json!({ "integrationID": provider.to_string() }),
            );
            self.feed()
                .emit("model.updated", json!({ "program": kind.name() }));
        }
        Ok(result)
    }
}
