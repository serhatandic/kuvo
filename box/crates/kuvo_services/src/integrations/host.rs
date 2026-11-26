//! Mac host interface (PROTOCOL.md): authenticated loopback calls for Keychain keys and site sign-ins.

use std::{path::{Path, PathBuf}, sync::Mutex, time::Duration};

use hmac::{Hmac, Mac};
use kuvo_store::format::ObjectFormat;
use serde_json::{Map, Value, json};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use url::Url;

use crate::config::{app_key_files, box_env, is_own_home_mac};

type HmacSha256 = Hmac<Sha256>;
pub const HOST_VERSION_HEADER: &str = "x-kuvo-host-version";
pub const HOST_VERSION: &str = "1";
pub const NOT_ANSWERING: &str = "Kuvo Code isn't answering for this Mac's keys. Try again in a moment.";

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct KeysUnavailable(pub String);

#[derive(Debug, Clone)]
pub struct HostAddress { pub url: String, pub password: String, pub said: String }

pub struct HostInterface {
    run: PathBuf,
    keys_on_host: bool,
    own_home_mac: bool,
    writes_key_files: bool,
    launch_host: Option<(String, String)>,
    proven_by: Mutex<Option<String>>,
    client: reqwest::Client,
}

impl HostInterface {
    pub fn for_home(home: &Path, is_mac: bool) -> Self {
        let own_home_mac = is_own_home_mac(home, is_mac);
        let (lu, lp) = (box_env(home, "KUVO_HOST_URL"), box_env(home, "KUVO_HOST_PASSWORD"));
        let keys_on_host = box_env(home, "KUVO_KEYS_ON_HOST").as_deref() == Some("1") || lu.is_some() || own_home_mac;
        Self {
            run: home.join("run"), keys_on_host, own_home_mac,
            writes_key_files: own_home_mac && keys_on_host && app_key_files(home).is_none(),
            launch_host: lu.zip(lp).map(|(u, p)| (u.trim_end_matches('/').to_owned(), p)),
            proven_by: Mutex::new(None),
            client: reqwest::Client::builder().no_proxy().build().unwrap_or_default(),
        }
    }

    pub fn keys_on_host(&self) -> bool { self.keys_on_host }
    pub fn writes_key_files(&self) -> bool { self.writes_key_files }

    pub fn app_host(&self) -> Result<Option<HostAddress>, KeysUnavailable> {
        if !self.keys_on_host { return Ok(None); }
        let file = self.run.join("host.json");
        if file.exists() {
            let unreadable = || KeysUnavailable("The Mac host rendezvous is unreadable.".into());
            let rec = ObjectFormat::additive("Mac host rendezvous").read(&file).map_err(|e| match e {
                kuvo_store::format::Error::Unsupported { .. } => KeysUnavailable(e.to_string()),
                _ => unreadable(),
            })?.ok_or_else(unreadable)?;
            let url = rec.fields.get("url").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(unreadable)?;
            let pw = rec.fields.get("password").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(unreadable)?;
            return Ok(Some(HostAddress { url: url.trim_end_matches('/').to_owned(), password: pw.into(), said: serde_json::to_string(&[url, pw]).unwrap_or_default() }));
        }
        Ok(self.launch_host.as_ref().map(|(u, p)| HostAddress { url: u.clone(), password: p.clone(), said: "launch".into() }))
    }

    pub fn open_app_host(&self) -> Option<HostAddress> {
        if !self.own_home_mac { return self.app_host().ok().flatten(); }
        let host = crate::mac_app::read(&self.run)?.host?;
        let said = format!("app {} {}", host.url, host.password);
        Some(HostAddress { url: host.url, password: host.password, said })
    }

    async fn prove_host(&self, host: &HostAddress) -> Result<(), KeysUnavailable> {
        if !host.said.starts_with("app ") && self.proven_by.lock().ok().and_then(|g| g.clone()).as_deref() == Some(host.said.as_str()) { return Ok(()); }
        let mut nb = [0u8; 32];
        getrandom::fill(&mut nb).map_err(|_| KeysUnavailable(NOT_ANSWERING.into()))?;
        let nonce = hex::encode(nb);
        let resp = self.client.post(format!("{}/hello", host.url)).header(HOST_VERSION_HEADER, HOST_VERSION)
            .timeout(Duration::from_secs(15)).json(&json!({ "nonce": nonce })).send().await.map_err(|_| KeysUnavailable(NOT_ANSWERING.into()))?;
        check_version(&resp)?;
        let ok = resp.status().is_success();
        let body = resp.json::<Value>().await.ok();
        let proof = ok.then_some(()).and_then(|()| body.as_ref()?.get("proof")?.as_str()).ok_or_else(|| KeysUnavailable(NOT_ANSWERING.into()))?;
        let exp = hello_proof(&host.password, &nonce, &host.url).ok_or_else(|| KeysUnavailable(NOT_ANSWERING.into()))?;
        if proof.len() != exp.len() || proof.as_bytes().ct_eq(exp.as_bytes()).unwrap_u8() != 1 { return Err(KeysUnavailable(NOT_ANSWERING.into())); }
        if !host.said.starts_with("app ") && let Ok(mut g) = self.proven_by.lock() { *g = Some(host.said.clone()); }
        Ok(())
    }

