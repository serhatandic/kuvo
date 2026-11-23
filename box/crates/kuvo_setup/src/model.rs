use std::collections::{BTreeMap, HashMap, HashSet};

use ed25519_dalek::SigningKey;
use kuvo_wire::{
    documents::{Envelope, Kind, MAX_SEQUENCE},
    keys::{Host, Identity as WireIdentity, address_of, decode_exact, encode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Error, object, random};

pub(crate) type Shared = BTreeMap<SharedKind, Entry>;
pub(crate) type Outcomes = BTreeMap<String, Outcome>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SharedKind {
    SetupNames,
    MembershipAndAccess,
    SetupDeparture,
    SetupMergeAuthorities,
    SetupMergeDenials,
    SetupSecurityAndReleaseChoice,
    QueueNumberAllocation,
    QueueLabels,
    OwnershipAssignments,
    OwnershipRecovery,
    MarketplaceInstallations,
    SetupPersonalities,
    SetupTimeZone,
    SetupStarredModels,
    SetupUsageDisplay,
}

impl SharedKind {
    pub fn published(self) -> bool {
        self != Self::QueueNumberAllocation
    }

    fn seeded(self) -> bool {
        matches!(
            self,
            Self::SetupNames
                | Self::QueueLabels
                | Self::SetupPersonalities
                | Self::SetupTimeZone
                | Self::SetupStarredModels
                | Self::SetupUsageDisplay
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub sequence: u64,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_edits: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Committed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Outcome {
    pub kind: SharedKind,
    pub epoch: u64,
    pub sequence: u64,
    pub base_revision: u64,
    pub proposal: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_edit: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub setup: String,
    pub epoch: u64,
    pub operation: String,
    pub proof: Envelope,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Primary,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Position {
    pub id: String,
    pub epoch: u64,
    pub primary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Setup {
    #[serde(flatten)]
    pub position: Position,
    pub sign: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "is_false")]
    pub aside: bool,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub checkpointed: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ComputerKind {
    Mac,
    Machine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Peer {
    pub key: String,
    pub address: String,
    pub kind: ComputerKind,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Membership {
    pub boxes: Vec<Peer>,
    #[serde(default)]
    pub left: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Membership {
    pub fn parse(value: &Value, position: &Position, sign: &str) -> Result<Self, Error> {
        if value.get("fields").is_some() {
            return Err(Error::Version("setup membership"));
        }
        let members: Self = serde_json::from_value(value.clone())
            .map_err(|_| Error::Invalid("setup membership"))?;
        let mut keys = HashSet::new();
        let retired = members
            .extra
            .get("computerHistory")
            .and_then(|history| history.get("retiredSigns"))
            .and_then(Value::as_array);
        for peer in &members.boxes {
            if Host::from_text(&peer.key, &peer.address).is_none()
                || !keys.insert(&peer.key)
                || members.left.contains(&peer.key)
                || peer.sign.as_ref().is_some_and(|sign| {
                    decode_exact::<32>(sign).is_none()
                        || retired
                            .is_some_and(|rows| rows.iter().any(|row| row.as_str() == Some(sign)))
                })
            {
                return Err(Error::Invalid("setup roster"));
            }
        }
        if !members
            .boxes
            .iter()
            .any(|peer| peer.key == position.primary && peer.sign.as_deref() == Some(sign))
        {
            return Err(Error::Invalid("setup primary"));
        }
        Ok(members)
    }

    pub fn hosts<'a>(&'a self, own: &'a str) -> impl Iterator<Item = Host> + 'a {
        self.boxes
            .iter()
            .filter(move |peer| peer.key != own)
            .filter_map(|peer| Host::from_text(&peer.key, &peer.address))
    }

    fn removed_grant(&self, key: &str, pairing: &str) -> bool {
        let devices = self.extra.get("devices");
        let removed = devices
            .and_then(|devices| devices.get("removed"))
            .and_then(Value::as_array);
        removed.is_some_and(|rows| rows.iter().any(|row| matches_grant(row, key, pairing)))
    }

    pub fn devices(&self) -> impl Iterator<Item = &Value> {
        self.extra
            .get("devices")
            .and_then(|device| device.get("phones"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
    }
}

pub(crate) fn matches_grant(device: &Value, key: &str, pairing: &str) -> bool {
    device.get("key").and_then(Value::as_str) == Some(key)
        && device
            .get("pairing")
            .and_then(Value::as_str)
            .unwrap_or_default()
            == pairing
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Link {
    #[serde(skip)]
    pub devices_disabled: bool,
    #[serde(default)]
    pub format_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<Setup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_setup: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shared: Shared,
    #[serde(default)]
    pub shared_outcomes: Outcomes,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub leadership: BTreeMap<u64, Envelope>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub installer_invites: BTreeMap<String, crate::installer::Invitation>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub(crate) struct Identity {
    pub key: [u8; 32],
    pub seed: [u8; 32],
    pub secret: [u8; 32],
    pub public: String,
    pub sign: String,
}

impl Identity {
    pub fn read(link: &Link) -> Result<Self, Error> {
        let key = decode_exact(link.key.as_deref().unwrap_or_default())
            .ok_or(Error::Invalid("link static key"))?;
        let seed = decode_exact(link.sign.as_deref().unwrap_or_default())
            .ok_or(Error::Invalid("link signing seed"))?;
        let secret = decode_exact(link.secret.as_deref().unwrap_or_default())
            .ok_or(Error::Invalid("link relay secret"))?;
        Ok(Self {
            public: encode(&WireIdentity::from_bytes(key, secret).public()),
            sign: encode(SigningKey::from_bytes(&seed).verifying_key().as_bytes()),
            key,
            seed,
            secret,
        })
    }

    pub fn self_peer(&self, kind: ComputerKind, name: &str) -> Peer {
        Peer {
            key: self.public.clone(),
            sign: Some(self.sign.clone()),
            address: encode(&address_of(&self.secret)),
            name: name.into(),
            kind,
            extra: Map::new(),
        }
    }
}

pub(crate) fn safe_token(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(crate) fn validate_position(position: &Position) -> Result<(), Error> {
    if !(1..=MAX_SEQUENCE).contains(&position.epoch)
        || !position
            .id
            .strip_prefix("set_")
            .is_some_and(|id| safe_token(id, 22, 22))
        || decode_exact::<32>(&position.primary).is_none()
    {
        return Err(Error::Invalid("setup position"));
    }
    Ok(())
}

pub(crate) fn validate_shared(values: &Shared) -> Result<(), Error> {
    if values
        .values()
        .any(|entry| !(1..=MAX_SEQUENCE).contains(&entry.sequence))
    {
        return Err(Error::Invalid("shared revision"));
    }
    Ok(())
}

pub(crate) fn membership(
    values: &Shared,
    position: &Position,
    sign: &str,
) -> Result<Membership, Error> {
    let entry = values
        .get(&SharedKind::MembershipAndAccess)
        .ok_or(Error::Invalid("missing membership"))?;
    Membership::parse(&entry.value, position, sign)
}

pub(crate) fn fingerprint(value: &Value) -> String {
    encode(&kuvo_wire::json::digest(value))
}

impl Link {
    pub fn setup(&self) -> Result<&Setup, Error> {
        self.setup
            .as_ref()
            .filter(|setup| !setup.aside)
            .ok_or(Error::Conflict)
    }

    pub fn member_list(&self) -> Result<Membership, Error> {
        let setup = self.setup()?;
        if self.shared_setup.as_ref() != Some(&setup.position.id) {
            return Err(Error::Invalid("shared setup scope"));
        }
        membership(&self.shared, &setup.position, &setup.sign)
    }

    pub fn disabled(&self, roster: &Membership, own: &str) -> bool {
        ["localWipe", "removalSource", "localUnlink", "macRotation"]
            .iter()
            .any(|field| {
                self.extra
                    .get(*field)
                    .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
            })
            || !roster.boxes.iter().any(|peer| peer.key == own)
    }

    pub fn grants(
        &self,
        roster: &Membership,
        own: &str,
    ) -> HashMap<[u8; 32], kuvo_server::ssh::Grant> {
        let Ok(setup) = self.setup() else {
            return HashMap::new();
        };
        if self.disabled(roster, own) {
            return HashMap::new();
        }
        let history = self.extra.get("formerDeviceRevocations");
        roster
            .devices()
            .filter_map(|device| {
                let key = device.get("key")?.as_str()?;
                let pairing = device
                    .get("pairing")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if (!pairing.is_empty() && !safe_token(pairing, 1, 32))
                    || roster.boxes.iter().any(|peer| peer.key == key)
                    || roster.removed_grant(key, pairing)
                    || [
                        Some(setup.position.id.as_str()),
                        (setup.role == Role::Member).then_some(setup.sign.as_str()),
                        Some("unattributed"),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|scope| {
                        history
                            .and_then(|history| history.get(scope))
                            .and_then(Value::as_array)
                            .is_some_and(|rows| {
                                rows.iter().any(|row| matches_grant(row, key, pairing))
                            })
                    })
                {
                    return None;
                }
                let name = device
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                Some((
                    decode_exact(key)?,
                    kuvo_server::ssh::Grant {
                        id: pairing.into(),
                        name,
                    },
                ))
            })
            .collect()
    }

    pub fn value(&self, kind: SharedKind) -> Option<&Value> {
        let setup = self.setup().ok()?;
        if self.shared_setup.as_ref() != Some(&setup.position.id) {
            return None;
        }
        self.shared.get(&kind).map(|entry| &entry.value)
    }

    pub fn retry(
        &self,
        identity: &Identity,
        kind: SharedKind,
        operation: &str,
        base_revision: Option<u64>,
        value: &Value,
    ) -> Result<Option<Receipt>, Error> {
        let Some(previous) = self.shared_outcomes.get(operation) else {
            return Ok(None);
        };
        let setup = self.setup()?;
        if setup.role == Role::Primary
            && setup.sign == identity.sign
            && setup.position.primary == identity.public
            && self.shared_setup.as_ref() == Some(&setup.position.id)
            && previous.status == Status::Committed
            && previous.kind == kind
            && previous.epoch == setup.position.epoch
            && base_revision == Some(previous.base_revision)
            && kind == SharedKind::SetupUsageDisplay
            && previous.mutation_digest.as_deref()
                == Some(fingerprint(&json!({"kind": kind, "value": value})).as_str())
        {
            return receipt(identity, &setup.position, operation.into(), previous).map(Some);
        }
        Err(Error::Conflict)
    }

    pub fn put(
        &mut self,
        identity: &Identity,
        kind: SharedKind,
        value: Value,
        expected: Option<u64>,
        operation: Option<String>,
    ) -> Result<Receipt, Error> {
        let setup = self.setup()?.clone();
        if setup.role != Role::Primary || setup.sign != identity.sign {
            return Err(Error::Conflict);
        }
        let operation = match operation {
            Some(operation) if safe_token(&operation, 22, 43) => operation,
            Some(_) => return Err(Error::Invalid("shared operation identifier")),
            None => encode(&random::<16>()?),
        };
        let digest = (kind == SharedKind::SetupUsageDisplay)
            .then(|| fingerprint(&json!({ "kind": kind, "value": value })));
        if let Some(previous) = self.retry(identity, kind, &operation, expected, &value)? {
            return Ok(previous);
        }
        let previous = self.shared.get(&kind);
        let base_revision = previous.map_or(0, |entry| entry.sequence);
        if expected.is_some_and(|expected| expected != base_revision)
            || base_revision >= MAX_SEQUENCE
        {
            return Err(Error::Conflict);
        }
        if kind == SharedKind::SetupSecurityAndReleaseChoice {
            return Err(Error::Invalid("release save through setup"));
        }
        if kind == SharedKind::MembershipAndAccess {
            Membership::parse(&value, &setup.position, &setup.sign)?;
        }
        let mut edits = previous.and_then(|entry| entry.offline_edits.clone());
        if previous.is_none() && kind.seeded() {
            edits = Some(
                json!({"versions": [], "choices": [], "receipts": [], "additions": {"setup-seed:": "1"}}),
            );
        }
        let sequence = base_revision + 1;
        let outcome = Outcome {
            kind,
            epoch: setup.position.epoch,
            sequence,
            base_revision,
            proposal: fingerprint(&json!({ "kind": kind, "value": value, "operation": operation })),
            status: Status::Committed,
            offline_edit: None,
            mutation_digest: digest,
        };
        self.shared.insert(
            kind,
            Entry {
                sequence,
                value,
                offline_edits: edits,
            },
        );
        self.check_capacity()?;
        let proof = receipt(identity, &setup.position, operation.clone(), &outcome)?;
        self.shared_outcomes.insert(operation, outcome);
        Ok(proof)
    }

    pub fn check_capacity(&self) -> Result<(), Error> {
        let mut ordinary = self.shared.clone();
        let denial = ordinary.remove(&SharedKind::SetupMergeDenials);
        if serde_json::to_vec(&ordinary)
            .expect("shared values serialize")
            .len()
            > kuvo_wire::documents::MAX_BYTES / 3
            || denial.is_some_and(|value| {
                serde_json::to_vec(&value).expect("denial serializes").len() > 16 * 1024
            })
        {
            return Err(Error::Capacity);
        }
        Ok(())
    }
}

pub(crate) fn receipt(
    identity: &Identity,
    position: &Position,
    operation: String,
    outcome: &Outcome,
) -> Result<Receipt, Error> {
    let proof = kuvo_wire::documents::sign(
        &identity.seed,
        Kind::SetupShared,
        outcome.sequence,
        object(json!({
            "id": position.id, "epoch": position.epoch, "operation": operation, "kind": outcome.kind,
            "baseRevision": outcome.base_revision, "proposal": outcome.proposal,
        })),
    )?;
    Ok(Receipt {
        setup: position.id.clone(),
        epoch: position.epoch,
        operation,
        proof,
        status: "committed".into(),
    })
}
