//! This Mac's identity stays in Kuvo Helper. Prove the helper at its rendezvous
//! before presenting its password, and keep the keys only in resident memory.

use std::{env, net::IpAddr, time::Duration};

use hmac::{Hmac, Mac};
use reqwest::{Client, Response, Url};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;

use crate::{Error, model::Link, random, storage::Files};

const VERSION_HEADER: &str = "x-kuvo-host-version";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Host {
    #[serde(default)]
    format_version: u8,
    url: String,
    password: String,
}

#[derive(Deserialize)]
struct Configuration {
    phones: bool,
    relay: String,
    name: Option<String>,
    #[serde(default)]
    own: bool,
}

#[derive(Deserialize)]
pub(crate) struct Keys {
    key: String,
    sign: String,
    secret: String,
}

pub(crate) struct Overlay {
    keys: Keys,
    relay: Option<String>,
    name: String,
    devices_disabled: bool,
}

impl Overlay {
    pub fn apply(&self, link: &mut Link) {
        link.key = Some(self.keys.key.clone());
        link.sign = Some(self.keys.sign.clone());
        link.secret = Some(self.keys.secret.clone());
        link.relay.clone_from(&self.relay);
        link.name = Some(self.name.clone());
        link.devices_disabled = self.devices_disabled;
    }
}

pub(crate) fn device_connections_disabled(files: &Files) -> Result<bool, Error> {
    Ok(files
        .read::<Configuration>("run/mac-link.json")?
        .is_some_and(|config| !config.phones))
}

fn version(response: Response) -> Result<Response, Error> {
    if response
        .headers()
        .get(VERSION_HEADER)
        .is_some_and(|version| version != "1")
    {
        return Err(Error::Version("Mac helper response"));
    }
    Ok(response)
}

pub(crate) async fn read(
    files: &Files,
    name: &str,
    relay: Option<String>,
) -> Result<Option<Overlay>, Error> {
    let Some(host) = Session::connect(files).await? else {
        return Ok(None);
    };
    let config = files.read::<Configuration>("run/mac-link.json")?;
    let device_on = config
        .as_ref()
        .is_none_or(|configuration| configuration.phones);
    let configured = config.as_ref().filter(|configuration| !configuration.own);
    let relay = configured
        .map(|configuration| configuration.relay.clone())
        .or(relay);
    let name = configured
        .and_then(|configuration| configuration.name.clone())
        .unwrap_or_else(|| name.into());
    let keys: Keys = host.ask("/link/identity", json!({})).await?;
    Ok(Some(Overlay {
        keys,
        relay,
        name,
        devices_disabled: !device_on,
    }))
}

#[cfg(target_os = "macos")]
pub(crate) async fn clear_integration_keys(files: &Files) -> Result<(), Error> {
    let host = Session::connect(files)
        .await?
        .ok_or(Error::Invalid("Mac helper rendezvous"))?;
    #[derive(Deserialize)]
    struct KeysToClear {
        names: std::collections::BTreeSet<String>,
    }
    let first: KeysToClear = host.ask("/keys/names", json!({})).await?;
    if first.names.iter().any(|name| {
        !name.strip_prefix("KUVO_KEY_").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|part| part.is_ascii_uppercase() || part.is_ascii_digit() || part == b'_')
        })
    }) {
        return Err(Error::Invalid("Mac helper key names"));
    }
    for name in first.names {
        let _: serde_json::Value = host.ask("/keys/delete", json!({"name": name})).await?;
    }
    let remaining: KeysToClear = host.ask("/keys/names", json!({})).await?;
    if !remaining.names.is_empty() {
        return Err(Error::Installer(
            "The host still holds Kuvo integration keys.".into(),
        ));
    }
    Ok(())
}

struct Session {
    client: Client,
    url: Url,
    password: String,
}

impl Session {
    async fn connect(files: &Files) -> Result<Option<Self>, Error> {
        let host = files.read::<Host>("run/host.json")?.or_else(|| {
            Some(Host {
                format_version: 0,
                url: env::var("KUVO_HOST_URL").ok()?,
                password: env::var("KUVO_HOST_PASSWORD").ok()?,
            })
        });
        let Some(host) = host else { return Ok(None) };
        if host.format_version > 1 {
            return Err(Error::Version("Mac helper rendezvous"));
        }
        let mut url = Url::parse(&host.url).map_err(|_| Error::Invalid("Mac helper address"))?;
        let address = url
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok());
        if url.scheme() != "http"
            || !address.is_some_and(|ip| ip.is_loopback())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || host.password.is_empty()
        {
            return Err(Error::Invalid("Mac helper rendezvous"));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?;
        let nonce = hex::encode(random::<32>()?);
        let target = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
        url.set_path("/hello");
        let hello = client
            .post(url.clone())
            .header(VERSION_HEADER, "1")
            .json(&json!({"nonce": nonce}))
            .send()
            .await?;
        #[derive(Deserialize)]
        struct Hello {
            proof: String,
        }
        let hello: Hello = version(hello)?.error_for_status()?.json().await?;
        let proof = hex::decode(hello.proof).map_err(|_| Error::Invalid("Mac helper proof"))?;
        let mut want = Hmac::<Sha256>::new_from_slice(host.password.as_bytes())
            .expect("HMAC accepts any key length");
        want.update(format!("kuvo-host-hello\0{nonce}\0{target}").as_bytes());
        want.verify_slice(&proof)
            .map_err(|_| Error::Invalid("Mac helper proof"))?;

        Ok(Some(Self {
            client,
            url,
            password: host.password,
        }))
    }

    async fn ask<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        input: serde_json::Value,
    ) -> Result<T, Error> {
        let mut url = self.url.clone();
        url.set_path(path);
        let answer = self
            .client
            .post(url)
            .timeout(Duration::from_secs(120))
            .header(VERSION_HEADER, "1")
            .bearer_auth(&self.password)
            .json(&input)
            .send()
            .await?;
        Ok(version(answer)?.error_for_status()?.json().await?)
    }
}
