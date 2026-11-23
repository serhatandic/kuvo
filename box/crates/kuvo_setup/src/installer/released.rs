//! Read the TypeScript unlink baseline for a stopped machine home. A changed
//! physical SQLite inventory still requires the logical database reader.

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

#[derive(Deserialize)]
struct Baseline {
    physical: String,
    usage: Usage,
}

#[derive(Deserialize)]
struct Usage {
    scope: Option<String>,
    digest: String,
    imported: bool,
}

pub(super) fn unchanged(files: &Files, link: &Link) -> Result<bool, Error> {
    let Some(record) = link.extra.get("releasedScopedWork") else {
        return Ok(false);
    };
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
    let baseline: Baseline = serde_json::from_value(record.clone())
        .map_err(|_| Error::Invalid("released work baseline"))?;
    if !unchanged_usage(files, &baseline.usage)? || pending_work(files)? {
        return Ok(false);
    }
    Ok(fingerprint(files).is_ok_and(|stamp| stamp == baseline.physical))
}

fn unchanged_usage(files: &Files, before: &Usage) -> Result<bool, Error> {
    let kept = files
        .read::<Value>("agents/usage/settings.json")?
        .unwrap_or_else(|| json!({"units": {}}));
    let Some(settings) = kept.as_object() else {
        return Ok(false);
    };
    if !settings
        .keys()
        .all(|key| matches!(key.as_str(), "units" | "kuvo" | "setup" | "imported"))
        || kept
            .get("kuvo")
            .is_some_and(|version| version.as_u64().is_none_or(|n| n > 1))
    {
        return Ok(false);
    }
    let Some(units) = kept.get("units").and_then(Value::as_object) else {
        return Ok(false);
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
            return Ok(false);
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
    let unchanged = digest[..32] == before.digest && imported == before.imported;
    let reset = before
        .scope
        .as_deref()
        .is_some_and(|scope| kept.get("setup").and_then(Value::as_str) != Some(scope))
        && units.is_empty()
        && !imported;
    Ok(unchanged || reset)
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

fn fingerprint(files: &Files) -> io::Result<String> {
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
    let bots = files.path("bots");
    let mut hash = Sha256::new();
    // Same traversal order as existing-work.ts. Headers and usage scope files
    // are handled independently of the inventory.
    for name in [
        "bots/personalities.json",
        "run/work-queue/labels.json",
        "run/work-queue/categories.json",
        "box/time-zone.json",
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
        stamp(&files.path(name), &bots, &mut hash)?;
    }
    let engine = files.path("data/opencode");
    for name in names(&engine)? {
        if name.starts_with("opencode") && name.ends_with(".db") {
            stamp(&engine.join(&name), &bots, &mut hash)?;
            stamp(&engine.join(format!("{name}-wal")), &bots, &mut hash)?;
            // The historical physical hash doesn't cover a pending rollback journal.
            if engine
                .join(format!("{name}-journal"))
                .symlink_metadata()
                .is_ok()
            {
                return Err(io::Error::other("retained database has a rollback journal"));
            }
        }
    }
    Ok(encode(&hash.finalize()))
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

fn stamp(path: &Path, bots: &Path, hash: &mut Sha256) -> io::Result<()> {
    let info = match path.symlink_metadata() {
        Ok(info) => info,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if info.is_dir() {
        for name in names(path)? {
            if name != ".DS_Store" && !(name == "workspace" && path.parent() == Some(bots)) {
                stamp(&path.join(name), bots, hash)?;
            }
        }
        return Ok(());
    }
    if !info.is_file() {
        return Err(io::Error::other("invalid retained work path"));
    }
    let _readable = File::open(path)?;
    let file = file_stamp(path)?.ok_or_else(|| io::Error::other("retained file changed"))?;
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::other("non-Unicode retained path"))?;
    hash.update(canonical(&json!([
        text,
        file.file.to_string(),
        file.size.to_string(),
        file.changed_ns.to_string()
    ])));
    Ok(())
}
