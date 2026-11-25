//! Signed plain unlink, as the main computer's writer. A removed peer uses
//! removal mail for the final handshake after ordinary peer trust is revoked.

use crate::cleanup;

use std::time::Duration;

use tokio::time::Instant;

use kuvo_protocol::routes::types::{
    GetKuvoComputersStatus200WipePendingItem as PendingWipe,
    GetKuvoComputersStatus200WipePendingItemNextAction4 as NextWipeAction,
};
use kuvo_wire::{
    documents::{self, Kind},
    keys::{Address, Host, decode_exact, encode},
    mail::Envelope,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Owner, Response, with_receipt};
use crate::{
    Error, checkpoint,
    edits::Submitter,
    installer,
    model::{
        ComputerKind, Identity, Link, Membership, Peer, Position, Role, Setup, SharedKind, receipt,
        safe_token,
    },
    object, random, wipe,
};

const LAYOUT_REVIEW: &str = "This computer’s data layout cannot be wiped from here. Cancel the wipe to leave its data in place.";
const EXPORT_REQUIRED: &str = "Relay backups require setup access. Choose a separate disk or SSH export to finish wiping this computer.";

#[derive(Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum WipeReview {
    UnsupportedLayout,
    ChooseExport,
}

impl WipeReview {
    fn read(value: Option<&Value>) -> Option<Self> {
        serde_json::from_value(value?.clone()).ok()
    }

    fn record(self, extra: &mut Map<String, Value>) -> bool {
        match self {
            Self::UnsupportedLayout if !extra.contains_key("error") => {
                extra.insert("error".into(), json!(LAYOUT_REVIEW));
            }
            Self::ChooseExport if extra.get("needsExport") != Some(&Value::Bool(true)) => {
                extra.insert("needsExport".into(), json!(true));
                extra.insert("error".into(), json!(EXPORT_REQUIRED));
            }
            _ => return false,
        }
        true
    }

    fn for_source(home: &std::path::Path, prior: Option<Self>) -> Result<Option<Self>, Error> {
        if prior == Some(Self::ChooseExport) {
            return Ok(prior);
        }
        let supported =
            wipe::supports_member(home).map_err(|error| Error::Installer(error.to_string()))?;
        if !supported {
            return Ok(Some(Self::UnsupportedLayout));
        }
        let relay = crate::backup::uses_automatic_relay_backup(home)
            .map_err(|error| Error::Installer(error.to_string()))?;
        Ok(if relay {
            Some(Self::ChooseExport)
        } else {
            prior
        })
    }
}

#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Unlink {
    #[serde(
        default,
        deserialize_with = "kuvo_protocol::json::present",
        skip_serializing_if = "Option::is_none"
    )]
    operation: Option<String>,
    #[serde(
        default,
        deserialize_with = "kuvo_protocol::json::present",
        skip_serializing_if = "Option::is_none"
    )]
    wipe: Option<bool>,
    #[serde(
        default,
        deserialize_with = "kuvo_protocol::json::present",
        skip_serializing_if = "Option::is_none"
    )]
    resolve: Option<ResolveWipe>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ResolveWipe {
    Cancel,
    Lost,
}

impl ResolveWipe {
    fn state(self) -> &'static str {
        match self {
            Self::Cancel => "cancelled",
            Self::Lost => "lost",
        }
    }
}

impl Unlink {
    pub(crate) const LOCAL_APP_ONLY: &str = "Prepare the wipe from Kuvo Code.";

    pub(super) fn operation(&self) -> Option<&str> {
        self.operation.as_deref()
    }

    pub(crate) fn on_route(&mut self, wipe: Option<bool>) {
        if wipe.is_some() {
            self.wipe = wipe;
        }
    }

    pub(crate) fn valid(&self) -> bool {
        self.operation
            .as_deref()
            .is_none_or(|op| safe_token(op, 22, 43))
            && !self.extra.contains_key("save")
            && self
                .extra
                .get("retryPreparation")
                .is_none_or(Value::is_boolean)
            && (!self.prepares_wipe() || self.operation.is_some())
            && (self.resolve.is_none() || self.operation.is_some() && !self.prepares_wipe())
    }

    pub(crate) fn requires_local_app(&self) -> bool {
        self.resolve.is_some() || self.prepares_wipe()
    }

