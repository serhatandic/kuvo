//! Receipts for settings writes, recorded under the settings writer's lock.
use crate::config::{SettingChange, map_value_at, sorted_json};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{io, path::Path};

#[derive(Serialize, Deserialize)]
struct SavedChange {
    path: Vec<String>,
    before: Value,
    after: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    setup: String,
    epoch: u64,
    id: String,
    expires: i64,
    changes: Vec<SavedChange>,
}

fn fits(path: &[String]) -> bool {
    !path.is_empty()
        && path.len() <= 16
        && path.iter().all(|k| {
            !k.is_empty()
                && k.encode_utf16().count() <= 200
                && !matches!(k.as_str(), "__proto__" | "prototype" | "constructor")
        })
}
fn changed(
    path: Vec<String>,
    before: &Value,
    after: &Value,
    catalog: Option<&str>,
    out: &mut Vec<SavedChange>,
) {
    if sorted_json(before) == sorted_json(after) {
        return;
    }
    let split = (path.len() == 1
        && matches!(
            path[0].as_str(),
            "mcp" | "agents" | "commands" | "providers"
        ))
        || path == ["mcp", "servers"];
    if split && (before.is_null() || before.is_object()) && (after.is_null() || after.is_object()) {
        let mut keys = Vec::new();
        for key in before
            .as_object()
            .into_iter()
            .flatten()
            .chain(after.as_object().into_iter().flatten())
            .map(|(k, _)| k)
        {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        for key in keys {
            let mut next = path.clone();
            next.push(key.clone());
            changed(next, &before[key], &after[key], None, out);
        }
    } else {
        out.push(SavedChange {
            path,
            before: before.clone(),
            after: after.clone(),
            catalog: catalog.map(str::to_owned),
        });
    }
}
pub fn record(
    home: &Path,
    scope: (String, u64),
    before: &Map<String, Value>,
    after: &Map<String, Value>,
    changes: &[SettingChange],
    catalog: Option<&str>,
) -> io::Result<Option<String>> {
    let mut paths = changes
        .iter()
        .map(|c| c.path.clone())
        .filter(|p| fits(p))
        .collect::<Vec<_>>();
    paths.sort_by_key(Vec::len);
    let mut roots = Vec::<Vec<String>>::new();
    for path in paths {
        if !roots.iter().any(|root| path.starts_with(root)) {
            roots.push(path);
        }
    }
    let mut selected = Vec::new();
    for path in roots {
        let refs = path.iter().map(String::as_str).collect::<Vec<_>>();
        let before = map_value_at(before, &refs).unwrap_or(&Value::Null);
        let after = map_value_at(after, &refs).unwrap_or(&Value::Null);
        changed(path, before, after, catalog, &mut selected);
    }
    if selected.is_empty()
        || selected.len() > 100
        || selected.iter().any(|c| !fits(&c.path))
        || serde_json::to_vec(&selected)?.len() > 512 * 1024
    {
        return Ok(None);
    }
    let file = home.join("box/config-apply/receipts.json");
    let now = jiff::Timestamp::now().as_millisecond();
    let mut receipts: Vec<Receipt> = kuvo_store::files::read(&file)?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?
        .unwrap_or_default();
    receipts.retain(|r| r.expires > now);
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let id = hex::encode(random);
    receipts.push(Receipt {
        setup: scope.0,
        epoch: scope.1,
        id: id.clone(),
        expires: now + 3_600_000,
        changes: selected,
    });
    if receipts.len() > 200 {
        receipts.drain(..receipts.len() - 200);
    }
    kuvo_store::files::write_secret(&file, &serde_json::to_vec(&receipts)?)?;
    Ok(Some(id))
}
