//! A reset spends the user's plan credit. The offer is short lived; a spend and
//! its maker's request and credit are kept before it can leave this computer.
use super::limits::{Account, Service, now};
use http::StatusCode;
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    shape::{Primitive, Shape},
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

const CONFIRM_FOR: u64 = 600_000;
const UNSETTLED_FOR: u64 = 86_400_000;
const KEEP_FOR: u64 = 30 * UNSETTLED_FOR;
const KEEP: usize = 200;
type Answer<T> = Result<T, Refusal>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Reset,
    Unconfirmed,
    NothingToReset,
    NoReset,
    NotNow,
    Failed,
    Unknown,
}
impl Outcome {
    fn open(self) -> bool {
        matches!(self, Self::Unknown | Self::Unconfirmed)
    }
    fn words(self) -> Option<&'static str> {
        Some(match self {
            Self::Reset => return None,
            Self::Unconfirmed => "It may still be going through: check again in a minute.",
            Self::NothingToReset => {
                "Nothing to reset: this account isn't at a limit. Nothing was used."
            }
            Self::NoReset => "That reset isn't available any more. Nothing was used.",
            Self::NotNow => "Resets can't be used right now. Nothing was used.",
            Self::Failed => "Didn't go through. Nothing was used.",
            Self::Unknown => {
                "No answer came, so it may have been used. Asking again won't use another."
            }
        })
    }
}
/// A maker reply is distinguished from a local stop before anything was sent.
pub struct Completion {
    pub outcome: Outcome,
    pub already: bool,
    pub until: Option<u64>,
    pub reached: bool,
}
impl From<Outcome> for Completion {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            already: false,
            until: None,
            reached: false,
        }
    }
}
impl Completion {
    pub fn maker(outcome: Outcome) -> Self {
        Self {
            reached: true,
            ..outcome.into()
        }
    }
}
pub trait Port: Send + Sync {
    /// Check the live sign-in and choose its soonest available grant. No spend.
    fn choose<'a>(
        &'a self,
        account: &'a Account,
    ) -> futures_util::future::BoxFuture<'a, Result<String, Outcome>>;
    /// Check the live sign-in again and spend exactly this target, with this maker request ID.
    fn consume<'a>(
        &'a self,
        account: &'a Account,
        request: &'a str,
        target: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Completion>;
    fn refresh<'a>(&'a self, provider: &'a str) -> futures_util::future::BoxFuture<'a, ()>;
}

