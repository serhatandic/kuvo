//! A paired device's change to an item another computer owns (TypeScript `forward-grant.ts` and
//! `takeScopedAct`). The member it reached sends the change on with the device's grant: the
//! setup, the device and the operation. The owner checks that the device still has access here and
//! decides each operation once, keeping its decision where moves carry it, so a retry, even after
//! the device lost access, is answered as it was.

use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::records::{Act, Method, Reply};

/// What a computer says once it takes scoped acts in every area: queue items so far, so it isn't
/// said yet.
pub(in crate::queue) const FEATURE: &str = "data.scoped-forward";
pub(in crate::queue) const REVOKED: &str =
    "The submitting device no longer has access. Check the record before sending again.";
pub(in crate::queue) const NO_GRANT: &str = "Use a current device grant and a valid operation id.";
const REUSED: &str = "That operation id is already in use.";
const CLAIMED: &str = "The owner has claimed this operation. Check the record before trying again.";
const EXPIRED: &str = "This forwarded operation expired.";
/// How old and how far ahead a forwarded operation may be when it's decided.
const ACT_MEMORY: i64 = 600_000;
const AHEAD: i64 = 60_000;

/// The device a change came from.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(in crate::queue) enum Submitter {
    Paired { key: String, pairing: String },
}

/// A paired device's grant for one operation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(in crate::queue) struct Grant {
    pub setup: String,
    pub submitter: Submitter,
    pub operation: String,
}

impl Grant {
    pub fn device(&self) -> Option<([u8; 32], &str)> {
        let Submitter::Paired { key, pairing } = &self.submitter;
        Some((kuvo_wire::keys::decode_exact(key)?, pairing))
    }
}

/// A paired device asking this computer for a change another computer keeps: its setup, key and
/// pairing, and the operation it named (none: the forwarded act's own id names it).
pub(in crate::queue) struct Device {
    pub setup: String,
    pub key: [u8; 32],
    pub pairing: String,
    pub operation: Option<String>,
}

impl Device {
    pub fn grant(&self, act: &str) -> Grant {
        Grant {
            setup: self.setup.clone(),
            submitter: Submitter::Paired {
                key: kuvo_wire::keys::encode(&self.key),
                pairing: self.pairing.clone(),
            },
            operation: self.operation.clone().unwrap_or_else(|| act.to_owned()),
        }
    }
}

/// The operation a client named for its change: the `x-kuvo-operation` header or the body's
/// `operation` (both, only if they agree), else its `idempotency-key`. `Err` for one that can't be
/// an operation id.
pub(in crate::queue) fn operation(
    headers: &http::HeaderMap,
    body: Option<&Value>,
) -> Result<Option<String>, ()> {
    let header = headers
        .get("x-kuvo-operation")
        .map(|value| value.to_str().map(str::to_owned).map_err(|_| ()))
        .transpose()?;
    let field = body.and_then(|body| body.get("operation"));
    let field = match field {
        None => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => return Err(()),
    };
    if header.is_some() && field.is_some() && header != field {
        return Err(());
    }
    let named = match header.or(field) {
        Some(named) => Some(named),
        None => headers
            .get("idempotency-key")
            .map(|value| value.to_str().map(str::to_owned).map_err(|_| ()))
            .transpose()?,
    };
    match named {
        Some(named) if !operation_id(&named) => Err(()),
        named => Ok(named),
    }
}

