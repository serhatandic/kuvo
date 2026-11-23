//! The installer code and the link records shared with the TypeScript box.

pub(crate) mod credentials;
mod join;
pub(crate) mod released;

use std::time::Duration;

use kuvo_store::home::Home;
use kuvo_wire::keys::{Host, decode, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Error,
    model::{ComputerKind, Entry, Link, Membership, Peer, SharedKind, safe_token},
};

pub use join::{Joined, cancellation_receipts, join, join_ssh};

pub fn enrolled_ssh_identity(home: &Home) -> Result<(String, String), Error> {
    let files = crate::storage::Files::new(home.root().to_owned());
    let link: Link = files.read(crate::actor::LINK)?.ok_or(Error::Conflict)?;
    if link.setup()?.role != crate::model::Role::Member {
        return Err(Error::Conflict);
    }
    let own = crate::model::Identity::read(&link)?.public;
    let path = files.path("machine-id");
    let machine = match std::fs::read_to_string(&path) {
        Ok(text) if text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit()) => text,
        Ok(_) => return Err(Error::Invalid("machine identity")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let identity = hex::encode(crate::random::<16>()?);
            files.write_bytes("machine-id", identity.as_bytes())?;
            identity
        }
        Err(source) => {
            return Err(Error::File {
                action: "reading",
                path,
                source,
            });
        }
    };
    Ok((own, machine))
}

pub(super) const INVITE_LIFETIME: u64 = 15 * 60_000;
pub(super) const CLAIM_LIFETIME: u64 = 24 * 60 * 60_000;
const PREFIX: &str = "kuvo-add-1.";
const CANCELLATION: &str = "kuvo-cancel-";
pub(crate) const RECOVERY_ROUTE: &str = "kuvo/setup/installer-recovery";

