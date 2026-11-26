//! Insights' portable transcript index. The SQLite schema is the same as the old service;
//! only changed transcripts are decoded, on demand, never in an idle background process.
use kuvo_os::timer_fires::{self, Timer};
use super::{native_index, pricing::Pricing};
use http::StatusCode;
use jiff::{Span, Timestamp, tz::TimeZone};
use kuvo_protocol::{
    JsonStatus, Route, RouteSpec,
    shape::{Primitive, Shape},
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[path = "insight_lineage.rs"]
pub(super) mod lineage;
#[path = "insight_peers.rs"]
mod peers;
#[path = "insight_summary.rs"]
mod summary;

const DAY: i64 = 86_400_000;
const SQL:&str="
CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS event (
 key TEXT PRIMARY KEY, seq INTEGER, sender TEXT, received INTEGER NOT NULL,
 at INTEGER NOT NULL, session TEXT NOT NULL, computer TEXT NOT NULL, bot TEXT, bot_name TEXT, title TEXT,
 provider TEXT NOT NULL, account TEXT, model TEXT, agent TEXT, human INTEGER NOT NULL, partial INTEGER NOT NULL,
 cost REAL, tokens INTEGER, input INTEGER, cache_read INTEGER, cache_write INTEGER, output INTEGER,
 skills TEXT NOT NULL, body TEXT NOT NULL, supersedes TEXT, processed INTEGER
);
CREATE INDEX IF NOT EXISTS event_time ON event(at, session);
CREATE INDEX IF NOT EXISTS event_session ON event(session, at);
CREATE INDEX IF NOT EXISTS event_computer ON event(computer, at, session);
CREATE INDEX IF NOT EXISTS event_bot ON event(bot, at, session);
CREATE INDEX IF NOT EXISTS event_provider ON event(provider, at, session);
CREATE INDEX IF NOT EXISTS event_account ON event(account, at, session);
CREATE INDEX IF NOT EXISTS event_seq ON event(seq);
CREATE INDEX IF NOT EXISTS event_supersedes ON event(supersedes) WHERE supersedes IS NOT NULL;";
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn hash(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).expect("metadata")))
}
fn get(db: &Connection, key: &str) -> Result<Option<Value>> {
    let body: Option<String> = db
        .query_row("SELECT body FROM state WHERE key=?", [key], |r| r.get(0))
        .optional()?;
    Ok(body.map(|b| serde_json::from_str(&b)).transpose()?)
}
fn set(db: &Connection, key: &str, value: &Value) -> Result<()> {
    db.execute(
        "INSERT INTO state VALUES (?,?) ON CONFLICT(key) DO UPDATE SET body=excluded.body",
        params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}
type FileStamp = Option<(u64, Option<u128>)>;
fn file_stamp(path: &std::path::Path) -> FileStamp {
    std::fs::metadata(path).ok().map(|m| {
        (
            m.len(),
            m.modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()),
        )
    })
}
struct PriceBook {
    stamp: [FileStamp; 3],
    catalog: Arc<Pricing>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
struct TranscriptRevision {
    generation: String,
    revision: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
struct TranscriptCheckpoint {
    stamp: String,
    header: String,
    cursor: Option<TranscriptRevision>,
    decoded: u64,
}
#[derive(Clone)]
pub struct Service {
    home: PathBuf,
    name: String,
    key: String,
    alias: Arc<std::sync::OnceLock<String>>,
    hostname: String,
    machine_identity: Arc<std::sync::OnceLock<String>>,
    gate: Arc<Mutex<()>>,
    caught_up: Arc<std::sync::atomic::AtomicBool>,
    prices: Arc<Mutex<Option<PriceBook>>>,
    history: Option<HistoryContext>,
    directory: Option<kuvo_peers::setup_lists::Reader>,
    native_ready: Arc<std::sync::atomic::AtomicBool>,
    engine_ready: Arc<std::sync::atomic::AtomicBool>,
    issues: Arc<Mutex<BTreeSet<String>>>,
    scope: Arc<Mutex<super::insight_mail::Scope>>,
    history_peers: Arc<Mutex<super::history_peers::Book>>,
    index_changes: tokio::sync::watch::Sender<u64>,
    manual_refresh: tokio::sync::broadcast::Sender<()>,
    summary_revision: Arc<std::sync::atomic::AtomicU64>,
    summary_cache: Arc<Mutex<BTreeMap<String, summary::Kept>>>,
    kept_summary: Arc<Mutex<Option<summary::KeptSummaries>>>,
    cursor_reader: Option<super::standalone::Readers>,
    cursor_busy: Arc<tokio::sync::Mutex<()>>,
}
#[derive(Clone)]
struct HistoryContext {
    settings: super::settings::Service,
    limits: super::limits::Service,
}
impl Service {
    pub fn new(home: PathBuf, name: String, key: String, hostname: &str) -> Self {
        let peer_history = super::history_peers::Book::read(&home.join("agents/usage/peers.json"));
        Self {
            home,
            name,
            key,
            hostname: hostname.to_owned(),
            machine_identity: Arc::default(),
            alias: Arc::default(),
            gate: Arc::default(),
            caught_up: Arc::default(),
            prices: Arc::default(),
            history: None,
            directory: None,
            native_ready: Arc::default(),
            engine_ready: Arc::default(),
            issues: Arc::default(),
            scope: Arc::default(),
            history_peers: Arc::new(Mutex::new(peer_history)),
            index_changes: tokio::sync::watch::channel(0).0,
            manual_refresh: tokio::sync::broadcast::channel(4).0,
            summary_revision: Arc::default(),
            summary_cache: Arc::default(),
            kept_summary: Arc::default(),
            cursor_reader: None,
            cursor_busy: Arc::default(),
        }
    }
    pub fn with_directory(mut self, directory: kuvo_peers::setup_lists::Reader) -> Self {
        self.directory = Some(directory);
        self
    }
    fn conversation_destination(&self, session: &str) -> Value {
        let key = self.directory.as_ref().and_then(|directory| {
            let view = directory.view();
            let owner = view.owner_of(
                kuvo_peers::directory::RecordScope::Session,
                session,
                directory.now(),
            )?;
            let key = owner.key?;
            (owner.local || view.roster.member(&key).is_some())
                .then(|| kuvo_wire::keys::encode(&key))
        });
        key.map_or_else(
            || json!({"status":"unavailable"}),
            |computer| json!({"status":"available","computer":computer}),
        )
    }
    fn peer_bots(&self) -> Vec<(String, kuvo_peers::directory::AnnouncedAgent)> {
        use kuvo_peers::directory::AgentKind;
        let Some(directory) = &self.directory else {
            return vec![];
        };
        let view = directory.view();
        view.roster
            .members
            .iter()
            .flat_map(|member| {
                let key = kuvo_wire::keys::encode(&member.key);
                view.told_agents(&member.key)
                    .into_iter()
                    .filter(|a| a.kind == AgentKind::Bot)
                    .map(move |bot| (key.clone(), bot))
            })
            .collect()
    }
    fn bot_homes(&self) -> BTreeMap<String, String> {
        let mut homes = self
            .peer_bots()
            .into_iter()
            .map(|(key, bot)| (bot.session, key))
            .collect::<BTreeMap<_, _>>();
        if let Some(directory) = &self.directory {
            if let Some(bots) = directory.view().local.bots.as_ref() {
                for bot in bots.values() {
                    homes.insert(bot.session.clone(), self.own_key().to_owned());
                }
            }
        }
        homes
    }
    pub fn with_history(
        mut self,
        settings: super::settings::Service,
        limits: super::limits::Service,
    ) -> Self {
        self.history = Some(HistoryContext { settings, limits });
        self
    }
    pub fn with_cursor(mut self, reader: Option<super::standalone::Readers>) -> Self {
        self.cursor_reader = reader.filter(super::standalone::Readers::cursor_history_available);
        self
    }
    pub async fn refresh_usage(&self) {
        self.recount().await;
        self.refresh_cursor().await;
    }
    pub(super) fn cursor_held(&self, account: &super::limits::Account, at: u64) -> bool {
        self.history
            .as_ref()
            .is_some_and(|history| history.limits.held_cursor(account, at))
    }
    pub async fn refresh_cursor(&self) {
        let Some(reader) = self.cursor_reader.as_ref() else {
            return;
        };
        let Ok(_busy) = self.cursor_busy.try_lock() else {
            return;
        };
        reader.refresh_cursor_history(self).await;
    }
    pub fn follow_cursor(
        &self,
        mut clients: tokio::sync::watch::Receiver<usize>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        self.cursor_reader.as_ref()?;
        let service = self.clone();
        Some(tokio::spawn(async move {
            service.refresh_cursor().await;
            loop {
                if clients.wait_for(|n| *n > 0).await.is_err() {
                    break;
                }
                let mut delay = timer_fires::interval(Timer::UsageInsightsRefresh, std::time::Duration::from_secs(300));
                delay.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        change=clients.changed()=>{if change.is_err(){return;}if *clients.borrow()==0{break;}},
                        _=delay.tick()=>{if *clients.borrow()==0{break;}service.refresh_cursor().await;}
                    }
                }
            }
        }))
    }
    fn fallback(&self) -> (Option<String>, bool) {
        self.history
            .as_ref()
            .map(|history| history.settings.codex_fallback())
            .unwrap_or_default()
    }
    fn ready(&self) -> bool {
        use std::sync::atomic::Ordering::Acquire;
        self.caught_up.load(Acquire)
            && self.native_ready.load(Acquire)
            && self.engine_ready.load(Acquire)
    }
    fn counted(&self, db: &Connection) -> Result<Option<i64>> {
        Ok(get(db, "rust-local-scan")?.and_then(|v| v.as_i64()))
    }
    fn price_index(
        &self,
        db: &mut Connection,
        now: i64,
    ) -> Result<(Arc<Pricing>, Option<String>, bool)> {
        let pricing = self.pricing()?;
        let (fallback, chosen) = self.fallback();
        let stamp = self
            .prices
            .lock()
            .expect("price book")
            .as_ref()
            .map(|book| book.stamp)
            .unwrap_or_default();
        let catalog = hash(&json!(["usage-pricing-v1", stamp]));
        let history_epoch = hash(&json!([catalog, fallback]));
        super::history::reprice(db, &history_epoch, &pricing, fallback.as_deref(), now)?;
        if get(db, "rust-insight-price-v1")?
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
            != Some(&catalog)
        {
            let tx = db.transaction()?;
            {
                let mut query = tx.prepare("SELECT key,body FROM event WHERE human=0")?;
                let mut rows = query.query([])?;
                while let Some(row) = rows.next()? {
                    let key: String = row.get(0)?;
                    let body: String = row.get(1)?;
                    let cost = estimate_body(&serde_json::from_str(&body)?, &pricing, now);
                    tx.execute(
                        "UPDATE event SET cost=? WHERE key=? AND cost IS NOT ?",
                        params![cost, key, cost],
                    )?;
                }
            }
            set(&tx, "rust-insight-price-v1", &json!(catalog))?;
            tx.commit()?;
            self.changed_index();
        }
        Ok((pricing, fallback, chosen))
    }
    fn machine_identity(&self) -> &str {
        self.machine_identity
            .get_or_init(|| kuvo_os::usage_computer::identity(&self.hostname))
    }
    fn alias(&self) -> &str {
        self.alias.get_or_init(|| {
            kuvo_store::files::text_of(&self.home.join("backup/restore.json"))
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| {
                    v["insightsAlias"]
                        .as_str()
                        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| {
                    hash(&json!(["kuvo insight computer", self.machine_identity()]))[..32]
                        .to_owned()
                })
        })
    }
    fn own_key(&self) -> &str {
        if self.key.is_empty() {
            self.alias()
        } else {
            &self.key
        }
    }
    fn open(&self) -> Result<Connection> {
        let folder = self.home.join("agents/usage");
        kuvo_store::files::create_private_dirs(&folder)?;
        let db = kuvo_store::sqlite::open_wal(
            &folder.join("insights.sqlite"),
            kuvo_store::sqlite::INSIGHTS_BUSY,
            "Insights database",
            &[|db| db.execute_batch(SQL)],
        )?;
        native_index::schema(&db)?;
        super::history::schema(&db)?;
        lineage::schema(&db)?;
        Ok(db)
    }
    fn pricing(&self) -> Result<Arc<Pricing>> {
        let folder = self.home.join("agents/usage/pricing");
        let stamp = ["litellm.json", "models_dev.json", "supplement.json"]
            .map(|name| file_stamp(&folder.join(name)));
        let mut book = self.prices.lock().expect("usage price book");
        if let Some(kept) = book.as_ref().filter(|kept| kept.stamp == stamp) {
            return Ok(kept.catalog.clone());
        }
        let catalog = Arc::new(Pricing::load(Some(&folder))?);
        *book = Some(PriceBook {
            stamp,
            catalog: catalog.clone(),
        });
        Ok(catalog)
    }