fn operation_id(text: &str) -> bool {
    (1..=80).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// A scoped act as it came: the act, the device's grant, the computer it's for and whether it may
/// only be answered from an earlier decision (the device lost access since).
pub(in crate::queue) struct Given {
    pub act: Act,
    pub grant: Grant,
    pub owner: String,
    pub replay_only: bool,
}

impl Given {
    pub fn read(value: &Value) -> Option<Self> {
        let readable = |value: &Value| {
            value
                .get("formatVersion")
                .is_none_or(|version| matches!(version.as_u64(), Some(0 | 1)))
        };
        if !readable(value) || !readable(&value["grant"]) {
            return None;
        }
        let act = Act::parse(&value["act"])?;
        let grant = serde_json::from_value::<Grant>(value["grant"].clone()).ok()?;
        let owner = value["owner"].as_str()?.to_owned();
        if !operation_id(&grant.operation) || grant.device().is_none() || matches!(act.method, Method::Get) {
            return None;
        }
        Some(Self {
            act,
            grant,
            owner,
            replay_only: value["replayOnly"] == true,
        })
    }

    /// What's decided: the change itself, by whom and for whom, apart from when it was sent.
    pub fn digest(&self) -> String {
        let mut intent = serde_json::to_value(&self.act).unwrap_or_default();
        if let Some(fields) = intent.as_object_mut() {
            fields.remove("id");
            fields.remove("at");
        }
        fingerprint(&json!({"intent":intent,"submitter":self.grant.submitter,"owner":self.owner}))
    }

    pub fn expired(&self, now: i64) -> bool {
        now - self.act.at > ACT_MEMORY || self.act.at - now > AHEAD
    }
}

/// What the owner decided for each operation, in `run/owned-data/mutations`.
pub(in crate::queue) struct Ledger {
    path: PathBuf,
    digest: String,
    forward: Value,
}

pub(in crate::queue) enum Earlier {
    None,
    Answer(Reply),
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Claim {
    format_version: u8,
    digest: String,
    forward: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer: Option<Reply>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<i64>,
}

impl Ledger {
    /// The decision for `given` on the item `item` of the queue `queue`, kept under `run`.
    pub fn new(run: &Path, given: &Given, item: &str, queue: &str) -> Self {
        let key = fingerprint(&json!(["scoped-forward", given.grant.setup, given.grant.operation]));
        Self {
            path: run.join("owned-data").join("mutations").join(format!("{key}.json")),
            digest: given.digest(),
            forward: json!({
                "kind":"forwardedQueueDecisions","setup":given.grant.setup,
                "operation":given.grant.operation,"owner":given.owner,
                "submitter":given.grant.submitter,"area":"queue","record":item,
                "ref":{"kind":"queueItems","id":item,"queue":queue},
            }),
        }
    }

    /// What was decided for this operation before: its answer; a refusal when the operation was
    /// used for another change, or when its decision was begun and never finished.
    pub fn earlier(&self) -> io::Result<Earlier> {
        let Some(bytes) = kuvo_store::files::read(&self.path)? else {
            return Ok(Earlier::None);
        };
        let claim = serde_json::from_slice::<Claim>(&bytes).map_err(io::Error::other)?;
        Ok(Earlier::Answer(if claim.digest != self.digest {
            refused(REUSED)
        } else {
            claim.answer.unwrap_or_else(|| refused(CLAIMED))
        }))
    }

    /// Notes that this operation is being decided, before anything is done.
    pub fn claim(&self) -> io::Result<()> {
        self.write(None, None)
    }

    /// Keeps `answer` as this operation's decision.
    pub fn decide(&self, answer: &Reply, now: i64) -> io::Result<()> {
        self.write(Some(answer.clone()), Some(now))
    }

    fn write(&self, answer: Option<Reply>, at: Option<i64>) -> io::Result<()> {
        let claim = Claim {
            format_version: 1,
            digest: self.digest.clone(),
            forward: self.forward.clone(),
            answer,
            at,
        };
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let bytes = serde_json::to_vec(&claim).map_err(io::Error::other)?;
        kuvo_store::files::write_atomic(&self.path, &bytes)
    }
}

pub(in crate::queue) const BOUND: &str =
    "That operation id is already bound to another change or owner.";

/// Binds the operation a client named to this change, for this owner, here before it's sent
/// (TypeScript `pinForward`): false where the operation was bound to another change or owner.
pub(in crate::queue) fn pin(
    run: &Path,
    grant: &Grant,
    owner: &str,
    listed_by: &str,
    act: Value,
) -> io::Result<bool> {
    let record = act["item"].clone();
    let mut intent = act;
    if let Some(fields) = intent.as_object_mut() {
        fields.remove("id");
        fields.remove("at");
    }
    let digest =
        fingerprint(&json!({"owner":owner,"intent":intent,"submitter":grant.submitter}));
    let key = fingerprint(&json!(["forward-origin", grant.setup, grant.operation]));
    let folders = [run.join("owned-data").join("mutations"), run.join("owned-data").join("forward-origins")];
    let mut missing = Vec::new();
    for folder in &folders {
        let path = folder.join(format!("{key}.json"));
        match kuvo_store::files::read(&path)? {
            Some(bytes) => {
                let kept = serde_json::from_slice::<Claim>(&bytes).map_err(io::Error::other)?;
                if kept.digest != digest {
                    return Ok(false);
                }
            }
            None => missing.push((folder, path)),
        }
    }
    let claim = Claim {
        format_version: 1,
        digest,
        forward: json!({
            "kind":"forwardedOperationOrigins","setup":grant.setup,"operation":grant.operation,
            "owner":owner,"listedBy":listed_by,"submitter":grant.submitter,"area":"queue",
            "record":record,
        }),
        answer: None,
        at: None,
    };
    let bytes = serde_json::to_vec(&claim).map_err(io::Error::other)?;
    for (folder, path) in missing {
        std::fs::create_dir_all(folder)?;
        kuvo_store::files::write_atomic(&path, &bytes)?;
    }
    Ok(true)
}

pub(in crate::queue) fn refused(message: &str) -> Reply {
    Reply {
        status: 409,
        value: json!({"message":message}),
    }
}

pub(in crate::queue) fn expired() -> Reply {
    refused(EXPIRED)
}

/// A value's SHA-256 over its JSON with every object's keys in order, as TypeScript's
/// `fingerprint` makes it.
fn fingerprint(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(fields) => {
                let mut keys: Vec<_> = fields.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|key| (key.clone(), sorted(&fields[key]))).collect::<Map<_, _>>())
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    let bytes = serde_json::to_vec(&sorted(value)).expect("JSON values serialize");
    kuvo_wire::keys::encode(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operation_is_named_once_and_must_read_as_an_id() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(operation(&headers, None), Ok(None));
        headers.insert("x-kuvo-operation", "op-1".parse().unwrap());
        assert_eq!(operation(&headers, Some(&json!({"operation":"op-1"}))), Ok(Some("op-1".into())));
        assert_eq!(operation(&headers, Some(&json!({"operation":"op-2"}))), Err(()));
        let mut keyed = http::HeaderMap::new();
        keyed.insert("idempotency-key", "not an id".parse().unwrap());
        assert_eq!(operation(&keyed, None), Err(()));
    }
}
