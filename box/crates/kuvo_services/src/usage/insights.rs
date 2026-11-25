//! Insights' portable transcript index. The SQLite schema is the same as the old service;
//! only changed transcripts are decoded, on demand, never in an idle background process.
use super::{
    native_index,
    pricing::{Pricing, Tokens},
};
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
    path::PathBuf,
    sync::{Arc, Mutex},
};

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
    gate: Arc<Mutex<()>>,
    caught_up: Arc<std::sync::atomic::AtomicBool>,
    prices: Arc<Mutex<Option<PriceBook>>>,
}
impl Service {
    pub fn new(home: PathBuf, name: String, key: String, hostname: &str) -> Self {
        Self {
            home,
            name,
            key,
            hostname: hostname.to_owned(),
            alias: Arc::default(),
            gate: Arc::default(),
            caught_up: Arc::default(),
            prices: Arc::default(),
        }
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
                    hash(&json!([
                        "kuvo insight computer",
                        kuvo_os::usage_computer::identity(&self.hostname)
                    ]))[..32]
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
            if let Some(parent) = session["parentID"].as_str() {
                wanted.push(parent.to_owned());
            }
            headers.insert(id, (folder, session));
        }
        let tx = db.transaction()?;
        let mut seq = get(&tx, "sequence")?.and_then(|v| v.as_u64()).unwrap_or(0);
        for (id, (folder, session)) in &headers {
            if changed.is_some_and(|selected| !selected.contains(id)) {
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
            let mut lineage = id.as_str();
            let mut seen = BTreeSet::new();
            while seen.insert(lineage) {
                let Some(parent) = headers
                    .get(lineage)
                    .and_then(|(_, s)| s["parentID"].as_str())
                else {
                    break;
                };
                if !headers.contains_key(parent) || seen.contains(parent) {
                    break;
                }
                lineage = parent;
            }
            let top = &headers[lineage].1;
            let bot = top["metadata"]["kuvo"]["by"]["bot"].as_str();
            let bot_name = top["metadata"]["kuvo"]["by"]["name"].as_str();
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
                let cost = tokens
                    .as_ref()
                    .and_then(|t| pricing.resolve(model_id?).map(|r| r.cost(t, true)));
                let count = tokens
                    .as_ref()
                    .map(|t| t.input + t.output + t.cache_write_5m + t.cache_write_1h);
                let processed = count.zip(tokens.as_ref()).map(|(n, t)| n + t.cache_read);
                let input = tokens.as_ref().map(|t| t.input);
                let cached = tokens.as_ref().map(|t| t.cache_read);
                let creation = tokens.as_ref().map(|t| t.cache_write_5m + t.cache_write_1h);
                let output = tokens.as_ref().map(|t| t.output);
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
                    body["skills"] = json!([]);
                    if let Some(t) = tokens.as_ref() {
                        body["tokens"] = json!({"input":t.input,"output":t.output,"cacheRead":t.cache_read,"cacheWrite5m":t.cache_write_5m,"cacheWrite1h":t.cache_write_1h});
                    }
                    if partial {
                        body["partial"] = json!(true);
                    }
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
            &json!({"scanned":now,"issues":["native agent and engine history"]}),
        )?;
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
            while let Some(change) = changes.recv().await {
                let service = service.clone();
                let limits = limits.clone();
                let result = tokio::task::spawn_blocking(move || {
                    service
                        .fold_native(change, &limits)
                        .map_err(|e| e.to_string())
                })
                .await;
                if !matches!(result, Ok(Ok(()))) {
                    eprintln!("Couldn't fold native usage: {result:?}");
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
        // Enumerate only on startup/new root or after event loss, outside the query gate.
        let (root, files) = match change {
            native_index::Change::Scan(root) => {
                let paths = native_index::files(&root.folder, now - native_index::KEEP);
                (root, Some(paths))
            }
            native_index::Change::Files(root, changed) => {
                let paths =
                    native_index::changed_files(&root.folder, &changed, now - native_index::KEEP);
                (root, Some(paths))
            }
            native_index::Change::Removed(root) => (root, None),
        };
        let mut affected = BTreeSet::new();
        if let Some(files) = files {
            let mut db = self.open()?;
            for path in files {
                let _gate = self.gate.lock().expect("insights gate");
                match native_index::fold_file(&mut db, &self.home, &root, &path, now) {
                    Ok(sessions) => affected.extend(sessions),
                    Err(error) => eprintln!("Couldn't fold usage file {}: {error}", path.display()),
                }
            }
            let logged = native_index::recent_limits(&db, &root, now.max(0) as u64)?;
            if !logged.is_empty() {
                limits.observe_logged(&root.provider, logged, now.max(0) as u64);
            }
        } else {
            let _gate = self.gate.lock().expect("insights gate");
            let mut db = self.open()?;
            affected = native_index::remove_root(&mut db, &root)?;
        }
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
            let native_call = if provider == "claude" {
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
                actual["pricingModels"] = json!([pricing_model]);
            }
            let t = native.counts.price_tokens();
            actual["tokens"] = json!({"input":t.input,"output":t.output,"cacheRead":t.cache_read,"cacheWrite5m":t.cache_write_5m,"cacheWrite1h":t.cache_write_1h,"fast":t.fast});
            if let Some(total) = native.counts.total {
                actual["reportedTotal"] = json!(total);
            }
            let cost = native.billed.or_else(|| {
                pricing
                    .resolve(
                        native
                            .pricing_model
                            .as_deref()
                            .or(native.model.as_deref())?,
                    )
                    .map(|r| r.cost(&t, true))
            });
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
    let previous: Option<String> = db
        .query_row("SELECT body FROM event WHERE key=?", [key], |r| r.get(0))
        .optional()?;
    if previous.as_deref() == Some(&encoded) {
        return Ok(());
    }
    *seq += 1;
    db.execute("INSERT INTO event VALUES (?1,?2,NULL,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,'[]',?22,?23,?24)
        ON CONFLICT(key) DO UPDATE SET seq=excluded.seq,at=excluded.at,session=excluded.session,computer=excluded.computer,bot=excluded.bot,bot_name=excluded.bot_name,title=excluded.title,provider=excluded.provider,account=excluded.account,model=excluded.model,agent=excluded.agent,human=excluded.human,partial=excluded.partial,cost=excluded.cost,tokens=excluded.tokens,input=excluded.input,cache_read=excluded.cache_read,cache_write=excluded.cache_write,output=excluded.output,body=excluded.body,supersedes=excluded.supersedes,processed=excluded.processed
        WHERE event.sender IS NULL AND event.computer=excluded.computer AND (event.partial=1 OR excluded.partial=0)",
        params![key,*seq,now,body["at"].as_i64(),field("session"),field("computer"),field("bot"),field("botName"),field("title"),field("provider"),field("account"),field("model"),field("agent"),body["human"].as_bool().unwrap_or(false),body["partial"].as_bool().unwrap_or(false),cost,count,input,cached,creation,output,encoded,field("supersedes"),processed])?;
    Ok(())
}
fn provider_name(provider: &str) -> &str {
    match provider.split('@').next().unwrap_or(provider) {
        "claude-code" => "claude",
        "codex-cli" => "codex",
        "grok-cli" => "grok",
        other => other,
    }
}
fn tokens(raw: &Value) -> Option<Tokens> {
    Some(Tokens {
        input: raw["input"].as_u64()?,
        output: raw["output"]
            .as_u64()?
            .saturating_add(raw["reasoning"].as_u64().unwrap_or(0)),
        cache_read: raw["cache"]["read"].as_u64()?,
        cache_write_5m: raw["cache"]["write"].as_u64()?,
        cache_write_1h: 0,
        fast: false,
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
        self.tokens += other.tokens;
        self.cached += other.cached;
        self.input += other.input;
    }
    fn rate(&self) -> Option<f64> {
        (self.input != 0).then(|| (self.cached as f64 / self.input as f64).min(1.0))
    }
    fn measures(&self) -> Value {
        json!({"cost":(self.cost*1e6).round()/1e6,"tokens":self.tokens,"cacheRate":self.rate()})
    }
}
#[derive(Debug, Clone)]
struct Event {
    session: String,
    title: Option<String>,
    computer: String,
    provider: String,
    model: Option<String>,
    bot: Option<String>,
    bot_name: Option<String>,
    root: Option<String>,
    own: bool,
    at: i64,
    sums: Sums,
    human: bool,
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
fn filter_rows(
    db: &Connection,
    keys: &[&str],
    params: &BTreeMap<String, String>,
    start: i64,
    now: i64,
) -> Result<Vec<Event>> {
    let mut query=db.prepare("SELECT session,title,computer,provider,model,bot,bot_name,json_extract(body,'$.root'),COALESCE(json_extract(body,'$.botOwn'),0),at,COALESCE(cost,0),COALESCE(processed,0),COALESCE(cache_read,0),COALESCE(input,0)+COALESCE(cache_read,0)+COALESCE(cache_write,0),human,account
        FROM event WHERE at>=?1 AND at<=?2 AND computer IN (?3,?4) AND NOT EXISTS (SELECT 1 FROM event newer WHERE newer.supersedes=event.key AND newer.computer IN (?3,?4))")?;
    let mut rows = query.query(params![start, now, keys[0], keys[1]])?;
    let mut out = vec![];
    while let Some(row) = rows.next()? {
        let event = Event {
            session: row.get(0)?,
            title: row.get(1)?,
            computer: row.get(2)?,
            provider: row.get(3)?,
            model: row.get(4)?,
            bot: row.get(5)?,
            bot_name: row.get(6)?,
            root: row.get(7)?,
            own: row.get::<_, i64>(8)? != 0,
            at: row.get(9)?,
            sums: Sums {
                cost: row.get(10)?,
                tokens: row.get(11)?,
                cached: row.get(12)?,
                input: row.get(13)?,
            },
            human: row.get::<_, i64>(14)? != 0,
        };
        let account: Option<String> = row.get(15)?;
        if params
            .get("computer")
            .is_some_and(|c| !keys.contains(&c.as_str()))
            || params.get("provider").is_some_and(|p| p != &event.provider)
            || params
                .get("account")
                .is_some_and(|a| account.as_ref() != Some(a))
            || params
                .get("bot")
                .is_some_and(|b| event.bot.as_ref() != Some(b))
            || params
                .get("session")
                .is_some_and(|s| s != &event.session && event.root.as_ref() != Some(s))
            || params
                .get("model")
                .is_some_and(|m| event.model.as_ref() != Some(m))
        {
            continue;
        }
        out.push(event);
    }
    Ok(out)
}
fn sum<'a>(rows: impl IntoIterator<Item = &'a Event>) -> Sums {
    let mut sums = Sums::default();
    for row in rows {
        sums.add(&row.sums)
    }
    sums
}
fn ranked<'a>(
    rows: &'a [Event],
    group: impl Fn(&'a Event) -> (String, String, Option<String>),
    kind: &str,
) -> Vec<Value> {
    let mut groups = BTreeMap::<String, (String, Option<String>, Sums)>::new();
    for row in rows.iter().filter(|r| !r.human) {
        let (id, name, provider) = group(row);
        let entry = groups
            .entry(id)
            .or_insert_with(|| (name, provider, Sums::default()));
        entry.2.add(&row.sums);
    }
    let mut rows: Vec<_> = groups
        .into_iter()
        .map(|(id, (name, provider, sums))| (id, name, provider, sums))
        .collect();
    rows.sort_by(|a, b| {
        b.3.cost
            .total_cmp(&a.3.cost)
            .then(b.3.tokens.cmp(&a.3.tokens))
            .then(a.0.cmp(&b.0))
    });
    rows.into_iter()
        .take(200)
        .map(|(id, name, provider, sums)| {
            let mut row = sums.measures();
            row["kind"] = json!(kind);
            row["id"] = json!(id);
            row["name"] = json!(name);
            if let Some(p) = provider {
                row["provider"] = json!(p);
            }
            row
        })
        .collect()
}
impl Service {
    fn summary(&self, params: BTreeMap<String, String>) -> Result<Value> {
        let _gate = self.gate.lock().expect("insights gate");
        let now = super::limits::now() as i64;
        let period = params
            .get("period")
            .map(|s| s.parse::<i32>())
            .transpose()?
            .unwrap_or(30);
        if ![7, 30, 90].contains(&period) {
            return Err("Choose a 7, 30 or 90 day period.".into());
        }
        let zone = params
            .get("zone")
            .and_then(|z| TimeZone::get(z).ok())
            .unwrap_or_else(TimeZone::system);
        let bounds = time_bounds(now, &zone, period * 2)?;
        let mut db = self.open()?;
        self.ensure(&mut db, now)?;
        let keys = [self.own_key(), self.alias()];
        let records = filter_rows(&db, &keys, &params, bounds[0].1, now)?;
        let start = bounds[period as usize].1;
        let prior = sum(records.iter().filter(|r| r.at < start));
        let current: Vec<_> = records.iter().filter(|r| r.at >= start).cloned().collect();
        let total = sum(&current);
        let trend = bounds[period as usize..]
            .iter()
            .enumerate()
            .map(|(i, (day, start))| {
                let end = bounds.get(i + period as usize + 1).map_or(now + 1, |v| v.1);
                let sums = sum(current.iter().filter(|r| r.at >= *start && r.at < end));
                json!({"day":day,"cost":sums.cost,"tokens":sums.tokens})
            })
            .collect::<Vec<_>>();
        let change=records.iter().any(|r|r.at<start&&!r.human).then(||json!({"cost":(prior.cost>0.0).then(||(total.cost-prior.cost)/prior.cost),"tokens":(prior.tokens>0).then(||(total.tokens as f64-prior.tokens as f64)/prior.tokens as f64),"cacheRate":total.rate().zip(prior.rate()).map(|(a,b)|a-b)}));
        let sessions = if params.contains_key("session") {
            ranked(
                &current,
                |r| {
                    (
                        r.session.clone(),
                        r.title.clone().unwrap_or_else(|| "Untitled session".into()),
                        None,
                    )
                },
                "session",
            )
        } else if params.contains_key("bot") {
            ranked(
                &current,
                |r| {
                    (
                        r.root.clone().unwrap_or_else(|| r.session.clone()),
                        r.title.clone().unwrap_or_else(|| "Untitled session".into()),
                        None,
                    )
                },
                "session",
            )
        } else {
            let mut by_bot = BTreeMap::<String, Vec<Event>>::new();
            let mut loose = vec![];
            for row in &current {
                if let Some(bot) = &row.bot {
                    by_bot.entry(bot.clone()).or_default().push(row.clone());
                } else {
                    loose.push(row.clone());
                }
            }
            let mut rows = ranked(
                &loose,
                |r| {
                    (
                        r.root.clone().unwrap_or_else(|| r.session.clone()),
                        r.title.clone().unwrap_or_else(|| "Untitled session".into()),
                        None,
                    )
                },
                "session",
            );
            for row in &mut rows {
                row["note"] = json!("No bot");
            }
            for (id, records) in by_bot {
                let mut children = ranked(
                    &records,
                    |r| {
                        (
                            r.root.clone().unwrap_or_else(|| r.session.clone()),
                            r.title.clone().unwrap_or_else(|| "Untitled session".into()),
                            None,
                        )
                    },
                    "session",
                );
                for child in &mut children {
                    if records
                        .iter()
                        .any(|record| record.own && child["id"] == record.session)
                    {
                        child["note"] = json!("Own chat");
                    }
                }
                let mut bot = sum(&records).measures();
                bot["kind"] = json!("bot");
                bot["id"] = json!(id);
                bot["name"] = json!(
                    records
                        .iter()
                        .find_map(|r| r.bot_name.as_deref())
                        .unwrap_or("A bot")
                );
                bot["sessions"] = json!(children);
                rows.push(bot);
            }
            rows.sort_by(|a, b| {
                b["cost"]
                    .as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&a["cost"].as_f64().unwrap_or(0.0))
                    .then(b["tokens"].as_u64().cmp(&a["tokens"].as_u64()))
            });
            rows
        };
        let providers = ranked(
            &current,
            |r| {
                (
                    r.provider.clone(),
                    r.provider.clone(),
                    Some(r.provider.clone()),
                )
            },
            "provider",
        );
        let models = ranked(
            &current,
            |r| {
                let model = r.model.clone().unwrap_or_else(|| "Unknown model".into());
                (model.clone(), model, Some(r.provider.clone()))
            },
            "model",
        );
        let computers = ranked(
            &current,
            |r| (r.computer.clone(), self.name.clone(), None),
            "computer",
        );
        let (first_key, first_name) =
            if params.contains_key("session") || params.contains_key("bot") {
                ("sessions", "Sessions")
            } else {
                ("bots", "Bots")
            };
        // This reader cannot claim a complete summary until native and engine history
        // sources are indexed. Existing local portable evidence is still queryable.
        Ok(
            json!({"data":{"state":"loading","period":period,"zone":zone.iana_name().unwrap_or("UTC"),"pendingComputers":[],"totals":total.measures(),"change":change,"trend":trend,"lists":[{"key":first_key,"name":first_name,"rows":sessions},{"key":"providers","name":"Providers","rows":providers},{"key":"models","name":"Models","rows":models},{"key":"computers","name":"Computers","rows":computers}],"standouts":[]}}),
        )
    }
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
pub fn register(routes: &mut Registry, service: Service) {
    register_views(routes, service.clone());
    routes.register::<Summary, _, _>(Access::Any, move |call| {
        let service = service.clone();
        let params = crate::artifacts::routes::parse_query(&call.request.path);
        async move {
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
const METRICS: &str = "SUM(cost),SUM(tokens),SUM(cache_read),SUM(input),SUM(cache_write),SUM(output),COALESCE(SUM(human),0),COUNT(*) FILTER(WHERE human=0),SUM(processed),COUNT(*) FILTER(WHERE human=0 AND cost IS NULL),COUNT(*) FILTER(WHERE human=0 AND partial=1)";
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
    fn new(keys: [&str; 2], given: &BTreeMap<String, String>, from: i64, to: i64) -> Self {
        use rusqlite::types::Value as Sql;
        let mut clauses=vec!["event.at>=?1 AND event.at<?2".to_owned(),"event.computer IN (?3,?4)".into(),"NOT EXISTS (SELECT 1 FROM event newer WHERE newer.supersedes=event.key AND newer.computer IN (?3,?4))".into()];
        let mut values = vec![
            Sql::Integer(from),
            Sql::Integer(to),
            Sql::Text(keys[0].to_owned()),
            Sql::Text(keys[1].to_owned()),
        ];
        // The local alias can also be used by the records made before a link existed.
        if given
            .get("computer")
            .is_some_and(|key| !keys.contains(&key.as_str()))
        {
            clauses.push("0".into());
        }
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
        let keys = [self.own_key(), self.alias()];
        let filter = Filter::new(keys, &params, days[0].1, now + 1);
        let local_computers = if keys[0] == keys[1] {
            vec![keys[0]]
        } else {
            keys.to_vec()
        };
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
                return Ok(
                    json!({"data":{"calls":calls,"next":next,"destination":{"status":"unavailable"},"pendingComputers":[],"localComputers":local_computers}}),
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
                for row in rows {
                    let (id, title, at, computer, m) = row?;
                    let mut row = json!(m);
                    row["session"] = json!(id);
                    row["title"] = json!(title);
                    row["at"] = json!(at);
                    row["computer"] = json!(computer);
                    row["home"] = json!(computer);
                    share(&mut row, &totals);
                    sessions.push(row);
                }
                let next = (sessions.len() > 50).then_some(offset + 50);
                sessions.truncate(50);
                return Ok(
                    json!({"data":{"sessions":sessions,"next":next,"pendingComputers":[],"localComputers":local_computers}}),
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
            let mut query=db.prepare(&format!("SELECT {name} AS name,MIN(provider),{METRICS} FROM event {extra} WHERE {where_sql} AND human=0 GROUP BY {group} ORDER BY COALESCE(SUM(cost),0) DESC,COALESCE(SUM(tokens),0) DESC,name LIMIT 2000"))?;
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
        let coverage = json!([{"key":self.own_key(),"name":self.name,"stale":false,"issues":["native agent and engine history"]}]);
        Ok(
            json!({"data":{"totals":total,"daily":daily,"providers":providers,"breakdowns":{"model":models,"agent":agents,"skill":skills,"day":days_shared},"computers":[{"key":self.own_key(),"name":self.name}],"bots":[],"coverage":coverage,"state":"partial","zone":zone.iana_name().unwrap_or("UTC"),"period":period,"pendingComputers":[],"updatedDay":null,"scanning":false,"localComputers":local_computers,"unavailable":[]}}),
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
async fn read_view(
    service: Service,
    view: View,
    params: BTreeMap<String, String>,
) -> std::result::Result<Value, Refusal> {
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
