//! Per-file native log checkpoints and compact observations; the data belongs
//! to the same SQLite index, not a second process or a JSON cache for each log.
use super::native_logs::{self, Cursor, Kind, Observation};
use kuvo_agents::accounts::{Accounts, AgentKind};
use kuvo_os::{
    platform::{Account, Environment, Platform, env_value},
    watch::{Notice, Options, WatchHub},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const DAY: i64 = 86_400_000;
pub const KEEP: i64 = 366 * DAY;
const MAX_FILES: usize = 100_000;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Root {
    pub folder: PathBuf,
    pub provider: String,
    pub kind: Kind,
    pub managed: bool,
    pub label: Option<String>,
    /// Provider-declared root order (live sessions before archives).
    pub order: u32,
    /// The sign-in file belongs to the Codex home, not necessarily the log root.
    auth: Option<PathBuf>,
    flat: bool,
}
fn key(path: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(path.as_os_str().as_encoded_bytes());
    hex::encode(hash.finalize())
}
fn hash(parts: &serde_json::Value) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(parts).expect("usage metadata"),
    ))
}
fn expand_path(personal: &Path, path: &str) -> PathBuf {
    path.strip_prefix("~/")
        .map(|tail| personal.join(tail))
        .unwrap_or_else(|| PathBuf::from(path))
}
fn own_path(env: &Environment, personal: &Path, name: &str, otherwise: &str) -> PathBuf {
    env_value(Platform::current(), env, name)
        .and_then(OsStr::to_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|path| expand_path(personal, path))
        .unwrap_or_else(|| personal.join(otherwise))
}
pub async fn roots(home: &Path) -> std::io::Result<Vec<Root>> {
    let env: Environment = std::env::vars_os().collect();
    let personal = Account::current()?.home_folder(Platform::current(), &env)?;
    let accounts = Accounts::for_home(&kuvo_store::home::Home::new(home.to_owned())?);
    let mut roots = vec![];
    for account in accounts.all().await? {
        let provider = account.provider().to_string();
        let managed = accounts.home(&account);
        let own = managed.is_none();
        let paths = match account.kind {
            AgentKind::Claude => {
                let folders = if let Some(managed) = managed {
                    vec![managed]
                } else if let Some(configured) =
                    env_value(Platform::current(), &env, "CLAUDE_CONFIG_DIR")
                        .and_then(OsStr::to_str)
                        .filter(|s| !s.trim().is_empty())
                {
                    configured
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| expand_path(&personal, s))
                        .collect()
                } else {
                    vec![
                        own_path(&env, &personal, "XDG_CONFIG_HOME", ".config").join("claude"),
                        personal.join(".claude"),
                    ]
                };
                folders
                    .into_iter()
                    .map(|folder| {
                        let path = if folder.file_name().is_some_and(|name| name == "projects") {
                            folder
                        } else {
                            folder.join("projects")
                        };
                        (Kind::Claude, path, None, false)
                    })
                    .collect()
            }
            AgentKind::Codex => {
                let homes = if let Some(managed) = managed {
                    vec![managed]
                } else if let Some(configured) = env_value(Platform::current(), &env, "CODEX_HOME")
                    .and_then(OsStr::to_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    configured
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|part| expand_path(&personal, part))
                        .collect()
                } else {
                    vec![personal.join(".codex")]
                };
                let mut seen = BTreeSet::new();
                let mut paths = vec![];
                for folder in homes {
                    let real = std::fs::canonicalize(&folder).unwrap_or_else(|_| folder.clone());
                    if !seen.insert(real) {
                        continue;
                    }
                    let sessions = folder.join("sessions");
                    let archive = folder.join("archived_sessions");
                    let auth = folder.join("auth.json");
                    if sessions.is_dir() || archive.is_dir() {
                        paths.push((Kind::Codex, sessions, Some(auth.clone()), false));
                        paths.push((Kind::Codex, archive, Some(auth), false));
                    } else {
                        paths.push((Kind::Codex, folder, Some(auth), true));
                    }
                }
                paths
            }
            AgentKind::Grok => vec![(
                Kind::Grok,
                super::grok_native::folder(managed, &env, &personal).join("sessions"),
                None,
                false,
            )],
            AgentKind::Antigravity => vec![(
                Kind::Antigravity,
                managed
                    .unwrap_or_else(|| home.join("agents/antigravity"))
                    .join("profile"),
                None,
                false,
            )],
            _ => vec![],
        };
        roots.extend(
            paths
                .into_iter()
                .enumerate()
                .map(|(order, (kind, folder, auth, flat))| Root {
                    managed: folder.starts_with(home)
                        && (!own || folder.starts_with(home.join("agents"))),
                    folder,
                    kind,
                    provider: provider.clone(),
                    label: account.label.clone(),
                    order: order.min(u32::MAX as usize) as u32,
                    auth,
                    flat,
                }),
        );
    }
    // Shared native roots belong to their actual owner, not each linked
    // account that happens to name the same directory.
    let mut owners = BTreeMap::<(Kind, PathBuf), String>::new();
    let real =
        |root: &Root| std::fs::canonicalize(&root.folder).unwrap_or_else(|_| root.folder.clone());
    for root in &roots {
        if root.kind == Kind::Codex
            && !std::fs::symlink_metadata(&root.folder).is_ok_and(|m| m.file_type().is_symlink())
        {
            owners
                .entry((root.kind, real(root)))
                .or_insert_with(|| root.provider.clone());
        }
    }
    for root in &roots {
        if matches!(root.kind, Kind::Codex | Kind::Grok) {
            owners
                .entry((root.kind, real(root)))
                .or_insert_with(|| root.provider.clone());
        }
    }
    roots.retain(|root| {
        owners
            .get(&(root.kind, real(root)))
            .is_none_or(|owner| owner == &root.provider)
    });
    for root in &mut roots {
        if let Ok(folder) = std::fs::canonicalize(&root.folder) {
            root.folder = folder;
        }
    }
    let mut seen = BTreeSet::new();
    roots.retain(|root| seen.insert((root.provider.clone(), root.folder.clone())));
    Ok(roots)
}
pub fn schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS native_root(root TEXT NOT NULL, provider TEXT NOT NULL, kind TEXT NOT NULL, label TEXT, scanned INTEGER, place TEXT NOT NULL, PRIMARY KEY(root,provider));
         CREATE TABLE IF NOT EXISTS native_file(source TEXT PRIMARY KEY, root TEXT NOT NULL, provider TEXT NOT NULL, kind TEXT NOT NULL, managed INTEGER NOT NULL, label TEXT, cursor TEXT NOT NULL, scanned INTEGER NOT NULL,source_order TEXT NOT NULL DEFAULT '');
         CREATE INDEX IF NOT EXISTS native_file_root ON native_file(root);
         CREATE TABLE IF NOT EXISTS native_limit(source TEXT NOT NULL, provider TEXT NOT NULL, window TEXT NOT NULL, read INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(source,window,read));
         CREATE INDEX IF NOT EXISTS native_limit_provider ON native_limit(provider,read);
         CREATE TABLE IF NOT EXISTS native_call(source TEXT NOT NULL, position INTEGER NOT NULL, ordinal INTEGER NOT NULL,
           provider TEXT NOT NULL, kind TEXT NOT NULL, managed INTEGER NOT NULL, dedup TEXT NOT NULL, message TEXT, sidechain INTEGER NOT NULL, processed INTEGER NOT NULL, speed INTEGER NOT NULL,
           native_session TEXT, at INTEGER NOT NULL, body TEXT NOT NULL, revision INTEGER NOT NULL, PRIMARY KEY(source,position,ordinal));
         CREATE INDEX IF NOT EXISTS native_call_provider_session ON native_call(provider,native_session,at);
         CREATE INDEX IF NOT EXISTS native_call_dedup ON native_call(provider,dedup);
         CREATE INDEX IF NOT EXISTS native_call_message ON native_call(provider,message);
         CREATE INDEX IF NOT EXISTS native_call_at ON native_call(at);
         CREATE TABLE IF NOT EXISTS native_thread(provider TEXT NOT NULL,native_session TEXT NOT NULL,session TEXT NOT NULL,PRIMARY KEY(provider,native_session,session));
         CREATE INDEX IF NOT EXISTS native_thread_session ON native_thread(session);
         CREATE TABLE IF NOT EXISTS native_step(key TEXT PRIMARY KEY, session TEXT NOT NULL, provider TEXT NOT NULL, thread TEXT NOT NULL, created INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS native_step_session ON native_step(session);
         CREATE TABLE IF NOT EXISTS native_sequence(id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
         INSERT OR IGNORE INTO native_sequence VALUES(1,0);"
    )?;
    let has: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('native_file') WHERE name='source_order')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        db.execute_batch(
            "ALTER TABLE native_file ADD COLUMN source_order TEXT NOT NULL DEFAULT '';",
        )?;
    }
    Ok(())
}
fn path_age(path: &Path) -> Option<i64> {
    std::fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}
