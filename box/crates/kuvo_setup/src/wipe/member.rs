//! Persist only the authenticated completion and a fresh identity outside the erase boundary.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_peers::{
    relay::{Config, Relay, RelayEvent},
    session::{BoxHello, ComputerKind},
};
use kuvo_store::files;
use kuvo_wire::{
    documents::{self, Kind},
    frames,
    keys::{Address, Host, decode, decode_exact, encode},
    mail::Envelope,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{identity, parent, private, valid_removal};
use crate::{
    Error,
    model::{Identity, Link, Peer, Role},
    object, random,
};

#[derive(Deserialize, Serialize)]
pub(crate) struct MemberWipe {
    pub setup: String,
    pub operation: String,
    pub primary: Peer,
    pub source: String,
}

impl MemberWipe {
    pub(super) fn from(link: &Link, authenticated_key: &str) -> Option<Self> {
        let kept = link.extra.get("localWipe")?;
        if ["backup", "generation", "requiresPreparation"]
            .iter()
            .any(|field| kept.get(field).is_some())
        {
            return None;
        }
        let wipe: Self = serde_json::from_value(kept.clone()).ok()?;
        let setup = link.setup().ok()?;
        (setup.role == Role::Member
            && !setup.aside
            && wipe.setup == setup.position.id
            && wipe.primary.key == setup.position.primary
            && wipe.primary.sign.as_deref() == Some(&setup.sign)
            && wipe.source == authenticated_key
            && valid_removal(&wipe.setup, &wipe.operation)
            && Host::from_text(&wipe.primary.key, &wipe.primary.address).is_some())
        .then_some(wipe)
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct Completion {
    setup: String,
    operation: String,
    primary: String,
    relay: String,
    reply: String,
    secret: String,
    done: String,
    proof: documents::Envelope,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
}

impl Completion {
    fn new(link: &Link, wipe: &MemberWipe) -> Result<Self, Error> {
        let own = Identity::read(link)?;
        let relay = link
            .relay
            .clone()
            .ok_or(Error::Invalid("wipe completion relay"))?;
        let target = Host::from_text(&wipe.primary.key, &wipe.primary.address)
            .ok_or(Error::Invalid("wipe completion peer"))?;
        let reply = random::<32>()?;
        let secret = random::<32>()?;
        let receipt = encode(&kuvo_wire::keys::Identity::from_bytes(reply, secret).public());
        let sealed = Envelope(object(
            json!({"done": wipe.operation, "setup": wipe.setup, "receipt": receipt}),
        ))
        .seal_removal(&own.key, &target.key)
        .map_err(|_| Error::Invalid("wipe completion mail"))?;
        let proof = documents::sign(
            &own.seed,
            Kind::SetupShared,
            1,
            object(json!({
                "removal": "completed", "setup": wipe.setup, "operation": wipe.operation, "source": own.public,
            })),
        )?;
        Ok(Self {
            setup: wipe.setup.clone(),
            operation: wipe.operation.clone(),
            primary: wipe.primary.key.clone(),
            relay,
            reply: encode(&reply),
            secret: encode(&secret),
            done: encode(&frames::relay_send(&target.address, &sealed)),
            proof,
            deleted: false,
        })
    }

    pub(super) fn prepare(receipts: &Path, link: &Link, wipe: &MemberWipe) -> io::Result<()> {
        if let Some(kept) = Self::read(receipts, &wipe.setup, &wipe.operation)? {
            if kept.primary == wipe.primary.key && !kept.deleted {
                return Ok(());
            }
            return Err(io::Error::other(
                "A different wipe completion is still pending.",
            ));
        }
        let completion = Self::new(link, wipe).map_err(io::Error::other)?;
        let parent = parent(receipts)?;
        files::create_private_dirs(receipts)?;
        private(receipts, true)?;
        files::write_atomic_through(
            &completion.file(receipts),
            &serde_json::to_vec(&completion)?,
            parent,
        )
    }

    pub(super) fn read(receipts: &Path, setup: &str, operation: &str) -> io::Result<Option<Self>> {
        if !valid_removal(setup, operation) || !private(receipts, true)? {
            return Ok(None);
        }
        let file = receipts.join(format!("{operation}.json"));
        if !private(&file, false)? || file.metadata()?.len() > 4096 {
            return Ok(None);
        }
        let kept: Self = serde_json::from_slice(&fs::read(file)?)?;
        Ok((kept.setup == setup && kept.operation == operation).then_some(kept))
    }

    fn file(&self, receipts: &Path) -> PathBuf {
        receipts.join(format!("{}.json", self.operation))
    }

    pub(super) async fn publish(mut self, root: &Path, receipts: &Path) -> io::Result<()> {
        if identity(root)?.is_some() {
            return Err(io::Error::other(
                "The wipe’s completion receipt is unavailable.",
            ));
        }
        let ack = receipts.join(format!("{}.ack", self.operation));
        let file = self.file(receipts);
        let parent = parent(receipts)?;
        files::write_atomic_through(&ack, &serde_json::to_vec(&self.proof)?, parent)?;
        self.deleted = true;
        files::write_atomic_through(&file, &serde_json::to_vec(&self)?, parent)?;
        if tokio::time::timeout(Duration::from_secs(16), self.report())
            .await
            .is_ok_and(|answer| answer == Some(true))
        {
            files::remove_file_synced(&file)?;
            files::remove_file_synced(&ack)?;
        }
        Ok(())
    }

    async fn report(&self) -> Option<bool> {
        let stored = decode(&self.done)?;
        let (2, address, frame) = split_relay_send(&stored)? else {
            return None;
        };
        let primary = decode_exact::<32>(&self.primary)?;
        let link = Relay::start(Config {
            url: self.relay.clone(),
            key: decode_exact(&self.reply)?,
            secret: decode_exact(&self.secret)?,
            hello: BoxHello {
                v: 1,
                kind: ComputerKind::Machine,
                name: String::new(),
                app: String::new(),
                engine: None,
            },
            peers: Vec::new(),
            grants: Default::default(),
        });
        let handle = link.handle();
        let mut updates = handle.subscribe()?;
        let mut retry = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = retry.tick() => { let _ = handle.send_removal_completion(*address, frame.to_vec()).await; }
                update = updates.recv() => match update.as_deref() {
                    Ok(RelayEvent::Removal { hinted_key, envelope, .. })
                        if hinted_key == &primary && envelope.0.get("setup").and_then(serde_json::Value::as_str) == Some(&self.setup)
                        && envelope.0.get("recorded").and_then(serde_json::Value::as_str) == Some(&self.operation) => return Some(true),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Some(false),
                    _ => {}
                }
            }
        }
    }
}

fn split_relay_send(bytes: &[u8]) -> Option<(u8, &Address, &[u8])> {
    let (kind, data) = bytes.split_first()?;
    let (address, frame) = data.split_first_chunk::<16>()?;
    Some((*kind, address, frame))
}
