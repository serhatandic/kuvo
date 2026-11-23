//! Signed roster and access documents for readers that still use the separate exports.
//! Accepted membership and the setup replica remain the authorities for local access.

use kuvo_wire::{
    documents::{self, Document, Envelope, Kind},
    keys::encode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Error,
    model::{Identity, Link, Membership, Peer, Position, Role, SharedKind, safe_token},
    object,
};

type Export = (Kind, Envelope);

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct DigestEntry(pub String, pub Kind, pub u64, pub String);

#[derive(Clone, Deserialize, Serialize)]
struct Counter {
    n: u64,
    h: String,
    b: String,
}

fn bucket(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Peers => Some("peers"),
        Kind::Phones => Some("phones"),
        Kind::Personalities => Some("personalities"),
        _ => None,
    }
}

fn previous(link: &Link, kind: Kind) -> Option<Counter> {
    let fields = link.extra.get("own")?.get(bucket(kind)?)?;
    serde_json::from_value(fields.clone()).ok()
}

fn body_hash(value: &Value) -> String {
    encode(&Sha256::digest(serde_json::to_vec(value).expect("signed body is JSON"))[..8])
}

fn named(link: &Link, mut peer: Peer) -> Peer {
    if let Some(name) = link
        .value(SharedKind::SetupNames)
        .and_then(|value| value.get(&peer.key))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
    {
        peer.name = name.into();
    }
    peer.extra.shift_remove("id");
    peer
}

pub(crate) fn published(
    link: &mut Link,
    identity: &Identity,
) -> Result<(Vec<Export>, bool), Error> {
    let setup = link.setup()?.clone();
    if setup.role != Role::Primary {
        return Ok((vec![], false));
    }
    let roster = link.member_list()?;
    let tip = link
        .leadership
        .get(&setup.position.epoch)
        .cloned()
        .map(documents::verify)
        .transpose()?
        .map(|doc| doc.digest());
    let boxes: Vec<_> = roster
        .boxes
        .iter()
        .filter(|peer| peer.sign.is_some())
        .cloned()
        .map(|peer| named(link, peer))
        .collect();
    let mut peers = json!({
        "boxes": boxes,
        "moved": roster.extra.get("moved").cloned().unwrap_or_else(|| json!([])),
        "left": roster.left.iter().rev().take(64).rev().collect::<Vec<_>>(),
        "setup": setup.position,
        "managed": true, "checkpointed": true,
    });
    if let Some(tip) = tip {
        peers["leadershipHash"] = json!(tip);
    }
    if let Some(removals) = roster.extra.get("removals").and_then(Value::as_object) {
        let removals: Vec<_> = removals
            .iter()
            .filter(|(_, removal)| removal["local"] != true)
            .map(|(key, _)| key)
            .collect();
        peers["removalKeys"] = json!(removals);
    }
    let devices = roster.extra.get("devices").unwrap_or(&Value::Null);
    let phones = json!({
        "phones": devices.get("phones").cloned().unwrap_or_else(|| json!([])),
        "removed": devices.get("removed").cloned().unwrap_or_else(|| json!([])),
        "setup": setup.position.id, "epoch": setup.position.epoch,
    });

    let mut live = Vec::new();
    let mut changed = false;
    for (kind, body) in [(Kind::Peers, peers), (Kind::Phones, phones)] {
        let b = body_hash(&body);
        let before = previous(link, kind);
        let dirty = before.as_ref().is_none_or(|counter| counter.b != b);
        let n = before.as_ref().map_or(0, |counter| counter.n) + u64::from(dirty);
        let envelope = documents::sign(&identity.seed, kind, n, object(body))?;
        let doc = documents::verify(envelope.clone())?;
        if dirty {
            let counters = link.extra.entry("own").or_insert_with(|| json!({}));
            let fields = counters
                .as_object_mut()
                .ok_or(Error::Invalid("signed document counters"))?;
            fields.insert(
                bucket(kind).unwrap().into(),
                json!(Counter {
                    n,
                    h: doc.digest(),
                    b
                }),
            );
            changed = true;
        }
        live.push((kind, envelope));
    }
    Ok((live, changed))
}

/// These are the main's original signatures. A member can forward them after reconnecting.
pub(crate) fn cached(link: &Link, kind: Kind) -> Option<Document> {
    let setup = link.setup().ok()?;
    let source = if let Some(bucket) = bucket(kind) {
        link.extra
            .get("docs")?
            .get(format!("{} {bucket}", setup.sign))?
    } else if kind == Kind::SetupShared {
        link.extra.get("setupReplica")?
    } else {
        return None;
    };
    let doc = documents::verify(serde_json::from_value(source.clone()).ok()?).ok()?;
    if doc.kind != kind || encode(&doc.origin) != setup.sign {
        return None;
    }
    let current = match kind {
        Kind::Peers => doc
            .body
            .get("setup")
            .and_then(|value| serde_json::from_value::<Position>(value.clone()).ok())
            .is_some_and(|position| position == setup.position),
        Kind::Phones => {
            doc.body.get("epoch").and_then(Value::as_u64) == Some(setup.position.epoch)
                && doc.body.get("setup").and_then(Value::as_str) == Some(&setup.position.id)
        }
        Kind::SetupShared => {
            doc.n == setup.position.epoch
                && doc.body.get("id").and_then(Value::as_str) == Some(&setup.position.id)
        }
        _ => false,
    };
    current.then_some(doc)
}