    fn prepares_wipe(&self) -> bool {
        self.extra.contains_key("prepare")
            || self.extra.contains_key("supersedes")
            || self.extra.get("retryPreparation") == Some(&Value::Bool(true))
    }

    pub(super) fn bind_operation(&mut self) -> Result<String, Error> {
        match &self.operation {
            Some(operation) => Ok(operation.clone()),
            None => {
                let operation = encode(&random::<16>()?);
                self.operation = Some(operation.clone());
                Ok(operation)
            }
        }
    }
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

    fn exchanges_mail(&self) -> bool {
        self.plain()
            || self.wipe
                && self.resolution() != Some("lost")
                && self.extra.get("local").is_none()
                && self.extra.get("generation").is_none()
                && self.extra.get("plan").is_none()
                && self.extra.get("requiresPreparation").is_none()
    }

    fn asks_erase(&self) -> bool {
        self.wipe && self.resolution().is_none()
    }

    fn resolution(&self) -> Option<&str> {
        self.extra
            .get("resolution")
            .and_then(Value::as_str)
            .filter(|resolution| matches!(*resolution, "cancelled" | "lost"))
    }

    fn plan(&self) -> Option<&Value> {
        self.extra.get("plan").filter(|plan| {
            plan["choiceHash"]
                .as_str()
                .is_some_and(|hash| safe_token(hash, 43, 43))
                && plan
                    .get("moveTo")
                    .is_none_or(|key| key.as_str().is_some_and(|key| safe_token(key, 43, 43)))
        })
    }

