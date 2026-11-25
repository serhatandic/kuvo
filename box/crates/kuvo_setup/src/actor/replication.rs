use std::time::Duration;

use kuvo_peers::relay;
use kuvo_wire::{
    documents::{self, Document, Kind},
    keys::{Host, encode},
    mail,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{Owner, installer_pending, new_setup};
use crate::{
    Error, checkpoint, document,
    lease::{self, Action},
    model::{
        Outcomes, Position, Role, Shared, SharedKind, membership, safe_token, validate_shared,
    },
    object,
};

fn needs_invite_lease(invite: &crate::installer::Invitation, now: u64, left: &[String]) -> bool {
    invite.live(now)
        && !invite
            .claim
            .as_ref()
            .is_some_and(|claim| left.contains(&claim.key))
        && !["cancelled", "activated", "credentialsCompleted"]
            .iter()
            .any(|flag| invite.extra.get(*flag) == Some(&Value::Bool(true)))
}

impl Owner {
    pub(super) fn relay_roster_dirty(&self) -> bool {
        self.roster()
            .is_ok_and(|roster| roster.boxes.len() == 1 && !roster.left.is_empty())
            && !self.lease.roster_current(&self.link)
    }

    pub(super) fn lease_needed(&self) -> bool {
        let (Ok(setup), Ok(own), Ok(roster)) = (self.link.setup(), self.own(), self.roster())
        else {
            return false;
        };
        self.link.relay.is_some()
            && !installer_pending(&self.link)
            && !self.link.disabled(&roster, &own.public)
            && !self.provisional(&own.public)
            && (setup.role == Role::Member
                || roster.boxes.len() > 1
                || self.relay_roster_dirty()
                || self
                    .link
                    .installer_invites
                    .values()
                    .any(|invite| needs_invite_lease(invite, lease::wall(), &roster.left)))
    }

    pub(super) async fn tick(&mut self) -> Result<(), Error> {
        if !self.lease_needed() {
            self.next = None;
            return Ok(());
        }
        self.observe_lease().await
    }

    pub(super) async fn tick_for_add(&mut self) -> Result<(), Error> {
        self.observe_lease().await
    }

    pub(super) async fn announce_lease(&self) {
        let Some(seen) = &self.lease.seen else { return };
        let Ok(own) = self.own() else { return };
        let Ok(roster) = self.roster() else { return };
        for peer in roster.boxes.iter().filter(|peer| peer.key != own.public) {
            // This message is only a hint to read the authoritative relay, not a lease grant.
            self.send_authority(
                &peer.key,
                json!({"kind":"lease-changed", "term":seen.term, "expires":seen.expires}),
            )
            .await;
        }
    }

    async fn observe_lease(&mut self) -> Result<(), Error> {
        let Ok(setup) = self.link.setup().cloned() else {
            return Ok(());
        };
        let Some(own) = self.identity.as_ref() else {
            return Ok(());
        };
        if self.link.relay.is_none()
            || installer_pending(&self.link)
            || self.link.disabled(&self.roster()?, &own.public)
            || self.provisional(&own.public)
        {
            return Ok(());
        }
        let prior = self
            .lease
            .seen
            .as_ref()
            .map(|seen| (seen.term, seen.expires));
        let seen = self.lease.seen.as_ref();
        let renew =
            setup.role == Role::Primary && seen.is_some_and(|lease| lease.holder == own.public);
        let (action, term) = if renew {
            (Action::Renew, seen.unwrap().term)
        } else {
            (Action::Read, 0)
        };
        let mut observation = self
            .lease
            .request(&self.files, &self.link, own, action, term, None)
            .await?;
        if setup.role == Role::Primary {
            match observation.as_ref() {
                None if self.lease.seen.is_none() => {
                    observation = self
                        .lease
                        .request(&self.files, &self.link, own, Action::Claim, 0, None)
                        .await?
                }
                Some((l, _)) if l.holder == own.public && !renew && !lease::Client::expired(l) => {
                    observation = self
                        .lease
                        .request(&self.files, &self.link, own, Action::Renew, l.term, None)
                        .await?;
                }
                _ => {}
            }
        }
        if let Some((seen, takeover_in)) = observation.as_ref()
            && takeover_in == &0
            && lease::Client::expired(seen)
            && (setup.role == Role::Member || seen.holder == own.public)
            && self.link.value(SharedKind::SetupDeparture).is_none()
            && !self.link.extra.contains_key("setupMerge")
        {
            observation = self
                .lease
                .request(&self.files, &self.link, own, Action::Claim, seen.term, None)
                .await?;
        }
        if setup.role == Role::Member
            && self.lease.can_promote(own)
            && self.link.value(SharedKind::SetupDeparture).is_none()
            && !self.link.extra.contains_key("setupMerge")
            && !self.provisional(&own.public)
        {
            self.promote().await?;
        }

        self.try_pending_tips().await;
        let setup = self.link.setup()?;
        let in_charge = setup.role == Role::Primary && self.lease.holds(&self.link);
        let changed = self
            .lease
            .seen
            .as_ref()
            .map(|seen| (seen.term, seen.expires))
            != prior;
        let takeover_in = observation.as_ref().map_or(2000, |(_, wait)| *wait);
        self.next = if self.lease_needed() {
            let wait = if self.relay_roster_dirty() {
                Duration::from_secs(2)
            } else {
                self.lease.next_observation(in_charge, takeover_in)
            };
            let left = self.roster()?.left;
            let invite_end = self
                .link
                .installer_invites
                .values()
                .filter(|invite| needs_invite_lease(invite, lease::wall(), &left))
                .map(|invite| {
                    invite
                        .claim
                        .as_ref()
                        .map_or(invite.expires, |claim| claim.until)
                })
                .min();
            Some(
                Instant::now()
                    + invite_end.map_or(wait, |at| {
                        wait.min(Duration::from_millis(
                            at.saturating_sub(lease::wall()).max(1),
                        ))
                    }),
            )
        } else {
            None
        };
        if changed && in_charge {
            self.announce_lease().await;
        }

        if setup.role == Role::Primary && self.lease.holds(&self.link) {
            self.publish(false).await?;
        } else {
            let primary = setup.position.primary.clone();
            let wants_tip = !self.link.leadership.contains_key(&setup.position.epoch);
            if self.next_digest <= Instant::now() {
                self.next_digest = Instant::now() + Duration::from_secs(10);
                self.publish(true).await?;
            }
            self.send_authority(
                &primary,
                json!({
                    "kind": "available", "wantsTip": wants_tip,
                }),
            )
            .await;
            if let Some(lease) = self
                .lease
                .seen
                .as_ref()
                .filter(|lease| lease.holder != primary)
            {
                self.send_authority(
                    &lease.holder,
                    json!({"kind": "relay-leadership", "term": lease.term}),
                )
                .await;
            }
        }
        Ok(())
    }

    pub(super) fn provisional(&self, key: &str) -> bool {
        [
            SharedKind::OwnershipAssignments,
            SharedKind::OwnershipRecovery,
        ]
        .iter()
        .any(|kind| {
            self.link
                .value(*kind)
                .and_then(|value| value.get("installer recovery"))
                .and_then(|rows| rows.get(key))
                .is_some()
        }) || self.roster().is_ok_and(|members| {
            members
                .extra
                .get("installerRecoveries")
                .and_then(|rows| rows.get(key))
                .is_some()
        })
    }

    async fn promote(&mut self) -> Result<(), Error> {
        let old = &self.link.setup()?.position;
        let own = self.own()?;
        let term = self.lease.seen.as_ref().ok_or(Error::Conflict)?.term;
        let position = Position {
            id: old.id.clone(),
            epoch: old.epoch + 1,
            primary: own.public.clone(),
        };
        let doc = checkpoint::build(
            &self.files,
            own,
            position,
            Some(old.primary.clone()),
            term,
            &self.link,
        )?;
        self.take_tip(doc).await?;
        self.publish(true).await
    }

    fn authorized_tip(
        &self,
        doc: &Document,
        cert: &checkpoint::Certificate,
    ) -> Result<bool, Error> {
        let setup = self.link.setup()?;
        if cert.setup.id != setup.position.id {
            return Ok(false);
        }
        if cert.setup == setup.position && !self.link.leadership.contains_key(&cert.setup.epoch) {
            return Ok(setup.sign == encode(&doc.origin)
                && setup.extra.get("leadershipHash").and_then(Value::as_str)
                    == Some(doc.digest().as_str()));
        }
        Ok(cert.setup.epoch > setup.position.epoch
            && self.lease.seen.as_ref().is_some_and(|lease| {
                lease.term == cert.relay_term
                    && lease.holder == cert.setup.primary
                    && lease.sign == encode(&doc.origin)
            }))
    }

    async fn take_tip(&mut self, doc: Document) -> Result<(), Error> {
        let cert = checkpoint::certificate(&doc)?;
        if !self.authorized_tip(&doc, &cert)? {
            return Err(Error::Conflict);
        }
        if let Some((_, manifest)) = self
            .manifests
            .iter()
            .find(|(hash, _)| hash == &cert.checkpoint)
        {
            if manifest.origin != doc.origin
                || manifest.n != doc.n
                || manifest.body.get("id")
                    != doc.body.get("setup").and_then(|setup| setup.get("id"))
            {
                return Err(Error::Invalid("checkpoint position"));
            }
            checkpoint::preserve(&self.files, manifest)?;
        }
        let (shared, outcomes) = checkpoint::resolve(&self.files, &doc, &cert)?;
        let own = self.own()?;
        let sign = encode(&doc.origin);
        let mut next = self.link.clone();
        if next.setup()?.position != cert.setup {
            let roster = membership(&shared, &cert.setup, &sign)?;
            let current = roster
                .boxes
                .iter()
                .any(|peer| peer.key == own.public && peer.sign.as_deref() == Some(&own.sign));
            if !current && !super::removal::retired_member(&roster, own) {
                return Err(Error::Conflict);
            }
            let role = if cert.setup.primary == own.public {
                Role::Primary
            } else {
                Role::Member
            };
            next.setup = Some(new_setup(cert.setup.clone(), sign, role));
            next.shared = shared;
            next.shared_outcomes.extend(outcomes);
            if role == Role::Primary
                && let Some(claim) = next
                    .extra
                    .get_mut("installerClaim")
                    .and_then(Value::as_object_mut)
                && claim.get("setup").and_then(Value::as_str) == Some(&cert.setup.id)
            {
                claim.insert("adopted".into(), json!(true));
            }
            for field in ["leadershipPending", "peerNames", "pairing", "appPairing"] {
                next.extra.shift_remove(field);
            }
        }
        next.leadership.insert(doc.n, doc.envelope);
        self.commit(next).await
    }

    async fn try_pending_tips(&mut self) {
        let mut candidates = std::mem::take(&mut self.pending_tips);
        while let Some(doc) = candidates.pop_front() {
            let current = self.link.setup().map_or(0, |setup| setup.position.epoch);
            if doc.n >= current && self.take_tip(doc.clone()).await.is_err() {
                self.pending_tips.push_back(doc);
            }
        }
    }

    pub(super) async fn receive(&mut self, event: &relay::RelayEvent) -> Result<(), Error> {
        match event {
            relay::RelayEvent::Installer { from, key, payload } => {
                self.receive_installer(*from, *key, payload).await?;
            }
            relay::RelayEvent::Removal {
                from,
                hinted_key,
                envelope,
            } => {
                self.receive_unlink(*hinted_key, *from, envelope).await?;
            }
            relay::RelayEvent::Connected(_) => {
                self.next = Some(Instant::now());
                self.next_edits = Some(Instant::now());
                self.publish(true).await?;
            }
            relay::RelayEvent::Mail { key, envelope, .. }
                if [
                    "docs",
                    "setupAuthority",
                    "scopedOfflineEdit",
                    "offlineEditReceipt",
                    "phoneSync",
                    "act",
                    "acted",
                    "scopedAct",
                ]
                .iter()
                .any(|field| envelope.0.contains_key(*field)) =>
            {
                let from = encode(key);
                if !self.roster()?.boxes.iter().any(|peer| peer.key == from) {
                    return Ok(());
                }
                if let Some(docs) = envelope.0.get("docs") {
                    self.receive_docs(*key, &from, docs).await?;
                }
                if let Some(given) = envelope.0.get("phoneSync") {
                    self.phone_sync(&from, given).await?;
                }
                if let Some(given) = envelope.0.get("setupAuthority") {
                    self.receive_authority(&from, given).await?;
                }
                if let Some(edit) = envelope
                    .0
                    .get("scopedOfflineEdit")
                    .filter(|_| !envelope.0.contains_key("offlineEdit"))
                {
                    self.receive_edit(&from, edit).await?;
                }
                if let Some(receipt) = envelope.0.get("offlineEditReceipt") {
                    self.receive_edit_receipt(&from, receipt)?;
                }
                if let Some(act) = envelope
                    .0
                    .get("act")
                    .filter(|act| act["area"] == "addComputer")
                {
                    self.receive_add_request(&from, act).await?;
                }
                let act = envelope.0.get("act");
                let scoped = envelope.0.get("scopedAct");
                if scoped.is_some() || act.is_some_and(|act| act["area"] == "computers") {
                    self.receive_forwarded_unlink(&from, act, scoped).await?;
                }
                if let Some(answer) = envelope.0.get("acted") {
                    self.receive_computer_answer(&from, answer);
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn phone_sync(&mut self, from: &str, given: &Value) -> Result<(), Error> {
        let Some(id) = given
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| safe_token(id, 22, 43))
        else {
            return Ok(());
        };
        let setup = self.link.setup()?;
        if given.get("ack").and_then(Value::as_bool) == Some(true)
            || given.get("origin").and_then(Value::as_str) != Some(&setup.sign)
        {
            return Ok(());
        }
        let Some(peer) = self.host(from) else {
            return Ok(());
        };
        if setup.role == Role::Primary {
            self.refresh_exports().await?;
        }
        let find = |kind| {
            self.exports
                .iter()
                .find(|(entry, _)| *entry == kind)
                .and_then(|(_, envelope)| documents::verify(envelope.clone()).ok())
                .or_else(|| document::cached(&self.link, kind))
        };
        let (Some(phones), Some(peers)) = (find(Kind::Phones), find(Kind::Peers)) else {
            return Ok(());
        };
        let list = format!("{}:{}", phones.n, phones.digest());
        let roster = format!("{}:{}", peers.n, peers.digest());
        self.send_doc(&peer, peers.envelope).await;
        self.send_doc(&peer, phones.envelope).await;
        self.send(
            &peer,
            json!({"phoneSync":{"id":id,"list":list,"peers":roster,"ack":true},"at":lease::wall()}),
        )
        .await;
        Ok(())
    }

    async fn receive_docs(
        &mut self,
        key: [u8; 32],
        from: &str,
        value: &Value,
    ) -> Result<(), Error> {
        #[derive(Deserialize)]
        struct Given {
            #[serde(default)]
            give: Vec<documents::Envelope>,
            part: Option<documents::Part>,
            #[serde(default)]
            have: Vec<document::DigestEntry>,
            #[serde(default)]
            reply: bool,
        }
        let given: Given = serde_json::from_value(value.clone())
            .map_err(|_| Error::Invalid("setup documents mail"))?;
        if let Some(part) = given.part
            && let Some(doc) = self.parts.receive(key, part)?
        {
            self.take_doc(from, doc).await?;
        }
        for envelope in given.give.into_iter().take(64) {
            if let Ok(doc) = documents::verify(envelope)
                && let Err(error) = self.take_doc(from, doc).await
                && !matches!(
                    error,
                    Error::Conflict | Error::Version(_) | Error::Invalid(_)
                )
            {
                eprintln!("receiving setup document: {error}");
            }
        }
        if !given.have.is_empty() {
            self.receive_digest(from, &given.have[..given.have.len().min(512)], given.reply)
                .await?;
        }
        Ok(())
    }

    async fn refresh_exports(&mut self) -> Result<(), Error> {
        if self.can_write().is_err() {
            self.exports.clear();
            return Ok(());
        }
        let mut next = self.link.clone();
        let (exports, changed) = document::published(&mut next, self.own()?)?;
        if changed {
            self.commit(next).await?;
        }
        self.exports = exports;
        Ok(())
    }

    async fn receive_digest(
        &mut self,
        from: &str,
        given: &[document::DigestEntry],
        reply: bool,
    ) -> Result<(), Error> {
        let Some(peer) = self.host(from) else {
            return Ok(());
        };
        if self.can_write().is_ok() {
            let mut next = self.link.clone();
            if document::recover_count(&mut next, self.own()?, given)? {
                self.commit(next).await?;
                self.refresh_exports().await?;
            }
        }
        let setup = self.link.setup()?;
        let remote = given.iter().find(|entry| entry.1 == Kind::Leadership);
        let behind = remote.is_none_or(|entry| entry.2 < setup.position.epoch)
            || (!self.link.leadership.contains_key(&setup.position.epoch)
                && remote.is_some_and(|entry| entry.2 == setup.position.epoch));
        if behind && let Some(envelope) = self.link.leadership.get(&setup.position.epoch) {
            for doc in checkpoint::packet(&self.files, envelope)? {
                self.send_doc(&peer, doc).await;
            }
        } else if setup.role == Role::Primary {
            self.publish_to(peer.clone(), false).await?;
        }
        if setup.role == Role::Member {
            for doc in document::stored(&self.link)
                .chain(document::cached(&self.link, Kind::SetupShared))
                .filter(|doc| document::missing(given, doc))
            {
                self.send_doc(&peer, doc.envelope).await;
            }
        }
        let has_later_tip = remote.is_some_and(|entry| entry.2 > setup.position.epoch);
        let ours = document::header(&self.link, &self.exports);
        let has_newer_export = given.iter().any(|entry| {
            entry.0 == setup.sign
                && matches!(entry.1, Kind::Peers | Kind::Phones)
                && ours
                    .iter()
                    .find(|known| known.0 == entry.0 && known.1 == entry.1)
                    .is_none_or(|known| {
                        known.2 < entry.2 || (known.2 == entry.2 && known.3 > entry.3)
                    })
        });
        if !reply && (behind || has_later_tip || has_newer_export) {
            self.send(
                &peer,
                json!({
                    "docs": {"have": ours, "reply": true}, "at": lease::wall(),
                }),
            )
            .await;
        }
        Ok(())
    }

    pub(super) async fn take_doc(&mut self, from: &str, doc: Document) -> Result<(), Error> {
        match doc.kind {
            Kind::SetupShared => self.take_replica(doc).await,
            Kind::SetupCheckpoint => {
                let setup = self.link.setup()?;
                if doc.body.get("id").and_then(Value::as_str) != Some(&setup.position.id) {
                    return Ok(());
                }
                let hash = checkpoint::reference(&doc);
                if doc.body.get("archive").and_then(Value::as_bool) == Some(true) {
                    if let Some(epoch) = self.manifests.iter().find_map(|(_, manifest)| {
                        let manifest_info = checkpoint::manifest(manifest).ok()?;
                        manifest_info.archives.contains(&hash).then_some(manifest.n)
                    }) {
                        checkpoint::outcomes(&doc, &setup.position.id, epoch)?;
                        checkpoint::preserve(&self.files, &doc)?;
                    }
                } else if self.lease.seen.as_ref().is_some_and(|lease| {
                    lease.sign == encode(&doc.origin) && doc.n > setup.position.epoch
                }) || (doc.n == setup.position.epoch
                    && setup.extra.contains_key("leadershipHash")
                    && encode(&doc.origin) == setup.sign)
                {
                    checkpoint::manifest(&doc)?;
                    if !self.manifests.iter().any(|(ref_hash, _)| ref_hash == &hash) {
                        if self.manifests.len() == 8 {
                            self.manifests.pop_front();
                        }
                        self.manifests.push_back((hash, doc));
                    }
                }
                self.try_pending_tips().await;
                Ok(())
            }
            Kind::Leadership => {
                let cert = checkpoint::certificate(&doc)?;
                if cert.setup.id != self.link.setup()?.position.id
                    || cert.setup.epoch < self.link.setup()?.position.epoch
                    || self.link.leadership.contains_key(&cert.setup.epoch)
                {
                    return Ok(());
                }
                if self.take_tip(doc.clone()).await.is_err()
                    && !self.pending_tips.iter().any(|old| old.hash == doc.hash)
                {
                    if self.pending_tips.len() == 8 {
                        self.pending_tips.pop_front();
                    }
                    self.pending_tips.push_back(doc);
                    self.next = Some(Instant::now());
                }
                Ok(())
            }
            Kind::Peers | Kind::Phones => {
                if let Some(next) = document::take(&self.link, &doc)? {
                    let from_other = next.setup()?.role == Role::Member;
                    self.commit(next).await?;
                    if from_other {
                        let own = &self.own()?.public;
                        let signer = encode(&doc.origin);
                        for peer in self.roster()?.boxes.into_iter().filter(|peer| {
                            peer.key != *own
                                && peer.key != from
                                && peer.sign.as_ref().is_some_and(|sign| *sign != signer)
                        }) {
                            if let Some(host) = Host::from_text(&peer.key, &peer.address) {
                                self.send_doc(&host, doc.envelope.clone()).await;
                            }
                        }
                    }
                }
                Ok(())
            }
            Kind::Personalities => Ok(()),
        }
    }

    async fn take_replica(&mut self, doc: Document) -> Result<(), Error> {
        #[derive(Deserialize)]
        struct Replica {
            id: String,
            epoch: u64,
            replica: Shared,
            #[serde(default)]
            outcomes: Outcomes,
        }
        let setup = self.link.setup()?;
        if setup.role != Role::Member
            || setup.sign != encode(&doc.origin)
            || setup.position.epoch != doc.n
        {
            return Ok(());
        }
        let replica: Replica = serde_json::from_value(Value::Object(doc.body))
            .map_err(|_| Error::Invalid("setup replica"))?;
        if replica.id != setup.position.id || replica.epoch != setup.position.epoch {
            return Ok(());
        }
        validate_shared(&replica.replica)?;
        membership(&replica.replica, &setup.position, &setup.sign)?;
        if replica.replica.keys().any(|kind| !kind.published())
            || replica.replica.iter().any(|(kind, entry)| {
                self.link.shared.get(kind).is_some_and(|old| {
                    old.sequence > entry.sequence
                        || (old.sequence == entry.sequence && old.value != entry.value)
                })
            })
        {
            return Ok(());
        }
        let mut next = self.link.clone();
        let mut changed = false;
        for (operation, outcome) in replica.outcomes.into_iter().take(256) {
            if safe_token(&operation, 22, 43)
                && outcome.epoch <= setup.position.epoch
                && !next.shared_outcomes.contains_key(&operation)
            {
                next.shared_outcomes.insert(operation, outcome);
                changed = true;
            }
        }
        if replica.replica.iter().any(|(kind, entry)| {
            self.link
                .shared
                .get(kind)
                .is_none_or(|old| old.sequence < entry.sequence)
        }) {
            next.shared.extend(replica.replica);
            changed = true;
        }
        let envelope = json!(doc.envelope);
        if next.extra.get("setupReplica") != Some(&envelope) {
            next.extra.insert("setupReplica".into(), envelope);
            changed = true;
        }
        if changed {
            self.commit(next).await?;
        }
        Ok(())
    }

    pub(super) async fn publish(&mut self, tip: bool) -> Result<(), Error> {
        if self.network.is_none() || self.link.setup().is_err() || self.identity.is_none() {
            return Ok(());
        }
        self.refresh_exports().await?;
        let setup = self.link.setup()?;
        let have = document::header(&self.link, &self.exports);
        for peer in self.roster()?.hosts(&self.own()?.public) {
            if tip {
                self.send(&peer, json!({"docs": {"have": have}, "at": lease::wall()}))
                    .await;
            }
            if setup.role == Role::Primary {
                self.publish_to(peer, tip).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn publish_to(&self, peer: Host, tip: bool) -> Result<(), Error> {
        let setup = self.link.setup()?;
        if tip && let Some(envelope) = self.link.leadership.get(&setup.position.epoch) {
            for doc in checkpoint::packet(&self.files, envelope)? {
                self.send_doc(&peer, doc).await;
            }
        }
        if self.can_write().is_ok() {
            for (_, export) in &self.exports {
                self.send_doc(&peer, export.clone()).await;
            }
            let replica: Shared = self
                .link
                .shared
                .iter()
                .filter(|(kind, _)| kind.published())
                .map(|(kind, value)| (*kind, value.clone()))
                .collect();
            let mut outcomes: Vec<_> = self.link.shared_outcomes.iter().collect();
            outcomes.sort_by_key(|(_, outcome)| (outcome.epoch, outcome.sequence));
            let outcomes: Outcomes = outcomes
                .into_iter()
                .rev()
                .take(256)
                .map(|(operation, outcome)| (operation.clone(), outcome.clone()))
                .collect();
            let body = json!({
                "id": setup.position.id, "epoch": setup.position.epoch,
                "relayTerm": self.lease.seen.as_ref().map(|lease| lease.term), "replica": replica, "outcomes": outcomes,
            });
            let doc = documents::sign(
                &self.own()?.seed,
                Kind::SetupShared,
                setup.position.epoch,
                object(body),
            )?;
            self.send_doc(&peer, doc).await;
        }
        Ok(())
    }

    async fn send_doc(&self, peer: &Host, doc: documents::Envelope) {
        if doc.d.len() + doc.s.len() + 16 > 48_000 {
            if let Ok(parts) = documents::split(&doc) {
                for part in parts {
                    self.send(peer, json!({"docs": {"part": part}, "at": lease::wall()}))
                        .await;
                }
            }
        } else {
            self.send(peer, json!({"docs": {"give": [doc]}, "at": lease::wall()}))
                .await;
        }
    }

    pub(super) async fn send(&self, peer: &Host, value: Value) -> bool {
        let Some(relay) = &self.network else {
            return false;
        };
        relay
            .sender
            .mail(peer.clone(), mail::Envelope(object(value)))
            .await
            .is_ok()
    }

    pub(super) fn host(&self, key: &str) -> Option<Host> {
        let peer = self
            .roster()
            .ok()?
            .boxes
            .into_iter()
            .find(|peer| peer.key == key)?;
        Host::from_text(&peer.key, &peer.address)
    }

    pub(super) async fn send_authority(&self, to: &str, mut fields: Value) -> bool {
        let Ok(setup) = self.link.setup() else {
            return false;
        };
        let Some(peer) = self.host(to) else {
            return false;
        };
        fields["id"] = json!(setup.position.id);
        fields["epoch"] = json!(setup.position.epoch);
        self.send(
            &peer,
            json!({"setupAuthority": fields, "at": lease::wall()}),
        )
        .await
    }
}
