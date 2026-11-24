//! Marketplace app connections: Vault credentials, OAuth 2.0 PKCE, loopback MCP HTTP relay
//! (`mcp-relay-v1.json`), and `opencode.json` projection (`mcp-projection-v1.json`).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, net::SocketAddr, path::PathBuf, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use hmac::{Hmac, Mac};
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use kuvo_store::locks::OwnedLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::oneshot};
use url::Url;

use crate::{
    config::{ConfigPaths, SERVERS, SettingsChanged, SettingsRead, map_value_at, sorted_json},
    skills::store::sha256_hex,
    vault::Resident as VaultResident,
};

use super::installations::{InstallationsStore, InstalledApp};

type HmacSha256 = Hmac<Sha256>;
pub const MARKETPLACE_SERVER_PREFIX: &str = "marketplace-";
const CALLBACK_PATH: &str = "/marketplace/oauth/callback";
const MAX_RELAY_BODY: usize = 4 * 1024 * 1024;

pub fn server_name(id: &str) -> String { format!("{MARKETPLACE_SERVER_PREFIX}{id}") }
pub fn credentials_name(app: &InstalledApp) -> String {
    format!("Marketplace · {} · {}", app.id, &sha256_hex(app.url.as_bytes())[..16])
}
fn new_key() -> String {
    let mut b = [0u8; 24];
    let _ = getrandom::fill(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}
fn app_key(key: &str, app: &InstalledApp) -> String {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("hmac key");
    mac.update(format!("{}\0{}", app.id, app.url).as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64) }

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayRecord { port: u16, key: String }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OAuthTokens {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_in: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OAuthMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    registration_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OAuthClient {
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredOAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")] tokens: Option<OAuthTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")] expiry: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] client: Option<OAuthClient>,
    #[serde(default, skip_serializing_if = "Option::is_none")] discovery: Option<OAuthMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")] redirect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] scope: Option<String>,
}

#[derive(Debug, Clone)]
struct Flow {
    app: InstalledApp, redirect: String, expires: u64, state: String, scope: Option<String>,
    done: bool, finishing: bool, client: Option<OAuthClient>, discovery: Option<OAuthMetadata>,
    verifier: String, tokens: Option<OAuthTokens>, expiry: Option<u64>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ConnectionError { pub status: u16, pub message: String }
impl ConnectionError {
    pub fn new(status: u16, message: impl Into<String>) -> Self { Self { status, message: message.into() } }
}

pub struct Connections {
    installations: Arc<InstallationsStore>,
    vault: Arc<VaultResident>,
    config: ConfigPaths,
    endpoint_file: PathBuf,
    app_index: PathBuf,
    projection_lock: PathBuf,
    relay: Mutex<Option<RelayRecord>>,
    flows: Mutex<HashMap<String, Flow>>,
    client: reqwest::Client,
}

impl Connections {
    pub fn new(installations: Arc<InstallationsStore>, vault: Arc<VaultResident>, config: ConfigPaths) -> Self {
        let dir = &installations.marketplace_dir;
        Self {
            endpoint_file: dir.join("mcp-relay-v1.json"),
            app_index: dir.join("mcp-projection-v1.json"),
            projection_lock: dir.join("mcp-projection.lock"),
            installations, vault, config,
            relay: Mutex::new(None),
            flows: Mutex::new(HashMap::new()),
            client: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(15)).build().unwrap_or_default(),
        }
    }

    fn stored_relay(&self) -> Option<RelayRecord> {
        let rec: RelayRecord = serde_json::from_str(&fs::read_to_string(&self.endpoint_file).ok()?).ok()?;
        let ok = rec.port > 0 && rec.key.len() == 32 && rec.key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
        ok.then_some(rec)
    }

    pub fn relay_port(&self) -> Option<u16> { self.relay.lock().ok().and_then(|g| g.as_ref().map(|r| r.port)) }

