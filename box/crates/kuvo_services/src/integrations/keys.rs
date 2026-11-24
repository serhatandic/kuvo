//! Integration keys (`KUVO_KEY_<ID>_<NAME>`): private files in `box/keys` or Mac host Keychain.

use std::{collections::BTreeSet, fs, path::{Path, PathBuf}, sync::Arc, time::Duration};

use http::StatusCode;
use kuvo_protocol::routes::{delete_kuvo_keys_name, get_kuvo_keys, get_kuvo_keys_name, put_kuvo_keys_name};
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use kuvo_store::locks::OwnedLock;
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use super::host::{HostAddress, HostInterface, KeysUnavailable, NOT_ANSWERING};

pub const FEATURES: &[&str] = &["keys"];

pub fn is_stored_key(name: &str) -> bool {
    name.strip_prefix("KUVO_KEY_").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
}
pub fn is_bot_key(name: &str) -> bool {
    name.split_once("__BOT_").is_some_and(|(h, t)| is_stored_key(h) && !t.is_empty() && t.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
}

pub struct KeyStore {
    box_keys: PathBuf,
    config_lock: PathBuf,
    key_files_lock: PathBuf,
    host: HostInterface,
    work_lock: AsyncMutex<()>,
}

impl KeyStore {
    pub fn for_home(home: &Path, is_mac: bool) -> Self {
        Self { box_keys: home.join("box/keys"), config_lock: home.join("run/.config.lock"), key_files_lock: home.join("run/.key-files.lock"), host: HostInterface::for_home(home, is_mac), work_lock: AsyncMutex::new(()) }
    }
    pub fn host(&self) -> &HostInterface { &self.host }

    fn key_host(&self) -> Result<Option<HostAddress>, KeysUnavailable> {
        let host = self.host.app_host()?;
        if self.host.keys_on_host() && host.is_none() { return Err(KeysUnavailable(NOT_ANSWERING.into())); }
        Ok(host)
    }
    fn check_name(&self, name: &str) -> Result<(), KeysUnavailable> {
        (is_stored_key(name) && !is_bot_key(name)).then_some(()).ok_or_else(|| KeysUnavailable("That isn't a key's name.".into()))
    }
    pub async fn key_work<T, Fut: std::future::Future<Output = T>>(&self, work: impl FnOnce() -> Fut) -> T {
        let _g = self.work_lock.lock().await;
        work().await
    }

    pub async fn key_names(&self) -> Result<Vec<String>, KeysUnavailable> {
        let raw: Vec<String> = if let Some(host) = self.key_host()? {
            let err = || KeysUnavailable("This Mac's keys couldn't be read.".into());
            self.host.ask_host(&host, "keys/names", &json!({}), false).await?.ok_or_else(err)?
                .get("names").and_then(Value::as_array).ok_or_else(err)?.iter().filter_map(Value::as_str).map(str::to_owned).collect()
        } else {
            kuvo_store::files::list_of(&self.box_keys).into_iter().filter_map(|os| os.into_string().ok()).collect()
        };
        Ok(raw.into_iter().filter(|n| is_stored_key(n) && !is_bot_key(n)).collect::<BTreeSet<_>>().into_iter().collect())
    }

    pub async fn read_key(&self, name: &str) -> Result<Option<String>, KeysUnavailable> {
        self.check_name(name)?;
        if let Some(host) = self.key_host()? {
            return Ok(self.host.ask_host(&host, "keys/get", &json!({ "name": name }), true).await?.and_then(|m| m.get("value")?.as_str().map(str::to_owned)));
        }
        Ok(kuvo_store::files::text_of(&self.box_keys.join(name)))
    }

    pub async fn write_key(&self, name: &str, value: &str) -> Result<(), KeysUnavailable> {
        self.check_name(name)?;
        if let Some(host) = self.key_host()? {
            let _l = self.host.writes_key_files().then(|| OwnedLock::acquire(&self.key_files_lock, Duration::from_secs(15)).map_err(|e| KeysUnavailable(e.to_string()))).transpose()?;
            self.host.ask_host(&host, "keys/set", &json!({ "name": name, "value": value }), false).await?;
            if self.host.writes_key_files() { self.write_key_file(name, value)?; }
            return Ok(());
        }
        self.write_key_file(name, value)
    }

    pub async fn delete_key(&self, name: &str) -> Result<(), KeysUnavailable> {
        self.check_name(name)?;
        if let Some(host) = self.key_host()? {
            let _l = self.host.writes_key_files().then(|| OwnedLock::acquire(&self.key_files_lock, Duration::from_secs(15)).map_err(|e| KeysUnavailable(e.to_string()))).transpose()?;
            self.host.ask_host(&host, "keys/delete", &json!({ "name": name }), false).await?;
            if self.host.writes_key_files() { self.remove_key_file(name)?; }
            return Ok(());
        }
        self.remove_key_file(name)
    }

    fn write_key_file(&self, name: &str, value: &str) -> Result<(), KeysUnavailable> {
        let _l = OwnedLock::acquire(&self.config_lock, Duration::from_secs(15)).map_err(|e| KeysUnavailable(e.to_string()))?;
        kuvo_store::files::write_secret(&self.box_keys.join(name), value.as_bytes()).map_err(|e| KeysUnavailable(e.to_string()))
    }
    fn remove_key_file(&self, name: &str) -> Result<(), KeysUnavailable> {
        let _l = OwnedLock::acquire(&self.config_lock, Duration::from_secs(15)).map_err(|e| KeysUnavailable(e.to_string()))?;
        match fs::remove_file(self.box_keys.join(name)) { Ok(()) => Ok(()), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), Err(e) => Err(KeysUnavailable(e.to_string())) }
    }
}