    /// Read only the compact usage projection of the new transcript format. Legacy JSON rows
    /// are consumed one conversation at a time. No prompt or reply words enter the index.
    pub fn scan(&self, db: &mut Connection, now: i64) -> Result<()> {
        self.scan_changed(db, now, None)
    }
    fn scan_changed(
        &self,
        db: &mut Connection,
        now: i64,
        changed: Option<&BTreeSet<String>>,
    ) -> Result<()> {
        let root = self.home.join("agents/sessions");
        let pricing = self.pricing()?;
        let mut headers = BTreeMap::new();
        let mut wanted = changed
            .cloned()
            .unwrap_or_else(|| {
                kuvo_store::files::list_of(&root)
                    .into_iter()
                    .filter_map(|name| name.to_str().map(str::to_owned))
                    .collect()
            })
            .into_iter()
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        while let Some(id) = wanted.pop() {
            if !id.starts_with("ses_")
                || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || !seen.insert(id.clone())
            {
                continue;
            }
            let folder = root.join(&id);
            let Some(text) = kuvo_store::files::text_of(&folder.join("session.json")) else {
                continue;
            };
            let Ok(session) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if session["id"].as_str() != Some(&id) {
                continue;
            }
            let node = lineage::Node::from_metadata(
                session["parentID"].as_str().map(str::to_owned),
                &session["metadata"],
            );
            wanted.extend(node.parent.iter().chain(node.starters.iter()).cloned());
            headers.insert(id, (folder, session));
        }
        let tx = db.transaction()?;
        let mut seq = get(&tx, "sequence")?.and_then(|v| v.as_u64()).unwrap_or(0);
        let nodes = headers
            .iter()
            .map(|(id, (_, session))| {
                (
                    id.clone(),
                    lineage::Node::from_metadata(
                        session["parentID"].as_str().map(str::to_owned),
                        &session["metadata"],
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let bots = lineage::Bots::read(&self.home);
        lineage::take(&tx, 1, &nodes, &bots, &mut seq, changed.is_none())?;
        for (id, (folder, session)) in &headers {
            if changed.is_some_and(|selected| !selected.contains(id)) {
                continue;
            }
            // OpenCode conversations are counted from its own database. Older portable
            // transcripts used these native-agent names before their provider IDs were
            // standardized; model routing must not reinterpret saved history as OpenCode.
            let provider = session["model"]["providerID"].as_str();
            let legacy_native = provider.is_some_and(|id| {
                matches!(id.split('@').next(), Some("claude" | "codex" | "grok"))
            });
            let agent = provider.and_then(kuvo_agents::accounts::Provider::of_model);
            if !legacy_native
                && agent
                    .is_some_and(|agent| agent.kind == kuvo_agents::accounts::AgentKind::OpenCode)
            {
                continue;
            }
            let stamp = hash(&json!(
                [
                    "session.json",
                    "agent.json",
                    "messages-v1.sqlite",
                    "messages-v1.sqlite-wal",
                    "messages.json"
                ]
                .map(|name| file_stamp(&folder.join(name))
                    .map(|(len, mtime)| (len, mtime.map(|n| n.to_string()))))
            ));
            let checkpoint = format!("scan/rust-transcript-v1/{id}");
            let before = get(&tx, &checkpoint)?
                .and_then(|v| serde_json::from_value::<TranscriptCheckpoint>(v).ok());
            let line = lineage::line_of(&tx, id)?;
            let lineage = line.root.as_str();
            let bot = line.bot.as_deref();
            let bot_name = line.name.as_deref();
            // A session's updated timestamp changes on each turn, but its attribution
            // usually does not. Only that relevant header signature forces re-attribution.
            let agent = kuvo_store::files::text_of(&folder.join("agent.json"))
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or_default();
            let threads = native_index::threads(&tx, id, &agent)?;
            let header = hash(&json!([
                session["title"],
                session["model"],
                session["parentID"],
                lineage,
                bot,
                bot_name,
                line.own,
                "skills-v1-buckets-v2",
                threads
            ]));
            if before
                .as_ref()
                .is_some_and(|b| b.stamp == stamp && b.header == header)
            {
                continue;
            }
            let mut decoded = 0;
            let mut cursor = None;
            let mut accept = |message: Value| -> Result<()> {
                let role = message["type"].as_str().unwrap_or_default();
                if !matches!(role, "user" | "assistant") {
                    return Ok(());
                }
                let human = role == "user";
                if human
                    && (session["parentID"].is_string()
                        || matches!(
                            message["metadata"]["source"].as_str(),
                            Some("agent" | "group" | "queue" | "wake" | "system")
                        ))
                {
                    return Ok(());
                }
                let completed = message["time"]["completed"].as_i64();
                if !human && completed.is_none() {
                    return Ok(());
                }
                let Some(at) = completed.or_else(|| message["time"]["created"].as_i64()) else {
                    return Ok(());
                };
                if at < now - 366 * DAY || at > now + DAY {
                    return Ok(());
                }
                if message["time"]["usageSession"]
                    .as_str()
                    .is_some_and(|s| s != id)
                {
                    return Ok(());
                }
                let Some(call) = message["id"].as_str() else {
                    return Ok(());
                };
                let computer = message["time"]["origin"].as_str().unwrap_or(self.alias());
                if computer != self.alias() && computer != self.own_key() {
                    return Ok(());
                }
                let model = if human {
                    &session["model"]
                } else {
                    &message["model"]
                };
                let account_provider = model["providerID"].as_str().unwrap_or("engine");
                let provider = provider_name(account_provider);
                let account = message["time"]["usageAccount"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{computer}/{account_provider}"));
                let key = hash(&json!(["kuvo", id, call]));
                let tokens = if human || provider == "cursor" {
                    None
                } else {
                    tokens(&message["tokens"])
                };
                let partial = !human && tokens.is_none();
                let model_id = model["id"].as_str();

                let input = tokens.as_ref().and_then(|t| t.input);
                let output = tokens.as_ref().and_then(|t| t.output);
                let cached = tokens.as_ref().and_then(|t| t.cache_read);
                let creation = tokens
                    .as_ref()
                    .and_then(|t| super::insight_mail::total([t.cache_write_5m, t.cache_write_1h]));
                let count = super::insight_mail::total([input, output, creation]);
                let processed = super::insight_mail::total([count, cached]);
                let mut body = json!({"key":key,"at":at,"session":id,"call":call,"computer":computer,"provider":provider,"account":account});
                let mut put = |name: &str, value: Option<&str>| {
                    if let Some(value) = value {
                        body[name] = json!(value);
                    }
                };
                put("title", session["title"].as_str());
                put("bot", bot);
                put("botName", bot_name);
                if lineage != id {
                    put("root", Some(lineage));
                }
                if human {
                    body["human"] = json!(true);
                } else {
                    put("model", model_id);
                    put("agent", message["agent"].as_str());
                    body["skills"] = json!(super::insight_skills::portable(&message));
                    if let Some(t) = tokens.as_ref() {
                        body["tokens"] = json!(t);
                    }
                    if partial {
                        body["partial"] = json!(true);
                    }
                }
                if line.own {
                    body["botOwn"] = json!(true);
                }
                if message["time"]["origin"].is_string() {
                    body["attributed"] = json!(true);
                }
                if message["time"]["usageAccount"].is_string() {
                    body["accountObserved"] = json!(true);
                }
                if !human {
                    let created = message["time"]["created"].as_i64().unwrap_or(at);
                    let thread = threads.get(account_provider).map(String::as_str);
                    if let Some(thread) = thread {
                        tx.execute(
                            "INSERT OR REPLACE INTO native_step VALUES(?,?,?,?,?)",
                            params![key, id, account_provider, thread, created],
                        )?;
                    } else {
                        tx.execute("DELETE FROM native_step WHERE key=?", [&key])?;
                    }
                    reconcile_native_step(
                        &tx,
                        &mut seq,
                        now,
                        &pricing,
                        &body,
                        account_provider,
                        thread,
                        created,
                    )?;
                }
                let cost = estimate_body(&body, &pricing, now);
                save_event(
                    &tx, &mut seq, now, body, cost, count, processed, input, cached, creation,
                    output,
                )?;
                Ok(())
            };
            if folder.join("messages-v1.sqlite").is_file() {
                let mut transcript = kuvo_store::sqlite::open_index(
                    &folder.join("messages-v1.sqlite"),
                    kuvo_store::sqlite::INSIGHTS_BUSY,
                )?;
                let read = transcript.transaction()?;
                let indexed:bool=read.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='insight_revision')",[],|r|r.get(0))?;
                if indexed {
                    cursor = Some(read.query_row(
                        "SELECT generation,revision FROM insight_revision WHERE id=1",
                        [],
                        |r| {
                            Ok(TranscriptRevision {
                                generation: r.get(0)?,
                                revision: r.get(1)?,
                            })
                        },
                    )?);
                }
                let from = before
                    .as_ref()
                    .filter(|b| b.header == header)
                    .and_then(|b| b.cursor.as_ref())
                    .filter(|old| {
                        cursor.as_ref().is_some_and(|new| {
                            old.generation == new.generation && old.revision <= new.revision
                        })
                    })
                    .map(|r| r.revision);
                if let Some(from) = from {
                    let mut query=read.prepare("SELECT m.insight FROM insight_change c JOIN message m ON m.position=c.position WHERE c.revision>? AND m.insight IS NOT NULL ORDER BY c.revision")?;
                    let mut rows = query.query([from])?;
                    while let Some(row) = rows.next()? {
                        let text: String = row.get(0)?;
                        accept(serde_json::from_str(&text)?)?;
                        decoded += 1;
                    }
                } else {
                    // The initial catch-up, a legacy v1 writer which hasn't installed
                    // the additive index, or a relevant ownership/header change.
                    let mut query = read.prepare(
                        "SELECT insight FROM message WHERE insight IS NOT NULL ORDER BY position",
                    )?;
                    let mut rows = query.query([])?;
                    while let Some(row) = rows.next()? {
                        let text: String = row.get(0)?;
                        accept(serde_json::from_str(&text)?)?;
                        decoded += 1;
                    }
                }
                read.commit()?;
            } else if let Some(text) = kuvo_store::files::text_of(&folder.join("messages.json")) {
                for message in serde_json::from_str::<Vec<Value>>(&text)? {
                    accept(message)?;
                    decoded += 1;
                }
            }
            set(
                &tx,
                &checkpoint,
                &serde_json::to_value(TranscriptCheckpoint {
                    stamp,
                    header,
                    cursor,
                    decoded,
                })?,
            )?;
        }
        set(&tx, "sequence", &json!(seq))?;
        // Native histories and engine observations have not been scanned by this reader yet.
        set(
            &tx,
            "rust-portable-scan",
            &json!({"scanned":now,"issues":[]}),
        )?;
        set(&tx, "rust-local-scan", &json!(now))?;
        tx.commit()?;
        Ok(())
    }
    /// Once opened, reads are served straight from SQLite. A query just before the first
    /// catch-up completes may help finish it, but it never schedules a periodic history scan.
    fn ensure(&self, db: &mut Connection, now: i64) -> Result<()> {
        use std::sync::atomic::Ordering;
        if !self.caught_up.load(Ordering::Acquire) {
            self.scan(db, now)?;
            self.caught_up.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub fn follow_native(
        &self,
        hub: kuvo_os::watch::WatchHub,
        limits: super::limits::Service,
    ) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        tokio::spawn(async move {
            let (send, mut changes) = tokio::sync::mpsc::channel(32);
            let registry =
                tokio::spawn(native_index::follow_roots(service.home.clone(), hub, send));
            struct Stop(tokio::task::JoinHandle<()>);
            impl Drop for Stop {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _registry = Stop(registry);
            let mut pending = BTreeSet::new();
            let mut scanned = BTreeSet::new();
            while let Some(change) = changes.recv().await {
                match &change {
                    native_index::Change::Roster(roots) => {
                        scanned.retain(|root| roots.contains(root));
                        pending = roots
                            .iter()
                            .filter(|root| !scanned.contains(*root))
                            .cloned()
                            .collect();
                        service
                            .native_ready
                            .store(false, std::sync::atomic::Ordering::Release);
                    }
                    native_index::Change::Scan(root) | native_index::Change::Removed(root) => {
                        pending.remove(root);
                        scanned.insert(root.clone());
                    }
                    _ => {}
                }
                let worker = service.clone();
                let limits = limits.clone();
                let result = tokio::task::spawn_blocking(move || {
                    worker
                        .fold_native(change, &limits)
                        .map_err(|e| e.to_string())
                })
                .await;
                if !matches!(result, Ok(Ok(()))) {
                    eprintln!("Couldn't fold native usage: {result:?}");
                }
                if pending.is_empty() {
                    if !service
                        .native_ready
                        .swap(true, std::sync::atomic::Ordering::AcqRel)
                    {
                        service.changed_index();
                    }
                }
            }
        })
    }
    fn fold_native(
        &self,
        change: native_index::Change,
        limits: &super::limits::Service,
    ) -> Result<()> {
        let now = Timestamp::now().as_millisecond();
        let mut affected;
        match change {
            native_index::Change::Roster(roots) => {
                let _gate = self.gate.lock().expect("insights gate");
                let mut db = self.open()?;
                affected =
                    native_index::reconcile_roots(&mut db, &roots, self.machine_identity(), now)?;
            }
            native_index::Change::Removed(root) => {
                let _gate = self.gate.lock().expect("insights gate");
                let mut db = self.open()?;
                affected = native_index::remove_root(&mut db, &root)?;
            }
            native_index::Change::Scan(root) => {
                let files;
                {
                    let _gate = self.gate.lock().expect("insights gate");
                    let scan =
                        native_index::scan_files(&root.folder, root.kind, now - native_index::KEEP);
                    let mut db = self.open()?;
                    affected = native_index::forget_missing(&mut db, &root, &scan)?;
                    files = scan.files;
                }
                affected.extend(self.fold_native_files(&root, files, limits, now)?);
            }
            native_index::Change::Files(root, changed) => {
                let files = native_index::changed_files(
                    &root.folder,
                    root.kind,
                    &changed,
                    now - native_index::KEEP,
                );
                affected = self.fold_native_files(&root, files, limits, now)?;
            }
        }
        self.reconcile_native(affected, now)?;
        self.changed_index();
        Ok(())
    }
    fn fold_native_files(
        &self,
        root: &native_index::Root,
        files: Vec<PathBuf>,
        limits: &super::limits::Service,
        now: i64,
    ) -> Result<BTreeSet<String>> {
        let mut affected = BTreeSet::new();
        let mut db = self.open()?;
        let (pricing, fallback, _) = {
            let _gate = self.gate.lock().expect("insights gate");
            self.price_index(&mut db, now)?
        };
        let mut failed = false;
        for path in files {
            let _gate = self.gate.lock().expect("insights gate");
            match native_index::fold_file(
                &mut db,
                &self.home,
                root,
                &path,
                now,
                &pricing,
                fallback.as_deref(),
            ) {
                Ok(sessions) => affected.extend(sessions),
                Err(error) => {
                    failed = true;
                    eprintln!("Couldn't fold usage file {}: {error}", path.display());
                }
            }
        }
        {
            let mut issues = self.issues.lock().expect("usage issues");
            if failed {
                issues.insert(root.provider.clone());
            } else {
                issues.remove(&root.provider);
            }
        }
        {
            let _gate = self.gate.lock().expect("insights gate");
            if !failed {
                native_index::scanned(&db, root, now)?;
            }
            set(&db, "rust-local-scan", &json!(now))?;
        }
        let logged = native_index::recent_limits(&db, root, now.max(0) as u64)?;
        if !logged.is_empty() {
            limits.observe_logged(&root.provider, logged, now.max(0) as u64);
        }
        Ok(affected)
    }
    fn reconcile_native(&self, affected: BTreeSet<String>, now: i64) -> Result<()> {
        if !affected.is_empty() {
            let _gate = self.gate.lock().expect("insights gate");
            let mut db = self.open()?;
            let pricing = self.pricing()?;
            let tx = db.transaction()?;
            let mut seq = get(&tx, "sequence")?.and_then(|v| v.as_u64()).unwrap_or(0);
            for session in affected {
                let steps = {
                    let mut query = tx.prepare_cached("SELECT n.provider,n.thread,n.created,e.body FROM native_step n JOIN event e ON n.key=e.key WHERE n.session=? ORDER BY n.created")?;
                    query
                        .query_map([session], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, i64>(2)?,
                                r.get::<_, String>(3)?,
                            ))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for (provider, thread, created, body) in steps {
                    reconcile_native_step(
                        &tx,
                        &mut seq,
                        now,
                        &pricing,
                        &serde_json::from_str(&body)?,
                        &provider,
                        Some(&thread),
                        created,
                    )?;
                }
            }
            set(&tx, "sequence", &json!(seq))?;
            tx.commit()?;
        }
        Ok(())
    }
    fn engine_folder(&self) -> PathBuf {
        std::env::var_os("KUVO_ENGINE_DATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join("data/opencode"))
    }
    pub async fn recount(&self) {
        let Some(history) = self.history.clone() else {
            return;
        };
        let roots = match native_index::roots(&self.home).await {
            Ok(roots) => roots,
            Err(error) => {
                eprintln!("Couldn't list usage roots: {error}");
                return;
            }
        };
        let service = self.clone();
        let _ = tokio::task::spawn_blocking(move || -> std::result::Result<(), String> {
            let count = || -> Result<()> {
                service
                    .fold_native(native_index::Change::Roster(roots.clone()), &history.limits)?;
                for root in roots {
                    service.fold_native(native_index::Change::Scan(root), &history.limits)?;
                }
                service
                    .native_ready
                    .store(true, std::sync::atomic::Ordering::Release);
                service.fold_engine(
                    super::engine_index::databases(&service.engine_folder())?,
                    true,
                )?;
                if !service
                    .engine_ready
                    .swap(true, std::sync::atomic::Ordering::AcqRel)
                {
                    service.changed_index();
                }
                let _gate = service.gate.lock().expect("insights gate");
                let mut db = service.open()?;
                service.scan(&mut db, Timestamp::now().as_millisecond())?;
                service
                    .caught_up
                    .store(true, std::sync::atomic::Ordering::Release);
                service.changed_index();
                Ok(())
            };
            count().map_err(|error| error.to_string())
        })
        .await;
    }
    fn history_view(&self, params: BTreeMap<String, String>) -> Result<Value> {
        let now = Timestamp::now().as_millisecond();
        let zone = params
            .get("zone")
            .and_then(|zone| TimeZone::get(zone).ok())
            .unwrap_or_else(TimeZone::system);
        let _gate = self.gate.lock().expect("insights gate");
        let mut db = self.open()?;
        let (pricing, fallback, chosen) = self.price_index(&mut db, now)?;
        let accounts = self.local_accounts(&db, now)?;
        let computer = super::history::Computer {
            key: self.own_key().into(),
            name: self.name.clone(),
            here: true,
            away: false,
            accounts,
        };
        let mut computers = vec![computer];
        computers.extend(self.peer_history(now));
        super::history::view(computers, now, zone, &pricing, fallback.as_deref(), chosen)
    }
    pub fn follow_prices(&self, personal: &std::path::Path) -> Option<tokio::task::JoinHandle<()>> {
        let mut feeds = super::price_feed::Feeds::for_home(&self.home, personal)?;
        let service = self.clone();
        Some(tokio::spawn(async move {
            loop {
                let now = super::limits::now();
                if feeds.refresh_due(now).await {
                    let service = service.clone();
                    let result =
                        tokio::task::spawn_blocking(move || -> std::result::Result<(), String> {
                            let _gate = service.gate.lock().expect("insights gate");
                            let mut db = service.open().map_err(|e| e.to_string())?;
                            service
                                .price_index(&mut db, Timestamp::now().as_millisecond())
                                .map_err(|e| e.to_string())?;
                            Ok(())
                        })
                        .await;
                    if !matches!(result, Ok(Ok(()))) {
                        eprintln!("Couldn't update usage prices: {result:?}");
                    }
                }
                timer_fires::sleep(Timer::UsagePriceFeed, std::time::Duration::from_millis(
                    feeds.next().saturating_sub(super::limits::now()).max(1000),
                ))
                .await;
            }
        }))
    }
    pub fn follow_engine(&self, hub: kuvo_os::watch::WatchHub) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        tokio::spawn(async move {
            let root = service.engine_folder();
            let (send, mut inbox) = tokio::sync::mpsc::channel(8);
            let task = tokio::spawn(super::engine_index::follow(root, hub, send));
            struct Stop(tokio::task::JoinHandle<()>);
            impl Drop for Stop {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _watch = Stop(task);
            while let Some(paths) = inbox.recv().await {
                let worker = service.clone();
                let result = tokio::task::spawn_blocking(move || {
                    worker.fold_engine(paths, false).map_err(|e| e.to_string())
                })
                .await;
                if !matches!(result, Ok(Ok(()))) {
                    eprintln!("Couldn't fold engine usage: {result:?}");
                }
                if !service
                    .engine_ready
                    .swap(true, std::sync::atomic::Ordering::AcqRel)
                {
                    service.changed_index();
                }
            }
        })
    }
    fn fold_engine(&self, paths: Vec<PathBuf>, force_headers: bool) -> Result<()> {
        let now = Timestamp::now().as_millisecond();
        let prices = self.pricing()?;
        let folder = self.engine_folder();
        let computer = self.machine_identity();
        let place = native_index::place_of(computer, &folder, None);
        let go = native_index::place_of(computer, &folder, Some("opencode:opencode-go"));
        let zen = native_index::place_of(computer, &folder, Some("opencode:opencode"));
        let key_of = |path: &Path| {
            format!(
                "scan/rust-engine-v1/{}",
                hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()))
            )
        };
        // Directory enumeration is tiny and lets a deleted channel lose its history
        // without reopening unchanged databases or touching durable Insights events.
        if let Ok(kept) = super::engine_index::databases(&folder) {
            let _gate = self.gate.lock().expect("insights gate");
            let db = self.open()?;
            let keys = kept.iter().map(|path| key_of(path)).collect::<Vec<_>>();
            super::engine_hosted_history::retain(&db, &keys)?;
        }
        let mut failed = false;
        for path in paths {
            if !path.is_file() {
                continue;
            }
            let _gate = self.gate.lock().expect("insights gate");
            let result = (|| -> Result<()> {
                let mut db = self.open()?;
                let key = key_of(&path);
                let checkpoint = get(&db, &key)?
                    .and_then(|v| serde_json::from_value::<super::engine_index::Checkpoint>(v).ok())
                    .unwrap_or_default();
                let tx = db.transaction()?;
                let mut seq = get(&tx, "sequence")?.and_then(|v| v.as_u64()).unwrap_or(0);
                let mut decoded = 0u64;
                // Read relevant saved session headers only on a changed engine channel.
                // An old index seeds that set once, including retained peer observations.
                let known = if checkpoint.lineage_version == 0 || force_headers {
                    let mut q = tx.prepare("SELECT DISTINCT session FROM event")?;
                    let rows = q.query_map([], |row| row.get::<_, String>(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                } else {
                    lineage::channel_sessions(&tx, &key)?
                };
                let mut lines = BTreeMap::<String, lineage::Line>::new();
                let next = super::engine_index::read(
                    &path,
                    now - 366 * DAY,
                    &checkpoint,
                    &known,
                    force_headers,
                    |version| super::engine_hosted_history::reset(&tx, &key, version),
                    |record, sessions| {
                        decoded += 1;
                        super::engine_codex_history::observe(&tx, &key, &record, &prices)?;
                        super::engine_hosted_history::observe(&tx, &key, &record)?;
                        let line = match lines.entry(record.session.clone()) {
                            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                            std::collections::btree_map::Entry::Vacant(entry) => {
                                entry.insert(lineage::line_of(&tx, &record.session)?)
                            }
                        };
                        self.accept_engine(&tx, &mut seq, now, &prices, record, sessions, line)
                    },
                )?;
                if let Some((next, sessions)) = next {
                    let bots = lineage::Bots::read(&self.home);
                    let nodes = sessions
                        .into_iter()
                        .map(|(id, header)| (id, header.node))
                        .collect::<BTreeMap<_, _>>();
                    lineage::keep_channel(&tx, &key, &nodes)?;
                    lineage::take(&tx, 0, &nodes, &bots, &mut seq, force_headers)?;
                    super::engine_codex_history::source(&tx, &key, next.sign_in, now, &place)?;
                    super::engine_hosted_history::source(
                        &tx,
                        &key,
                        &path.to_string_lossy(),
                        now,
                        &go,
                        &zen,
                    )?;
                    set(&tx, &key, &serde_json::to_value(next)?)?;
                    set(&tx, &format!("{key}/decoded"), &json!(decoded))?;
                    set(&tx, "sequence", &json!(seq))?;
                }
                tx.commit()?;
                Ok(())
            })();
            if let Err(error) = result {
                failed = true;
                eprintln!(
                    "Couldn't read engine usage database {}: {error}",
                    path.display()
                );
            }
        }
        {
            let mut issues = self.issues.lock().expect("usage issues");
            if failed {
                issues.insert("opencode".into());
            } else {
                issues.remove("opencode");
            }
        }
        let _gate = self.gate.lock().expect("insights gate");
        let db = self.open()?;
        set(&db, "rust-local-scan", &json!(now))?;
        self.changed_index();
        Ok(())
    }
    fn accept_engine(
        &self,
        db: &Connection,
        seq: &mut u64,
        now: i64,
        pricing: &Pricing,
        record: super::engine_index::Record,
        sessions: &BTreeMap<String, super::engine_index::Session>,
        line: &lineage::Line,
    ) -> Result<()> {
        let human = record.role == "user";
        let saved = sessions.get(&record.session);
        if human
            && (saved.is_some_and(|s| s.node.parent.is_some() || s.seat)
                || matches!(
                    record.source.as_deref(),
                    Some("agent" | "group" | "queue" | "wake" | "system")
                ))
        {
            return Ok(());
        }
        if !human && record.completed.is_none() && record.usage.is_none() {
            return Ok(());
        }
        let at = record.completed.unwrap_or(record.created);
        if at < now - 366 * DAY || at > now + DAY {
            return Ok(());
        }
        let computer = record.origin.as_deref().unwrap_or(self.alias());
        if computer != self.alias() && computer != self.own_key() {
            return Ok(());
        }
        let provider = record.provider.as_deref().unwrap_or("opencode");
        let account = record
            .observed_account
            .clone()
            .unwrap_or_else(|| format!("{computer}/{provider}"));
        let key = hash(&json!(["engine", record.session, record.call]));
        let mut body = json!({"key":key,"at":at,"session":record.session,"call":record.call,"computer":computer,"provider":provider,"account":account});
        if let Some(s) = saved {
            if let Some(title) = &s.title {
                body["title"] = json!(title);
            }
        }
        if line.root != record.session {
            body["root"] = json!(line.root);
        }
        if let Some(bot) = &line.bot {
            body["bot"] = json!(bot);
        }
        if let Some(name) = &line.name {
            body["botName"] = json!(name);
        }
        if line.own {
            body["botOwn"] = json!(true);
        }
        if record.origin.is_some() {
            body["attributed"] = json!(true);
        }
        if record.observed_account.is_some() {
            body["accountObserved"] = json!(true);
        }
        if let Some(model) = &record.model {
            body["model"] = json!(model);
        }
        if let Some(agent) = &record.agent {
            body["agent"] = json!(agent);
        }
        if let Some(skills) = &record.skills {
            body["skills"] = json!(skills);
        }
        let mut input = None;
        let mut output = None;
        let mut cached = None;
        let mut creation = None;
        let mut processed = None;
        let mut count = None;
        if human {
            body["human"] = json!(true);
        } else if let Some(raw) = record.usage {
            let buckets = tokens(&raw);
            if let Some(t) = buckets.as_ref() {
                input = t.input;
                output = t.output;
                cached = t.cache_read;
                creation = super::insight_mail::total([t.cache_write_5m, t.cache_write_1h]);
                count = super::insight_mail::total([input, output, creation]);
                processed = super::insight_mail::total(t.fields());
                body["tokens"] = json!(t);
            }
            if let Some(total) = token_count(&raw["total"]) {
                processed = Some(total);
                body["reportedTotal"] = json!(total);
            }
            if buckets.is_none() {
                body["partial"] = json!(true);
            }
        } else {
            body["partial"] = json!(true);
        }
        let cost = estimate_body(&body, pricing, now);
        save_event(
            db, seq, now, body, cost, count, processed, input, cached, creation, output,
        )?;
        Ok(())
    }
    pub fn follow(&self, hub: kuvo_os::watch::WatchHub) -> tokio::task::JoinHandle<()> {
        use kuvo_os::watch::{Notice, Options};
        let service = self.clone();
        tokio::spawn(async move {
            // Subscribe before the initial catch-up: writes concurrent with it are not lost.
            let root = service.home.join("agents/sessions");
            let mut watch = match hub
                .subscribe(
                    root,
                    Options {
                        recursive: true,
                        create: true,
                    },
                )
                .await
            {
                Ok(watch) => watch,
                Err(error) => {
                    eprintln!("Couldn't follow Insights transcripts: {error}");
                    return;
                }
            };
            while let Some(notice) = watch.recv().await {
                let mut changed = BTreeSet::new();
                let mut rescan = false;
                let mut add = |notice| match notice {
                    Notice::Changed { paths } => {
                        for path in paths {
                            if let Some(id) = path.iter().next().and_then(|part| part.to_str()) {
                                changed.insert(id.to_owned());
                            }
                        }
                    }
                    Notice::Rescan => rescan = true,
                    Notice::Failed(error) => {
                        eprintln!("Insights file watch: {error}");
                        rescan = true;
                    }
                };
                add(notice);
                // File writes arrive in bursts (SQLite WALs and metadata). Batch a burst,
                // not a timed full-history walk. Access-only events never reach this watcher.
                let until = tokio::time::Instant::now() + std::time::Duration::from_millis(250);
                while let Ok(Some(next)) = tokio::time::timeout_at(until, watch.recv()).await {
                    add(next);
                }
                if !rescan && changed.is_empty() {
                    continue;
                }
                let service = service.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let _gate = service.gate.lock().expect("insights gate");
                    let mut db = service.open().map_err(|e| e.to_string())?;
                    let now = jiff::Timestamp::now().as_millisecond();
                    if rescan {
                        service.scan(&mut db, now).map_err(|e| e.to_string())?;
                        service
                            .caught_up
                            .store(true, std::sync::atomic::Ordering::Release);
                    } else {
                        service
                            .scan_changed(&mut db, now, Some(&changed))
                            .map_err(|e| e.to_string())?;
                    }
                    service.changed_index();
                    Ok::<_, String>(())
                })
                .await;
                if !matches!(result, Ok(Ok(()))) {
                    eprintln!("Couldn't fold Insights updates: {result:?}");
                }
            }
        })
    }
}
/// Reconcile independently of the source transcript. A file notification can
/// refine a completed portable step using only the saved metadata, not its words.
fn reconcile_native_step(
    db: &Connection,
    seq: &mut u64,
    now: i64,
    pricing: &Pricing,
    body: &Value,
    account_provider: &str,
    thread: Option<&str>,
    created: i64,
) -> Result<()> {
    let key = body["key"].as_str().ok_or("missing portable usage key")?;
    let provider = body["provider"].as_str().ok_or("missing usage provider")?;
    let at = body["at"].as_i64().ok_or("missing usage time")?;
    let mut active = BTreeSet::new();
    if let Some(thread) = thread {
        for (dedup, native) in native_index::candidates(
            db,
            account_provider,
            thread,
            created.saturating_sub(1000),
            at.saturating_add(1000),
        )? {
            let native_call = if provider == "antigravity" {
                native.call.as_deref().unwrap_or(&dedup)
            } else if provider == "claude" {
                native
                    .message
                    .as_deref()
                    .or(native.request.as_deref())
                    .unwrap_or(&dedup)
            } else {
                &dedup
            };
            let native_key = hash(&json!([provider, thread, native_call]));
            // A native observation may land in overlapping portable turns; the
            // first claimant owns it, including after a restart.
            let owner: Option<Option<String>> = db
                .query_row(
                    "SELECT supersedes FROM event WHERE key=?",
                    [&native_key],
                    |r| r.get(0),
                )
                .optional()?;
            if owner.is_some_and(|owner| owner.as_deref() != Some(key)) {
                continue;
            }
            active.insert(native_key.clone());
            let mut actual = body.clone();
            actual["key"] = json!(native_key);
            actual["call"] = json!(native_call);
            actual["supersedes"] = json!(key);
            actual["nativeSession"] = json!(thread);
            actual["at"] = json!(native.at);
            actual.as_object_mut().expect("object").remove("partial");
            if let Some(model) = &native.model {
                actual["model"] = json!(model);
            }
            if let Some(pricing_model) = &native.pricing_model {
                actual["pricingModels"] = if provider == "antigravity" {
                    json!([native.model, pricing_model])
                } else {
                    json!([pricing_model])
                };
            }
            let t = native.counts.price_tokens();
            actual["tokens"] = json!({"input":t.input,"output":t.output,"cacheRead":t.cache_read,"cacheWrite5m":t.cache_write_5m,"cacheWrite1h":t.cache_write_1h,"fast":t.fast});
            if let Some(total) = native.counts.total {
                actual["reportedTotal"] = json!(total);
            }
            if native.counts.ultrafast {
                actual["tokens"]["ultrafast"] = json!(true);
            }
            let cost = estimate_body(&actual, pricing, now);
            let count = t
                .input
                .saturating_add(t.output)
                .saturating_add(t.cache_write_5m)
                .saturating_add(t.cache_write_1h);
            save_event(
                db,
                seq,
                now,
                actual,
                cost,
                Some(count),
                Some(
                    native
                        .counts
                        .total
                        .unwrap_or(count.saturating_add(t.cache_read)),
                ),
                Some(t.input),
                Some(t.cache_read),
                Some(t.cache_write_5m.saturating_add(t.cache_write_1h)),
                Some(t.output),
            )?;
        }
    }
    let old = {
        let mut query =
            db.prepare_cached("SELECT key FROM event WHERE supersedes=? AND sender IS NULL")?;
        query
            .query_map([key], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for stale in old.into_iter().filter(|key| !active.contains(key)) {
        db.execute("DELETE FROM event WHERE key=? AND sender IS NULL", [&stale])?;
    }
    Ok(())
}

/// Keep the wire-compatible compact record and its SQL projection together.
#[allow(clippy::too_many_arguments)]
fn save_event(
    db: &Connection,
    seq: &mut u64,
    now: i64,
    body: Value,
    cost: Option<f64>,
    count: Option<u64>,
    processed: Option<u64>,
    input: Option<u64>,
    cached: Option<u64>,
    creation: Option<u64>,
    output: Option<u64>,
) -> Result<()> {
    let field = |name: &str| body[name].as_str();
    let key = field("key").ok_or("missing usage event key")?;
    let encoded = serde_json::to_string(&body)?;
    // Explicit absence belongs in the record; incomplete buckets belong in
    // the indexed aggregate flag. The wire retains the known/null buckets.
    let partial = body["human"] != true
        && (body["partial"] == true
            || [
                "input",
                "output",
                "cacheRead",
                "cacheWrite5m",
                "cacheWrite1h",
            ]
            .iter()
            .any(|field| body["tokens"][*field].as_u64().is_none()));
    let previous: Option<(String, Option<String>, Option<u64>)> = db
        .query_row(
            "SELECT body,sender,seq FROM event WHERE key=?",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if previous
        .as_ref()
        .is_some_and(|(body, sender, seq)| body == &encoded && sender.is_none() && seq.is_some())
    {
        return Ok(());
    }
    *seq += 1;
    db.execute("INSERT INTO event VALUES (?1,?2,NULL,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)
        ON CONFLICT(key) DO UPDATE SET seq=excluded.seq,sender=NULL,received=MIN(event.received,excluded.received),at=excluded.at,session=excluded.session,computer=excluded.computer,bot=excluded.bot,bot_name=excluded.bot_name,title=excluded.title,provider=excluded.provider,account=excluded.account,model=excluded.model,agent=excluded.agent,human=excluded.human,partial=excluded.partial,cost=excluded.cost,tokens=excluded.tokens,input=excluded.input,cache_read=excluded.cache_read,cache_write=excluded.cache_write,output=excluded.output,skills=excluded.skills,body=excluded.body,supersedes=excluded.supersedes,processed=excluded.processed
        WHERE (event.computer=excluded.computer OR (json_extract(excluded.body,'$.attributed')=1 AND COALESCE(json_extract(event.body,'$.attributed'),0)=0)) AND (event.partial=1 OR excluded.partial=0)",
        params![key,*seq,now,body["at"].as_i64(),field("session"),field("computer"),field("bot"),field("botName"),field("title"),field("provider"),field("account"),field("model"),field("agent"),body["human"].as_bool().unwrap_or(false),partial,cost,count,input,cached,creation,output,serde_json::to_string(body.get("skills").unwrap_or(&json!([])))?,encoded,field("supersedes"),processed])?;
    Ok(())
}
fn estimate_body(body: &Value, pricing: &Pricing, now: i64) -> Option<f64> {
    if body["human"] == true || body["partial"] == true {
        return None;
    }
    let t = &body["tokens"];
    let counts = super::native_logs::Counts {
        input: t["input"].as_u64()?,
        output: t["output"].as_u64()?,
        cache_read: t["cacheRead"].as_u64()?,
        cache_write_5m: t["cacheWrite5m"].as_u64()?,
        cache_write_1h: t["cacheWrite1h"].as_u64()?,
        fast: t["fast"] == true,
        ultrafast: t["ultrafast"] == true,
        ..Default::default()
    };
    let model = body["model"].as_str()?;
    let models = body["pricingModels"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_else(|| vec![model]);
    if body["provider"] == "codex" {
        return models
            .into_iter()
            .find_map(|m| super::codex_pricing::price(pricing, m, now, &counts, None).cost);
    }
    models.into_iter().find_map(|m| {
        pricing
            .resolve(m)
            .map(|rates| rates.cost(&counts.price_tokens(), body["provider"] != "cursor"))
    })
}
fn provider_name(provider: &str) -> &str {
    match provider.split('@').next().unwrap_or(provider) {
        "claude-code" => "claude",
        "codex-cli" => "codex",
        "grok-cli" => "grok",
        other => other,
    }
}
fn token_count(raw: &Value) -> Option<u64> {
    super::engine_readers::number(raw)
        .filter(|n| *n >= 0. && *n <= 9_007_199_254_740_991. && n.fract() == 0.)
        .map(|n| n as u64)
}
fn tokens(raw: &Value) -> Option<super::insight_mail::Buckets> {
    if !raw.is_object() {
        return None;
    }
    // The reference exposes unknown buckets as null. Known counts still
    // contribute to totals; a partial observation never estimates spend.
    let count = token_count;
    Some(super::insight_mail::Buckets {
        input: count(&raw["input"]),
        output: count(&raw["output"])
            .map(|n| n.saturating_add(count(&raw["reasoning"]).unwrap_or(0))),
        cache_read: count(&raw["cache"]["read"]),
        cache_write_5m: count(&raw["cache"]["write"]),
        cache_write_1h: Some(0),
        fast: false,
        ultrafast: false,
    })
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct Sums {
    cost: f64,
    tokens: u64,
    cached: u64,
    input: u64,
}
impl Sums {
    fn add(&mut self, other: &Self) {
        self.cost += other.cost;
        self.tokens = self.tokens.saturating_add(other.tokens);
        self.cached = self.cached.saturating_add(other.cached);
        self.input = self.input.saturating_add(other.input);
    }
    fn rate(&self) -> Option<f64> {
        (self.input != 0).then(|| (self.cached as f64 / self.input as f64).min(1.0))
    }
    fn measures(&self) -> Value {
        json!({"cost":(self.cost*1e6).round()/1e6,"tokens":self.tokens,"cacheRate":self.rate()})
    }
}
fn time_bounds(now: i64, zone: &TimeZone, period: i32) -> Result<Vec<(String, i64)>> {
    let today = Timestamp::from_millisecond(now)?
        .to_zoned(zone.clone())
        .date();
    let mut days = vec![];
    for days_ago in (0..period).rev() {
        let date = today.checked_sub(Span::new().days(days_ago))?;
        let start = date
            .at(0, 0, 0, 0)
            .to_zoned(zone.clone())?
            .timestamp()
            .as_millisecond();
        days.push((date.to_string(), start));
    }
    Ok(days)
}
struct Summary;
impl RouteSpec for Summary {
    type Body = ();
    const ROUTE: &'static Route = &Route {
        method: "GET",
        path: "kuvo/agents/api/kuvo/insights/summary",
        feature: "",
        answers: &[(200, &Shape::Primitive(Primitive::Any))],
        body: None,
    };
}
impl JsonStatus<200> for Summary {
    type Body = Value;
}
macro_rules! usage_route {
    ($name:ident,$method:literal,$path:literal,$body:ty,$shape:expr,$status:literal) => {
        struct $name;
        impl RouteSpec for $name {
            type Body = $body;
            const ROUTE: &'static Route = &Route {
                method: $method,
                path: $path,
                feature: "",
                answers: &[($status, &Shape::Primitive(Primitive::Any))],
                body: $shape,
            };
        }
        impl JsonStatus<$status> for $name {
            type Body = Value;
        }
    };
}
usage_route!(History, "GET", "kuvo/agents/api/kuvo/usage", (), None, 200);
usage_route!(
    HistoryRefresh,
    "POST",
    "kuvo/agents/api/kuvo/usage/refresh",
    (),
    None,
    200
);
usage_route!(
    HistoryPricing,
    "PUT",
    "kuvo/agents/api/kuvo/usage/pricing",
    Value,
    Some(&Shape::Primitive(Primitive::Any)),
    200
);
usage_route!(
    InsightsRefresh,
    "POST",
    "kuvo/agents/api/kuvo/insights/refresh",
    (),
    None,
    202
);
async fn read_history(
    service: Service,
    params: BTreeMap<String, String>,
) -> std::result::Result<Value, Refusal> {
    tokio::task::spawn_blocking(move || {
        service
            .history_view(params)
            .map_err(|e| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, format!("Usage: {e}")))
    })
    .await
    .map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}
fn register_history(routes: &mut Registry, service: Service) {
    let get = service.clone();
    routes.register::<History, _, _>(Access::Any, move |call| {
        let service = get.clone();
        let params = crate::artifacts::routes::parse_query(&call.request.path);
        async move {
            read_history(service, params)
                .await
                .map(RouteReply::json::<200>)
        }
    });
    let refresh = service.clone();
    routes.register::<HistoryRefresh, _, _>(Access::Any, move |call| {
        let service = refresh.clone();
        let params = crate::artifacts::routes::parse_query(&call.request.path);
        async move {
            let reader = service.clone();
            let task = tokio::spawn(async move {
                reader.refresh_usage().await;
            });
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
            service.request_peer_refresh();
            read_history(service, params)
                .await
                .map(RouteReply::json::<200>)
        }
    });
    let pricing = service.clone();
    routes.register::<HistoryPricing, _, _>(Access::Any, move |call| {
        let service = pricing.clone();
        let params = crate::artifacts::routes::parse_query(&call.request.path);
        async move {
            let model = call.body["codex"]
                .get("fallback")
                .filter(|v| v.is_null() || v.is_string())
                .ok_or_else(|| {
                    Refusal::new(
                        StatusCode::BAD_REQUEST,
                        "Say {codex: {fallback}}: a model, or null for none.",
                    )
                })?;
            let model = if model == "" {
                Value::Null
            } else {
                model.clone()
            };
            let Some(history) = service.history.as_ref() else {
                return Err(Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Usage settings aren't kept on this computer.",
                ));
            };
            let kept = history.settings.set_codex_fallback(model).await?;
            service.changed_index();
            let mut result = read_history(service, params).await?;
            if let Some(save) = kept["data"].get("save") {
                result["data"]["save"] = save.clone();
            }
            Ok(RouteReply::json::<200>(result))
        }
    });
    routes.register::<InsightsRefresh, _, _>(Access::Any, move |_| {
        let service = service.clone();
        async move {
            service.request_peer_refresh();
            tokio::spawn(async move {
                service.recount().await;
            });
            Ok(RouteReply::json::<202>(json!({"data":{"scanning":true}})))
        }
    });
}
pub fn register(routes: &mut Registry, service: Service) {
    register_views(routes, service.clone());
    register_history(routes, service.clone());
    routes.register::<Summary, _, _>(Access::Any, move |call| {
        let service = service.clone();
        let mut params = crate::artifacts::routes::parse_query(&call.request.path);
        async move {
            validate_period(
                &mut params,
                &[7, 30, 90],
                "Choose a 7, 30 or 90 day period.",
            )?;
            if params.get("refresh").is_some_and(|value| value == "1") {
                service.request_peer_refresh();
                let reader = service.clone();
                let task = tokio::spawn(async move {
                    reader.recount().await;
                });
                let _ = tokio::time::timeout(std::time::Duration::from_secs(60), task).await;
            }
            tokio::task::spawn_blocking(move || {
                service
                    .summary(params)
                    .map(RouteReply::json::<200>)
                    .map_err(|e| {
                        Refusal::new(StatusCode::SERVICE_UNAVAILABLE, format!("Insights: {e}"))
                    })
            })
            .await
            .map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        }
    });
}

// Overview and pages read the SQLite index with bounded SQL results instead of
// expanding every call in memory. Only recognised columns are interpolated.
#[derive(Debug, Clone, Serialize)]
struct Metrics {
    cost: Option<f64>,
    tokens: Option<u64>,
    cached: Option<u64>,
    input: Option<u64>,
    creation: Option<u64>,
    output: Option<u64>,
    messages: u64,
    calls: u64,
    processed: Option<u64>,
    unpriced: u64,
    partial: u64,
}
const METRICS: &str = "SUM(cost) AS cost,SUM(tokens) AS tokens,SUM(cache_read) AS cached,SUM(input) AS input,SUM(cache_write) AS creation,SUM(output) AS output,COALESCE(SUM(human),0) AS messages,COUNT(*) FILTER(WHERE human=0) AS calls,SUM(processed) AS processed,COUNT(*) FILTER(WHERE human=0 AND cost IS NULL) AS unpriced,COUNT(*) FILTER(WHERE human=0 AND partial=1) AS partial";
fn metrics(row: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<Metrics> {
    Ok(Metrics {
        cost: row.get(start)?,
        tokens: row.get(start + 1)?,
        cached: row.get(start + 2)?,
        input: row.get(start + 3)?,
        creation: row.get(start + 4)?,
        output: row.get(start + 5)?,
        messages: row.get(start + 6)?,
        calls: row.get(start + 7)?,
        processed: row.get(start + 8)?,
        unpriced: row.get(start + 9)?,
        partial: row.get(start + 10)?,
    })
}
fn share(row: &mut Value, totals: &Metrics) {
    row["costShare"] = json!(
        totals
            .cost
            .filter(|v| *v != 0.0)
            .zip(row["cost"].as_f64())
            .map(|(t, n)| n / t)
    );
    row["tokenShare"] = json!(
        totals
            .tokens
            .filter(|v| *v != 0)
            .zip(row["tokens"].as_u64())
            .map(|(t, n)| n as f64 / t as f64)
    );
}
struct Filter {
    clauses: String,
    values: Vec<rusqlite::types::Value>,
}
impl Filter {
    fn new(
        keys: &[String],
        selected: &[String],
        given: &BTreeMap<String, String>,
        from: i64,
        to: i64,
    ) -> Self {
        use rusqlite::types::Value as Sql;
        let mut clauses=vec!["event.at>=?1 AND event.at<?2".to_owned(),"event.computer IN (SELECT value FROM json_each(?4))".into(),"NOT EXISTS (SELECT 1 FROM event newer WHERE newer.supersedes=event.key AND newer.computer IN (SELECT value FROM json_each(?3)))".into()];
        let mut values = vec![
            Sql::Integer(from),
            Sql::Integer(to),
            Sql::Text(serde_json::to_string(keys).expect("computer keys")),
            Sql::Text(serde_json::to_string(selected).expect("selected keys")),
        ];
        for key in ["bot", "account", "provider", "session"] {
            if let Some(value) = given.get(key).filter(|v| !v.is_empty()) {
                values.push(Sql::Text(value.clone()));
                clauses.push(format!("event.{key}=?{}", values.len()));
            }
        }
        Self {
            clauses: clauses.join(" AND "),
            values,
        }
    }
    fn args(&self) -> impl rusqlite::Params + '_ {
        rusqlite::params_from_iter(self.values.iter())
    }
    fn range(&self, start: i64, end: i64) -> Self {
        let mut values = self.values.clone();
        values[0] = start.into();
        values[1] = end.into();
        Self {
            clauses: self.clauses.clone(),
            values,
        }
    }
}
enum View {
    Overview,
    Sessions,
    Detail,
}
impl Service {
    fn view(&self, params: BTreeMap<String, String>, view: View) -> Result<Value> {
        let _gate = self.gate.lock().expect("insights gate");
        let now = super::limits::now() as i64;
        let period = params
            .get("period")
            .map(|s| s.parse::<i32>())
            .transpose()?
            .unwrap_or(30);
        let offset = params
            .get("offset")
            .map(|s| s.parse::<u64>())
            .transpose()?
            .unwrap_or(0);
        if ![7, 30, 90, 365].contains(&period)
            || offset > 1_000_000
            || matches!(view, View::Detail) && !params.contains_key("session")
        {
            return Err("Choose a 7, 30, 90 or 365 day period and a valid page.".into());
        }
        let zone = params
            .get("zone")
            .and_then(|z| TimeZone::get(z).ok())
            .unwrap_or_else(TimeZone::system);
        let days = time_bounds(now, &zone, period)?;
        let mut db = self.open()?;
        self.ensure(&mut db, now)?;
        let scope = self.scope();
        let (keys, selected) = self.keys(&scope, params.get("computer").map(String::as_str));
        let filter = Filter::new(&keys, &selected, &params, days[0].1, now + 1);
        let mut local_computers = vec![self.own_key()];
        if self.alias() != self.own_key() {
            local_computers.push(self.alias());
        }
        let pending =
            self.pending_computers(&db, &scope, params.get("computer").map(String::as_str))?;
        let where_sql = &filter.clauses;
        match view {
            View::Detail => {
                let mut query=db.prepare(&format!("SELECT body,cost,tokens FROM event WHERE {where_sql} AND human=0 ORDER BY at,key LIMIT 101 OFFSET {offset}"))?;
                let rows = query.query_map(filter.args(), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<u64>>(2)?,
                    ))
                })?;
                let mut calls = vec![];
                for row in rows {
                    let (body, cost, count) = row?;
                    let mut body: Value = serde_json::from_str(&body)?;
                    body["cost"] = json!(cost);
                    body["tokenCount"] = json!(count);
                    calls.push(body);
                }
                let next = (calls.len() > 100).then_some(offset + 100);
                calls.truncate(100);
                let destination = self.conversation_destination(
                    params
                        .get("session")
                        .map(String::as_str)
                        .unwrap_or_default(),
                );
                return Ok(
                    json!({"data":{"calls":calls,"next":next,"destination":destination,"pendingComputers":pending,"localComputers":local_computers}}),
                );
            }
            View::Sessions => {
                let totals = db.query_row(
                    &format!("SELECT {METRICS} FROM event WHERE {where_sql}"),
                    filter.args(),
                    |r| metrics(r, 0),
                )?;
                let mut query=db.prepare(&format!("SELECT session,MAX(title),MAX(at),MAX(latest_computer),{METRICS} FROM (SELECT event.*,FIRST_VALUE(computer) OVER(PARTITION BY session ORDER BY at DESC,key) AS latest_computer FROM event WHERE {where_sql}) GROUP BY session ORDER BY at DESC,session LIMIT 51 OFFSET {offset}"))?;
                let rows = query.query_map(filter.args(), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        metrics(row, 4)?,
                    ))
                })?;
                let mut sessions = vec![];
                let homes = self.bot_homes();
                for row in rows {
                    let (id, title, at, computer, m) = row?;
                    let mut row = json!(m);
                    row["session"] = json!(id);
                    row["title"] = json!(title);
                    row["at"] = json!(at);
                    row["computer"] = json!(computer);
                    row["home"] = json!(homes.get(&id).unwrap_or(&computer));
                    share(&mut row, &totals);
                    sessions.push(row);
                }
                let next = (sessions.len() > 50).then_some(offset + 50);
                sessions.truncate(50);
                return Ok(
                    json!({"data":{"sessions":sessions,"next":next,"pendingComputers":pending,"localComputers":local_computers}}),
                );
            }
            View::Overview => {}
        }
        let (totals, sessions) = db.query_row(
            &format!("SELECT {METRICS},COUNT(DISTINCT session) FROM event WHERE {where_sql}"),
            filter.args(),
            |row| Ok((metrics(row, 0)?, row.get::<_, u64>(11)?)),
        )?;
        let mut daily = vec![];
        for (i, (name, start)) in days.iter().enumerate() {
            let range = filter.range(*start, days.get(i + 1).map_or(now + 1, |v| v.1));
            let facts = db.query_row(
                &format!("SELECT {METRICS} FROM event WHERE {where_sql}"),
                range.args(),
                |row| metrics(row, 0),
            )?;
            let mut value = json!(facts);
            value["name"] = json!(name);
            daily.push(value);
        }
        let grouped = |name: &str, extra: &str, group: &str| -> Result<Vec<Value>> {
            let mut query=db.prepare(&format!("SELECT {name} AS name,MIN(provider) AS provider,{METRICS} FROM event {extra} WHERE {where_sql} AND human=0 GROUP BY {group} ORDER BY COALESCE(cost,0) DESC,COALESCE(tokens,0) DESC,name LIMIT 2000"))?;
            let rows = query.query_map(filter.args(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    metrics(row, 2)?,
                ))
            })?;
            let mut values = vec![];
            for row in rows {
                let (name, provider, m) = row?;
                let mut v = json!(m);
                v["name"] = json!(name);
                v["provider"] = json!(provider);
                share(&mut v, &totals);
                values.push(v);
            }
            Ok(values)
        };
        let providers = grouped("provider", "", "1")?;
        let models = grouped("COALESCE(model,'Unknown model')", "", "1,provider")?;
        let agents = grouped(
            "COALESCE(MAX(bot_name),agent,provider)",
            "",
            "COALESCE(bot,agent,provider),CASE WHEN bot IS NULL THEN provider END",
        )?;
        let skills = grouped("skill.value", ",json_each(event.skills) AS skill", "1")?;
        let days_shared = daily
            .iter()
            .cloned()
            .map(|mut row| {
                share(&mut row, &totals);
                row
            })
            .collect::<Vec<_>>();
        let mut total = json!(totals);
        total["sessions"] = json!(sessions);
        let issues = self.issues.lock().expect("usage issues").clone();
        let counted = self.counted(&db)?;
        let mut current = vec![(self.own_key().to_owned(), self.name.clone())];
        if let Some(directory) = &self.directory {
            for member in &directory.view().roster.members {
                let key = kuvo_wire::keys::encode(&member.key);
                if let Some(name) = scope.peers.get(&key) {
                    current.push((key, name.clone()));
                }
            }
        } else {
            current.extend(
                scope
                    .peers
                    .iter()
                    .map(|(id, name)| (id.clone(), name.clone())),
            );
        }
        let chosen = params.get("computer").map(String::as_str);
        let requested_provider = params.get("provider").map(String::as_str);
        let mut coverage = vec![];
        for (id, name) in &current {
            if chosen.is_some_and(|key| key != id) {
                continue;
            }
            let (scanned, issues, stale) = if id == self.own_key() {
                (counted, issues.iter().cloned().collect::<Vec<_>>(), false)
            } else {
                let kept = get(&db, &format!("peer/{id}"))?.unwrap_or_default();
                let scanned = kept["scanned"].as_i64();
                let issues = kept["issues"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                // A change-driven index remains current while its peer is connected,
                // even with no new log lines to advance its counted timestamp.
                let away = self
                    .directory
                    .as_ref()
                    .and_then(|d| kuvo_wire::keys::decode_exact(id).map(|key| d.view().away(&key)))
                    .unwrap_or(true);
                (
                    scanned,
                    issues,
                    away && scanned.is_some_and(|at| now - at > 10 * 60_000),
                )
            };
            let issues = issues
                .into_iter()
                .filter(|issue| requested_provider.is_none_or(|provider| provider == issue))
                .collect::<Vec<_>>();
            let mut row = json!({"key":id,"name":name,"stale":stale,"issues":issues});
            if let Some(scanned) = scanned {
                row["scanned"] = json!(scanned);
            }
            coverage.push(row);
        }
        let computers = current
            .into_iter()
            .chain(
                scope
                    .historical
                    .iter()
                    .map(|(id, name)| (id.clone(), name.clone())),
            )
            .map(|(key, name)| json!({"key":key,"name":name}))
            .collect::<Vec<_>>();
        let bot_choices = {
            let mut query=db.prepare("SELECT bot,COALESCE(MAX(bot_name),bot) FROM event WHERE bot IS NOT NULL AND computer IN (SELECT value FROM json_each(?)) GROUP BY bot ORDER BY 2 COLLATE NOCASE,bot")?;
            let rows = query.query_map([serde_json::to_string(&keys)?], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut choices = vec![];
            for row in rows {
                let (key, name) = row?;
                choices.push(json!({"key":key,"name":name}));
            }
            choices
        };
        let state = if !self.ready() || !pending.is_empty() {
            "loading"
        } else if coverage.iter().any(|r| r["stale"] == true) {
            "stale"
        } else if totals.partial > 0
            || coverage.iter().any(|r| {
                !r["scanned"].is_number() || r["issues"].as_array().is_some_and(|a| !a.is_empty())
            })
        {
            "partial"
        } else if sessions == 0 {
            "empty"
        } else {
            "ready"
        };
        let unavailable = if providers
            .iter()
            .any(|r| r["name"] == "cursor" && r["partial"].as_u64().unwrap_or(0) > 0)
        {
            vec!["Cursor API call counts where the run only reports context size"]
        } else {
            vec![]
        };
        let updated_day = counted
            .and_then(|at| Timestamp::from_millisecond(at).ok())
            .map(|at| at.to_zoned(zone.clone()).date().to_string());
        Ok(
            json!({"data":{"totals":total,"daily":daily,"providers":providers,"breakdowns":{"model":models,"agent":agents,"skill":skills,"day":days_shared},"computers":computers,"bots":bot_choices,"coverage":coverage,"state":state,"zone":zone.iana_name().unwrap_or("UTC"),"period":period,"pendingComputers":pending,"updatedDay":updated_day,"scanning":!self.ready(),"localComputers":local_computers,"unavailable":unavailable}}),
        )
    }
}

