//! Read-only engine channels. Metadata projections are folded into the Insights
//! index without launching the engine. The file watcher drives each changed channel.
use kuvo_os::watch::{Notice, Options, WatchHub};
use kuvo_store::files::FileStamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stamp {
    identity: String,
    revision: String,
    #[serde(default)]
    bytes: u64,
}
fn signature(stamp: &FileStamp) -> String {
    format!(
        "{}:{}:{:?}:{}:{}:{}",
        stamp.device, stamp.file, stamp.created_ns, stamp.modified_ns, stamp.changed_ns, stamp.size
    )
}
pub fn stamp(path: &Path) -> Result<Option<Stamp>> {
    let Some(main) = kuvo_store::files::file_stamp(path)? else {
        return Ok(None);
    };
    let wal = kuvo_store::files::file_stamp(&side(path, "-wal"))?;
    Ok(Some(Stamp {
        identity: format!("{}:{}:{:?}", main.device, main.file, main.created_ns),
        bytes: main.size,
        revision: format!(
            "{}:{}",
            signature(&main),
            wal.as_ref().map(signature).unwrap_or_default()
        ),
    }))
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Checkpoint {
    pub stamp: Option<Stamp>,
    pub watermarks: BTreeMap<String, i64>,
    #[serde(default)]
    pub headers: u64,
    #[serde(default)]
    pub auth_revision: Option<String>,
    #[serde(default)]
    pub sign_in: super::engine_codex_history::SignIn,
    #[serde(default)]
    history_version: u8,
    #[serde(default)]
    pub lineage_version: u8,
    #[serde(default)]
    pub skills_version: u8,
}
fn side(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}
fn engine_name(name: &str) -> bool {
    name.starts_with("opencode") && name.ends_with(".db")
}
pub fn owned(root: &Path, path: &Path) -> Result<Option<PathBuf>> {
    let root = std::fs::canonicalize(root)?;
    let path = match std::fs::canonicalize(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !path.starts_with(&root) || path == root || !path.is_file() {
        return Ok(None);
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        match std::fs::canonicalize(side(&path, suffix)) {
            Ok(file) if !file.starts_with(&root) => {
                return Err("Engine database side file is outside its data folder".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Some(path))
}
pub fn databases(root: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut paths = BTreeSet::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(engine_name) {
            if let Some(path) = owned(root, &entry.path())? {
                paths.insert(path);
            }
        }
    }
    Ok(paths.into_iter().collect())
}
pub fn changed_databases(root: &Path, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();
    for relative in paths {
        if relative.as_os_str().is_empty() || relative.file_name().is_some_and(|n| n == "auth.json")
        {
            return databases(root);
        }
        let Some(name) = relative.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let name = name.strip_suffix("-wal").unwrap_or(name);
        if !engine_name(name) {
            continue;
        }
        let path = root.join(name);
        if let Some(path) = owned(root, &path)? {
            files.insert(path);
        } else if !path.exists() {
            files.insert(path);
        } // removal wakes the metadata reconciliation
    }
    Ok(files.into_iter().collect())
}
pub fn open_maker(path: &Path) -> Result<Connection> {
    let open = || -> Result<Connection> {
        let db = kuvo_store::sqlite::open_index(path, kuvo_store::sqlite::INSIGHTS_BUSY)?;
        db.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| {
            r.get::<_, u64>(0)
        })?;
        Ok(db)
    };
    match open() {
        Ok(db) => Ok(db),
        Err(error)
            if error
                .to_string()
                .to_ascii_lowercase()
                .contains("unable to open")
                && std::fs::metadata(side(path, "-wal")).map_or(true, |m| m.len() == 0) =>
        {
            let mut uri =
                url::Url::from_file_path(path).map_err(|_| "Invalid maker database path")?;
            uri.set_query(Some("immutable=1"));
            Ok(kuvo_store::sqlite::open_immutable_index(
                uri.as_str(),
                kuvo_store::sqlite::INSIGHTS_BUSY,
            )?)
        }
        Err(error) => Err(error),
    }
}
fn columns(db: &Connection, table: &str) -> rusqlite::Result<BTreeSet<String>> {
    let mut query = db.prepare("SELECT name FROM pragma_table_info(?)")?;
    query
        .query_map([table], |r| r.get::<_, String>(0))?
        .collect()
}
#[derive(Clone, Debug)]
pub struct Session {
    pub title: Option<String>,
    pub node: super::insights::lineage::Node,
    pub seat: bool,
}
#[derive(Debug)]
pub struct Record {
    pub session: String,
    pub call: String,
    pub role: String,
    pub created: i64,
    pub completed: Option<i64>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub usage: Option<Value>,
    pub skills: Option<Vec<String>>,
    pub source: Option<String>,
    pub origin: Option<String>,
    pub observed_account: Option<String>,
    pub cost: Option<f64>,
    pub version: i64,
    pub codex_history: bool,
}
/// A channel is read in a snapshot. A watermark uses the maker's indexed change
/// column and an overlap for concurrent writes with the same clock tick.
pub fn read(
    path: &Path,
    since: i64,
    previous: &Checkpoint,
    known_headers: &[String],
    force_headers: bool,
    mut reset: impl FnMut(i64) -> Result<()>,
    mut take: impl FnMut(Record, &BTreeMap<String, Session>) -> Result<()>,
) -> Result<Option<(Checkpoint, BTreeMap<String, Session>)>> {
    let Some(stamp) = stamp(path)? else {
        return Ok(None);
    };
    let folder = path.parent().ok_or("Engine database has no folder")?;
    let auth_revision = kuvo_store::files::file_stamp(&folder.join("auth.json"))
        .ok()
        .flatten()
        .as_ref()
        .map(signature);
    let unchanged = previous.history_version == 2
        && previous.lineage_version == 1
        && previous.skills_version == 1
        && previous.stamp.as_ref() == Some(&stamp)
        && previous.auth_revision == auth_revision;
    if unchanged && !force_headers {
        return Ok(None);
    }
    let db = open_maker(path)?;
    let read = db.unchecked_transaction()?;
    let sign_in = super::engine_codex_history::sign_in(&read, folder).unwrap_or_else(|error| {
        eprintln!("Couldn't read the engine's ChatGPT history sign-in: {error}");
        previous.sign_in
    });
    let mut sessions = BTreeMap::new();
    let mut missing = BTreeSet::new();
    let mut headers = vec![];
    for table in ["session_v2", "session"] {
        let names = columns(&read, table)?;
        if !names.contains("id") {
            continue;
        }
        let col = |field: &'static str| if names.contains(field) { field } else { "NULL" };
        let metadata = col("metadata");
        let sql = format!(
            "SELECT id,{},{},{metadata} FROM {table} WHERE id=?",
            col("parent_id"),
            col("title")
        );
        headers.push(sql);
    }
    let mut checkpoint = Checkpoint {
        stamp: Some(stamp.clone()),
        watermarks: BTreeMap::new(),
        headers: 0,
        auth_revision,
        sign_in,
        history_version: 2,
        lineage_version: 1,
        skills_version: 1,
    };
    collect_headers(
        &read,
        &headers,
        known_headers.iter().cloned(),
        &mut sessions,
        &mut missing,
    )?;
    // Explicit recount may have acquired retained peer calls from an unchanged
    // channel. Repair their compact lineage without replaying any maker calls.
    if unchanged {
        checkpoint.watermarks = previous.watermarks.clone();
        checkpoint.headers = sessions.len() as u64;
        read.commit()?;
        return Ok(Some((checkpoint, sessions)));
    }
    let part_columns = columns(&read, "part")?;
    let legacy_parts = part_columns.contains("message_id") && part_columns.contains("data");
    let skill_parts = |data: &str, from: &str| {
        format!(
            "(SELECT json_group_array(json_array(COALESCE(json_extract({data},'$.name'),json_extract({data},'$.tool')),json_object('skill',json_extract({data},'$.state.input.skill'),'name',json_extract({data},'$.state.input.name'),'file_path',json_extract({data},'$.state.input.file_path'),'filePath',json_extract({data},'$.state.input.filePath'),'path',json_extract({data},'$.state.input.path')))) FROM {from} WHERE json_valid({data}) AND json_extract({data},'$.type')='tool' AND json_extract({data},'$.state.status')='completed' AND lower(COALESCE(json_extract({data},'$.name'),json_extract({data},'$.tool'))) IN ('skill','read','read_file') {{tail}})"
        )
    };
    for (table, role) in [
        ("message", "json_extract(data,'$.role')"),
        ("session_message", "type"),
    ] {
        let names = columns(&read, table)?;
        let version = if table == "message" { 1 } else { 2 };
        if !["id", "session_id", "time_created", "data"]
            .iter()
            .all(|field| names.contains(*field))
            || table == "session_message" && !names.contains("type")
        {
            reset(version)?;
            continue;
        }
        let has_update = names.contains("time_updated");
        let after = if has_update {
            let last: Option<i64> = read.query_row(
                &format!("SELECT MAX(time_updated) FROM {table}"),
                [],
                |row| row.get(0),
            )?;
            if let Some(last) = last {
                checkpoint.watermarks.insert(table.into(), last);
            }
            let before =
                previous.watermarks.get(table).copied().filter(|before| {
                    previous.stamp.as_ref().is_some_and(|old| {
                        old.identity == stamp.identity && old.bytes <= stamp.bytes
                    }) && previous.history_version == 2
                        && previous.skills_version == 1
                        && last.is_some_and(|last| last >= *before)
                        && previous.sign_in == sign_in
                });
            before.map(|old| old.saturating_sub(3_600_000))
        } else {
            None
        };
        if after.is_none() {
            reset(version)?;
        }
        let changed = if after.is_some() {
            "AND (time_updated IS NULL OR time_updated>=?2)"
        } else {
            "AND ?2 IS NULL"
        };
        let parts = if version == 2 {
            skill_parts(
                "p.value",
                "json_each(json_extract(session_message.data,'$.content')) AS p",
            )
            .replace("{tail}", "")
        } else if legacy_parts {
            skill_parts("p.data", "part p").replace("{tail}", "AND p.message_id=message.id")
        } else {
            "NULL".into()
        };
        let sql = format!(
            r#"SELECT session_id,id,{role},time_created,json_extract(data,'$.time.completed'),
        COALESCE(json_extract(data,'$.model.providerID'),json_extract(data,'$.providerID')),
        COALESCE(json_extract(data,'$.model.id'),json_extract(data,'$.modelID')),
        json_extract(data,'$.agent'),json_extract(data,'$.tokens'),
        json_extract(data,'$.metadata.source'),json_extract(data,'$.time.origin'),json_extract(data,'$.time.usageAccount'),
        json_extract(data,'$.cost'),json_type(data,'$.finish'),{parts} FROM {table}
        WHERE time_created>=?1 {changed} AND json_valid(data)
        AND ({role} IN ('user','assistant') OR ({role}='compaction' AND json_extract(data,'$.status')='completed'))"#
        );
        let mut query = read.prepare(&sql)?;
        let mut rows = query.query(params![since, after])?;
        while let Some(row) = rows.next()? {
            let usage: Option<String> = row.get(8).ok().flatten();
            let mut record = Record {
                session: row.get(0)?,
                call: row.get(1)?,
                role: row.get(2)?,
                created: row.get(3)?,
                completed: row.get(4)?,
                provider: row.get(5)?,
                model: row.get(6)?,
                agent: row.get(7)?,
                usage: usage.and_then(|v| serde_json::from_str(&v).ok()),
                skills: row
                    .get::<_, Option<String>>(14)
                    .ok()
                    .flatten()
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                    .and_then(|parts| super::insight_skills::engine(&parts)),
                source: row.get(9)?,
                origin: row.get(10)?,
                observed_account: row.get(11)?,
                cost: row.get(12).ok().flatten(),
                version,
                codex_history: false,
            };
            let finished = record.completed.is_some()
                || row.get::<_, Option<String>>(13)?.as_deref() == Some("text")
                || record.role == "compaction";
            record.codex_history = sign_in.active()
                && record.provider.as_deref() == Some("openai")
                && record.cost == Some(0.)
                && matches!(record.role.as_str(), "assistant" | "compaction")
                && finished
                && (record.version == 1
                    || sign_in
                        .since()
                        .is_none_or(|at| record.completed.unwrap_or(record.created) >= at));
            // Only a changed call's session and parent lineage are read. A
            // channel with many inactive headers does not walk them for one call.
            collect_headers(
                &read,
                &headers,
                [record.session.clone()],
                &mut sessions,
                &mut missing,
            )?;
            take(record, &sessions)?;
        }
    }
    checkpoint.headers = sessions.len() as u64;
    read.commit()?;
    Ok(Some((checkpoint, sessions)))
}
fn collect_headers(
    db: &Connection,
    headers: &[String],
    pending: impl IntoIterator<Item = String>,
    sessions: &mut BTreeMap<String, Session>,
    missing: &mut BTreeSet<String>,
) -> Result<()> {
    let mut pending = pending.into_iter().collect::<Vec<_>>();
    while let Some(key) = pending.pop() {
        if sessions.contains_key(&key) || missing.contains(&key) {
            continue;
        }
        let mut found = None;
        for sql in headers {
            let mut query = db.prepare_cached(sql)?;
            found = query
                .query_row([&key], |row| {
                    let metadata: Option<String> = row.get(3).ok().flatten();
                    let meta = metadata
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .unwrap_or_default();
                    let parent = row.get(1)?;
                    Ok(Session {
                        title: row.get(2)?,
                        node: super::insights::lineage::Node::from_metadata(parent, &meta),
                        seat: !meta["kuvo"]["seat"].is_null(),
                    })
                })
                .optional()?;
            if found.is_some() {
                break;
            }
        }
        if let Some(session) = found {
            pending.extend(
                session
                    .node
                    .parent
                    .iter()
                    .chain(session.node.starters.iter())
                    .cloned(),
            );
            sessions.insert(key, session);
        } else {
            missing.insert(key);
        }
    }
    Ok(())
}
pub async fn follow(root: PathBuf, hub: WatchHub, send: tokio::sync::mpsc::Sender<Vec<PathBuf>>) {
    loop {
        let exists = root.is_dir();
        let folder = if exists {
            root.clone()
        } else {
            let Some(parent) = root.ancestors().skip(1).find(|p| p.is_dir()) else {
                return;
            };
            parent.to_owned()
        };
        let mut watch = match hub
            .subscribe(
                &folder,
                Options {
                    recursive: false,
                    create: false,
                },
            )
            .await
        {
            Ok(watch) => watch,
            Err(error) => {
                eprintln!("Couldn't follow engine usage: {error}");
                return;
            }
        };
        if !exists && send.send(vec![]).await.is_err() {
            return;
        }
        let mut live = kuvo_os::watch_live::LiveFiles::new().ok().flatten();
        if exists {
            if let Some(live) = live.as_mut() {
                let _ = live.track(&root);
                let _ = live.track(&root.join("auth.json"));
            }
        }
        loop {
            let notice = tokio::select! {
                notice = watch.recv() => match notice { Some(notice) => notice, None => return },
                result = live_events(&mut live), if exists && live.is_some() => {
                    match result { Ok(paths) => Notice::Changed { paths: paths.into_iter().filter_map(|p| p.strip_prefix(&root).ok().map(Path::to_owned)).collect() }, Err(error) => { eprintln!("Couldn't watch live engine database: {error}"); live = None; continue; } }
                }
            };
            if !exists {
                if root
                    .ancestors()
                    .any(|p| p != folder && p.starts_with(&folder) && p.is_dir())
                {
                    break;
                }
                continue;
            }
            if !root.is_dir() {
                break;
            }
            let mut files = BTreeSet::new();
            let mut all = false;
            let mut add = |notice| match notice {
                Notice::Changed { paths } => files.extend(paths),
                Notice::Rescan | Notice::Failed(_) => all = true,
            };
            add(notice);
            let until = tokio::time::Instant::now() + Duration::from_millis(250);
            while let Ok(Some(next)) = tokio::time::timeout_at(until, watch.recv()).await {
                add(next);
            }
            // A directory change or legacy auth change is a full discovery,
            // including the empty list when the last channel was removed.
            let all = all
                || files.iter().any(|path| {
                    path.as_os_str().is_empty()
                        || path.file_name().is_some_and(|name| name == "auth.json")
                });
            let paths = (if all {
                databases(&root)
            } else {
                changed_databases(&root, files.into_iter().collect())
            })
            .map_err(|e| e.to_string());
            match paths {
                Ok(paths) if all || !paths.is_empty() => {
                    if let Some(live) = live.as_mut() {
                        for path in &paths {
                            let _ = live.track(path);
                            let _ = live.track(&side(path, "-wal"));
                        }
                    }
                    if send.send(paths).await.is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(error) => eprintln!("Couldn't list engine usage: {error}"),
            }
        }
    }
}
pub async fn live_events(
    live: &mut Option<kuvo_os::watch_live::LiveFiles>,
) -> std::io::Result<Vec<PathBuf>> {
    match live {
        Some(live) => live.recv().await,
        None => std::future::pending().await,
    }
}
/// Read the active OpenCode credential rows from its own data, without an engine
/// process. A supported but empty credential table is a known signed-out answer.
/// A missing legacy table may still use auth.json in that same engine directory.
pub fn credentials(root: &Path) -> Result<Option<Value>> {
    let mut paths = databases(root)?;
    // Prefer the current channel; if it doesn't exist, the most recently written
    // channel is the best local evidence.
    paths.sort_by_key(|p| {
        let current = p.file_name().is_some_and(|name| name == "opencode.db");
        let mtime = std::fs::metadata(p).ok().and_then(|m| m.modified().ok());
        (std::cmp::Reverse(current), std::cmp::Reverse(mtime))
    });
    for path in paths {
        let db = open_maker(&path)?;
        let names = columns(&db, "credential")?;
        if !["integration_id", "value", "active"]
            .iter()
            .all(|name| names.contains(*name))
        {
            continue;
        }
        let order = if names.contains("time_updated") {
            " ORDER BY time_updated DESC"
        } else {
            ""
        };
        let sql = format!(
            "SELECT integration_id,value FROM credential WHERE active=1 AND integration_id IN ('openai','xai','github-copilot','openrouter','zai','zai-coding-plan','opencode-go'){order}"
        );
        let mut query = db.prepare(&sql)?;
        let mut rows = query.query([])?;
        let mut listed = BTreeSet::new();
        let mut result = vec![];
        while let Some(row) = rows.next()? {
            let integration: String = row.get(0)?;
            if !listed.insert(integration.clone()) {
                continue;
            }
            let raw: String = row.get(1)?;
            let mut value: Value = serde_json::from_str(&raw)?;
            if let Some(nested) = value.as_str() {
                value = serde_json::from_str(nested)?;
            }
            if value.is_object() {
                result.push(
                    serde_json::json!({"integrationID":integration,"active":true,"value":value}),
                );
            }
        }
        return Ok(Some(Value::Array(result)));
    }
    let path = root.join("auth.json");
    let Some(raw) = kuvo_store::files::read(&path)? else {
        return Ok(None);
    };
    let value: Value = serde_json::from_slice(&raw)?;
    let Some(rows) = value.as_object() else {
        return Err("Engine auth file is not an object".into());
    };
    let result = rows
        .iter()
        .filter(|(id, value)| {
            matches!(
                id.as_str(),
                "openai"
                    | "xai"
                    | "github-copilot"
                    | "openrouter"
                    | "zai"
                    | "zai-coding-plan"
                    | "opencode-go"
            ) && value.is_object()
        })
        .map(|(id, value)| serde_json::json!({"integrationID":id,"active":true,"value":value}))
        .collect();
    Ok(Some(Value::Array(result)))
}
