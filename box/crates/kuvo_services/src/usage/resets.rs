//! A reset spends the user's plan credit. The offer is short lived; a spend and
//! its maker's request and credit are kept before it can leave this computer.
use super::{
    limits::{Account, Service, now},
};
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
    ) -> futures_util::future::BoxFuture<'a, Outcome>;
    fn refresh<'a>(&'a self, provider: &'a str) -> futures_util::future::BoxFuture<'a, ()>;
}

#[derive(Clone)]
struct Confirm {
    row: String,
    at: u64,
    account: Account,
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
    provider: String,
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
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Decision {
    on: bool,
    at: u64,
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
        }
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
                decision.at = now().max(decision.at.saturating_add(1));
            }
        }
        self.save(&changed)?;
        *journal = changed;
        Ok(())
    }
    pub async fn always(&self, row: Option<&str>, provider: Option<&str>, on: bool) -> Answer<()> {
        let (account, row) = self
            .limits
            .reset_account(row, provider)
            .ok_or_else(|| missing("No account of yours by that name has resets."))?;
        if account.identity.is_none() {
            return Err(conflict(
                "This account doesn't say who it is yet, so Always Allow can't be kept for it.",
            ));
        }
        let mut inner = self.inner.lock().await;
        let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
        self.see(journal, &account, &row)?;
        let mut changed = journal.clone();
        let at = now().max(
            changed
                .allowed
                .get(&row)
                .map_or(0, |d| d.at.saturating_add(1)),
        );
        changed.allowed.insert(row, Decision { on, at });
        self.save(&changed)?;
        *journal = changed;
        Ok(())
    }
    pub async fn offer(&self, row: &str) -> Answer<Value> {
        let (account, _) = self
            .limits
            .reset_account(Some(row), None)
            .ok_or_else(|| missing("No account of yours by that name."))?;
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
        let mut inner = self.inner.lock().await;
        let journal = inner.journal.as_mut().map_err(|e| failure(&*e))?;
        self.see(journal, &account, row)?;
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
            },
        );
        let mut offer = json!({"confirm":confirm,"row":row,"service":card["service"],"count":count,"on":self.limits.here(),"always":always,"expires":at+CONFIRM_FOR});
        if reset["refills"].as_array().is_some_and(|a| !a.is_empty()) {
            offer["refills"] = reset["refills"].clone();
        }
        Ok(offer)
    }
    fn done(&self, row: &str, outcome: Outcome, open: bool) -> Value {
        let mut done = json!({"outcome":outcome});
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
            if matches!(journal.spends[i].state, State::Done) {
                return Ok(self.done(
                    row,
                    journal.spends[i].outcome.unwrap_or(Outcome::Unknown),
                    false,
                ));
            }
        } else if ticket.is_none() {
            return Err(conflict(
                "That confirmation has expired: ask for the reset again.",
            ));
        }
        let open = existing.or_else(|| {
            journal
                .spends
                .iter()
                .position(|s| s.row == row && matches!(s.state, State::Sending))
        });
        let account = if let Some(i) = open {
            self.limits
                .reset_account(Some(row), Some(&journal.spends[i].provider))
                .map(|(a, _)| a)
        } else {
            ticket.map(|c| c.account)
        };
        let Some(account) = account else {
            return Ok(self.done(row, Outcome::NotNow, open.is_some()));
        };
        // A previous unresolved request never becomes a new one. It can be confirmed by a
        // person even if previously attempted under Always Allow.
        self.see(journal, &account, row)?;
        if (unasked || existing.is_some_and(|i| journal.spends[i].unasked))
            && !Self::allowed(journal, row)
        {
            let mut done = self.done(row, Outcome::Failed, open.is_some());
            done["message"] = json!(
                "Always Allow is off for this account now: nothing was used. Ask the user first."
            );
            return Ok(done);
        }
        let mut changed = journal.clone();
        let i = if let Some(i) = open {
            let s = &mut changed.spends[i];
            if existing.is_none() {
                s.also.push(confirm.into());
            }
            if !unasked {
                s.unasked = false;
            }
            i
        } else {
            changed.spends.push(Spend {
                id: confirm.into(),
                row: row.into(),
                provider: account.provider.clone(),
                request: random::<16>()?,
                target: None,
                also: vec![],
                by: by.map(|s| s.chars().take(200).collect()),
                unasked,
                at: now(),
                state: State::Sending,
                outcome: None,
            });
            changed.spends.len() - 1
        };
        self.save(&changed)?;
        *journal = changed;
        let previously_sent = journal.spends[i].target.is_some();
        let target = match journal.spends[i].target.clone() {
            Some(target) => Ok(target),
            None => self.port.choose(&account).await,
        };
        let outcome = match target {
            Ok(target) => {
                let mut changed = journal.clone();
                changed.spends[i].target = Some(target.clone());
                self.save(&changed)?;
                *journal = changed;
                self.port
                    .consume(&account, &journal.spends[i].request, &target)
                    .await
            }
            Err(outcome) => outcome,
        };
        // A failure before a retry reached its maker cannot settle an earlier lost response.
        let still_open = outcome.open()
            || (previously_sent
                && matches!(
                    outcome,
                    Outcome::NotNow | Outcome::Failed | Outcome::NoReset
                ));
        if !still_open {
            let mut changed = journal.clone();
            changed.spends[i].state = State::Done;
            changed.spends[i].outcome = Some(outcome);
            self.save(&changed)?;
            *journal = changed;
        }
        if matches!(outcome, Outcome::Reset | Outcome::Unconfirmed) {
            self.port.refresh(&account.provider).await;
        }
        eprintln!("Plan reset of {}: {outcome:?}", account.provider);
        Ok(self.done(row, outcome, still_open))
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