macro_rules! route {
    ($name:ident,$path:literal) => {
        struct $name;
        impl RouteSpec for $name {
            type Body = ();
            const ROUTE: &'static Route = &Route {
                method: "GET",
                path: $path,
                feature: "",
                answers: &[(200, &Shape::Primitive(Primitive::Any))],
                body: None,
            };
        }
        impl JsonStatus<200> for $name {
            type Body = Value;
        }
    };
}
route!(Overview, "kuvo/agents/api/kuvo/insights");
route!(Sessions, "kuvo/agents/api/kuvo/insights/sessions");
route!(Detail, "kuvo/agents/api/kuvo/insights/sessions/:id");
pub fn register_views(routes: &mut Registry, service: Service) {
    let overview = service.clone();
    routes.register::<Overview, _, _>(Access::Any, move |call| {
        let service = overview.clone();
        let params = crate::artifacts::routes::parse_query(&call.request.path);
        async move {
            read_view(
                service,
                if params.contains_key("session") {
                    View::Detail
                } else {
                    View::Overview
                },
                params,
            )
            .await
            .map(RouteReply::json::<200>)
        }
    });
    let sessions = service.clone();
    routes.register::<Sessions, _, _>(Access::Any, move |call| {
        let service = sessions.clone();
        let params = crate::artifacts::routes::parse_query(&call.request.path);
        async move {
            read_view(service, View::Sessions, params)
                .await
                .map(RouteReply::json::<200>)
        }
    });
    routes.register::<Detail, _, _>(Access::Any, move |call| {
        let service = service.clone();
        let mut params = crate::artifacts::routes::parse_query(&call.request.path);
        if let Some(id) = call.param("id") {
            params.insert("session".into(), crate::artifacts::routes::decode_param(id));
        }
        async move {
            read_view(service, View::Detail, params)
                .await
                .map(RouteReply::json::<200>)
        }
    });
}
fn validate_period(
    params: &mut BTreeMap<String, String>,
    allowed: &[i32],
    message: &str,
) -> std::result::Result<(), Refusal> {
    let given = params.get("period").map(String::as_str).unwrap_or("30");
    let period = given
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| allowed.iter().any(|p| f64::from(*p) == *n))
        .map(|n| n as i32)
        .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, message))?;
    if params.contains_key("period") {
        params.insert("period".into(), period.to_string());
    }
    Ok(())
}
async fn read_view(
    service: Service,
    view: View,
    mut params: BTreeMap<String, String>,
) -> std::result::Result<Value, Refusal> {
    let message = "Choose a 7, 30, 90 or 365 day period and a valid page.";
    validate_period(&mut params, &[7, 30, 90, 365], message)?;
    if let Some(given) = params.get("offset") {
        let offset = if given.trim().is_empty() {
            Some(0.)
        } else {
            given.trim().parse::<f64>().ok()
        };
        let offset = offset
            .filter(|n| n.is_finite() && *n >= 0. && *n <= 1_000_000. && n.fract() == 0.)
            .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, message))?
            as u64;
        params.insert("offset".into(), offset.to_string());
    }
    if matches!(view, View::Detail) && params.get("session").is_none_or(String::is_empty) {
        return Err(Refusal::new(StatusCode::BAD_REQUEST, message));
    }
    tokio::task::spawn_blocking(move || {
        service
            .view(params, view)
            .map_err(|e| Refusal::new(StatusCode::SERVICE_UNAVAILABLE, format!("Insights: {e}")))
    })
    .await
    .map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