    fn wipe_row(&self) -> PendingWipe {
        const EXPORT_DISK: &str =
            "Choose an export on a separate disk or another computer over SSH.";

        let error = self
            .extra
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
            .map(|error| error.chars().take(500).collect::<String>());
        let has_plan = self.plan().is_some();
        let needs_export = self.extra.get("needsExport").and_then(Value::as_bool) == Some(true);
        let next_action = if !has_plan && needs_export {
            Some(NextWipeAction::ChooseExport)
        } else {
            match error.as_deref() {
                Some(LAYOUT_REVIEW) => Some(NextWipeAction::CancelWipe),
                Some(EXPORT_REQUIRED | EXPORT_DISK) if has_plan => {
                    Some(NextWipeAction::ReviseExport)
                }
                _ => None,
            }
        };
        let unresolved: Vec<_> = self
            .extra
            .get("unresolved")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(100)
            .map(str::to_owned)
            .collect();
        PendingWipe {
            key: self.peer.key.clone(),
            name: self.peer.name.clone(),
            operation: self.operation.clone(),
            error,
            next_action,
            unresolved: (!unresolved.is_empty()).then_some(unresolved),
        }
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
        mut request: Unlink,
        by: Submitter,
    ) -> Result<Response, Error> {
        if !request.valid() {
            return Ok(answer(400, "Send a valid removal request."));
        }
        if decode_exact::<32>(&key).is_none() {
            return Ok(answer(404, "That isn’t one of your computers."));
        }
        if !by.current(&self.link) {
            return Ok(answer(403, "The submitting device no longer has access."));
        }
        if request.requires_local_app() && !matches!(by, Submitter::Computer) {
            return Ok(answer(403, Unlink::LOCAL_APP_ONLY));
        }
        let setup = self.link.setup()?.clone();
        if setup.role != Role::Primary {
            return Ok(answer(409, "Only the main computer can remove a computer."));
        }
        if request.prepares_wipe() {
            return Ok(answer(409, "Wipe preparation is unavailable."));
        }

        let mut roster = self.roster()?;
        if key == setup.position.primary {
            if roster.boxes.len() == 1 {
                if request.resolve.is_some() {
                    let (status, message) = if removal_for(&roster, &key).is_some() {
                        (409, "The removal has already advanced past this change.")
                    } else {
                        (404, "That removal operation is unknown.")
                    };
                    return Ok(answer(status, message));
                }
                return self.wipe_last(setup, roster, request).await;
            }
            return Ok(answer(409, "Change the main computer before unlinking it."));
        }
        if let Some(record) = removal_for(&roster, &key) {
            if (!record.plain() && !record.wipe)
                || request.wipe.is_some_and(|wipe| wipe != record.wipe)
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
            if let Some(resolution) = request.resolve {
                return self.resolve_wipe(roster, record, resolution, saved).await;
            }
            if let Err(error) = self.clean_saved_ssh_address(&key).await {
                eprintln!("Removal machines-list cleanup failed: {error}");
            }
            return Ok(removal_reply(&record, saved));
        }
        if request.resolve.is_some() {
            return Ok(answer(404, "That removal operation is unknown."));
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
        let operation = request.bind_operation()?;
        if self.link.shared_outcomes.contains_key(&operation) {
            return Ok(answer(
                409,
                "This setup already has a decision for this removal.",
            ));
        }
        let mut next = self.link.clone();
        if let Some(destination) = crate::ssh::saved_machines(&self.files)
            .and_then(|saved| {
                crate::ssh::saved_ssh_destination(
                    &self.files,
                    &saved,
                    &self.link,
                    &roster.boxes[at],
                )
            })
            .ok()
            .flatten()
        {
            next.pending_machine_cleanup
                .insert(key.clone(), destination);
        }
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
            wipe: request.wipe == Some(true),
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
        let mut reply = removal_reply(&record, Some(proof));
        if let Err(error) = self.clean_saved_ssh_address(&key).await {
            eprintln!("Removal machines-list cleanup failed: {error}");
            reply.status = 503;
            reply.value["message"] = json!(
                "The computer’s access was removed, but the saved machines list couldn’t be updated."
            );
        }
        Ok(reply)
    }

    async fn clean_saved_ssh_address(&mut self, key: &str) -> Result<(), Error> {
        let Some(destination) = self.link.pending_machine_cleanup.get(key) else {
            return Ok(());
        };
        crate::ssh::forget_ssh_destination(&self.files, &self.link, destination)?;
        let mut next = self.link.clone();
        next.pending_machine_cleanup.remove(key);
        self.commit(next).await
    }

    async fn resolve_wipe(
        &mut self,
        mut roster: Membership,
        mut record: Removal,
        resolution: ResolveWipe,
        saved: Option<crate::Receipt>,
    ) -> Result<Response, Error> {
        if record.resolution() == Some(resolution.state()) {
            return Ok(removal_reply(&record, saved));
        }
        let held = record.plan().is_some()
            || record.extra.get("needsExport") == Some(&Value::Bool(true))
            || record.extra.get("error").and_then(Value::as_str) == Some(LAYOUT_REVIEW);
        let can_cancel = held
            && record.extra.get("generation").is_none()
            && record.extra.get("withdrawnGeneration").is_none();
        if !record.asks_erase()
            || record.delivered
            || (resolution == ResolveWipe::Cancel && !can_cancel)
        {
            return Ok(answer(
                409,
                "The removal has already advanced past this change.",
            ));
        }
        self.can_write()?;
        record
            .extra
            .insert("resolution".into(), json!(resolution.state()));
        for detail in ["error", "needsExport", "unresolved"] {
            record.extra.shift_remove(detail);
        }
        insert_removal(&mut roster, &record)?;
        let mut next = self.link.clone();
        next.put(
            self.own()?,
            SharedKind::MembershipAndAccess,
            serde_json::to_value(roster).expect("membership serializes"),
            None,
            None,
        )?;
        self.commit(next).await?;
        if let Err(error) = self.publish(false).await {
            eprintln!("publishing resolved removal: {error}");
        }
        if resolution == ResolveWipe::Cancel
            && let Err(error) = self.offer_unlinks().await
        {
            eprintln!("publishing cancellation offer: {error}");
        }
        Ok(removal_reply(&record, saved))
    }

    async fn wipe_last(
        &mut self,
        setup: Setup,
        mut roster: Membership,
        mut request: Unlink,
    ) -> Result<Response, Error> {
        if let Some(kept) = wipe::LocalWipe::from(&self.link) {
            return if request.operation().is_some_and(|op| op != kept.operation) {
                Ok(answer(
                    409,
                    "A wipe is already scheduled for this computer.",
                ))
            } else {
                let saved = receipt(
                    self.own()?,
                    &setup.position,
                    kept.operation.clone(),
                    &self.link.shared_outcomes[&kept.operation],
                )?;
                Ok(local_wipe_reply(&kept.operation, saved))
            };
        }
        self.can_write()?;
        let home = self.files.root().to_path_buf();
        if let Err(error) = tokio::task::spawn_blocking(move || wipe::check(&home))
            .await
            .map_err(|_| Error::Invalid("checking local wipe layout"))?
        {
            return Ok(answer(409, &error.to_string()));
        }
        let operation = request.bind_operation()?;
        if self.link.shared_outcomes.contains_key(&operation) {
            return Ok(answer(
                409,
                "This setup already has a decision for this removal.",
            ));
        }
        let record = Removal {
            peer: roster.boxes[0].clone(),
            operation: operation.clone(),
            wipe: true,
            delivered: false,
            extra: object(json!({"local": true})),
        };
        insert_removal(&mut roster, &record)?;
        if let Some(devices) = roster
            .extra
            .get_mut("devices")
            .and_then(Value::as_object_mut)
        {
            devices.insert("phones".into(), json!([]));
        }
        let mut next = self.link.clone();
        let proof = next.put(
            self.own()?,
            SharedKind::MembershipAndAccess,
            json!(roster),
            None,
            Some(operation.clone()),
        )?;
        next.extra.insert(
            "localWipe".into(),
            json!(wipe::LocalWipe {
                setup: setup.position.id,
                operation: operation.clone()
            }),
        );
        for path in ["pairing", "appPairing"] {
            next.extra.shift_remove(path);
        }
        next.extra.insert("phones".into(), json!([]));
        self.commit(next).await?;
        Ok(local_wipe_reply(&operation, proof))
    }

    /// Removals still offered to their computer, as `offer_unlinks` sends them (a wipe too).
    pub(super) fn pending_unlinks(&self) -> bool {
        self.roster().is_ok_and(|roster| {
            roster
                .extra
                .get("removals")
                .and_then(Value::as_object)
                .is_some_and(|rows| {
                    rows.iter().any(|(key, value)| {
                        read_removal(key, value)
                            .is_some_and(|record| record.exchanges_mail() && !record.delivered)
                    })
                })
        })
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
            let Some(record) = read_removal(key, value)
                .filter(|record| record.exchanges_mail() && !record.delivered)
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
        from: Address,
        envelope: &Envelope,
    ) -> Result<(), Error> {
        if self.link.extra.contains_key("localUnlink")
            || self
                .link
                .setup()
                .is_ok_and(|setup| setup.role == Role::Member)
        {
            return self.take_member_unlink(sender, from, envelope).await;
        }
        if self.can_write().is_err() {
            return Ok(());
        }
        let setup = self.link.setup()?.position.clone();
        let key = encode(&sender);
        let mut roster = self.roster()?;
        let Some(mut record) = removal_for(&roster, &key).filter(Removal::exchanges_mail) else {
            return Ok(());
        };
        let Some(peer) = record.host() else {
            return Ok(());
        };
        let message = &envelope.0;
        if message.get("setup").and_then(Value::as_str) != Some(&setup.id) {
            return Ok(());
        }
        if !record.delivered
            && let Some(epoch) = message.get("sync").and_then(Value::as_u64)
            && (1..setup.epoch).contains(&epoch)
            && let Some(signed) = self.link.leadership.get(&(epoch + 1))
        {
            for doc in checkpoint::packet(&self.files, signed)? {
                if doc.d.len() + doc.s.len() + 16 > 48_000 {
                    for part in documents::split(&doc)? {
                        self.send_removal(&peer, json!({"authorityPart": part}))
                            .await;
                    }
                } else {
                    self.send_removal(&peer, json!({"authority": doc})).await;
                }
            }
        } else if record.asks_erase()
            && !record.delivered
            && let Some(review) = WipeReview::read(message.get("review"))
            && message.get("operation").and_then(Value::as_str) == Some(&record.operation)
            && message.get("epoch").and_then(Value::as_u64) == Some(setup.epoch)
        {
            if review.record(&mut record.extra) {
                insert_removal(&mut roster, &record)?;
                let mut next = self.link.clone();
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
        } else if message.get("done").and_then(Value::as_str) == Some(&record.operation)
            && !message.contains_key("generation")
        {
            let erase = record.asks_erase();
            if !record.delivered {
                record.delivered = true;
                record.extra.shift_remove("error");
                record.extra.shift_remove("unresolved");
                insert_removal(&mut roster, &record)?;
                let mut next = self.link.clone();
                if !erase {
                    let acks = next.extra.entry("removalAcks").or_insert_with(|| json!({}));
                    acks.as_object_mut()
                        .ok_or(Error::Invalid("removal acknowledgments"))?
                        .insert(key.clone(), json!(record.operation));
                }
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
            let reply = message
                .get("receipt")
                .and_then(Value::as_str)
                .and_then(decode_exact)
                .map_or(peer, |key| Host { key, address: from });
            self.send_removal(
                &reply,
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
                    "key": key, "operation": record.operation, "wipe": record.asks_erase(), "nonce": nonce,
                })),
            )?;
            self.send_removal(&peer, json!({"instruction": doc})).await;
        }
        Ok(())
    }

