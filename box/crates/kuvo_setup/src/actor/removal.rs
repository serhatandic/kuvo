//! Signed plain unlink, as the main computer's writer. A removed peer uses
//! removal mail for the final handshake after ordinary peer trust is revoked.

mod cleanup;

use std::time::Duration;

use tokio::time::Instant;

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
    model::{
        ComputerKind, Link, Membership, Peer, Position, Role, Setup, SharedKind, receipt,
        safe_token,
    },
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

pub(super) struct Challenge {
    position: Position,
    operation: String,
    nonce: String,
    expires: Instant,
}

#[derive(Clone, Deserialize, Serialize)]
struct LocalUnlink {
    setup: String,
    operation: String,
    primary: Peer,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    completed: bool,
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
        if self.link.extra.contains_key("localUnlink")
            || self
                .link
                .setup()
                .is_ok_and(|setup| setup.role == Role::Member)
        {
            return self.take_member_unlink(sender, envelope).await;
        }
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

    async fn take_member_unlink(
        &mut self,
        sender: [u8; 32],
        envelope: &Envelope,
    ) -> Result<(), Error> {
        if self.kind != ComputerKind::Machine {
            return Ok(());
        }
        let message = &envelope.0;
        let source = encode(&sender);
        if let Some(pending) = self.local_unlink()? {
            if pending.completed
                && pending.primary.key == source
                && message.get("setup").and_then(Value::as_str) == Some(&pending.setup)
                && message.get("recorded").and_then(Value::as_str) == Some(&pending.operation)
            {
                let mut next = self.link.clone();
                next.extra.shift_remove("localUnlink");
                self.commit(next).await?;
                self.next = Instant::now();
            }
            return Ok(());
        }
        let setup = self.link.setup()?.clone();
        if setup.role != Role::Member
            || setup.position.primary != source
            || ["localWipe", "removalSource", "setupMerge"]
                .iter()
                .any(|field| self.link.extra.contains_key(*field))
        {
            return Ok(());
        }
        let roster = self.roster()?;
        let Some(primary) = roster.boxes.into_iter().find(|peer| peer.key == source) else {
            return Ok(());
        };
        let Some(host) = Host::from_text(&primary.key, &primary.address) else {
            return Ok(());
        };
        let key = self.own()?.public.clone();
        let Some((kind, signed)) = message
            .get("offer")
            .map(|signed| ("offer", signed))
            .or_else(|| {
                message
                    .get("instruction")
                    .map(|signed| ("instruction", signed))
            })
        else {
            return Ok(());
        };
        let Some(doc) = serde_json::from_value(signed.clone())
            .ok()
            .and_then(|doc| documents::verify(doc).ok())
        else {
            return Ok(());
        };
        let position = &setup.position;
        let body = &doc.body;
        if doc.kind != Kind::SetupShared
            || encode(&doc.origin) != setup.sign
            || doc.n != position.epoch
            || body.get("removal").and_then(Value::as_str) != Some(kind)
            || body.get("setup").and_then(Value::as_str) != Some(&position.id)
            || body.get("primary").and_then(Value::as_str) != Some(&position.primary)
            || body.get("key").and_then(Value::as_str) != Some(&key)
        {
            return Ok(());
        }
        let Some(operation) = body
            .get("operation")
            .and_then(Value::as_str)
            .filter(|op| safe_token(op, 22, 43))
        else {
            return Ok(());
        };
        let now = Instant::now();
        let matches = |challenge: &Challenge| {
            challenge.position == *position
                && challenge.operation == operation
                && now < challenge.expires
        };
        if kind == "offer" {
            if !self.unlink_challenge.as_ref().is_some_and(matches) {
                self.unlink_challenge = Some(Challenge {
                    position: position.clone(),
                    operation: operation.into(),
                    nonce: encode(&random::<16>()?),
                    expires: now + Duration::from_secs(30),
                });
            }
            let nonce = &self.unlink_challenge.as_ref().ok_or(Error::Conflict)?.nonce;
            self.send_removal(
                &host,
                json!({"request": operation, "setup": position.id, "nonce": nonce}),
            )
            .await;
        } else if self.unlink_challenge.as_ref().is_some_and(|challenge| {
            matches(challenge)
                && body.get("nonce").and_then(Value::as_str) == Some(&challenge.nonce)
        }) && body.get("wipe") == Some(&Value::Bool(false))
            && !["plan", "generation", "requiresPreparation"]
                .iter()
                .any(|field| body.contains_key(*field))
        {
            let pending = LocalUnlink {
                setup: position.id.clone(),
                operation: operation.into(),
                primary,
                completed: false,
            };
            let mut next = self.link.clone();
            next.extra.insert("localUnlink".into(), json!(pending));
            next.extra.insert("phones".into(), json!([]));
            next.extra.shift_remove("pairing");
            next.extra.shift_remove("appPairing");
            self.commit(next).await?;
            self.unlink_challenge = None;
            self.resume_local_unlink().await?;
        }
        Ok(())
    }

