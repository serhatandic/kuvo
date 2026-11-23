//! Accept a credential seed over the local installer command.

use std::{collections::HashSet, io, path::Path, time::Duration};

use kuvo_protocol::version::{ROUTE_VERSION, ROUTE_VERSION_HEADER};
use kuvo_store::{
    home::Home,
    layout,
    locks::{OnBusy, OwnedLock, StaleLock},
};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{lease, model::Link, preferences, storage::Files};

pub(crate) mod source;

const MARK: &str = "run/seeded.json";
const SETTINGS: &str = "box/opencode.json";
const INVALID: &str =
    "box seed takes JSON on its standard input: {credentials: [{integrationID, label, value}]}.";
const KEYS_FAILED: &str = "The selected keys could not be saved.";
const INTEGRATIONS_FAILED: &str = "The selected integrations could not be saved.";

#[derive(Serialize)]
pub struct Answer {
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    taken: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keys: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl Answer {
    fn refuse(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            taken: None,
            keys: None,
            message: Some(message.into()),
        }
    }

    fn copied(taken: usize, keys: Option<usize>) -> Self {
        Self {
            status: 200,
            taken: Some(taken),
            keys,
            message: None,
        }
    }
}

#[derive(Serialize)]
struct Credential {
    #[serde(rename = "integrationID")]
    integration_id: String,
    label: String,
    value: Map<String, Value>,
    activate: bool,
}

impl Credential {
    fn portable(row: &Value) -> Option<Self> {
        let id = row["integrationID"].as_str()?;
        let value = row["value"].as_object()?;
        if id.is_empty()
            || id.len() > 100
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || renews_itself(id, &row["value"])
        {
            return None;
        }
        Some(Self {
            integration_id: id.into(),
            label: preferences::truncate(row["label"].as_str().unwrap_or(""), 200),
            value: value.clone(),
            activate: true,
        })
    }
}

fn renews_itself(id: &str, value: &Value) -> bool {
    id != "github-copilot"
        && value["refresh"]
            .as_str()
            .is_some_and(|refresh| !refresh.is_empty())
}

#[derive(Deserialize)]
struct Engine {
    port: u16,
    password: String,
}

pub async fn take(home: &Home, input: &[u8]) -> Answer {
    let Ok(given) = serde_json::from_slice::<Value>(input) else {
        return Answer::refuse(400, INVALID);
    };
    if let Some(version) = given.get("formatVersion")
        && !matches!(version.as_f64(), Some(0.0 | 1.0))
    {
        let shown = version
            .as_f64()
            .filter(|v| v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0)
            .map_or_else(|| "invalid".to_owned(), |v| v.to_string());
        return Answer::refuse(
            400,
            format!(
                "Unsupported Box seed format. Box seed format version {shown} is unsupported (this build writes 1)."
            ),
        );
    }
    if !given["credentials"].is_array() {
        return Answer::refuse(400, INVALID);
    }
    let files = Files::new(home.root().to_owned());
    let workspace = || -> Result<_, crate::Error> {
        layout::open(home)?;
        let guard = lock(&files).map_err(|source| crate::Error::File {
            action: "claiming credential copy at",
            path: files.root().into(),
            source,
        })?;
        crate::actor::validate_disk(&files)?;
        let link = files.read::<Link>(crate::actor::LINK)?;
        if link.is_some_and(|link| {
            ["localUnlink", "localWipe", "removalSource"]
                .iter()
                .any(|field| link.extra.get(*field).is_some_and(|value| !value.is_null()))
        }) {
            return Err(crate::Error::Conflict);
        }
        Ok(guard)
    };
    let Ok(_guard) = workspace() else {
        return Answer::refuse(503, "This computer could not prepare to copy credentials.");
    };
    take_selected(&files, &given, true).await
}

pub(crate) async fn apply_installer(files: &Files, given: &Value, expected: &Value) -> bool {
    if !given["keys"].is_object()
        || given
            .get("formatVersion")
            .is_some_and(|value| !matches!(value.as_f64(), Some(0.0 | 1.0)))
        || !files
            .read::<Value>(crate::actor::LINK)
            .is_ok_and(|actual| actual.as_ref() == Some(expected))
    {
        return false;
    }
    take_selected(files, given, false).await.status == 200
}

