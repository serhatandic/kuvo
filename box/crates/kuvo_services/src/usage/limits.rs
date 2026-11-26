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

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// A name from a newer computer; no local reader or reset is inferred.
    Future(String),
}
impl Serialize for Provider {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}
impl<'de> Deserialize<'de> for Provider {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        if name.is_empty()
            || name.len() > 40
            || !name.as_bytes()[0].is_ascii_lowercase() && !name.as_bytes()[0].is_ascii_digit()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(serde::de::Error::custom("Invalid usage service name"));
        }
        Ok(match name.as_str() {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "antigravity" => Self::Antigravity,
            "cursor" => Self::Cursor,
            "grok" => Self::Grok,
            "copilot" => Self::Copilot,
            "openrouter" => Self::Openrouter,
            "zai" => Self::Zai,
            "ollama" => Self::Ollama,
            "opencode" => Self::Opencode,
            _ => Self::Future(name),
        })
    }
}
impl Provider {
    pub fn name(&self) -> &str {
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
            Self::Future(name) => name,
        }
    }
    fn title(&self, agent: bool) -> &str {
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
            Self::Future(name) => name,
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
    // The local Claude settings card key; never included in peer account mail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_card: Option<String>,
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
    pub fn notice_card_of(raw: &str) -> Option<String> {
        let suffix = raw.strip_prefix("anthropic:")?;
        Some(format!(
            "claude@{}",
            hex::encode(&Sha256::digest(suffix.to_lowercase().as_bytes())[..4])
        ))
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
            service: account.service.clone(),
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
    #[serde(default, skip_serializing_if = "is_zero")]
    refusals: u8,
    /// This locally admitted sign-in deferred a read to a peer’s real observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    borrowed_until: Option<u64>,
}
fn is_zero(value: &u8) -> bool {
    *value == 0
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    version: u32,
    entries: BTreeMap<String, Entry>,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notice_setup: Option<String>,
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
fn peer_borrow_until(checked: u64, now: u64, every: u64, agreed: Option<u64>) -> Option<u64> {
    let next = checked.saturating_add(every).saturating_add(2 * 60_000);
    // Honor the slack only if a fresh reading already deferred this computer.
    (now.saturating_sub(checked) < every || agreed.is_some_and(|at| at > now && next > now))
        .then_some(next)
}

/// A setup change invalidates notice decisions, not the accepted usage numbers.
fn update_notice_scope(
    state: &mut Saved,
    previous: Option<&str>,
    scope: Option<&str>,
) -> (bool, bool) {
    let replaced = scope.is_some_and(|scope| {
        state
            .notice_setup
            .as_deref()
            .or(previous)
            .is_some_and(|old| old != scope)
    });
    if replaced {
        for entry in state.entries.values_mut() {
            entry.notices.clear();
            for trail in entry.trails.values_mut() {
                trail.clear_fired();
            }
        }
    }
    let changed = scope.is_some_and(|scope| state.notice_setup.as_deref() != Some(scope));
    if changed {
        state.notice_setup = scope.map(str::to_owned);
    }
    (changed, replaced)
}

/// Resolve the pins on the same notice occurrence, not on every new directory
/// view. A known pin survives a holder leaving; only a conflicting pin can
/// change it. An unpinned decision waits until membership is known.
fn reconcile_notice_pins(
    state: &mut Saved,
    book: &mut Book,
    local_key: &str,
    now: u64,
) -> (bool, bool) {
    if book.scope.is_none() {
        return (false, false);
    }
    let mut facts = BTreeMap::<String, (String, Option<String>)>::new();
    let local_facts = state.entries.values().flat_map(|e| &e.notices);
    let peer_facts = book
        .peers
        .iter()
        .filter(|(key, peer)| book.roster.contains_key(*key) && peer.setup == book.scope)
        .flat_map(|(_, peer)| peer.accounts.iter().flat_map(|a| &a.notices));
    for fact in local_facts.chain(peer_facts).filter(|f| f.current(now)) {
        let (_, pin) = facts
            .entry(fact.id.clone())
            .or_insert_with(|| (fact.row.clone(), None));
        if let Some(given) = &fact.forwarder {
            if pin.as_ref().is_none_or(|old| given < old) {
                *pin = Some(given.clone());
            }
        }
    }
    for (row, pin) in facts.values_mut() {
        if pin.is_some() {
            continue;
        }
        let local = state
            .entries
            .values()
            .any(|entry| entry.account.row(local_key) == row.as_str())
            .then_some(local_key);
        let peer = book
            .peers
            .iter()
            .filter(|(key, peer)| {
                book.roster.contains_key(*key)
                    && peer.setup == book.scope
                    && now.saturating_sub(peer.heard) <= AWAY
            })
            .filter(|(key, peer)| {
                peer.accounts
                    .iter()
                    .any(|account| account.row(key) == row.as_str())
            })
            .map(|(key, _)| key.as_str())
            .min();
        *pin = local.into_iter().chain(peer).min().map(str::to_owned);
    }
    let update = |fact: &mut Fact| {
        let Some((_, pin)) = facts.get(&fact.id) else {
            return false;
        };
        if &fact.forwarder == pin {
            return false;
        }
        fact.forwarder = pin.clone();
        true
    };
    let mut local_changed = false;
    for fact in state.entries.values_mut().flat_map(|e| &mut e.notices) {
        local_changed |= update(fact);
    }
    let mut peer_changed = false;
    for (key, peer) in &mut book.peers {
        if !book.roster.contains_key(key) || peer.setup != book.scope {
            continue;
        }
        for fact in peer
            .accounts
            .iter_mut()
            .flat_map(|account| &mut account.notices)
        {
            peer_changed |= update(fact);
        }
    }
    (local_changed, peer_changed)
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
type Tell = dyn Fn(Fact) -> bool + Send + Sync;
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
    told: Arc<Mutex<BTreeMap<String, u64>>>,
    refreshing: Arc<tokio::sync::Mutex<()>>,
    /// Odd while a client manual round runs, advanced when it ends. A manual
    /// press can join that round even if it completes before its reply wait.
    explicit_round: Arc<std::sync::atomic::AtomicU64>,
    last_refresh: Arc<std::sync::atomic::AtomicU64>,
    last_turn_save: Arc<std::sync::atomic::AtomicU64>,
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
        for entry in saved.entries.values_mut() {
            let count = entry.trails.len();
            entry.trails.retain(|_, trail| trail.valid_kept());
            if entry.trails.len() != count {
                entry
                    .window_order
                    .retain(|id| entry.trails.contains_key(id));
                entry
                    .partial_missing
                    .retain(|id| entry.trails.contains_key(id));
                entry
                    .notices
                    .retain(|notice| entry.trails.contains_key(&notice.window));
                entry.facts.checked = None;
                entry.facts.values_read = None;
                entry.facts.next = None;
                entry.borrowed_until = None;
            }
        }
        let legacy_cache = if file.exists() || cache_error.is_some() {
            super::legacy_limits::Cache::default()
        } else {
            super::legacy_limits::Cache::load(&home)
        };
        let peer_file = home.join("agents/usage/accepted-limits-peers.json");
        let legacy = home.join("agents/limits-peers.json");
        let older = home.join("agents/usage/limits-peers.json");
        let peer_book = Book::read(if peer_file.exists() {
            &peer_file
        } else if legacy.exists() {
            &legacy
        } else {
            &older
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
            told: Arc::default(),
            refreshing: Arc::default(),
            explicit_round: Arc::default(),
            last_refresh: Arc::default(),
            last_turn_save: Arc::default(),
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
                    service: acc.service.clone(),
                    account: acc.account.clone(),
                    agent: acc.account.as_ref().map(|_| acc.service.clone()),
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
        let (changed, replaced_scope) = {
            let mut state = self.state.lock().expect("limits lock");
            let mut book = self.peer_book.lock().expect("peer limits");
            let (marker_changed, replaced_scope) =
                update_notice_scope(&mut state, book.scope.as_deref(), scope);
            let roster_changed = book.roster(names, scope);
            let (local_changed, peer_changed) =
                if marker_changed || roster_changed || replaced_scope {
                    reconcile_notice_pins(&mut state, &mut book, &self.key, now)
                } else {
                    (false, false)
                };
            // A scope marker alone must not create an empty accepted cache
            // before the still-admitted legacy accounts have been imported.
            if !state.entries.is_empty() && (marker_changed || local_changed || replaced_scope) {
                self.save_notice_entries(&state);
            }
            if roster_changed || peer_changed {
                self.save_peers(&book);
            }
            (
                roster_changed || local_changed || peer_changed || replaced_scope,
                replaced_scope,
            )
        };
        if replaced_scope {
            self.told.lock().expect("usage notice deliveries").clear();
        }
        if changed {
            self.replay_notices(now);
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
        {
            let has_notices = reading
                .accounts
                .iter()
                .any(|account| !account.notices.is_empty());
            let mut state = has_notices.then(|| self.state.lock().expect("limits lock"));
            let mut book = self.peer_book.lock().expect("peer limits");
            if !book.roster.contains_key(key) {
                return;
            }
            book.take(key, name, reading, now);
            if let Some(state) = state.as_deref_mut() {
                let (local_changed, _) = reconcile_notice_pins(state, &mut book, &self.key, now);
                if local_changed {
                    self.save_notice_entries(state);
                }
            }
            self.save_peers(&book);
        }
        self.replay_notices(now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
        if !refresh.is_empty() {
            let _ = self.local_refresh.send(refresh);
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
    fn save_notice_entries(&self, state: &Saved) {
        if self.cache_error.is_none()
            && let Ok(bytes) = serde_json::to_vec(state)
            && let Err(error) = kuvo_store::files::write_secret(&self.file, &bytes)
        {
            eprintln!("Couldn't keep usage notice decisions: {error}");
        }
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
                if let Err(error) = self.observe_facts_inner(
                    account.clone(),
                    vec![window],
                    None,
                    now,
                    true,
                    false,
                    None,
                ) {
                    eprintln!("Couldn't keep logged plan limits: {error}");
                }
            }
        }
    }
    pub fn attach_notices(&self, tell: Arc<Tell>) {
        let _ = self.tell.set(tell);
    }
    /// Replay after sign-in/roster confirmation, and retry failed notice
    /// journal writes on later observations. The rendered answer remains the
    /// authority for which facts still belong to current accepted windows.
    pub fn replay_notices(&self, at: u64) {
        let facts = {
            let state = self.state.lock().expect("limits lock");
            self.publish(&state, at).1
        };
        self.dispatch_notices(facts, at);
    }
    fn dispatch_notices(&self, facts: Vec<Fact>, at: u64) {
        let Some(tell) = self.tell.get() else {
            return;
        };
        let mut delivered = self.told.lock().expect("usage notice deliveries");
        delivered.retain(|_, read| at.saturating_sub(*read) < 600_000);
        for fact in facts
            .into_iter()
            .filter(|fact| fact.forwarder.as_deref() == Some(self.key.as_str()))
        {
            if !delivered.contains_key(&fact.id) && tell(fact.clone()) {
                delivered.insert(fact.id, fact.at);
            }
        }
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
        self.observe_facts_inner(account, windows, facts, now, false, false, None)
    }
    /// A reader admitted the account before asking its maker. If a sign-out
    /// was published while it waited, that answer can no longer resurrect it.
    pub fn observe_confirmed_facts(
        &self,
        account: Account,
        windows: Vec<Window>,
        facts: super::plan_facts::Facts,
        started: u64,
        now: u64,
    ) -> std::io::Result<()> {
        self.observe_facts_inner(
            account,
            windows,
            Some(facts),
            now,
            true,
            false,
            Some(started),
        )
    }
    pub fn observe_reset_facts(
        &self,
        account: Account,
        facts: super::plan_facts::Facts,
        now: u64,
    ) -> std::io::Result<()> {
        self.observe_facts_inner(account, vec![], Some(facts), now, true, true, None)
    }
    fn observe_facts_inner(
        &self,
        account: Account,
        mut windows: Vec<Window>,
        facts: Option<super::plan_facts::Facts>,
        now: u64,
        require_current: bool,
        only_resets: bool,
        maker_started: Option<u64>,
    ) -> std::io::Result<()> {
        use super::plan_facts::ProblemKind;
        let heard = facts.is_none()
            && !windows.is_empty()
            && windows
                .iter()
                .all(|w| w.source == super::readings::Source::Turn);
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
            refusals: 0,
            borrowed_until: None,
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
        if let Some(started) = maker_started {
            // A turn can hear a new quota while a maker request is in flight.
            // An answer to the earlier request cannot supersede that window.
            windows.retain(|window| {
                !equivalent_trail(&entry.trails, window)
                    .and_then(|id| entry.trails.get(&id))
                    .is_some_and(|trail| {
                        trail.current.source == super::readings::Source::Turn
                            && trail.current.read >= started
                    })
            });
        }
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
        if let Some(mut facts) = facts {
            if !only_resets && facts.problem.is_none() {
                entry.borrowed_until = None;
            }
            if entry.account.account.is_none() && !only_resets {
                if let Some(problem) = facts.problem.as_mut() {
                    if problem.kind == ProblemKind::RateLimited {
                        entry.refusals = entry.refusals.saturating_add(1);
                        let wait = problem
                            .until
                            .map(|until| until.max(now + 10 * 60_000))
                            .unwrap_or_else(|| {
                                now + (60
                                    * 60_000
                                    * (1u64 << entry.refusals.saturating_sub(1).min(3)))
                                .min(6 * 60 * 60_000)
                            });
                        facts.next = Some(wait);
                        problem.until = Some(wait);
                    }
                } else {
                    entry.refusals = 0;
                }
            }
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
                    if problem.kind == ProblemKind::Ended {
                        facts.plan = facts.plan.or_else(|| entry.facts.plan.clone());
                    }
                    entry.facts = facts;
                } else {
                    if entry.facts.balance.is_some() || !entry.facts.extras.is_empty() {
                        entry.facts.values_read = entry.facts.meter_read();
                    }
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
            let toggles = self.settings.notification_milestones(
                entry.account.service.name(),
                entry.account.notice_card.as_deref(),
                &window,
            );
            let semantic = window.limit_key();
            let shared_trail = book
                .groups
                .get(&row_id)
                .and_then(|group| group.get(&semantic))
                .cloned();
            let local_toggles = toggles.as_ref().map(|toggles| {
                if shared_trail.is_none() {
                    toggles.clone()
                } else {
                    BTreeSet::new()
                }
            });
            let prior_id = equivalent_trail(&entry.trails, &window);
            let mut local = prior_id.as_ref().and_then(|id| entry.trails.remove(id));
            if let Some(trail) = local.as_mut() {
                if let Some(fired) = trail.observe_visible(window.clone(), local_toggles.as_ref()) {
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
                if toggles.is_none()
                    && let Some(trail) = local.as_mut()
                {
                    trail.hide_notice();
                }
            }
            if let Some(mut shared) = shared_trail {
                if let Some(local) = local.as_ref() {
                    shared.merge_fired(local);
                }
                if let Some(fired) = shared.observe_visible(window, toggles.as_ref()) {
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
            if book.scope.is_none() {
                continue;
            }
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
        // The accepted reading is always live. Normal in-window turn samples
        // need not write the whole cache on each assistant response. Preserve
        // new windows/sign-ins, dormant scope transitions and new notice facts
        // immediately; ordinary samples keep TS's thirty-second disk cadence.
        let critical = state.entries.get(&provider).is_none_or(|old| {
            !same_sign_in(&old.account, &entry.account)
                || entry.trails.values().any(|trail| {
                    old.trails.get(&trail.current.id).is_none_or(|prior| {
                        prior.current.resets_at != trail.current.resets_at
                            || prior.notice_is_dormant() != trail.notice_is_dormant()
                    })
                })
        });
        use std::sync::atomic::Ordering::Relaxed;
        let last = self.last_turn_save.load(Relaxed);
        let persist = !heard
            || critical
            || !facts.is_empty()
            || last == 0
            || now.saturating_sub(last) >= 30_000;
        let before = persist.then(|| state.clone());
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
        if persist {
            if let Err(err) = kuvo_store::files::write_secret(
                &self.file,
                &serde_json::to_vec(&*state).map_err(std::io::Error::other)?,
            ) {
                if let Some(before) = before {
                    *state = before;
                }
                return Err(err);
            }
            if heard {
                self.last_turn_save.store(now, Relaxed);
            }
            let book = self.peer_book.lock().expect("peer limits");
            if book.groups.contains_key(&row_id) {
                self.save_peers(&book);
            }
        }
        let (_, current_notices) = self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
        drop(state);
        self.dispatch_notices(current_notices, now);
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
                    refusals: 0,
                    borrowed_until: None,
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
            if a.sign_in != entry.account.sign_in
                && entry.facts.problem.as_ref().is_some_and(|p| {
                    matches!(
                        p.kind,
                        super::plan_facts::ProblemKind::Expired
                            | super::plan_facts::ProblemKind::Unreadable
                    )
                })
            {
                entry.facts.problem = None;
                entry.facts.next = None;
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
                .is_none_or(|e| a.sign_in != e.account.sign_in || !same_sign_in(a, &e.account))
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
        let mut changed = state.entries.len() != before;
        for account in accounts {
            if let Some(entry) = state.entries.get_mut(&account.provider)
                && entry.account.account.is_none()
                && entry.account.sign_in != account.sign_in
            {
                entry.account.sign_in = account.sign_in.clone();
                // A renewal of the same maker identity makes an expired
                // sign-in eligible again, not a new usage window.
                if entry.facts.problem.as_ref().is_some_and(|p| {
                    matches!(
                        p.kind,
                        super::plan_facts::ProblemKind::Expired
                            | super::plan_facts::ProblemKind::Unreadable
                    )
                }) {
                    entry.facts.problem = None;
                    entry.facts.next = None;
                }
                changed = true;
            }
        }
        if !changed {
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
        if state.entries.len() == before && state.order == order {
            return;
        }
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
                    && entry.facts.problem.as_ref().is_some_and(|p| match p.kind {
                        super::plan_facts::ProblemKind::Expired
                        | super::plan_facts::ProblemKind::Unreadable => true,
                        super::plan_facts::ProblemKind::RateLimited => {
                            entry.facts.next.is_some_and(|n| n > now)
                        }
                        _ => false,
                    })
            })
            .map(|e| e.account.provider.clone())
            .collect()
    }

    /// The Cursor plan and account-wide history share a maker's pause. A
    /// retained refusal belongs to the credential that received it, not a
    /// replacement sign-in. Unlike normal freshness, explicit refresh cannot
    /// override it.
    pub(super) fn held_cursor(&self, account: &Account, now: u64) -> bool {
        if account.service != Provider::Cursor {
            return false;
        }
        let state = self.state.lock().expect("limits lock");
        state.entries.get(&account.provider).is_some_and(|entry| {
            account.sign_in == entry.account.sign_in
                && entry.facts.problem.as_ref().is_some_and(|problem| {
                    use super::plan_facts::ProblemKind;
                    match problem.kind {
                        ProblemKind::Expired => true,
                        ProblemKind::RateLimited => problem
                            .until
                            .or(entry.facts.next)
                            .is_some_and(|until| until > now),
                        _ => false,
                    }
                })
        })
    }

    /// A connected client need not ask the same maker again while its own
    /// reading is current or an admitted peer just read the same account.
    /// A borrowed reading never becomes a local checked timestamp or notice.
    pub fn defer_passive(&self, account: &Account, now: u64, every: u64) -> bool {
        let mut state = self.state.lock().expect("limits lock");
        let mut account = account.clone();
        let old = state
            .entries
            .get(&account.provider)
            .filter(|entry| same_sign_in(&entry.account, &account));
        if let Some(entry) = old {
            if account.identity.is_none() && account.sign_in == entry.account.sign_in {
                account.identity = entry.account.identity.clone();
            }
            if entry.facts.next.is_some_and(|at| at > now)
                || entry.facts.problem.as_ref().is_some_and(|p| {
                    matches!(
                        p.kind,
                        super::plan_facts::ProblemKind::Expired
                            | super::plan_facts::ProblemKind::Unreadable
                    )
                })
            {
                return true;
            }
        }
        let peer_checked = {
            let book = self.peer_book.lock().expect("peer limits");
            book.freshest(&account).and_then(|peer| peer.facts.checked)
        };
        let Some(checked) = peer_checked else {
            return false;
        };
        let Some(next) = peer_borrow_until(
            checked,
            now,
            every,
            old.and_then(|entry| entry.borrowed_until),
        ) else {
            return false;
        };
        if old.is_some_and(|entry| entry.borrowed_until == Some(next) && entry.account == account) {
            return true;
        }
        let mut entry = old.cloned().unwrap_or_else(|| Entry {
            account: account.clone(),
            facts: Default::default(),
            trails: BTreeMap::new(),
            partial_missing: BTreeSet::new(),
            window_order: vec![],
            notices: vec![],
            refusals: 0,
            borrowed_until: None,
        });
        entry.account = account.clone();
        entry.borrowed_until = Some(next);
        if !state.order.contains(&account.provider) {
            state.order.push(account.provider.clone());
        }
        state.entries.insert(account.provider, entry);
        if self.cache_error.is_none()
            && let Ok(bytes) = serde_json::to_vec(&*state)
            && let Err(error) = kuvo_store::files::write_secret(&self.file, &bytes)
        {
            eprintln!("Couldn't keep the peer plan read schedule: {error}");
        }
        self.publish(&state, now);
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
        true
    }
    pub fn defer_passive_provider(&self, provider: &str, now: u64, every: u64) -> bool {
        let account = self
            .state
            .lock()
            .expect("limits lock")
            .entries
            .get(provider)
            .map(|entry| entry.account.clone());
        account.is_some_and(|account| self.defer_passive(&account, now, every))
    }

    /// Completion belongs to a refresh round, not to any one provider.
    fn checked_times(&self) -> BTreeMap<String, u64> {
        self.state
            .lock()
            .expect("limits lock")
            .entries
            .iter()
            .filter_map(|(id, entry)| entry.facts.checked.map(|at| (id.clone(), at)))
            .collect()
    }
    fn round_failed(&self, before: &BTreeMap<String, u64>, transport_failed: bool) -> bool {
        let state = self.state.lock().expect("limits lock");
        let mut checked = false;
        for (id, entry) in &state.entries {
            let Some(at) = entry.facts.checked.filter(|at| before.get(id) != Some(at)) else {
                continue;
            };
            checked = true;
            let Some(problem) = &entry.facts.problem else {
                return false;
            };
            if problem.kind == super::plan_facts::ProblemKind::Ended
                || problem.unknown && entry.trails.values().any(|trail| trail.current.read == at)
            {
                return false;
            }
        }
        checked || transport_failed
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
        self.publish(&state, now).0
    }
    fn publish(&self, state: &Saved, now: u64) -> (Value, Vec<Fact>) {
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
        let borrowed = state
            .ordered()
            .into_iter()
            .filter_map(|entry| {
                entry.borrowed_until?;
                let peer = book.freshest(&entry.account)?;
                (peer.facts.checked > entry.facts.checked)
                    .then(|| (entry.account.provider.clone(), peer.clone()))
            })
            .collect::<BTreeMap<_, _>>();
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
            let (mut local, row) = project(entry, key, name, now);
            if here {
                if let Some(peer) = borrowed.get(&entry.account.provider) {
                    let fields = local.as_object_mut().expect("account");
                    fields.insert(
                        "windows".into(),
                        json!(
                            peer.windows
                                .iter()
                                .map(|w| shown(w, now))
                                .collect::<Vec<_>>()
                        ),
                    );
                    if let Some(plan) = &peer.facts.plan {
                        fields.insert("plan".into(), json!(plan));
                    }
                    if let Some(balance) = &peer.facts.balance {
                        fields.insert("balance".into(), json!(balance));
                    }
                    if peer.facts.extras.is_empty() {
                        fields.shift_remove("extras");
                    } else {
                        fields.insert("extras".into(), json!(peer.facts.extras));
                    }
                    fields.shift_remove("problem");
                    fields.insert("next".into(), json!(entry.borrowed_until));
                }
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
                    refusals: 0,
                    borrowed_until: None,
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
                reconcile_account_trails(agreed, &mut group.candidates);
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
            .pending()
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
            if let Some(extras) = row.get_mut("extras").and_then(Value::as_array_mut) {
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
            .filter(|n| n.forwarder.as_deref() == Some(self.key.as_str()))
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
        (json!({"data":data}), own_notices)
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
fn reconcile_account_trails(
    agreed: &mut BTreeMap<String, Trail>,
    candidates: &mut BTreeMap<String, Vec<Trail>>,
) {
    agreed.retain(|id, _| candidates.contains_key(id));
    for (id, candidates) in candidates {
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
}
fn ordered_trails(entry: &Entry) -> Vec<&Trail> {
    let mut seen = BTreeSet::new();
    entry
        .window_order
        .iter()
        .chain(entry.trails.keys())
        .filter(|id| seen.insert((*id).clone()))
        .filter_map(|id| {
            entry
                .trails
                .get(id)
                .filter(|trail| trail.current.valid_kept())
        })
        .collect()
}
fn ring_spans<'a>(
    current: &[&'a Window],
    service: &Provider,
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
    if *service == Provider::Antigravity {
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
        facts.shift_remove("valuesRead");
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
    let (short, long) = ring_spans(&current, &acc.service, now);
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
    let bar = match (rings.get("short"), rings.get("long")) {
        (Some(short), Some(long)) => Some(
            if short["used"].as_f64().unwrap_or(0.0) >= long["used"].as_f64().unwrap_or(0.0) {
                ("short", short)
            } else {
                ("long", long)
            },
        ),
        (Some(short), None) => Some(("short", short)),
        (None, Some(long)) => Some(("long", long)),
        _ => None,
    };
    if let Some((bar, ring)) = bar {
        row["bar"] = json!(bar);
        row["level"] = ring["level"].clone();
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
                    false,
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
                            start_read(&connected_service,connected_reader.clone(),false,false,None,false);
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
            start_read(
                &menu_service,
                menu_reader.clone(),
                false,
                false,
                None,
                false,
            );
        }
    });
    let deadlines = service.clone();
    let deadline_reader = read.clone();
    tokio::spawn(async move {
        let mut changes = deadlines.changes.subscribe();
        let mut attempted = BTreeMap::<String, u64>::new();
        let mut projected_through = now();
        loop {
            let next = {
                let state = deadlines.state.lock().expect("limits lock");
                let read = state
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
                    .map(|(provider, at)| (Some(provider), at));
                // A window starts over at its reset, regardless of whether the
                // maker can be asked again yet. Publish that view without a probe.
                let book = deadlines.peer_book.lock().expect("peer limits");
                let reset = state
                    .entries
                    .values()
                    .flat_map(|entry| {
                        entry
                            .trails
                            .values()
                            .filter_map(|trail| trail.current.resets_at)
                    })
                    .chain(
                        book.peers
                            .iter()
                            .filter(|(key, peer)| {
                                book.roster.contains_key(*key)
                                    && book.scope.is_some()
                                    && peer.setup == book.scope
                            })
                            .flat_map(|(_, peer)| {
                                peer.accounts.iter().flat_map(|account| {
                                    account.windows.iter().filter_map(|window| window.resets_at)
                                })
                            }),
                    )
                    .filter(|at| *at > projected_through)
                    .min()
                    .map(|at| (None, at));
                read.into_iter()
                    .chain(reset)
                    .min_by_key(|(provider, at)| (*at, provider.is_some()))
            };
            if let Some((provider, at)) = next {
                tokio::select! {
                    result=changes.changed()=>{if result.is_err(){return;}},
                    _=tokio::time::sleep(std::time::Duration::from_millis(at.saturating_sub(now())))=>{
                        if let Some(provider) = provider {
                            if start_read(&deadlines,deadline_reader.clone(),true,false,Some(provider.clone()),false) {
                                attempted.insert(provider,at);
                            } else if changes.changed().await.is_err(){return;}
                        } else {
                            projected_through = now().max(at);
                            let service = deadlines.clone();
                            let _ = tokio::task::spawn_blocking(move || service.answer(projected_through)).await;
                            deadlines.changes.send_modify(|n| *n = n.wrapping_add(1));
                        }
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
            use std::sync::atomic::Ordering::Relaxed;
            let round = service.explicit_round.load(Relaxed);
            if !start_read(&service, read.clone(), true, true, None, true) && round % 2 == 0 {
                // A passive or peer-selected read is not a full manual round.
                // Honor the press after it, unless another client round did.
                if let Ok(guard) = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    service.refreshing.lock(),
                )
                .await
                {
                    let already_read = service.explicit_round.load(Relaxed) != round;
                    drop(guard);
                    if !already_read {
                        start_read(&service, read, true, true, None, true);
                    }
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
                        _=demand.tick()=>{start_read(&service,read.clone(),false,false,None,false);},
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
    fanout: bool,
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
        if explicit && fanout {
            service.explicit_round.fetch_add(1, Relaxed);
        }
        let service = service.clone();
        tokio::spawn(async move {
            let before = explicit.then(|| service.checked_times());
            let transport_failed = read(provider, explicit).await;
            if let Some(before) = before {
                let failed = service.round_failed(&before, transport_failed);
                service.refresh_failed.store(failed, Relaxed);
                if !failed {
                    service.refresh_completed.store(now(), Relaxed);
                }
                if fanout {
                    service.explicit_round.fetch_add(1, Relaxed);
                }
            }
            drop(guard);
            // Only the initiating client round asks peers, after local sign-ins
            // and their readings are known. A peer asked to read never fans out.
            if fanout {
                let _ = service.remote_refresh.send(());
            }
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

#[cfg(test)]
mod notice_and_peer_tests {
    use super::*;
    use crate::usage::readings::{CardPrediction, Source};
    const MIN: u64 = 60_000;
    const START: u64 = 1_800_000_000_000;
    const WEEK: u64 = 7 * 24 * 60 * MIN;
    fn weekly(at: u64, used: f64) -> Window {
        Window {
            id: "seven_day".into(),
            used,
            minutes: Some(10080),
            resets_at: Some(START + WEEK),
            model: None,
            name: None,
            amount: None,
            read: at,
            source: Source::Turn,
        }
    }
    fn account() -> Account {
        Account {
            provider: "claude-code".into(),
            service: Provider::Claude,
            account: Some("default".into()),
            label: None,
            identity: Some("abcdefabcdefabcd".into()),
            sign_in: "fixture".into(),
            notice_card: None,
        }
    }
    #[test]
    fn a_fresh_peer_defers_only_until_its_agreed_grace_period_ends() {
        let every = 30 * MIN;
        let deadline = START + 32 * MIN;
        assert_eq!(
            peer_borrow_until(START, START + MIN, every, None),
            Some(deadline)
        );
        assert_eq!(
            peer_borrow_until(START, START + 31 * MIN, every, None),
            None
        );
        assert_eq!(
            peer_borrow_until(START, START + 31 * MIN, every, Some(deadline)),
            Some(deadline)
        );
        assert_eq!(
            peer_borrow_until(START, deadline, every, Some(deadline)),
            None
        );
        assert_eq!(
            peer_borrow_until(START + 31 * MIN, deadline, every, Some(deadline)),
            Some(START + 63 * MIN)
        );
    }
    #[test]
    fn parallel_notice_occurrences_reconcile_the_pin_and_keep_it_after_directory_changes() {
        let now = START + 10 * MIN;
        let window = weekly(now, 0.95);
        let account = account();
        let mut trail = Trail::new(window.clone()).unwrap();
        trail.mark_fired_at(Milestone::UnderTenPercent, now);
        let local = Fact::new(&account, "z", &window, Milestone::UnderTenPercent, 1);
        let mut remote = Fact::new(&account, "a", &window, Milestone::UnderTenPercent, 1);
        assert_eq!(local.id, remote.id);
        assert_ne!(
            local.id,
            Fact::new(&account, "a", &window, Milestone::UnderTenPercent, 2).id
        );
        remote.forwarder = Some("a".into());
        let mut state = Saved::default();
        state.entries.insert(
            account.provider.clone(),
            Entry {
                account: account.clone(),
                facts: Default::default(),
                trails: BTreeMap::from([(window.id.clone(), trail)]),
                partial_missing: BTreeSet::new(),
                window_order: vec![window.id.clone()],
                notices: vec![local],
                refusals: 0,
                borrowed_until: None,
            },
        );
        let mut book = Book::default();
        assert_eq!(
            reconcile_notice_pins(&mut state, &mut book, "z", now),
            (false, false)
        );
        assert!(state.entries["claude-code"].notices[0].forwarder.is_none());
        assert_eq!(
            update_notice_scope(&mut state, None, Some("set_fixture")),
            (true, false)
        );
        book.roster(
            &BTreeMap::from([("a".into(), "Peer".into())]),
            Some("set_fixture"),
        );
        assert_eq!(
            reconcile_notice_pins(&mut state, &mut book, "z", now),
            (true, false)
        );
        assert_eq!(
            state.entries["claude-code"].notices[0].forwarder.as_deref(),
            Some("z")
        );
        let incoming = Reading {
            at: now,
            accounts: vec![AccountMail {
                provider: account.provider.clone(),
                service: account.service.clone(),
                account: account.account.clone(),
                agent: Some(Provider::Claude),
                identity: account.identity.clone(),
                label: None,
                facts: Default::default(),
                windows: vec![window.clone()],
                notices: vec![remote],
                notice_state: BTreeMap::new(),
                agents_reset: false,
            }],
            ask: false,
            refresh: vec![],
            notice_setup: Some("set_fixture".into()),
        };
        let admitted = Reading::from(&incoming.wire(), now, Some("set_fixture")).unwrap();
        book.take("a", "Peer", admitted, now);
        assert_eq!(
            reconcile_notice_pins(&mut state, &mut book, "z", now),
            (true, false)
        );
        assert_eq!(
            state.entries["claude-code"].notices[0].forwarder.as_deref(),
            Some("a")
        );
        book.roster(&BTreeMap::new(), Some("set_fixture"));
        assert_eq!(
            reconcile_notice_pins(&mut state, &mut book, "z", now),
            (false, false)
        );
        let mut state: Saved =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            state.entries["claude-code"].notices[0].forwarder.as_deref(),
            Some("a")
        );
        assert_eq!(
            update_notice_scope(&mut state, Some("set_fixture"), Some("set_next")),
            (true, true)
        );
        let kept = &state.entries["claude-code"];
        assert!(kept.notices.is_empty());
        assert_eq!(
            kept.trails["seven_day"].occurrence(Milestone::UnderTenPercent),
            0
        );
        assert_eq!(kept.trails["seven_day"].current, window);
    }
    #[test]
    fn weekly_forecasts_combine_both_holders_and_survive_older_fallback_and_restart() {
        let key = weekly(START, 0.42).limit_key();
        let mut agreed = BTreeMap::new();
        let mut candidates = vec![];
        for hour in [0, 2, 4, 6] {
            candidates.push(Trail::new(weekly(START + hour * 60 * MIN, 0.42)).unwrap());
            if candidates.len() > 2 {
                candidates.remove(0);
            }
            reconcile_account_trails(
                &mut agreed,
                &mut BTreeMap::from([(key.clone(), candidates.clone())]),
            );
        }
        let now = START + 6 * 60 * MIN;
        assert_eq!(
            agreed[&key].card_prediction(now),
            Some(CardPrediction::Lasts)
        );
        reconcile_account_trails(
            &mut agreed,
            &mut BTreeMap::from([(
                key.clone(),
                vec![Trail::new(weekly(START + 4 * 60 * MIN, 0.42)).unwrap()],
            )]),
        );
        assert_eq!(agreed[&key].current.read, now);
        assert_eq!(
            agreed[&key].card_prediction(now),
            Some(CardPrediction::Lasts)
        );
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("peers.json");
        let mut book = Book::default();
        book.scope = Some("set_fixture".into());
        book.groups.insert("account".into(), agreed);
        book.store(&file).unwrap();
        let kept = Book::read(&file).unwrap();
        assert_eq!(
            kept.groups["account"][&key].card_prediction(now),
            Some(CardPrediction::Lasts)
        );
    }
}
