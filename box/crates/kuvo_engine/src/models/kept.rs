//! OpenCode's model lists kept for when it isn't running, so listing models never starts it:
//! its own answers as it last gave them; else the models its agent last offered a conversation;
//! else, where it has run before, what it keeps on disk (the providers you signed in to, and the
//! model catalog it downloaded). A computer where it never ran keeps none. The catalog it
//! downloaded (megabytes) is read only to make one of these, never for a picker.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// OpenCode's answers the picker reads (`api/provider`, `api/model`, `api/integration` and
/// `api/model/default`, the last absent where it didn't answer).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct Raw {
    pub providers: Vec<Value>,
    pub models: Vec<Value>,
    pub accounts: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Value>,
}

/// Where a kept list came from, best first.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Source {
    /// OpenCode's own answers.
    Engine,
    /// The models its agent offered (its own list), named from its catalog.
    Agent,
    /// Read from its database: the providers signed in to and its catalog.
    Disk,
}

#[derive(Deserialize, Serialize)]
struct Kept {
    source: Source,
    /// When the agent's list it was made from was written (ms), for an `Agent` one.
    #[serde(default)]
    agent: u64,
    raw: Raw,
}

fn file(home: &Path) -> PathBuf {
    home.join("run/engine/catalog.json")
}

/// The models OpenCode's agent last offered a conversation (`provider/model`), as it keeps them.
fn agent_file(home: &Path) -> PathBuf {
    home.join("run/agents/opencode/catalogs/opencode-agent.json")
}

fn database(home: &Path) -> PathBuf {
    std::env::var_os("KUVO_ENGINE_DATA")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join("data/opencode"), PathBuf::from)
        .join("opencode.db")
}

/// Whether OpenCode has run on this computer, or a list of its is kept.
pub(crate) fn known(home: &Path) -> bool {
    file(home).is_file() || database(home).is_file()
}

/// OpenCode's own answers, kept as it gave them.
pub(crate) fn keep(home: &Path, raw: &Raw) {
    let kept = Kept {
        source: Source::Engine,
        agent: modified(&agent_file(home)),
        raw: raw.clone(),
    };
    write(home, &kept);
}

fn write(home: &Path, kept: &Kept) {
    let path = file(home);
    let same = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .is_some_and(|old| old.source == kept.source && old.raw == kept.raw);
    if same {
        return;
    }
    let saved = serde_json::to_vec(kept).map_err(std::io::Error::other).and_then(|bytes| {
        kuvo_store::files::private_folder(path.parent().unwrap_or(home))?;
        kuvo_store::files::write_atomic(&path, &bytes)
    });
    if let Err(error) = saved {
        eprintln!("Kuvo could not keep OpenCode's models: {error}");
    }
}

fn modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |at| at.as_millis() as u64)
}

/// The list kept, made again from its agent's when that's newer, or made from disk where OpenCode
/// ran and none is kept. None where it never ran.
pub(crate) fn read(home: &Path) -> Option<(Source, Raw)> {
    let kept = std::fs::read(file(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok());
    let offered = modified(&agent_file(home));
    let newer = offered > 0 && kept.as_ref().is_none_or(|kept| offered > kept.agent);
    // (OpenCode's own answers say more: they stand unless its agent offers a model they lack.)
    let listed = |kept: &Kept, models: &[(String, String, String)]| {
        kept.source == Source::Engine
            && models.iter().all(|(provider, id, _)| {
                kept.raw.models.iter().any(|model| model["providerID"] == **provider && model["id"] == **id)
            })
    };
    if newer
        && let Some(models) = agent_models(home)
        && !kept.as_ref().is_some_and(|kept| listed(kept, &models))
    {
        let raw = made(&database(home), Some(&models));
        if let Some(raw) = raw {
            write(home, &Kept { source: Source::Agent, agent: offered, raw: raw.clone() });
            return Some((Source::Agent, raw));
        }
    }
    if let Some(kept) = kept {
        return Some((kept.source, kept.raw));
    }
    let raw = made(&database(home), None)?;
    write(home, &Kept { source: Source::Disk, agent: 0, raw: raw.clone() });
    Some((Source::Disk, raw))
}

/// The agent's offered models: `provider/model` and the name it gave (`provider/Name`).
fn agent_models(home: &Path) -> Option<Vec<(String, String, String)>> {
    let kept: Value = serde_json::from_slice(&std::fs::read(agent_file(home)).ok()?).ok()?;
    let models = kept["models"].as_array()?.iter().filter_map(|model| {
        let (provider, id) = model["id"].as_str()?.split_once('/')?;
        let name = model["name"].as_str().unwrap_or(id);
        let name = name.split_once('/').map_or(name, |(_, name)| name);
        Some((provider.to_owned(), id.to_owned(), name.to_owned()))
    });
    Some(models.collect())
}

/// Read only, never waiting on OpenCode.
fn open(database: &Path) -> Option<rusqlite::Connection> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = rusqlite::Connection::open_with_flags(database, flags).ok()?;
    connection.busy_timeout(Duration::from_secs(2)).ok()?;
    Some(connection)
}

