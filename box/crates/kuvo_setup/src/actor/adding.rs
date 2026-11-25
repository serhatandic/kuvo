use kuvo_wire::{
    documents::{self, Kind},
    keys::{Address, Host, address_of, decode_exact, encode},
};
use serde_json::{Value, json};

use super::{Owner, Response, lease};
use crate::{
    Error, document,
    edits::Submitter,
    installer::{
        self, Applicant, CLAIM_LIFETIME, Cancellation, Claim, Device, INVITE_LIFETIME, Invitation,
        hash, relay_url,
    },
    model::{Membership, Peer, Role, SharedKind, Status, safe_token},
    random,
    seed::source::Consent,
};

impl Owner {
    pub(super) fn ssh_admission(&self, by: &Submitter) -> Result<Response, Error> {
        if !by.current(&self.link) {
            return Ok(refused(403, "Pair this device before adding a computer."));
        }
        let Some(setup) = self.link.setup().ok() else {
            return Ok(refused(
                409,
                "This computer can add members once its setup is ready.",
            ));
        };
        if setup.role != Role::Primary {
            let main = self
                .roster()?
                .boxes
                .into_iter()
                .find(|peer| peer.key == setup.position.primary);
            let name = main
                .as_ref()
                .map(|peer| peer.name.as_str())
                .unwrap_or("your main computer");
            return Ok(Response {
                status: 409,
                value: json!({"message": format!("Add computers on {name}, your main computer."), "primary": setup.position.primary}),
            });
        }
        self.can_write()?;
        if self.link.relay.is_none() {
            return Ok(refused(
                503,
                "This computer isn’t linked to your other computers yet, so it can’t add one.",
            ));
        }
        Ok(Response::ok(json!({})))
    }

    pub(super) async fn keep_ssh_address(
        &mut self,
        key: String,
        invite: String,
        destination: String,
        identity: String,
        distro: Option<String>,
        by: Submitter,
    ) -> Result<Response, Error> {
        let allowed = self.ssh_admission(&by)?;
        if allowed.status != 200 {
            return Ok(allowed);
        }
        let mut roster = self.roster()?;
        let Some(peer) = roster.boxes.iter_mut().find(|peer| peer.key == key) else {
            return Ok(refused(
                409,
                "The setup changed before the SSH address was saved.",
            ));
        };
        if !self
            .link
            .installer_invites
            .get(&invite)
            .and_then(|invite| invite.claim.as_ref())
            .is_some_and(|claim| claim.matches(peer))
        {
            return Ok(refused(
                409,
                "The setup changed before the SSH address was saved.",
            ));
        }
        peer.extra.insert("id".into(), json!(destination));
        let mut addresses = crate::ssh::saved_machines(&self.files)?;
        let record = addresses
            .iter_mut()
            .find(|row| row["destination"] == destination);
        let mut entry = record
            .as_deref()
            .cloned()
            .unwrap_or_else(|| json!({"destination": destination}));
        entry["identity"] = json!(identity);
        if let Some(distro) = distro {
            entry["wslDistro"] = json!(distro);
        }
        if let Some(record) = record {
            *record = entry;
        } else {
            addresses.push(entry);
        }
        crate::ssh::keep_ssh_host(&self.files, &destination, peer)?;
        let mut next = self.link.clone();
        next.put(
            self.own()?,
            SharedKind::MembershipAndAccess,
            serde_json::to_value(roster).expect("roster serializes"),
            None,
            None,
        )?;
        self.files.write("machines.json", &addresses)?;
        self.commit(next).await?;
        self.publish(false).await?;
        Ok(Response::ok(json!({})))
    }

