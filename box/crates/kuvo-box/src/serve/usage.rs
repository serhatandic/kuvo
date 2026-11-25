//! Join the agents' turn observation seam to the usage service and the existing notice history.
use kuvo_agents::{
    PlanLimits,
    accounts::{AgentKind, Provider},
    resident::ResidentAgents,
    session::PlanLimitReading,
};
use kuvo_services::usage::{
    limits::{self, Account, Service},
    readings::{Milestone, Source, Window},
};
use kuvo_work::notices::{Notice, Notices};
use serde_json::json;
use std::sync::Arc;

fn account_of(
    entry: &kuvo_agents::roster::Entry,
    provider: &Provider,
    sign_in: String,
    raw_identity: String,
) -> Account {
    let service = match provider.kind {
        AgentKind::Claude => limits::Provider::Claude,
        AgentKind::Codex => limits::Provider::Codex,
        AgentKind::Antigravity => limits::Provider::Antigravity,
        AgentKind::Cursor => limits::Provider::Cursor,
        AgentKind::Grok => limits::Provider::Grok,
    };
    Account {
        provider: provider.to_string(),
        service,
        account: Some(provider.account.clone()),
        label: entry.account.label.clone(),
        identity: Some(Account::identity_of(&raw_identity)),
        sign_in,
    }
}

pub async fn confirm(service: &Service, agents: &ResidentAgents) {
    let roster = agents.roster();
    let mut accounts = vec![];
    for entry in roster.all() {
        if let Some((mark, identity)) = entry.agent.plan_sign_in().await {
            accounts.push(account_of(entry, &entry.account.provider(), mark, identity));
        }
    }
    service.confirm_accounts(&accounts, limits::now());
}

