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

impl Owner {
    pub(super) async fn tick(&mut self) -> Result<(), Error> {
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
            self.lease
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
        if setup.role == Role::Primary && self.lease.holds(&self.link) {
            self.next = Instant::now() + Duration::from_secs(10);
            self.publish(false).await?;
        } else {
            let primary = setup.position.primary.clone();
            self.send_authority(&primary, json!({
                "kind": "available", "wantsTip": !self.link.leadership.contains_key(&setup.position.epoch),
            })).await;
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
            if !roster
                .boxes
                .iter()
                .any(|peer| peer.key == own.public && peer.sign.as_deref() == Some(&own.sign))
            {
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

    pub(super) async fn receive(&mut self, event: relay::Event) -> Result<(), Error> {
        match event {
            relay::Event::Connected(_) => {
                self.next = Instant::now();
                self.publish(true).await?;
            }
            relay::Event::Session(kuvo_peers::session::Event::Accepted(id, _)) => {
                if let Some(relay) = &self.network {
                    let _ = relay.sender.close(id).await;
                }
            }
            relay::Event::Mail { key, envelope, .. } => {
                let from = encode(&key);
                if !self.roster()?.boxes.iter().any(|peer| peer.key == from) {
                    return Ok(());
                }
                if let Some(docs) = envelope.0.get("docs") {
                    self.receive_docs(key, &from, docs).await?;
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
            }
            _ => {}
        }
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
        if !reply && behind {
            self.send(&peer, json!({
                "docs": {"have": document::header(&self.link, &self.exports), "reply": true}, "at": lease::wall(),
            })).await;
        }
        Ok(())
    }

    async fn take_doc(&mut self, from: &str, doc: Document) -> Result<(), Error> {
        match doc.kind {
            Kind::SetupShared => self.take_replica(from, doc).await,
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
                    self.next = Instant::now();
                }
                Ok(())
            }
            Kind::Peers | Kind::Phones => {
                if let Some(next) = document::take(&self.link, &doc)? {
                    self.commit(next).await?;
                }
                Ok(())
            }
            Kind::Personalities => Ok(()),
        }
    }

    async fn take_replica(&mut self, from: &str, doc: Document) -> Result<(), Error> {
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
            || setup.position.primary != from
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
                self.link
                    .shared
                    .get(kind)
                    .is_some_and(|old| old.sequence > entry.sequence)
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
