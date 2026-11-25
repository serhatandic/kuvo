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
            read: at,
            source,
        })
        .collect()
}
pub async fn refresh(service: Service, agents: ResidentAgents, selected: Option<String>) -> bool {
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
        match entry.agent.read_plan_limits().await {
            Ok(Some(reading)) => {
                let Some((mark, identity)) = entry.agent.plan_sign_in().await else {
                    continue;
                };
                if reading.sign_in.as_deref() != Some(&mark) {
                    continue;
                }
                let account = account_of(entry, &provider, mark, identity);
                let at = limits::now();
                let windows = windows(reading, at, Source::Usage);
                let service = service.clone();
                match tokio::task::spawn_blocking(move || service.observe(account, windows, at))
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
