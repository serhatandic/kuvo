use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, VerifyingKey};
use kuvo_server::{Caller, DispatchRequest, LocalOrigin, Transport};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{Vault, helper::millis};

const PROOF_FOR: u64 = 300_000;

pub(super) struct Clients {
    link: PathBuf,
    nonces: HashMap<String, u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Link {
    id: String,
    #[serde(default)]
    macs: Vec<String>,
    #[serde(default)]
    mac_signs: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Proof {
    mac: String,
    at: serde_json::Value,
    nonce: String,
    proof: String,
}

impl Clients {
    pub fn new(home: &Path) -> Self {
        Self {
            link: home.join("run/link.json"),
            nonces: HashMap::new(),
        }
    }

    pub fn client(&mut self, vault: &mut Vault, request: &DispatchRequest) -> Option<String> {
        if matches!(
            request.caller,
            Caller::Local(Transport::Http {
                origin: LocalOrigin::App | LocalOrigin::VaultHolder,
                ..
            })
        ) {
            return Some("mac".into());
        }
        if let Some(proof) = request
            .headers
            .get("x-kuvo-vault-proof")
            .and_then(|header| {
                URL_SAFE_NO_PAD
                    .decode(header.as_bytes())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Proof>(&bytes).ok())
            })
        {
            let hash = hex::encode(Sha256::digest(&request.body));
            let rest = format!("\0{}\0{}\0{hash}", request.method, request.path);
            if let Some(mac) = self.proved(proof, "kuvo-vault-request", &rest) {
                return Some(format!("mac:{mac}"));
            }
        }
        request
            .headers
            .get("x-kuvo-vault-key")
            .and_then(|header| header.to_str().ok())
            .filter(|key| vault.unlock(key))
            .map(|_| "mac".into())
    }

    pub fn key_request(&mut self, given: serde_json::Map<String, serde_json::Value>) -> bool {
        serde_json::from_value(serde_json::Value::Object(given))
            .ok()
            .and_then(|proof| self.proved(proof, "kuvo-vault-key", ""))
            .is_some()
    }

    fn proved(&mut self, proof: Proof, tag: &str, rest: &str) -> Option<String> {
        if !(16..=64).contains(&proof.nonce.len())
            || !proof
                .nonce
                .bytes()
                .all(|char| char.is_ascii_alphanumeric() || matches!(char, b'-' | b'_'))
        {
            return None;
        }
        let at = proof
            .at
            .as_u64()
            .or_else(|| proof.at.as_str()?.parse().ok())?;
        let now = millis();
        if now.abs_diff(at) > PROOF_FOR {
            return None;
        }
        self.nonces.retain(|_, until| *until >= now);
        if self.nonces.contains_key(&proof.nonce) {
            return None;
        }
        let link: Link = serde_json::from_slice(&fs::read(&self.link).ok()?).ok()?;
        if !link.macs.contains(&proof.mac) {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(link.mac_signs.get(&proof.mac)?)
            .ok()?;
        let key = VerifyingKey::from_bytes(&bytes.try_into().ok()?).ok()?;
        let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(proof.proof).ok()?).ok()?;
        let message = format!("{tag}\0{}\0{at}\0{}{rest}", link.id, proof.nonce);
        key.verify_strict(message.as_bytes(), &signature).ok()?;
        self.nonces.insert(proof.nonce, now + PROOF_FOR * 2);
        Some(proof.mac)
    }
}
