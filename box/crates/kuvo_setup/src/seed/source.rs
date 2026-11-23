use std::{collections::HashSet, fs, io};

use kuvo_wire::keys::encode;
use reqwest::Method;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{
    SETTINGS, host_keys, no_redirected_path, open_engine, portable_key, renews_itself, request,
};
use crate::{model::safe_token, storage::Files};

pub(crate) const MISSING: &str =
    "Some selected sign-ins or keys are no longer available. Open the checklist again.";
pub(crate) const MAX_BYTES: usize = 512 * 1024;

pub(crate) struct Prepared {
    pub digest: String,
    pub seed: Vec<u8>,
}

pub(crate) enum Consent {
    None,
    Selected(Prepared),
    Kept(String),
}

impl Consent {
    pub fn digest(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Selected(value) => Some(&value.digest),
            Self::Kept(digest) => Some(digest),
        }
    }
}

#[derive(Default)]
pub(crate) struct Selection(Vec<String>);

impl Selection {
    pub fn read(given: Option<Value>) -> Option<Self> {
        let Some(given) = given else {
            return Some(Self::default());
        };
        let entries = given.as_array().filter(|items| items.len() <= 300)?;
        let mut seen = HashSet::new();
        let mut ids = Vec::with_capacity(entries.len());
        for entry in entries {
            let id = entry.as_str().filter(|id| safe_token(id, 43, 43))?;
            if seen.insert(id) {
                ids.push(id.to_owned());
            }
        }
        Some(Self(ids))
    }

    pub fn empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn ids(&self) -> &[String] {
        &self.0
    }

    pub fn digest(&self) -> String {
        digest(&self.0)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Key,
    SignIn,
}

#[derive(Serialize)]
struct Item<'a> {
    id: &'a str,
    kind: Kind,
    label: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    integration: Option<&'a str>,
    checked: bool,
}

struct Key {
    id: String,
    name: String,
}

struct SignIn {
    id: String,
    integration: String,
    label: String,
    value: Value,
}

#[derive(Default)]
struct Accounts {
    portable: Vec<SignIn>,
    renewing: usize,
}

pub(crate) struct Inventory {
    keys: Option<Vec<Key>>,
    accounts: Option<Accounts>,
}

impl Inventory {
    pub async fn read(files: &Files) -> Self {
        let keys = safe_key_names(files).ok();
        let accounts = engine_accounts(files).await;
        Self { keys, accounts }
    }

    pub fn checklist(&self) -> Value {
        let accounts = self.accounts.as_ref();
        let sign_ins = accounts.into_iter().flat_map(|accounts| &accounts.portable);
        let keys = self.keys.iter().flatten();
        let items: Vec<_> = sign_ins
            .map(|item| Item {
                id: &item.id,
                kind: Kind::SignIn,
                label: if item.label.is_empty() {
                    &item.integration
                } else {
                    &item.label
                },
                integration: Some(&item.integration),
                checked: true,
            })
            .chain(keys.map(|key| Item {
                id: &key.id,
                kind: Kind::Key,
                label: &key.name,
                integration: None,
                checked: true,
            }))
            .collect();
        let unavailable: Vec<_> = [
            (self.keys.is_none(), "keys"),
            (accounts.is_none(), "sign-ins"),
        ]
        .into_iter()
        .filter_map(|(missing, source)| missing.then_some(source))
        .collect();
        json!({"items": items, "renewing": accounts.map_or(0, |list| list.renewing), "unavailable": unavailable})
    }

