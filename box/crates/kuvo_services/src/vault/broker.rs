//! Authorization and consent shared by fills, tools and SSH.
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use tokio::time;
use tokio_util::sync::CancellationToken;

use super::{
    Error, How, Item, Result,
    activity::{Action, Entry},
    asks::{Answer, AskKind, Book, Decision, Question},
    helper::millis,
    identity::{Identity, Reader},
    routes::{Local, Resident},
};

const ONCE_FOR: u64 = 30 * 60_000;
const AUTOPILOT_LOOK: Duration = Duration::from_secs(5);
pub(super) const LEFT_UNDONE: &str = "This is on Autopilot: nothing was put to the user, and nothing was chosen for you. If your own research settles it, decide it, say in a line what you chose and why, and go on. If only the user can do it (paying, signing in, making an account, handing over a key, or anything destructive that can't be undone), don't decide it, do it or ask again: leave that step undone, carry on with everything else, and list what's left for them in your final reply.";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Allowed {
    Own,
    Once,
    Always,
}

impl Allowed {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::Once => "once",
            Self::Always => "always",
        }
    }
}

pub(super) enum UseDenied {
    Refused(String),
    Autopilot,
}

#[derive(Clone, Default)]
pub(super) struct Where {
    pub site: Option<String>,
    pub program: Option<String>,
    pub why: Option<String>,
}

pub(super) struct Broker {
    pub reader: Reader,
    pub asks: Arc<Book>,
    pub account: PathBuf,
    pub home: PathBuf,
    pub machine: Option<String>,
    pub gate: Gate,
    pub ledger: Arc<super::replay::Ledger>,
    once: Mutex<HashMap<(String, String), u64>>,
    filled: Mutex<VecDeque<String>>,
}

#[derive(Clone, Copy)]
pub(super) struct Gate {
    pub agents: bool,
    pub waiting: bool,
}

impl Gate {
    fn new(home: &Path, account: &Path, os_account: &Path) -> Self {
        let real_home = os_account.join(".kuvo-code");
        let test_account_home = account.join(".kuvo-code");
        let real = located(home) == located(&real_home)
            || located(home) == located(&test_account_home)
            || located(home)
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| {
                    name.to_string_lossy()
                        .to_ascii_lowercase()
                        .starts_with("run.kuvo.code")
                });
        // The production gate needs the resident fence check and the agents'
        // process status. Neither an old marker nor environment alone opens it.
        Self {
            agents: !real && std::env::var("KUVO_VAULT_AGENTS").as_deref() == Ok("1"),
            waiting: false,
        }
    }
}