    fn local_unlink(&self) -> Result<Option<LocalUnlink>, Error> {
        self.link
            .extra
            .get("localUnlink")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| Error::Invalid("local unlink journal"))
    }

    pub(super) async fn resume_local_unlink(&mut self) -> Result<(), Error> {
        if self.kind != ComputerKind::Machine {
            return Ok(());
        }
        let Some(mut pending) = self.local_unlink()? else {
            return Ok(());
        };
        let Some(host) = Host::from_text(&pending.primary.key, &pending.primary.address) else {
            return Err(Error::Invalid("unlink issuer"));
        };
        if !pending.completed {
            let files = self.files.clone();
            let before = self.link.clone();
            let position = before.setup()?.clone();
            if position.role != Role::Member
                || position.position.id != pending.setup
                || position.position.primary != pending.primary.key
            {
                return Err(Error::Conflict);
            }
            let kind = self.kind;
            pending.completed = true;
            let saved = pending.clone();
            let next = tokio::task::spawn_blocking(move || {
                cleanup::clear(&files).map_err(|source| Error::File {
                    action: "clearing credentials at",
                    path: files.path(""),
                    source,
                })?;
                let identity = crate::model::Identity::read(&before)?;
                let mut after = released_link(&before, &position, &identity.public);
                crate::installer::released::keep_baseline(&files, &mut after);
                super::bootstrap(&files, &mut after, &identity, kind)?;
                let new_setup = after.setup()?.position.id.clone();
                after.extra.insert("releasedSetup".into(), json!(new_setup));
                after.extra.insert("localUnlink".into(), json!(saved));
                Ok::<_, Error>(after)
            })
            .await
            .map_err(|_| Error::Invalid("unlink cleanup task"))??;
            self.commit(next).await?;
            self.lease.seen = None;
            self.exports.clear();
            self.manifests.clear();
            self.pending_tips.clear();
        }
        self.send_removal(
            &host,
            json!({"done": pending.operation, "setup": pending.setup}),
        )
        .await;
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

fn released_link(before: &Link, setup: &Setup, key: &str) -> Link {
    let mut link = Link {
        key: before.key.clone(),
        sign: before.sign.clone(),
        secret: before.secret.clone(),
        relay: before.relay.clone(),
        name: before.name.clone(),
        devices_disabled: before.devices_disabled,
        ..Link::default()
    };
    for field in [
        "startsFromOthers",
        "formerDeviceRevocations",
        "formerPhoneIssuers",
        "installerCancellations",
    ] {
        if let Some(value) = before.extra.get(field) {
            link.extra.insert(field.into(), value.clone());
        }
    }
    link.extra.insert("phones".into(), json!([]));
    link.extra.insert("departedKey".into(), json!(key));

    let issuers = link
        .extra
        .entry("formerPhoneIssuers")
        .or_insert_with(|| json!({}));
    let prior = &mut issuers[&setup.sign];
    if !prior.is_array() {
        *prior = json!([]);
    }
    if let Some(scopes) = prior.as_array_mut()
        && !scopes.iter().any(|item| item == &setup.position.id)
    {
        scopes.push(json!(setup.position.id));
    }
    if let Some(removed) = before
        .extra
        .get("removed")
        .and_then(Value::as_array)
        .filter(|rows| !rows.is_empty())
    {
        let history = link
            .extra
            .entry("formerDeviceRevocations")
            .or_insert_with(|| json!({}));
        let prior = &mut history[&setup.position.id];
        if !prior.is_array() {
            *prior = json!([]);
        }
        if let Some(revoked) = prior.as_array_mut() {
            for row in removed {
                if !revoked
                    .iter()
                    .any(|old| old["key"] == row["key"] && old["pairing"] == row["pairing"])
                {
                    revoked.push(row.clone());
                }
            }
        }
    }
    link
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
