//! 0.7's first launch: what this computer already has that Kuvo Code can use (`kuvo/accounts/found`),
//! and a found key copied into the engine (`kuvo/accounts/found/use`). Names only: no key leaves
//! the box. Goes on the user's word, once no Mac from before 1.0 connects.

use std::path::{Path, PathBuf};

use http::StatusCode;
use kuvo_agents::account_rows::AccountRows;
use kuvo_os::platform::{Account, Platform};
use kuvo_protocol::routes::{get_kuvo_accounts_found as found, post_kuvo_accounts_found_use as use_found};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_services::integrations::routes::{EngineClient, EngineTarget};
use http::Method;
use serde_json::{Map, Value, json};

/// A sign-in an opencode used apart from Kuvo Code keeps: a key can be copied in; a sign-in
/// renewing its own token can't (the two apps would sign each other out), so it's done again here.
struct Kept {
    integration: String,
    key: Option<Value>,
}

/// Its `auth.json`, in its own data folder (or a test's, `KUVO_DEMO_OPENCODE_AUTH`).
fn auth_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("KUVO_DEMO_OPENCODE_AUTH").filter(|file| !file.is_empty()) {
        return Some(file.into());
    }
    let env = std::env::vars_os().collect();
    let home = Account::current().ok()?.home_folder(Platform::current(), &env).ok()?;
    let own = match std::env::var_os("KUVO_OWN_XDG_DATA_HOME") {
        Some(own) => Some(PathBuf::from(own)),
        None => std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|data| !data.starts_with(home.join(".kuvo-code"))),
    };
    let data = own.filter(|own| own.is_absolute()).unwrap_or_else(|| home.join(".local/share"));
    Some(data.join("opencode/auth.json"))
}

fn kept(file: Option<&Path>) -> Vec<Kept> {
    let Some(Value::Object(entries)) = file.and_then(|file| std::fs::read(file).ok()).and_then(|bytes| serde_json::from_slice(&bytes).ok()) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
        .into_iter()
        .filter_map(|(name, entry)| {
            let integration = name.trim_end_matches('/').to_owned();
            let kind = entry["type"].as_str();
            let key = entry["key"].as_str().filter(|key| !key.is_empty());
            match (kind, key) {
                _ if integration.is_empty() => None,
                (Some("api"), Some(key)) => {
                    let mut value = json!({"type": "key", "key": key});
                    if entry["metadata"].is_object() {
                        value["metadata"] = entry["metadata"].clone();
                    }
                    Some(Kept { integration, key: Some(value) })
                }
                (Some("oauth"), _) if entry["refresh"].is_string() => Some(Kept { integration, key: None }),
                _ => None,
            }
        })
        .collect()
}

/// The engine's integrations, or none read (not running, or no list yet).
async fn integrations(engine: &EngineClient, target: Option<&EngineTarget>) -> Option<Vec<Map<String, Value>>> {
    // Once its plugins are set up, which its providers' list waits for.
    engine.call(target?, Method::GET, "/api/provider", None).await?;
    let listed = engine.call(target?, Method::GET, "/api/integration", None).await?;
    let listed: Vec<_> = listed.as_array()?.iter().filter_map(|each| each.as_object().filter(|each| each["id"].is_string()).cloned()).collect();
    (!listed.is_empty()).then_some(listed)
}