    pub async fn ask_host(&self, host: &HostAddress, call: &str, body: &Value, allow_missing: bool) -> Result<Option<Map<String, Value>>, KeysUnavailable> {
        self.prove_host(host).await?;
        let secs = if matches!(call, "keys/get" | "keys/set" | "link/identity" | "link/rotate" | "vault/get" | "vault/set") { 120 } else { 15 };
        let resp = match self.client.post(format!("{}/{call}", host.url)).header(HOST_VERSION_HEADER, HOST_VERSION)
            .header("authorization", format!("Bearer {}", host.password)).timeout(Duration::from_secs(secs)).json(body).send().await
        {
            Ok(r) => r,
            Err(_) => { if let Ok(mut g) = self.proven_by.lock() { *g = None; } return Err(KeysUnavailable(NOT_ANSWERING.into())); }
        };
        check_version(&resp)?;
        let status = resp.status();
        if allow_missing && status.as_u16() == 404 { return Ok(None); }
        let answer = resp.json::<Value>().await.ok();
        if !status.is_success() {
            let said = answer.as_ref().and_then(|v| v.get("message")?.as_str())
                .map(|m| m.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty())
                .map(|m| format!(" {}", m.chars().take(300).collect::<String>())).unwrap_or_default();
            return Err(KeysUnavailable(format!("This Mac's keys couldn't be reached ({}).{said}", status.as_u16())));
        }
        Ok(Some(answer.and_then(|v| v.as_object().cloned()).unwrap_or_default()))
    }

    pub async fn open_browser_app(&self, bundle: &str) -> Result<(), String> {
        let couldnt = |why: &str| format!("Kuvo Code couldn't be opened in the background on this Mac for its browser ({why}). Ask the user to open Kuvo Code, then try again.");
        let missing = || couldnt("nothing that opens it is running on this Mac");
        let host = self.app_host().ok().flatten().ok_or_else(missing)?;
        self.prove_host(&host).await.map_err(|_| missing())?;
        let response = self.client.post(format!("{}/app/open", host.url)).bearer_auth(&host.password)
            .timeout(Duration::from_secs(25)).json(&json!({"bundle":bundle})).send().await
            .map_err(|e| if e.is_timeout() {couldnt("macOS didn't open it in time")} else {missing()})?;
        let status = response.status();
        if status.is_success() {return Ok(());}
        if status.as_u16() == 410 {return Err("Kuvo Code isn't where it was installed on this Mac any more (it was moved or deleted), so its browser can't be opened. Ask the user to open Kuvo Code, then try again.".into());}
        if status.as_u16() == 404 {return Err(couldnt("Kuvo on this Mac can't open it yet"));}
        let response = response.json::<Value>().await.ok();
        let why = response.as_ref().and_then(|v|v["message"].as_str()).filter(|s|!s.trim().is_empty()).map(|s|s.chars().take(300).collect::<String>()).unwrap_or_else(||format!("it said {}",status.as_u16()));
        Err(couldnt(&why))
    }

    pub async fn site_sign_ins(&self) -> Option<Map<String, Value>> {
        self.ask_host(&self.open_app_host()?, "sites/signed-in", &json!({}), false).await.ok()??.get("sites")?.as_object().cloned()
    }
}

fn check_version(resp: &reqwest::Response) -> Result<(), KeysUnavailable> {
    if resp.headers().get(HOST_VERSION_HEADER).is_some_and(|v| v.to_str().ok() != Some(HOST_VERSION)) {
        return Err(KeysUnavailable("The Mac host uses an unsupported format.".into()));
    }
    Ok(())
}
fn hello_proof(password: &str, nonce: &str, url_str: &str) -> Option<String> {
    let u = Url::parse(url_str).ok()?;
    let mut mac = HmacSha256::new_from_slice(password.as_bytes()).ok()?;
    mac.update(format!("kuvo-host-hello\0{nonce}\0{}:{}", u.host_str()?, u.port().unwrap_or(80)).as_bytes());
    Some(hex::encode(mac.finalize().into_bytes()))
}
