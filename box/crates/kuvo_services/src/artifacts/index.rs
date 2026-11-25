//! Disposable SQLite index of the artifact journal (`artifacts/index-v1.sqlite`),
//! matching `box/src/link/artifacts/index.ts` schema and semantics 1:1.

use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, params};

use super::store::{Artifact, Scope};

pub struct ArtifactIndex { log: PathBuf, db: Connection }

impl ArtifactIndex {
    pub fn open(log: PathBuf, file: &Path) -> io::Result<Self> {
        ensure_private_file(file)?;
        let db = Connection::open(file).map_err(io::Error::other)?;
        db.execute_batch(
            "PRAGMA busy_timeout = 3000; PRAGMA synchronous = NORMAL; PRAGMA cache_size = -1024; PRAGMA temp_store = FILE;
             CREATE TABLE IF NOT EXISTS state (stamp TEXT NOT NULL, lines INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS artifact (id TEXT PRIMARY KEY, pinned INTEGER NOT NULL, updated TEXT NOT NULL, body TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS artifact_order ON artifact(pinned DESC, updated DESC, id DESC);
             CREATE TABLE IF NOT EXISTS scope (kind TEXT, name TEXT, id TEXT, PRIMARY KEY (kind, name, id)) WITHOUT ROWID;",
        ).map_err(io::Error::other)?;
        Ok(Self { log, db })
    }

    pub fn get(&self, id: &str) -> Option<Artifact> {
        let body: String = self.db.query_row("SELECT body FROM artifact WHERE id = ?", [id], |r| r.get(0)).optional().ok()??;
        serde_json::from_str(&body).ok()
    }

    pub fn put(&self, artifact: &Artifact) -> io::Result<()> {
        let body = serde_json::to_string(artifact).map_err(io::Error::other)?;
        self.db.execute(
            "INSERT OR REPLACE INTO artifact VALUES (?, ?, ?, ?)",
            params![artifact.id, i32::from(artifact.pinned.is_some()), artifact.updated, body],
        ).map_err(io::Error::other)?;
        let mut add = self.db.prepare_cached("INSERT OR IGNORE INTO scope VALUES (?, ?, ?)").map_err(io::Error::other)?;
        for s in &artifact.where_.sessions { add.execute(params!["session", s, artifact.id]).map_err(io::Error::other)?; }
        for b in &artifact.where_.bots { add.execute(params!["bot", b, artifact.id]).map_err(io::Error::other)?; }
        Ok(())
    }

    pub fn drop_id(&self, id: &str) -> io::Result<()> {
        self.db.execute("DELETE FROM artifact WHERE id = ?", [id]).map_err(io::Error::other)?;
        self.db.execute("DELETE FROM scope WHERE id = ?", [id]).map_err(io::Error::other)?;
        Ok(())
    }

    pub fn values(&self, scope: &Scope, ordered: bool) -> Vec<Artifact> {
        let order = if ordered { " ORDER BY pinned DESC, updated DESC, artifact.id DESC" } else { "" };
        let (filter, args): (&str, Vec<&str>) = match (scope.session.as_deref(), scope.bot.as_deref()) {
            (Some(s), _) => (" WHERE artifact.id IN (SELECT id FROM scope WHERE kind = ? AND name = ?)", vec!["session", s]),
            (_, Some(b)) => (" WHERE artifact.id IN (SELECT id FROM scope WHERE kind = ? AND name = ?)", vec!["bot", b]),
            _ => ("", Vec::new()),
        };
        let Ok(mut stmt) = self.db.prepare(&format!("SELECT body FROM artifact{filter}{order}")) else { return Vec::new() };
        let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0)) else { return Vec::new() };
        rows.filter_map(Result::ok).filter_map(|b| serde_json::from_str(&b).ok()).collect()
    }

    fn pair_query(&self, sql: &str, after: &str) -> Vec<(String, String)> {
        let Ok(mut stmt) = self.db.prepare_cached(sql) else { return Vec::new() };
        let Ok(rows) = stmt.query_map([after], |r| Ok((r.get(0)?, r.get(1)?))) else { return Vec::new() };
        rows.filter_map(Result::ok).collect()
    }

    pub fn page(&self, after: &str) -> Vec<(String, String)> {
        self.pair_query("SELECT id, body FROM artifact WHERE id > ? ORDER BY id LIMIT 64", after)
    }

    pub fn paths(&self, after: &str) -> Vec<(String, String)> {
        self.pair_query("SELECT id, json_extract(body, '$.path') AS path FROM artifact WHERE id > ? ORDER BY id LIMIT 64", after)
    }

    pub fn months(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.log).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".jsonl")).collect();
        names.sort();
        names
    }

    fn stamp(&self) -> io::Result<String> {
        let mut parts = Vec::new();
        for name in self.months() {
            let (ino, size, mtime_ns) = file_stamp_parts(&fs::metadata(self.log.join(&name))?);
            parts.push(serde_json::json!([name, ino.to_string(), size.to_string(), mtime_ns.to_string()]));
        }
        serde_json::to_string(&parts).map_err(io::Error::other)
    }

    pub fn update<T, E: From<io::Error>>(&mut self, work: impl FnOnce(&Self) -> Result<T, E>) -> Result<T, E> {
        self.db.execute_batch("BEGIN IMMEDIATE").map_err(io::Error::other)?;
        match work(self) {
            Ok(val) => { self.db.execute_batch("COMMIT").map_err(io::Error::other)?; Ok(val) }
            Err(err) => { let _ = self.db.execute_batch("ROLLBACK"); Err(err) }
        }
    }

    pub fn load<E: From<io::Error>>(&mut self, replay: impl FnOnce(&Self) -> Result<usize, E>) -> Result<usize, E> {
        let stamp = self.stamp()?;
        let prev: Option<(String, i64)> = self.db
            .query_row("SELECT stamp, lines FROM state", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional().map_err(io::Error::other)?;
        if let Some((prev_stamp, prev_lines)) = prev && prev_stamp == stamp {
            return Ok(prev_lines.max(0) as usize);
        }
        self.update(|this| {
            this.db.execute_batch("DELETE FROM state; DELETE FROM scope; DELETE FROM artifact").map_err(io::Error::other)?;
            let lines = replay(this)?;
            this.db.execute("INSERT INTO state VALUES (?, ?)", params![stamp, lines as i64]).map_err(io::Error::other)?;
            Ok(lines)
        })
    }

    pub fn appended(&mut self, artifact: &Artifact) -> io::Result<()> {
        let stamp = self.stamp()?;
        self.update::<(), io::Error>(|this| {
            this.put(artifact)?;
            this.db.execute("UPDATE state SET stamp = ?, lines = lines + 1", [stamp]).map_err(io::Error::other)?;
            Ok(())
        })
    }

    pub fn compacted(&self, lines: usize) -> io::Result<()> {
        let stamp = self.stamp()?;
        self.db.execute("UPDATE state SET stamp = ?, lines = ?", params![stamp, lines as i64]).map_err(io::Error::other)?;
        Ok(())
    }
}

fn ensure_private_file(file: &Path) -> io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
    opts.open(file).map(|_| ())
}

#[cfg(unix)]
fn file_stamp_parts(meta: &fs::Metadata) -> (u64, u64, i128) {
    use std::os::unix::fs::MetadataExt;
    (meta.ino(), meta.size(), i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()))
}

#[cfg(not(unix))]
fn file_stamp_parts(meta: &fs::Metadata) -> (u64, u64, i128) {
    let mtime_ns = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i128);
    (0, meta.len(), mtime_ns)
}
