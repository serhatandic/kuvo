//! Ownership facts and authenticated transport supplied by the peers runtime.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use http::StatusCode;
use kuvo_server::{Caller, Pairing, Refusal, Transport};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type ComputerKey = [u8; 32];
pub type PeerReply<'a> = Pin<Box<dyn Future<Output = Result<OwnedReply, Refusal>> + Send + 'a>>;

#[derive(Clone)]
pub struct OwnerScope {
    pub setup: Option<String>,
    pub here: Option<ComputerKey>,
    pub owner: Option<SessionOwner>,
    pub receipt_issuer: Option<ComputerKey>,
    pub held: bool,
}

impl OwnerScope {
    pub(crate) fn owner_key(&self) -> Option<ComputerKey> {
        self.owner
            .as_ref()
            .and_then(|owner| owner.key)
            .or(self.here)
    }
}

#[derive(Clone)]
pub struct SessionOwner {
    pub key: Option<ComputerKey>,
    pub name: String,
    pub local: bool,
    pub reachable: bool,
    pub features: Vec<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DeviceAccess {
    Allowed,
    Revoked,
    Checking,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Submitter {
    Computer,
    Paired {
        #[serde(with = "text_key")]
        key: ComputerKey,
        pairing: String,
    },
    Ssh {
        #[serde(with = "text_key")]
        key: ComputerKey,
    },
}

impl Submitter {
    pub(crate) fn from_request(caller: &Caller) -> Result<Self, Refusal> {
        crate::typed::author(caller)?;
        crate::consent::current(caller, &tokio_util::sync::CancellationToken::new())?;
        Ok(match caller.pairing() {
            Some(pair) => Self::Paired {
                key: pair.key,
                pairing: pair.id.clone(),
            },
            None => Self::Computer,
        })
    }

    pub(crate) fn author_matches(&self, reader: &Self) -> bool {
        match (self, reader) {
            (Self::Paired { key, .. }, Self::Paired { key: other, .. }) => key == other,
            (left, right) => left == right,
        }
    }

    pub(crate) fn authorize(&self, authority: &dyn SessionOwnership) -> Result<(), Refusal> {
        if let Self::Paired { key, pairing } = self {
            return match authority.current_pairing(key, pairing) {
                DeviceAccess::Allowed => Ok(()),
                DeviceAccess::Revoked => Err(Refusal::new(
                    StatusCode::FORBIDDEN,
                    "The submitting device no longer has access. Check the record before sending again.",
                )),
                DeviceAccess::Checking => Err(Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Checking the submitting device's access.",
                )),
            };
        }
        Ok(())
    }

    pub(crate) fn caller(&self, authority: Arc<dyn SessionOwnership>, desktop: bool) -> Caller {
        if let Self::Paired { key, pairing } = self {
            let key = *key;
            let id = pairing.clone();
            return Caller::Paired {
                transport: Transport::Relay,
                pairing: Pairing::verified(
                    key,
                    pairing.clone(),
                    String::new(),
                    desktop,
                    move || authority.current_pairing(&key, &id) == DeviceAccess::Allowed,
                ),
            };
        }
        Caller::Local(Transport::Ssh { internal: true })
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct OwnedRequest {
    pub setup: String,
    pub session: String,
    pub operation: String,
    pub submitter: Submitter,
    pub action: OwnerAction,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum OwnerAction {
    Submit {
        draft: Value,
    },
    Outcome {
        #[serde(with = "text_key")]
        issuer: ComputerKey,
        query: Value,
    },
    Deliver {
        message: Value,
    },
}

pub struct OwnedReply {
    pub status: u16,
    pub body: Value,
}

pub trait SessionOwnership: Send + Sync {
    fn scope(&self, session: &str, issuer: Option<ComputerKey>) -> Result<OwnerScope, Refusal>;
    fn members(&self) -> Vec<SessionOwner> {
        Vec::new()
    }
    fn current_pairing(&self, key: &ComputerKey, pairing: &str) -> DeviceAccess;
    fn request(&self, target: ComputerKey, request: OwnedRequest, wait: Duration) -> PeerReply<'_>;
}

impl crate::Engine {
    pub fn with_session_ownership(mut self, authority: Arc<dyn SessionOwnership>) -> Self {
        self.ownership = Some(authority);
        self
    }
}

pub(crate) mod text_key {
    use super::ComputerKey;
    use kuvo_wire::keys::{decode_exact, encode};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key: &ComputerKey, output: S) -> Result<S::Ok, S::Error> {
        output.serialize_str(&encode(key))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<ComputerKey, D::Error> {
        let text = String::deserialize(input)?;
        decode_exact(&text)
            .filter(|key| encode(key) == text)
            .ok_or_else(|| serde::de::Error::custom("Invalid computer key"))
    }
}
