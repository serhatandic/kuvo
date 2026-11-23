//! The installer code and the link records shared with the TypeScript box.

mod join;
pub(crate) mod released;

use kuvo_wire::keys::{Host, decode, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{
    Error,
    model::{ComputerKind, Entry, Link, Membership, Peer, SharedKind, safe_token},
};

pub use join::{Joined, join};

pub(super) const INVITE_LIFETIME: u64 = 15 * 60_000;
pub(super) const CLAIM_LIFETIME: u64 = 24 * 60 * 60_000;
const PREFIX: &str = "kuvo-add-1.";

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

    fn read(text: &str) -> Option<(Self, Host)> {
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
