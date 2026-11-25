//! Read the TypeScript unlink baseline for a stopped machine home.

mod sqlite;

pub(super) use sqlite::staged_database_has_no_sessions;

use std::{
    fs::{self, File},
    io,
    path::Path,
};

use kuvo_store::files::file_stamp;
use kuvo_wire::{json::canonical, keys::encode};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Error, model::Link, storage::Files};

#[derive(Deserialize, serde::Serialize)]
struct Baseline {
    physical: String,
    logical: Option<String>,
    usage: Usage,
}

#[derive(Deserialize, serde::Serialize)]
struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    digest: String,
    imported: bool,
}

pub(super) fn unchanged(files: &Files, link: &Link) -> Result<bool, Error> {
    let Some(release) = link.extra.get("releasedSetup").and_then(Value::as_str) else {
        return Ok(false);
    };
    // After the first write of this attempt the old bootstrap keys are gone,
    // but the same baseline remains attached to the reserved identity.
    if link.setup.as_ref().map(|setup| setup.position.id.as_str()) != Some(release)
        && !(link.setup.is_none() && link.extra.contains_key("codeIdentity"))
    {
        return Ok(false);
    }
    if pending_work(files)? {
        return Ok(false);
    }
    if let Some(record) = link.extra.get("releasedScopedWork") {
        let baseline: Baseline = serde_json::from_value(record.clone())
            .map_err(|_| Error::Invalid("released work baseline"))?;
        if !unchanged_usage(files, &baseline.usage)? {
            return Ok(false);
        }
        return Ok(
            inventory_matches(files, true, &baseline.physical, baseline.logical.as_deref())
                .unwrap_or(false),
        );
    }
    let Some(physical) = link.extra.get("releasedWork").and_then(Value::as_str) else {
        return Ok(false);
    };
    let logical = link
        .extra
        .get("releasedDatabaseWork")
        .and_then(Value::as_str);
    Ok(inventory_matches(files, false, physical, logical).unwrap_or(false))
}

fn unchanged_usage(files: &Files, before: &Usage) -> Result<bool, Error> {
    let Some(now) = usage(files)? else {
        return Ok(false);
    };
    let unchanged = now.digest == before.digest && now.imported == before.imported;
    let reset = before.scope.is_some()
        && now.scope != before.scope
        && now.digest == empty_usage_digest()
        && !now.imported;
    Ok(unchanged || reset)
}

fn empty_usage_digest() -> String {
    hex::encode(Sha256::digest(b"[]"))[..32].into()
}

fn usage(files: &Files) -> Result<Option<Usage>, Error> {
    let kept = files
        .read::<Value>("agents/usage/settings.json")?
        .unwrap_or_else(|| json!({"units": {}}));
    let Some(settings) = kept.as_object() else {
        return Ok(None);
    };
    if !settings
        .keys()
        .all(|key| matches!(key.as_str(), "units" | "kuvo" | "setup" | "imported"))
        || kept
            .get("kuvo")
            .is_some_and(|version| version.as_u64().is_none_or(|n| n > 1))
    {
        return Ok(None);
    }
    let Some(units) = kept.get("units").and_then(Value::as_object) else {
        return Ok(None);
    };
    let imported = kept.get("imported") == Some(&Value::Bool(true));
    let mut entries = Vec::new();
    let mut names: Vec<_> = units.keys().collect();
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    for name in names {
        let unit = &units[name];
        let stamp = &unit["stamp"];
        if stamp["v"].as_u64().is_none() || !stamp["by"].is_string() || unit.get("value").is_none()
        {
            return Ok(None);
        }
        entries.push(json!([
            name,
            stamp["v"],
            stamp["by"],
            stamp["imported"] == true,
            unit["value"]
        ]));
    }
    let digest = hex::encode(Sha256::digest(canonical(&json!(entries))));
    Ok(Some(Usage {
        scope: kept.get("setup").and_then(Value::as_str).map(str::to_owned),
        digest: digest[..32].into(),
        imported,
    }))
}

