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
        self, Applicant, CLAIM_LIFETIME, Cancellation, Claim, Code, Device, INVITE_LIFETIME,
        Invitation, hash, relay_url,
    },
    model::{Peer, Role, SharedKind, Status, safe_token},
    random,
};

impl Owner {
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
        let name = crate::preferences::short_name(&name, 101);
        if name.is_empty() || name.encode_utf16().count() > 100 {
            return Ok(refused(400, "Give it a name of up to 100 characters."));
        }
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
        credentials: Option<Value>,
        submitter: Submitter,
    ) -> Result<Response, Error> {
        if let Some(given) = credentials {
            let Some(ids) = given.as_array().filter(|ids| {
                ids.len() <= 300
                    && ids
                        .iter()
                        .all(|id| id.as_str().is_some_and(|id| safe_token(id, 43, 43)))
            }) else {
                return Ok(refused(
                    400,
                    "Choose sign-ins and keys by their checklist IDs.",
                ));
            };
            if !ids.is_empty() {
                return Ok(refused(
                    503,
                    "Some selected sign-ins or keys are no longer available. Open the checklist again.",
                ));
            }
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
            let _ = self.tick().await;
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
        let token = encode(&random::<24>()?);
        let now = lease::wall();
        let expires = now + INVITE_LIFETIME;
        let code = Code::new(
            expires,
            position.id.clone(),
            relay.clone(),
            Host {
                key: decode_exact(&own.public).ok_or(Error::Invalid("installer primary key"))?,
                address: address_of(&own.secret),
            },
            token,
        );
        let mut next = self.link.clone();
        next.installer_invites.retain(|_, entry| entry.live(now));
        if next.installer_invites.len() >= 32 {
            return Ok(refused(
                409,
                "There are too many installers waiting to join. Use an existing code or wait for it to expire.",
            ));
        }
        next.installer_invites.insert(
            hash(&code.token),
            Invitation {
                setup: position.id,
                epoch: position.epoch,
                expires,
                actor,
                claim: None,
                extra: Default::default(),
            },
        );
        self.commit(next).await?;
        Ok(Response {
            status: 201,
            value: json!({"code": code.encoded(), "expires": expires}),
        })
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
            || given.cancel
            || given.merging
            || given.credential_ack
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
            || invite.extra.contains_key("credentialSelectionDigest")
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
            if !self.lease.holds(&self.link) {
                self.tick().await?;
                if !self.lease.holds(&self.link) || self.can_write().is_err() {
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
        let mut answer = json!({"copyCredentials": false});
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
        self.publish(false).await
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