/// Authenticated usage seat mail. Only the offered row and idempotent request
/// name cross the boundary. Actual provider targets stay with the holder.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResetMail {
    pub kind: ResetKind,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<RemoteAsk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<RemoteAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always: Option<RemoteDecision>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResetKind {
    Reset,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteAsk {
    pub id: String,
    pub row: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unasked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteAnswer {
    pub id: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteDecision {
    pub row: String,
    pub on: bool,
    pub at: u64,
}
impl ResetMail {
    pub fn ask(ask: RemoteAsk) -> Self {
        Self {
            kind: ResetKind::Reset,
            at: now(),
            ask: Some(ask),
            answer: None,
            always: None,
        }
    }
    pub fn answer(answer: RemoteAnswer) -> Self {
        Self {
            kind: ResetKind::Reset,
            at: now(),
            ask: None,
            answer: Some(answer),
            always: None,
        }
    }
    pub fn decision(always: RemoteDecision) -> Self {
        Self {
            kind: ResetKind::Reset,
            at: now(),
            ask: None,
            answer: None,
            always: Some(always),
        }
    }
}
/// The resident's existing relay and authenticated setup directory.
pub trait PeerTransport: Send + Sync {
    fn known(&self) -> bool;
    fn member(&self, key: &str) -> bool;
    fn reachable(&self, key: &str) -> bool;
    fn send<'a>(
        &'a self,
        key: &'a str,
        mail: ResetMail,
    ) -> futures_util::future::BoxFuture<'a, bool>;
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn shared_row(row: &str) -> bool {
    row.split_once('#').is_some_and(|(service, id)| {
        matches!(service, "codex" | "claude")
            && id.len() == 16
            && id.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

#[derive(Clone)]
struct Confirm {
    row: String,
    at: u64,
    account: Option<Account>,
    via: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum State {
    Sending,
    Done,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Spend {
    id: String,
    row: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<String>,
    request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    also: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unasked: bool,
    at: u64,
    state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    until: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    already: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Decision {
    on: bool,
    at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Journal {
    #[serde(default)]
    spends: Vec<Spend>,
    #[serde(default)]
    allowed: BTreeMap<String, Decision>,
    #[serde(default)]
    seen: BTreeMap<String, String>,
}
struct Inner {
    journal: Result<Journal, String>,
    confirms: BTreeMap<String, Confirm>,
}
#[derive(Clone)]
pub struct Resets {
    limits: Service,
    file: PathBuf,
    port: Arc<dyn Port>,
    inner: Arc<Mutex<Inner>>,
    transport: Option<Arc<dyn PeerTransport>>,
    pending: Arc<
        std::sync::Mutex<BTreeMap<(String, String), tokio::sync::oneshot::Sender<RemoteAnswer>>>,
    >,
    remote_gate: Arc<Mutex<()>>,
}

fn refusal(status: StatusCode, message: impl Into<String>) -> Refusal {
    Refusal::json(status, json!({"message":message.into()}))
}
fn bad(message: &str) -> Refusal {
    refusal(StatusCode::BAD_REQUEST, message)
}
fn conflict(message: &str) -> Refusal {
    refusal(StatusCode::CONFLICT, message)
}
fn missing(message: &str) -> Refusal {
    refusal(StatusCode::NOT_FOUND, message)
}
fn failure(error: impl std::fmt::Display) -> Refusal {
    refusal(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Couldn't keep the reset journal: {error}"),
    )
}
fn random<const N: usize>() -> Answer<String> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(failure)?;
    Ok(hex::encode(bytes))
}
fn remote_spend(row: &str, confirm: &str, by: Option<&str>, unasked: bool, via: &str) -> Spend {
    Spend {
        id: confirm.into(),
        row: row.into(),
        provider: String::new(),
        via: Some(via.into()),
        request: confirm.into(),
        target: None,
        also: vec![],
        by: by.map(|s| s.chars().take(200).collect()),
        unasked,
        at: now(),
        state: State::Sending,
        outcome: None,
        until: None,
        already: false,
        finished: None,
    }
}
fn remote_ask(spend: &Spend) -> RemoteAsk {
    RemoteAsk {
        id: spend.id.clone(),
        row: spend.row.clone(),
        by: spend.by.clone(),
        unasked: spend.unasked,
    }
}
fn overlap(journal: &Journal, row: &str, via: Option<&str>, entered: u64) -> Option<usize> {
    journal.spends.iter().rposition(|s| {
        s.row == row
            && s.via.as_deref() == via
            && matches!(s.state, State::Done)
            && s.finished.is_some_and(|at| at >= entered)
    })
}
fn attach_alias(spend: &mut Spend, confirm: &str) {
    if !spend.also.iter().any(|id| id == confirm) {
        spend.also.push(confirm.into());
        if spend.also.len() > 20 {
            spend.also.remove(0);
        }
    }
}
fn prune_spends(journal: &mut Journal) {
    let at = now();
    journal
        .spends
        .retain(|s| at.saturating_sub(s.at) < KEEP_FOR);
    while journal.spends.len() >= KEEP {
        let Some(i) = journal.spends.iter().position(|s| {
            matches!(s.state, State::Done) || at.saturating_sub(s.at) >= UNSETTLED_FOR
        }) else {
            break;
        };
        journal.spends.remove(i);
    }
}
impl Resets {
    pub fn new(home: &std::path::Path, limits: Service, port: Arc<dyn Port>) -> Self {
        let file = home.join("agents/limits-resets.json");
        let journal = match kuvo_store::files::read(&file) {
            Ok(None) => Ok(Journal::default()),
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        if let Ok(journal) = &journal {
            limits.reset_permissions(
                journal
                    .allowed
                    .iter()
                    .map(|(id, d)| (id.clone(), d.on))
                    .collect(),
            );
        }
        Self {
            limits,
            file,
            port,
            inner: Arc::new(Mutex::new(Inner {
                journal,
                confirms: BTreeMap::new(),
            })),
            transport: None,
            pending: Default::default(),
            remote_gate: Default::default(),
        }
    }
    pub fn with_peers(mut self, transport: Arc<dyn PeerTransport>) -> Self {
        self.transport = Some(transport);
        self
    }
    fn save(&self, journal: &Journal) -> Answer<()> {
        let bytes = serde_json::to_vec(journal).map_err(failure)?;
        kuvo_store::files::write_secret(&self.file, &bytes).map_err(failure)?;
        self.limits.reset_permissions(
            journal
                .allowed
                .iter()
                .map(|(id, d)| (id.clone(), d.on))
                .collect(),
        );
        Ok(())
    }
    pub fn follow_sign_ins(&self) {
        let resets = self.clone();
        let mut changes = self.limits.subscribe();
        tokio::spawn(async move {
            loop {
                {
                    let accounts = resets.limits.reset_sign_ins();
                    let mut inner = resets.inner.lock().await;
                    if let Ok(journal) = inner.journal.as_mut() {
                        // Only a different known identity revokes a previous Always Allow. Merely
                        // opening the box with an unknown sign-in creates no journal.
                        for (account, row) in accounts {
                            if journal.seen.contains_key(&account.provider) {
                                if resets.see(journal, &account, &row).is_err() {
                                    eprintln!("Couldn't keep the reset's current sign-in.");
                                }
                            }
                        }
                    }
                }
                if changes.changed().await.is_err() {
                    break;
                }
            }
        });
    }
    fn allowed(journal: &Journal, row: &str) -> bool {
        journal.allowed.get(row).is_some_and(|d| d.on)
    }
    fn see(&self, journal: &mut Journal, account: &Account, row: &str) -> Answer<()> {
        if journal
            .seen
            .get(&account.provider)
            .is_some_and(|old| old == row)
        {
            return Ok(());
        }
        let mut changed = journal.clone();
        if let Some(old) = changed.seen.insert(account.provider.clone(), row.into()) {
            if let Some(decision) = changed.allowed.get_mut(&old) {
                decision.on = false;
                decision.from = None;
                decision.at = now().max(decision.at.saturating_add(1));
            }
        }
        self.save(&changed)?;
        *journal = changed;
        Ok(())
    }
    pub async fn always(&self, row: Option<&str>, provider: Option<&str>, on: bool) -> Answer<()> {
        let local = self.limits.reset_account(row, provider);
        let row = local
            .as_ref()
            .map(|(_, row)| row.clone())
            .or_else(|| {
                row.filter(|r| self.limits.row(r).is_some())
                    .map(str::to_owned)
            })
            .ok_or_else(|| missing("No account of yours by that name has resets."))?;
        if !shared_row(&row) {
            return Err(conflict(
                "This account doesn't say who it is yet, so Always Allow can't be kept for it.",
            ));
        }
        let mut inner = self.inner.lock().await;
        let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
        if let Some((account, _)) = local {
            self.see(journal, &account, &row)?;
        }
        let mut changed = journal.clone();
        let at = now().max(
            changed
                .allowed
                .get(&row)
                .map_or(0, |d| d.at.saturating_add(1)),
        );
        changed.allowed.insert(row, Decision { on, at, from: None });
        self.save(&changed)?;
        *journal = changed;
        Ok(())
    }
    pub async fn offer(&self, row: &str) -> Answer<Value> {
        let card = self
            .limits
            .row(row)
            .ok_or_else(|| missing("No account of yours by that name."))?;
        let extras = card["extras"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let Some(reset) = extras.iter().find(|e| e["kind"] == "resets") else {
            let msg = extras
                .iter()
                .find(|e| e["kind"] == "unreadable")
                .and_then(|e| e["message"].as_str())
                .unwrap_or("This account has no resets.");
            return Err(conflict(msg));
        };
        let count = reset["count"].as_u64().unwrap_or(0);
        if count == 0 {
            return Err(conflict("This account has no resets left."));
        }
        if reset["usable"] != true {
            return Err(conflict(
                "None of your computers can use this account's resets now.",
            ));
        }
        let holder = &reset["on"];
        let (account, via) = if holder["here"] == true {
            (
                Some(
                    self.limits
                        .reset_account(Some(row), None)
                        .ok_or_else(|| {
                            conflict("This computer's sign-in can't use this account's resets.")
                        })?
                        .0,
                ),
                None,
            )
        } else {
            let key = holder["key"].as_str().ok_or_else(|| {
                conflict("None of your computers can use this account's resets now.")
            })?;
            if !self.transport.as_ref().is_some_and(|t| t.reachable(key)) {
                return Err(refusal(
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "{} can't be reached now: its reset is used there.",
                        holder["name"]
                            .as_str()
                            .unwrap_or("The computer with its sign-in")
                    ),
                ));
            }
            (None, Some(key.to_owned()))
        };
        let mut inner = self.inner.lock().await;
        let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
        if let Some(account) = &account {
            self.see(journal, account, row)?;
        }
        let always = Self::allowed(journal, row);
        let at = now();
        let confirm = format!("rsc_{}", random::<12>()?);
        inner
            .confirms
            .retain(|_, c| at.saturating_sub(c.at) <= CONFIRM_FOR);
        inner.confirms.insert(
            confirm.clone(),
            Confirm {
                row: row.into(),
                at,
                account,
                via,
            },
        );
        let mut offer = json!({"confirm":confirm,"row":row,"service":card["service"],"count":count,"on":holder,"always":always,"expires":at+CONFIRM_FOR});
        if reset["refills"].as_array().is_some_and(|a| !a.is_empty()) {
            offer["refills"] = reset["refills"].clone();
        }
        Ok(offer)
    }
    fn done(&self, row: &str, outcome: Outcome, open: bool) -> Value {
        self.done_with(row, outcome, open, None)
    }
    fn done_with(&self, row: &str, outcome: Outcome, open: bool, until: Option<u64>) -> Value {
        let mut done = json!({"outcome":outcome});
        if let Some(until) = until {
            done["until"] = json!(until);
        }
        if let Some(message) = if open && !outcome.open() {
            Some(
                "It can't be asked of its maker from here right now. An earlier try may have gone through: asking again later won't use another.",
            )
        } else {
            outcome.words()
        } {
            done["message"] = json!(message);
        }
        if let Some(row) = self.limits.row(row) {
            done["row"] = row;
        }
        done
    }
    pub async fn spend(
        &self,
        row: &str,
        confirm: &str,
        by: Option<&str>,
        unasked: bool,
    ) -> Answer<Value> {
        let entered = now();
        let remote = {
            let inner = self.inner.lock().await;
            let journal = inner.journal.as_ref().map_err(failure)?;
            journal
                .spends
                .iter()
                .find(|s| {
                    s.row == row && (s.id == confirm || s.also.iter().any(|id| id == confirm))
                })
                .and_then(|s| s.via.clone())
                .or_else(|| {
                    inner
                        .confirms
                        .get(confirm)
                        .filter(|c| c.row == row)
                        .and_then(|c| c.via.clone())
                })
        };
        if let Some(via) = remote {
            return self
                .spend_remote(row, confirm, by, unasked, &via, entered)
                .await;
        }
        // Serialize spending, not the rest of usage, so two clients never choose the same credit
        // concurrently. No cancellation window exists between the journal and the maker's call.
        let mut inner = self.inner.lock().await;
        let ticket = inner
            .confirms
            .get(confirm)
            .filter(|c| c.row == row && now().saturating_sub(c.at) <= CONFIRM_FOR)
            .cloned();
        let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
        let existing = journal
            .spends
            .iter()
            .position(|s| s.row == row && (s.id == confirm || s.also.iter().any(|a| a == confirm)));
        if let Some(i) = existing {
            let kept = &journal.spends[i];
            if matches!(kept.state, State::Done) {
                return Ok(self.done_with(
                    row,
                    kept.outcome.unwrap_or(Outcome::Unknown),
                    false,
                    kept.until,
                ));
            }
        } else if ticket.is_none() {
            return Err(missing("That confirmation has expired: ask again."));
        }
        if existing.is_none() && (!unasked || Self::allowed(journal, row)) {
            if let Some(i) = overlap(journal, row, None, entered) {
                let mut changed = journal.clone();
                let kept = &mut changed.spends[i];
                attach_alias(kept, confirm);
                let outcome = kept.outcome.unwrap_or(Outcome::Unknown);
                let until = kept.until;
                self.save(&changed)?;
                *journal = changed;
                return Ok(self.done_with(row, outcome, false, until));
            }
        }
        let open = existing.or_else(|| {
            journal.spends.iter().rposition(|s| {
                s.row == row
                    && s.via.is_none()
                    && matches!(s.state, State::Sending)
                    && now().saturating_sub(s.at) < UNSETTLED_FOR
            })
        });
        let account = if let Some(i) = open {
            self.limits
                .reset_account(Some(row), Some(&journal.spends[i].provider))
                .map(|(a, _)| a)
        } else {
            ticket.as_ref().and_then(|c| c.account.clone())
        };
        let Some(account) = account else {
            return Ok(self.done(row, Outcome::NotNow, open.is_some()));
        };
        self.see(journal, &account, row)?;
        if (unasked || existing.is_some_and(|i| journal.spends[i].unasked))
            && (!Self::allowed(journal, row) || self.transport.as_ref().is_some_and(|t| !t.known()))
        {
            let mut done = self.done(row, Outcome::Failed, open.is_some());
            done["message"] = json!(
                "Always Allow is off for this account now: nothing was used. Ask the user first."
            );
            return Ok(done);
        }
        if let Some(i) = open {
            let later =
                existing.is_none() && now().saturating_sub(journal.spends[i].at) >= CONFIRM_FOR;
            let mut changed = journal.clone();
            if !unasked {
                changed.spends[i].unasked = false;
            }
            if existing.is_none() && !later {
                attach_alias(&mut changed.spends[i], confirm);
            }
            self.save(&changed)?;
            *journal = changed;
            let (completion, still_open) =
                self.run_local(journal, i, &account, confirm, true).await?;
            if !later || still_open || completion.outcome == Outcome::Reset && !completion.already {
                if later {
                    let mut changed = journal.clone();
                    attach_alias(&mut changed.spends[i], confirm);
                    self.save(&changed)?;
                    *journal = changed;
                }
                return Ok(self.done_with(row, completion.outcome, still_open, completion.until));
            }
            // An older unsettled request was already done or definitely refused.
            // This confirmation may now spend one new credit, under a new maker ID.
        }
        let Some(account) = ticket.and_then(|c| c.account) else {
            return Ok(self.done(row, Outcome::NotNow, false));
        };
        self.see(journal, &account, row)?;
        let mut changed = journal.clone();
        prune_spends(&mut changed);
        changed.spends.push(Spend {
            id: confirm.into(),
            row: row.into(),
            provider: account.provider.clone(),
            via: None,
            request: random::<16>()?,
            target: None,
            also: vec![],
            by: by.map(|s| s.chars().take(200).collect()),
            unasked,
            at: now(),
            state: State::Sending,
            outcome: None,
            until: None,
            already: false,
            finished: None,
        });
        let i = changed.spends.len() - 1;
        self.save(&changed)?;
        *journal = changed;
        let (completion, still_open) = self.run_local(journal, i, &account, confirm, false).await?;
        Ok(self.done_with(row, completion.outcome, still_open, completion.until))
    }
    async fn run_local(
        &self,
        journal: &mut Journal,
        i: usize,
        account: &Account,
        confirm: &str,
        previous: bool,
    ) -> Answer<(Completion, bool)> {
        let target = match journal.spends[i].target.clone() {
            Some(target) => Ok(target),
            None => self.port.choose(account).await,
        };
        let completion = match target {
            Ok(target) => {
                let mut changed = journal.clone();
                changed.spends[i].target = Some(target.clone());
                self.save(&changed)?;
                *journal = changed;
                if confirm.split_once(':').is_some_and(|(from, _)| {
                    !self.transport.as_ref().is_some_and(|t| t.member(from))
                }) {
                    Outcome::NotNow.into()
                } else {
                    self.port
                        .consume(account, &journal.spends[i].request, &target)
                        .await
                }
            }
            Err(outcome) => outcome.into(),
        };
        let still_open = completion.outcome.open() || previous && !completion.reached;
        if !still_open {
            let mut changed = journal.clone();
            let kept = &mut changed.spends[i];
            kept.state = State::Done;
            kept.outcome = Some(completion.outcome);
            kept.until = completion.until;
            kept.already = completion.already;
            kept.finished = Some(now());
            self.save(&changed)?;
            *journal = changed;
        }
        if matches!(
            completion.outcome,
            Outcome::Reset | Outcome::Unconfirmed | Outcome::NoReset | Outcome::Unknown
        ) {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                self.port.refresh(&account.provider),
            )
            .await;
            if !still_open && journal.spends[i].finished.is_some_and(|at| at < now()) {
                let mut changed = journal.clone();
                changed.spends[i].finished = Some(now());
                self.save(&changed)?;
                *journal = changed;
            }
        }
        eprintln!(
            "Plan reset of {}: {:?}",
            account.provider, completion.outcome
        );
        Ok((completion, still_open))
    }
    async fn spend_remote(
        &self,
        row: &str,
        confirm: &str,
        by: Option<&str>,
        unasked: bool,
        via: &str,
        entered: u64,
    ) -> Answer<Value> {
        let _remote = self.remote_gate.lock().await;
        let (mut id, mut ask, mut prior, mut later) = {
            let mut inner = self.inner.lock().await;
            let ticket = inner
                .confirms
                .get(confirm)
                .filter(|c| {
                    c.row == row
                        && c.via.as_deref() == Some(via)
                        && now().saturating_sub(c.at) <= CONFIRM_FOR
                })
                .cloned();
            let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
            let existing = journal.spends.iter().position(|s| {
                s.row == row
                    && s.via.as_deref() == Some(via)
                    && (s.id == confirm || s.also.iter().any(|id| id == confirm))
            });
            if let Some(i) = existing {
                let kept = &journal.spends[i];
                if matches!(kept.state, State::Done) {
                    return Ok(self.done_with(
                        row,
                        kept.outcome.unwrap_or(Outcome::Unknown),
                        false,
                        kept.until,
                    ));
                }
            } else if ticket.is_none() {
                return Err(missing("That confirmation has expired: ask again."));
            }
            if existing.is_none() && (!unasked || !journal.allowed.get(row).is_some_and(|d| !d.on))
            {
                if let Some(i) = overlap(journal, row, Some(via), entered) {
                    let mut changed = journal.clone();
                    let kept = &mut changed.spends[i];
                    attach_alias(kept, confirm);
                    let outcome = kept.outcome.unwrap_or(Outcome::Unknown);
                    let until = kept.until;
                    self.save(&changed)?;
                    *journal = changed;
                    return Ok(self.done_with(row, outcome, false, until));
                }
            }
            let open = existing.or_else(|| {
                journal.spends.iter().rposition(|s| {
                    s.row == row
                        && s.via.as_deref() == Some(via)
                        && matches!(s.state, State::Sending)
                        && now().saturating_sub(s.at) < UNSETTLED_FOR
                })
            });
            if unasked && journal.allowed.get(row).is_some_and(|d| !d.on) {
                let mut done = self.done(row, Outcome::Failed, open.is_some());
                done["message"] = json!(
                    "Always Allow is off for this account now: nothing was used. Ask the user first."
                );
                return Ok(done);
            }
            if !self.transport.as_ref().is_some_and(|t| t.member(via)) {
                let mut done = self.done(row, Outcome::NotNow, open.is_some());
                done["message"] = json!(if open.is_some() {
                    "The computer with its sign-in isn't one of yours now, so it can't be asked again from here. An earlier try may have gone through: nothing more was used."
                } else {
                    "The computer with its sign-in isn't one of yours now, so it doesn't use your resets: nothing was used."
                });
                return Ok(done);
            }
            if !self.transport.as_ref().is_some_and(|t| t.reachable(via)) {
                return Ok(self.done(row, Outcome::NotNow, open.is_some()));
            }
            let mut changed = journal.clone();
            let later = open.is_some_and(|i| {
                existing.is_none() && now().saturating_sub(journal.spends[i].at) >= CONFIRM_FOR
            });
            let i = if let Some(i) = open {
                let spend = &mut changed.spends[i];
                if existing.is_none() && !later {
                    attach_alias(spend, confirm);
                }
                if !unasked {
                    spend.unasked = false;
                }
                i
            } else {
                prune_spends(&mut changed);
                changed
                    .spends
                    .push(remote_spend(row, confirm, by, unasked, via));
                changed.spends.len() - 1
            };
            self.save(&changed)?;
            *journal = changed;
            let spend = &journal.spends[i];
            (spend.id.clone(), remote_ask(spend), open.is_some(), later)
        };
        loop {
            let (sent, result) = self.remote_exchange(via, ask.clone()).await;
            let Some(answer) = result else {
                if later {
                    let mut inner = self.inner.lock().await;
                    let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
                    let mut changed = journal.clone();
                    if let Some(spend) = changed
                        .spends
                        .iter_mut()
                        .find(|s| s.id == id && s.via.as_deref() == Some(via))
                    {
                        attach_alias(spend, confirm);
                        self.save(&changed)?;
                        *journal = changed;
                    }
                }
                return Ok(self.done(
                    row,
                    if sent {
                        Outcome::Unknown
                    } else {
                        Outcome::NotNow
                    },
                    prior || sent,
                ));
            };
            let terminal = !answer.open && !answer.outcome.open();
            let next = later && terminal && (answer.outcome != Outcome::Reset || answer.already);
            {
                let mut inner = self.inner.lock().await;
                let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
                let mut changed = journal.clone();
                if let Some(spend) = changed
                    .spends
                    .iter_mut()
                    .find(|s| s.id == id && s.via.as_deref() == Some(via))
                {
                    if terminal {
                        spend.state = State::Done;
                        spend.outcome = Some(answer.outcome);
                        spend.until = answer.until;
                        spend.already = answer.already;
                        spend.finished = Some(now());
                    }
                    if later && !next {
                        attach_alias(spend, confirm);
                    }
                }
                if next {
                    if !self.transport.as_ref().is_some_and(|t| t.reachable(via)) {
                        self.save(&changed)?;
                        *journal = changed;
                        return Ok(self.done(row, Outcome::NotNow, false));
                    }
                    prune_spends(&mut changed);
                    changed
                        .spends
                        .push(remote_spend(row, confirm, by, unasked, via));
                    id = confirm.into();
                    ask = remote_ask(changed.spends.last().expect("new remote spend"));
                    prior = false;
                    later = false;
                }
                self.save(&changed)?;
                *journal = changed;
            }
            if next {
                continue;
            }
            let mut done = self.done_with(row, answer.outcome, answer.open, answer.until);
            if let Some(message) = answer.message {
                done["message"] = json!(message);
            }
            return Ok(done);
        }
    }
    async fn remote_exchange(&self, via: &str, ask: RemoteAsk) -> (bool, Option<RemoteAnswer>) {
        let key = (via.to_owned(), ask.id.clone());
        let (send, receive) = tokio::sync::oneshot::channel();
        self.pending
            .lock()
            .expect("reset answers")
            .insert(key.clone(), send);
        let sent = if let Some(transport) = &self.transport {
            transport.send(via, ResetMail::ask(ask)).await
        } else {
            false
        };
        let result = if sent {
            tokio::time::timeout(std::time::Duration::from_secs(45), receive)
                .await
                .ok()
                .and_then(Result::ok)
        } else {
            None
        };
        self.pending.lock().expect("reset answers").remove(&key);
        (sent, result)
    }
    pub fn take_answer(&self, from: &str, answer: RemoteAnswer) {
        if valid_id(&answer.id) {
            if let Some(send) = self
                .pending
                .lock()
                .expect("reset answers")
                .remove(&(from.to_owned(), answer.id.clone()))
            {
                let _ = send.send(answer);
            }
        }
    }
    pub async fn take_decision(&self, from: &str, decision: RemoteDecision) -> Answer<()> {
        if !shared_row(&decision.row) || !self.transport.as_ref().is_some_and(|t| t.member(from)) {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
        if journal
            .allowed
            .get(&decision.row)
            .is_some_and(|d| d.at > decision.at || d.at == decision.at && (!d.on || decision.on))
        {
            return Ok(());
        }
        let mut changed = journal.clone();
        changed.allowed.insert(
            decision.row,
            Decision {
                on: decision.on,
                at: decision.at,
                from: Some(from.into()),
            },
        );
        self.save(&changed)?;
        *journal = changed;
        Ok(())
    }
    pub async fn decisions(&self) -> Vec<RemoteDecision> {
        let mut inner = self.inner.lock().await;
        let Ok(journal) = inner.journal.as_mut() else {
            return vec![];
        };
        if let Some(transport) = &self.transport
            && transport.known()
        {
            let mut changed = journal.clone();
            changed
                .allowed
                .retain(|_, d| d.from.as_ref().is_none_or(|from| transport.member(from)));
            if changed.allowed.len() != journal.allowed.len() && self.save(&changed).is_ok() {
                *journal = changed;
            }
        }
        journal
            .allowed
            .iter()
            .filter(|(_, d)| d.from.is_none())
            .map(|(row, d)| RemoteDecision {
                row: row.clone(),
                on: d.on,
                at: d.at,
            })
            .collect()
    }
    pub async fn take_ask(&self, from: &str, ask: RemoteAsk) -> RemoteAnswer {
        let failed = |message: &str| RemoteAnswer {
            id: ask.id.clone(),
            outcome: Outcome::Failed,
            open: false,
            already: false,
            until: None,
            message: Some(message.into()),
        };
        if !valid_id(&ask.id) || !shared_row(&ask.row) {
            return failed("That isn't a reset request.");
        }
        if !self.transport.as_ref().is_some_and(|t| t.member(from)) {
            return failed("The computer isn't one of yours now: nothing was used.");
        }
        let key = format!("{from}:{}", ask.id);
        let prior_done = {
            let mut inner = self.inner.lock().await;
            let known = inner.journal.as_ref().is_ok_and(|j| {
                j.spends.iter().any(|s| {
                    s.row == ask.row && (s.id == key || s.also.iter().any(|id| id == &key))
                })
            });
            if known && !ask.unasked {
                if let Ok(journal) = inner.journal.as_mut() {
                    let mut changed = journal.clone();
                    if let Some(spend) = changed.spends.iter_mut().find(|s| {
                        s.row == ask.row
                            && (s.id == key || s.also.iter().any(|id| id == &key))
                            && s.unasked
                    }) {
                        spend.unasked = false;
                        if self.save(&changed).is_err() {
                            return failed("Couldn't keep the confirmed reset request.");
                        }
                        *journal = changed;
                    }
                }
            }
            let already = inner.journal.as_ref().is_ok_and(|j| {
                j.spends.iter().any(|s| {
                    s.row == ask.row
                        && (s.id == key || s.also.iter().any(|id| id == &key))
                        && matches!(s.state, State::Done)
                        && s.outcome == Some(Outcome::Reset)
                })
            });
            if !known {
                let Some((account, _)) = self.limits.reset_account(Some(&ask.row), None) else {
                    return failed("This computer's sign-in can't use this account's resets.");
                };
                inner.confirms.insert(
                    key.clone(),
                    Confirm {
                        row: ask.row.clone(),
                        at: now(),
                        account: Some(account),
                        via: None,
                    },
                );
            }
            already
        };
        // A peer may not substitute a provider or credit target. The local
        // account and its maker choose those, just as for a local confirmation.
        let result = self
            .spend(&ask.row, &key, ask.by.as_deref(), ask.unasked)
            .await;
        let answer = match result {
            Ok(done) => done,
            Err(error) => return failed(&refusal_words(error).await),
        };
        let outcome = serde_json::from_value(answer["outcome"].clone()).unwrap_or(Outcome::Unknown);
        let (open, already) = {
            let inner = self.inner.lock().await;
            let kept = inner.journal.as_ref().ok().and_then(|j| {
                j.spends.iter().find(|s| {
                    s.row == ask.row && (s.id == key || s.also.iter().any(|id| id == &key))
                })
            });
            (
                kept.is_some_and(|s| matches!(s.state, State::Sending)),
                prior_done || kept.is_some_and(|s| s.already),
            )
        };
        RemoteAnswer {
            id: ask.id,
            outcome,
            open,
            already: already && outcome == Outcome::Reset,
            until: answer["until"].as_u64(),
            message: answer["message"].as_str().map(str::to_owned),
        }
    }

    pub async fn spend_for(
        &self,
        row: &str,
        by: &str,
        always: bool,
        unasked: bool,
    ) -> (String, bool) {
        let result = async {
            if always {
                self.always(Some(row), None, true).await?;
            }
            let offer = self.offer(row).await?;
            let confirm = offer["confirm"]
                .as_str()
                .ok_or_else(|| conflict("The reset couldn't be asked for."))?;
            self.spend(row, confirm, Some(by), unasked).await
        }
        .await;
        match result {
            Ok(done) => {
                let name = done["row"]["title"].as_str().unwrap_or("the account");
                let (text, error) = match done["outcome"].as_str() {
                    Some("reset") => (
                        format!("Used one of {name}'s rate-limit resets: its limits started over."),
                        false,
                    ),
                    Some("unconfirmed") => (
                        format!(
                            "The reset of {name} may still be going through: read plan_limits again in a minute."
                        ),
                        false,
                    ),
                    _ => (
                        done["message"]
                            .as_str()
                            .unwrap_or("The reset didn't go through. Nothing was used.")
                            .to_owned(),
                        true,
                    ),
                };
                (text, error)
            }
            Err(error) => (
                format!(
                    "The reset didn't go through: {}",
                    refusal_words(error).await
                ),
                true,
            ),
        }
    }
    pub async fn find(&self, wanted: &str) -> Result<Value, String> {
        let rows = self.limits.answer(now())["data"]["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let name = wanted.trim().to_lowercase();
        if let Some(row) = rows
            .iter()
            .find(|row| row["id"] == wanted.trim())
            .or_else(|| {
                rows.iter()
                    .filter(|row| {
                        row["extras"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|e| e["kind"] == "resets"))
                    })
                    .find(|row| {
                        row["title"].as_str().is_some_and(|title| {
                            let t = title.to_lowercase();
                            t == name || t.starts_with(&(name.clone() + " "))
                        })
                    })
            })
        {
            return Ok(row.clone());
        }
        Err("None of the user's accounts has rate-limit resets by that name.".into())
    }
}
pub async fn refusal_words(error: Refusal) -> String {
    use http_body_util::BodyExt;
    let reply = error.reply();
    let value = match reply.body.collect().await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes.to_bytes()).unwrap_or_default(),
        Err(_) => Value::Null,
    };
    value["message"]
        .as_str()
        .unwrap_or("Couldn't use the reset.")
        .to_owned()
}
struct Reset;
impl RouteSpec for Reset {
    type Body = Value;
    const ROUTE: &'static Route = &Route {
        method: "POST",
        path: "kuvo/agents/api/kuvo/limits/reset",
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: Some(&Shape::Primitive(Primitive::Any)),
    };
}
impl JsonStatus<200> for Reset {
    type Body = Value;
}
struct Always;
impl RouteSpec for Always {
    type Body = Value;
    const ROUTE: &'static Route = &Route {
        method: "PUT",
        path: "kuvo/agents/api/kuvo/limits/reset/always",
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: Some(&Shape::Primitive(Primitive::Any)),
    };
}
impl JsonStatus<200> for Always {
    type Body = Value;
}
pub fn register(routes: &mut Registry, resets: Resets) {
    let post = resets.clone();
    routes.register::<Reset, _, _>(Access::Any, move |call| {
        let resets = post.clone();
        async move {
            let body: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
            let row = body["row"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad("Say which account (row)."))?;
            let answer = if let Some(confirm) = body.get("confirm") {
                let confirm = confirm
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| bad("That isn't a confirmation."))?;
                // A disconnected client must not interrupt a spend after it was journaled.
                let (row, confirm, by, unasked) = (
                    row.to_owned(),
                    confirm.to_owned(),
                    body["by"].as_str().map(str::to_owned),
                    body["unasked"] == true,
                );
                tokio::spawn(
                    async move { resets.spend(&row, &confirm, by.as_deref(), unasked).await },
                )
                .await
                .map_err(failure)??
            } else {
                resets.offer(row).await?
            };
            Ok(RouteReply::json::<200>(json!({"data":answer})))
        }
    });
    routes.register::<Always, _, _>(Access::Any, move |call| {
        let resets = resets.clone();
        async move {
            let body: Value = serde_json::from_slice(&call.request.body).unwrap_or_default();
            let row = body["row"].as_str();
            let provider = body["provider"].as_str();
            let on = body["always"]
                .as_bool()
                .filter(|_| row.is_some() || provider.is_some())
                .ok_or_else(|| bad("Say which account (row or provider), and always."))?;
            resets.always(row, provider, on).await?;
            Ok(RouteReply::json::<200>(json!({"data":{}})))
        }
    });
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::usage::{
        limits::Provider,
        plan_facts::{Extra, Facts},
        readings::{Source, Window},
        settings,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Maker {
        chose: AtomicUsize,
        used: AtomicUsize,
    }
    impl Port for Maker {
        fn choose<'a>(
            &'a self,
            _: &'a Account,
        ) -> futures_util::future::BoxFuture<'a, Result<String, Outcome>> {
            Box::pin(async move {
                self.chose.fetch_add(1, Ordering::SeqCst);
                Ok("fixture-credit".into())
            })
        }
        fn consume<'a>(
            &'a self,
            _: &'a Account,
            _: &'a str,
            _: &'a str,
        ) -> futures_util::future::BoxFuture<'a, Completion> {
            Box::pin(async move {
                self.used.fetch_add(1, Ordering::SeqCst);
                Completion::maker(Outcome::Reset)
            })
        }
        fn refresh<'a>(&'a self, _: &'a str) -> futures_util::future::BoxFuture<'a, ()> {
            Box::pin(async {})
        }
    }
    #[derive(Default)]
    struct Directory(AtomicBool);
    impl PeerTransport for Directory {
        fn known(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn member(&self, key: &str) -> bool {
            self.known() && key == "fixture-peer"
        }
        fn reachable(&self, key: &str) -> bool {
            self.member(key)
        }
        fn send<'a>(
            &'a self,
            _: &'a str,
            _: ResetMail,
        ) -> futures_util::future::BoxFuture<'a, bool> {
            Box::pin(async { false })
        }
    }

    #[tokio::test]
    async fn a_kept_allowance_cannot_spend_before_the_directory_is_known_or_after_the_holder_turns_it_off()
     {
        let home = tempfile::tempdir().unwrap();
        let setup = kuvo_setup::SetupRuntime::prepare(home.path(), "Fixture".into(), false, None)
            .await
            .unwrap()
            .start(None)
            .unwrap();
        let prefs = settings::Service::new(home.path().to_owned(), setup.handle());
        let limits = Service::new(
            home.path().to_owned(),
            prefs,
            Default::default(),
            "Fixture".into(),
            "fixture-local".into(),
        );
        let account = Account {
            provider: "codex-cli".into(),
            service: Provider::Codex,
            account: Some("default".into()),
            label: None,
            identity: Some("0123456789abcdef".into()),
            sign_in: "fixture".into(),
            notice_card: None,
        };
        let row = account.row("fixture-local");
        let at = now();
        limits
            .observe_facts(
                account,
                vec![Window {
                    id: "primary".into(),
                    used: 1.0,
                    minutes: Some(300),
                    resets_at: Some(at + 3600000),
                    model: None,
                    name: None,
                    amount: None,
                    read: at,
                    source: Source::Usage,
                }],
                Some(Facts {
                    checked: Some(at),
                    extras: vec![Extra::Resets {
                        id: "resets".into(),
                        name: "Rate Limit Resets".into(),
                        count: 1,
                        expiries: vec![at + 3600000],
                        refills: vec!["primary".into()],
                        usable: true,
                    }],
                    ..Default::default()
                }),
                at,
            )
            .unwrap();
        let maker = Arc::new(Maker::default());
        let directory = Arc::new(Directory::default());
        let resets = Resets::new(home.path(), limits, maker.clone()).with_peers(directory.clone());
        assert!(resets.always(Some(&row), None, true).await.is_ok());
        let refused = resets.spend_for(&row, "fixture-agent", false, true).await;
        assert!(refused.1);
        assert!(refused.0.contains("Ask the user"), "{:?}", refused);
        assert_eq!(maker.chose.load(Ordering::SeqCst), 0);
        assert_eq!(maker.used.load(Ordering::SeqCst), 0);
        let saved = std::fs::read(home.path().join("agents/limits-resets.json")).unwrap();
        assert!(
            serde_json::from_slice::<Journal>(&saved)
                .unwrap()
                .spends
                .is_empty()
        );
        directory.0.store(true, Ordering::SeqCst);
        assert!(resets.always(Some(&row), None, false).await.is_ok());
        let answer = resets
            .take_ask(
                "fixture-peer",
                RemoteAsk {
                    id: "fixture-ask".into(),
                    row: row.clone(),
                    by: Some("fixture-agent".into()),
                    unasked: true,
                },
            )
            .await;
        assert_eq!(answer.outcome, Outcome::Failed);
        assert!(
            answer
                .message
                .unwrap_or_default()
                .contains("Always Allow is off")
        );
        assert_eq!(maker.chose.load(Ordering::SeqCst), 0);
        assert_eq!(maker.used.load(Ordering::SeqCst), 0);
        assert!(resets.always(Some(&row), None, true).await.is_ok());
        let done = resets.spend_for(&row, "fixture-agent", false, true).await;
        assert!(!done.1, "{:?}", done);
        assert_eq!(maker.used.load(Ordering::SeqCst), 1);
    }
}