pub(crate) fn keep_baseline(files: &Files, link: &mut Link) {
    let baseline = || -> Result<_, Box<dyn std::error::Error>> {
        let (physical, logical) = work_stamp(files, false)?;
        let (scoped_physical, scoped_logical) = work_stamp(files, true)?;
        let usage = usage(files)?.ok_or(Error::Invalid("usage settings"))?;
        let released = Baseline {
            physical: scoped_physical,
            logical: scoped_logical,
            usage,
        };
        Ok((physical, logical, released))
    };
    match baseline() {
        Ok((physical, logical, scoped)) => {
            link.extra.insert("releasedWork".into(), json!(physical));
            if let Some(logical) = logical {
                link.extra
                    .insert("releasedDatabaseWork".into(), json!(logical));
            }
            link.extra
                .insert("releasedScopedWork".into(), json!(scoped));
        }
        Err(_) => {
            link.extra.insert("releasedWork".into(), Value::Null);
        }
    }
}

fn work_stamp(files: &Files, omit_usage: bool) -> io::Result<(String, Option<String>)> {
    let before = inventory(files, omit_usage, Index::Left)?;
    let Some(logical) = before.logical()? else {
        return Ok((before.physical, None));
    };
    let after = inventory(files, omit_usage, Index::Left)?;
    if after.stability != before.stability {
        return Err(io::Error::other("retained work changed during the scan"));
    }
    Ok((after.physical, Some(logical)))
}

