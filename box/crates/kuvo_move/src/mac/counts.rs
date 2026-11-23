use super::{engine, invalid};
use crate::{Error, entries, json_at};
use kuvo_store::sqlite;
use serde::{Deserialize, Serialize};
use std::{ffi::OsStr, path::Path, time::Duration};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Inventory {
    sessions: u64,
    projects: u64,
    messages: u64,
    worktrees: u64,
    bots: u64,
    wake_ups: u64,
    worktree_records: u64,
    vault_items: u64,
    accounts: u64,
    group_chats: u64,
    broken_links: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Comparison {
    before: Inventory,
    after: Inventory,
}

impl Comparison {
    pub(super) fn new(before: Inventory, after: Inventory) -> Result<Self, Error> {
        let this = Self { before, after };
        for (field, old, new) in this.pairs() {
            if (field == "brokenLinks" && new > old) || (field != "brokenLinks" && new < old) {
                return Err(invalid(&format!(
                    "The staged move changed {field} from {old} to {new}."
                )));
            }
        }
        Ok(this)
    }

    pub(super) fn report(&self) -> serde_json::Value {
        let mut values = serde_json::Map::new();
        for (name, before, after) in self.pairs() {
            values.insert(format!("{name}Before"), before.into());
            values.insert(name.into(), after.into());
        }
        values.into()
    }

    fn pairs(&self) -> [(&'static str, u64, u64); 11] {
        let Self {
            before: a,
            after: b,
        } = self;
        [
            ("sessions", a.sessions, b.sessions),
            ("projects", a.projects, b.projects),
            ("messages", a.messages, b.messages),
            ("worktrees", a.worktrees, b.worktrees),
            ("bots", a.bots, b.bots),
            ("wakeUps", a.wake_ups, b.wake_ups),
            ("worktreeRecords", a.worktree_records, b.worktree_records),
            ("vaultItems", a.vault_items, b.vault_items),
            ("accounts", a.accounts, b.accounts),
            ("groupChats", a.group_chats, b.group_chats),
            ("brokenLinks", a.broken_links, b.broken_links),
        ]
    }
}

pub(super) fn read(stage: &Path) -> Result<Inventory, Error> {
    let mut counts = Inventory::default();
    for path in engine::databases(&stage.join("data"))? {
        let db = sqlite::open_index(&path, Duration::from_secs(3))?;
        for (table, total) in [
            ("session_v2", &mut counts.sessions),
            ("project", &mut counts.projects),
            ("session_message", &mut counts.messages),
            ("worktree", &mut counts.worktrees),
        ] {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?)",
                [table],
                |row| row.get(0),
            )?;
            if exists {
                *total += db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get::<_, u64>(0)
                })?;
            }
        }
        let integrity: String = db.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(invalid(&format!(
                "The copied database at {} failed its integrity check.",
                path.display()
            )));
        }
        counts.broken_links +=
            db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, u64>(0)
            })?;
    }
    counts.wake_ups = folders(&stage.join("wakes"))?.len() as u64;
    for bot in folders(&stage.join("bots"))? {
        counts.bots += 1;
        counts.wake_ups += entries(&bot.join("wakes"))?
            .iter()
            .filter(|entry| visible(&entry.file_name()))
            .count() as u64;
    }
    counts.worktree_records = json_at::<serde_json::Value>(&stage.join("box/worktrees.json"))
        .ok()
        .flatten()
        .and_then(|value| {
            value
                .get("worktrees")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or_default() as u64;
    counts.vault_items = entries(&stage.join("box/vault/items"))?
        .iter()
        .filter(|entry| entry.path().extension() == Some(OsStr::new("json")))
        .count() as u64;
    for agent in folders(&stage.join("agents"))? {
        counts.accounts += folders(&agent.join("accounts"))?.len() as u64;
    }
    counts.group_chats = folders(&stage.join("groups"))?.len() as u64;
    Ok(counts)
}

fn visible(name: &OsStr) -> bool {
    !name.to_string_lossy().starts_with('.')
}

fn folders(folder: &Path) -> Result<Vec<std::path::PathBuf>, Error> {
    if crate::metadata(folder)?.is_some_and(|meta| !meta.is_dir()) {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in entries(folder)? {
        if visible(&entry.file_name()) && entry.file_type()?.is_dir() {
            result.push(entry.path());
        }
    }
    Ok(result)
}