/// The catalog OpenCode downloaded: each provider's name and models, as it keeps it.
fn catalog(connection: &rusqlite::Connection) -> Option<Map<String, Value>> {
    let kept: String = connection
        .query_row("SELECT value FROM kv WHERE key = 'models-dev:v2:catalog'", [], |row| row.get(0))
        .ok()?;
    let kept: Value = serde_json::from_str(&kept).ok()?;
    serde_json::from_str(kept["body"].as_str()?).ok()
}

/// The providers signed in to (an active sign-in), by OpenCode's name for them.
fn signed_in(connection: &rusqlite::Connection) -> BTreeSet<String> {
    let Ok(mut query) = connection
        .prepare("SELECT DISTINCT integration_id FROM credential WHERE active = 1 AND integration_id IS NOT NULL")
    else {
        return BTreeSet::new();
    };
    query
        .query_map([], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// A model OpenCode offers without asking anything more of it: one that chats with tools, not
/// retired or early, and (for OpenCode's own provider, with no sign-in) free.
fn offerable(provider: &str, model: &Value) -> bool {
    let chats = model["type"].as_str().is_none_or(|kind| kind == "chat");
    let tools = model["capabilities"]["tools"]["supported"] != false && model["tool_call"] != false;
    let settled = !matches!(model["status"].as_str(), Some("deprecated" | "alpha" | "beta"));
    let free = || {
        let price = |side: &str| model["cost"]["text"][side]["price"].as_f64();
        price("input") == Some(0.0) && price("output") == Some(0.0)
    };
    chats && tools && settled && (provider != "opencode" || free())
}

/// The picker's answers made from what's on disk: the models its agent offered (`offered`), or
/// else those of the providers signed in to and OpenCode's free ones. Only what OpenCode would
/// offer: one it's unsure of is left out.
fn made(database: &Path, offered: Option<&[(String, String, String)]>) -> Option<Raw> {
    let connection = database.is_file().then(|| open(database)).flatten();
    let catalog = connection.as_ref().and_then(catalog).unwrap_or_default();
    let mut models: BTreeMap<(String, String), Value> = BTreeMap::new();
    let row = |provider: &str, id: &str, name: &str, known: Option<&Value>| {
        let mut row = json!({"providerID": provider, "id": id, "name": name, "enabled": true});
        if let Some(known) = known {
            let released = known["release_date"].as_str().and_then(|day| {
                format!("{day}T00:00:00Z").parse::<jiff::Timestamp>().ok()
            });
            if let Some(released) = released {
                row["time"] = json!({"released": released.as_millisecond()});
            }
            if let Some(limit) = known.get("limit") {
                row["limit"] = limit.clone();
            }
        }
        row
    };
    match offered {
        Some(offered) => {
            for (provider, id, name) in offered {
                let known = catalog.get(provider).and_then(|entry| entry["models"].get(id));
                models.insert((provider.clone(), id.clone()), row(provider, id, name, known));
            }
        }
        None => {
            let connection = connection.as_ref()?;
            let mut providers = signed_in(connection);
            providers.insert("opencode".into());
            for provider in providers {
                let Some(listed) = catalog.get(&provider).and_then(|entry| entry["models"].as_object()) else {
                    continue;
                };
                for (id, model) in listed.iter().filter(|(_, model)| offerable(&provider, model)) {
                    let name = model["name"].as_str().unwrap_or(id);
                    models.insert((provider.clone(), id.clone()), row(&provider, id, name, Some(model)));
                }
            }
        }
    }
    if models.is_empty() {
        return None;
    }
    let providers: BTreeSet<&str> = models.keys().map(|(provider, _)| provider.as_str()).collect();
    let accounts = providers
        .iter()
        .map(|provider| {
            let name = catalog.get(*provider).and_then(|entry| entry["name"].as_str()).unwrap_or(provider);
            json!({"id": provider, "name": name, "methods": [], "connections": []})
        })
        .collect();
    Some(Raw {
        providers: providers.iter().map(|provider| json!({"id": provider})).collect(),
        models: models.into_values().collect(),
        accounts,
        fallback: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database as OpenCode keeps one: its catalog, and sign-ins.
    fn opencode_database(home: &Path, signed_in: &[&str]) {
        let path = home.join("data/opencode/opencode.db");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE kv (key text PRIMARY KEY, value text NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL);
                 CREATE TABLE credential (id text PRIMARY KEY, integration_id text, label text NOT NULL, value text NOT NULL, connector_id text, method_id text, active integer, time_created integer NOT NULL, time_updated integer NOT NULL);",
            )
            .unwrap();
        let free = json!({"text": {"input": {"price": 0}, "output": {"price": 0}}});
        let paid = json!({"text": {"input": {"price": 1}, "output": {"price": 2}}});
        let body = json!({
            "opencode": {"name": "OpenCode Zen", "models": {
                "big-pickle": {"name": "Big Pickle", "type": "chat", "cost": free, "release_date": "2026-01-02"},
                "paid-one": {"name": "Paid One", "type": "chat", "cost": paid}}},
            "opencode-go": {"name": "OpenCode Go", "models": {
                "deepseek-v4.1-flash": {"name": "DeepSeek V4.1 Flash", "type": "chat", "cost": paid},
                "old-one": {"name": "Old One", "status": "deprecated"},
                "no-tools": {"name": "No Tools", "capabilities": {"tools": {"supported": false}}},
                "pictures": {"name": "Pictures", "type": "image"}}},
            "anthropic": {"name": "Anthropic", "models": {"claude": {"name": "Claude"}}},
        });
        let kept = json!({"updatedAt": 1, "body": body.to_string()}).to_string();
        connection.execute("INSERT INTO kv VALUES ('models-dev:v2:catalog', ?1, 1, 1)", [kept]).unwrap();
        for (index, provider) in signed_in.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO credential VALUES (?1, ?2, 'me', '{}', NULL, NULL, 1, 1, 1)",
                    rusqlite::params![format!("c{index}"), provider],
                )
                .unwrap();
        }
        connection
            .execute("INSERT INTO credential VALUES ('gone', 'anthropic', 'me', '{}', NULL, NULL, 0, 1, 1)", [])
            .unwrap();
    }

    fn ids(raw: &Raw) -> Vec<String> {
        raw.models.iter().map(|model| format!("{}/{}", model["providerID"].as_str().unwrap(), model["id"].as_str().unwrap())).collect()
    }

    #[test]
    fn where_opencode_never_ran_nothing_is_kept_or_made() {
        let home = tempfile::tempdir().unwrap();
        assert!(!known(home.path()));
        assert_eq!(read(home.path()), None);
        assert!(!file(home.path()).exists());
    }

    #[test]
    fn where_it_ran_its_signed_in_providers_and_free_models_are_made_from_disk_once() {
        let home = tempfile::tempdir().unwrap();
        opencode_database(home.path(), &["opencode-go"]);
        let (source, raw) = read(home.path()).unwrap();
        assert_eq!(source, Source::Disk);
        // Only what it would offer: not a retired or tool-less model, not a paid Zen one without
        // a sign-in, not a provider signed out of.
        assert_eq!(ids(&raw), ["opencode/big-pickle", "opencode-go/deepseek-v4.1-flash"]);
        assert_eq!(raw.models[0]["time"]["released"], 1_767_312_000_000_i64);
        assert_eq!(raw.accounts[1]["name"], "OpenCode Go");
        // Kept, so the catalog isn't read again.
        assert!(file(home.path()).is_file());
        std::fs::remove_file(home.path().join("data/opencode/opencode.db")).unwrap();
        assert_eq!(read(home.path()).unwrap().1, raw);
    }

    #[test]
    fn the_models_its_agent_offered_replace_a_list_made_from_disk_and_its_own_answers_replace_both() {
        let home = tempfile::tempdir().unwrap();
        opencode_database(home.path(), &[]);
        let offered = agent_file(home.path());
        std::fs::create_dir_all(offered.parent().unwrap()).unwrap();
        std::fs::write(&offered, json!({"credential": "opencode", "models": [
            {"id": "opencode-go/deepseek-v4.1-flash", "name": "opencode-go/DeepSeek V4.1 Flash"},
            {"id": "standin/scripted", "name": "standin/Scripted"}]}).to_string()).unwrap();
        let (source, raw) = read(home.path()).unwrap();
        assert_eq!(source, Source::Agent);
        assert_eq!(ids(&raw), ["opencode-go/deepseek-v4.1-flash", "standin/scripted"]);
        assert_eq!(raw.models[0]["name"], "DeepSeek V4.1 Flash");
        let own = Raw { providers: vec![json!({"id": "x"})], models: vec![json!({"providerID": "x", "id": "y"})], ..Raw::default() };
        keep(home.path(), &own);
        assert_eq!(read(home.path()), Some((Source::Engine, own)));
    }
}