    pub(super) fn keep_ssh_alias(
        &self,
        identity: &str,
        expected_key: Option<&str>,
        destination: &str,
        name: &str,
        revisions: &crate::ssh::RemovalRevisions,
        by: &Submitter,
    ) -> Result<Response, Error> {
        let allowed = self.ssh_admission(by)?;
        if allowed.status != 200 {
            return Ok(allowed);
        }
        let roster = self.roster()?;
        if let Some(key) = expected_key {
            let before = revisions.get(key).copied().unwrap_or_default();
            let now = roster
                .extra
                .get("removalRevisions")
                .and_then(|revisions| revisions.get(key))
                .and_then(Value::as_u64)
                .unwrap_or_default();
            if before != now {
                return Ok(Response {
                    status: 409,
                    value: json!({"message": format!("{name} was removed from your computers. Add it again to restore access.")}),
                });
            }
        }
        let mut addresses = crate::ssh::saved_machines(&self.files)?;
        let Some(machine) = addresses.iter_mut().find(|row| {
            row["identity"].as_str() == Some(identity) && row["destination"] != destination
        }) else {
            return Ok(refused(404, "That isn't one of your computers."));
        };
        let Some(known) = roster.boxes.iter().find(|peer| {
            machine["destination"]
                .as_str()
                .is_some_and(|saved| peer.extra.get("id").and_then(Value::as_str) == Some(saved))
                && expected_key.is_none_or(|expected| peer.key == expected)
        }) else {
            return Ok(refused(404, "That isn't one of your computers."));
        };
        let mut aliases = machine["otherAddresses"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !aliases.iter().any(|alias| alias == destination) {
            aliases.push(json!(destination));
            machine["otherAddresses"] = json!(aliases);
            self.files.write("machines.json", &addresses)?;
        }
        Ok(Response::ok(json!({"key": known.key, "name": known.name})))
    }

    pub(super) async fn cancel_installer(
        &mut self,
        receipt: Cancellation,
    ) -> Result<Response, Error> {
        const SAVE_FAILED: &str = "The main computer could not save this cancellation. Check its connection to Kuvo’s relay, then retry the receipt.";
        let Ok(setup) = self.link.setup() else {
            return Ok(refused(
                409,
                "Run this command on the current main computer of the setup that issued the code.",
            ));
        };
        let position = setup.position.clone();
        if setup.role != Role::Primary || position.id != receipt.setup {
            return Ok(refused(
                409,
                "Run this command on the current main computer of the setup that issued the code.",
            ));
        }
        let mut roster = self.roster()?;
        if roster.left.contains(&receipt.key)
            && self.link.shared_outcomes.iter().any(|(id, outcome)| {
                outcome.kind == SharedKind::MembershipAndAccess
                    && outcome.status == Status::Committed
                    && *id == receipt.operation(outcome.epoch)
            })
        {
            return Ok(Response::ok(
                json!({"message": "The provisional installer entry was already retired."}),
            ));
        }
        let mut recoveries = installer::recoveries(&self.link, &roster);
        let entry = recoveries.get(&receipt.key);
        if entry.is_none_or(|entry| {
            entry["invite"] != receipt.invite || entry["commitment"] != hash(&receipt.secret)
        }) {
            return Ok(refused(
                409,
                "The accepted roster has no matching installer recovery entry. No computer was retired.",
            ));
        }
        if self.can_write().is_err()
            || position.primary == receipt.key
            || roster
                .extra
                .get("wipePreparations")
                .and_then(|rows| rows.get(&receipt.key))
                .is_some()
            || self
                .link
                .installer_invites
                .get(&receipt.invite)
                .is_some_and(|invite| {
                    ["activated", "credentialsCompleted"]
                        .iter()
                        .any(|flag| invite.extra.get(*flag) == Some(&Value::Bool(true)))
                })
        {
            return Ok(refused(409, SAVE_FAILED));
        }
        let Some(index) = roster.boxes.iter().position(|peer| peer.key == receipt.key) else {
            return Ok(refused(409, SAVE_FAILED));
        };
        let peer = roster.boxes.remove(index);
        let mut next = self.link.clone();
        recoveries.shift_remove(&receipt.key);
        installer::keep_recoveries(&mut next, &mut roster, recoveries)?;
        super::removal::retire_computer(&mut roster, &position.id, &peer, &position.primary)?;
        if let Some(invite) = next.installer_invites.get_mut(&receipt.invite)
            && invite
                .claim
                .as_ref()
                .is_some_and(|claim| claim.matches(&peer))
        {
            invite.extra.insert("cancelled".into(), json!(true));
            invite.extra.shift_remove("credentialSeed");
            invite.extra.shift_remove("credentialSelectionDigest");
        }
        let write = next.put(
            self.own()?,
            SharedKind::MembershipAndAccess,
            serde_json::to_value(roster).expect("roster serializes"),
            None,
            Some(receipt.operation(position.epoch)),
        );
        if write.is_err() || self.commit(next).await.is_err() {
            return Ok(refused(409, SAVE_FAILED));
        }
        if let Err(error) = self.publish(false).await {
            eprintln!("publishing installer cancellation: {error}");
        }
        Ok(Response::ok(
            json!({"message": "The provisional installer entry was retired."}),
        ))
    }

    pub(super) async fn rename_computer(
        &mut self,
        key: String,
        name: String,
        by: Submitter,
    ) -> Result<Response, Error> {
        if !by.current(&self.link) {
            return Ok(refused(403, "The submitting device no longer has access."));
        }
        if !self.roster()?.boxes.iter().any(|peer| peer.key == key) {
            return Ok(refused(404, "That isn't one of your computers."));
        }
        let Some(name) = crate::routes::given_name(&name) else {
            return Ok(refused(400, "Give it a name of up to 100 characters."));
        };
        let mut names = self
            .link
            .value(SharedKind::SetupNames)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        names.insert(key, json!(name));
        let receipt = self
            .save(SharedKind::SetupNames, Value::Object(names), None, None)
            .await?;
        Ok(Response::ok(super::with_receipt(
            json!({"name": name}),
            receipt,
        )))
    }

    pub(super) async fn issue_installer_code(
        &mut self,
        credentials: Option<crate::seed::source::Prepared>,
        submitter: Submitter,
    ) -> Result<Response, Error> {
        let token = encode(&random::<24>()?);
        let consent = credentials.map_or(Consent::None, Consent::Selected);
        let offer = self
            .issue_installer_offer(hash(&token), consent, submitter)
            .await?;
        installer::complete_offer(offer, token)
    }

    pub(super) async fn issue_installer_offer(
        &mut self,
        id: String,
        credentials: Consent,
        submitter: Submitter,
    ) -> Result<Response, Error> {
        if !safe_token(&id, 43, 43) {
            return Ok(refused(400, "Send the installer invitation hash."));
        }
        let Some(setup) = self
            .link
            .setup()
            .ok()
            .filter(|setup| setup.role == Role::Primary)
        else {
            return Ok(refused(
                409,
                "Only the main computer can issue an installer code.",
            ));
        };
        let position = setup.position.clone();
        // A sole computer may issue a code while offline. The claim needs the relay lease.
        if !self.lease.holds(&self.link) {
            let _ = self.tick_for_add().await;
        }
        self.can_write()?;
        let actor = match submitter {
            Submitter::Computer => None,
            Submitter::Paired { key, pairing } => {
                let device = Device { key, pairing };
                if !self.installer_device(&device) {
                    return Ok(refused(403, "Pair this device before adding a computer."));
                }
                Some(device)
            }
        };
        let Some(relay) = self.link.relay.as_ref().filter(|url| relay_url(url, true)) else {
            return Ok(refused(503, "This computer can't reach your relay yet."));
        };
        let own = self.own()?;
        let now = lease::wall();
        let prior = self.link.installer_invites.get(&id);
        let requested = credentials.digest();
        if let Some(prior) = prior
            && (prior.setup != position.id
                || prior.epoch != position.epoch
                || !prior.live(now)
                || prior.actor != actor
                || prior
                    .extra
                    .get("credentialSelectionDigest")
                    .and_then(Value::as_str)
                    != requested
                || prior.extra.get("credentialsCompleted") == Some(&Value::Bool(true))
                || prior.extra.contains_key("merge"))
        {
            return Ok(refused(
                409,
                "This installer invitation is no longer available.",
            ));
        }
        let expiry = prior.map_or(now + INVITE_LIFETIME, |prior| prior.expires);
        let answer = Response {
            status: 201,
            value: json!({
                "expires": expiry, "setup": position.id, "relay": relay,
                "primary": own.public, "address": encode(&address_of(&own.secret)),
            }),
        };
        if prior.is_some() {
            return Ok(answer);
        }
        if matches!(credentials, Consent::Kept(_)) {
            return Ok(refused(
                409,
                "This installer invitation is no longer available.",
            ));
        }
        let mut next = self.link.clone();
        next.installer_invites.retain(|_, entry| entry.live(now));
        if next.installer_invites.len() >= 32 {
            return Ok(refused(
                409,
                "There are too many installers waiting to join. Use an existing code or wait for it to expire.",
            ));
        }
        let mut fields = serde_json::Map::new();
        if let Consent::Selected(chosen) = credentials {
            let sealed = installer::credentials::seal(&chosen.seed, &own.key, &id)?;
            fields.insert("credentialSeed".into(), json!(sealed));
            fields.insert("credentialSelectionDigest".into(), json!(chosen.digest));
        }
        next.installer_invites.insert(
            id,
            Invitation {
                setup: position.id,
                epoch: position.epoch,
                expires: expiry,
                actor,
                claim: None,
                extra: fields,
            },
        );
        self.commit(next).await?;
        Ok(answer)
    }

    fn installer_device(&self, device: &Device) -> bool {
        !self.link.devices_disabled
            && self
                .identity
                .as_ref()
                .zip(self.roster().ok())
                .is_some_and(|(own, roster)| {
                    decode_exact::<32>(&device.key).is_some_and(|key| {
                        self.link
                            .grants(&roster, &own.public)
                            .get(&key)
                            .is_some_and(|grant| grant.id == device.pairing)
                    })
                })
    }

    pub(super) async fn receive_installer(
        &mut self,
        from: Address,
        key: [u8; 32],
        payload: &[u8],
    ) -> Result<(), Error> {
        let Ok(given) = serde_json::from_slice::<Applicant>(payload) else {
            return Ok(());
        };
        if !safe_token(&given.token, 32, 32)
            || !safe_token(&given.nonce, 32, 32)
            || decode_exact::<32>(&given.sign).is_none()
            || decode_exact::<16>(&given.address).is_none()
            || given.name.encode_utf16().count() > 100
            || given.merging
            || (given.credential_ack && given.activation_ack)
            || (given.cancel && (given.credential_ack || given.activation_ack))
        {
            return Ok(());
        }
        let id = hash(&given.token);
        let Some(invite) = self.link.installer_invites.get(&id).cloned() else {
            return Ok(());
        };
        if invite.extra.contains_key("merge") {
            return Ok(());
        }
        let name = crate::preferences::short_name(&given.name, 100);
        let peer = Peer {
            key: encode(&key),
            sign: Some(given.sign.clone()),
            address: given.address.clone(),
            kind: given.kind,
            name: if name.is_empty() {
                "Computer".into()
            } else {
                name
            },
            extra: Default::default(),
        };
        let to = Host { address: from, key };
        let roster = self.roster()?;
        if given.cancel {
            return self
                .cancel_applicant(&id, invite, &peer, roster, &to, &given)
                .await;
        }
        let enrolled = roster.boxes.iter().any(|each| {
            each.key == peer.key && each.sign == peer.sign && each.address == peer.address
        });
        let claimed = invite
            .claim
            .as_ref()
            .is_some_and(|claim| claim.matches(&peer));
        let answer = if invite.extra.get("cancelled") == Some(&Value::Bool(true)) {
            Some(
                json!({"message": "This enrollment was cancelled. Get a new installer code from your setup."}),
            )
        } else if !self.link.setup().is_ok_and(|setup| {
            setup.role == Role::Primary
                && setup.position.id == invite.setup
                && (setup.position.epoch == invite.epoch || claimed && enrolled)
        }) {
            Some(
                json!({"message": "Your setup’s main computer changed. Get a new installer code from the same setup.", "retrySameSetup": true}),
            )
        } else if !invite.live(lease::wall())
            || (invite.claim.is_some() && !claimed)
            || (!claimed
                && invite
                    .actor
                    .as_ref()
                    .is_some_and(|device| !self.installer_device(device)))
        {
            Some(
                json!({"message": "This installer code is no longer available. Get a new code from your setup."}),
            )
        } else if invite.extra.contains_key("credentialSeed")
            && !given.credential_receipt
            && !given.activation_ack
        {
            Some(
                json!({"message": "Update this computer’s installer to copy the sign-ins and keys selected for this code.", "retrySameSetup": true}),
            )
        } else if (invite.extra.contains_key("credentialSelectionDigest")
            && !invite.extra.contains_key("credentialSeed"))
            || invite
                .extra
                .get("credentials")
                .and_then(Value::as_array)
                .is_some_and(|ids| !ids.is_empty())
        {
            Some(
                json!({"message": "Open the checklist again to issue a code for these sign-ins and keys."}),
            )
        } else {
            None
        };
        if let Some(answer) = answer {
            self.installer_reply(&to, &given.nonce, answer).await;
            return Ok(());
        }
        if self.can_write().is_err() {
            return Ok(());
        }
        if given.activation_ack {
            if claimed && enrolled {
                self.activate_installer(&to, &given.nonce, &id, &peer)
                    .await?;
            }
            return Ok(());
        }
        if given.credential_ack {
            if claimed && enrolled {
                self.ack_installer_credentials(&to, &given.nonce, &id)
                    .await?;
            }
            return Ok(());
        }
        if !given.confirmed {
            self.installer_reply(
                &to,
                &given.nonce,
                json!({ "ready": true, "recoveryCapable": true }),
            )
            .await;
            return Ok(());
        }
        if !enrolled {
            let setup_id = self.link.setup()?.position.id.clone();
            let solo_high = crate::lease::required_local_high(&self.files, &setup_id)?;
            if !self.lease.holds(&self.link)
                || self
                    .lease
                    .seen
                    .as_ref()
                    .is_none_or(|seen| seen.high < solo_high)
            {
                self.tick_for_add().await?;
                if !self.lease.holds(&self.link)
                    || self.can_write().is_err()
                    || self
                        .lease
                        .seen
                        .as_ref()
                        .is_none_or(|seen| seen.high < solo_high)
                {
                    return Ok(());
                }
            }
            if let Some(message) = self.enroll_installer(&id, &given, peer).await? {
                self.installer_reply(&to, &given.nonce, json!({"message": message}))
                    .await;
                return Ok(());
            }
        } else if !claimed {
            // An already-enrolled peer cannot bind a fresh invitation to its membership.
            self.installer_reply(
                &to,
                &given.nonce,
                json!({"message": "This enrollment changed."}),
            )
            .await;
            return Ok(());
        }
        self.installer_membership(&to, &id, &given).await
    }

    async fn cancel_applicant(
        &mut self,
        id: &str,
        invite: Invitation,
        peer: &Peer,
        mut roster: Membership,
        to: &Host,
        given: &Applicant,
    ) -> Result<(), Error> {
        let completed = |flag| invite.extra.get(flag) == Some(&Value::Bool(true));
        if !given.activation_ack_capable
            || !self
                .link
                .setup()
                .is_ok_and(|setup| setup.role == Role::Primary && setup.position.id == invite.setup)
            || self.can_write().is_err()
            || ["localUnlink", "localWipe", "removalSource"]
                .iter()
                .any(|key| {
                    self.link
                        .extra
                        .get(*key)
                        .is_some_and(|value| !value.is_null())
                })
            || completed("activated")
            || completed("credentialsCompleted")
            || (!completed("cancelled") && !invite.live(lease::wall()))
            || invite.claim.as_ref().map_or_else(
                || {
                    invite
                        .actor
                        .as_ref()
                        .is_some_and(|device| !self.installer_device(device))
                },
                |claim| !claim.matches(peer),
            )
        {
            return Ok(());
        }
        let listed = roster
            .boxes
            .iter()
            .position(|listed| listed.key == peer.key);
        let mut recoveries = installer::recoveries(&self.link, &roster);
        if let Some(at) = listed {
            let previous = &roster.boxes[at];
            if previous.sign != peer.sign
                || previous.address != peer.address
                || roster
                    .extra
                    .get("wipePreparations")
                    .is_some_and(|rows| rows.get(&peer.key).is_some())
            {
                return Ok(());
            }
            let Some(claim) = invite.claim.as_ref() else {
                return Ok(());
            };
            if claim.extra.get("provisional") != Some(&Value::Bool(true)) {
                self.installer_reply(to, &given.nonce, json!({
                    "message": "The main computer could not verify that this invitation created the roster entry. Review that entry on the main computer."
                })).await;
                return Ok(());
            }
            if !self.link.shared_outcomes.get(id).is_some_and(|entry| {
                entry.kind == SharedKind::MembershipAndAccess && entry.status == Status::Committed
            }) || (claim.extra.get("recoveryBound") == Some(&Value::Bool(true))
                && !recoveries
                    .get(&peer.key)
                    .is_some_and(|entry| entry["invite"] == id))
            {
                return Ok(());
            }
        } else if completed("cancelled") {
            self.installer_reply(to, &given.nonce, json!({"cancelled": true}))
                .await;
            return Ok(());
        }

        let mut next = self.link.clone();
        let entry = next.installer_invites.get_mut(id).ok_or(Error::Conflict)?;
        entry.claim.get_or_insert_with(|| Claim {
            key: peer.key.clone(),
            sign: given.sign.clone(),
            address: peer.address.clone(),
            until: lease::wall() + CLAIM_LIFETIME,
            extra: Default::default(),
        });
        entry.extra.insert("cancelled".into(), json!(true));
        entry.extra.shift_remove("credentialSeed");
        entry.extra.shift_remove("credentialSelectionDigest");
        if let Some(at) = listed {
            let peer = roster.boxes.remove(at);
            recoveries.shift_remove(&peer.key);
            installer::keep_recoveries(&mut next, &mut roster, recoveries)?;
            let own = self.own()?;
            super::removal::retire_computer(&mut roster, &invite.setup, &peer, &own.public)?;
            next.put(
                own,
                SharedKind::MembershipAndAccess,
                serde_json::to_value(roster).expect("membership serializes"),
                None,
                None,
            )?;
        }
        self.commit(next).await?;
        if listed.is_some()
            && let Err(error) = self.publish(false).await
        {
            eprintln!("publishing installer cancellation: {error}");
        }
        self.installer_reply(to, &given.nonce, json!({"cancelled": true}))
            .await;
        Ok(())
    }

    async fn enroll_installer(
        &mut self,
        id: &str,
        given: &Applicant,
        peer: Peer,
    ) -> Result<Option<String>, Error> {
        let mut roster = self.roster()?;
        let sign = &given.sign;
        if roster.left.contains(&peer.key)
            || roster
                .boxes
                .iter()
                .any(|each| each.key == peer.key || each.sign.as_ref() == Some(sign))
            || roster
                .extra
                .get("computerHistory")
                .and_then(|h| h["retiredSigns"].as_array())
                .is_some_and(|rows| rows.iter().any(|old| old == sign))
            || roster
                .extra
                .get("removals")
                .and_then(Value::as_object)
                .is_some_and(|rows| rows.values().any(|old| old["peer"]["sign"] == *sign))
        {
            return Ok(Some(format!(
                "{} presented membership keys that cannot be used. Update Kuvo Code there and add it again.",
                peer.name
            )));
        }
        if roster.boxes.len() >= 65 {
            return Ok(Some(
                "The setup already has the maximum number of computers.".into(),
            ));
        }
        let mut reserved = self.link.clone();
        let invite = reserved
            .installer_invites
            .get_mut(id)
            .ok_or(Error::Conflict)?;
        let setup = invite.setup.clone();
        invite.claim.get_or_insert_with(|| Claim {
            key: peer.key.clone(),
            sign: sign.clone(),
            address: peer.address.clone(),
            until: lease::wall() + CLAIM_LIFETIME,
            extra: Default::default(),
        });
        self.commit(reserved).await?;

        let mut next = self.link.clone();
        if next.shared_outcomes.contains_key(id) {
            return Ok(Some("This enrollment changed.".into()));
        }
        let recovery = given
            .recovery_commitment
            .as_deref()
            .filter(|text| given.activation_ack_capable && safe_token(text, 43, 43));
        let fields = &mut next
            .installer_invites
            .get_mut(id)
            .and_then(|invite| invite.claim.as_mut())
            .ok_or(Error::Conflict)?
            .extra;
        if given.activation_ack_capable {
            fields.insert("provisional".into(), json!(true));
        }
        if let Some(commitment) = recovery {
            fields.insert("recoveryBound".into(), json!(true));
            let mut recoveries = installer::recoveries(&next, &roster);
            recoveries.insert(
                peer.key.clone(),
                json!({
                    "invite": id, "commitment": commitment, "sign": sign, "address": peer.address,
                }),
            );
            installer::keep_recoveries(&mut next, &mut roster, recoveries)?;
        }
        let own = self.own()?;
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
            .filter(|key| *key != &own.public)
            .chain(std::iter::once(&peer.key))
        {
            if !slots.iter().any(|slot| slot["key"] == *key) {
                slots.push(json!({"key": key}));
            }
        }
        roster.boxes.push(peer);
        next.put(
            own,
            SharedKind::MembershipAndAccess,
            serde_json::to_value(roster).expect("roster serializes"),
            None,
            Some(id.into()),
        )?;
        self.commit(next).await?;
        Ok(None)
    }

    async fn activate_installer(
        &mut self,
        to: &Host,
        nonce: &str,
        id: &str,
        peer: &Peer,
    ) -> Result<(), Error> {
        let mut roster = self.roster()?;
        let mut next = self.link.clone();
        let invite = next.installer_invites.get_mut(id).ok_or(Error::Conflict)?;
        let portable = invite
            .claim
            .as_ref()
            .is_some_and(|claim| claim.extra.get("recoveryBound") == Some(&Value::Bool(true)));
        let mut recoveries = installer::recoveries(&next, &roster);
        if recoveries
            .get(&peer.key)
            .is_some_and(|recovery| recovery["invite"] == id)
        {
            recoveries.shift_remove(&peer.key);
            installer::keep_recoveries(&mut next, &mut roster, recoveries)?;
            next.put(
                self.own()?,
                SharedKind::MembershipAndAccess,
                serde_json::to_value(roster).expect("roster serializes"),
                None,
                None,
            )?;
        }
        invite_activated(&mut next, id)?;
        self.commit(next).await?;
        let mut answer = json!({"activated": true});
        if portable {
            answer["portableActivation"] = json!(true);
        }
        self.installer_reply(to, nonce, answer).await;
        self.publish(false).await
    }

    async fn ack_installer_credentials(
        &mut self,
        to: &Host,
        nonce: &str,
        id: &str,
    ) -> Result<(), Error> {
        let mut next = self.link.clone();
        let invite = next.installer_invites.get_mut(id).ok_or(Error::Conflict)?;
        if invite.extra.get("credentialsCompleted") != Some(&Value::Bool(true)) {
            if !invite.extra.contains_key("credentialSeed")
                || invite.extra.get("activated") != Some(&Value::Bool(true))
            {
                return Ok(());
            }
            invite.extra.shift_remove("credentialSeed");
            invite.extra.shift_remove("credentialSelectionDigest");
            invite
                .extra
                .insert("credentialsCompleted".into(), json!(true));
            self.commit(next).await?;
        }
        self.installer_reply(to, nonce, json!({"credentialComplete": true}))
            .await;
        Ok(())
    }

    async fn installer_membership(
        &mut self,
        to: &Host,
        id: &str,
        given: &Applicant,
    ) -> Result<(), Error> {
        let mut next = self.link.clone();
        let (exports, changed) = document::published(&mut next, self.own()?)?;
        if changed {
            self.commit(next).await?;
        }
        let Some((_, envelope)) = exports.into_iter().find(|(kind, _)| *kind == Kind::Peers) else {
            return Ok(());
        };
        let invite = self.link.installer_invites.get(id).ok_or(Error::Conflict)?;
        let seed = invite.extra.get("credentialSeed").and_then(Value::as_str);
        let mut answer = json!({"copyCredentials": seed.is_some()});
        if seed.is_some()
            && let Some(claim) = &invite.claim
        {
            answer["credentialUntil"] = json!(claim.until);
        }
        if given.activation_ack_capable {
            answer["activationReceipt"] = json!(true);
        }
        if self
            .link
            .installer_invites
            .get(id)
            .and_then(|invite| invite.claim.as_ref())
            .is_some_and(|claim| {
                claim
                    .extra
                    .get("recoveryBound")
                    .is_some_and(|flag| flag == true)
            })
        {
            answer["portableActivation"] = json!(true);
        }
        if envelope.d.len() + envelope.s.len() + 16 <= 48_000 {
            answer["docs"] = json!({"give": [envelope]});
            self.installer_reply(to, &given.nonce, answer).await;
        } else {
            for part in documents::split(&envelope)? {
                answer["docs"] = json!({"part": part});
                self.installer_reply(to, &given.nonce, answer.clone()).await;
            }
        }
        if let Some(seed) = seed {
            self.send_installer_credentials(to, &given.nonce, id, seed)
                .await?;
        }
        self.publish(false).await
    }

    async fn send_installer_credentials(
        &self,
        to: &Host,
        nonce: &str,
        id: &str,
        seed: &str,
    ) -> Result<(), Error> {
        use installer::credentials::{COPY_FAILED, open, split};
        match open(seed, &self.own()?.key, id) {
            Some(payload) if payload.len() < 34_000 => {
                let body = match serde_json::from_slice::<Value>(&payload) {
                    Ok(seed) => json!({"seed": seed}),
                    Err(_) => json!({"credentialProblem": COPY_FAILED}),
                };
                self.installer_reply(to, nonce, body).await;
            }
            Some(payload) => {
                for part in split(&payload)? {
                    self.installer_reply(to, nonce, json!({"seedPart": part}))
                        .await;
                }
            }
            None => {
                self.installer_reply(to, nonce, json!({"credentialProblem": COPY_FAILED}))
                    .await
            }
        }
        Ok(())
    }

    async fn installer_reply(&self, to: &Host, nonce: &str, mut answer: Value) {
        answer["nonce"] = json!(nonce);
        if let Some(network) = &self.network {
            let _ = network
                .sender
                .installer(
                    to.clone(),
                    serde_json::to_vec(&answer).expect("installer reply serializes"),
                )
                .await;
        }
    }
}

fn invite_activated(next: &mut crate::model::Link, id: &str) -> Result<(), Error> {
    next.installer_invites
        .get_mut(id)
        .ok_or(Error::Conflict)?
        .extra
        .insert("activated".into(), json!(true));
    Ok(())
}

fn refused(status: u16, message: &str) -> Response {
    Response {
        status,
        value: json!({"message": message}),
    }
}