fn decode_key_param(raw: Option<&str>) -> Result<String, Refusal> {
    let name = percent_decode_str(raw.unwrap_or("")).decode_utf8().map(|s| s.into_owned()).unwrap_or_default();
    (is_stored_key(&name) && !name.contains("__")).then_some(name).ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "That isn't a key's name."))
}
fn unavail(err: KeysUnavailable) -> Refusal { Refusal::new(StatusCode::SERVICE_UNAVAILABLE, err.0) }

pub fn register(routes: &mut Registry, keys: Arc<KeyStore>) {
    let k = keys.clone();
    routes.register::<get_kuvo_keys::Spec, _, _>(Access::Any, move |_| {
        let k = k.clone();
        async move { Ok(RouteReply::json::<200>(get_kuvo_keys::Status200 { keys: k.key_names().await.map_err(unavail)?.into_iter().filter(|n| !n.contains("__")).collect() })) }
    });
    let k = keys.clone();
    routes.register::<get_kuvo_keys_name::Spec, _, _>(Access::Any, move |call| {
        let (k, raw) = (k.clone(), call.param("name").map(str::to_owned));
        async move {
            match k.read_key(&decode_key_param(raw.as_deref())?).await.map_err(unavail)? {
                Some(value) => Ok(RouteReply::json::<200>(get_kuvo_keys_name::Status200 { value })),
                None => Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))),
            }
        }
    });
    let k = keys.clone();
    routes.register::<put_kuvo_keys_name::Spec, _, _>(Access::Any, move |call| {
        let (k, raw, body) = (k.clone(), call.param("name").map(str::to_owned), call.request.body.clone());
        async move {
            let name = decode_key_param(raw.as_deref())?;
            let value = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v.get("value")?.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, r#"Send {"value": "…"}."#))?;
            k.key_work(|| k.write_key(&name, &value)).await.map_err(unavail)?;
            Ok(RouteReply::json::<200>(put_kuvo_keys_name::Status200 {}))
        }
    });
    routes.register::<delete_kuvo_keys_name::Spec, _, _>(Access::Any, move |call| {
        let (k, raw) = (keys.clone(), call.param("name").map(str::to_owned));
        async move {
            let name = decode_key_param(raw.as_deref())?;
            k.key_work(|| k.delete_key(&name)).await.map_err(unavail)?;
            Ok(RouteReply::json::<200>(delete_kuvo_keys_name::Status200 {}))
        }
    });
}