fn pending_work(files: &Files) -> Result<bool, Error> {
    if let Some(numbers) = files.read::<Value>("run/work-queue/numbers.json")?
        && (numbers["next"].as_u64().unwrap_or(1) > 1
            || ["asking", "leases", "used", "given", "granted"]
                .iter()
                .any(|field| {
                    numbers.get(*field).is_some_and(|v| {
                        !v.is_null()
                            && !v.as_array().is_some_and(Vec::is_empty)
                            && !v.as_object().is_some_and(serde_json::Map::is_empty)
                    })
                }))
    {
        return Ok(true);
    }
    for folder in [
        "run/owned-data/outbox",
        "run/owned-data/edit-outbox",
        "run/work-queue/forward",
        "run/outbox",
        "run/outbox/sending",
        "run/waiting",
        "run/work-queue/queues",
    ] {
        if files
            .names(folder)?
            .iter()
            .any(|name| name.ends_with(".json") || name.ends_with(".failed"))
        {
            return Ok(true);
        }
    }
    for record in [
        "run/queues.json",
        "run/work/asks.json",
        "run/work/wakes.json",
    ] {
        if let Some(value) = files.read::<Value>(record)? {
            let Value::Object(mut entry) = value else {
                return Ok(true);
            };
            entry.shift_remove("formatVersion");
            if entry.values().any(|v| {
                !v.as_array().is_some_and(Vec::is_empty)
                    && !v.as_object().is_some_and(serde_json::Map::is_empty)
            }) {
                return Ok(true);
            }
        }
    }
    if let Some(value) = files.read::<Value>("run/work/background.json")? {
        let Some(places) = value.as_object() else {
            return Ok(true);
        };
        if places.values().any(|place| {
            place
                .get("sessions")
                .and_then(Value::as_object)
                .is_none_or(|sessions| !sessions.is_empty())
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn inventory_matches(
    files: &Files,
    omit_usage: bool,
    physical: &str,
    logical: Option<&str>,
) -> io::Result<bool> {
    for index in [Index::Left, Index::Stamped] {
        let before = inventory(files, omit_usage, index)?;
        if before.physical == physical {
            return Ok(true);
        }
        let Some(expected) = logical else {
            continue;
        };
        if before.logical()?.as_deref() == Some(expected)
            && inventory(files, omit_usage, index)?.stability == before.stability
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The artifact index is disposable and rewritten while its store is open, so
/// baselines leave it out. Those kept before that still stamp it.
#[derive(Clone, Copy, PartialEq)]
enum Index {
    Left,
    Stamped,
}

struct Inventory {
    physical: String,
    stability: String,
    content: Sha256,
    databases: Vec<(std::path::PathBuf, bool)>,
    bytes: u64,
}

impl Inventory {
    fn logical(&self) -> io::Result<Option<String>> {
        if self.bytes > 64 * 1024 * 1024 {
            return Ok(None);
        }
        let mut hash = self.content.clone();
        match sqlite::stamp(&mut hash, &self.databases) {
            Ok(()) => Ok(Some(encode(&hash.finalize()))),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn inventory(files: &Files, omit_usage: bool, index: Index) -> io::Result<Inventory> {
    // A machine opened with relocated TypeScript data needs the move reader.
    // Do not check a substitute path for that data.
    for key in [
        "KUVO_AGENTS_HOME",
        "KUVO_ENGINE_DB_FOLDER",
        "KUVO_ENGINE_DATA",
        "KUVO_BOTS",
        "KUVO_WAKES",
        "KUVO_CHATS",
        "KUVO_GROUPS",
        "KUVO_ARTIFACTS",
        "KUVO_MAC_ATTACHMENTS",
    ] {
        if std::env::var_os(key).is_some_and(|value| !value.is_empty()) {
            return Err(io::Error::other("relocated retained work"));
        }
    }
    let skip = Skip {
        bots: files.path("bots"),
        artifacts: (index == Index::Left).then(|| files.path("artifacts")),
    };
    let mut hash = Sha256::new();
    // Same traversal order as existing-work.ts. Headers and usage scope files
    // are handled independently of the inventory.
    for name in [
        "bots/personalities.json",
        "run/work-queue/labels.json",
        "run/work-queue/categories.json",
        "box/time-zone.json",
        "agents/usage/settings.json",
        "run/session-settings.json",
        "run/named.json",
        "run/archived.json",
        "bots",
        "data/opencode/storage",
        "agents/sessions",
        "chats",
        "groups",
        "wakes",
        "artifacts",
        "attachments",
        "run/work-queue/files",
        "run/work/notes",
    ] {
        if !omit_usage || name != "agents/usage/settings.json" {
            stamp(&files.path(name), &skip, &mut hash)?;
        }
    }
    let content = hash.clone();
    let mut stable = hash.clone();
    let mut databases = Vec::new();
    let mut bytes = 0u64;
    let engine = files.path("data/opencode");
    for name in names(&engine)? {
        if !name.starts_with("opencode") || !name.ends_with(".db") {
            continue;
        }
        let path = engine.join(&name);
        let (main, size) =
            file_entry(&path)?.ok_or_else(|| io::Error::other("missing retained database"))?;
        hash.update(&main);
        stable.update(main);
        bytes = bytes.saturating_add(size);
        let mut immutable = true;
        for (suffix, physical) in [("-wal", true), ("-journal", false)] {
            if let Some((entry, size)) = file_entry(&engine.join(format!("{name}{suffix}")))? {
                if physical {
                    hash.update(&entry);
                }
                if size > 0 {
                    stable.update(entry);
                    bytes = bytes.saturating_add(size);
                    immutable = false;
                    if !physical {
                        return Err(io::Error::other("retained database has a rollback journal"));
                    }
                }
            }
        }
        if !immutable {
            // A live WAL read may use the SQLite shared-memory sidecar.
            file_entry(&engine.join(format!("{name}-shm")))?;
        }
        databases.push((path, immutable));
    }
    Ok(Inventory {
        physical: encode(&hash.finalize()),
        stability: encode(&stable.finalize()),
        content,
        databases,
        bytes,
    })
}

fn names(folder: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut names = entries
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| io::Error::other("non-Unicode retained path"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    Ok(names)
}

/// Folders whose entries the inventory leaves out: bots' workspaces and the artifact index.
struct Skip {
    bots: std::path::PathBuf,
    artifacts: Option<std::path::PathBuf>,
}

fn stamp(path: &Path, skip: &Skip, hash: &mut Sha256) -> io::Result<()> {
    let info = match path.symlink_metadata() {
        Ok(info) => info,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if info.is_dir() {
        for name in names(path)? {
            let index = name.starts_with("index-v1.sqlite")
                && skip.artifacts.as_deref() == Some(path);
            if name != ".DS_Store"
                && !(name == "workspace" && path.parent() == Some(skip.bots.as_path()))
                && !index
            {
                stamp(&path.join(name), skip, hash)?;
            }
        }
        return Ok(());
    }
    let (entry, _) = file_entry(path)?.ok_or_else(|| io::Error::other("retained file changed"))?;
    hash.update(entry);
    Ok(())
}

fn file_entry(path: &Path) -> io::Result<Option<(String, u64)>> {
    match path.symlink_metadata() {
        Ok(info) if info.is_file() => (),
        Ok(_) => return Err(io::Error::other("invalid retained work path")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let _readable = File::open(path)?;
    let file = file_stamp(path)?.ok_or_else(|| io::Error::other("retained file changed"))?;
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::other("non-Unicode retained path"))?;
    Ok(Some((
        canonical(&json!([
            text,
            file.file.to_string(),
            file.size.to_string(),
            file.changed_ns.to_string()
        ])),
        file.size,
    )))
}
