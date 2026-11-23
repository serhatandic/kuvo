use std::collections::HashSet;

use kuvo_wire::{
    documents::{self, Document, Envelope, Kind},
    keys::{decode_exact, encode},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    Error,
    model::{
        Identity, Link, Outcomes, Position, Shared, membership, safe_token, validate_position,
        validate_shared,
    },
    object,
    storage::Files,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Certificate {
    pub setup: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    pub relay_term: u64,
    pub checkpoint: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub id: String,
    pub shared: Shared,
    pub archives: Vec<String>,
}

fn digest(doc: &Document) -> String {
    let mut hash = Sha256::new();
    if doc.body.get("archive").and_then(|value| value.as_bool()) == Some(true) {
        hash.update(b"outcomes\0");
        hash.update(serde_json::to_vec(&doc.body).expect("document body serializes"));
    } else {
        hash.update(doc.envelope.d.as_bytes());
    }
    encode(&hash.finalize())
}

fn location(hash: &str) -> Result<String, Error> {
    if hash.len() != 43 || decode_exact::<32>(hash).is_none() {
        return Err(Error::Invalid("checkpoint hash"));
    }
    Ok(format!("run/setup-checkpoints/{hash}.json"))
}

pub(crate) fn read(files: &Files, hash: &str) -> Result<Document, Error> {
    let envelope = files
        .read(&location(hash)?)?
        .ok_or(Error::Invalid("missing setup checkpoint"))?;
    let doc = documents::verify(envelope)?;
    if doc.kind != Kind::SetupCheckpoint || digest(&doc) != hash {
        return Err(Error::Invalid("checkpoint hash"));
    }
    Ok(doc)
}

pub(crate) fn preserve(files: &Files, doc: &Document) -> Result<(), Error> {
    let hash = digest(doc);
    if read(files, &hash).is_ok() {
        return Ok(());
    }
    files.write(&location(&hash)?, &doc.envelope)
}

fn body<T: serde::de::DeserializeOwned>(doc: &Document) -> Result<T, Error> {
    serde_json::from_value(serde_json::Value::Object(doc.body.clone()))
        .map_err(|_| Error::Invalid("checkpoint fields"))
}

pub(crate) fn certificate(doc: &Document) -> Result<Certificate, Error> {
    if doc.kind != Kind::Leadership
        || doc.body.contains_key("standby")
        || doc.body.contains_key("successorChain")
    {
        return Err(Error::Version("setup leadership"));
    }
    let cert: Certificate = body(doc)?;
    validate_position(&cert.setup)?;
    location(&cert.checkpoint)?;
    if doc.n != cert.setup.epoch || cert.relay_term == 0 {
        return Err(Error::Invalid("leadership epoch or relay term"));
    }
    Ok(cert)
}

pub(crate) fn manifest(doc: &Document) -> Result<Manifest, Error> {
    if doc.kind != Kind::SetupCheckpoint || doc.body.contains_key("archive") {
        return Err(Error::Invalid("setup checkpoint manifest"));
    }
    let manifest: Manifest = body(doc)?;
    validate_shared(&manifest.shared)?;
    let mut seen = HashSet::new();
    for reference in &manifest.archives {
        location(reference)?;
        if !seen.insert(reference) {
            return Err(Error::Invalid("duplicate outcome archive"));
        }
    }
    Ok(manifest)
}

#[derive(Deserialize)]
struct Archive {
    archive: bool,
    id: String,
    outcomes: Outcomes,
}

pub(crate) fn outcomes(doc: &Document, setup: &str, epoch: u64) -> Result<Outcomes, Error> {
    let page: Archive = body(doc)?;
    if doc.kind != Kind::SetupCheckpoint
        || !page.archive
        || page.id != setup
        || page.outcomes.iter().any(|(operation, outcome)| {
            !safe_token(operation, 22, 43)
                || outcome.epoch == 0
                || outcome.epoch >= epoch
                || outcome.sequence == 0
                || outcome.base_revision != outcome.sequence - 1
        })
    {
        return Err(Error::Invalid("setup outcome archive"));
    }
    Ok(page.outcomes)
}

pub(crate) fn build(
    files: &Files,
    identity: &Identity,
    next: Position,
    previous: Option<String>,
    term: u64,
    link: &Link,
) -> Result<Document, Error> {
    validate_position(&next)?;
    validate_shared(&link.shared)?;
    membership(&link.shared, &next, &identity.sign)?;
    let mut entries: Vec<_> = link.shared_outcomes.iter().collect();
    entries.sort_by(|(ka, a), (kb, b)| {
        (a.epoch, a.sequence)
            .cmp(&(b.epoch, b.sequence))
            .then_with(|| json!(a.kind).as_str().cmp(&json!(b.kind).as_str()))
            .then_with(|| ka.cmp(kb))
    });
    let mut archives = Vec::new();
    for entries in entries.chunks(256) {
        let ordered: serde_json::Map<_, _> = entries
            .iter()
            .map(|(operation, outcome)| ((*operation).clone(), json!(outcome)))
            .collect();
        let page = documents::verify(documents::sign(
            &identity.seed,
            Kind::SetupCheckpoint,
            1,
            object(json!({"archive": true, "id": next.id, "outcomes": ordered})),
        )?)?;
        outcomes(&page, &next.id, next.epoch)?;
        preserve(files, &page)?;
        archives.push(digest(&page));
    }
    let checkpoint = documents::verify(documents::sign(
        &identity.seed,
        Kind::SetupCheckpoint,
        next.epoch,
        object(json!({"id": next.id, "shared": link.shared, "archives": archives})),
    )?)?;
    let certificate = Certificate {
        setup: next,
        previous,
        relay_term: term,
        checkpoint: digest(&checkpoint),
    };
    let signed = documents::verify(documents::sign(
        &identity.seed,
        Kind::Leadership,
        certificate.setup.epoch,
        object(json!(certificate)),
    )?)?;
    preserve(files, &checkpoint)?;
    Ok(signed)
}

pub(crate) fn resolve(
    files: &Files,
    doc: &Document,
    cert: &Certificate,
) -> Result<(Shared, Outcomes), Error> {
    let checkpoint = read(files, &cert.checkpoint)?;
    let manifest = manifest(&checkpoint)?;
    if manifest.id != cert.setup.id || doc.origin != checkpoint.origin || checkpoint.n != doc.n {
        return Err(Error::Invalid("leadership checkpoint binding"));
    }
    membership(&manifest.shared, &cert.setup, &encode(&doc.origin))?;
    let mut all = Outcomes::new();
    for reference in manifest.archives {
        for (operation, outcome) in outcomes(&read(files, &reference)?, &cert.setup.id, doc.n)? {
            if all.insert(operation, outcome).is_some() {
                return Err(Error::Invalid("duplicate archived setup operation"));
            }
        }
    }
    Ok((manifest.shared, all))
}

pub(crate) fn packet(files: &Files, envelope: &Envelope) -> Result<Vec<Envelope>, Error> {
    let doc = documents::verify(envelope.clone())?;
    let cert = certificate(&doc)?;
    let checkpoint = read(files, &cert.checkpoint)?;
    let manifest = manifest(&checkpoint)?;
    let mut packet = vec![checkpoint.envelope];
    for reference in manifest.archives {
        packet.push(read(files, &reference)?.envelope);
    }
    packet.push(envelope.clone());
    Ok(packet)
}

pub(crate) fn reference(doc: &Document) -> String {
    digest(doc)
}
