//! `GET kuvo/integrations`, `POST kuvo/integrations/:id`, and `DELETE kuvo/integrations/:id`.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use http::StatusCode;
use kuvo_protocol::{
    routes::{delete_kuvo_integrations_id, get_kuvo_integrations, post_kuvo_integrations_id},
    version::{ROUTE_VERSION, ROUTE_VERSION_HEADER},
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::config::{ConfigPaths, SERVERS, SettingsChanged, SettingsRead, UNOWNED_SETTINGS, map_value_at};

use super::{
    host::KeysUnavailable,
    keys::{KeyStore, is_stored_key},
    upgrade::{CatalogIntegration, integration_catalog, integrations_file_for},
};

pub const FEATURES: &[&str] = &["integrations"];
const SEGMENT_ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

#[derive(Debug, Clone, Deserialize)]
pub struct EngineTarget {
    pub port: u16,
    pub password: String,
}

impl EngineTarget {
    pub fn read(home: &Path) -> Option<Self> {
        let p: Self = serde_json::from_str(&fs::read_to_string(home.join("run/engine.json")).ok()?).ok()?;
        (p.port > 0).then_some(p)
    }
    pub fn auth_header(&self) -> String { format!("Basic {}", STANDARD.encode(format!("opencode:{}", self.password))) }
}

#[derive(Clone)]
pub struct EngineClient { home: PathBuf, client: reqwest::Client }

impl EngineClient {
    pub fn for_home(home: &Path) -> Self {
        Self { home: home.to_owned(), client: reqwest::Client::builder().no_proxy().build().unwrap_or_default() }
    }
    pub fn target(&self) -> Option<EngineTarget> { EngineTarget::read(&self.home) }

    pub async fn call(&self, t: &EngineTarget, method: reqwest::Method, route: &str, folder: Option<&str>) -> Option<Value> {
        let mut req = self.client.request(method, format!("http://127.0.0.1:{}{route}", t.port))
            .header(ROUTE_VERSION_HEADER, ROUTE_VERSION).header("authorization", t.auth_header()).timeout(Duration::from_secs(10));
        if let Some(dir) = folder { req = req.header("x-opencode-directory", utf8_percent_encode(dir, SEGMENT_ENCODE).to_string()); }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let text = resp.text().await.ok()?;
        if text.is_empty() { return Some(Value::Null); }
        let val: Value = serde_json::from_str(&text).ok()?;
        Some(val.get("data").cloned().unwrap_or(val))
    }

    /// A JSON body sent to the engine; its answer's data, or none when it refused.
    pub async fn post(&self, t: &EngineTarget, route: &str, body: &Value) -> Option<Value> {
        let resp = self.client.post(format!("http://127.0.0.1:{}{route}", t.port))
            .header(ROUTE_VERSION_HEADER, ROUTE_VERSION).header("authorization", t.auth_header())
            .json(body).timeout(Duration::from_secs(10)).send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let val: Value = resp.json().await.unwrap_or(Value::Null);
        Some(val.get("data").cloned().unwrap_or(val))
    }

    pub async fn reconnect_everywhere(&self, t: &EngineTarget, id: &str) {
        let connect = format!("/api/experimental/mcp/{}/connect", utf8_percent_encode(id, SEGMENT_ENCODE));
        let _ = self.call(t, reqwest::Method::POST, &connect, None).await;
        if let Some(Value::Array(items)) = self.call(t, reqwest::Method::GET, "/api/debug/location", None).await {
            let dirs: HashSet<String> = items.into_iter().filter_map(|i| i.get("directory")?.as_str().filter(|s| !s.is_empty()).map(str::to_owned)).collect();
            for dir in dirs { let _ = self.call(t, reqwest::Method::POST, &connect, Some(&dir)).await; }
        }
    }
}

struct EngineServer { name: String, status: Option<String>, integration_id: Option<String> }
struct EngineCredential { id: String, integration_id: String, active: bool, is_oauth: bool }

fn parse_servers(val: Option<Value>) -> Option<Vec<EngineServer>> {
    Some(val?.as_array()?.iter().filter_map(|i| Some(EngineServer {
        name: i.get("name")?.as_str()?.into(),
        status: i.get("status")?.get("status")?.as_str().map(str::to_owned),
        integration_id: i.get("integrationID").and_then(Value::as_str).map(str::to_owned),
    })).collect())
}

fn parse_credentials(val: Option<Value>) -> Option<Vec<EngineCredential>> {
    Some(val?.as_array()?.iter().filter_map(|i| Some(EngineCredential {
        id: i.get("id")?.as_str()?.into(),
        integration_id: i.get("integrationID").and_then(Value::as_str).unwrap_or("").into(),
        active: i.get("active").and_then(Value::as_bool).unwrap_or(false),
        is_oauth: i.get("value").and_then(|v| v.get("type")?.as_str()) == Some("oauth"),
    })).collect())
}

pub struct IntegrationsService {
    config: ConfigPaths,
    keys: Arc<KeyStore>,
    engine: EngineClient,
    catalog_file: PathBuf,
    integration_data: PathBuf,
}

impl IntegrationsService {
    pub fn new(home: &Path, config: ConfigPaths, keys: Arc<KeyStore>) -> Self {
        let integration_data = crate::config::box_env(home, "KUVO_INTEGRATIONS").map(PathBuf::from).unwrap_or_else(|| home.join("integrations"));
        Self { catalog_file: integrations_file_for(home, config.is_mac), config, keys, engine: EngineClient::for_home(home), integration_data }
    }

    fn health(&self, entry: &CatalogIntegration, srv: Option<&EngineServer>, off: bool, signed_in: Option<&HashSet<String>>) -> &'static str {
        if off { return "disabled"; }
        let mut h = match srv.and_then(|s| s.status.as_deref()) {
            Some("connected") => "connected", Some("needs_auth") => "needsSignIn", Some("failed") => "failed", _ => "starting",
        };
        if h == "connected" && let Some(id) = srv.and_then(|s| s.integration_id.as_deref()) && signed_in.is_some_and(|set| !set.contains(id)) && matches!(entry.way.as_str(), "official" | "app") {
            h = "needsSignIn";
        }
        if h == "connected" && let Some(file) = &entry.sign_in_file {
            let base = Path::new(file).file_name().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(file));
            if !self.integration_data.join(&entry.id).join(base).exists() { h = "waitsForCode"; }
        }
        h
    }

    async fn entries(&self, catalog: &[CatalogIntegration], target: Option<&EngineTarget>) -> (Vec<Value>, bool) {
        let settings = match self.config.read_settings() { SettingsRead::Ok(m) => m, SettingsRead::Problem(_) => Map::new() };
        let (mut servers, mut signed_in, mut had_sign_in) = (None, Some(HashSet::new()), HashSet::new());
        if let Some(t) = target {
            servers = parse_servers(self.engine.call(t, reqwest::Method::GET, "/api/mcp", None).await);
            signed_in = parse_credentials(self.engine.call(t, reqwest::Method::GET, "/api/credential", None).await).map(|creds| {
                let mut active = HashSet::new();
                for c in creds.into_iter().filter(|c| c.is_oauth) {
                    if c.active { active.insert(c.integration_id.clone()); }
                    had_sign_in.insert(c.integration_id);
                }
                active
            });
        }
        let engine_down = servers.is_none();
        let names = self.keys.key_names().await.ok();
        let sites = if catalog.iter().any(|e| e.way == "browser") && self.config.is_mac { self.keys.host().site_sign_ins().await } else { None };
        let list = catalog.iter().map(|e| {
            let mut shown = e.raw.clone();
            for hidden in ["settings", "variables", "signInFile", "signInCookie"] { shown.shift_remove(hidden); }
            let added = map_value_at(&settings, &[SERVERS[0], SERVERS[1], &e.id]).is_some();
            shown.insert("added".into(), Value::Bool(added));
            if let (Some(vars), Some(kept)) = (&e.variables, &names) {
                shown.insert("keysSaved".into(), Value::Bool(vars.values().all(|v| v.as_str().is_some_and(|n| kept.iter().any(|k| k == n)))));
            }
            if e.way == "browser" && let Some(v) = sites.as_ref().and_then(|s| s.get(&e.id)?.as_bool()) { shown.insert("siteSignedIn".into(), Value::Bool(v)); }
            if added {
                let off = map_value_at(&settings, &[SERVERS[0], SERVERS[1], &e.id, "disabled"]).and_then(Value::as_bool).unwrap_or(false);
                let srv = servers.as_ref().and_then(|l| l.iter().find(|s| s.name == e.id));
                let h = (!engine_down || off).then(|| self.health(e, srv, off, signed_in.as_ref()));
                if let Some(h) = h { shown.insert("health".into(), json!(h)); }
                if let Some(sid) = srv.and_then(|s| s.integration_id.as_ref()) {
                    shown.insert("signIn".into(), json!(sid));
                    if h == Some("needsSignIn") && had_sign_in.contains(sid) { shown.insert("signInEnded".into(), Value::Bool(true)); }
                }
            }
            Value::Object(shown)
        }).collect();
        (list, engine_down)
    }

    async fn list(&self) -> Result<Value, Refusal> {
        self.require_owned()?;
        let catalog = integration_catalog(&self.catalog_file, self.config.is_mac);
        let read = self.config.read_settings();
        let (entries, engine_down) = self.entries(&catalog, self.engine.target().as_ref()).await;
        let mut out = json!({ "integrations": entries });
        if engine_down { out["engineDown"] = Value::Bool(true); }
        if let Some(p) = read.problem() { out["problem"] = json!(p); }
        Ok(out)
    }

    async fn change(&self, changes: Value) -> Result<(), Refusal> {
        let cfg = self.config.clone();
        match tokio::task::spawn_blocking(move || cfg.change_settings(&changes)).await.map_err(|e| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
            SettingsChanged::Ok(_) => Ok(()),
            SettingsChanged::Err { status, message, .. } => Err(Refusal::new(StatusCode::from_u16(status).unwrap_or(StatusCode::CONFLICT), message)),
        }
    }

    async fn add(&self, id: &str, body: &[u8]) -> Result<Value, Refusal> {
        self.require_owned()?;
        let entry = self.find_entry(id)?;
        let settings = self.read_settings_ok()?;
        let added = map_value_at(&settings, &[SERVERS[0], SERVERS[1], &entry.id]).is_some();
        let Some(entry_settings) = entry.settings.as_ref().filter(|_| entry.way != "browser") else {
            return Err(Refusal::new(StatusCode::CONFLICT, format!("The agent uses {} on its site: sign in to it in the browser on {}.", entry.title, if self.config.is_mac { "this Mac" } else { "this machine" })));
        };
        let vars = entry.variables.clone().unwrap_or_default();
        if vars.values().any(|v| !v.as_str().is_some_and(is_stored_key)) {
            return Err(Refusal::new(StatusCode::CONFLICT, format!("{} can't be added here: its keys aren't named as Kuvo Code names them.", entry.title)));
        }
        let given = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.get("keys")?.as_object().cloned()).unwrap_or_default();
        let names: Vec<String> = vars.keys().cloned().collect();
        let mut new_keys = false;
        if !names.is_empty() {
            let sent: Vec<String> = names.iter().filter(|n| given.get(*n).and_then(Value::as_str).is_some_and(|v| !v.trim().is_empty())).cloned().collect();
            if sent.len() < names.len() {
                let kept = self.keys.key_names().await.map_err(unavail)?;
                if names.iter().filter(|n| !sent.contains(n)).any(|n| !kept.iter().any(|k| k == vars[n].as_str().unwrap_or(""))) {
                    let labels = entry.raw.get("keys").and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|k| k.get("label")?.as_str()).collect::<Vec<_>>().join(" and ")).unwrap_or_default();
                    return Err(Refusal::new(StatusCode::BAD_REQUEST, format!("{} needs {labels}: send them as {{\"keys\": {{\"{}\": …}}}}.", entry.title, names[0])));
                }
            }
            if !sent.is_empty() {
                self.keys.key_work(|| async {
                    for n in &sent { self.keys.write_key(vars[n].as_str().unwrap_or(""), given[n].as_str().unwrap_or("").trim()).await?; }
                    Ok::<(), KeysUnavailable>(())
                }).await.map_err(unavail)?;
                new_keys = true;
            }
        }
        let now = map_value_at(&self.read_settings_ok()?, &[SERVERS[0], SERVERS[1], &entry.id]).cloned();
        if added && now.is_none() { return Err(Refusal::new(StatusCode::CONFLICT, format!("{} was removed meanwhile.", entry.title))); }
        self.change(json!([{ "path": [SERVERS[0], SERVERS[1], &entry.id], "value": now.clone().unwrap_or_else(|| entry_settings.clone()), "was": now.unwrap_or(Value::Null) }])).await?;
        let target = self.engine.target();
        if new_keys && let Some(t) = target.clone() {
            let (eng, eid) = (self.engine.clone(), entry.id.clone());
            tokio::spawn(async move { tokio::time::sleep(Duration::from_secs(2)).await; eng.reconnect_everywhere(&t, &eid).await; });
        }
        let (mut entries, _) = self.entries(&[entry], target.as_ref()).await;
        Ok(json!({ "integration": entries.pop().unwrap_or(Value::Null) }))
    }

    async fn remove(&self, id: &str) -> Result<Value, Refusal> {
        self.require_owned()?;
        let entry = self.find_entry(id)?;
        let added = map_value_at(&self.read_settings_ok()?, &[SERVERS[0], SERVERS[1], &entry.id]).is_some();
        let own_vars: Vec<String> = entry.variables.as_ref().map(|v| v.values().filter_map(Value::as_str).filter(|s| is_stored_key(s)).map(str::to_owned).collect()).unwrap_or_default();
        if !added {
            let kept: Vec<String> = if own_vars.is_empty() { Vec::new() } else { self.keys.key_names().await.map_err(unavail)?.into_iter().filter(|k| own_vars.contains(k)).collect() };
            if kept.is_empty() { return Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))); }
            self.keys.key_work(|| async { for n in &kept { self.keys.delete_key(n).await?; } Ok::<(), KeysUnavailable>(()) }).await.map_err(unavail)?;
            return Ok(json!({}));
        }
        if !own_vars.is_empty() && let Err(e) = self.keys.key_names().await {
            return Err(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, format!("{} wasn't removed: its key couldn't be taken away. {}", entry.title, e.0)));
        }
        let target = self.engine.target();
        let sign_in = match &target {
            Some(t) => parse_servers(self.engine.call(t, reqwest::Method::GET, "/api/mcp", None).await).and_then(|s| s.into_iter().find(|x| x.name == entry.id)?.integration_id),
            None => None,
        };
        self.change(json!([{ "path": [SERVERS[0], SERVERS[1], &entry.id], "value": Value::Null }])).await?;
        if let (Some(t), Some(sid)) = (&target, &sign_in) && let Some(creds) = parse_credentials(self.engine.call(t, reqwest::Method::GET, "/api/credential", None).await) {
            for c in creds.into_iter().filter(|c| c.integration_id == *sid) {
                let _ = self.engine.call(t, reqwest::Method::DELETE, &format!("/api/credential/{}", utf8_percent_encode(&c.id, SEGMENT_ENCODE)), None).await;
            }
        }
        for name in [entry.id.clone(), format!("{}.json", entry.id)] {
            let p = self.integration_data.join(name);
            let _ = fs::remove_dir_all(&p).or_else(|_| fs::remove_file(&p));
        }
        if !own_vars.is_empty() && let Err(e) = self.keys.key_work(|| async { for v in &own_vars { self.keys.delete_key(v).await?; } Ok::<(), KeysUnavailable>(()) }).await {
            return Err(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, format!("{} is removed, but its key is still kept. {}", entry.title, e.0)));
        }
        Ok(json!({}))
    }

    fn find_entry(&self, id: &str) -> Result<CatalogIntegration, Refusal> {
        integration_catalog(&self.catalog_file, self.config.is_mac).into_iter().find(|e| e.id == id).ok_or_else(|| Refusal::json(StatusCode::NOT_FOUND, json!({})))
    }
    fn read_settings_ok(&self) -> Result<Map<String, Value>, Refusal> {
        match self.config.read_settings() {
            SettingsRead::Ok(m) => Ok(m),
            SettingsRead::Problem(p) => Err(Refusal::new(StatusCode::CONFLICT, format!("The engine's settings file can't be changed from here: {p}"))),
        }
    }
    fn require_owned(&self) -> Result<(), Refusal> {
        self.config.owns_settings().then_some(()).ok_or_else(|| Refusal::new(StatusCode::CONFLICT, UNOWNED_SETTINGS))
    }
}

