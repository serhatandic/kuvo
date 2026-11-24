//! Client for the unchanged Kuvo Helper host protocol. This runs on blocking
//! workers, including when called through the backup service's synchronous seam.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{
    Url,
    blocking::{Client, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{
    Error, File, Generation, Result, Secret, Signed, Values, check_id,
    crypto::{self, Key},
};

const REVISION: &str = "x-kuvo-host-version";
const UNAVAILABLE: &str = "Kuvo Code isn't answering for this Mac's keys. Try again in a moment.";
const STARTING: &str = "This Mac's vault is starting. Try again in a moment.";
const UNKEPT: &str =
    "This Mac's vault couldn't keep what it changed last in the Keychain. Try again in a moment.";

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    #[serde(default)]
    format_version: u8,
    #[serde(default)]
    password: Option<Secret>,
    #[serde(default)]
    host_vault_secret: Option<Secret>,
    #[serde(default)]
    vault_file_key: Option<Secret>,
}

impl Launch {
    pub fn read(reader: impl BufRead) -> std::io::Result<Self> {
        let mut bytes = Zeroizing::new(Vec::new());
        reader.take(16 * 1024).read_until(b'\n', &mut bytes)?;
        let launch: Self = serde_json::from_slice(&bytes)
            .map_err(|_| std::io::Error::other("The Mac Helper's launch message is unreadable."))?;
        if launch.format_version > 1 {
            return Err(std::io::Error::other(
                "The Mac Helper's launch message uses an unsupported format.",
            ));
        }
        if launch
            .password
            .as_ref()
            .is_none_or(|password| password.expose().is_empty())
        {
            return Err(std::io::Error::other(
                "The Mac Helper didn't send the resident's password.",
            ));
        }
        Ok(launch)
    }

    /// The loopback client's password is published in the private rendezvous.
    pub fn take_password(&mut self) -> Option<String> {
        self.password
            .take()
            .map(|password| password.expose().to_owned())
    }
}

pub(super) struct Keychain {
    address: PathBuf,
    secret: Option<Secret>,
    key: Option<Key>,
    priming: Priming,
    unkept: BTreeSet<File>,
}

enum Priming {
    Waiting,
    Failed(Error),
    Ready,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Host {
    #[serde(default)]
    format_version: u8,
    url: Url,
    password: Secret,
}

#[derive(Default, Deserialize)]
struct Generations {
    #[serde(default)]
    generations: BTreeMap<String, Generation>,
    #[serde(default)]
    adopt: bool,
}

impl Keychain {
    pub(super) fn new(home: &Path, launch: Launch) -> Self {
        let key = launch.vault_file_key.and_then(|key| {
            let mut bytes = Zeroizing::new([0; 33]);
            let decoded = URL_SAFE_NO_PAD
                .decode_slice(key.expose(), bytes.as_mut())
                .ok()?;
            if decoded != 32 {
                return None;
            }
            let mut key = Zeroizing::new([0; 32]);
            key.copy_from_slice(&bytes[..32]);
            Some(key)
        });
        Self {
            address: home.join("run/host.json"),
            secret: launch.host_vault_secret,
            key,
            priming: Priming::Waiting,
            unkept: BTreeSet::new(),
        }
    }

    pub(super) fn file_key(&self) -> Option<Key> {
        self.key.as_ref().map(|key| Zeroizing::new(**key))
    }

    pub(super) fn prime(&mut self, signed: &mut Signed) {
        if matches!(self.priming, Priming::Ready) {
            return;
        }
        let kept = match self.call::<Generations>("vault/generations", &json!({}), false) {
            Ok(kept) => kept.unwrap_or_default(),
            Err(error) => {
                self.priming = Priming::Failed(error);
                return;
            }
        };
        let on_disk = signed.generations();
        for file in File::ALL {
            let name = file.name();
            if let Some(generation) = kept.generations.get(name) {
                signed.remember(file, *generation);
            } else if kept.adopt && on_disk[name] > 0 {
                signed.remember(file, on_disk[name]);
                self.remember(file, on_disk[name]);
            } else if signed.exists(file) {
                signed.remember(file, 9_007_199_254_740_991);
            }
        }
        self.priming = Priming::Ready;
    }

    pub(super) fn remember(&mut self, file: File, generation: Generation) {
        match self.call::<Generations>(
            "vault/generations",
            &json!({"file": file.name(), "generation": generation}),
            false,
        ) {
            Ok(_) => {
                self.unkept.remove(&file);
            }
            Err(_) => {
                self.unkept.insert(file);
            }
        }
    }

    pub(super) fn waiting(&self) -> Option<&str> {
        match &self.priming {
            Priming::Waiting => Some(STARTING),
            Priming::Failed(error) => Some(&error.message),
            Priming::Ready if !self.unkept.is_empty() => Some(UNKEPT),
            Priming::Ready => None,
        }
    }

    pub(super) fn values(&self, id: &str) -> Result<Values> {
        check_id(id)?;
        #[derive(Default, Deserialize)]
        struct Answer {
            #[serde(default)]
            values: Values,
        }
        Ok(self
            .call::<Answer>("vault/get", &json!({"id": id}), true)?
            .unwrap_or_default()
            .values)
    }