#[cfg(test)]
mod price_cache {
    use super::*;

    #[test]
    fn day_bounds_follow_civil_midnight_across_dst_and_fractional_offsets() {
        let epoch = |time: &str| time.parse::<Timestamp>().unwrap().as_millisecond();
        let zone = TimeZone::get("America/New_York").unwrap();
        let bounds = time_bounds(epoch("2026-03-09T12:00:00Z"), &zone, 7).unwrap();
        assert_eq!(
            bounds[5],
            ("2026-03-08".into(), epoch("2026-03-08T05:00:00Z"))
        );
        assert_eq!(
            bounds[6],
            ("2026-03-09".into(), epoch("2026-03-09T04:00:00Z"))
        );
        let fall = time_bounds(epoch("2026-11-02T12:00:00Z"), &zone, 7).unwrap();
        assert_eq!(
            fall[5],
            ("2026-11-01".into(), epoch("2026-11-01T04:00:00Z"))
        );
        assert_eq!(
            fall[6],
            ("2026-11-02".into(), epoch("2026-11-02T05:00:00Z"))
        );
        let nepal = time_bounds(
            epoch("2026-11-18T12:00:00Z"),
            &TimeZone::get("Asia/Kathmandu").unwrap(),
            7,
        )
        .unwrap();
        assert_eq!(
            nepal[6],
            ("2026-11-18".into(), epoch("2026-11-17T18:15:00Z"))
        );
    }

