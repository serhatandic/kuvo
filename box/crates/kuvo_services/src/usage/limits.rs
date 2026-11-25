//! Resident-local accepted plan limits and their client projections.
use super::{
    limits_peers::{AWAY, AccountMail, Book, Reading},
    readings::{Milestone, Trail, Window, equivalent_trail},
    settings,
};
#[path = "limits_view.rs"]
mod view;
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
    collections::{BTreeMap, BTreeSet},
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarder: Option<String>,
}
impl Fact {
    fn new(
        account: &Account,
        key: &str,
        window: &Window,
        milestone: Milestone,
        count: u32,
    ) -> Self {
        let row = account.row(key);
        let hash = Sha256::digest(
            serde_json::to_vec(&(
                &row,
                window.limit_key(),
                window
                    .resets_at
                    .map(|at| at.saturating_add(500) / 1000)
                    .unwrap_or(0),
                milestone,
                count,
            ))
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
            forwarder: None,
        }
    }
    fn current(&self, now: u64) -> bool {
        self.at <= now && now - self.at < 600_000 && self.resets_at.is_none_or(|a| a > now)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    account: Account,
    #[serde(default)]
    facts: super::plan_facts::Facts,
    trails: BTreeMap<String, Trail>,
    #[serde(default)]
    partial_missing: BTreeSet<String>,
    #[serde(default)]
    window_order: Vec<String>,
    #[serde(default)]
    notices: Vec<Fact>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    version: u32,
    entries: BTreeMap<String, Entry>,
    #[serde(default)]
    order: Vec<String>,
}
impl Saved {
    fn ordered(&self) -> Vec<&Entry> {
        let mut seen = std::collections::BTreeSet::new();
        self.order
            .iter()
            .chain(self.entries.keys())
            .filter(|provider| seen.insert(*provider))
            .filter_map(|provider| self.entries.get(provider))
            .collect()
    }
}
fn same_account(a: &Account, b: &Account) -> bool {
    a.service == b.service
        && a.identity
            .as_ref()
            .is_some_and(|id| b.identity.as_ref() == Some(id))
}
fn same_sign_in(a: &Account, b: &Account) -> bool {
    if a.identity.is_some() && b.identity.is_some() {
        same_account(a, b)
    } else {
        a.provider == b.provider && a.sign_in == b.sign_in
    }
}
type Tell = dyn Fn(Fact) + Send + Sync;
#[derive(Clone)]
pub struct Service {
    file: PathBuf,
    state: Arc<Mutex<Saved>>,
    cache_error: Option<Arc<str>>,
    legacy: Arc<Mutex<super::legacy_limits::Cache>>,
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
    reset_permission: Arc<Mutex<BTreeMap<String, bool>>>,
    peer_file: PathBuf,
    peer_book: Arc<Mutex<Book>>,
    remote_refresh: tokio::sync::broadcast::Sender<()>,
    local_refresh: tokio::sync::broadcast::Sender<Vec<String>>,
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
        let legacy_cache = if file.exists() || cache_error.is_some() {
            super::legacy_limits::Cache::default()
        } else {
            super::legacy_limits::Cache::load(&home)
        };
        let peer_file = home.join("agents/usage/accepted-limits-peers.json");
        let legacy = home.join("agents/usage/limits-peers.json");
        let peer_book = Book::read(if peer_file.exists() {
            &peer_file
        } else {
            &legacy
        })
        .unwrap_or_default();
        Self {
            file,
            state: Arc::new(Mutex::new(saved)),
            cache_error,
            legacy: Arc::new(Mutex::new(legacy_cache)),
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
            reset_permission: Arc::default(),
            peer_file,
            peer_book: Arc::new(Mutex::new(peer_book)),
            remote_refresh: tokio::sync::broadcast::channel(8).0,
            local_refresh: tokio::sync::broadcast::channel(8).0,
        }
    }
    pub fn peer_snapshot(&self, scope: Option<String>, now: u64) -> Reading {
        let state = self.state.lock().expect("limits lock");
        let allowed = self.reset_permission.lock().expect("reset permissions");
        let book = self.peer_book.lock().expect("peer limits");
        let accounts = state
            .ordered()
            .into_iter()
            .filter(|entry| {
                !entry
                    .facts
                    .problem
                    .as_ref()
                    .is_some_and(|p| p.kind == super::plan_facts::ProblemKind::None)
            })
            .map(|entry| {
                let acc = &entry.account;
                AccountMail {
                    provider: acc.provider.clone(),
                    service: acc.service,
                    account: acc.account.clone(),
                    agent: acc.account.as_ref().map(|_| acc.service),
                    identity: acc.identity.clone(),
                    label: acc.label.clone(),
                    facts: entry.facts.clone(),
                    windows: ordered_trails(entry)
                        .into_iter()
                        .map(|trail| trail.current.clone())
                        .collect(),
                    notices: if scope.is_some() {
                        entry
                            .notices
                            .iter()
                            .filter(|fact| fact.current(now))
                            .cloned()
                            .collect()
                    } else {
                        vec![]
                    },
                    notice_state: if scope.is_some() {
                        let shared = book.groups.get(&acc.row(&self.key));
                        entry
                            .trails
                            .iter()
                            .map(|(id, trail)| {
                                let mut trail = trail.clone();
                                if let Some(other) =
                                    shared.and_then(|group| group.get(&trail.current.limit_key()))
                                {
                                    trail.merge_fired(other);
                                }
                                (id.clone(), trail.notice_state())
                            })
                            .collect()
                    } else {
                        BTreeMap::new()
                    },
                    agents_reset: allowed.get(&acc.row(&self.key)) == Some(&true),
                }
            })
            .collect();
        Reading {
            at: now,
            accounts,
            ask: false,
            refresh: vec![],
            notice_setup: scope,
        }
    }
    pub fn peer_roster(&self, names: &BTreeMap<String, String>, scope: Option<&str>, now: u64) {
        let changed = {
            let mut book = self.peer_book.lock().expect("peer limits");
            if !book.roster(names, scope) {
                false
            } else {
                self.save_peers(&book);
                true
            }
        };
        if changed {
            self.answer(now);
            self.changes.send_modify(|n| *n = n.wrapping_add(1));
        }
    }
    /// The next accepted peer reading to become away. No peer means no deadline.
    pub fn peer_expiry(&self, now: u64) -> Option<u64> {
        let book = self.peer_book.lock().expect("peer limits");
        book.peers
            .iter()
            .filter(|(key, _)| book.roster.contains_key(*key))
            .map(|(_, peer)| peer.heard.saturating_add(AWAY).saturating_add(1))
            .filter(|at| *at > now)
            .min()
    }
    pub fn take_peer(&self, key: &str, name: &str, reading: Reading, now: u64) {
        let refresh = reading.refresh.clone();
        let notices = {
            let mut book = self.peer_book.lock().expect("peer limits");
            if !book.roster.contains_key(key) {
                return;
            }
            let notes = book.take(key, name, reading, now);
            self.save_peers(&book);
            notes
        };
        let accepted = self.answer(now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
        if !refresh.is_empty() {
            let _ = self.local_refresh.send(refresh);
        }
        if let Some(tell) = self.tell.get() {
            for fact in notices.into_iter().filter(|fact| {
                fact.forwarder.as_deref() == Some(self.key.as_str())
                    && accepted["data"]["notices"]
                        .as_array()
                        .is_some_and(|facts| facts.iter().any(|kept| kept["id"] == fact.id))
            }) {
                tell(fact);
            }
        }
    }
    pub fn remote_refreshes(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.remote_refresh.subscribe()
    }
    pub fn peer_refresh_targets(&self, now: u64) -> BTreeMap<String, Vec<String>> {
        let answer = self.answer(now);
        let mut targets = BTreeMap::<String, Vec<String>>::new();
        for row in answer["data"]["rows"].as_array().into_iter().flatten() {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            let Some(boxes) = row["boxes"].as_array() else {
                continue;
            };
            if boxes.iter().any(|b| b["here"] == true) {
                continue;
            }
            if let Some(peer) = boxes
                .iter()
                .find(|b| b["away"] != true)
                .or_else(|| boxes.first())
                .and_then(|b| b["key"].as_str())
            {
                let rows = targets.entry(peer.into()).or_default();
                if rows.len() < 200 {
                    rows.push(id.into());
                }
            }
        }
        targets
    }
    fn save_peers(&self, book: &Book) {
        if let Err(error) = book.store(&self.peer_file) {
            eprintln!("Couldn't keep peer limits: {error}");
        }
    }
    /// Local history may supply a fresh quota snapshot without waking Codex.
    /// Reconfirm the provider sign-in separately; a new credential's log cannot
    /// populate the account that was removed or replaced.
    pub fn observe_logged(&self, provider: &str, windows: Vec<Window>, now: u64) {
        let account = self.state.lock().ok().and_then(|state| {
            state
                .entries
                .get(provider)
                .map(|entry| entry.account.clone())
        });
        let Some(account) = account else {
            return;
        };
        for window in windows {
            let fresh = self.state.lock().is_ok_and(|state| {
                state.entries.get(provider).is_some_and(|entry| {
                    same_sign_in(&account, &entry.account)
                        && entry
                            .trails
                            .get(&window.id)
                            .is_none_or(|old| window.read > old.current.read)
                })
            });
            if fresh {
                if let Err(error) =
                    self.observe_facts_inner(account.clone(), vec![window], None, now, true, false)
                {
                    eprintln!("Couldn't keep logged plan limits: {error}");
                }
            }
        }
    }
    pub fn attach_notices(&self, tell: Arc<Tell>) {
        let _ = self.tell.set(tell);
    }
    pub fn observe(&self, account: Account, windows: Vec<Window>, now: u64) -> std::io::Result<()> {
        self.observe_facts(account, windows, None, now)
    }
    pub fn observe_facts(
        &self,
        account: Account,
        windows: Vec<Window>,
        facts: Option<super::plan_facts::Facts>,
        now: u64,
    ) -> std::io::Result<()> {
        self.observe_facts_inner(account, windows, facts, now, false, false)
    }
    /// A reader admitted the account before asking its maker. If a sign-out
    /// was published while it waited, that answer can no longer resurrect it.
    pub fn observe_confirmed_facts(
        &self,
        account: Account,
        windows: Vec<Window>,
        facts: super::plan_facts::Facts,
        now: u64,
    ) -> std::io::Result<()> {
        self.observe_facts_inner(account, windows, Some(facts), now, true, false)
    }
    pub fn observe_reset_facts(
        &self,
        account: Account,
        facts: super::plan_facts::Facts,
        now: u64,
    ) -> std::io::Result<()> {
        self.observe_facts_inner(account, vec![], Some(facts), now, true, true)
    }
    fn observe_facts_inner(
        &self,
        account: Account,
        windows: Vec<Window>,
        facts: Option<super::plan_facts::Facts>,
        now: u64,
        require_current: bool,
        only_resets: bool,
    ) -> std::io::Result<()> {
        use super::plan_facts::ProblemKind;
        if let Some(error) = &self.cache_error {
            return Err(std::io::Error::other(format!(
                "Accepted limits cache: {error}"
            )));
        }
        let mut state = self.state.lock().expect("limits lock");
        if require_current
            && !state
                .entries
                .get(&account.provider)
                .is_some_and(|e| same_sign_in(&account, &e.account))
        {
            return Ok(());
        }
        let before = state.clone();
        let old = state
            .entries
            .get(&account.provider)
            .filter(|entry| same_sign_in(&entry.account, &account));
        let sibling = state
            .entries
            .values()
            .filter(|entry| same_account(&entry.account, &account))
            .max_by_key(|entry| entry.trails.values().map(|t| t.current.read).max());
        let mut entry = old.or(sibling).cloned().unwrap_or_else(|| Entry {
            account: account.clone(),
            facts: Default::default(),
            trails: BTreeMap::new(),
            partial_missing: BTreeSet::new(),
            window_order: vec![],
            notices: vec![],
        });
        if entry.trails.is_empty() && entry.facts.checked.is_none() {
            if let Some((facts, trails, order)) = self
                .legacy
                .lock()
                .expect("legacy plan readings")
                .import(&account, now)
            {
                entry.facts = facts;
                entry.trails = trails;
                entry.window_order = order;
            }
        }
        // All local sign-ins to the same maker account keep the same accepted window.
        // A turn reading of one must not create a second trail or second warning for it.
        if let Some(other) = sibling {
            for trail in other.trails.values() {
                if let Some(old_id) = equivalent_trail(&entry.trails, &trail.current) {
                    let mut current = entry.trails.remove(&old_id).expect("known trail");
                    if current
                        .observe(trail.current.clone(), &Default::default())
                        .is_some()
                    {
                        current.merge_fired(trail);
                    }
                    entry.trails.insert(current.current.id.clone(), current);
                } else {
                    entry.trails.insert(trail.current.id.clone(), trail.clone());
                }
            }
            for fact in &other.notices {
                if !entry.notices.iter().any(|n| n.id == fact.id) {
                    entry.notices.push(fact.clone());
                }
            }
        }
        entry.account = account;
        let mut window_order = if facts
            .as_ref()
            .is_some_and(|facts| !only_resets && facts.problem.is_none())
        {
            windows.iter().map(|w| w.id.clone()).collect::<Vec<_>>()
        } else {
            entry.window_order.clone()
        };
        for id in entry
            .window_order
            .iter()
            .chain(windows.iter().map(|w| &w.id))
        {
            if !window_order.contains(id) {
                window_order.push(id.clone());
            }
        }
        if let Some(facts) = facts {
            if only_resets {
                entry.facts.extras.retain(|extra| match extra {
                    super::plan_facts::Extra::Resets { .. } => false,
                    super::plan_facts::Extra::Unreadable { id, .. } if id == "resets" => false,
                    _ => true,
                });
                entry.facts.extras.extend(facts.extras);
                if entry.facts.checked.is_none() {
                    entry.facts.checked = facts.checked;
                    entry.facts.next = facts.next;
                }
            } else if let Some(problem) = facts.problem.as_ref() {
                if matches!(problem.kind, ProblemKind::Ended | ProblemKind::None) {
                    entry.trails.clear();
                    entry.partial_missing.clear();
                    entry.notices.clear();
                    entry.facts = facts;
                } else {
                    entry.facts.problem = facts.problem;
                    entry.facts.checked = facts.checked;
                    entry.facts.next = facts.next;
                }
            } else {
                // A successful reading replaces its old maker windows. A turn-only
                // window stays, and a partial read keeps missing future windows (or
                // a reset-less one for just one read, even across restart).
                let present = windows
                    .iter()
                    .map(Window::limit_key)
                    .collect::<BTreeSet<_>>();
                let missing = entry
                    .trails
                    .iter()
                    .filter(|(_, trail)| !present.contains(&trail.current.limit_key()))
                    .map(|(id, _)| id.clone())
                    .collect::<BTreeSet<_>>();
                let before = &entry.partial_missing;
                entry.trails.retain(|id, trail| {
                    present.contains(&trail.current.limit_key())
                        || trail.current.source == super::readings::Source::Turn
                        || facts.partial
                            && trail
                                .current
                                .resets_at
                                .map_or(!before.contains(id), |at| at > now)
                });
                entry.partial_missing = if facts.partial {
                    missing
                } else {
                    BTreeSet::new()
                };
                let mut facts = facts;
                if facts.plan.is_none() {
                    facts.plan = entry.facts.plan.clone();
                }
                entry.facts = facts;
            }
        }

        let mut facts = vec![];
        let row_id = entry.account.row(&self.key);
        let mut book = self.peer_book.lock().expect("peer limits");
        for window in windows {
            let toggles = self
                .settings
                .notification_milestones(entry.account.service.name(), &window);
            let semantic = window.limit_key();
            let shared_trail = book
                .groups
                .get(&row_id)
                .and_then(|group| group.get(&semantic))
                .cloned();
            let local_toggles = if shared_trail.is_none() {
                toggles.clone()
            } else {
                BTreeSet::new()
            };
            let prior_id = equivalent_trail(&entry.trails, &window);
            let mut local = prior_id.as_ref().and_then(|id| entry.trails.remove(id));
            if let Some(trail) = local.as_mut() {
                if let Some(fired) = trail.observe(window.clone(), &local_toggles) {
                    facts.extend(fired.into_iter().map(|m| {
                        Fact::new(
                            &entry.account,
                            &self.key,
                            &trail.current,
                            m,
                            trail.occurrence(m),
                        )
                    }));
                    if let Some(old_id) = prior_id
                        .as_ref()
                        .filter(|old_id| **old_id != trail.current.id)
                    {
                        for fact in &mut entry.notices {
                            if fact.window == *old_id && fact.resets_at == trail.current.resets_at {
                                fact.window = trail.current.id.clone();
                            }
                        }
                    }
                }
            } else {
                local = Trail::new(window.clone());
            }
            if let Some(mut shared) = shared_trail {
                if let Some(local) = local.as_ref() {
                    shared.merge_fired(local);
                }
                if let Some(fired) = shared.observe(window, &toggles) {
                    for m in fired {
                        if let Some(local) = local.as_mut() {
                            local.merge_fired(&shared);
                        }
                        facts.push(Fact::new(
                            &entry.account,
                            &self.key,
                            &shared.current,
                            m,
                            shared.occurrence(m),
                        ));
                    }
                }
                if let Some(group) = book.groups.get_mut(&row_id) {
                    group.insert(semantic, shared);
                }
            }
            if let Some(trail) = local {
                entry.trails.insert(trail.current.id.clone(), trail);
            }
        }
        // Keep the chosen forwarder pinned to the fact. A peer merely relaying it cannot
        // create another push or tell it again.
        for fact in &mut facts {
            let peer = book
                .peers
                .iter()
                .filter(|(id, peer)| {
                    book.roster.contains_key(*id)
                        && peer.setup == book.scope
                        && book.scope.is_some()
                        && now.saturating_sub(peer.heard) <= AWAY
                        && peer
                            .accounts
                            .iter()
                            .any(|a| same_account(&entry.account, &a.account()))
                })
                .map(|(id, _)| id.as_str())
                .min();
            fact.forwarder = Some(
                peer.map_or(self.key.as_str(), |id| id.min(&self.key))
                    .into(),
            );
        }
        drop(book);
        entry.notices.retain(|fact| {
            fact.current(now)
                && entry
                    .trails
                    .get(&fact.window)
                    .is_some_and(|t| t.current.resets_at == fact.resets_at)
        });
        entry.notices.extend(facts.clone());
        window_order.retain(|id| entry.trails.contains_key(id));
        entry.window_order = window_order;
        let provider = entry.account.provider.clone();
        for sibling in state
            .entries
            .values_mut()
            .filter(|s| same_account(&s.account, &entry.account))
        {
            sibling.trails = entry.trails.clone();
            sibling.partial_missing = entry.partial_missing.clone();
            sibling.window_order = entry.window_order.clone();
            sibling.notices = entry.notices.clone();
        }
        if !state.order.contains(&provider) {
            state.order.push(provider.clone());
        }
        state.entries.insert(provider, entry);
        if let Err(err) = kuvo_store::files::write_secret(
            &self.file,
            &serde_json::to_vec(&*state).map_err(std::io::Error::other)?,
        ) {
            *state = before;
            return Err(err);
        }
        {
            let book = self.peer_book.lock().expect("peer limits");
            if book.groups.contains_key(&row_id) {
                self.save_peers(&book);
            }
        }
        self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
        drop(state);
        if let Some(tell) = self.tell.get() {
            for fact in facts
                .into_iter()
                .filter(|fact| fact.forwarder.as_deref().is_none_or(|id| id == self.key))
            {
                tell(fact);
            }
        }
        Ok(())
    }
    /// Only current sign-ins may expose kept readings after restart.
    pub fn confirm_accounts(&self, accounts: &[Account], now: u64) {
        self.confirm_group(accounts, &Default::default(), false, now)
    }
    pub fn confirm_standalone_accounts(
        &self,
        accounts: &[Account],
        unreadable: &std::collections::BTreeSet<String>,
        now: u64,
    ) {
        self.confirm_group(accounts, unreadable, true, now)
    }
    fn confirm_group(
        &self,
        accounts: &[Account],
        unreadable: &std::collections::BTreeSet<String>,
        standalone: bool,
        now: u64,
    ) {
        let belongs = |account: &Account| {
            account.account.is_some()
                && (matches!(
                    account.service,
                    Provider::Cursor | Provider::Antigravity | Provider::Grok
                ) == standalone)
        };
        let mut state = self.state.lock().expect("limits lock");
        let before = state
            .entries
            .iter()
            .map(|(id, e)| (id.clone(), e.account.clone()))
            .collect::<BTreeMap<_, _>>();
        let old_order = state.order.clone();
        state.entries.retain(|provider, entry| {
            !belongs(&entry.account)
                || unreadable.contains(provider)
                || accounts
                    .iter()
                    .any(|a| provider == &a.provider && same_sign_in(a, &entry.account))
        });
        let mut imported = false;
        for account in accounts {
            let entry = state
                .entries
                .entry(account.provider.clone())
                .or_insert_with(|| Entry {
                    account: account.clone(),
                    facts: Default::default(),
                    trails: BTreeMap::new(),
                    partial_missing: BTreeSet::new(),
                    window_order: vec![],
                    notices: vec![],
                });
            if entry.trails.is_empty() && entry.facts.checked.is_none() {
                if let Some((facts, trails, order)) = self
                    .legacy
                    .lock()
                    .expect("legacy plan readings")
                    .import(account, now)
                {
                    entry.facts = facts;
                    entry.trails = trails;
                    entry.window_order = order;
                    imported = true;
                }
            }
            // Cursor's stable maker identity is learned during the token exchange.
            // The same unchanged key keeps that identity between demand reads.
            let mut a = account.clone();
            if a.identity.is_none() && a.sign_in == entry.account.sign_in {
                a.identity = entry.account.identity.clone();
            }
            entry.account = a;
        }
        let mut order = state
            .order
            .iter()
            .filter(|id| state.entries.get(*id).is_some())
            .cloned()
            .collect::<Vec<_>>();
        for account in accounts {
            if !order.contains(&account.provider) {
                order.push(account.provider.clone());
            }
        }
        // All default accounts precede added accounts, and the engine's follow both.
        // Keep the existing order within each group; a read-only check must not
        // reorder rows or notify clients just because the groups finished in another order.
        order.sort_by_key(|id| match state.entries.get(id).map(|e| &e.account) {
            Some(a) if a.account.as_deref() == Some("default") => match a.service {
                Provider::Claude => 0,
                Provider::Codex => 1,
                Provider::Cursor => 2,
                Provider::Grok => 3,
                Provider::Antigravity => 4,
                _ => 5,
            },
            Some(a) if a.account.is_some() => 6,
            _ => 7,
        });
        state.order = order;
        let changed = before.len() != state.entries.len()
            || state
                .entries
                .iter()
                .any(|(id, e)| before.get(id) != Some(&e.account))
            || old_order != state.order;
        if !changed && !imported {
            return;
        }
        // Persist imports, removals and changed sign-ins, even when the account count stays equal.
        let removed = before.iter().any(|(id, a)| {
            state
                .entries
                .get(id)
                .is_none_or(|e| !same_sign_in(a, &e.account))
        });
        if (imported || removed && self.file.exists()) && self.cache_error.is_none() {
            if let Ok(bytes) = serde_json::to_vec(&*state) {
                if let Err(error) = kuvo_store::files::write_secret(&self.file, &bytes) {
                    eprintln!("Couldn't keep the current plan sign-ins: {error}");
                }
            }
        }
        self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Withdraw only rows whose current credential no longer belongs to them.
    /// A new engine credential isn't a plan reading until its maker answers.
    pub fn has_engine_accounts(&self) -> bool {
        self.state
            .lock()
            .expect("limits lock")
            .entries
            .values()
            .any(|e| e.account.account.is_none())
    }
    pub fn confirm_engine_sign_ins(&self, accounts: &[Account], now: u64) {
        let mut state = self.state.lock().expect("limits lock");
        let before = state.entries.len();
        state.entries.retain(|provider, entry| {
            entry.account.account.is_some()
                || accounts
                    .iter()
                    .any(|a| provider == &a.provider && same_sign_in(a, &entry.account))
        });
        if state.entries.len() == before {
            return;
        }
        let keep = state
            .entries
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        state.order.retain(|id| keep.contains(id));
        if self.file.exists() && self.cache_error.is_none() {
            if let Ok(bytes) = serde_json::to_vec(&*state) {
                if let Err(error) = kuvo_store::files::write_secret(&self.file, &bytes) {
                    eprintln!("Couldn't keep the current engine plan sign-ins: {error}");
                }
            }
        }
        self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn confirm_engine_accounts(&self, providers: &[String], now: u64) {
        let mut state = self.state.lock().expect("limits lock");
        let before = state.entries.len();
        state.entries.retain(|provider, entry| {
            entry.account.account.is_some() || providers.contains(provider)
        });
        let mut order = state
            .order
            .iter()
            .filter(|p| {
                state
                    .entries
                    .get(*p)
                    .is_some_and(|e| e.account.account.is_some())
            })
            .cloned()
            .collect::<Vec<_>>();
        order.extend_from_slice(providers);
        state.order = order;
        if state.entries.len() < before && self.file.exists() && self.cache_error.is_none() {
            if let Ok(bytes) = serde_json::to_vec(&*state) {
                let _ = kuvo_store::files::write_secret(&self.file, &bytes);
            }
        }
        self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }
    pub fn held_engine_providers(&self, now: u64) -> std::collections::BTreeSet<String> {
        let state = self.state.lock().expect("limits lock");
        state
            .entries
            .values()
            .filter(|entry| {
                entry.account.account.is_none()
                    && entry
                        .facts
                        .problem
                        .as_ref()
                        .is_some_and(|p| p.kind == super::plan_facts::ProblemKind::RateLimited)
                    && entry.facts.next.is_some_and(|n| n > now)
            })
            .map(|e| e.account.provider.clone())
            .collect()
    }

    pub fn reset_permissions(&self, allowed: BTreeMap<String, bool>) {
        *self.reset_permission.lock().expect("reset permissions") = allowed;
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }
    pub fn reset_sign_ins(&self) -> Vec<(Account, String)> {
        let state = self.state.lock().expect("limits lock");
        state
            .ordered()
            .into_iter()
            .filter(|e| {
                e.account.service == Provider::Codex
                    || e.account.service == Provider::Claude && e.account.account.is_some()
            })
            .map(|e| (e.account.clone(), e.account.row(&self.key)))
            .collect()
    }
    pub fn here(&self) -> Value {
        json!({"key":self.key,"name":self.name,"here":true})
    }
    pub fn row(&self, id: &str) -> Option<Value> {
        self.answer(now())["data"]["rows"]
            .as_array()?
            .iter()
            .find(|r| r["id"] == id)
            .cloned()
    }
    /// The resident's eligible sign-in for a reset; an optional provider pins a previous offer.
    pub fn reset_account(
        &self,
        row: Option<&str>,
        provider: Option<&str>,
    ) -> Option<(Account, String)> {
        let state = self.state.lock().expect("limits lock");
        let mut candidates = state
            .ordered()
            .into_iter()
            .filter(|entry| {
                entry.account.service == Provider::Codex
                    || entry.account.service == Provider::Claude && entry.account.account.is_some()
            })
            .filter(|entry| provider.is_none_or(|p| p == entry.account.provider))
            .filter_map(|entry| {
                let id = entry.account.row(&self.key);
                row.is_none_or(|row| row == id).then_some((entry, id))
            })
            .collect::<Vec<_>>();
        if provider.is_none() {
            candidates.sort_by_key(|(entry, _)| {
                !entry.facts.extras.iter().any(|extra| {
                    matches!(extra, super::plan_facts::Extra::Resets { usable: true, .. })
                })
            });
        }
        candidates
            .first()
            .map(|(entry, id)| (entry.account.clone(), id.clone()))
    }
    pub fn describe(&self, zone: Option<&str>) -> String {
        let now = now();
        let answer = self.answer(now);
        super::plan_tool_words::describe(&self.name, &answer["data"]["rows"], now, zone)
    }
    pub fn answer(&self, now: u64) -> Value {
        let state = self.state.lock().expect("limits lock");
        self.publish(&state, now)
    }
    fn publish(&self, state: &Saved, now: u64) -> Value {
        let mut accounts = vec![];
        let mut rows: Vec<Value> = vec![];
        let mut notices: Vec<Fact> = vec![];
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
        struct Group {
            entry: Entry,
            boxes: Vec<Value>,
            candidates: BTreeMap<String, Vec<Trail>>,
            order: Vec<String>,
            reset_members: Vec<(Value, super::plan_facts::Extra, Option<u64>, bool)>,
            members: Vec<view::Member>,
        }
        let mut groups = Vec::<Group>::new();
        let mut group_ids = BTreeMap::<String, usize>::new();
        let mut book = self.peer_book.lock().expect("peer limits");
        let mut add = |entry: &Entry,
                       key: &str,
                       name: &str,
                       here: bool,
                       away: bool,
                       away_at: Option<u64>,
                       agents_reset: bool| {
            if entry
                .facts
                .problem
                .as_ref()
                .is_some_and(|p| p.kind == super::plan_facts::ProblemKind::None)
            {
                return;
            }
            let (local, row) = project(entry, key, name, now);
            if here {
                accounts.push(local);
            }
            let id = entry.account.row(key);
            let mut box_record = row["boxes"][0].clone();
            if !here {
                box_record
                    .as_object_mut()
                    .expect("box")
                    .shift_remove("here");
            }
            if away {
                box_record["away"] = json!(true);
            }
            if let Some(checked) = entry.facts.checked {
                box_record["checked"] = json!(checked);
            }
            let group = if let Some(index) = group_ids.get(&id) {
                &mut groups[*index]
            } else {
                group_ids.insert(id.clone(), groups.len());
                groups.push(Group {
                    entry: entry.clone(),
                    boxes: vec![],
                    candidates: BTreeMap::new(),
                    order: vec![],
                    reset_members: vec![],
                    members: vec![],
                });
                groups.last_mut().expect("group")
            };
            for extra in &entry.facts.extras {
                if matches!(extra, super::plan_facts::Extra::Resets { .. }) {
                    group.reset_members.push((
                        box_record.clone(),
                        extra.clone(),
                        entry.facts.checked,
                        agents_reset,
                    ));
                }
            }
            group.boxes.push(box_record);
            group.members.push(view::Member {
                entry: entry.clone(),
                name: name.to_owned(),
                here,
                away,
                away_at,
            });
            let label = group
                .entry
                .account
                .label
                .clone()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| entry.account.label.clone().filter(|s| !s.trim().is_empty()));
            if group.entry.account.account.is_none() && entry.account.account.is_some() {
                group.entry.account = entry.account.clone();
            }
            group.entry.account.label = label;
            for trail in ordered_trails(entry) {
                let key = trail.current.limit_key();
                if !group.order.contains(&key) {
                    group.order.push(key.clone());
                }
                updated = updated.max(trail.current.read);
                group.candidates.entry(key).or_default().push(trail.clone());
            }
            for fact in entry.notices.iter().filter(|n| n.current(now)) {
                if !notices.iter().any(|n| n.id == fact.id) {
                    notices.push(fact.clone());
                }
            }
        };
        let allowed = self
            .reset_permission
            .lock()
            .expect("reset permissions")
            .clone();
        for entry in state.ordered() {
            add(
                entry,
                &self.key,
                &self.name,
                true,
                false,
                None,
                allowed.get(&entry.account.row(&self.key)) == Some(&true),
            );
        }
        let mut peers = book
            .peers
            .iter()
            .filter(|(key, _)| book.roster.contains_key(*key))
            .collect::<Vec<_>>();
        peers.sort_by(|(a, x), (b, y)| x.name.cmp(&y.name).then(a.cmp(b)));
        for (key, peer) in peers {
            for account in &peer.accounts {
                let trails = peer
                    .trails
                    .get(&account.row(key))
                    .cloned()
                    .unwrap_or_else(|| {
                        account
                            .windows
                            .iter()
                            .filter_map(|w| Trail::new(w.clone()).map(|t| (w.id.clone(), t)))
                            .collect()
                    });
                let entry = Entry {
                    account: account.account(),
                    facts: account.facts.clone(),
                    trails,
                    partial_missing: BTreeSet::new(),
                    window_order: account.windows.iter().map(|w| w.id.clone()).collect(),
                    notices: account.notices.clone(),
                };
                add(
                    &entry,
                    key,
                    &peer.name,
                    false,
                    now.saturating_sub(peer.heard) > AWAY,
                    Some(peer.heard.saturating_add(AWAY)),
                    account.agents_reset,
                );
            }
        }
        drop(add);
        // Reconcile in observation order against the already accepted account window,
        // not by whichever computer's checked timestamp happened to be newest.
        let before = serde_json::to_vec(&book.groups).ok();
        let paired = !book.roster.is_empty();
        book.groups.retain(|id, _| group_ids.contains_key(id));
        for (id, index) in group_ids {
            let group = &mut groups[index];
            let had = book.groups.contains_key(&id);
            if paired
                || group.boxes.len() > 1
                || had
                || group.candidates.values().any(|c| c.len() > 1)
            {
                let agreed = book.groups.entry(id.clone()).or_default();
                agreed.retain(|id, _| group.candidates.contains_key(id));
                for (id, candidates) in &mut group.candidates {
                    candidates.sort_by_key(|t| t.current.read);
                    for candidate in candidates {
                        if let Some(current) = agreed.get_mut(id) {
                            current.observe(candidate.current.clone(), &BTreeSet::new());
                            current.merge_fired(candidate);
                        } else {
                            agreed.insert(id.clone(), candidate.clone());
                        }
                    }
                }
                group.entry.window_order = group
                    .order
                    .iter()
                    .filter_map(|key| agreed.get(key).map(|t| t.current.id.clone()))
                    .collect();
                group.entry.trails = agreed
                    .values()
                    .map(|trail| (trail.current.id.clone(), trail.clone()))
                    .collect();
            }
        }
        if before != serde_json::to_vec(&book.groups).ok() {
            self.save_peers(&book);
        }
        let pending = book
            .roster
            .iter()
            .filter(|(key, _)| !book.peers.contains_key(*key))
            .map(|(id, name)| json!({"id":id,"name":name}))
            .collect::<Vec<_>>();
        drop(book);
        let mut row_entries = vec![];
        for mut group in groups {
            group.entry.facts = view::merged_facts(&group.members);
            let first = &group.boxes[0];
            let key = first["key"].as_str().unwrap_or_default();
            let name = first["name"].as_str().unwrap_or_default();
            let (_, mut row) = project(&group.entry, key, name, now);
            row["boxes"] = json!(group.boxes);
            if group.boxes.iter().all(|b| b["away"] == true) {
                row["away"] = json!(true);
            }
            row["usage"] = view::display(&group.entry, &group.members, now);
            for fact in notices
                .iter_mut()
                .filter(|n| row["id"].as_str() == Some(&n.row))
            {
                if let Some(source) = group
                    .members
                    .iter()
                    .find_map(|m| m.entry.trails.get(&fact.window))
                {
                    let key = source.current.limit_key();
                    if let Some(chosen) = group.entry.trails.values().find(|t| {
                        t.current.limit_key() == key && t.current.resets_at == fact.resets_at
                    }) {
                        fact.window = chosen.current.id.clone();
                    }
                }
            }
            let row_allowed = allowed.get(row["id"].as_str().unwrap_or_default()).copied();
            if let Some(extras) = row["extras"].as_array_mut() {
                let unread = extras
                    .iter()
                    .position(|e| e["kind"] == "unreadable" && e["id"] == "resets");
                if let Some(index) = unread {
                    if let Some((_, extra, _, _)) = group
                        .reset_members
                        .iter()
                        .max_by_key(|(_, _, checked, _)| *checked)
                    {
                        extras[index] = serde_json::to_value(extra).unwrap_or_default();
                    }
                }
                if let Some(extra) = extras.iter_mut().find(|e| e["kind"] == "resets") {
                    let can =
                        |(_, e, _, _): &&(Value, super::plan_facts::Extra, Option<u64>, bool)| {
                            matches!(e, super::plan_facts::Extra::Resets { usable: true, .. })
                        };
                    let holder = group
                        .reset_members
                        .iter()
                        .filter(can)
                        .find(|(b, _, _, _)| b["away"] != true)
                        .or_else(|| group.reset_members.iter().find(can));
                    extra["usable"] = json!(holder.is_some());
                    if let Some((b, _, _, agents)) = holder {
                        let mut on = json!({"key":b["key"],"name":b["name"]});
                        if b["here"] == true {
                            on["here"] = json!(true);
                        }
                        extra["on"] = on;
                        if *agents && row_allowed != Some(false) {
                            extra["agents"] = json!(true);
                        }
                    }
                }
            }
            rows.push(row);
            row_entries.push(group.entry);
        }
        view::named(&mut rows);
        let mut siblings = BTreeMap::<String, usize>::new();
        for row in &rows {
            if view::railed(row) {
                *siblings.entry(view::rail_key(row).to_owned()).or_default() += 1;
            }
        }
        for (row, entry) in rows.iter_mut().zip(&row_entries) {
            let count = siblings.get(view::rail_key(row)).copied().unwrap_or(1);
            row["card"] = view::card(entry, row, now, count);
        }
        notices.retain(|notice| {
            rows.iter()
                .find(|row| row["id"] == notice.row)
                .and_then(|row| row["windows"].as_array())
                .is_some_and(|windows| {
                    windows.iter().any(|w| {
                        w["id"] == notice.window && w["resetsAt"].as_u64() == notice.resets_at
                    })
                })
        });
        let own_notices = notices
            .iter()
            .filter(|n| n.forwarder.as_deref().is_none_or(|id| id == self.key))
            .cloned()
            .collect::<Vec<_>>();
        let rail = view::rail(&rows);
        self.menu.publish(&rows);
        let mut data = json!({"accounts":accounts,"rows":rows,"rail":rail,"services":services,"pendingComputers":pending,"refreshPending":self.refreshing.try_lock().is_err(),"refreshFailed":self.cache_error.is_some()||self.refresh_failed.load(std::sync::atomic::Ordering::Relaxed),"notificationAvailability":settings::notification_availability(),"notices":notices,"ownNotices":own_notices});
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
        value
            .as_object_mut()
            .expect("window")
            .shift_remove("amount");
    }
    value
}
fn ordered_trails(entry: &Entry) -> Vec<&Trail> {
    let mut seen = BTreeSet::new();
    entry
        .window_order
        .iter()
        .chain(entry.trails.keys())
        .filter(|id| seen.insert((*id).clone()))
        .filter_map(|id| entry.trails.get(id))
        .collect()
}
fn ring_spans<'a>(
    current: &[&'a Window],
    service: Provider,
    now: u64,
) -> (Option<&'a String>, Option<&'a String>) {
    let Some(first) = current.first() else {
        return (None, None);
    };
    let mut whole = current
        .iter()
        .copied()
        .filter(|w| w.model.as_deref().is_none_or(str::is_empty))
        .collect::<Vec<_>>();
    if whole.is_empty() {
        let model = first.model.as_deref().unwrap_or_default();
        whole = current
            .iter()
            .copied()
            .filter(|w| {
                w.model
                    .as_deref()
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(model)
            })
            .collect();
    }
    let plain = whole
        .iter()
        .copied()
        .filter(|w| w.name.as_deref().is_none_or(str::is_empty))
        .collect::<Vec<_>>();
    let mut lead = whole[0];
    let used = |w: &Window| if w.expired(now) { 0.0 } else { w.used };
    if service == Provider::Antigravity {
        for w in &whole {
            if used(w) > used(lead) {
                lead = w;
            }
        }
    }
    fn stem(id: &str) -> &str {
        id.rfind(['-', '_']).map_or(id, |at| &id[..at])
    }
    let pool = if plain.is_empty() {
        whole
            .into_iter()
            .filter(|w| stem(&w.id) == stem(&lead.id))
            .collect::<Vec<_>>()
    } else {
        plain
    };
    let (mut short, mut long): (Option<&Window>, Option<&Window>) = (None, None);
    for window in &pool {
        let Some(minutes) = window.minutes.filter(|m| *m > 0) else {
            continue;
        };
        if minutes < 1440 {
            if short.is_none_or(|old| minutes < old.minutes.unwrap_or_default()) {
                short = Some(window);
            }
        } else if long.is_none_or(|old| minutes > old.minutes.unwrap_or_default()) {
            long = Some(window);
        }
    }
    if short.is_none() && long.is_none() {
        long = pool.first().copied();
    }
    (short.map(|w| &w.id), long.map(|w| &w.id))
}
fn project(entry: &Entry, key: &str, name: &str, now: u64) -> (Value, Value) {
    let acc = &entry.account;
    let title = acc.title();
    let mut local = json!({"provider":acc.provider,"service":acc.service,"title":title,"windows":ordered_trails(entry).into_iter().map(|t|shown(&t.current,now)).collect::<Vec<_>>()});
    if let Ok(Value::Object(mut facts)) = serde_json::to_value(&entry.facts) {
        facts.shift_remove("partial");
        local.as_object_mut().expect("account").extend(facts);
    }
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
    let ordered = ordered_trails(entry);
    let current: Vec<_> = ordered.iter().map(|t| &t.current).collect();
    let (short, long) = ring_spans(&current, acc.service, now);
    let mut row = local.clone();
    let mut rings = Map::new();
    for (value, trail) in row["windows"]
        .as_array_mut()
        .expect("windows")
        .iter_mut()
        .zip(ordered)
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
    if rings.is_empty() {
        let balance = entry.facts.balance.as_ref().and_then(|b| {
            b.limit
                .filter(|n| *n > 0.0)
                .map(|limit| 1.0 - b.left / limit)
        });
        let extras = entry.facts.extras.iter().filter_map(|extra| match extra {
            super::plan_facts::Extra::Capped { amount, limit, .. } if *limit > 0.0 => {
                Some(amount / limit)
            }
            _ => None,
        });
        if let Some(used) = balance.into_iter().chain(extras).reduce(f64::max) {
            row["level"] = json!(level(used));
        }
    }
    if entry
        .facts
        .problem
        .as_ref()
        .is_some_and(|p| p.kind == super::plan_facts::ProblemKind::Ended)
    {
        row["ended"] = json!(true);
        row["windows"] = json!([]);
        row["rings"] = json!({});
        row.as_object_mut().expect("row").shift_remove("level");
        row.as_object_mut().expect("row").shift_remove("bar");
    }
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
    F: Fn(Option<String>, bool) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = bool> + Send + 'static,
{
    let read = Arc::new(read);
    let inbound = service.clone();
    let inbound_read = read.clone();
    let mut requests = service.local_refresh.subscribe();
    tokio::spawn(async move {
        while let Ok(ids) = requests.recv().await {
            let local = {
                let state = inbound.state.lock().expect("limits lock");
                state
                    .entries
                    .values()
                    .filter(|entry| ids.contains(&entry.account.row(&inbound.key)))
                    .map(|entry| entry.account.provider.clone())
                    .collect::<Vec<_>>()
            };
            if !local.is_empty() {
                start_read(
                    &inbound,
                    inbound_read.clone(),
                    true,
                    true,
                    if local.len() == 1 {
                        local.into_iter().next()
                    } else {
                        None
                    },
                );
            }
        }
    });
    let mut clients = routes.client_activity();
    let connected_service = service.clone();
    let connected_reader = read.clone();
    tokio::spawn(async move {
        use std::time::Duration;
        loop {
            if let Some(()) = clients.wait_for(|n| *n > 0).await.ok().map(|_| ()) {
                // Do not keep a periodic wake when the final client leaves, and don't read
                // just because a brief startup/request probe was connected for a moment.
                let mut next = tokio::time::Instant::now() + Duration::from_secs(60);
                loop {
                    tokio::select! {
                        changed=clients.changed()=>{
                            if changed.is_err(){return;}
                            if *clients.borrow()==0 {break;}
                        },
                        _=tokio::time::sleep_until(next)=>{
                            if *clients.borrow()==0 {break;}
                            start_read(&connected_service,connected_reader.clone(),false,false,None);
                            next=tokio::time::Instant::now()+Duration::from_secs(60);
                        }
                    }
                }
            } else {
                return;
            }
        }
    });

    let menu_service = service.clone();
    let menu_reader = read.clone();
    let mut requested = service.menu.requests();
    tokio::spawn(async move {
        while requested.changed().await.is_ok() {
            start_read(&menu_service, menu_reader.clone(), false, false, None);
        }
    });
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
                        if start_read(&deadlines,deadline_reader.clone(),true,false,Some(provider.clone())) {
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
            let _ = service.remote_refresh.send(());
            if !start_read(&service, read.clone(), true, true, None) {
                if let Ok(guard) = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    service.refreshing.lock(),
                )
                .await
                {
                    drop(guard);
                    start_read(&service, read, true, true, None);
                }
            }
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
                        _=demand.tick()=>{start_read(&service,read.clone(),false,false,None);},
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
    explicit: bool,
    provider: Option<String>,
) -> bool
where
    F: Fn(Option<String>, bool) -> Fut + Send + Sync + 'static,
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
            let failed = read(provider, explicit).await;
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

pub fn register<F, Fut>(routes: &mut Registry, service: Service, verify: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    routes.register::<Get, _, _>(Access::Any, move |_| {
        let service = service.clone();
        let verify = verify();
        async move {
            verify.await;
            tokio::task::spawn_blocking(move || RouteReply::json::<200>(service.answer(now())))
                .await
                .map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
    });
}