pub async fn apply_cancellation(home: &Home, receipt: &str) -> Result<String, Error> {
    #[derive(Deserialize)]
    struct LocalService {
        port: u16,
        password: String,
    }

    let files = crate::storage::Files::new(home.root().to_owned());
    let service = files
        .read::<LocalService>("run/serve.json")
        .ok()
        .flatten()
        .filter(|service| service.port != 0 && !service.password.is_empty())
        .ok_or_else(|| Error::Installer("Start Kuvo on the main computer and try again.".into()))?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?
        .post(format!(
            "http://127.0.0.1:{}/{RECOVERY_ROUTE}",
            service.port
        ))
        .basic_auth("opencode", Some(service.password))
        .header("x-kuvo-from", "mac")
        .header("x-kuvo-route-version", "1")
        .json(&json!({"receipt": receipt.trim()}))
        .send()
        .await?;
    let success = response.status().is_success();
    let body = response.json::<Value>().await?;
    let message = body["message"]
        .as_str()
        .unwrap_or("The roster retirement could not be confirmed.");
    if success {
        Ok(message.into())
    } else {
        Err(Error::Installer(message.into()))
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Cancellation {
    pub setup: String,
    pub invite: String,
    pub key: String,
    pub secret: String,
}

impl Cancellation {
    pub(crate) fn read(input: &str) -> Result<Self, Error> {
        const INVALID: &str = "Enter an installer cancellation receipt.";
        let (version, payload) = input
            .strip_prefix(CANCELLATION)
            .filter(|_| input.len() < 512)
            .and_then(|rest| rest.split_once('.'))
            .ok_or_else(|| Error::Installer(INVALID.into()))?;
        if version != "1" {
            let version = version
                .parse::<u64>()
                .ok()
                .filter(|_| version.len() <= 15 && version.bytes().all(|c| c.is_ascii_digit()))
                .map_or_else(|| "invalid".to_owned(), |n| n.to_string());
            return Err(Error::Installer(format!(
                "kuvo-cancel format version {version} is unsupported (this build writes 1)."
            )));
        }
        let receipt: Self = decode(payload)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| Error::Installer(INVALID.into()))?;
        if !receipt
            .setup
            .strip_prefix("set_")
            .is_some_and(|id| safe_token(id, 22, 22))
            || !safe_token(&receipt.invite, 43, 43)
            || !safe_token(&receipt.key, 43, 43)
            || !safe_token(&receipt.secret, 32, 32)
        {
            return Err(Error::Installer(INVALID.into()));
        }
        Ok(receipt)
    }

    fn encode(&self) -> String {
        format!(
            "{CANCELLATION}1.{}",
            encode(&serde_json::to_vec(self).expect("receipt serializes"))
        )
    }

    pub(super) fn operation(&self, epoch: u64) -> String {
        hash(
            &json!([
                "installer-cancellation",
                self.setup,
                self.invite,
                self.key,
                self.secret,
                epoch
            ])
            .to_string(),
        )
    }
}

pub(super) fn hash(token: &str) -> String {
    encode(&Sha256::digest(token))
}

pub(super) fn relay_url(value: &str, allow_local: bool) -> bool {
    value.len() <= 512
        && reqwest::Url::parse(value).is_ok_and(|url| {
            (url.scheme() == "wss"
                || (allow_local
                    && url.scheme() == "ws"
                    && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))))
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
}

#[derive(Serialize, Deserialize)]
pub(super) struct Code {
    pub expires: u64,
    pub setup: String,
    pub relay: String,
    pub address: String,
    pub primary: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    merge: Option<serde_json::Value>,
}

impl Code {
    pub(super) fn new(
        expires: u64,
        setup: String,
        relay: String,
        primary: Host,
        token: String,
    ) -> Self {
        Self {
            expires,
            setup,
            relay,
            address: encode(&primary.address),
            primary: encode(&primary.key),
            token,
            merge: None,
        }
    }

    pub(super) fn read(text: &str) -> Option<(Self, Host)> {
        let text = text.trim();
        if text.len() > 4096 {
            return None;
        }
        let code: Self = serde_json::from_slice(&decode(text.strip_prefix(PREFIX)?)?).ok()?;
        let host = Host::from_text(&code.primary, &code.address)?;
        (code.expires < (1u64 << 53)
            && code
                .setup
                .strip_prefix("set_")
                .is_some_and(|id| safe_token(id, 22, 22))
            && relay_url(&code.relay, true)
            && safe_token(&code.address, 22, 22)
            && safe_token(&code.primary, 43, 43)
            && safe_token(&code.token, 32, 32))
        .then_some((code, host))
    }

    pub(super) fn encoded(&self) -> String {
        format!(
            "{PREFIX}{}",
            encode(&serde_json::to_vec(self).expect("installer code serializes"))
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Invitation {
    pub setup: String,
    pub epoch: u64,
    pub expires: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Device>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<Claim>,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Device {
    pub key: String,
    pub pairing: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Claim {
    pub key: String,
    pub sign: String,
    pub address: String,
    pub until: u64,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Invitation {
    pub fn live(&self, now: u64) -> bool {
        self.claim
            .as_ref()
            .map_or(self.expires, |claim| claim.until)
            > now
    }
}

impl Claim {
    pub fn matches(&self, peer: &Peer) -> bool {
        self.key == peer.key
            && Some(&self.sign) == peer.sign.as_ref()
            && self.address == peer.address
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Applicant {
    pub token: String,
    pub nonce: String,
    pub address: String,
    pub sign: String,
    pub kind: ComputerKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirmed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub activation_ack: bool,
    #[serde(default)]
    pub activation_ack_capable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_commitment: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub credential_receipt: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancel: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merging: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub credential_ack: bool,
}

const RECOVERIES: &str = "installer recovery";

pub(super) fn recoveries(link: &Link, roster: &Membership) -> Map<String, Value> {
    let carried = link
        .value(SharedKind::OwnershipRecovery)
        .and_then(|fields| fields.get(RECOVERIES))
        .or_else(|| {
            link.value(SharedKind::OwnershipAssignments)
                .and_then(|fields| fields.get(RECOVERIES))
        });
    let flat = roster.extra.get("installerRecoveries");
    roster
        .boxes
        .iter()
        .filter_map(|peer| {
            let row = carried
                .and_then(|map| map.get(&peer.key))
                .or_else(|| flat.and_then(|map| map.get(&peer.key)))?;
            (row["sign"].as_str() == peer.sign.as_deref()
                && row["address"] == peer.address
                && ["invite", "commitment"].iter().all(|field| {
                    row[*field]
                        .as_str()
                        .is_some_and(|id| safe_token(id, 43, 43))
                }))
            .then(|| (peer.key.clone(), row.clone()))
        })
        .collect()
}

/// Layout 23 carries these commitments in the signed ownership recovery map.
/// It advances alongside the roster in the same link write and has no separate operation receipt.
pub(super) fn keep_recoveries(
    link: &mut Link,
    roster: &mut Membership,
    recoveries: Map<String, Value>,
) -> Result<(), Error> {
    roster.extra.shift_remove("installerRecoveries");
    let kept = link
        .shared
        .entry(SharedKind::OwnershipRecovery)
        .or_insert_with(|| Entry {
            sequence: 0,
            value: Value::Object(Map::new()),
            offline_edits: None,
        });
    kept.sequence += 1;
    if kept.sequence > kuvo_wire::documents::MAX_SEQUENCE {
        return Err(Error::Capacity);
    }
    kept.value
        .as_object_mut()
        .ok_or(Error::Invalid("ownership recovery"))?
        .insert(RECOVERIES.into(), Value::Object(recoveries));
    Ok(())
}
