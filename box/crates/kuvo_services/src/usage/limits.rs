//! Resident-local accepted plan limits and their client projections.
use super::{
    readings::{Milestone, Trail, Window},
    settings,
};
use crate::limits_menu::MenuLimits;
use http::StatusCode;
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    shape::{Primitive, Shape},
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Antigravity,
    Cursor,
    Grok,
    Copilot,
    Openrouter,
    Zai,
    Ollama,
    Opencode,
}
impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::Copilot => "copilot",
            Self::Openrouter => "openrouter",
            Self::Zai => "zai",
            Self::Ollama => "ollama",
            Self::Opencode => "opencode",
        }
    }
    fn title(self, agent: bool) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex if agent => "Codex",
            Self::Codex => "ChatGPT",
            Self::Antigravity => "Antigravity",
            Self::Cursor => "Cursor",
            Self::Grok if agent => "Grok",
            Self::Grok => "SuperGrok",
            Self::Copilot => "Copilot",
            Self::Openrouter => "OpenRouter",
            Self::Zai => "Z.ai",
            Self::Ollama => "Ollama",
            Self::Opencode => "OpenCode Go",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub provider: String,
    pub service: Provider,
    pub account: Option<String>,
    pub label: Option<String>,
    // Identity is a domain-separated hash of the maker's account id, not the credential.
    pub identity: Option<String>,
    pub sign_in: String,
}
impl Account {
    pub fn row(&self, key: &str) -> String {
        self.identity.as_ref().map_or_else(
            || format!("{key}/{}", self.provider),
            |id| format!("{}#{id}", self.service.name()),
        )
    }
    pub fn title(&self) -> String {
        let base = self.service.title(self.account.is_some());
        self.label
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map_or_else(|| base.into(), |s| format!("{base} · {}", s.trim()))
    }
    pub fn identity_of(raw: &str) -> String {
        hex::encode(&Sha256::digest(format!("kuvo-code plan limits account v1\n{raw}"))[..8])
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub id: String,
    pub row: String,
    pub service: Provider,
    pub account_name: String,
    pub window_name: String,
    pub window: String,
    pub milestone: Milestone,
    pub at: u64,
    pub remaining: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
}
impl Fact {
    fn new(account: &Account, key: &str, window: &Window, milestone: Milestone) -> Self {
        let row = account.row(key);
        let hash = Sha256::digest(
            serde_json::to_vec(&(&row, &window.id, window.resets_at, milestone))
                .expect("serializable notice"),
        );
        Self {
            id: format!("usage_{}", hex::encode(hash)),
            row,
            service: account.service,
            account_name: account.title(),
            window_name: window.title(),
            window: window.id.clone(),
            milestone,
            at: window.read,
            remaining: 1.0 - window.used,
            resets_at: window.resets_at,
        }
    }
    fn current(&self, now: u64) -> bool {
        self.at <= now && now - self.at < 600_000 && self.resets_at.is_none_or(|a| a > now)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    account: Account,
    trails: BTreeMap<String, Trail>,
    #[serde(default)]
    notices: Vec<Fact>,
}
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    version: u32,
    entries: BTreeMap<String, Entry>,
}
type Tell = dyn Fn(Fact) + Send + Sync;
#[derive(Clone)]
pub struct Service {
    file: PathBuf,
    state: Arc<Mutex<Saved>>,
    cache_error: Option<Arc<str>>,
    settings: settings::Service,
    menu: MenuLimits,
    name: String,
    key: String,
    tell: Arc<std::sync::OnceLock<Arc<Tell>>>,
    refreshing: Arc<tokio::sync::Mutex<()>>,
    last_refresh: Arc<std::sync::atomic::AtomicU64>,
    refresh_failed: Arc<std::sync::atomic::AtomicBool>,
    refresh_completed: Arc<std::sync::atomic::AtomicU64>,
    changes: tokio::sync::watch::Sender<u64>,
}
impl Service {
    pub fn new(
        home: PathBuf,
        settings: settings::Service,
        menu: MenuLimits,
        name: String,
        key: String,
    ) -> Self {
        // Keep the additional acceptance evidence separately from the legacy cache, which
        // lacks corroboration and current-window pace state. A bad cache must never stop
        // the resident or be overwritten with an empty reading.
        let file = home.join("agents/usage/accepted-limits.json");
        let read = || -> Result<Saved, Box<dyn std::error::Error>> {
            let kept = match kuvo_store::files::read(&file)? {
                Some(bytes) => serde_json::from_slice::<Saved>(&bytes)?,
                None => Saved::default(),
            };
            if kept.version > 1 {
                return Err("Unsupported accepted plan limits format".into());
            }
            Ok(kept)
        };
        let (mut saved, cache_error) = match read() {
            Ok(saved) => (saved, None),
            Err(error) => {
                eprintln!("Could not read accepted plan limits: {error}");
                (Saved::default(), Some(Arc::<str>::from(error.to_string())))
            }
        };
        saved.version = 1;
        Self {
            file,
            state: Arc::new(Mutex::new(saved)),
            cache_error,
            settings,
            menu,
            name,
            key,
            tell: Arc::default(),
            refreshing: Arc::default(),
            last_refresh: Arc::default(),
            refresh_failed: Arc::default(),
            refresh_completed: Arc::default(),
            changes: tokio::sync::watch::channel(0).0,
        }
    }
    pub fn attach_notices(&self, tell: Arc<Tell>) {
        let _ = self.tell.set(tell);
    }
    pub fn observe(&self, account: Account, windows: Vec<Window>, now: u64) -> std::io::Result<()> {
        if let Some(error) = &self.cache_error {
            return Err(std::io::Error::other(format!(
                "Accepted limits cache: {error}"
            )));
        }
        let mut state = self.state.lock().expect("limits lock");
        let old = state.entries.get(&account.provider);
        let mut entry = old
            .filter(|e| {
                e.account.sign_in == account.sign_in
                    || e.account.identity.is_some() && e.account.identity == account.identity
            })
            .cloned()
            .unwrap_or_else(|| Entry {
                account: account.clone(),
                trails: BTreeMap::new(),
                notices: vec![],
            });
        entry.account = account;
        let mut facts = vec![];
        for window in windows {
            let toggles = self
                .settings
                .notification_milestones(entry.account.service.name(), &window);
            if let Some(trail) = entry.trails.get_mut(&window.id) {
                if let Some(fired) = trail.observe(window, &toggles) {
                    facts.extend(
                        fired
                            .into_iter()
                            .map(|m| Fact::new(&entry.account, &self.key, &trail.current, m)),
                    );
                }
            } else if let Some(trail) = Trail::new(window) {
                entry.trails.insert(trail.current.id.clone(), trail);
            }
        }
        entry.notices.retain(|fact| {
            fact.current(now)
                && entry
                    .trails
                    .get(&fact.window)
                    .is_some_and(|t| t.current.resets_at == fact.resets_at)
        });
        entry.notices.extend(facts.clone());
        let provider = entry.account.provider.clone();
        let previous = state.entries.insert(provider.clone(), entry);
        if let Err(err) = kuvo_store::files::write_secret(
            &self.file,
            &serde_json::to_vec(&*state).map_err(std::io::Error::other)?,
        ) {
            if let Some(previous) = previous {
                state.entries.insert(provider, previous);
            } else {
                state.entries.remove(&provider);
            }
            return Err(err);
        }
        self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
        drop(state);
        if let Some(tell) = self.tell.get() {
            for fact in facts {
                tell(fact);
            }
        }
        Ok(())
    }
    /// Only current sign-ins may expose kept readings after restart.
    pub fn confirm_accounts(&self, accounts: &[Account], now: u64) {
        let mut state = self.state.lock().expect("limits lock");
        state.entries.retain(|provider, entry| {
            accounts.iter().any(|account| {
                provider == &account.provider && account.identity == entry.account.identity
            })
        });
        for account in accounts {
            state
                .entries
                .entry(account.provider.clone())
                .or_insert_with(|| Entry {
                    account: account.clone(),
                    trails: BTreeMap::new(),
                    notices: vec![],
                })
                .account = account.clone();
        }
        self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn describe(&self) -> String {
        let now = now();
        let state = self.state.lock().expect("limits lock");
        if state.entries.is_empty() {
            return format!(
                "No plan limits are known on {}: no coding agent here is signed in to a plan whose limits can be read.",
                self.name
            );
        }
        let mut lines = vec![format!(
            "Plan limits of your accounts (signed in on {} unless it says where; used is how much of the window is gone):",
            self.name
        )];
        for entry in state.entries.values() {
            lines.push(format!(
                "- {}, provider {}:",
                entry.account.title(),
                entry.account.provider
            ));
            if entry.trails.is_empty() {
                lines.push("  - none read yet".into());
            }
            for trail in entry.trails.values() {
                let window = &trail.current;
                let title = window.title();
                if window.expired(now) {
                    lines.push(format!("  - {title}: started over, not read since"));
                    continue;
                }
                let resets = window
                    .resets_at
                    .and_then(|at| jiff::Timestamp::from_millisecond(at.try_into().ok()?).ok())
                    .map_or(String::new(), |at| format!(", resets {at}"));
                let minutes = now.saturating_sub(window.read) / 60_000;
                let since = if minutes == 0 {
                    "just now".into()
                } else if minutes < 60 {
                    format!("{minutes}m ago")
                } else {
                    format!("{}h ago", minutes / 60)
                };
                lines.push(format!(
                    "  - {title}: {}% used{resets}, as of {since}",
                    (window.used * 100.0).round()
                ));
            }
        }
        lines.join("\n")
    }
    pub fn answer(&self, now: u64) -> Value {
        let state = self.state.lock().expect("limits lock");
        self.publish(&state, now)
    }
    fn publish(&self, state: &Saved, now: u64) -> Value {
        let mut accounts = vec![];
        let mut rows = vec![];
        let mut rail = vec![];
        let mut notices = vec![];
        // The contract names the supported service families even when no account is
        // signed into one. Keep the TypeScript order; active rows remain independent.
        let services = [
            "claude",
            "codex",
            "cursor",
            "grok",
            "antigravity",
            "copilot",
            "openrouter",
            "zai",
            "opencode",
        ];
        let mut updated = 0;
        for entry in state.entries.values() {
            let account = &entry.account;
            let (local, row) = project(entry, &self.key, &self.name, now);
            if let Some(read) = entry.trails.values().map(|t| t.current.read).max() {
                updated = updated.max(read);
            }
            if !entry.trails.is_empty() {
                rail.push(json!({"key":account.service,"rows":[row["id"]],"ring":row["id"],"selected":row["id"],"title":row["title"]}));
            }
            accounts.push(local);
            rows.push(row);
            notices.extend(entry.notices.iter().filter(|n| n.current(now)).cloned());
        }
        self.menu.publish(&rows);
        let mut data = json!({"accounts":accounts,"rows":rows,"rail":rail,"services":services,"pendingComputers":[],"refreshPending":self.refreshing.try_lock().is_err(),"refreshFailed":self.cache_error.is_some()||self.refresh_failed.load(std::sync::atomic::Ordering::Relaxed),"notificationAvailability":settings::notification_availability(),"notices":notices,"ownNotices":notices});
        let completed = self
            .refresh_completed
            .load(std::sync::atomic::Ordering::Relaxed);
        if completed > 0 {
            data["refreshCompletedAt"] = json!(completed);
        }
        if updated > 0 {
            data["usageUpdatedAt"] = json!(updated);
        }
        json!({"data":data})
    }
}
fn level(used: f64) -> &'static str {
    if used >= 0.9 {
        "over"
    } else if used >= 0.7 {
        "near"
    } else {
        "calm"
    }
}
fn shown(window: &Window, now: u64) -> Value {
    let mut value = json!(window);
    value["title"] = json!(window.title());
    if window.expired(now) {
        value["used"] = json!(0);
        value["expired"] = json!(true);
        value
            .as_object_mut()
            .expect("window")
            .shift_remove("resetsAt");
    }
    value
}
fn project(entry: &Entry, key: &str, name: &str, now: u64) -> (Value, Value) {
    let acc = &entry.account;
    let title = acc.title();
    let mut local = json!({"provider":acc.provider,"service":acc.service,"title":title,"windows":entry.trails.values().map(|t|shown(&t.current,now)).collect::<Vec<_>>()});
    let mut box_record = json!({"key":key,"name":name,"provider":acc.provider,"here":true});
    if let Some(account) = &acc.account {
        local["account"] = json!(account);
        local["agent"] = json!(acc.service);
        box_record["account"] = json!(account);
        box_record["agent"] = json!(acc.service);
    }
    if let Some(label) = &acc.label {
        local["label"] = json!(label);
        box_record["label"] = json!(label);
    }
    if let Some(identity) = &acc.identity {
        local["identity"] = json!(identity);
    }
    let current: Vec<_> = entry.trails.values().map(|t| &t.current).collect();
    let whole: Vec<_> = current
        .iter()
        .copied()
        .filter(|w| w.model.is_none())
        .collect();
    let pool = if whole.is_empty() {
        current
            .iter()
            .copied()
            .filter(|w| w.model == current.first().and_then(|w| w.model.clone()))
            .collect()
    } else {
        whole
    };
    let plain: Vec<_> = pool.iter().copied().filter(|w| w.name.is_none()).collect();
    let choices = if plain.is_empty() { pool } else { plain };
    let short = choices
        .iter()
        .filter(|w| w.minutes.is_some_and(|m| m < 1440))
        .min_by_key(|w| w.minutes)
        .map(|w| &w.id);
    let long = choices
        .iter()
        .filter(|w| w.minutes.is_some_and(|m| m >= 1440))
        .max_by_key(|w| w.minutes)
        .map(|w| &w.id)
        .or_else(|| {
            short
                .is_none()
                .then(|| choices.first().map(|w| &w.id))
                .flatten()
        });
    let mut row = local.clone();
    let mut rings = Map::new();
    let mut meters = vec![];
    let mut lines = vec![];
    for (value, trail) in row["windows"]
        .as_array_mut()
        .expect("windows")
        .iter_mut()
        .zip(entry.trails.values())
    {
        let window = &trail.current;
        let used = if window.expired(now) {
            0.0
        } else {
            window.used
        };
        let l = level(used);
        value["level"] = json!(l);
        let span = if short == Some(&window.id) {
            Some("short")
        } else if long == Some(&window.id) {
            Some("long")
        } else {
            None
        };
        if let Some(span) = span {
            value["span"] = json!(span);
            let mut ring = json!({"window":window.id,"used":used,"level":l});
            if !window.expired(now)
                && let Some(reset) = window.resets_at
            {
                ring["resetsAt"] = json!(reset);
            }
            rings.insert(span.into(), ring);
        }
        let meter_title = if window.minutes == Some(300) && window.name.is_none() {
            "5-hour".into()
        } else {
            window.title()
        };
        let fresh = window
            .read
            .saturating_add(20 * 60_000)
            .min(window.resets_at.unwrap_or(u64::MAX))
            .saturating_sub(now);
        let mut meter = json!({"title":meter_title,"share":window.used,"value":format!("{}%",(window.used*100.0).round()),"level":if window.used>0.7 {level(window.used)}else{"calm"},"readAt":window.read,"freshForMs":fresh,"resetLabels":[]});
        if let Some(reset) = window.resets_at {
            meter["resetsAt"] = json!(reset);
        }
        meters.push(meter);
        let mut words =
            vec![json!({"text":format!("{}% left",((1.0-used)*100.0).round()),"tone":"muted"})];
        if window.expired(now) {
            words = vec![json!({"text":"Started over, not read since","tone":"muted"})];
        } else if let Some(reset) = window.resets_at {
            words.push(json!({"text":" · ","tone":"faint"}));
            words.push(json!({"text":"resets ","at":reset,"as":"time","tone":"muted"}));
        }
        lines.push(json!({"window":window.id,"title":meter_title,"left":1.0-used,"tone":if used>=0.995 {"danger"}else if used>=0.9 {"warning"}else{"calm"},"words":words}));
    }
    row["id"] = json!(acc.row(key));
    row["boxes"] = json!([box_record]);
    row["rings"] = json!(rings);
    if !rings.is_empty() {
        let bar = if rings.get("short").is_some_and(|s| {
            s["used"].as_f64().unwrap_or(0.0)
                >= rings
                    .get("long")
                    .and_then(|l| l["used"].as_f64())
                    .unwrap_or(0.0)
        }) {
            "short"
        } else {
            "long"
        };
        row["bar"] = json!(bar);
        row["level"] = rings[bar]["level"].clone();
    }
    let updated = current.iter().map(|w| w.read).max();
    let mut usage = json!({"title":title,"computers":[{"name":name,"here":true}],"meters":meters});
    if let Some(updated) = updated {
        usage["updatedAt"] = json!(updated);
    }
    let tightest = lines
        .iter()
        .filter_map(|l| l["left"].as_f64())
        .reduce(f64::min);
    let mut card = json!({"title":title,"name":acc.label.as_deref().unwrap_or(&title),"tone":if tightest.is_none() {"unknown"}else if tightest.is_some_and(|l|l<=0.005) {"danger"}else{"calm"},"lines":lines});
    if let Some(left) = tightest {
        card["left"] = json!(left);
    }
    if current.iter().any(|w| {
        matches!(w.source, super::readings::Source::Turn)
            && now.saturating_sub(w.read) <= 30 * 60_000
    }) {
        card["inUse"] = json!(true);
    }
    row["usage"] = usage;
    row["card"] = card;
    (local, row)
}
pub fn now() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}
struct Get;
impl RouteSpec for Get {
    type Body = ();
    const ROUTE: &'static Route = &Route {
        method: "GET",
        path: "kuvo/agents/api/kuvo/limits",
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: None,
    };
}
impl JsonStatus<200> for Get {
    type Body = Value;
}
struct Refresh;
impl RouteSpec for Refresh {
    type Body = Value;
    const ROUTE: &'static Route = &Route {
        method: "POST",
        path: "kuvo/agents/api/kuvo/limits/refresh",
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: Some(&Shape::Primitive(Primitive::Any)),
    };
}
impl JsonStatus<200> for Refresh {
    type Body = Value;
}
struct Events;
impl RouteSpec for Events {
    type Body = ();
    const ROUTE: &'static Route = &Route {
        method: "GET",
        path: "kuvo/agents/api/kuvo/limits/events",
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: None,
    };
}
impl JsonStatus<200> for Events {
    type Body = Value;
}
pub fn register_refresh<F, Fut>(routes: &mut Registry, service: Service, read: F)
where
    F: Fn(Option<String>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = bool> + Send + 'static,
{
    let read = Arc::new(read);
    let deadlines = service.clone();
    let deadline_reader = read.clone();
    tokio::spawn(async move {
        let mut changes = deadlines.changes.subscribe();
        let mut attempted = BTreeMap::<String, u64>::new();
        loop {
            let next = {
                let state = deadlines.state.lock().expect("limits lock");
                state
                    .entries
                    .values()
                    .flat_map(|entry| {
                        entry.trails.values().filter_map(|trail| {
                            trail
                                .check_at()
                                .map(|at| (entry.account.provider.clone(), at))
                        })
                    })
                    .filter(|(provider, at)| attempted.get(provider).is_none_or(|last| at > last))
                    .min_by_key(|(_, at)| *at)
            };
            if let Some((provider, at)) = next {
                tokio::select! {
                    result=changes.changed()=>{if result.is_err(){return;}},
                    _=tokio::time::sleep(std::time::Duration::from_millis(at.saturating_sub(now())))=>{
                        if start_read(&deadlines,deadline_reader.clone(),true,Some(provider.clone())) {
                            attempted.insert(provider,at);
                        } else if changes.changed().await.is_err(){return;}
                    }
                }
            } else if changes.changed().await.is_err() {
                return;
            }
        }
    });
    let reader = read.clone();
    let manual = service.clone();
    routes.register::<Refresh, _, _>(Access::Any, move |_| {
        let (service, read) = (manual.clone(), reader.clone());
        async move {
            start_read(&service, read, true, None);
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(2), service.refreshing.lock())
                    .await;
            let answer = tokio::task::spawn_blocking(move || service.answer(now()))
                .await
                .map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Ok(RouteReply::json::<200>(answer))
        }
    });
    routes.register::<Events,_,_>(Access::Any, move |call| {
        let (service,read)=(service.clone(),read.clone());
        async move {
            let mut changed=service.changes.subscribe();
            let mut heartbeat=tokio::time::interval(std::time::Duration::from_secs(15));
            let mut demand=tokio::time::interval(std::time::Duration::from_secs(600));
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            demand.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let stream=async_stream::try_stream!{
                yield kuvo_server::JsonLine::new(json!({"changed":true}))?;
                loop {
                    tokio::select!{
                        _=call.cancel.cancelled()=>break,
                        update=changed.changed()=>{
                            if update.is_err(){break;}
                            yield kuvo_server::JsonLine::new(json!({"changed":true}))?;
                        }
                        _=heartbeat.tick()=>yield kuvo_server::JsonLine::new(json!({"heartbeat":true}))?,
                        _=demand.tick()=>{start_read(&service,read.clone(),false,None);},
                    }
                }
            };
            Ok(RouteReply::ndjson::<200>(stream))
        }
    });
}
fn start_read<F, Fut>(
    service: &Service,
    read: Arc<F>,
    manual: bool,
    provider: Option<String>,
) -> bool
where
    F: Fn(Option<String>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = bool> + Send + 'static,
{
    use std::sync::atomic::Ordering::Relaxed;
    let at = now();
    if !manual && at.saturating_sub(service.last_refresh.load(Relaxed)) < 600_000 {
        return false;
    }
    if let Ok(guard) = service.refreshing.clone().try_lock_owned() {
        service.last_refresh.store(at, Relaxed);
        let service = service.clone();
        tokio::spawn(async move {
            let failed = read(provider).await;
            service.refresh_failed.store(failed, Relaxed);
            if manual {
                service.refresh_completed.store(now(), Relaxed);
            }
            drop(guard);
            let _ = tokio::task::spawn_blocking({
                let service = service.clone();
                move || service.answer(now())
            })
            .await;
            service.changes.send_modify(|n| *n = n.wrapping_add(1));
        });
        true
    } else {
        false
    }
}

pub fn register(routes: &mut Registry, service: Service) {
    routes.register::<Get, _, _>(Access::Any, move |_| {
        let service = service.clone();
        async move {
            tokio::task::spawn_blocking(move || RouteReply::json::<200>(service.answer(now())))
                .await
                .map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
    });
}