fn jsonl(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str) == Some("jsonl")
}
fn maker_file(path: &Path, kind: Kind) -> bool {
    if kind != Kind::Antigravity {
        return jsonl(path);
    }
    path.extension().and_then(OsStr::to_str) == Some("db")
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "conversations")
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with("antigravity"))
}
fn maker_age(path: &Path, kind: Kind) -> Option<i64> {
    let base = path_age(path);
    if kind != Kind::Antigravity {
        return base;
    }
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    base.max(path_age(Path::new(&wal)))
}
/// The first catch-up and an event-loss recovery walk names/mtime only. Every
/// regular file is opened at most when its stored cursor says it changed.
pub fn files(root: &Path, kind: Kind, since: i64) -> Vec<PathBuf> {
    let mut pending = vec![root.to_owned()];
    let mut result = vec![];
    let mut walked = 0;
    while let Some(folder) = pending.pop() {
        if !std::fs::symlink_metadata(&folder)
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            walked += 1;
            if walked > MAX_FILES * 4 {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file()
                && maker_file(&entry.path(), kind)
                && maker_age(&entry.path(), kind).is_some_and(|at| at >= since)
            {
                result.push(entry.path());
            }
        }
        if walked > MAX_FILES * 4 {
            break;
        }
    }
    if result.len() > MAX_FILES {
        result.sort_by_key(|p| std::cmp::Reverse(path_age(p)));
        result.truncate(MAX_FILES);
    }
    result.sort();
    result
}
pub fn changed_files(root: &Path, kind: Kind, changed: &[PathBuf], since: i64) -> Vec<PathBuf> {
    let mut result = BTreeSet::new();
    for relative in changed {
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let path = root.join(relative);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            result.extend(files(&path, kind, since));
        } else {
            let path = if kind == Kind::Antigravity {
                path.file_name()
                    .and_then(OsStr::to_str)
                    .and_then(|name| name.strip_suffix("-wal"))
                    .map(|name| path.with_file_name(name))
                    .unwrap_or(path)
            } else {
                path
            };
            if maker_file(&path, kind) {
                result.insert(path);
            }
        }
    }
    result.into_iter().collect()
}
fn affected(db: &Connection, source: &str) -> Result<BTreeSet<String>> {
    let mut query=db.prepare("SELECT DISTINCT t.session FROM native_call n JOIN native_thread t ON t.provider=n.provider AND t.native_session=n.native_session WHERE n.source=?")?;
    let rows = query.query_map([source], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
pub fn fold_file(
    db: &mut Connection,
    home: &Path,
    root: &Root,
    path: &Path,
    now: i64,
    pricing: &super::pricing::Pricing,
    fallback: Option<&str>,
) -> Result<BTreeSet<String>> {
    let source = hash(&json!([key(path), root.provider]));
    let root_key = key(&root.folder);
    let before: Option<(String, String)> = db
        .query_row(
            "SELECT cursor,source_order FROM native_file WHERE source=?",
            [&source],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let cursor = before
        .as_ref()
        .and_then(|(s, _)| serde_json::from_str::<Cursor>(s).ok())
        .unwrap_or_default();
    let relative = path.strip_prefix(&root.folder).unwrap_or(path);
    let archive = if root.flat
        && relative
            .components()
            .next()
            .is_some_and(|c| c.as_os_str() == "archived_sessions")
    {
        1
    } else {
        0
    };
    let source_order = if root.kind == Kind::Codex {
        // Codex declares its home order, live logs before archived copies.
        format!(
            "{:08x}/{archive}/{}",
            root.order,
            relative.to_string_lossy()
        )
    } else {
        // Claude and Grok traverse the combined roots in absolute file order.
        path.to_string_lossy().into_owned()
    };
    let mut reordered = BTreeSet::new();
    if before
        .as_ref()
        .is_some_and(|(_, order)| order != &source_order)
    {
        reordered = affected(db, &source)?;
        db.execute(
            "UPDATE native_file SET source_order=? WHERE source=?",
            params![source_order, source],
        )?;
    }
    if !path.exists() {
        let sessions = affected(db, &source)?;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM native_call WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_price WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_limit WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_file WHERE source=?", [&source])?;
        tx.commit()?;
        return Ok(sessions);
    }
    if maker_age(path, root.kind).is_some_and(|at| at < now - KEEP) {
        return Ok(BTreeSet::new());
    }
    if root.kind == Kind::Grok && path.file_name().is_none_or(|name| name != "updates.jsonl") {
        return Ok(BTreeSet::new());
    }
    // File notifications may refer to an ancestor symlink. A reader never leaves its root.
    if !std::fs::canonicalize(path)
        .ok()
        .zip(std::fs::canonicalize(&root.folder).ok())
        .is_some_and(|(file, folder)| file.starts_with(folder))
    {
        return Ok(BTreeSet::new());
    }
    let managed = root.managed
        && std::fs::canonicalize(path)
            .ok()
            .zip(std::fs::canonicalize(home).ok())
            .is_some_and(|(path, home)| path.starts_with(home));
    let (cursor_body, rewritten, events, limits) = if root.kind == Kind::Antigravity {
        let Some(stamp) = super::engine_index::stamp(path)? else {
            return Ok(BTreeSet::new());
        };
        if before
            .as_ref()
            .and_then(|(s, _)| serde_json::from_str::<super::engine_index::Stamp>(s).ok())
            .as_ref()
            == Some(&stamp)
        {
            return Ok(reordered);
        }
        let Some(session) = super::antigravity_history::database_session(path) else {
            return Ok(BTreeSet::new());
        };
        let Some(owned) = super::engine_index::owned(&root.folder, path)? else {
            return Ok(BTreeSet::new());
        };
        let db = super::engine_index::open_maker(&owned)?;
        let mut events = vec![];
        super::antigravity_history::read(&db, session, now - KEEP, |index, event| {
            events.push((index, event));
            Ok(())
        })?;
        (serde_json::to_string(&stamp)?, true, events, vec![])
    } else {
        let Some(changes) = native_logs::read(path, root.kind, &cursor, now - KEEP)? else {
            return Ok(reordered);
        };
        (
            serde_json::to_string(&changes.cursor)?,
            changes.rewritten,
            changes.events,
            changes.limits,
        )
    };
    let mut sessions = reordered;
    if rewritten {
        sessions.extend(affected(db, &source)?);
    }
    let tx = db.transaction()?;
    tx.execute(
        "UPDATE native_sequence SET revision=revision+1 WHERE id=1",
        [],
    )?;
    let revision: u64 =
        tx.query_row("SELECT revision FROM native_sequence WHERE id=1", [], |r| {
            r.get(0)
        })?;
    if rewritten {
        tx.execute("DELETE FROM native_call WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_price WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_limit WHERE source=?", [&source])?;
    }
    let mut ordinal = 0u64;
    let mut last_position = None;
    for (position, mut event) in events {
        if root.kind == Kind::Grok && event.session.is_none() {
            event.session = path
                .parent()
                .and_then(Path::file_name)
                .and_then(OsStr::to_str)
                .map(str::to_owned);
        }
        if last_position != Some(position) {
            ordinal = 0;
            last_position = Some(position);
        } else {
            ordinal += 1;
        }
        let dedup = event
            .dedup_key(root.kind)
            .unwrap_or_else(|| hash(&json!([source, position, ordinal])));
        let body = serde_json::to_string(&event)?;
        let processed = event.counts.processed().min(i64::MAX as u64);
        tx.execute("INSERT OR REPLACE INTO native_call VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![source,position,ordinal,root.provider,root.kind.name(),managed,dedup,event.message,event.sidechain,processed,event.has_speed,event.session,event.at,body,revision])?;
        if event.at >= now - 35 * DAY {
            super::history::put(
                &tx, &source, position, ordinal, &event, root.kind, pricing, fallback,
            )?;
        }
        if managed && let Some(session) = event.session.as_deref() {
            let mut query = tx.prepare_cached(
                "SELECT session FROM native_thread WHERE provider=? AND native_session=?",
            )?;
            sessions.extend(
                query
                    .query_map(params![root.provider, session], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
    }
    tx.execute(
        "DELETE FROM native_limit WHERE read<?",
        [now.saturating_sub(20 * 60_000)],
    )?;
    for window in limits
        .into_iter()
        .filter(|w| w.read >= now.saturating_sub(20 * 60_000) as u64)
    {
        tx.execute(
            "INSERT OR REPLACE INTO native_limit VALUES(?,?,?,?,?)",
            params![
                source,
                root.provider,
                window.id,
                window.read,
                serde_json::to_string(&window)?
            ],
        )?;
    }
    tx.execute("INSERT INTO native_file(source,root,provider,kind,managed,label,cursor,scanned,source_order) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(source) DO UPDATE SET cursor=excluded.cursor,scanned=excluded.scanned,label=excluded.label,source_order=excluded.source_order",params![source,root_key,root.provider,root.kind.name(),managed,root.label,cursor_body,now,source_order])?;
    tx.commit()?;
    Ok(sessions)
}
/// Register native thread IDs for a portable conversation. The native observations
/// only replace a compact step if the log belongs to a Kuvo-managed account.
pub fn threads(
    db: &Connection,
    id: &str,
    agent: &serde_json::Value,
) -> Result<BTreeMap<String, String>> {
    let mut threads = BTreeMap::new();
    let find = |provider: &str, state: &serde_json::Value| -> Option<String> {
        let p = kuvo_agents::accounts::Provider::parse(provider)?;
        let field = match p.kind {
            AgentKind::Claude => "claudeSession",
            AgentKind::Codex => "codexThread",
            AgentKind::Grok | AgentKind::Antigravity => "acpSession",
            _ => return None,
        };
        state[field]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(slots) = agent["threads"].as_object() {
        for (provider, thread) in slots {
            if let Some(native) = find(provider, thread.get("state").unwrap_or(agent)) {
                threads.insert(provider.clone(), native);
            }
        }
    }
    for provider in ["claude-code", "codex-cli", "grok-cli", "antigravity"] {
        if !threads.contains_key(provider)
            && let Some(native) = find(provider, agent)
        {
            threads.insert(provider.into(), native);
        }
    }
    let kept = {
        let mut query =
            db.prepare_cached("SELECT provider,native_session FROM native_thread WHERE session=?")?;
        query
            .query_map([id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?
    };
    if kept == threads {
        return Ok(threads);
    }
    db.execute("DELETE FROM native_thread WHERE session=?", [id])?;
    for (provider, native) in &threads {
        db.execute(
            "INSERT OR IGNORE INTO native_thread VALUES(?1,?2,?3)",
            params![provider, native, id],
        )?;
    }
    Ok(threads)
}
/// One canonical observation per copied maker call. The parent's Claude line
/// wins a sidechain replay even when the latter used a different request ID.
// Separate the two identity lookups so SQLite uses (provider,dedup) and
// (provider,message), not a full provider scan per call.
pub(super) const CANONICAL: &str = r#"
NOT EXISTS (
    SELECT 1 FROM native_call better
    JOIN native_file bf ON bf.source=better.source
    JOIN native_file nf ON nf.source=n.source
    WHERE better.provider=n.provider AND better.dedup=n.dedup
      AND CASE WHEN n.kind='codex' THEN
        (bf.source_order,better.source,better.position,better.ordinal) <
        (nf.source_order,n.source,n.position,n.ordinal)
      ELSE
        (better.sidechain,-better.processed,-better.speed,better.source,better.position,better.ordinal) <
        (n.sidechain,-n.processed,-n.speed,n.source,n.position,n.ordinal)
      END
)
AND (n.kind<>'claude' OR n.message IS NULL OR NOT EXISTS (
    SELECT 1 FROM native_call better
    WHERE better.provider=n.provider AND better.message=n.message
      AND (n.sidechain=1 OR better.sidechain=1)
      AND (better.sidechain,-better.processed,-better.speed,better.source,better.position,better.ordinal) <
          (n.sidechain,-n.processed,-n.speed,n.source,n.position,n.ordinal)
))
"#;
pub fn candidates(
    db: &Connection,
    provider: &str,
    thread: &str,
    start: i64,
    end: i64,
) -> Result<Vec<(String, Observation)>> {
    let mut query=db.prepare(&format!("SELECT n.dedup,n.body FROM native_call n WHERE n.provider=? AND n.native_session=? AND n.managed=1 AND n.at>=? AND n.at<=? AND {CANONICAL} ORDER BY n.at,n.source,n.position,n.ordinal"))?;
    let mut rows = query.query(params![provider, thread, start, end])?;
    let mut result = vec![];
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let body: String = row.get(1)?;
        result.push((key, serde_json::from_str(&body)?));
    }
    Ok(result)
}
#[derive(Clone, Debug)]
pub enum Change {
    Roster(Vec<Root>),
    Scan(Root),
    Files(Root, Vec<PathBuf>),
    Removed(Root),
}
async fn send(tx: &tokio::sync::mpsc::Sender<Change>, change: Change) -> bool {
    tx.send(change).await.is_ok()
}
async fn watch_root(root: Root, hub: WatchHub, tx: tokio::sync::mpsc::Sender<Change>) {
    loop {
        // No footprint in a provider's folder: if it doesn't exist, follow only its
        // nearest existing parent and let the provider create it.
        let folder = root.folder.clone();
        let exists = folder.is_dir();
        let parent = if exists {
            folder.clone()
        } else {
            let Some(parent) = folder
                .ancestors()
                .skip(1)
                .find(|p| p.is_dir())
                .map(Path::to_owned)
            else {
                return;
            };
            parent
        };
        let mut watch = match hub
            .subscribe(
                &parent,
                Options {
                    recursive: exists,
                    create: false,
                },
            )
            .await
        {
            Ok(watch) => watch,
            Err(error) => {
                eprintln!("Couldn't watch {} usage history: {error}", root.provider);
                return;
            }
        };
        if !exists && !send(&tx, Change::Scan(root.clone())).await {
            return;
        }
        if exists {
            let mut live = if root.kind == Kind::Antigravity {
                kuvo_os::watch_live::LiveFiles::new().ok().flatten()
            } else {
                None
            };
            loop {
                let event = tokio::select! {
                    event = watch.recv() => match event { Some(event) => event, None => return },
                    result = super::engine_index::live_events(&mut live), if live.is_some() => {
                        match result { Ok(paths) => Notice::Changed { paths: paths.into_iter().filter_map(|p|p.strip_prefix(&folder).ok().map(Path::to_owned)).collect() }, Err(error) => { eprintln!("Couldn't watch live provider database: {error}"); live = None; continue; } }
                    }
                };
                let mut files = BTreeSet::new();
                let mut scan = false;
                let mut add = |event| match event {
                    Notice::Changed { paths } => files.extend(paths),
                    Notice::Rescan | Notice::Failed(_) => scan = true,
                };
                add(event);
                let until = tokio::time::Instant::now() + Duration::from_millis(250);
                while let Ok(Some(event)) = tokio::time::timeout_at(until, watch.recv()).await {
                    add(event);
                }
                if !folder.is_dir() {
                    if !send(&tx, Change::Removed(root.clone())).await {
                        return;
                    }
                    break;
                }
                if let Some(live) = live.as_mut() {
                    let since = super::limits::now() as i64 - KEEP;
                    let mut paths = if scan {
                        self::files(&folder, root.kind, since)
                    } else {
                        changed_files(
                            &folder,
                            root.kind,
                            &files.iter().cloned().collect::<Vec<_>>(),
                            since,
                        )
                    };
                    paths.sort_by_key(|path| maker_age(path, root.kind));
                    let recent = paths.len().saturating_sub(40);
                    for path in paths.into_iter().skip(recent) {
                        if let Some(parent) = path.parent() {
                            let _ = live.track(parent);
                        }
                        let _ = live.track(&path);
                        let mut wal = path.as_os_str().to_owned();
                        wal.push("-wal");
                        let _ = live.track(Path::new(&wal));
                    }
                }
                if scan {
                    if !send(&tx, Change::Scan(root.clone())).await {
                        return;
                    }
                } else if !files.is_empty() {
                    if !send(
                        &tx,
                        Change::Files(root.clone(), files.into_iter().collect()),
                    )
                    .await
                    {
                        return;
                    }
                }
            }
        } else {
            let next = folder
                .strip_prefix(&parent)
                .ok()
                .and_then(|p| p.iter().next())
                .map(ToOwned::to_owned);
            while let Some(notice) = watch.recv().await {
                let relevant = match notice {
                    Notice::Changed { paths } => {
                        paths.iter().any(|p| p.iter().next() == next.as_deref())
                    }
                    _ => true,
                };
                if relevant
                    && folder
                        .ancestors()
                        .any(|p| p != parent && p.starts_with(&parent) && p.is_dir())
                {
                    break;
                }
            }
        }
    }
}
/// One registry owns root watches; an account added/removed changes only that
/// root's subscription. With no change there is no schedule to walk history.
pub async fn follow_roots(home: PathBuf, hub: WatchHub, tx: tokio::sync::mpsc::Sender<Change>) {
    let mut account_watch = match hub
        .subscribe(
            home.join("agents"),
            Options {
                recursive: false,
                create: true,
            },
        )
        .await
    {
        Ok(watch) => watch,
        Err(error) => {
            eprintln!("Couldn't follow usage accounts: {error}");
            return;
        }
    };
    let watchers = BTreeMap::<Root, tokio::task::JoinHandle<()>>::new();
    struct Stop(BTreeMap<Root, tokio::task::JoinHandle<()>>);
    impl Drop for Stop {
        fn drop(&mut self) {
            for task in self.0.values() {
                task.abort();
            }
        }
    }
    let mut watchers = Stop(watchers);
    while let Some(notice) = account_watch.recv().await {
        if matches!(&notice,Notice::Changed{paths} if !paths.iter().any(|p|p==Path::new("accounts.json")))
        {
            continue;
        }
        let Ok(wanted) = roots(&home).await else {
            continue;
        };
        let wanted = wanted.into_iter().collect::<BTreeSet<_>>();
        if !watchers.0.keys().eq(wanted.iter())
            && !send(&tx, Change::Roster(wanted.iter().cloned().collect())).await
        {
            return;
        }
        let gone = watchers
            .0
            .keys()
            .filter(|root| !wanted.contains(*root))
            .cloned()
            .collect::<Vec<_>>();
        for root in gone {
            if let Some(task) = watchers.0.remove(&root) {
                task.abort();
            }
            if !wanted
                .iter()
                .any(|next| next.provider == root.provider && next.folder == root.folder)
                && !send(&tx, Change::Removed(root)).await
            {
                return;
            }
        }
        for root in wanted {
            if watchers
                .0
                .get(&root)
                .is_some_and(|task| !task.is_finished())
            {
                continue;
            }
            watchers.0.insert(
                root.clone(),
                tokio::spawn(watch_root(root, hub.clone(), tx.clone())),
            );
        }
    }
}
pub fn remove_root(db: &mut Connection, root: &Root) -> Result<BTreeSet<String>> {
    let root_key = key(&root.folder);
    let sources = {
        let mut query = db.prepare("SELECT source FROM native_file WHERE root=? AND provider=?")?;
        let rows = query.query_map(params![root_key, root.provider], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut sessions = BTreeSet::new();
    for source in &sources {
        sessions.extend(affected(db, source)?);
    }
    let tx = db.transaction()?;
    for source in sources {
        tx.execute("DELETE FROM native_call WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_price WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_limit WHERE source=?", [&source])?;
        tx.execute("DELETE FROM native_file WHERE source=?", [&source])?;
    }
    tx.commit()?;
    Ok(sessions)
}

/// The newest logs may belong to a prior sign-in at the same path. Do not attach
/// any reading written before the current credential file, or older than twenty
/// minutes, to a confirmed account.
pub fn recent_limits(
    db: &Connection,
    root: &Root,
    now: u64,
) -> Result<Vec<super::readings::Window>> {
    if root.kind != Kind::Codex {
        return Ok(vec![]);
    }
    let Some(auth_at) = root
        .auth
        .as_deref()
        .and_then(path_age)
        .and_then(|at| u64::try_from(at).ok())
    else {
        return Ok(vec![]);
    };
    let since = auth_at.max(now.saturating_sub(20 * 60_000));
    let mut query=db.prepare_cached("SELECT l.body FROM native_limit l JOIN native_file f ON f.source=l.source WHERE l.provider=? AND f.root=? AND l.read>=? AND l.read<=? ORDER BY l.read")?;
    let rows = query.query_map(params![root.provider, key(&root.folder), since, now], |r| {
        r.get::<_, String>(0)
    })?;
    let mut windows = vec![];
    for row in rows {
        windows.push(serde_json::from_str(&row?)?);
    }
    Ok(windows)
}

pub fn scanned(db: &Connection, root: &Root, now: i64) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE native_root SET scanned=? WHERE root=? AND provider=?",
        params![now, key(&root.folder), root.provider],
    )?;
    Ok(())
}
pub fn reconcile_roots(
    db: &mut Connection,
    roots: &[Root],
    computer: &str,
    now: i64,
) -> Result<BTreeSet<String>> {
    let wanted = roots
        .iter()
        .map(|root| ((key(&root.folder), root.provider.clone()), root))
        .collect::<BTreeMap<_, _>>();
    let stale = {
        let mut query = db.prepare(
            "SELECT root,provider FROM native_root UNION SELECT root,provider FROM native_file",
        )?;
        query
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let tx = db.transaction()?;
    let mut sessions = BTreeSet::new();
    for (root, provider) in stale.into_iter().filter(|key| !wanted.contains_key(key)) {
        let sources = {
            let mut query =
                tx.prepare_cached("SELECT source FROM native_file WHERE root=? AND provider=?")?;
            query
                .query_map(params![root, provider], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for source in sources {
            sessions.extend(affected(&tx, &source)?);
            for table in ["native_call", "native_price", "native_limit", "native_file"] {
                tx.execute(&format!("DELETE FROM {table} WHERE source=?"), [&source])?;
            }
        }
        tx.execute(
            "DELETE FROM native_root WHERE root=? AND provider=?",
            params![root, provider],
        )?;
    }
    for ((root_key, provider), root) in wanted {
        let place = place_of(computer, &root.folder, None);
        tx.execute("INSERT INTO native_root VALUES(?,?,?,?,?,?) ON CONFLICT(root,provider) DO UPDATE SET label=excluded.label,place=excluded.place",params![root_key,provider,root.kind.name(),root.label,now,place])?;
    }
    tx.commit()?;
    Ok(sessions)
}

pub fn place_of(computer: &str, folder: &Path, part: Option<&str>) -> String {
    let folder = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_owned());
    let mut hash = Sha256::new();
    hash.update(b"kuvo-code usage place v2\n");
    hash.update(computer);
    hash.update(b"\n");
    hash.update(folder.as_os_str().as_encoded_bytes());
    if let Some(part) = part {
        hash.update(b"\n");
        hash.update(part);
    }
    hex::encode(&hash.finalize()[..8])
}
