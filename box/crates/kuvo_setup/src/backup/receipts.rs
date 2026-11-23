//! A journal slot can contain opaque history, an origin shadow, or evidence
//! whose validated address must still belong to the restoring computer.
use std::{fs, path::Path};

use kuvo_store::format::{ObjectFormat, supports_legacy_object_version};
use serde::Deserialize;
use serde_json::Value;

use super::{Error, Result, archive, recovery::record_key, url_token};

const OWNED: ObjectFormat = ObjectFormat::additive("Owned data");

pub enum ReceiptOwner {
    Primary,
    Session(Option<String>),
    Group(Option<String>),
    Queue {
        queue: Option<String>,
        item: Option<String>,
    },
    Client,
    ReplayFence(String),
    Issuer(String),
}

pub(super) enum Journal {
    Opaque,
    Origin,
    Addressed {
        setup: String,
        owner: Option<ReceiptOwner>,
    },
}

impl Journal {
    pub(super) fn capture(name: &str, file: &Path) -> Result<Self> {
        Self::read(name, file, true)
    }

    pub(super) fn restore(name: &str, file: &Path) -> Result<Self> {
        Self::read(name, file, false)
    }

    fn read(name: &str, file: &Path, capturing: bool) -> Result<Self> {
        let address = |prefix| {
            name.strip_prefix(prefix)?
                .strip_suffix(".json")
                .filter(|key| url_token(key, 43))
        };
        if let Some(key) = address("home/run/owned-data/submissions/") {
            let value: Value = match serde_json::from_reader(archive::regular(file)?) {
                Ok(value) => value,
                Err(_) => return Ok(Self::Opaque),
            };
            if value
                .as_object()
                .is_some_and(supports_legacy_object_version)
                && let Some(receipt) = submission(&value["submission"], key)
            {
                return Ok(receipt);
            }
        }
        let Some(key) = address("home/run/owned-data/mutations/") else {
            return Ok(Self::Opaque);
        };
        let value: Value = serde_json::from_reader(archive::regular(file)?)?;
        if capturing
            && !value
                .as_object()
                .is_some_and(supports_legacy_object_version)
        {
            return Ok(Self::Opaque);
        }
        let record = OWNED
            .decode(value)
            .map_err(|error| Error::Detail(error.to_string()))?
            .fields;
        let replay = record
            .get("draftReplayFence")
            .map(|value| fence(value, key))
            .transpose()?;
        let scoped = record
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|digest| digest.strip_prefix("scoped:"))
            .is_some_and(|key| url_token(key, 43));
        // Capture classifies draft fences before forward records. Recovery
        // verifies both address stamps before it can place any evidence.
        if capturing && let Some(replay) = replay {
            return Ok(if scoped && !record.contains_key("answer") {
                replay
            } else {
                Self::Opaque
            });
        }
        let forward = record
            .get("forward")
            .map(|value| forwarded(value, key))
            .transpose()?;
        if let Some(forward) = forward {
            return Ok(forward);
        }
        if let Some(replay) = replay {
            return Ok(if scoped && !record.contains_key("answer") {
                replay
            } else {
                Self::Opaque
            });
        }
        let has_answer = record.contains_key("answer");
        if capturing && !has_answer {
            let twin = file
                .parent()
                .and_then(Path::parent)
                .expect("mutation folder")
                .join("forward-origins")
                .join(format!("{key}.json"));
            match fs::symlink_metadata(&twin) {
                Ok(_) => {
                    let value: Value = serde_json::from_reader(archive::regular(&twin)?)?;
                    let origin = OWNED
                        .decode(value)
                        .map_err(|error| Error::Detail(error.to_string()))?;
                    if origin.fields.get("digest") == record.get("digest") {
                        return Ok(Self::Origin);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self::Opaque)
    }
}

fn setup_id(value: &str) -> bool {
    value
        .strip_prefix("set_")
        .is_some_and(|token| url_token(token, 22))
}

fn token(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && url_token(value, value.len())
}

fn session_id(value: &str) -> bool {
    value.strip_prefix("ses_").is_some_and(|id| token(id, 95))
}

fn submission(value: &Value, key: &str) -> Option<Journal> {
    let session = value["session"]
        .as_str()
        .filter(|session| session_id(session))?;
    let handle = value["id"].as_str()?;
    let mut parts = handle.split('.');
    let setup = parts.next().filter(|setup| setup_id(setup))?;
    let issuer = parts.next().filter(|issuer| url_token(issuer, 43))?;
    parts.next().filter(|id| token(id, 80))?;
    if parts.next().is_some()
        || value["setup"] != setup
        || value["origin"] != issuer
        || key != record_key(&["draft-submission", setup, session, handle])
    {
        return None;
    }
    Some(Journal::Addressed {
        setup: setup.into(),
        owner: Some(ReceiptOwner::Issuer(issuer.into())),
    })
}

fn fence(value: &Value, key: &str) -> Result<Journal> {
    let invalid = || Error::Message("Invalid draft replay fence.");
    let setup = value["setup"]
        .as_str()
        .filter(|setup| setup_id(setup))
        .ok_or_else(invalid)?;
    let session = value["session"]
        .as_str()
        .filter(|session| session_id(session))
        .ok_or_else(invalid)?;
    let id = value["id"]
        .as_str()
        .filter(|id| token(id, 80))
        .ok_or_else(invalid)?;
    if key != record_key(&["typed-draft", setup, session, id]) {
        return Err(invalid());
    }
    Ok(Journal::Addressed {
        setup: setup.into(),
        owner: Some(ReceiptOwner::ReplayFence(session.into())),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Forwarded {
    kind: String,
    setup: String,
    operation: String,
    owner: String,
    listed_by: Option<String>,
    submitter: Value,
    #[serde(rename = "area")]
    _area: String,
    record: String,
    #[serde(rename = "ref")]
    reference: Option<Reference>,
}

#[derive(Deserialize)]
struct Reference {
    kind: String,
    id: Option<String>,
    queue: Option<String>,
}

impl Reference {
    fn check(self) -> Result<(ReceiptOwner, &'static str)> {
        if self
            .id
            .iter()
            .chain(self.queue.iter())
            .any(|id| !token(id, 100))
        {
            return Err(Error::Message(
                "Invalid forwarded operation record address.",
            ));
        }
        let (owner, decision) = match self.kind.as_str() {
            "sessionQueue" | "sessionSchedules" => {
                (ReceiptOwner::Session(self.id), "forwardedSessionDecisions")
            }
            "groupSettings" => (ReceiptOwner::Group(self.id), "forwardedGroupDecisions"),
            "queueItems" => (
                ReceiptOwner::Queue {
                    item: self.id,
                    queue: self.queue,
                },
                "forwardedQueueDecisions",
            ),
            "queueLabels"
            | "setupNames"
            | "setupTimeZone"
            | "setupStarredModels"
            | "setupPersonalities"
            | "membershipAndAccess"
            | "setupLeadership" => (ReceiptOwner::Primary, "forwardedPrimaryDecisions"),
            "clientIdentity" => (ReceiptOwner::Client, "forwardedComputerDecisions"),
            _ => {
                return Err(Error::Message(
                    "Invalid forwarded operation record address.",
                ));
            }
        };
        Ok((owner, decision))
    }
}

fn forwarded(value: &Value, key: &str) -> Result<Journal> {
    let invalid = || Error::Message("Invalid forwarded operation evidence.");
    let evidence: Forwarded = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    if value.get("listedBy").is_some_and(Value::is_null) {
        return Err(invalid());
    }
    let submitter = &evidence.submitter;
    let known_key = submitter["key"]
        .as_str()
        .is_some_and(|key| url_token(key, 43));
    let submitter_valid = match submitter["kind"].as_str() {
        Some("computer") => true,
        Some("ssh") => known_key,
        Some("paired") => {
            known_key
                && submitter["pairing"]
                    .as_str()
                    .is_some_and(|pairing| pairing.len() <= 32 && url_token(pairing, pairing.len()))
        }
        _ => false,
    };
    let origin = evidence.kind == "forwardedOperationOrigins";
    if !setup_id(&evidence.setup)
        || !token(&evidence.operation, 80)
        || !url_token(&evidence.owner, 43)
        || evidence
            .listed_by
            .as_ref()
            .is_some_and(|key| !url_token(key, 43))
        || evidence.record.is_empty()
        || !submitter_valid
        || !matches!(
            evidence.kind.as_str(),
            "forwardedOperationOrigins"
                | "forwardedComputerDecisions"
                | "forwardedPrimaryDecisions"
                | "forwardedSessionDecisions"
                | "forwardedGroupDecisions"
                | "forwardedQueueDecisions"
        )
    {
        return Err(invalid());
    }
    let prefix = if origin {
        "forward-origin"
    } else {
        "scoped-forward"
    };
    if key != record_key(&[prefix, &evidence.setup, &evidence.operation]) {
        return Err(Error::Message(
            "Forwarded operation evidence has the wrong address.",
        ));
    }
    if value.get("ref").is_some_and(|reference| {
        reference.is_null()
            || ["id", "queue"]
                .iter()
                .any(|field| reference.get(field).is_some_and(Value::is_null))
    }) {
        return Err(Error::Message(
            "Invalid forwarded operation record address.",
        ));
    }
    let checked = evidence.reference.map(Reference::check).transpose()?;
    if origin {
        return Ok(Journal::Origin);
    }
    let decision = checked
        .as_ref()
        .map_or("forwardedComputerDecisions", |(_, kind)| *kind);
    if evidence.kind != decision {
        return Err(Error::Message(
            "Forwarded operation evidence has the wrong owner policy.",
        ));
    }
    Ok(Journal::Addressed {
        setup: evidence.setup,
        owner: checked.map(|(owner, _)| owner),
    })
}