pub(crate) fn header(link: &Link, exports: &[Export]) -> Vec<DigestEntry> {
    let leader = link
        .setup()
        .ok()
        .and_then(|setup| link.leadership.get(&setup.position.epoch))
        .into_iter()
        .cloned();
    leader
        .chain(exports.iter().map(|(_, export)| export.clone()))
        .chain(cached(link, Kind::SetupShared).map(|doc| doc.envelope))
        .chain(
            link.extra
                .get("docs")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(Map::values)
                .filter_map(|value| serde_json::from_value::<Envelope>(value.clone()).ok()),
        )
        .filter_map(|value| {
            let doc = documents::verify(value).ok()?;
            Some(DigestEntry(
                encode(&doc.origin),
                doc.kind,
                doc.n,
                doc.digest(),
            ))
        })
        .collect()
}

pub(crate) fn take(link: &Link, doc: &Document) -> Result<Option<Link>, Error> {
    let setup = link.setup()?;
    let kind = doc.kind;
    let sign = encode(&doc.origin);
    if !matches!(kind, Kind::Peers | Kind::Phones) || setup.sign != sign {
        return Ok(None);
    }
    match kind {
        Kind::Peers => {
            let position = doc
                .body
                .get("setup")
                .and_then(|value| serde_json::from_value::<Position>(value.clone()).ok());
            if position.as_ref() != Some(&setup.position)
                || Membership::parse(&Value::Object(doc.body.clone()), &setup.position, &sign)
                    .is_err()
            {
                return Ok(None);
            }
        }
        Kind::Phones => {
            let scope = doc.body.get("setup");
            let old_scope = link
                .extra
                .get("formerPhoneIssuers")
                .and_then(|history| history.get(&sign))
                .and_then(Value::as_array);
            if doc.body.get("epoch").and_then(Value::as_u64) != Some(setup.position.epoch)
                || scope.is_some_and(|id| id.as_str() != Some(&setup.position.id))
                || (scope.is_none()
                    && old_scope.is_some_and(|rows| {
                        rows.iter()
                            .any(|id| id.as_str() != Some(&setup.position.id))
                    }))
            {
                return Ok(None);
            }
        }
        _ => unreachable!("checked the export kind"),
    }
    let mut next = link.clone();
    let bucket = bucket(kind).expect("a roster or access document");
    if setup.role == Role::Primary {
        if doc.n > (1 << 52) || previous(link, kind).is_some_and(|old| old.n >= doc.n) {
            return Ok(None);
        }
        let counters = next
            .extra
            .entry("own")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or(Error::Invalid("signed document counters"))?;
        counters.insert(
            bucket.into(),
            json!(Counter {
                n: doc.n,
                h: doc.digest(),
                b: String::new()
            }),
        );
        return Ok(Some(next));
    }
    let stored = next
        .extra
        .entry("docs")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or(Error::Invalid("stored signed documents"))?;
    let key = format!("{sign} {bucket}");
    let had = stored
        .get(&key)
        .cloned()
        .and_then(|envelope| documents::verify(serde_json::from_value(envelope).ok()?).ok());
    if had
        .as_ref()
        .is_some_and(|old| old.n > doc.n || (old.n == doc.n && old.digest() <= doc.digest()))
    {
        return Ok(None);
    }
    stored.insert(key, json!(doc.envelope));
    Ok(Some(next))
}

pub(crate) fn recover_count(
    link: &mut Link,
    identity: &Identity,
    entries: &[DigestEntry],
) -> Result<bool, Error> {
    let mut changed = false;
    if link.setup()?.role != Role::Primary {
        return Ok(false);
    }
    for DigestEntry(origin, kind, sequence, hash) in entries {
        if origin != &identity.sign
            || !matches!(kind, Kind::Peers | Kind::Phones)
            || !(1..=(1 << 52)).contains(sequence)
            || !safe_token(hash, 11, 11)
            || previous(link, *kind).is_some_and(|old| old.n >= *sequence)
        {
            continue;
        }
        let counters = link
            .extra
            .entry("own")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or(Error::Invalid("signed document counters"))?;
        counters.insert(
            bucket(*kind).unwrap().into(),
            json!(Counter {
                n: *sequence,
                h: hash.clone(),
                b: String::new()
            }),
        );
        changed = true;
    }
    Ok(changed)
}