fn unavail(err: KeysUnavailable) -> Refusal { Refusal::new(StatusCode::SERVICE_UNAVAILABLE, err.0) }
fn decode_id(raw: Option<&str>) -> String { percent_decode_str(raw.unwrap_or("")).decode_utf8().map(|v| v.into_owned()).unwrap_or_default() }

pub fn register(routes: &mut Registry, service: Arc<IntegrationsService>) {
    let s = service.clone();
    routes.register::<get_kuvo_integrations::Spec, _, _>(Access::Any, move |_| {
        let s = s.clone();
        async move { Ok(RouteReply::json_as_kept::<200>(s.list().await?)) }
    });
    let s = service.clone();
    routes.register::<post_kuvo_integrations_id::Spec, _, _>(Access::Any, move |call| {
        let (s, id, body) = (s.clone(), decode_id(call.param("id")), call.request.body.clone());
        async move { Ok(RouteReply::json_as_kept::<200>(s.add(&id, &body).await?)) }
    });
    routes.register::<delete_kuvo_integrations_id::Spec, _, _>(Access::Any, move |call| {
        let (s, id) = (service.clone(), decode_id(call.param("id")));
        async move { let _ = s.remove(&id).await?; Ok(RouteReply::json::<200>(delete_kuvo_integrations_id::Status200 {})) }
    });
}