fn connections(integration: &Map<String, Value>) -> &[Value] {
    integration.get("connections").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// Signed in through the engine and working (a plan, a key), a key in the shell aside.
fn working(connection: &Value) -> bool {
    connection["type"] != "env" && matches!(connection["status"]["status"].as_str(), None | Some("connected"))
}

async fn list(rows: Option<&AccountRows>, engine: &EngineClient) -> Value {
    let target = engine.target();
    let (engine_list, agent_list) = tokio::join!(integrations(engine, target.as_ref()), async {
        match rows {
            Some(rows) => Some(rows.list(true).await),
            None => None,
        }
    });
    let unread: Vec<_> = [("engine", engine_list.is_none()), ("agents", agent_list.is_none())]
        .into_iter()
        .filter_map(|(part, missing)| missing.then_some(part))
        .collect();
    let engine_list = engine_list.unwrap_or_default();
    let agents: Vec<_> = agent_list
        .unwrap_or_default()
        .iter()
        .filter(|row| row["metadata"]["source"] == "agent")
        .map(|row| {
            let signed_in = row["connections"].as_array().is_some_and(|all| !all.is_empty());
            let mut agent = json!({"id": row["id"], "name": row["name"].as_str().or(row["id"].as_str()),
                "installed": row["metadata"]["installed"] != false, "signedIn": signed_in});
            if let Some(label) = row["metadata"]["account"].as_str().filter(|label| signed_in && !label.is_empty()) {
                agent["label"] = json!(label);
            }
            agent
        })
        .collect();
    let named = |id: &str| engine_list.iter().find(|each| each["id"] == id);
    let opencode: Vec<_> = kept(auth_file().as_deref())
        .into_iter()
        .map(|entry| {
            let known = named(&entry.integration);
            json!({"id": format!("opencode:{}", entry.integration), "integrationID": entry.integration,
                "name": known.and_then(|each| each["name"].as_str()).unwrap_or(&entry.integration),
                "kind": if entry.key.is_some() { "key" } else { "signIn" },
                "connected": known.is_some_and(|each| connections(each).iter().any(working))})
        })
        .collect();
    let shell: Vec<_> = engine_list
        .iter()
        .flat_map(|each| {
            connections(each).iter().filter(|connection| connection["type"] == "env").filter_map(|connection| {
                let variable = connection["name"].as_str().filter(|name| !name.is_empty())?;
                Some(json!({"integrationID": each["id"], "name": each["name"].as_str().or(each["id"].as_str()), "variable": variable}))
            })
        })
        .collect();
    let connected: Vec<_> = engine_list.iter().filter(|each| connections(each).iter().any(working)).map(|each| each["id"].clone()).collect();
    json!({"agents": agents, "opencode": opencode, "shell": shell, "connected": connected, "unread": unread})
}

/// Copies a found key into the engine as its provider's active sign-in. Only keys.
async fn copy(id: &str, engine: &EngineClient) -> Result<(), (StatusCode, &'static str)> {
    let entry = id
        .strip_prefix("opencode:")
        .and_then(|name| kept(auth_file().as_deref()).into_iter().find(|each| each.integration == name))
        .ok_or((StatusCode::NOT_FOUND, "That key isn’t in opencode’s sign-ins any more."))?;
    let key = entry.key.ok_or((StatusCode::CONFLICT, "This is a sign-in, not a key: sign in to it here instead."))?;
    let target = engine.target().ok_or((StatusCode::SERVICE_UNAVAILABLE, "The engine isn’t running."))?;
    // Brought over already (a second tap, the phone too): once is enough.
    let listed = engine.call(&target, Method::GET, "/api/integration", None).await;
    let known = listed.as_ref().and_then(Value::as_array).and_then(|all| all.iter().find(|each| each["id"] == entry.integration.as_str()));
    if known.and_then(|each| each["connections"].as_array()).is_some_and(|all| all.iter().any(|connection| connection["type"] == "credential")) {
        return Ok(());
    }
    let body = json!({"integrationID": entry.integration, "label": "API key", "value": key, "activate": true});
    engine
        .post(&target, "/api/credential", &body)
        .await
        .ok_or((StatusCode::BAD_GATEWAY, "The engine didn’t take the key. Try again in a moment."))
        .map(drop)
}

pub const FEATURE: &str = "accounts.found";

pub fn register(rows: Option<AccountRows>, home: &Path, routes: &mut Registry) {
    let engine = EngineClient::for_home(home);
    let reading = engine.clone();
    routes.register::<found::Spec, _, _>(Access::Any, move |_| {
        let (rows, engine) = (rows.clone(), reading.clone());
        async move {
            let value = list(rows.as_ref(), &engine).await;
            serde_json::from_value(value).map(RouteReply::json::<200>).map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
        }
    });
    routes.register::<use_found::Spec, _, _>(Access::Any, move |call| {
        let engine = engine.clone();
        async move {
            let given: Value = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
            let id = given["id"].as_str().filter(|id| !id.is_empty()).ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Say which one to use."))?;
            copy(id, &engine).await.map_err(|(status, message)| Refusal::new(status, message))?;
            Ok(RouteReply::json::<200>(use_found::Status200 {}))
        }
    });
}