    #[test]
    fn a_price_book_is_shared_until_a_catalog_file_changes() {
        let scratch = tempfile::tempdir().unwrap();
        let service = Service::new(
            scratch.path().to_owned(),
            "Fixture".into(),
            "fixture".into(),
            "fixture",
        );
        let first = service.pricing().unwrap();
        assert!(Arc::ptr_eq(&first, &service.pricing().unwrap()));
        let folder = scratch.path().join("agents/usage/pricing");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("litellm.json"), br#"{"models":{}}"#).unwrap();
        let newer = service.pricing().unwrap();
        assert!(!Arc::ptr_eq(&first, &newer));
        assert!(Arc::ptr_eq(&newer, &service.pricing().unwrap()));
    }
}

#[cfg(test)]
mod lineage_repair_tests {
    use super::*;

    #[test]
    fn a_manual_engine_header_refresh_repairs_retained_calls_without_recounting_their_origin() {
        let temp = tempfile::tempdir().unwrap();
        let service = Service::new(
            temp.path().into(),
            "New computer".into(),
            "new-key".into(),
            "fixture",
        );
        let folder = service.home.join("data/opencode");
        std::fs::create_dir_all(&folder).unwrap();
        let maker = folder.join("opencode.db");
        let at = Timestamp::now().as_millisecond() - 60_000;
        let db = Connection::open(&maker).unwrap();
        db.execute_batch("CREATE TABLE session_v2(id TEXT PRIMARY KEY,title TEXT,parent_id TEXT,metadata TEXT);CREATE TABLE session_message(id TEXT,session_id TEXT,time_created INTEGER,time_updated INTEGER,type TEXT,data TEXT);").unwrap();
        for (id, parent, metadata) in [
            ("starter", None, json!({"kuvo":{"startedBy":"bot-chat"}})),
            ("work", None, json!({"kuvo":{"startedBy":"starter"}})),
            (
                "child",
                Some("work"),
                json!({"kuvo":{"by":{"bot":"other"}}}),
            ),
        ] {
            db.execute(
                "INSERT INTO session_v2 VALUES(?,?,?,?)",
                params![id, id, parent, metadata.to_string()],
            )
            .unwrap();
        }
        // The maker has an old copy with different counts. It is not a local
        // observation of the retained call and must not replace its origin.
        db.execute("INSERT INTO session_message VALUES(?,?,?,?,?,?)",params!["old","work",at,at,"assistant",json!({"model":{"providerID":"claude","id":"claude-opus-4-6"},"time":{"completed":at,"origin":"first"},"tokens":{"input":9999,"output":50,"cache":{"read":500,"write":2}}}).to_string()]).unwrap();
        drop(db);
        let bot = service.home.join("bots/bot_lead");
        std::fs::create_dir_all(&bot).unwrap();
        std::fs::write(
            bot.join("bot.json"),
            json!({"id":"bot_lead","kuvo":1,"name":"Lead"}).to_string(),
        )
        .unwrap();
        std::fs::write(bot.join("session"), "bot-chat").unwrap();
        service.fold_engine(vec![maker.clone()], false).unwrap();
        // The other computer's observations arrive after the maker channel's
        // first scan. The next explicit refresh must recover their lineage even
        // though that channel's database revision did not change.
        let db = service.open().unwrap();
        let mut seq = 0;
        for session in ["work", "child"] {
            let key = hash(&json!(["engine", session, "old"]));
            save_event(&db,&mut seq,at,json!({"key":key,"at":at,"session":session,"call":"old","computer":"first","provider":"claude","model":"claude-opus-4-6","attributed":true,"tokens":{"input":100,"output":50,"cacheRead":500,"cacheWrite5m":2,"cacheWrite1h":0}}),Some(0.123),Some(152),Some(652),Some(100),Some(500),Some(2),Some(50)).unwrap();
        }
        set(&db, "sequence", &json!(seq)).unwrap();
        drop(db);
        service.fold_engine(vec![maker.clone()], true).unwrap();
        let db = service.open().unwrap();
        for (session, root) in [("work", None), ("child", Some("work"))] {
            let (body, processed, cost): (String, u64, f64) = db
                .query_row(
                    "SELECT body,processed,cost FROM event WHERE session=?",
                    [session],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            let body: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["computer"], "first");
            assert_eq!(processed, 652);
            assert_eq!(cost, 0.123);
            assert_eq!(body["bot"], "bot_lead");
            assert_eq!(body["root"].as_str(), root);
        }
        let key = format!(
            "scan/rust-engine-v1/{}",
            hex::encode(Sha256::digest(maker.as_os_str().as_encoded_bytes()))
        );
        assert_eq!(get(&db, &format!("{key}/decoded")).unwrap(), Some(json!(0)));
    }
}