    pub(super) fn put_values(&self, id: &str, values: &Values) -> Result<()> {
        check_id(id)?;
        #[derive(Serialize)]
        struct Given<'a> {
            id: &'a str,
            values: BTreeMap<&'a str, &'a str>,
        }
        let given = Given {
            id,
            values: values
                .iter()
                .map(|(field, secret)| (field.as_str(), secret.expose()))
                .collect(),
        };
        self.call::<Value>("vault/set", &given, false)?;
        Ok(())
    }

    pub(super) fn delete_values(&self, id: &str) -> Result<()> {
        check_id(id)?;
        self.call::<Value>("vault/delete", &json!({"id": id}), false)?;
        Ok(())
    }

    fn call<T: DeserializeOwned>(
        &self,
        name: &str,
        given: &impl Serialize,
        missing: bool,
    ) -> Result<Option<T>> {
        let secret = self.secret.as_ref().ok_or_else(|| {
            Error::refused(503, "This Mac's vault can only be reached by Kuvo running on this Mac, not by a one-off command.")
        })?;
        let record = Zeroizing::new(fs::read(&self.address).map_err(|_| unavailable())?);
        let mut host: Host = serde_json::from_slice(&record)
            .map_err(|_| Error::refused(503, "The Mac host rendezvous is unreadable."))?;
        if host.format_version > 1 {
            return Err(Error::refused(
                503,
                "The Mac host uses an unsupported format.",
            ));
        }
        if host.url.scheme() != "http"
            || host.url.host_str() != Some("127.0.0.1")
            || !host.url.username().is_empty()
            || host.url.password().is_some()
            || host.url.path() != "/"
            || host.url.query().is_some()
            || host.url.fragment().is_some()
            || host.password.expose().is_empty()
        {
            return Err(unavailable());
        }
        let target = format!(
            "127.0.0.1:{}",
            host.url.port_or_known_default().ok_or_else(unavailable)?
        );
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| unavailable())?;
        let nonce = hex::encode(&*crypto::random::<32>()?);
        host.url.set_path("/hello");
        let hello = version(
            client
                .post(host.url.clone())
                .header(REVISION, "1")
                .json(&json!({"nonce": nonce, "vault": true}))
                .send()
                .map_err(|_| unavailable())?,
        )?;
        let hello: Hello = hello
            .error_for_status()
            .map_err(|_| unavailable())?
            .json()
            .map_err(|_| unavailable())?;
        let proves = |tag: &str, key: &Secret, given: &str| {
            let expected = crypto::hmac(
                key.expose().as_bytes(),
                &[
                    tag.as_bytes(),
                    b"\0",
                    nonce.as_bytes(),
                    b"\0",
                    target.as_bytes(),
                ],
            );
            let proof = hex::encode(&*expected);
            bool::from(proof.as_bytes().ct_eq(given.as_bytes()))
        };
        // Never present the password or the request body to an unproved port.
        if !proves("kuvo-host-hello", &host.password, &hello.proof)
            || !proves("kuvo-host-vault", secret, &hello.vault_proof)
        {
            return Err(unavailable());
        }
        let mut body = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *body, given)
            .map_err(|_| Error::refused(503, "Couldn't encode the Mac vault request."))?;
        let digest = hex::encode(Sha256::digest(&body));
        let at = millis().to_string();
        let proof = hex::encode(&*crypto::hmac(
            secret.expose().as_bytes(),
            &[
                b"kuvo-host-vault-call\0",
                name.as_bytes(),
                b"\0",
                digest.as_bytes(),
                b"\0",
                at.as_bytes(),
            ],
        ));
        host.url.set_path(&format!("/{name}"));
        let mut request = client
            .post(host.url)
            .header(REVISION, "1")
            .bearer_auth(host.password.expose())
            .header("content-type", "application/json")
            .header("x-kuvo-vault-auth", proof)
            .header("x-kuvo-vault-at", at);
        if matches!(name, "vault/get" | "vault/set") {
            request = request.timeout(Duration::from_secs(120));
        }
        let response = version(
            request
                .body(bytes::Bytes::from_owner(SecretBody(body)))
                .send()
                .map_err(|_| unavailable())?,
        )?;
        let status = response.status().as_u16();
        if missing && status == 404 {
            return Ok(None);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        response
            .take(8 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if !(200..300).contains(&status) {
            #[derive(Deserialize)]
            struct Refusal {
                message: String,
            }
            let message = serde_json::from_slice::<Refusal>(&bytes)
                .ok()
                .map(|given| super::items::one_line(&given.message, 300))
                .filter(|message| !message.trim().is_empty());
            let mut error = format!("This Mac's keys couldn't be reached ({status}).");
            if let Some(message) = message {
                error.push(' ');
                error.push_str(&message);
            }
            return Err(Error::refused(503, error));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| Error::refused(503, "The Mac Helper's vault reply is unreadable."))
    }
}

struct SecretBody(Zeroizing<Vec<u8>>);

impl AsRef<[u8]> for SecretBody {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hello {
    proof: String,
    vault_proof: String,
}

fn unavailable() -> Error {
    Error::refused(503, UNAVAILABLE)
}

fn version(response: Response) -> Result<Response> {
    if response
        .headers()
        .get(REVISION)
        .is_some_and(|version| version != "1")
    {
        return Err(Error::refused(
            503,
            "The Mac host uses an unsupported format.",
        ));
    }
    Ok(response)
}

pub(super) fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