    async fn take_member_unlink(
        &mut self,
        sender: [u8; 32],
        from: Address,
        envelope: &Envelope,
    ) -> Result<(), Error> {
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
                self.next = Some(Instant::now());
            }
            return Ok(());
        }
        let setup = self.link.setup()?.clone();
        if setup.role != Role::Member
            || ["localWipe", "setupMerge"]
                .iter()
                .any(|field| self.link.extra.contains_key(*field))
        {
            return Ok(());
        }
        if self
            .lease
            .seen
            .as_ref()
            .is_some_and(|lease| lease.holder == source)
        {
            let authority = if let Some(part) = message.get("authorityPart") {
                serde_json::from_value(part.clone())
                    .ok()
                    .and_then(|part| self.parts.receive(sender, part).ok().flatten())
            } else {
                message
                    .get("authority")
                    .and_then(|doc| serde_json::from_value(doc.clone()).ok())
                    .and_then(|doc| documents::verify(doc).ok())
            };
            if let Some(doc) =
                authority.filter(|doc| matches!(doc.kind, Kind::SetupCheckpoint | Kind::Leadership))
            {
                self.take_doc(&source, doc).await?;
                return Ok(());
            }
        }
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
            || doc.n < position.epoch
            || body.get("removal").and_then(Value::as_str) != Some(kind)
            || body.get("setup").and_then(Value::as_str) != Some(&position.id)
            || body.get("primary").and_then(Value::as_str) != Some(&source)
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
        if kind == "offer" && doc.n > position.epoch {
            self.next = Some(Instant::now());
            let host = self.host(&source).unwrap_or(Host {
                key: sender,
                address: from,
            });
            self.send_removal(&host, json!({"sync": position.epoch, "setup": position.id}))
                .await;
            return Ok(());
        }
        if source != position.primary
            || doc.n != position.epoch
            || encode(&doc.origin) != setup.sign
        {
            return Ok(());
        }
        let Some(primary) = self
            .roster()?
            .boxes
            .into_iter()
            .find(|peer| peer.key == source)
        else {
            return Ok(());
        };
        let Some(host) = Host::from_text(&primary.key, &primary.address) else {
            return Ok(());
        };
        let prior = match self.link.extra.get("removalSource") {
            Some(held) if held["setup"] == position.id && held["operation"] == operation => {
                let Some(review) = WipeReview::read(held.get("review")) else {
                    return Ok(());
                };
                Some(review)
            }
            Some(_) => return Ok(()),
            None => None,
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
        }) && body.get("wipe").and_then(Value::as_bool).is_some()
            && !["plan", "generation", "requiresPreparation"]
                .iter()
                .any(|field| body.contains_key(*field))
        {
            if body["wipe"] == true {
                let review = WipeReview::for_source(self.files.root(), prior)?;
                if prior.is_none() || prior != review {
                    let mut next = self.link.clone();
                    if let Some(review) = review {
                        next.extra.insert(
                            "removalSource".into(),
                            json!({
                                "setup": position.id, "operation": operation, "review": review,
                            }),
                        );
                    } else {
                        let approved = wipe::member::MemberWipe {
                            setup: position.id.clone(),
                            operation: operation.into(),
                            primary,
                            source: key,
                        };
                        next.extra.insert("localWipe".into(), json!(approved));
                    }
                    next.extra.insert("phones".into(), json!([]));
                    next.extra.shift_remove("pairing");
                    next.extra.shift_remove("appPairing");
                    self.commit(next).await?;
                }
                self.unlink_challenge = None;
                if let Some(review) = review {
                    self.send_removal(&host, json!({
                        "setup": position.id, "operation": operation, "epoch": position.epoch, "review": review,
                    })).await;
                } else {
                    self.next = Some(Instant::now());
                }
                return Ok(());
            }
            if self.kind != ComputerKind::Machine {
                return Ok(());
            }
            let pending = LocalUnlink {
                setup: position.id.clone(),
                operation: operation.into(),
                primary,
                completed: false,
            };
            let mut next = self.link.clone();
            next.extra.shift_remove("removalSource");
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

pub(super) fn retire_computer(
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

pub(super) fn retired_member(roster: &Membership, identity: &Identity) -> bool {
    roster.left.contains(&identity.public)
        && removal_for(roster, &identity.public).is_some_and(|record| {
            record.exchanges_mail() && record.peer.sign.as_deref() == Some(&identity.sign)
        })
}

pub(crate) enum WipeProgress {
    Pending,
    Delivered,
    Closed,
    Review(String),
}

pub(crate) fn wipe_progress(link: &Link, key: &str, operation: &str) -> WipeProgress {
    let Ok(roster) = link.member_list() else {
        return WipeProgress::Pending;
    };
    let Some(record) =
        removal_for(&roster, key).filter(|record| record.wipe && record.operation == operation)
    else {
        return WipeProgress::Pending;
    };
    if record.delivered && record.resolution().is_none() {
        WipeProgress::Delivered
    } else if let Some(error) = record.wipe_row().error {
        WipeProgress::Review(error)
    } else if record.resolution().is_some() {
        WipeProgress::Closed
    } else {
        WipeProgress::Pending
    }
}

pub(super) fn pending_wipes(link: &Link) -> Vec<PendingWipe> {
    let Ok(roster) = link.member_list() else {
        return Vec::new();
    };
    roster
        .extra
        .get("removals")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            let record = read_removal(key, value)?;
            let current = roster.boxes.iter().any(|peer| peer.key == *key);
            let local = record.extra.get("local").and_then(Value::as_bool) == Some(true)
                && current
                && roster.boxes.len() == 1;
            let removed = !current && roster.left.contains(key);
            if !record.wipe
                || record.delivered
                || (!local && !removed)
                || record.resolution().is_some()
            {
                return None;
            }
            record.host()?;
            Some(record.wipe_row())
        })
        .collect()
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

fn removal_reply(record: &Removal, save: Option<crate::Receipt>) -> Response {
    if !record.wipe {
        return unlinked(&record.operation, save);
    }
    let row = record.wipe_row();
    let plan = record.plan();
    let prepared = record
        .extra
        .get("generation")
        .and_then(Value::as_str)
        .is_some_and(|generation| safe_token(generation, 43, 43));
    let state = match record.resolution() {
        Some(resolution) => resolution,
        _ if record.delivered => "wiped",
        _ if row.error.is_some() => "review",
        _ if !prepared && plan.is_some_and(|plan| plan.get("moveTo").is_some()) => "moving",
        _ if !prepared && plan.is_some_and(|plan| plan["archive"] == true) => "exporting",
        _ => "wipe-pending",
    };
    let mut value = json!({"removal": {
        "operation": record.operation, "state": state, "accessRemoved": true,
    }});
    if let Some(error) = row.error {
        value["message"] = json!(error);
    }
    if state == "review"
        && let Some(action) = row.next_action
    {
        value["removal"]["nextAction"] = json!(action);
    }
    if let Some(unresolved) = row.unresolved {
        value["unresolved"] = json!(unresolved);
    }
    if let Some(point) = record.extra.get("export")
        && point["source"].is_string()
        && point["id"].is_string()
        && point["at"].is_number()
    {
        value["export"] = json!({"source": point["source"], "id": point["id"], "at": point["at"]});
    }
    if let Some(save) = save {
        value = with_receipt(value, save);
    }
    Response::ok(value)
}

fn local_wipe_reply(operation: &str, save: crate::Receipt) -> Response {
    let mut value = json!({"removal": {"operation": operation, "state": "wipe-pending", "accessRemoved": true}});
    if std::env::var_os("KUVO_BOX").is_none_or(|kind| kind != "mac") {
        value["offer"] = json!("reinstall-local");
    }
    Response::ok(with_receipt(value, save))
}

fn answer(status: u16, message: &str) -> Response {
    Response {
        status,
        value: json!({"message": message}),
    }
}