async fn take_selected(files: &Files, given: &Value, once: bool) -> Answer {
    let Some(credentials) = given["credentials"].as_array() else {
        return Answer::refuse(400, INVALID);
    };
    let keys = if let Some(keys) = given["keys"].as_object() {
        let Ok(count) = take_keys(files, keys) else {
            return Answer::refuse(503, KEYS_FAILED);
        };
        if take_integrations(files, keys, &given["integrations"]).is_err() {
            return Answer::refuse(503, INTEGRATIONS_FAILED);
        }
        Some(count)
    } else {
        None
    };
    if credentials.is_empty() || once && files.path(MARK).exists() {
        return Answer::copied(0, keys);
    }
    let Some((client, engine, mut existing)) = existing_accounts(files).await else {
        return Answer::refuse(503, "Its engine isn't answering yet.");
    };
    let mut taken = 0;
    for entry in credentials
        .iter()
        .take(100)
        .filter_map(Credential::portable)
    {
        if existing.contains(&entry.integration_id) {
            continue;
        }
        let answer = request(&client, &engine, Method::POST, "api/credential")
            .json(&entry)
            .send()
            .await;
        let accepted = match answer {
            Ok(reply) if reply.status().is_success() => reply.bytes().await.is_ok_and(|data| {
                data.is_empty() || serde_json::from_slice::<Value>(&data).is_ok()
            }),
            _ => false,
        };
        if !accepted {
            let mut failure = Answer::refuse(503, "Some sign-ins could not be saved. Try again.");
            failure.taken = Some(taken);
            return failure;
        }
        existing.insert(entry.integration_id);
        taken += 1;
    }
    if once {
        let _ = files.write(
            MARK,
            &serde_json::json!({"at": lease::wall(), "taken": taken}),
        );
    }
    Answer::copied(taken, keys)
}

async fn existing_accounts(files: &Files) -> Option<(Client, Engine, HashSet<String>)> {
    let (client, engine) = open_engine(files)?;
    let reply = request(&client, &engine, Method::GET, "api/credential")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let value = reply.json::<Value>().await.ok()?;
    let ids = value["data"]
        .as_array()
        .or_else(|| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|row| row["integrationID"].as_str().map(str::to_owned))
        .collect();
    Some((client, engine, ids))
}

fn open_engine(files: &Files) -> Option<(Client, Engine)> {
    let engine = files
        .read::<Engine>("run/engine.json")
        .ok()
        .flatten()
        .filter(|engine| engine.port != 0 && !engine.password.is_empty())?;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .ok()?;
    Some((client, engine))
}

fn request(
    client: &Client,
    engine: &Engine,
    method: Method,
    route: &str,
) -> reqwest::RequestBuilder {
    client
        .request(method, format!("http://127.0.0.1:{}/{route}", engine.port))
        .basic_auth("opencode", Some(&engine.password))
        .header(ROUTE_VERSION_HEADER, ROUTE_VERSION)
}

