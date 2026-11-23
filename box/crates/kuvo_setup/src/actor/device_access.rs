//! Managed device mutations use the signed membership writer on the current main.
//! Peer routes can call this adapter after authenticating their caller.

use std::time::Duration;

use hmac::{Hmac, Mac};
use kuvo_server::{Caller, LocalOrigin, Transport};
use kuvo_wire::{
    documents::{self, Kind},
    keys::{decode_exact, encode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::{sync::oneshot, time::Instant};

use super::{Command, Owner, Request, Response, SetupHandle};
use crate::{
    Error, Receipt, SharedKind,
    edits::Submitter,
    model::{Role, Status, fingerprint, matches_grant, receipt, safe_token},
    preferences::short_name,
    random,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ManagedDeviceChange {
    PairDesktop { key: String, name: String },
    Revoke { grants: Vec<ManagedDeviceGrant> },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ManagedDeviceGrant {
    pub key: String,
    pub pairing: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedDeviceSave {
    pub pairing: Option<String>,
    pub receipt: Option<Receipt>,
}

impl SetupHandle {
    /// Commit a managed device change or forward it to the current main.
    /// Reuse `operation` for retries of the same change by the same authenticated caller.
    pub async fn change_devices(
        &self,
        caller: &Caller,
        operation: Option<String>,
        change: ManagedDeviceChange,
    ) -> Result<ManagedDeviceSave, Error> {
        let submitter = match caller {
            Caller::Local(Transport::Http {
                origin: LocalOrigin::App | LocalOrigin::Mac,
                ..
            }) => Submitter::Computer,
            Caller::Paired { pairing, .. }
                if pairing.is_current() && matches!(change, ManagedDeviceChange::Revoke { .. }) =>
            {
                Submitter::Paired {
                    key: encode(&pairing.key),
                    pairing: pairing.id.clone(),
                }
            }
            _ => return Err(Error::DevicePermission),
        };
        let operation = operation.unwrap_or(encode(&random::<16>()?));
        let mutation = Mutation {
            operation,
            submitter,
            change,
        };
        mutation.validate()?;
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(Request {
                command: Command::DeviceChange(mutation),
                reply,
            })
            .await
            .map_err(|_| Error::Conflict)?;
        let saved = answer.await.map_err(|_| Error::Conflict)??;
        serde_json::from_value(saved.value).map_err(|_| Error::Invalid("device save reply"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Mutation {
    operation: String,
    submitter: Submitter,
    change: ManagedDeviceChange,
}

impl Mutation {
    fn validate(&self) -> Result<(), Error> {
        let key = |key: &str| safe_token(key, 43, 43) && decode_exact::<32>(key).is_some();
        if !safe_token(&self.operation, 22, 43) {
            return Err(Error::Invalid("device operation"));
        }
        match &self.change {
            ManagedDeviceChange::PairDesktop { key: device, name }
                if key(device)
                    && name.encode_utf16().count() <= 100
                    && self.submitter == Submitter::Computer =>
            {
                Ok(())
            }
            ManagedDeviceChange::Revoke { grants }
                if !grants.is_empty()
                    && grants.len() <= 100
                    && grants
                        .iter()
                        .all(|grant| key(&grant.key) && safe_token(&grant.pairing, 0, 32)) =>
            {
                Ok(())
            }
            _ => Err(Error::Invalid("managed device change")),
        }
    }
}

pub(super) struct Pending {
    primary: String,
    epoch: u64,
    operation: String,
    pair: bool,
    expires: Instant,
    reply: oneshot::Sender<Result<Response, Error>>,
}

impl Owner {
    pub(super) async fn ask_device_change(
        &mut self,
        mutation: Mutation,
        reply: oneshot::Sender<Result<Response, Error>>,
    ) {
        if let Err(error) = mutation.validate() {
            let _ = reply.send(Err(error));
            return;
        }
        let setup = match self.link.setup() {
            Ok(setup) => setup.clone(),
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        if setup.role == Role::Primary {
            let saved = self
                .apply_device_change(&mutation, None)
                .await
                .map(|result| Response::ok(json!(result)));
            let _ = reply.send(saved);
            return;
        }
        if !mutation.submitter.current(&self.link) || self.pending_devices.len() >= 32 {
            let _ = reply.send(Err(Error::DevicePermission));
            return;
        }
        let request = match random::<16>() {
            Ok(bytes) => encode(&bytes),
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let primary = setup.position.primary;
        if !self
            .send_authority(
                &primary,
                json!({"kind": "device-save", "request": request, "mutation": mutation}),
            )
            .await
        {
            let _ = reply.send(Err(Error::Fenced));
            return;
        }
        self.pending_devices.insert(
            request,
            Pending {
                primary,
                epoch: setup.position.epoch,
                operation: mutation.operation.clone(),
                pair: matches!(mutation.change, ManagedDeviceChange::PairDesktop { .. }),
                expires: Instant::now() + Duration::from_secs(20),
                reply,
            },
        );
    }

    pub(super) fn expire_device_changes(&mut self) {
        let now = Instant::now();
        let expired: Vec<_> = self
            .pending_devices
            .iter()
            .filter(|(_, pending)| pending.expires <= now || pending.reply.is_closed())
            .map(|(request, _)| request.clone())
            .collect();
        for request in expired {
            if let Some(pending) = self.pending_devices.remove(&request) {
                let _ = pending.reply.send(Err(Error::Fenced));
            }
        }
    }

    pub(super) async fn receive_device_change(
        &mut self,
        member: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let setup = self.link.setup()?;
        if setup.role != Role::Primary || value["epoch"].as_u64() != Some(setup.position.epoch) {
            return Ok(());
        }
        let Some(request) = value["request"]
            .as_str()
            .filter(|request| safe_token(request, 22, 22))
        else {
            return Ok(());
        };
        let mutation: Mutation = serde_json::from_value(value["mutation"].clone())
            .map_err(|_| Error::Invalid("forwarded device change"))?;
        let result = self.apply_device_change(&mutation, Some(member)).await;
        let answer = match result {
            Ok(saved) => json!({"saved": saved}),
            Err(Error::DevicePermission) => json!({"error": "permission"}),
            Err(Error::Fenced) => json!({"error": "fenced"}),
            Err(Error::DeviceRemoved) => json!({"error": "removed"}),
            Err(Error::Capacity) => json!({"error": "capacity"}),
            Err(Error::Invalid(_)) => json!({"error": "invalid"}),
            Err(_) => json!({"error": "conflict"}),
        };
        self.send_authority(
            member,
            json!({"kind": "device-saved", "request": request, "answer": answer}),
        )
        .await;
        Ok(())
    }

    pub(super) fn receive_device_result(&mut self, from: &str, value: &Value) -> Result<(), Error> {
        let Some(request) = value["request"].as_str() else {
            return Ok(());
        };
        if !self.pending_devices.get(request).is_some_and(|pending| {
            pending.primary == from
                && value["epoch"].as_u64() == Some(pending.epoch)
                && pending.expires > Instant::now()
        }) {
            return Ok(());
        }
        let pending = self
            .pending_devices
            .remove(request)
            .expect("device request was pending");
        let answer = &value["answer"];
        let result = if let Some(saved) = answer.get("saved") {
            self.verify_device_result(&pending, saved)
                .map(|()| Response::ok(saved.clone()))
        } else {
            Err(match answer["error"].as_str() {
                Some("permission") => Error::DevicePermission,
                Some("fenced") => Error::Fenced,
                Some("removed") => Error::DeviceRemoved,
                Some("capacity") => Error::Capacity,
                Some("invalid") => Error::Invalid("forwarded device change"),
                _ => Error::Conflict,
            })
        };
        let _ = pending.reply.send(result);
        Ok(())
    }

    fn verify_device_result(&self, pending: &Pending, answer: &Value) -> Result<(), Error> {
        let setup = self.link.setup()?;
        let own = self.own()?;
        let saved: ManagedDeviceSave = serde_json::from_value(answer.clone())
            .map_err(|_| Error::Invalid("device save reply"))?;
        let pairing_matches = match saved.pairing.as_deref() {
            Some(pairing) => pending.pair && safe_token(pairing, 0, 32),
            None => !pending.pair,
        };
        if !pairing_matches
            || setup.position.primary != pending.primary
            || setup.position.epoch != pending.epoch
        {
            return Err(Error::Conflict);
        }
        let Some(receipt) = saved.receipt else {
            return if pending.pair {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        };
        let doc = documents::verify(receipt.proof)?;
        if receipt.status != "committed"
            || receipt.operation != pending.operation
            || receipt.setup != setup.position.id
            || receipt.epoch != pending.epoch
            || encode(&doc.origin) != setup.sign
            || doc.kind != Kind::SetupShared
            || doc.body.get("id").and_then(Value::as_str) != Some(&receipt.setup)
            || doc.body.get("epoch").and_then(Value::as_u64) != Some(receipt.epoch)
            || doc.body.get("operation").and_then(Value::as_str) != Some(&receipt.operation)
            || doc.body.get("kind").and_then(Value::as_str) != Some("membershipAndAccess")
            || doc.body.get("requestingMember").and_then(Value::as_str) != Some(&own.public)
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    async fn apply_device_change(
        &mut self,
        mutation: &Mutation,
        member: Option<&str>,
    ) -> Result<ManagedDeviceSave, Error> {
        mutation.validate()?;
        let setup = self.link.setup()?.clone();
        let own = self.own()?;
        let roster = self.roster()?;
        if setup.role != Role::Primary
            || member.is_some_and(|key| {
                !roster
                    .boxes
                    .iter()
                    .any(|peer| peer.key == key && peer.sign.is_some())
                    || self.provisional(key)
            })
        {
            return Err(Error::DevicePermission);
        }
        let change = &mutation.change;
        let operation = &mutation.operation;
        let digest = fingerprint(&json!({"mutation": mutation, "requestingMember": member}));
        let is_computer = |key: &str| roster.boxes.iter().any(|peer| peer.key == key);
        let new_pairing = || {
            let mut mac =
                Hmac::<Sha256>::new_from_slice(&own.seed).expect("HMAC accepts any signing seed");
            mac.update(b"kuvo-setup desktop pairing v1\0");
            mac.update(setup.position.id.as_bytes());
            mac.update(digest.as_bytes());
            encode(&mac.finalize().into_bytes()[..12])
        };

        if let Some(previous) = self.link.shared_outcomes.get(operation) {
            if previous.kind != SharedKind::MembershipAndAccess
                || previous.status != Status::Committed
                || previous.epoch != setup.position.epoch
                || previous.mutation_digest.as_deref() != Some(&digest)
            {
                return Err(Error::Conflict);
            }
            let original = receipt(own, &setup.position, operation.clone(), previous)?;
            return Ok(ManagedDeviceSave {
                pairing: matches!(change, ManagedDeviceChange::PairDesktop { .. })
                    .then(new_pairing),
                receipt: Some(bind(original, &own.seed, member)?),
            });
        }
        self.can_write()?;
        if !mutation.submitter.current(&self.link) {
            return Err(Error::DevicePermission);
        }
        let mut membership = self
            .link
            .value(SharedKind::MembershipAndAccess)
            .cloned()
            .ok_or(Error::Conflict)?;
        let mut phones: Vec<_> = membership["devices"]["phones"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut removed: Vec<_> = membership["devices"]["removed"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let pairing = match change {
            ManagedDeviceChange::PairDesktop { key, name } => {
                if self.link.devices_disabled || self.link.relay.is_none() || is_computer(key) {
                    return Err(Error::DevicePermission);
                }
                if removed.iter().any(|entry| entry["key"] == key.as_str()) {
                    return Err(Error::DeviceRemoved);
                }
                if let Some(kept) = phones.iter().find(|entry| entry["key"] == key.as_str()) {
                    if kept["kind"] != "desktop" {
                        return Err(Error::Conflict);
                    }
                    return Ok(ManagedDeviceSave {
                        pairing: Some(kept["pairing"].as_str().unwrap_or_default().to_owned()),
                        receipt: None,
                    });
                }
                if phones.len() >= 64 {
                    return Err(Error::Capacity);
                }
                let pairing = new_pairing();
                phones.push(json!({"key": key, "name": short_name(name, 100), "pairing": pairing, "kind": "desktop"}));
                Some(pairing)
            }
            ManagedDeviceChange::Revoke { grants } => {
                for grant in grants {
                    if is_computer(&grant.key)
                        || !phones
                            .iter()
                            .any(|entry| matches_grant(entry, &grant.key, &grant.pairing))
                    {
                        return Err(Error::Invalid("devices to revoke"));
                    }
                }
                for grant in grants {
                    phones.retain(|entry| !matches_grant(entry, &grant.key, &grant.pairing));
                    if !removed
                        .iter()
                        .any(|entry| matches_grant(entry, &grant.key, &grant.pairing))
                    {
                        removed.push(json!(grant));
                    }
                }
                None
            }
        };
        membership["devices"] = json!({"phones": phones, "removed": removed});
        let mut next = self.link.clone();
        let unbound = next.put(
            own,
            SharedKind::MembershipAndAccess,
            membership,
            None,
            Some(operation.clone()),
        )?;
        next.shared_outcomes
            .get_mut(operation)
            .expect("device change recorded an outcome")
            .mutation_digest = Some(digest);
        let receipt = bind(unbound, &own.seed, member)?;
        self.commit(next).await?;
        if let Err(error) = self.publish(false).await {
            eprintln!("publishing accepted device grants: {error}");
        }
        Ok(ManagedDeviceSave {
            pairing,
            receipt: Some(receipt),
        })
    }
}

fn bind(mut receipt: Receipt, seed: &[u8; 32], member: Option<&str>) -> Result<Receipt, Error> {
    if let Some(member) = member {
        let mut doc = documents::verify(receipt.proof)?;
        doc.body.insert("requestingMember".into(), json!(member));
        receipt.proof = documents::sign(seed, Kind::SetupShared, doc.n, doc.body)?;
    }
    Ok(receipt)
}