fn windows(reading: PlanLimitReading, at: u64, source: Source) -> Vec<Window> {
    reading
        .windows
        .into_iter()
        .map(|w| Window {
            id: w.id,
            used: w.used,
            minutes: w.minutes,
            resets_at: w.resets_at,
            model: w.model,
            name: None,
            amount: None,
            read: at,
            source,
        })
        .collect()
}
pub async fn refresh(
    service: Service,
    agents: ResidentAgents,
    selected: Option<String>,
    explicit: bool,
) -> bool {
    let roster = agents.roster();
    let mut failed = false;
    for entry in roster.all() {
        let provider = entry.account.provider();
        if selected
            .as_ref()
            .is_some_and(|id| id != &provider.to_string())
        {
            continue;
        }
        match entry.agent.read_plan_limits(explicit).await {
            Ok(Some(reading)) => {
                let Some((mark, identity)) = entry.agent.plan_sign_in().await else {
                    continue;
                };
                if reading.reading.sign_in.as_deref() != Some(&mark) {
                    continue;
                }
                let account = account_of(entry, &provider, mark, identity);
                let at = limits::now();
                let facts = direct_facts(&reading, at, provider.kind);
                let windows = windows(reading.reading, at, Source::Usage);
                let service = service.clone();
                match tokio::task::spawn_blocking(move || {
                    service.observe_facts(account, windows, Some(facts), at)
                })
                .await
                {
                    Ok(Ok(())) => {}
                    error => {
                        failed = true;
                        eprintln!("Could not keep plan limits: {error:?}");
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                failed = true;
                eprintln!("Could not refresh {} limits: {error}", provider);
            }
        }
    }
    failed
}

fn direct_facts(
    direct: &kuvo_agents::session::DirectPlanReading,
    at: u64,
    kind: AgentKind,
) -> kuvo_services::usage::plan_facts::Facts {
    use kuvo_services::usage::plan_facts::{Extra, Facts, Problem, ProblemKind, Unit};
    let mut facts = Facts {
        plan: direct.plan.clone(),
        checked: Some(at),
        next: Some(
            at + if kind == AgentKind::Claude {
                15 * 60_000
            } else {
                10 * 60_000
            },
        ),
        ..Default::default()
    };
    if direct.no_plan {
        facts.problem = Some(Problem::new(
            ProblemKind::None,
            "Not signed in to a plan here.",
        ));
    }
    if let Some(spend) = &direct.extra_usage {
        let (id, name) = ("extra_usage".to_owned(), "Extra Usage".to_owned());
        facts.extras.push(if let Some(limit) = spend.limit {
            Extra::Capped {
                id,
                name,
                amount: spend.amount,
                limit,
                unit: Unit::Usd,
                resets_at: spend.resets_at,
            }
        } else {
            Extra::Spent {
                id,
                name,
                amount: spend.amount,
                unit: Unit::Usd,
                resets_at: spend.resets_at,
            }
        });
    }
    if let Some(credits) = direct.credits {
        facts.extras.push(Extra::Left {
            id: "credits".into(),
            name: "Extra Usage".into(),
            amount: credits,
            unit: Unit::Credits,
        });
    }
    if let Some(resets) = &direct.resets {
        facts.extras.push(Extra::Resets {
            id: "resets".into(),
            name: "Rate Limit Resets".into(),
            count: resets.count,
            expiries: resets
                .targets
                .iter()
                .filter_map(|t| t.expires_at)
                .take(50)
                .collect(),
            refills: resets.refills.clone(),
            usable: resets.count > 0 && !resets.targets.is_empty(),
        });
    } else if let Some(message) = &direct.reset_unreadable {
        facts.extras.push(Extra::Unreadable {
            id: "resets".into(),
            name: "Rate Limit Resets".into(),
            message: message.clone(),
        });
    }
    facts
}

pub fn engine_reader(
    home: &std::path::Path,
    account: &std::path::Path,
) -> Option<kuvo_services::usage::engine_readers::Reader> {
    use kuvo_services::usage::engine_readers::Reader;
    if std::env::var("KUVO_TEST_PATH_ONLY").as_deref() == Ok("1") {
        return Reader::fixture(std::env::var("KUVO_TEST_USAGE_BASE").ok()?.parse().ok()?).ok();
    }
    if home != account.join(".kuvo-code")
        || std::env::var("KUVO_TEST_PRICE_FEEDS").as_deref() == Ok("off")
    {
        return None;
    }
    Reader::new().ok()
}

pub async fn refresh_engine(
    service: Service,
    engine: kuvo_engine::Engine,
    reader: kuvo_services::usage::engine_readers::Reader,
    selected: Option<String>,
) -> bool {
    // Never wake the engine solely to poll quota. If it hasn't opened its credential
    // store, the last readings stay as of when their makers supplied them.
    if selected
        .as_deref()
        .is_some_and(|p| Provider::parse(p).is_some())
    {
        return false;
    }
    let Some(engine) = engine.running_bot_engine().await else {
        return false;
    };
    let credentials = match engine
        .call(
            http::Method::GET,
            "api/credential",
            None,
            std::time::Duration::from_secs(15),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            eprintln!("Could not read engine plan accounts: {error}");
            return true;
        }
    };
    let at = limits::now();
    let held = service.held_engine_providers(at);
    let Some(batch) = reader
        .read(credentials, at, &held, selected.as_deref())
        .await
    else {
        return true;
    };
    match tokio::task::spawn_blocking(move || {
        let mut failed = false;
        for plan in batch.plans {
            if plan
                .facts
                .problem
                .as_ref()
                .is_some_and(|p| p.kind == kuvo_services::usage::plan_facts::ProblemKind::Failed)
            {
                failed = true;
            }
            if let Err(error) =
                service.observe_facts(plan.account, plan.windows, Some(plan.facts), at)
            {
                failed = true;
                eprintln!("Could not keep engine plan limits: {error}");
            }
        }
        service.confirm_engine_accounts(&batch.providers, at);
        failed
    })
    .await
    {
        Ok(failed) => failed,
        Err(error) => {
            eprintln!("Could not keep engine plan accounts: {error}");
            true
        }
    }
}

pub struct Turns {
    queue: tokio::sync::mpsc::UnboundedSender<(String, PlanLimitReading, u64)>,
}
impl Turns {
    pub async fn follow(service: Service, agents: ResidentAgents) {
        let mut feed = agents.feed().subscribe();

        loop {
            match feed.recv().await {
                Ok(event) if event.kind == "kuvo.agent.account.changed" => {}
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
            confirm(&service, &agents).await;
        }
    }

    pub fn start(service: Service, agents: ResidentAgents) -> Arc<Self> {
        let (queue, mut readings) =
            tokio::sync::mpsc::unbounded_channel::<(String, PlanLimitReading, u64)>();
        tokio::spawn(async move {
            while let Some((account, reading, at)) = readings.recv().await {
                let Some(provider) = Provider::parse(&account) else {
                    continue;
                };
                let Some(sign_in) = reading.sign_in.as_ref() else {
                    continue;
                };
                let roster = agents.roster();
                let Some(entry) = roster.find(&provider) else {
                    continue;
                };
                let Some((current_mark, raw_identity)) = entry.agent.plan_sign_in().await else {
                    continue;
                };
                if &current_mark != sign_in {
                    continue;
                }
                let account = account_of(entry, &provider, current_mark, raw_identity);
                let windows = windows(reading, at, Source::Turn);
                let service = service.clone();
                match tokio::task::spawn_blocking(move || service.observe(account, windows, at))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("Could not keep plan limits: {error}"),
                    Err(error) => eprintln!("Could not keep plan limits: {error}"),
                }
            }
        });
        Arc::new(Self { queue })
    }
}
impl PlanLimits for Turns {
    fn heard(&self, account: &str, reading: &PlanLimitReading) {
        let _ = self
            .queue
            .send((account.into(), reading.clone(), limits::now()));
    }
}
pub fn notices(service: &Service, notices: Notices) {
    service.attach_notices(Arc::new(move |fact| {
        let (title, body) = match fact.milestone {
            Milestone::UnderTenPercent => {
                ("Almost Out", "Under 10% usage remaining for this window.")
            }
            Milestone::HealthyToClose => (
                "Cutting It Close",
                "Projected to finish close to your limit.",
            ),
            Milestone::CloseToRunningOut => (
                "Will Run Out",
                "Projected to finish before the limit resets.",
            ),
        };
        let mut data = json!({"notice":fact.id,"row":fact.row,"at":fact.at.to_string()});
        if let Some(at) = fact.resets_at {
            data["expires"] = json!(at.to_string());
        }
        let id = fact.id.clone();
        let notice = Notice {
            category: "usage".into(),
            title: title.into(),
            subtitle: Some(format!("{} {}", fact.account_name, fact.window_name)),
            body: Some(body.into()),
            thread: Some(format!("usage:{}", fact.row)),
            data: data.as_object().expect("notice data").clone(),
        };
        let _ = notices.keep(notice, false, |kept| {
            kept.iter()
                .any(|entry| {
                    entry
                        .notice
                        .data
                        .get("notice")
                        .and_then(serde_json::Value::as_str)
                        == Some(id.as_str())
                })
                .then(|| "Already told.".into())
        });
    }));
}

pub struct ResetPort {
    pub agents: Option<ResidentAgents>,
    pub limits: Service,
}
impl ResetPort {
    async fn agent(
        &self,
        account: &Account,
    ) -> Result<(Arc<dyn kuvo_agents::roster::Agent>, String), kuvo_services::usage::resets::Outcome>
    {
        use kuvo_services::usage::resets::Outcome;
        let agents = self.agents.as_ref().ok_or(Outcome::NotNow)?;
        let provider = Provider::parse(&account.provider).ok_or(Outcome::NoReset)?;
        let roster = agents.roster();
        let entry = roster.find(&provider).ok_or(Outcome::NotNow)?;
        let (_, raw) = entry.agent.plan_sign_in().await.ok_or(Outcome::NotNow)?;
        if account.identity.as_ref() != Some(&Account::identity_of(&raw)) {
            return Err(Outcome::NoReset);
        }
        Ok((entry.agent.clone(), raw))
    }
}
impl kuvo_services::usage::resets::Port for ResetPort {
    fn choose<'a>(
        &'a self,
        account: &'a Account,
    ) -> futures_core::future::BoxFuture<'a, Result<String, kuvo_services::usage::resets::Outcome>>
    {
        use kuvo_services::usage::resets::Outcome;
        Box::pin(async move {
            let (agent, _) = self.agent(account).await?;
            let plan = agent
                .read_plan_limits(false)
                .await
                .map_err(|_| Outcome::NotNow)?
                .ok_or(Outcome::NotNow)?;
            self.agent(account).await?;
            plan.resets
                .and_then(|r| r.targets.into_iter().next())
                .map(|t| t.id)
                .ok_or(Outcome::NoReset)
        })
    }
    fn consume<'a>(
        &'a self,
        account: &'a Account,
        request: &'a str,
        target: &'a str,
    ) -> futures_core::future::BoxFuture<'a, kuvo_services::usage::resets::Outcome> {
        use kuvo_agents::session::ResetOutcome as Native;
        use kuvo_services::usage::resets::Outcome;
        Box::pin(async move {
            let (agent, identity) = match self.agent(account).await {
                Ok(a) => a,
                Err(o) => return o,
            };
            match agent
                .consume_plan_reset(&identity, request, target)
                .await
                .unwrap_or(Native::Unknown)
            {
                Native::Reset | Native::AlreadyRedeemed => Outcome::Reset,
                Native::NothingToReset => Outcome::NothingToReset,
                Native::NoReset => Outcome::NoReset,
                Native::NotNow => Outcome::NotNow,
                Native::Failed => Outcome::Failed,
                Native::Unconfirmed => Outcome::Unconfirmed,
                Native::Unknown => Outcome::Unknown,
            }
        })
    }
    fn refresh<'a>(&'a self, provider: &'a str) -> futures_core::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(agents) = self.agents.as_ref() {
                refresh(self.limits.clone(), agents.clone(), Some(provider.into()), false).await;
            }
        })
    }
}