fn settings_lock(files: &Files) -> io::Result<Option<StaleLock>> {
    StaleLock::acquire(
        &files.path("run/.config.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )
}

pub(crate) fn lock(files: &Files) -> io::Result<OwnedLock> {
    let mut lock = files.root().as_os_str().to_os_string();
    lock.push("-credential-copy.lock");
    OwnedLock::acquire(Path::new(&lock), Duration::from_secs(3))
}

fn portable_key(name: &str) -> bool {
    name.strip_prefix("KUVO_KEY_").is_some_and(|tail| {
        !tail.is_empty()
            && tail
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            && !tail.rsplit_once("__BOT_").is_some_and(|(before, id)| {
                !before.is_empty()
                    && !id.is_empty()
                    && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
            && !tail.starts_with("APPLE_CALENDAR_")
            && !tail.starts_with("CHROME_DEVTOOLS_")
    })
}

fn host_keys(files: &Files) -> bool {
    std::env::var("KUVO_KEYS_ON_HOST").as_deref() == Ok("1")
        || ["KUVO_KEYS", "KUVO_HOST_URL"]
            .iter()
            .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
        || files.path("run/host.json").exists()
}

fn take_keys(files: &Files, keys: &Map<String, Value>) -> io::Result<usize> {
    if host_keys(files) {
        return Err(io::Error::other("copy requires the host key store"));
    }
    let _guard = settings_lock(files)?;
    let key_dir = files.path("box/keys");
    no_redirected_path(&files.path("box"))?;
    no_redirected_path(&key_dir)?;
    let mut kept = 0;
    for (name, text) in keys.iter().take(200) {
        let text = text
            .as_str()
            .filter(|_| portable_key(name))
            .ok_or_else(|| io::Error::other("invalid selected key"))?;
        let file = key_dir.join(name);
        match file.symlink_metadata() {
            Ok(info) if info.is_file() => (),
            Ok(_) => return Err(io::Error::other("invalid selected key path")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                kuvo_store::files::write_atomic(&file, text.as_bytes())?;
            }
            Err(error) => return Err(error),
        }
        kept += 1;
    }
    Ok(kept)
}

fn no_redirected_path(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(io::Error::other("redirected settings path"))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn take_integrations(
    files: &Files,
    selected: &Map<String, Value>,
    given: &Value,
) -> Result<(), crate::Error> {
    let Some(given) = given.as_array().filter(|entries| !entries.is_empty()) else {
        return Ok(());
    };
    let catalog = files
        .read::<Value>("bin/integrations.json")?
        .ok_or(crate::Error::Invalid("installed integration catalog"))?;
    let entries = catalog["integrations"]
        .as_array()
        .ok_or(crate::Error::Invalid("installed integration catalog"))?;
    let _guard = settings_lock(files).map_err(|source| crate::Error::File {
        action: "claiming",
        path: files.path(SETTINGS),
        source,
    })?;
    no_redirected_path(&files.path(SETTINGS)).map_err(|source| crate::Error::File {
        action: "opening",
        path: files.path(SETTINGS),
        source,
    })?;
    let mut settings: Map<String, Value> = files.read(SETTINGS)?.unwrap_or_default();
    let mut changed = false;
    for entry in entries {
        let Some(id) = entry["id"]
            .as_str()
            .filter(|id| !id.is_empty() && *id != "__proto__")
        else {
            continue;
        };
        let Some(choice) = given
            .iter()
            .rev()
            .find(|choice| choice["id"].as_str() == Some(id))
        else {
            continue;
        };
        let Some(variables) = entry["variables"]
            .as_object()
            .filter(|vars| !vars.is_empty())
        else {
            continue;
        };
        if entry["macOnly"] == true
            || !variables.values().all(|value| {
                value
                    .as_str()
                    .is_some_and(|key| portable_key(key) && selected.contains_key(key))
            })
        {
            continue;
        }
        let Some(mut config) = entry["settings"].as_object().cloned() else {
            continue;
        };
        let mcp = settings
            .entry("mcp")
            .or_insert_with(|| Value::Object(Map::new()));
        let servers = if mcp["servers"].is_object() {
            &mut mcp["servers"]
        } else {
            mcp
        };
        let servers = servers
            .as_object_mut()
            .ok_or(crate::Error::Invalid("MCP settings"))?;
        if servers.contains_key(id) {
            continue;
        }
        if choice["disabled"] == true {
            config.insert("disabled".into(), Value::Bool(true));
        }
        let mut configuration = Value::Object(config);
        name_key_paths(
            &mut configuration,
            &files.path("box/keys").to_string_lossy(),
        );
        servers.insert(id.into(), configuration);
        changed = true;
    }
    if changed {
        let mut settings = Value::Object(settings);
        settings.sort_all_objects();
        let mut bytes = serde_json::to_vec_pretty(&settings).expect("JSON settings serialize");
        bytes.push(b'\n');
        files.write_bytes(SETTINGS, &bytes)?;
    }
    Ok(())
}

fn name_key_paths(value: &mut Value, folder: &str) {
    match value {
        Value::String(text) => {
            *text = text.replace("{file:~/.kuvo-code/box/keys/", &format!("{{file:{folder}/"))
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| name_key_paths(item, folder)),
        Value::Object(items) => items
            .values_mut()
            .for_each(|item| name_key_paths(item, folder)),
        _ => (),
    }
}