    pub fn capture(
        self,
        files: &Files,
        selection: &Selection,
    ) -> Result<Vec<u8>, (u16, &'static str)> {
        let unavailable = self.keys.is_none() || self.accounts.is_none();
        let mut keys = Map::new();
        let mut credentials = Vec::new();
        for key in self
            .keys
            .into_iter()
            .flatten()
            .filter(|key| selection.0.contains(&key.id))
        {
            let path = files.path("box/keys").join(&key.name);
            let read = || -> io::Result<String> {
                let metadata = path.symlink_metadata()?;
                if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
                    return Err(io::Error::other("key has invalid size or path"));
                }
                fs::read_to_string(path)
            };
            match read() {
                Ok(value) if !value.is_empty() => {
                    keys.insert(key.name, json!(value));
                }
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(_) => return Err((503, MISSING)),
            }
        }
        for item in self
            .accounts
            .into_iter()
            .flat_map(|list| list.portable)
            .filter(|item| selection.0.contains(&item.id))
        {
            credentials.push(json!({"integrationID": item.integration, "label": item.label, "value": item.value}));
        }
        if credentials.len() + keys.len() < selection.0.len() {
            return Err((if unavailable { 503 } else { 409 }, MISSING));
        }
        let integrations = configured_integrations(files, &keys);
        let seed = serde_json::to_vec(&json!({"formatVersion": 1, "keys": keys, "credentials": credentials, "integrations": integrations}))
            .expect("credential seed serializes");
        if seed.len() > MAX_BYTES {
            return Err((409, "The selected sign-ins and keys are too large to copy."));
        }
        Ok(seed)
    }
}

fn safe_key_names(files: &Files) -> io::Result<Vec<Key>> {
    if host_keys(files) {
        return Err(io::Error::other("keys require the host store"));
    }
    no_redirected_path(&files.path("box"))?;
    no_redirected_path(&files.path("box/keys"))?;
    let mut names = files.names("box/keys").map_err(io::Error::other)?;
    names.sort_unstable();
    Ok(names
        .into_iter()
        .filter(|name| portable_key(name))
        .take(200)
        .map(|name| Key {
            id: digest(&["key", name.as_str()]),
            name,
        })
        .collect())
}

async fn engine_accounts(files: &Files) -> Option<Accounts> {
    let (client, engine) = open_engine(files)?;
    let fetch = |route: &'static str| async {
        request(&client, &engine, Method::GET, route)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json::<Value>()
            .await
            .ok()
    };
    let (integrations, credentials) =
        tokio::join!(fetch("api/integration"), fetch("api/credential"));
    let integrations = integrations?;
    let accounts: HashSet<_> = list(&integrations)
        .iter()
        .filter(|item| item["metadata"]["source"] != "mcp")
        .filter_map(|item| item["id"].as_str())
        .collect();
    let mut result = Accounts::default();
    for row in list(&credentials?) {
        let Some(id) = row["integrationID"].as_str() else {
            continue;
        };
        if row["active"] != true || !accounts.contains(id) || !row["value"].is_object() {
            continue;
        }
        if renews_itself(id, &row["value"]) {
            result.renewing += 1;
        } else if result.portable.len() < 100 {
            let label = row["label"].as_str().unwrap_or_default();
            result.portable.push(SignIn {
                id: digest(&["sign-in", id, row["id"].as_str().unwrap_or(label)]),
                integration: id.into(),
                label: label.into(),
                value: row["value"].clone(),
            });
        }
    }
    Some(result)
}

fn list(value: &Value) -> &[Value] {
    value["data"]
        .as_array()
        .or_else(|| value.as_array())
        .map_or(&[], Vec::as_slice)
}

fn configured_integrations(files: &Files, keys: &Map<String, Value>) -> Vec<Value> {
    let settings = files
        .read::<Value>(SETTINGS)
        .ok()
        .flatten()
        .unwrap_or_default();
    let mcp = &settings["mcp"];
    let configured = if mcp["servers"].is_object() {
        &mcp["servers"]
    } else {
        mcp
    };
    let catalog = files
        .read::<Value>("bin/integrations.json")
        .ok()
        .flatten()
        .unwrap_or_default();
    catalog["integrations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = entry["id"].as_str()?;
            let saved = configured[id].as_object()?;
            let vars = entry["variables"]
                .as_object()
                .filter(|vars| !vars.is_empty())?;
            if entry["macOnly"] == true
                || !vars
                    .values()
                    .all(|value| value.as_str().is_some_and(|key| keys.contains_key(key)))
            {
                return None;
            }
            Some(if saved.get("disabled") == Some(&Value::Bool(true)) {
                json!({"id": id, "disabled": true})
            } else {
                json!({"id": id})
            })
        })
        .collect()
}

fn digest<T: Serialize>(value: T) -> String {
    encode(&Sha256::digest(
        serde_json::to_vec(&value).expect("selection ID serializes"),
    ))
}