fn located(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

impl Broker {
    pub fn new(
        home: &Path,
        bots: &Path,
        account: PathBuf,
        os_account: &Path,
        machine: Option<String>,
    ) -> Self {
        let gate = Gate::new(home, &account, os_account);
        Self {
            gate,
            ledger: Arc::new(super::replay::Ledger::new(home)),
            account,
            home: home.to_owned(),
            machine,
            reader: Reader::new(home, bots),
            asks: Arc::new(Book::default()),
            once: Mutex::default(),
            filled: Mutex::default(),
        }
    }

    pub fn first_fill(&self, call: String) -> bool {
        let Ok(mut filled) = self.filled.lock() else {
            return false;
        };
        if filled.contains(&call) {
            return false;
        }
        filled.push_back(call);
        if filled.len() > 1000 {
            filled.pop_front();
        }
        true
    }

    pub fn allow_once(&self, caller: &Identity, item: &str) {
        if let Ok(mut once) = self.once.lock() {
            let now = millis();
            once.retain(|_, until| *until >= now);
            once.insert((caller.root.clone(), item.into()), now + ONCE_FOR);
        }
    }

    pub fn has_once(&self, caller: &Identity, item: &str, take: bool) -> bool {
        let Ok(mut once) = self.once.lock() else {
            return false;
        };
        let key = (caller.root.clone(), item.into());
        let valid = once.get(&key).is_some_and(|until| *until >= millis());
        if take || !valid {
            once.remove(&key);
        }
        valid
    }

    pub fn question(&self, kind: AskKind, caller: &Identity) -> Question {
        let mut question = Question::default();
        question.kind = kind;
        question.session = (!caller.root.is_empty()).then(|| caller.root.clone());
        question.session_name = caller.name.clone();
        question.bot = caller.bot.as_ref().map(|bot| bot.id.clone());
        question.bot_name = caller.bot.as_ref().map(|bot| bot.name.clone());
        question.machine = self.machine.clone();
        question.group = caller.group.clone();
        question.always = caller.always_for(&self.account).is_some();
        question
    }

    pub async fn wait(
        &self,
        caller: &Identity,
        question: Question,
        cancel: &CancellationToken,
    ) -> Result<Option<Answer>> {
        if caller.autopilot {
            return Ok(None);
        }
        let wait = self.asks.ask(question, cancel);
        tokio::pin!(wait);
        if caller.session.is_empty() {
            return wait.await.map(Some);
        }
        let check = async {
            let mut clock =
                time::interval_at(time::Instant::now() + AUTOPILOT_LOOK, AUTOPILOT_LOOK);
            clock.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            loop {
                clock.tick().await;
                if self.reader.caller(&caller.session).await.autopilot {
                    break;
                }
            }
        };
        tokio::select! { value = &mut wait => value.map(Some), () = check => Ok(None) }
    }
}

impl Resident {
    pub(super) async fn authorize(
        self: &Arc<Self>,
        item: &Item,
        caller: &Identity,
        place: &Where,
        ask_without: bool,
        cancel: &CancellationToken,
    ) -> Result<std::result::Result<Allowed, UseDenied>> {
        let authorization = self.block(|local| {
            if let Some(paused) = local.vault.paused_for(
                caller.bot.as_ref().map(|bot| bot.id.as_str()),
                Some(&caller.root),
            )? {
                local.note_agent(
                    Action::Refused,
                    Some(item),
                    caller,
                    place,
                    Some("paused"),
                    None,
                )?;
                return Ok(Some(Err(UseDenied::Refused(paused))));
            }
            let grant = caller.grant(item, &local.vault.grants()?, false);
            if grant == Some(How::Own) {
                return Ok(Some(Ok(Allowed::Own)));
            }
            if self.broker.has_once(caller, &item.id, true) {
                return Ok(Some(Ok(Allowed::Once)));
            }
            if grant.is_none() && !ask_without {
                local.note_agent(
                    Action::Refused,
                    Some(item),
                    caller,
                    place,
                    Some("no grant"),
                    None,
                )?;
                return Ok(Some(Err(UseDenied::Refused(format!(
                    "You may not use “{}”. Ask the user for it with vault_ask.",
                    item.name
                )))));
            }
            if !caller.autopilot {
                local.note_agent(Action::Asked, Some(item), caller, place, None, None)?;
            }
            Ok(None)
        })?;
        if let Some(authorization) = authorization {
            return Ok(authorization);
        }

        let mut question = self.broker.question(AskKind::Use, caller);
        question.item = Some(item.id.clone());
        question.item_name = Some(item.name.clone());
        question.site = place.site.clone();
        question.why = place.why.clone();
        let answer = self.broker.wait(caller, question, cancel).await?;
        self.block(|local| {
            let Some(answer) = answer else {
                local.note_agent(
                    Action::Refused,
                    Some(item),
                    caller,
                    place,
                    Some("autopilot"),
                    None,
                )?;
                return Ok(Err(UseDenied::Autopilot));
            };
            if answer.decision == Decision::No {
                local.note_agent(
                    Action::Declined,
                    Some(item),
                    caller,
                    place,
                    None,
                    answer.by.as_deref(),
                )?;
                return Ok(Err(UseDenied::Refused(format!(
                    "The user didn't let you use “{}”.",
                    item.name
                ))));
            }
            let who = (answer.decision == Decision::Always)
                .then(|| caller.always_for(&self.broker.account))
                .flatten();
            let allowed = if let Some(who) = who {
                local
                    .vault
                    .set_grant(&item.id, who, Some(How::Own), millis())?;
                local.note_agent(
                    Action::Granted,
                    Some(item),
                    caller,
                    &Where::default(),
                    Some("own"),
                    answer.by.as_deref(),
                )?;
                Allowed::Always
            } else {
                Allowed::Once
            };
            local.note_agent(
                Action::Allowed,
                Some(item),
                caller,
                place,
                Some(allowed.as_str()),
                answer.by.as_deref(),
            )?;
            Ok(Ok(allowed))
        })
    }

    // These operations inspect files and may call the blocking Keychain client.
    // All call sites are inside the resident's multi-threaded Tokio runtime.
    pub(super) fn block<T>(&self, work: impl FnOnce(&mut Local) -> Result<T>) -> Result<T> {
        tokio::task::block_in_place(|| self.with(work))
    }
}

impl Local {
    pub(super) fn note_agent(
        &mut self,
        what: Action,
        item: Option<&Item>,
        caller: &Identity,
        place: &Where,
        how: Option<&str>,
        by: Option<&str>,
    ) -> Result<()> {
        self.activity
            .note(Entry::agent(what, item, caller, place, how, by, millis())?)
    }
}

impl From<UseDenied> for Error {
    fn from(denied: UseDenied) -> Self {
        match denied {
            UseDenied::Refused(why) => Self::refused(403, why),
            UseDenied::Autopilot => Self::refused(403, LEFT_UNDONE),
        }
    }
}
