//! Signed plain unlink, as the main computer's writer. A removed peer uses
//! removal mail for the final handshake after ordinary peer trust is revoked.

use kuvo_wire::{
    documents::{self, Kind},
    keys::{Host, decode_exact, encode},
    mail::Envelope,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Owner, Response, with_receipt};
use crate::{
    Error,
    edits::Submitter,
    installer,
    model::{Membership, Peer, Role, SharedKind, receipt, safe_token},
    object, random,
};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Unlink {
    operation: Option<String>,
    wipe: Option<bool>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Removal {
    peer: Peer,
    operation: String,
    wipe: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    delivered: bool,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl Removal {
    fn plain(&self) -> bool {
        !self.wipe
            && ![
                "local",
                "resolution",
                "plan",
                "generation",
                "requiresPreparation",
            ]
            .iter()
            .any(|field| self.extra.contains_key(*field))
    }

    fn host(&self) -> Option<Host> {
        Host::from_text(&self.peer.key, &self.peer.address)
    }
}

impl Owner {
    pub(super) async fn unlink_computer(
        &mut self,
        key: String,
        request: Unlink,
        explicit: bool,
        by: Submitter,
    ) -> Result<Response, Error> {
        if request
            .operation
            .as_deref()
            .is_some_and(|op| !safe_token(op, 22, 43))
            || request.extra.contains_key("save")
        {
            return Ok(answer(400, "Send a valid removal request."));
        }
        if decode_exact::<32>(&key).is_none() {
            return Ok(answer(404, "That isn’t one of your computers."));
        }
        if !by.current(&self.link) {
            return Ok(answer(403, "The submitting device no longer has access."));
        }
        let setup = self.link.setup()?.clone();
        if setup.role != Role::Primary {
            return Ok(answer(409, "Only the main computer can remove a computer."));
        }
        if key == setup.position.primary {
            return Ok(answer(409, "Change the main computer before unlinking it."));
        }
        if !explicit && request.wipe == Some(true)
            || ["prepare", "retryPreparation", "resolve", "supersedes"]
                .iter()
                .any(|field| request.extra.contains_key(*field))
        {
            return Ok(answer(409, "Wipe preparation is unavailable."));
        }

        let mut roster = self.roster()?;
        if let Some(record) = removal_for(&roster, &key) {
            if !record.plain()
                || request
                    .operation
                    .as_ref()
                    .is_some_and(|op| *op != record.operation)
            {
                return Ok(answer(
                    409,
                    "This computer has already been removed with a different request.",
                ));
            }
            let saved = self
                .link
                .shared_outcomes
                .get(&record.operation)
                .filter(|outcome| {
                    outcome.epoch == setup.position.epoch
                        && outcome.kind == SharedKind::MembershipAndAccess
                })
                .map(|outcome| {
                    receipt(
                        self.own()?,
                        &setup.position,
                        record.operation.clone(),
                        outcome,
                    )
                })
                .transpose()?;
            return Ok(unlinked(&record.operation, saved));
        }
        let Some(at) = roster.boxes.iter().position(|peer| peer.key == key) else {
            return Ok(answer(404, "That isn't one of your computers."));
        };
        self.can_write()?;
        if roster
            .extra
            .get("wipePreparations")
            .is_some_and(|records| records.get(&key).is_some())
        {
            return Ok(answer(
                409,
                "The wipe preparation changed. Refresh the removal and try again.",
            ));
        }
        let operation = match request.operation {
            Some(operation) => operation,
            None => encode(&random::<16>()?),
        };
        if self.link.shared_outcomes.contains_key(&operation) {
            return Ok(answer(
                409,
                "This setup already has a decision for this removal.",
            ));
        }
        let mut next = self.link.clone();
        let mut recoveries = installer::recoveries(&next, &roster);
        if recoveries.shift_remove(&key).is_some() {
            installer::keep_recoveries(&mut next, &mut roster, recoveries)?;
        }
        let mut peer = roster.boxes.remove(at);
        peer.extra.shift_remove("id");
        if let Some(name) = next
            .value(SharedKind::SetupNames)
            .and_then(|names| names[&key].as_str())
        {
            peer.name = name.to_owned();
        }
        retire_computer(
            &mut roster,
            &setup.position.id,
            &peer,
            &setup.position.primary,
        )?;
        let record = Removal {
            peer,
            operation: operation.clone(),
            wipe: false,
            delivered: false,
            extra: Default::default(),
        };
        insert_removal(&mut roster, &record)?;
        let proof = next.put(
            self.own()?,
            SharedKind::MembershipAndAccess,
            serde_json::to_value(roster).expect("membership serializes"),
            None,
            Some(operation.clone()),
        )?;
        self.commit(next).await?;
        if let Err(error) = self.offer_unlinks().await {
            eprintln!("publishing removal offer: {error}");
        }
        if let Err(error) = self.publish(false).await {
            eprintln!("publishing accepted removal: {error}");
        }
        Ok(unlinked(&operation, Some(proof)))
    }

    pub(super) async fn offer_unlinks(&self) -> Result<(), Error> {
        if self.network.is_none() || self.can_write().is_err() {
            return Ok(());
        }
        let setup = &self.link.setup()?.position;
        let own = self.own()?;
        let roster = self.roster()?;
        let removals = roster.extra.get("removals").and_then(Value::as_object);
        for (key, value) in removals.into_iter().flatten() {
            let Some(record) =
                read_removal(key, value).filter(|record| record.plain() && !record.delivered)
            else {
                continue;
            };
            let Some(host) = record.host() else { continue };
            let doc = documents::sign(
                &own.seed,
                Kind::SetupShared,
                setup.epoch,
                object(json!({
                    "removal": "offer", "setup": setup.id, "primary": setup.primary,
                    "key": key, "operation": record.operation,
                })),
            )?;
            self.send_removal(&host, json!({"offer": doc})).await;
        }
        Ok(())
    }

    pub(super) async fn receive_unlink(
        &mut self,
        sender: [u8; 32],
        envelope: &Envelope,
    ) -> Result<(), Error> {
        if self.can_write().is_err() {
            return Ok(());
        }
        let setup = self.link.setup()?.position.clone();
        let key = encode(&sender);
        let mut roster = self.roster()?;
        let Some(mut record) = removal_for(&roster, &key).filter(Removal::plain) else {
            return Ok(());
        };
        let Some(peer) = record.host() else {
            return Ok(());
        };
        let message = &envelope.0;
        if message.get("setup").and_then(Value::as_str) != Some(&setup.id) {
            return Ok(());
        }
        if message.get("done").and_then(Value::as_str) == Some(&record.operation)
            && !message.contains_key("generation")
        {
            if !record.delivered {
                record.delivered = true;
                insert_removal(&mut roster, &record)?;
                let mut next = self.link.clone();
                let acks = next.extra.entry("removalAcks").or_insert_with(|| json!({}));
                acks.as_object_mut()
                    .ok_or(Error::Invalid("removal acknowledgments"))?
                    .insert(key.clone(), json!(record.operation));
                next.put(
                    self.own()?,
                    SharedKind::MembershipAndAccess,
                    serde_json::to_value(roster).expect("membership serializes"),
                    None,
                    None,
                )?;
                self.commit(next).await?;
                self.publish(false).await?;
            }
            self.send_removal(
                &peer,
                json!({"recorded": record.operation, "setup": setup.id}),
            )
            .await;
        } else if !record.delivered
            && message.get("request").and_then(Value::as_str) == Some(&record.operation)
            && let Some(nonce) = message
                .get("nonce")
                .and_then(Value::as_str)
                .filter(|nonce| safe_token(nonce, 22, 43))
        {
            let doc = documents::sign(
                &self.own()?.seed,
                Kind::SetupShared,
                setup.epoch,
                object(json!({
                    "removal": "instruction", "setup": setup.id, "primary": setup.primary,
                    "key": key, "operation": record.operation, "wipe": false, "nonce": nonce,
                })),
            )?;
            self.send_removal(&peer, json!({"instruction": doc})).await;
        }
        Ok(())
    }

    async fn send_removal(&self, target: &Host, message: Value) {
        if let Some(network) = &self.network {
            let _ = network
                .sender
                .removal(target.clone(), Envelope(object(message)))
                .await;
        }
    }
}

fn retire_computer(
    roster: &mut Membership,
    setup: &str,
    peer: &Peer,
    primary: &str,
) -> Result<(), Error> {
    roster.left.push(peer.key.clone());
    let revisions = roster
        .extra
        .entry("removalRevisions")
        .or_insert_with(|| json!({}));
    let revisions = revisions
        .as_object_mut()
        .ok_or(Error::Invalid("removal revisions"))?;
    let revision = revisions
        .get(&peer.key)
        .and_then(Value::as_u64)
        .unwrap_or_default();
    revisions.insert(
        peer.key.clone(),
        json!(revision.checked_add(1).ok_or(Error::Capacity)?),
    );

    let history = roster.extra.entry("computerHistory").or_insert_with(|| {
        json!({
            "setup": setup, "slots": [], "retiredSigns": [],
        })
    });
    let slots = history["slots"]
        .as_array_mut()
        .ok_or(Error::Invalid("computer history"))?;
    for key in roster
        .boxes
        .iter()
        .map(|entry| &entry.key)
        .filter(|key| *key != primary)
        .chain(std::iter::once(&peer.key))
    {
        if !slots.iter().any(|slot| slot["key"] == *key) {
            slots.push(json!({"key": key}));
        }
    }
    if let Some(sign) = &peer.sign {
        let signs = history["retiredSigns"]
            .as_array_mut()
            .ok_or(Error::Invalid("retired signers"))?;
        if !signs.iter().any(|old| old == sign) {
            signs.push(json!(sign));
        }
    }
    history["retiredNames"][&peer.key] = json!(peer.name);
    if let Some(merged) = roster
        .extra
        .get_mut("mergedComputers")
        .and_then(Value::as_object_mut)
    {
        merged.shift_remove(&peer.key);
        if merged.is_empty() {
            roster.extra.shift_remove("mergedComputers");
        }
    }
    Ok(())
}

fn read_removal(key: &str, value: &Value) -> Option<Removal> {
    let record: Removal = serde_json::from_value(value.clone()).ok()?;
    (record.peer.key == key && safe_token(&record.operation, 22, 43)).then_some(record)
}

fn removal_for(roster: &Membership, key: &str) -> Option<Removal> {
    read_removal(key, roster.extra.get("removals")?.get(key)?)
}

fn insert_removal(roster: &mut Membership, record: &Removal) -> Result<(), Error> {
    let records = roster.extra.entry("removals").or_insert_with(|| json!({}));
    records
        .as_object_mut()
        .ok_or(Error::Invalid("removals"))?
        .insert(record.peer.key.clone(), json!(record));
    Ok(())
}

fn unlinked(operation: &str, save: Option<crate::Receipt>) -> Response {
    let mut value =
        json!({"removal": {"operation": operation, "state": "unlinked", "accessRemoved": true}});
    if let Some(save) = save {
        value = with_receipt(value, save);
    }
    Response::ok(value)
}

fn answer(status: u16, message: &str) -> Response {
    Response {
        status,
        value: json!({"message": message}),
    }
}
