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
use serde_json::{Value, json};
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
        // (Its plan's limits are read from its own database: it has no plan sign-in here.)
        AgentKind::OpenCode => limits::Provider::Opencode,
    };
    Account {
        provider: provider.to_string(),
        service,
        account: Some(provider.account.clone()),
        label: entry.account.label.clone(),
        identity: Some(Account::identity_of(&raw_identity)),
        notice_card: Account::notice_card_of(&raw_identity),
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

/// Account admission is cheap local work, not a quota refresh. In particular a
/// signed-out CLI can keep all of its transcripts and no longer has live rings.
pub async fn verify_sign_ins(
    service: &Service,
    agents: Option<&ResidentAgents>,
    standalone: Option<&kuvo_services::usage::standalone::Readers>,
    engine: &kuvo_engine::Engine,
    home: &std::path::Path,
) {
    let native = async {
        if let Some(agents) = agents {
            confirm(service, agents).await;
        } else {
            service.confirm_accounts(&[], limits::now());
        }
    };
    let standalone = async {
        if let Some(readers) = standalone {
            readers.confirm(service).await;
        } else {
            service.confirm_standalone_accounts(&[], &Default::default(), limits::now());
        }
    };
    let engine = async {
        if !service.has_engine_accounts() {
            return;
        }
        // A current engine keeps this evidence in SQLite. The legacy API
        // fallback addresses only an already-running engine; don't let a
        // blocked legacy process delay drawing the rings.
        let current = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            engine_credentials(engine, home.to_owned()),
        )
        .await;
        match current {
            Ok(Ok(Some(credentials))) => {
                if let Some(accounts) =
                    kuvo_services::usage::engine_readers::admitted_accounts(&credentials)
                {
                    service.confirm_engine_sign_ins(&accounts, limits::now());
                }
            }
            Ok(Ok(None)) => service.confirm_engine_sign_ins(&[], limits::now()),
            _ => {} // An unreadable credential source is not a confirmed sign-out.
        }
    };
    tokio::join!(native, standalone, engine);
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
        if !explicit
            && selected.is_none()
            && service.defer_passive_provider(
                &provider.to_string(),
                limits::now(),
                if provider.kind == AgentKind::Claude {
                    15 * 60_000
                } else {
                    10 * 60_000
                },
            )
        {
            continue;
        }
        let started = limits::now();
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
                let only_resets = reading.only_resets;
                let windows = windows(reading.reading, at, Source::Usage);
                let service = service.clone();
                match tokio::task::spawn_blocking(move || {
                    if only_resets {
                        service.observe_reset_facts(account, facts, at)
                    } else {
                        service.observe_confirmed_facts(account, windows, facts, started, at)
                    }
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
    use kuvo_agents::session::DirectPlanStatus;
    facts.problem = match direct.status {
        DirectPlanStatus::Ready => None,
        DirectPlanStatus::Failed => Some(Problem::new(
            ProblemKind::Failed,
            format!("{} didn't answer about its plan.", kind.display_name()),
        )),
        DirectPlanStatus::NoPlan => Some(Problem::new(
            ProblemKind::None,
            "Not signed in to a plan here.",
        )),
    };
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
            resets_at: None,
        });
    }
    if let Some(resets) = &direct.resets {
        facts.extras.push(Extra::Resets {
            id: "resets".into(),
            name: "Rate Limit Resets".into(),
            count: resets.count,
            expiries: resets.display_expiries.clone().unwrap_or_else(|| {
                resets
                    .targets
                    .iter()
                    .filter_map(|t| t.expires_at)
                    .take(50)
                    .collect()
            }),
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

async fn engine_credentials(
    engine: &kuvo_engine::Engine,
    home: std::path::PathBuf,
) -> Result<Option<Value>, String> {
    match kuvo_services::usage::engine_readers::kept_credentials(home).await? {
        Some(credentials) => Ok(Some(credentials)),
        None => {
            let Some(engine) = engine.running_bot_engine().await else {
                return Ok(None);
            };
            engine
                .call(
                    http::Method::GET,
                    "api/credential",
                    None,
                    std::time::Duration::from_secs(15),
                )
                .await
                .map(Some)
                .map_err(|e| e.to_string())
        }
    }
}

pub async fn refresh_engine(
    service: Service,
    engine: kuvo_engine::Engine,
    reader: kuvo_services::usage::engine_readers::Reader,
    home: std::path::PathBuf,
    selected: Option<String>,
    explicit: bool,
) -> bool {
    // Never wake the engine solely to poll quota. A saved reading is usable only
    // when the engine still has the matching active credential.
    if selected
        .as_deref()
        .is_some_and(|p| Provider::parse(p).is_some())
    {
        return false;
    }
    let credentials = match engine_credentials(&engine, home.clone()).await {
        Ok(Some(credentials)) => credentials,
        Ok(None) => {
            service.confirm_engine_sign_ins(&[], limits::now());
            return false;
        }
        Err(error) => {
            eprintln!("Could not read engine plan accounts: {error}");
            return true;
        }
    };
    let at = limits::now();
    if let Some(accounts) = kuvo_services::usage::engine_readers::admitted_accounts(&credentials) {
        service.confirm_engine_sign_ins(&accounts, at);
    }
    let mut held = service.held_engine_providers(at);
    if !explicit
        && selected.is_none()
        && let Some(accounts) =
            kuvo_services::usage::engine_readers::admitted_accounts(&credentials)
    {
        for account in accounts {
            if !held.contains(&account.provider) && service.defer_passive(&account, at, 10 * 60_000)
            {
                held.insert(account.provider);
            }
        }
    }
    let Some(mut batch) = reader
        .read(credentials, at, &held, selected.as_deref())
        .await
    else {
        return true;
    };
    // A maker's reply can arrive after the user has changed or removed a key.
    // The current SQLite credential is authoritative, never the old response.
    match engine_credentials(&engine, home).await {
        Ok(Some(credentials)) => {
            let Some(accounts) =
                kuvo_services::usage::engine_readers::admitted_accounts(&credentials)
            else {
                return true;
            };
            service.confirm_engine_sign_ins(&accounts, limits::now());
            batch.plans.retain(|p| {
                accounts
                    .iter()
                    .any(|a| a.provider == p.account.provider && a.sign_in == p.account.sign_in)
            });
            batch
                .providers
                .retain(|p| accounts.iter().any(|a| &a.provider == p));
        }
        Ok(None) => {
            service.confirm_engine_sign_ins(&[], limits::now());
            return false;
        }
        Err(error) => {
            eprintln!("Could not recheck engine plan accounts: {error}");
            return true;
        }
    }
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
    pub async fn follow(
        service: Service,
        agents: ResidentAgents,
        standalone: Option<kuvo_services::usage::standalone::Readers>,
    ) {
        let mut feed = agents.feed().subscribe();

        loop {
            match feed.recv().await {
                Ok(event) if event.kind == "kuvo.agent.account.changed" => {}
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
            confirm(&service, &agents).await;
            if let Some(readers) = &standalone {
                readers.confirm(&service).await;
            }
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
        notices
            .keep_at(
                notice,
                false,
                i64::try_from(fact.at).unwrap_or(i64::MAX),
                |kept| {
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
                },
            )
            .is_some()
    }));
}

pub struct ResetPort {
    pub agents: Option<ResidentAgents>,
    pub limits: Service,
    pub engine: Option<(
        kuvo_engine::Engine,
        kuvo_services::usage::engine_readers::Reader,
        std::path::PathBuf,
    )>,
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
            if account.provider == "openai" && account.account.is_none() {
                let (engine, reader, home) = self.engine.as_ref().ok_or(Outcome::NotNow)?;
                let credentials = engine_credentials(engine, home.clone())
                    .await
                    .map_err(|_| Outcome::NotNow)?
                    .ok_or(Outcome::NotNow)?;
                return reader
                    .choose_reset(credentials, account, limits::now())
                    .await;
            }
            let (agent, _) = self.agent(account).await?;
            let plan = agent
                .read_plan_limits(account.service == kuvo_services::usage::limits::Provider::Codex)
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
    ) -> futures_core::future::BoxFuture<'a, kuvo_services::usage::resets::Completion> {
        use kuvo_agents::session::ResetOutcome as Native;
        use kuvo_services::usage::resets::{Completion, Outcome};
        Box::pin(async move {
            if account.provider == "openai" && account.account.is_none() {
                let Some((engine, reader, home)) = &self.engine else {
                    return Outcome::NotNow.into();
                };
                let Ok(Some(credentials)) = engine_credentials(engine, home.clone()).await else {
                    return Outcome::NotNow.into();
                };
                return reader
                    .consume_reset(credentials, account, request, target, limits::now())
                    .await;
            }
            let (agent, identity) = match self.agent(account).await {
                Ok(a) => a,
                Err(o) => return o.into(),
            };
            match agent
                .consume_plan_reset(&identity, request, target)
                .await
                .unwrap_or(Native::Unknown)
            {
                Native::Reset => Completion::maker(Outcome::Reset),
                Native::AlreadyRedeemed => Completion {
                    already: true,
                    ..Completion::maker(Outcome::Reset)
                },
                Native::NothingToReset => Completion::maker(Outcome::NothingToReset),
                Native::MakerNoReset => Completion::maker(Outcome::NoReset),
                Native::MakerFailed => Completion::maker(Outcome::Failed),
                Native::MakerNotNow(until) => Completion {
                    until,
                    ..Completion::maker(Outcome::NotNow)
                },
                Native::NoReset => Outcome::NoReset.into(),
                Native::NotNow => Outcome::NotNow.into(),
                Native::Failed => Outcome::Failed.into(),
                Native::Unconfirmed => Completion::maker(Outcome::Unconfirmed),
                Native::Unknown => Completion::maker(Outcome::Unknown),
            }
        })
    }
    fn refresh<'a>(&'a self, provider: &'a str) -> futures_core::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            if provider == "openai" {
                if let Some((engine, reader, home)) = &self.engine {
                    refresh_engine(
                        self.limits.clone(),
                        engine.clone(),
                        reader.clone(),
                        home.clone(),
                        Some(provider.into()),
                        true,
                    )
                    .await;
                }
                return;
            }
            if let Some(agents) = self.agents.as_ref() {
                refresh(
                    self.limits.clone(),
                    agents.clone(),
                    Some(provider.into()),
                    false,
                )
                .await;
            }
        })
    }
}
