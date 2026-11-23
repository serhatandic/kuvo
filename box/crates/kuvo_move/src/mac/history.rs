//! Move coding-agent histories with the folders used by their sessions.
use super::{database::Rekey, paths::PathMap};
use crate::{Disk, Error, entries, metadata};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) fn folders(source: &Path, databases: &[Rekey]) -> Result<BTreeSet<String>, Error> {
    let mut folders: BTreeSet<_> = databases
        .iter()
        .flat_map(|db| db.snapshots.iter().map(|(_, path)| path.clone()))
        .collect();
    for (folder, child) in [("Chats", ""), ("bots", "workspace")] {
        for entry in entries(&source.join(folder))? {
            let path = if child.is_empty() {
                entry.path()
            } else {
                entry.path().join(child)
            };
            if let Some(folder) = path.to_str() {
                folders.insert(folder.to_owned());
            }
        }
    }
    Ok(folders)
}

pub(super) fn rewrite(disk: &Disk, map: &PathMap, folders: &BTreeSet<String>) -> Result<(), Error> {
    let mut claude = accounts(disk, "claude")?;
    claude.push(disk.path("agents/claude/console")?);
    for settings in claude {
        if !metadata(&settings)?.is_some_and(|entry| entry.is_dir()) {
            continue;
        }
        let projects = Disk(settings).path("projects")?;
        if !metadata(&projects)?.is_some_and(|entry| entry.is_dir()) {
            continue;
        }
        for folder in folders {
            let Some(next) = map.absolute(folder) else {
                continue;
            };
            let from = projects.join(claude_slug(folder));
            let to = projects.join(claude_slug(&next));
            if from != to && metadata(&from)?.is_some() && metadata(&to)?.is_none() {
                files::place_mac_entry(&from, &to)?;
            }
        }
    }
    for settings in accounts(disk, "grok")? {
        rewrite_grok(&settings, map)?;
    }
    let mut cursor = accounts(disk, "cursor")?;
    cursor.push(disk.path("agents/cursor")?);
    for settings in cursor {
        if !metadata(&settings)?.is_some_and(|entry| entry.is_dir()) {
            continue;
        }
        let file = Disk(settings).path("agents/agents.ndjson")?;
        if !metadata(&file)?.is_some_and(|entry| entry.is_file()) {
            continue;
        }
        let Some(text) = files::read(&file)?.and_then(|bytes| String::from_utf8(bytes).ok()) else {
            continue;
        };
        let mut changed = false;
        let rewritten = text
            .split('\n')
            .map(|line| {
                if let Ok(mut value) = serde_json::from_str::<Value>(line)
                    && map.json(&mut value)
                {
                    changed = true;
                    value.to_string()
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if changed {
            files::rewrite_mac_entry(&file, rewritten.as_bytes())?;
        }
    }
    Ok(())
}

fn accounts(disk: &Disk, agent: &str) -> Result<Vec<PathBuf>, Error> {
    let folder = disk.path(Path::new("agents").join(agent).join("accounts"))?;
    let mut accounts = Vec::new();
    if metadata(&folder)?.is_some_and(|entry| entry.is_dir()) {
        for entry in entries(&folder)? {
            if entry.file_type()?.is_dir() {
                accounts.push(entry.path());
            }
        }
    }
    Ok(accounts)
}

fn rewrite_grok(home: &Path, map: &PathMap) -> Result<(), Error> {
    let sessions = home.join("sessions");
    if !metadata(&sessions)?.is_some_and(|entry| entry.is_dir()) {
        return Ok(());
    }
    let mut changed = false;
    for entry in entries(&sessions)? {
        let name = entry.file_name();
        let Some(folder) = name.to_str().and_then(decode) else {
            continue;
        };
        let Some(next) = map.absolute(&folder) else {
            continue;
        };
        let name = encode(&next);
        if name.len() > 255 {
            continue;
        }
        let to = sessions.join(name);
        if metadata(&to)?.is_none() {
            files::place_mac_entry(&entry.path(), &to)?;
            changed = true;
        }
    }
    if changed {
        for name in [
            "session_search.sqlite",
            "session_search.sqlite-wal",
            "session_search.sqlite-shm",
        ] {
            files::remove_file_synced(&sessions.join(name))?;
        }
    }
    Ok(())
}

fn claude_slug(folder: &str) -> String {
    let units = folder.encode_utf16();
    let mut slug: String = units
        .clone()
        .map(|unit| {
            u8::try_from(unit)
                .ok()
                .filter(u8::is_ascii_alphanumeric)
                .map_or('-', char::from)
        })
        .collect();
    if slug.len() <= 200 {
        return slug;
    }
    let hash = units.fold(0i32, |sum, unit| {
        sum.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    let mut value = hash.unsigned_abs();
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit(value % 36, 36).expect("base 36 digit"));
        value /= 36;
        if value == 0 {
            break;
        }
    }
    slug.truncate(200);
    slug.push('-');
    slug.extend(digits.into_iter().rev());
    slug
}

fn decode(name: &str) -> Option<String> {
    let mut decoded = Vec::new();
    let mut chars = name.bytes();
    while let Some(byte) = chars.next() {
        if byte == b'%' {
            let hi = char::from(chars.next()?).to_digit(16)? as u8;
            let lo = char::from(chars.next()?).to_digit(16)? as u8;
            decoded.push(hi * 16 + lo);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).ok()
}

fn encode(folder: &str) -> String {
    use std::fmt::Write;
    let mut name = String::new();
    for byte in folder.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            name.push(char::from(byte));
        } else {
            write!(name, "%{byte:02X}").expect("append to string");
        }
    }
    name
}

/// Only relocations in a configured agent's history directory appear here.
#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct External(Vec<Relocation>);

#[derive(Debug, Deserialize, Serialize)]
struct Relocation {
    from: PathBuf,
    to: PathBuf,
    device: u64,
    inode: u64,
    cache: bool,
}

impl External {
    pub(super) fn plan(
        account: &Path,
        map: &PathMap,
        folders: &BTreeSet<String>,
    ) -> Result<Self, Error> {
        let mut layout = Self::default();
        let [projects, sessions] = agent_dirs(account);
        if metadata(&projects)?.is_some_and(|entry| entry.is_dir()) {
            for folder in folders {
                if let Some(next) = map.absolute(folder) {
                    layout.add(
                        projects.join(claude_slug(folder)),
                        projects.join(claude_slug(&next)),
                        false,
                    )?;
                }
            }
        }
        if metadata(&sessions)?.is_some_and(|entry| entry.is_dir()) {
            let before = layout.0.len();
            let mut entries = entries(&sessions)?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let name = entry.file_name();
                let Some(folder) = name.to_str().and_then(decode) else {
                    continue;
                };
                if let Some(next) = map.absolute(&folder) {
                    layout.add(entry.path(), sessions.join(encode(&next)), false)?;
                }
            }
            if layout.0.len() != before {
                for name in [
                    "session_search.sqlite",
                    "session_search.sqlite-wal",
                    "session_search.sqlite-shm",
                ] {
                    layout.add(
                        sessions.join(name),
                        sessions.join(format!("{name}.before-kuvo-move")),
                        true,
                    )?;
                }
            }
        }
        Ok(layout)
    }

    fn add(&mut self, from: PathBuf, to: PathBuf, cache: bool) -> Result<(), Error> {
        if from == to
            || to.file_name().is_none_or(|name| name.len() > 255)
            || metadata(&to)?.is_some()
            || self
                .0
                .iter()
                .any(|entry| entry.from == from || entry.to == to)
        {
            return Ok(());
        }
        if let Some(meta) = metadata(&from)?
            && (!cache || !meta.is_dir())
        {
            self.0.push(Relocation {
                from,
                to,
                device: meta.dev(),
                inode: meta.ino(),
                cache,
            });
        }
        Ok(())
    }

    pub(super) fn validate(&self, account: &Path) -> Result<(), Error> {
        let roots = agent_dirs(account);
        for entry in &self.0 {
            let parent = entry.from.parent().filter(|parent| parent.is_absolute());
            if parent != entry.to.parent()
                || !roots.iter().any(|root| Some(root.as_path()) == parent)
                || entry
                    .from
                    .components()
                    .chain(entry.to.components())
                    .any(|part| part == std::path::Component::ParentDir)
                || parent.is_none_or(|path| {
                    !metadata(path).is_ok_and(|meta| meta.is_some_and(|meta| meta.is_dir()))
                })
            {
                return Err(super::invalid(
                    "The saved move has an invalid coding-agent history path.",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn apply(&self, mut first: impl FnMut()) -> Result<(), Error> {
        for (index, entry) in self.0.iter().enumerate() {
            match (metadata(&entry.from)?, metadata(&entry.to)?) {
                (Some(from), None) if entry.matches(&from) => {
                    files::place_mac_entry(&entry.from, &entry.to)?;
                    if index == 0 {
                        first();
                    }
                }
                (None, Some(to)) if entry.matches(&to) => {}
                _ => return Err(Error::Busy(entry.to.clone())),
            }
        }
        Ok(())
    }

    pub(super) fn undo(&self) -> Result<(), Error> {
        for entry in self.0.iter().rev() {
            if let (None, Some(to)) = (metadata(&entry.from)?, metadata(&entry.to)?)
                && entry.matches(&to)
            {
                files::place_mac_entry(&entry.to, &entry.from)?;
            }
        }
        Ok(())
    }

    pub(super) fn retire_caches(&self) -> Result<(), Error> {
        for entry in self.0.iter().filter(|entry| entry.cache) {
            if metadata(&entry.to)?.is_some_and(|to| entry.matches(&to)) {
                files::remove_file_synced(&entry.to)?;
            }
        }
        Ok(())
    }

    pub(super) fn actions(&self) -> impl Iterator<Item = Value> {
        self.0.iter().map(|entry| json!({ "act": "rename", "from": entry.from, "to": entry.to, "formatVersion": 1 }))
    }
}

impl Relocation {
    fn matches(&self, meta: &std::fs::Metadata) -> bool {
        self.device == meta.dev() && self.inode == meta.ino()
    }
}

fn agent_dirs(account: &Path) -> [PathBuf; 2] {
    let configured = |key: &str, fallback: &str| {
        let setting = std::env::var(key).unwrap_or_default();
        let path = match setting.trim() {
            "" => account.join(fallback),
            "~" if key == "GROK_HOME" => account.into(),
            value if key == "GROK_HOME" && value.starts_with("~/") => account.join(&value[2..]),
            value => account.join(value),
        };
        // Keep configured profiles at their resolved path, so a changed
        // symlink can't redirect replay somewhere else.
        path.canonicalize().unwrap_or(path)
    };
    [
        configured("CLAUDE_CONFIG_DIR", ".claude").join("projects"),
        configured("GROK_HOME", ".grok").join("sessions"),
    ]
}
