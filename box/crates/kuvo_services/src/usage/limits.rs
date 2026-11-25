//! Resident-local accepted plan limits and their client projections.
use kuvo_os::timer_fires::{self, Timer};
use super::{
    limits_peers::{AWAY, AccountMail, Book, Reading},
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
                &window.id,
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
                    windows: entry
                        .trails
                        .values()
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
                                if let Some(other) = shared.and_then(|group| group.get(id)) {
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
    pub fn observe_reset_facts(
        &self,
        account: Account,
        facts: super::plan_facts::Facts,
        now: u64,
    ) -> std::io::Result<()> {
        self.observe_facts_inner(account, vec![], Some(facts), now, false, true)
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
            notices: vec![],
        });
        // All local sign-ins to the same maker account keep the same accepted window.
        // A turn reading of one must not create a second trail or second warning for it.
        if let Some(other) = sibling {
            for (id, trail) in &other.trails {
                if let Some(current) = entry.trails.get_mut(id) {
                    if trail.current.read > current.current.read {
                        if current
                            .observe(trail.current.clone(), &Default::default())
                            .is_some()
                        {
                            *current = trail.clone();
                        }
                    }
                } else {
                    entry.trails.insert(id.clone(), trail.clone());
                }
            }
            for fact in &other.notices {
                if !entry.notices.iter().any(|n| n.id == fact.id) {
                    entry.notices.push(fact.clone());
                }
            }
        }
        entry.account = account;
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
                    entry.notices.clear();
                    entry.facts = facts;
                } else {
                    entry.facts.problem = facts.problem;
                    entry.facts.checked = facts.checked;
                    entry.facts.next = facts.next;
                }
            } else {
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
            let shared_trail = book
                .groups
                .get(&row_id)
                .and_then(|group| group.get(&window.id))
                .cloned();
            let local_toggles = if shared_trail.is_none() {
                toggles.clone()
            } else {
                BTreeSet::new()
            };
            let local_window = window.clone();
            if let Some(trail) = entry.trails.get_mut(&window.id) {
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
                }
            } else if let Some(trail) = Trail::new(window.clone()) {
                entry.trails.insert(trail.current.id.clone(), trail);
            }
            if let Some(mut trail) = shared_trail {
                if let Some(local) = entry.trails.get(&window.id) {
                    trail.merge_fired(local);
                }
                if let Some(fired) = trail.observe(local_window, &toggles) {
                    for m in fired {
                        if let Some(local) = entry.trails.get_mut(&window.id) {
                            local.merge_fired(&trail);
                        }
                        facts.push(Fact::new(
                            &entry.account,
                            &self.key,
                            &trail.current,
                            m,
                            trail.occurrence(m),
                        ));
                    }
                }
                if let Some(group) = book.groups.get_mut(&row_id) {
                    group.insert(window.id.clone(), trail);
                }
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
        let provider = entry.account.provider.clone();
        for sibling in state
            .entries
            .values_mut()
            .filter(|s| same_account(&s.account, &entry.account))
        {
            sibling.trails = entry.trails.clone();
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
        let before = state.entries.len();
        state.entries.retain(|provider, entry| {
            !belongs(&entry.account)
                || unreadable.contains(provider)
                || accounts
                    .iter()
                    .any(|a| provider == &a.provider && same_sign_in(a, &entry.account))
        });
        for account in accounts {
            let entry = state
                .entries
                .entry(account.provider.clone())
                .or_insert_with(|| Entry {
                    account: account.clone(),
                    facts: Default::default(),
                    trails: BTreeMap::new(),
                    notices: vec![],
                });
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
            .filter(|id| {
                state
                    .entries
                    .get(*id)
                    .is_some_and(|e| !belongs(&e.account) || unreadable.contains(*id))
            })
            .cloned()
            .collect::<Vec<_>>();
        for account in accounts {
            if !order.contains(&account.provider) {
                order.push(account.provider.clone());
            }
        }
        // Coding agent accounts come before engine-only sign-ins in the contract.
        order.sort_by_key(|id| {
            state
                .entries
                .get(id)
                .is_some_and(|e| e.account.account.is_none())
        });
        state.order = order;
        if state.entries.len() < before && self.file.exists() && self.cache_error.is_none() {
            if let Ok(bytes) = serde_json::to_vec(&*state) {
                if let Err(error) = kuvo_store::files::write_secret(&self.file, &bytes) {
                    eprintln!("Couldn't keep the current plan sign-ins: {error}");
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
                matches!(e.account.service, Provider::Codex | Provider::Claude)
                    && e.account.account.is_some()
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
        state
            .ordered()
            .into_iter()
            .filter(|entry| {
                matches!(entry.account.service, Provider::Codex | Provider::Claude)
                    && entry.account.account.is_some()
            })
            .filter(|entry| provider.is_none_or(|p| p == entry.account.provider))
            .find_map(|entry| {
                let id = entry.account.row(&self.key);
                row.is_none_or(|row| row == id)
                    .then(|| (entry.account.clone(), id))
            })
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
        let mut seen = std::collections::BTreeSet::new();
        for entry in state.ordered() {
            if entry
                .facts
                .problem
                .as_ref()
                .is_some_and(|p| p.kind == super::plan_facts::ProblemKind::None)
            {
                continue;
            }
            if !seen.insert(entry.account.row(&self.key)) {
                continue;
            }
            lines.push(format!(
                "- {}{}, provider {}:",
                entry.account.title(),
                entry
                    .facts
                    .plan
                    .as_ref()
                    .map_or(String::new(), |plan| format!(", {plan}")),
                entry.account.provider
            ));
            if entry.trails.is_empty()
                && entry.facts.extras.is_empty()
                && entry.facts.balance.is_none()
            {
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
            let amount = |n: f64, unit: super::plan_facts::Unit| match unit {
                super::plan_facts::Unit::Usd => format!("${n:.2}"),
                super::plan_facts::Unit::Requests => format!("{:.0} requests", n),
                super::plan_facts::Unit::Credits => format!("{:.0} credits", n),
            };
            if let Some(b) = &entry.facts.balance {
                lines.push(format!(
                    "  - credits: {} left{}",
                    amount(b.left, b.unit),
                    b.limit
                        .map_or(String::new(), |n| format!(" of {}", amount(n, b.unit)))
                ));
            }
            use super::plan_facts::Extra;
            for extra in &entry.facts.extras {
                match extra {
                    Extra::Resets { count, usable, .. } => {
                        let id = entry.account.row(&self.key);
                        let always = self
                            .reset_permission
                            .lock()
                            .expect("reset permissions")
                            .get(&id)
                            == Some(&true);
                        let use_words = if *count > 0 && *usable {
                            format!(
                                "; use_limit_reset with account \"{id}\" uses one ({})",
                                if always {
                                    "the user allows it without asking"
                                } else {
                                    "the user approves each"
                                }
                            )
                        } else if *count > 0 {
                            "; none can be used from your computers now".into()
                        } else {
                            String::new()
                        };
                        lines.push(format!(
                            "  - rate-limit resets: {count} available{use_words}"
                        ));
                    }
                    Extra::Unreadable { message, .. } => lines.push(format!("  - {message}")),
                    Extra::Left {
                        name,
                        amount: n,
                        unit,
                        ..
                    } => lines.push(format!("  - {name}: {} left", amount(*n, *unit))),
                    Extra::Spent {
                        name,
                        amount: n,
                        unit,
                        resets_at,
                        ..
                    } if resets_at.is_none_or(|at| at > now) => {
                        lines.push(format!("  - {name}: {} spent", amount(*n, *unit)))
                    }
                    Extra::Capped {
                        name,
                        amount: n,
                        limit,
                        unit,
                        resets_at,
                        ..
                    } if resets_at.is_none_or(|at| at > now) => lines.push(format!(
                        "  - {name}: {} of {} used",
                        amount(*n, *unit),
                        amount(*limit, *unit)
                    )),
                    _ => {}
                }
            }
            if let Some(problem) = &entry.facts.problem {
                lines.push(format!("  - Not read just now: {}", problem.message));
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
        let mut rows: Vec<Value> = vec![];
        let mut rail = vec![];
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
            reset_members: Vec<(Value, super::plan_facts::Extra, Option<u64>, bool)>,
        }
        let mut groups = Vec::<Group>::new();
        let mut group_ids = BTreeMap::<String, usize>::new();
        let mut book = self.peer_book.lock().expect("peer limits");
        let mut add =
            |entry: &Entry, key: &str, name: &str, here: bool, away: bool, agents_reset: bool| {
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
                        reset_members: vec![],
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
                if entry.facts.checked > group.entry.facts.checked {
                    group.entry.facts = entry.facts.clone();
                }
                for (id, trail) in &entry.trails {
                    updated = updated.max(trail.current.read);
                    group
                        .candidates
                        .entry(id.clone())
                        .or_default()
                        .push(trail.clone());
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
                    notices: account.notices.clone(),
                };
                add(
                    &entry,
                    key,
                    &peer.name,
                    false,
                    now.saturating_sub(peer.heard) > AWAY,
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
            if paired || group.boxes.len() > 1 || had {
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
                group.entry.trails = agreed.clone();
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
        for group in groups {
            let first = &group.boxes[0];
            let key = first["key"].as_str().unwrap_or_default();
            let name = first["name"].as_str().unwrap_or_default();
            let (_, mut row) = project(&group.entry, key, name, now);
            row["boxes"] = json!(group.boxes);
            if group.boxes.iter().all(|b| b["away"] == true) {
                row["away"] = json!(true);
            }
            row["usage"]["computers"] = json!(
                group
                    .boxes
                    .iter()
                    .map(|b| {
                        let mut computer = json!({"name":b["name"]});
                        if b["here"] == true {
                            computer["here"] = json!(true);
                        }
                        if b["away"] == true {
                            computer["away"] = json!(true);
                        }
                        computer
                    })
                    .collect::<Vec<_>>()
            );
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
        for row in &rows {
            if row["windows"]
                .as_array()
                .is_some_and(|windows| !windows.is_empty())
                && row["ended"] != true
            {
                let mut item = json!({"key":row["agent"].as_str().or(row["service"].as_str()).unwrap_or_default(),"rows":[row["id"]],"ring":row["id"],"selected":row["id"],"title":row["title"]});
                if let Some(dot) = row["card"].get("dot") {
                    item["dot"] = dot.clone();
                }
                rail.push(item);
            }
        }
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
fn project(entry: &Entry, key: &str, name: &str, now: u64) -> (Value, Value) {
    let acc = &entry.account;
    let title = acc.title();
    let mut local = json!({"provider":acc.provider,"service":acc.service,"title":title,"windows":entry.trails.values().map(|t|shown(&t.current,now)).collect::<Vec<_>>()});
    if let Ok(Value::Object(facts)) = serde_json::to_value(&entry.facts) {
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
        row["card"]["lines"] = json!([]);
        row["card"]["note"] = json!("Plan ended.");
        row["usage"]["meters"] = json!([]);
        row["usage"]["status"] = json!({"words":"Plan ended.","warning":false});
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
                        _=timer_fires::sleep_until(Timer::UsageClientDemand, next)=>{
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
                    _=timer_fires::sleep(Timer::UsageResetDeadline, std::time::Duration::from_millis(at.saturating_sub(now())))=>{
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
            let mut heartbeat=timer_fires::interval(Timer::UsageLimitHeartbeat, std::time::Duration::from_secs(15));
            let mut demand=timer_fires::interval(Timer::UsageDemand, std::time::Duration::from_secs(600));
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