    pub fn connection_state(&self, app: &InstalledApp) -> &'static str {
        if app.auth == "open" { return "ready"; }
        let name = credentials_name(app);
        self.vault.with_vault(|v| {
            if !v.made() || v.locked() { return Ok("locked"); }
            Ok(if v.named(&name)?.is_some() { "ready" } else { "sign-in" })
        }).unwrap_or("locked")
    }

    fn keep_credential(&self, app: &InstalledApp, record: BTreeMap<String, String>) -> Result<(), ConnectionError> {
        let host = Url::parse(&app.url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
        let name = credentials_name(app);
        let given = Map::from_iter([("kind".into(), json!("key")), ("name".into(), json!(name)), ("sites".into(), json!([host])), ("values".into(), json!(record))]);
        let now = VaultResident::now_millis();
        self.vault.with_vault(|v| {
            if let Some(before) = v.named(&name)? { v.change(&before.id, given, now)?; } else { v.add(given, now)?; }
            Ok(())
        }).map_err(|e| ConnectionError::new(e.status, e.message.into_owned()))
    }

    pub fn connect_token(&self, app: &InstalledApp, token: &str) -> Result<(), ConnectionError> {
        if !(4..=8192).contains(&token.len()) || token.bytes().any(|b| !(0x21..=0x7e).contains(&b)) {
            return Err(ConnectionError::new(400, "Enter the personal access token issued by the service."));
        }
        self.keep_credential(app, BTreeMap::from([("token".into(), token.into())]))
    }

    pub fn disconnect(&self, app: &InstalledApp) -> Result<(), ConnectionError> {
        if app.auth == "open" { return Ok(()); }
        let name = credentials_name(app);
        self.vault.with_vault(|v| {
            if v.locked() { return Err(crate::vault::Error::refused(423, "Unlock this computer’s vault to disconnect the app.")); }
            if let Some(item) = v.named(&name)? { v.remove(&item.id)?; }
            Ok(())
        }).map_err(|e| ConnectionError::new(e.status, e.message.into_owned()))
    }

    pub fn clean_removed_connections(&self) {
        let Ok(state) = self.installations.read() else { return };
        let keep: HashSet<String> = state.apps.iter().map(credentials_name).collect();
        let _ = self.vault.with_vault(|v| {
            if !v.made() || v.locked() { return Ok(()); }
            for item in v.items()? {
                if is_cred_name(&item.name) && !keep.contains(&item.name) && item.fields.iter().any(|f| f == "token" || f == "oauth") {
                    let _ = v.remove(&item.id);
                }
            }
            Ok(())
        });
    }

    async fn https_fetch(&self, req: reqwest::RequestBuilder, url: &str) -> Result<reqwest::Response, ConnectionError> {
        let u = Url::parse(url).map_err(|_| ConnectionError::new(502, "The service must use HTTPS for sign-in."))?;
        let blocked_ip = match u.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_private() || ip.is_link_local() || ip.is_broadcast() || ip.is_unspecified(),
            Some(url::Host::Ipv6(ip)) => ip.is_unspecified() || ip.is_multicast() || (ip.segments()[0] & 0xfe00) == 0xfc00
                || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_private() || v4.is_link_local() || v4.is_broadcast() || v4.is_unspecified()),
            _ => false,
        };
        if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() || blocked_ip {
            return Err(ConnectionError::new(502, "The service must use HTTPS for sign-in."));
        }
        req.send().await.map_err(|e| ConnectionError::new(502, e.to_string()))
    }

    async fn discover_oauth(&self, app_url: &str, prm: Option<&str>) -> Result<OAuthMetadata, ConnectionError> {
        let origin = Url::parse(app_url).map_err(|_| ConnectionError::new(502, "Invalid MCP server URL."))?.origin().ascii_serialization();
        let prm_url = prm.map(str::to_owned).unwrap_or_else(|| format!("{origin}/.well-known/oauth-protected-resource"));
        let auth_srv = if let Ok(r) = self.https_fetch(self.client.get(&prm_url), &prm_url).await && r.status().is_success()
            && let Ok(v) = r.json::<Value>().await
        {
            v.get("authorization_servers").and_then(Value::as_array).and_then(|a| a.first()?.as_str().map(str::to_owned)).unwrap_or_else(|| origin.clone())
        } else { origin.clone() };
        let as_origin = Url::parse(&auth_srv).map(|u| u.origin().ascii_serialization()).unwrap_or(origin);
        for url in [format!("{}/.well-known/oauth-authorization-server", auth_srv.trim_end_matches('/')), format!("{as_origin}/.well-known/oauth-authorization-server"), format!("{as_origin}/.well-known/openid-configuration")] {
            if let Ok(r) = self.https_fetch(self.client.get(&url), &url).await && r.status().is_success()
                && let Ok(m) = r.json::<OAuthMetadata>().await && !m.authorization_endpoint.is_empty() && !m.token_endpoint.is_empty()
            { return Ok(m); }
        }
        Err(ConnectionError::new(502, "The service didn’t provide a sign-in page."))
    }

    pub async fn start_sign_in(&self, app: &InstalledApp) -> Result<Value, ConnectionError> {
        if self.connection_state(app) == "locked" { return Err(ConnectionError::new(423, "Unlock this computer’s vault before signing in.")); }
        let port = self.relay_port().ok_or_else(|| ConnectionError::new(503, "The Marketplace connection service is starting. Try again in a moment."))?;
        let now = now_ms();
        if let Ok(mut map) = self.flows.lock() {
            map.retain(|_, f| f.expires > now);
            if map.len() >= 20 { return Err(ConnectionError::new(409, "Finish or cancel a sign-in before starting another.")); }
        }
        let init = json!({ "jsonrpc": "2.0", "method": "initialize", "id": 1, "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "Kuvo Code", "version": "1" } } });
        let challenge = self.https_fetch(self.client.post(&app.url).header("accept", "application/json, text/event-stream").header("content-type", "application/json").json(&init), &app.url).await?;
        let (prm, offered) = parse_www_auth(challenge.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).unwrap_or(""));
        let scope = if app.origin == "catalog" && app.id == "notion" { Some("default".into()) } else { offered };
        drop(challenge);

        let meta = self.discover_oauth(&app.url, prm.as_deref()).await?;
        let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let mut ci = OAuthClient { client_id: "kuvo-code".into(), client_secret: None };
        if let Some(reg) = &meta.registration_endpoint {
            let r = self.https_fetch(self.client.post(reg).json(&json!({ "client_name": "Kuvo Code", "redirect_uris": [&redirect], "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"], "token_endpoint_auth_method": "none" })), reg).await?;
            if r.status().is_success() && let Ok(c) = r.json::<OAuthClient>().await && !c.client_id.is_empty() { ci = c; }
        }
        let (state, verifier) = (new_key(), format!("{}{}", new_key(), new_key()));
        let mut auth_url = Url::parse(&meta.authorization_endpoint).map_err(|_| ConnectionError::new(502, "The sign-in page must use HTTPS."))?;
        if auth_url.scheme() != "https" || !auth_url.username().is_empty() || auth_url.password().is_some() {
            return Err(ConnectionError::new(502, "The sign-in page must use HTTPS."));
        }
        {
            let mut p = auth_url.query_pairs_mut();
            p.append_pair("response_type", "code").append_pair("client_id", &ci.client_id).append_pair("redirect_uri", &redirect)
                .append_pair("state", &state).append_pair("code_challenge", &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))).append_pair("code_challenge_method", "S256");
            if let Some(sc) = &scope { p.append_pair("scope", sc); }
        }
        let flow = Flow { app: app.clone(), redirect, expires: now_ms() + 10 * 60_000, state: state.clone(), scope, done: false, finishing: false, client: Some(ci), discovery: Some(meta), verifier, tokens: None, expiry: None };
        if let Ok(mut map) = self.flows.lock() { map.insert(state.clone(), flow); }
        Ok(json!({ "attempt": state, "opens": auth_url.as_str(), "title": format!("Sign in to {}", app.title), "callbackLabel": "Paste the address the sign-in page ends on", "submit": "Finish Sign-in" }))
    }

    fn active_flow(&self, app: &InstalledApp, attempt: &str) -> Result<Flow, ConnectionError> {
        self.flows.lock().map_err(|_| ConnectionError::new(500, "lock poisoned"))?
            .get(attempt).filter(|f| f.app.id == app.id && f.app.url == app.url && f.expires > now_ms()).cloned()
            .ok_or_else(|| ConnectionError::new(404, "That sign-in has ended. Start again."))
    }

    pub fn sign_in_status(&self, app: &InstalledApp, attempt: &str) -> Result<Value, ConnectionError> {
        Ok(json!({ "state": if self.active_flow(app, attempt)?.done { "connected" } else { "waiting" } }))
    }

    pub fn cancel_sign_in(&self, app: &InstalledApp, attempt: &str) -> Result<(), ConnectionError> {
        let mut g = self.flows.lock().map_err(|_| ConnectionError::new(500, "lock poisoned"))?;
        let f = g.get(attempt).filter(|f| f.app.id == app.id && f.app.url == app.url && f.expires > now_ms())
            .ok_or_else(|| ConnectionError::new(404, "That sign-in has ended. Start again."))?;
        if f.finishing { return Err(ConnectionError::new(409, "This sign-in is finishing.")); }
        g.remove(attempt);
        Ok(())
    }

    pub async fn finish_sign_in(&self, app: &InstalledApp, attempt: &str, address: &str) -> Result<(), ConnectionError> {
        let mut flow = self.active_flow(app, attempt)?;
        let url = Url::parse(address).map_err(|_| ConnectionError::new(400, "Paste the full address from the sign-in page."))?;
        let cb = Url::parse(&flow.redirect).map_err(|_| ConnectionError::new(400, "That address doesn’t match this sign-in."))?;
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let code = q.get("code").map(String::as_str).unwrap_or("");
        if url.origin() != cb.origin() || url.path() != cb.path() || !url.username().is_empty() || url.password().is_some()
            || q.get("state").map(String::as_str) != Some(flow.state.as_str()) || code.is_empty() || code.len() > 8192
        { return Err(ConnectionError::new(400, "That address doesn’t match this sign-in.")); }
        if flow.done { return Ok(()); }
        if let Ok(mut g) = self.flows.lock() && let Some(f) = g.get_mut(attempt) { f.finishing = true; }
        let (meta, ci) = (flow.discovery.as_ref().ok_or_else(|| ConnectionError::new(502, "Start sign-in again."))?, flow.client.as_ref().ok_or_else(|| ConnectionError::new(502, "Start sign-in again."))?);
        let mut form = vec![("grant_type", "authorization_code".into()), ("code", code.into()), ("redirect_uri", flow.redirect.clone()), ("client_id", ci.client_id.clone()), ("code_verifier", flow.verifier.clone())];
        if let Some(sec) = &ci.client_secret { form.push(("client_secret", sec.clone())); }
        let tok: Option<OAuthTokens> = match self.https_fetch(self.client.post(&meta.token_endpoint).form(&form), &meta.token_endpoint).await {
            Ok(r) if r.status().is_success() => r.json().await.ok(),
            _ => None,
        };
        let Some(tokens) = tok.filter(|t| !t.access_token.is_empty()) else {
            if let Ok(mut g) = self.flows.lock() && let Some(f) = g.get_mut(attempt) { f.finishing = false; }
            return Err(ConnectionError::new(502, "The service didn’t return a token."));
        };
        if !self.installations.read().ok().is_some_and(|st| st.apps.iter().any(|a| a.id == app.id && a.url == app.url)) {
            return Err(ConnectionError::new(409, "Add the app again before connecting it."));
        }
        flow.expiry = tokens.expires_in.map(|s| now_ms() + s * 1000);
        flow.tokens = Some(tokens);
        let stored = StoredOAuth { tokens: flow.tokens.clone(), expiry: flow.expiry, client: flow.client.clone(), discovery: flow.discovery.clone(), redirect: Some(flow.redirect.clone()), scope: flow.scope.clone() };
        self.keep_credential(app, BTreeMap::from([("oauth".into(), serde_json::to_string(&stored).unwrap_or_default())]))?;
        if let Ok(mut g) = self.flows.lock() && let Some(f) = g.get_mut(attempt) { f.done = true; f.finishing = false; }
        Ok(())
    }

    async fn access(&self, app: &InstalledApp) -> Result<String, ConnectionError> {
        let name = credentials_name(app);
        let vals = self.vault.with_vault(|v| {
            if v.paused_for(None, None)?.is_some() { return Err(crate::vault::Error::refused(403, "Connect this app in Marketplace.")); }
            let item = v.named(&name)?.ok_or_else(|| crate::vault::Error::refused(403, "Connect this app in Marketplace."))?;
            Ok(v.values(&item.id)?.into_iter().map(|(k, s)| (k, s.expose().to_owned())).collect::<BTreeMap<_, _>>())
        }).map_err(|e| ConnectionError::new(e.status, e.message.into_owned()))?;
        if app.auth == "token" && let Some(tok) = vals.get("token").filter(|s| !s.is_empty()) { return Ok(tok.clone()); }
        let mut st: StoredOAuth = serde_json::from_str(vals.get("oauth").map(String::as_str).unwrap_or("{}")).unwrap_or_default();
        if let Some(t) = st.tokens.as_ref().filter(|t| !t.access_token.is_empty()) && st.expiry.is_none_or(|e| e > now_ms() + 30_000) {
            return Ok(t.access_token.clone());
        }
        let err = || ConnectionError::new(403, "Sign in to this app again.");
        let (refresh, meta, ci) = (st.tokens.as_ref().and_then(|t| t.refresh_token.clone()).ok_or_else(err)?, st.discovery.as_ref().ok_or_else(err)?, st.client.as_ref().ok_or_else(err)?);
        let mut form = vec![("grant_type", "refresh_token".into()), ("refresh_token", refresh), ("client_id", ci.client_id.clone())];
        if let Some(sec) = &ci.client_secret { form.push(("client_secret", sec.clone())); }
        let r = self.https_fetch(self.client.post(&meta.token_endpoint).form(&form), &meta.token_endpoint).await?;
        if !r.status().is_success() { return Err(err()); }
        let tokens: OAuthTokens = r.json().await.map_err(|_| err())?;
        if tokens.access_token.is_empty() { return Err(err()); }
        let tok = tokens.access_token.clone();
        st.expiry = tokens.expires_in.map(|s| now_ms() + s * 1000);
        st.tokens = Some(tokens);
        self.keep_credential(app, BTreeMap::from([("oauth".into(), serde_json::to_string(&st).unwrap_or_default())]))?;
        Ok(tok)
    }

    pub fn project_apps(&self, apps: Option<&[InstalledApp]>) -> Vec<String> {
        let owned;
        let apps = match apps { Some(a) => a, None => match self.installations.read() { Ok(s) => { owned = s.apps; &owned } Err(e) => return vec![e] } };
        if !self.config.owns_settings() {
            return if apps.is_empty() { Vec::new() } else { vec!["Update this computer’s box before using Marketplace apps.".into()] };
        }
        let _ = kuvo_store::files::create_private_dirs(&self.installations.marketplace_dir);
        let Ok(_g) = OwnedLock::acquire(&self.projection_lock, Duration::from_secs(15)) else { return vec!["Couldn’t lock Marketplace app projection.".into()]; };
        let settings = match self.config.read_settings() { SettingsRead::Ok(m) => m, SettingsRead::Problem(p) => return vec![p] };
        let current = self.relay.lock().ok().and_then(|g| g.clone()).or_else(|| self.stored_relay());
        let (before, pending) = fs::read_to_string(&self.app_index).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| (v.get("before").and_then(Value::as_object).cloned().unwrap_or_default(), v.get("pending").and_then(Value::as_object).cloned().unwrap_or_default()))
            .unwrap_or_default();
        let mut wanted = Map::new();
        for app in apps {
            if app.auth == "open" {
                wanted.insert(server_name(&app.id), json!({ "type": "remote", "url": app.url, "oauth": false, "codemode": false }));
            } else if let Some(rel) = &current && self.connection_state(app) == "ready" {
                wanted.insert(server_name(&app.id), json!({ "type": "remote", "oauth": false, "codemode": false, "url": format!("http://127.0.0.1:{}/marketplace/mcp/{}", rel.port, app.id), "headers": { "Authorization": format!("Bearer {}", app_key(&rel.key, app)) } }));
            }
        }
        let same = |a: Option<&Value>, b: Option<&Value>| match (a, b) { (None, None) => true, (Some(x), Some(y)) => sorted_json(x) == sorted_json(y), _ => false };
        let names: HashSet<String> = before.keys().chain(pending.keys()).chain(wanted.keys()).cloned().collect();
        let (mut changes, mut problems) = (Vec::new(), Vec::new());
        for name in names {
            if !name.starts_with(MARKETPLACE_SERVER_PREFIX) { continue; }
            let (was, val) = (map_value_at(&settings, &[SERVERS[0], SERVERS[1], &name]), wanted.get(&name));
            if same(was, val) { continue; }
            if was.is_some() && !same(was, before.get(&name)) && !same(was, pending.get(&name)) {
                problems.push(format!("The MCP server {name} on this computer is already in use."));
                wanted.shift_remove(&name);
                continue;
            }
            changes.push(json!({ "path": [SERVERS[0], SERVERS[1], name], "value": val.cloned().unwrap_or(Value::Null), "was": was.cloned().unwrap_or(Value::Null) }));
        }
        if !changes.is_empty() {
            let confirmed: Map<String, Value> = before.keys().chain(pending.keys()).filter_map(|n| {
                let now = map_value_at(&settings, &[SERVERS[0], SERVERS[1], n])?;
                (same(Some(now), before.get(n)) || same(Some(now), pending.get(n))).then(|| (n.clone(), now.clone()))
            }).collect();
            let _ = kuvo_store::files::write_atomic(&self.app_index, &serde_json::to_vec(&json!({ "before": confirmed, "pending": wanted })).unwrap_or_default());
            if let SettingsChanged::Err { message, .. } = self.config.change_settings(&Value::Array(changes)) {
                problems.push(message);
                return problems;
            }
        }
        if !pending.is_empty() || sorted_json(&Value::Object(wanted.clone())) != sorted_json(&Value::Object(before)) {
            let _ = kuvo_store::files::write_atomic(&self.app_index, &serde_json::to_vec(&json!({ "before": wanted })).unwrap_or_default());
        }
        problems
    }

    pub async fn start_relay(self: &Arc<Self>) -> Result<RelayServer, std::io::Error> {
        let before = self.stored_relay();
        let key = before.as_ref().map(|r| r.key.clone()).unwrap_or_else(new_key);
        let listener = match before.as_ref().map(|r| r.port) {
            Some(p) => match TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], p))).await {
                Ok(l) => l,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?,
                Err(e) => return Err(e),
            },
            None => TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?,
        };
        let port = listener.local_addr()?.port();
        let rec = RelayRecord { port, key };
        kuvo_store::files::write_atomic(&self.endpoint_file, &serde_json::to_vec(&rec).map_err(std::io::Error::other)?)?;
        if let Ok(mut g) = self.relay.lock() { *g = Some(rec); }
        let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
        let me = self.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    acc = listener.accept() => {
                        let Ok((stream, _)) = acc else { break };
                        let conn = me.clone();
                        tokio::spawn(async move {
                            let svc = service_fn(move |req| { let c = conn.clone(); async move { Ok::<_, std::convert::Infallible>(c.handle_relay_request(req).await) } });
                            let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
                        });
                    }
                }
            }
            if let Ok(mut g) = me.relay.lock() && g.as_ref().is_some_and(|r| r.port == port) { *g = None; }
        });
        Ok(RelayServer { stop: Some(stop_tx), task })
    }

    async fn handle_relay_request(&self, req: Request<Incoming>) -> Response<Full<Bytes>> {
        let Some(rec) = self.relay.lock().ok().and_then(|g| g.clone()) else {
            return plain(StatusCode::SERVICE_UNAVAILABLE, "This computer couldn’t load its Marketplace connections.");
        };
        let host = format!("127.0.0.1:{}", rec.port);
        let host_ok = req.headers().get("host").and_then(|v| v.to_str().ok()) == Some(host.as_str());
        let (path, query) = (req.uri().path().to_owned(), req.uri().query().map(str::to_owned));
        if path == CALLBACK_PATH && req.method() == Method::GET {
            let state = query.as_deref().and_then(|q| form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == "state").map(|(_, v)| v.into_owned())).unwrap_or_default();
            let flow = self.flows.lock().ok().and_then(|m| m.get(&state).cloned());
            if host_ok && let Some(flow) = flow {
                let full = format!("http://{host}{path}{}", query.as_ref().map(|q| format!("?{q}")).unwrap_or_default());
                if self.finish_sign_in(&flow.app, &flow.state, &full).await.is_ok() {
                    let _ = self.project_apps(None);
                    let mut r = plain(StatusCode::OK, "Connected. Return to Kuvo Code.");
                    r.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
                    r.headers_mut().insert("referrer-policy", HeaderValue::from_static("no-referrer"));
                    return r;
                }
            }
            return plain(StatusCode::BAD_REQUEST, "The sign-in did not finish. Return to Kuvo Code.");
        }
        let Some(id) = path.strip_prefix("/marketplace/mcp/").filter(|s| !s.is_empty() && !s.contains('/')) else { return empty_status(StatusCode::NOT_FOUND); };
        let Some(app) = self.installations.read().ok().and_then(|st| st.apps.into_iter().find(|a| a.id == id && a.auth != "open")) else { return empty_status(StatusCode::NOT_FOUND); };
        let expected = format!("Bearer {}", app_key(&rec.key, &app));
        let given = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
        if !host_ok || query.is_some() || given.len() != expected.len() || given.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() != 1 {
            return empty_status(StatusCode::NOT_FOUND);
        }
        let method = req.method().clone();
        if !matches!(method, Method::GET | Method::POST | Method::DELETE) { return empty_status(StatusCode::METHOD_NOT_ALLOWED); }
        let mut fwd = HeaderMap::new();
        for n in ["accept", "content-type", "mcp-session-id", "mcp-protocol-version", "last-event-id"] {
            if let Some(v) = req.headers().get(n) { fwd.insert(n, v.clone()); }
        }
        let Ok(token) = self.access(&app).await else { return plain(StatusCode::FORBIDDEN, "Connect this app in Marketplace or unlock this computer’s vault."); };
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}")) { fwd.insert("authorization", v); }
        let Ok(body) = req.into_body().collect().await.map(|c| c.to_bytes()) else { return empty_status(StatusCode::BAD_REQUEST); };
        if body.len() > MAX_RELAY_BODY { return empty_status(StatusCode::PAYLOAD_TOO_LARGE); }
        let mut up = self.client.request(method.clone(), &app.url).headers(fwd);
        if method == Method::POST { up = up.body(body); }
        let Ok(upstream) = up.send().await else { return plain(StatusCode::BAD_GATEWAY, "The app could not be reached."); };
        let status = upstream.status();
        let kept_headers: Vec<_> = ["content-type", "mcp-session-id", "mcp-protocol-version"].into_iter()
            .filter_map(|n| Some((n, upstream.headers().get(n)?.clone()))).collect();
        let mut resp = Response::new(Full::new(upstream.bytes().await.unwrap_or_default()));
        *resp.status_mut() = status;
        for (n, v) in kept_headers { resp.headers_mut().insert(n, v); }
        resp
    }
}

pub struct RelayServer { stop: Option<oneshot::Sender<()>>, task: tokio::task::JoinHandle<()> }
impl RelayServer {
    pub async fn stop(mut self) {
        if let Some(tx) = self.stop.take() { let _ = tx.send(()); }
        let _ = self.task.await;
    }
}

fn plain(status: StatusCode, text: &'static str) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from_static(text.as_bytes())));
    *r.status_mut() = status;
    r.headers_mut().insert("content-type", HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}
fn empty_status(status: StatusCode) -> Response<Full<Bytes>> { let mut r = Response::new(Full::new(Bytes::new())); *r.status_mut() = status; r }
fn is_cred_name(name: &str) -> bool {
    name.strip_prefix("Marketplace · ").and_then(|r| r.rsplit_once(" · ")).is_some_and(|(id, h)| {
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && h.len() == 16 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}
fn parse_www_auth(h: &str) -> (Option<String>, Option<String>) {
    let get = |k: &str| {
        let p = format!("{k}=\"");
        let s = h.find(&p)? + p.len();
        Some(h[s..s + h[s..].find('"')?].to_owned())
    };
    (get("resource_metadata"), get("scope"))
}
