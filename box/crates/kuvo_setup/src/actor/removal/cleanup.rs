//! Credential cleanup for machine homes. An unlink is acknowledged only after
//! supported stores are empty and their parent directories have been synced.

use std::{fs, io, path::Path, time::Duration};

use kuvo_store::files::{remove_file_synced, remove_tree_synced};
use rusqlite::{Connection, OpenFlags};

use crate::storage::Files;

pub(super) fn clear(files: &Files) -> io::Result<()> {
    for name in [
        "KUVO_ENGINE_DATA",
        "KUVO_ENGINE_DB_FOLDER",
        "KUVO_AGENTS_HOME",
        "KUVO_KEYS",
        "KUVO_INTEGRATIONS",
        "KUVO_HOST_URL",
    ] {
        if std::env::var_os(name).is_some_and(|value| !value.is_empty()) {
            return Err(io::Error::other(
                "unlink needs the relocated credential store",
            ));
        }
    }
    if std::env::var("KUVO_KEYS_ON_HOST").as_deref() == Ok("1") {
        return Err(io::Error::other("unlink needs the host's key helper"));
    }
    let root = files.path("").canonicalize()?;
    let engine = root.join("data/opencode");
    for entry in entries(&root, &engine)? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if starts_with_file(name, "auth.json") {
            remove_file_synced(&entry.path())?;
        } else if name.starts_with("opencode") && name.ends_with(".db") {
            if !entry.file_type()?.is_file() {
                return Err(io::Error::other("invalid engine credential database"));
            }
            for suffix in ["-wal", "-shm", "-journal"] {
                if let Some(found) = metadata(&engine.join(format!("{name}{suffix}")))?
                    && !found.is_file()
                {
                    return Err(io::Error::other("invalid engine credential sidecar"));
                }
            }
            clear_engine(&entry.path())?;
        }
    }
    let agents = root.join("agents");
    for each in ["claude", "codex", "grok", "cursor", "antigravity"] {
        let folder = agents.join(each);
        if cfg!(target_os = "macos") && each == "claude" && metadata(&folder)?.is_some() {
            return Err(io::Error::other("unlink needs the Mac credential helper"));
        }
        clear_agent(&root, &folder, each == "claude")?;
    }
    for relative in ["integrations", "keys", "box/keys", "box/bot-keys"] {
        let path = root.join(relative);
        if metadata(&path)?.is_some() {
            parent_in_home(&root, &path)?;
            remove_tree_synced(&path)?;
        }
    }
    for relative in ["run/seeded.json", "run/.ssh-fresh-add"] {
        let path = root.join(relative);
        if metadata(&path)?.is_some() {
            parent_in_home(&root, &path)?;
            remove_file_synced(&path)?;
        }
    }
    Ok(())
}

fn clear_engine(path: &Path) -> io::Result<()> {
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(io::Error::other)?;
    db.busy_timeout(Duration::from_secs(5))
        .map_err(io::Error::other)?;
    db.pragma_update(None, "trusted_schema", false)
        .map_err(io::Error::other)?;
    db.pragma_update(None, "secure_delete", true)
        .map_err(io::Error::other)?;
    db.pragma_update(None, "synchronous", "FULL")
        .map_err(io::Error::other)?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let has: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type='table' AND name='credential')",
            [],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    if has {
        tx.execute("DELETE FROM credential", [])
            .map_err(io::Error::other)?;
        ensure_empty(&tx)?;
    }
    tx.commit().map_err(io::Error::other)?;
    let busy: i64 = db
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(io::Error::other)?;
    if busy != 0 {
        return Err(io::Error::other("credential WAL is busy"));
    }
    if has {
        ensure_empty(&db)?;
    }
    Ok(())
}

fn ensure_empty(db: &Connection) -> io::Result<()> {
    let remains: bool = db
        .query_row("SELECT EXISTS (SELECT 1 FROM credential)", [], |row| {
            row.get(0)
        })
        .map_err(io::Error::other)?;
    if remains {
        return Err(io::Error::other("engine credentials remain"));
    }
    Ok(())
}

fn clear_agent(root: &Path, folder: &Path, claude_root: bool) -> io::Result<()> {
    for entry in entries(root, folder)? {
        let path = entry.path();
        let name = entry.file_name();
        if claude_root && name == "tokens" {
            remove_tree_synced(&path)?;
        } else if entry.file_type()?.is_dir() {
            clear_agent(root, &path, false)?;
        } else if name.to_str().is_some_and(|name| {
            name.starts_with(".auth-")
                || [
                    ".credentials.json",
                    "auth.json",
                    "acp_token.json",
                    "oauth-token.json",
                ]
                .iter()
                .any(|file| starts_with_file(name, file))
        }) {
            remove_file_synced(&path)?;
        }
    }
    Ok(())
}

fn starts_with_file(name: &str, file: &str) -> bool {
    name == file
        || name
            .strip_prefix(file)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match path.symlink_metadata() {
        Ok(info) => Ok(Some(info)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn parent_in_home(root: &Path, path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid credential path"))?;
    if !parent.starts_with(root) || parent.canonicalize()? != parent {
        return Err(io::Error::other("a credential folder was redirected"));
    }
    Ok(())
}

fn entries(root: &Path, folder: &Path) -> io::Result<Vec<fs::DirEntry>> {
    match metadata(folder)? {
        Some(info) if info.is_dir() => {
            parent_in_home(root, folder)?;
            fs::read_dir(folder)?.collect()
        }
        Some(_) => Err(io::Error::other("invalid credential directory")),
        None => Ok(Vec::new()),
    }
}
