use super::invalid;
use crate::{Error, entries, metadata};
use kuvo_os::process::{Spawn, Stdio};
use kuvo_store::{files, sqlite};
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};
use url::Url;

const OLD: (u64, &str) = (50, "20261010180456_project_time_archived");
pub(super) const PINNED: (u64, &str) = (59, "20261030054400_job");

pub(super) fn check_marker_path(source_box: &Path) -> Result<(), Error> {
    let _ = kuvo_store::layout::inspect(&kuvo_store::home::Home::new(source_box.into())?)?;
    Ok(())
}

pub(super) fn databases(data: &Path) -> Result<Vec<PathBuf>, Error> {
    for folder in [data.to_owned(), data.join("opencode")] {
        if metadata(&folder)?.is_some_and(|meta| !meta.is_dir()) {
            return Err(invalid("The staged engine data path must be a directory."));
        }
    }
    let mut databases = Vec::new();
    for entry in entries(&data.join("opencode"))? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_db = name.starts_with("opencode") && name.ends_with(".db");
        let is_companion = name.starts_with("opencode")
            && (name.ends_with(".db-shm") || name.ends_with(".db-wal"));
        if !(is_db || is_companion) {
            continue;
        }
        if !entry.file_type()?.is_file() {
            return Err(invalid(
                "The staged database and its journals must be regular files.",
            ));
        }
        if is_db {
            databases.push(entry.path());
        }
    }
    Ok(databases)
}

pub(super) fn migrate(engine: &Path, data: &Path, scratch: &Path) -> Result<u64, Error> {
    let mut due = None;
    for file in databases(data)? {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let db = sqlite::open_index(&file, Duration::from_secs(3))?;
        match schema(&db)?.as_ref().map(|(n, last)| (*n, last.as_str())) {
            None | Some(PINNED) => {}
            Some(OLD) if name == "opencode.db" => {
                due = Some((file, counts(&db)?));
            }
            Some(other) => {
                return Err(invalid(&format!(
                    "The staged engine database {name} has unsupported migrations ({other:?})."
                )));
            }
        }
    }

    let Some((file, before)) = due else {
        return Ok(0);
    };
    with_engine(engine, data, scratch, |_, _| Ok(()))?;
    let db = sqlite::open_index(&file, Duration::from_secs(3))?;
    if schema(&db)?.as_ref().map(|(n, last)| (*n, last.as_str())) != Some(PINNED) {
        return Err(invalid(
            "The pinned engine did not bring the staged database up to date.",
        ));
    }
    let after = counts(&db)?;
    for (table, before_count) in &before {
        if after
            .get(table)
            .is_some_and(|current| current < before_count)
        {
            return Err(invalid(&format!(
                "The engine migration lost rows from {table}."
            )));
        }
    }
    Ok(if after.contains_key("session_pending") {
        0
    } else {
        before.get("session_pending").copied().unwrap_or(0)
    })
}

pub(super) fn schema(db: &Connection) -> Result<Option<(u64, String)>, rusqlite::Error> {
    let count: u64 = db.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table'",
        [],
        |row| row.get(0),
    )?;
    if count == 0 {
        return Ok(None);
    }
    db.query_row("SELECT count(*), max(id) FROM migration", [], |row| {
        Ok(Some((
            row.get(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        )))
    })
}

fn counts(db: &Connection) -> Result<BTreeMap<String, u64>, rusqlite::Error> {
    let mut tables = db.prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")?;
    tables
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|name| {
            let name = name?;
            let quoted = name.replace('"', "\"\"");
            let count = db.query_row(&format!("SELECT count(*) FROM \"{quoted}\""), [], |r| {
                r.get(0)
            })?;
            Ok((name, count))
        })
        .collect()
}

pub(super) fn with_engine<T>(
    engine: &Path,
    data: &Path,
    scratch: &Path,
    check: impl FnOnce(Url, &str) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut child = Spawn::new(engine, Default::default());
    child.args = ["serve", "--stdio", "--port", "0"].map(Into::into).into();
    child.stdin = Stdio::Pipe;
    child.stdout = Stdio::Pipe;
    // Never collect engine logs in the import receipt. They may contain paths or values.
    child.stderr = Stdio::Null;
    let make = |name| -> Result<PathBuf, Error> {
        let path = scratch.join(name);
        files::create_private_dirs(&path)?;
        Ok(path)
    };
    child.cwd = Some(make("workspace")?);
    for (name, folder) in [
        ("HOME", "home"),
        ("TMPDIR", "tmp"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        child
            .env
            .insert(name.into(), make(folder)?.into_os_string());
    }
    child.env.insert("XDG_DATA_HOME".into(), data.into());
    let mut bytes = [0; 24];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    let password = hex::encode(bytes);
    child
        .env
        .insert("OPENCODE_PASSWORD".into(), password.clone().into());
    for (name, value) in [
        ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
        ("OPENCODE_CLIENT", "desktop"),
        ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
        ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
        ("OPENCODE_DISABLE_FREE_MODELS", "1"),
        ("OPENCODE_DISABLE_PROJECT_CONFIG", "1"),
        ("OPENCODE_DISABLE_CLAUDE_CODE", "1"),
        ("OPENCODE_DISABLE_FILEWATCHER", "1"),
        ("HTTP_PROXY", "http://127.0.0.1:9"),
        ("HTTPS_PROXY", "http://127.0.0.1:9"),
        ("ALL_PROXY", "http://127.0.0.1:9"),
        ("http_proxy", "http://127.0.0.1:9"),
        ("https_proxy", "http://127.0.0.1:9"),
        ("NO_PROXY", "localhost,127.0.0.1,::1"),
        ("no_proxy", "localhost,127.0.0.1,::1"),
    ] {
        child.env.insert(name.into(), value.into());
    }
    let mut worker = child.spawn()?;
    let stdout = worker
        .stdout
        .take()
        .ok_or_else(|| invalid("The engine has no output pipe."))?;
    let (ready, waiting) = mpsc::sync_channel(1);
    let read = thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !sent
                && let Some(url) = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|value| Url::parse(value.get("url")?.as_str()?).ok())
                && url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some()
                && url.username().is_empty()
                && url.password().is_none()
            {
                let _ = ready.send(url);
                sent = true;
            }
        }
    });
    let result = waiting
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| invalid("The pinned engine stopped before it opened the staged database."))
        .and_then(|url| check(url, &password));
    let stopped = worker.stop(Duration::from_secs(2), |worker| {
        worker.stdin.take();
    });
    let _ = read.join();
    stopped?;
    result
}
