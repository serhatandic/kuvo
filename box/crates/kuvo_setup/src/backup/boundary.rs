//! The accepted setup captured for an archive. No private identity files enter it.
use std::{cell::OnceCell, path::Path};

use serde_json::{Value, json};

use crate::{
    SetupHandle,
    actor::LINK,
    checkpoint,
    model::{Identity, Link, Role, SharedKind},
    storage::Files,
};
use kuvo_peers::moves::BackupFences;
use kuvo_store::{
    declarations::{self, Backup},
    locks::StaleLock,
};
use kuvo_wire::{
    documents::{self, Kind},
    keys::encode,
};

use super::{Authority, Error, Manifest, RestorePoint, Result, Source, Standalone};

pub(super) struct Boundary {
    files: Files,
    setup: SetupHandle,
    local: Standalone,
    link: Option<Link>,
    paired: bool,
    accepted: Option<Value>,
    fingerprint: String,
    fences: BackupFences,
    roots: super::inventory::Roots,
    relay: Option<super::relay::mail::PeerMail>,
    approval: OnceCell<()>,
}

impl Boundary {
    pub fn read(
        files: Files,
        setup: SetupHandle,
        roots: super::inventory::Roots,
        restoring: bool,
    ) -> Result<Self> {
        let roster = setup.rosters().borrow().clone();
        let link: Option<Link> = files.read(LINK).map_err(setup_error)?;
        let position = link.as_ref().and_then(|link| link.setup().ok());
        let membership = link.as_ref().and_then(|link| link.member_list().ok());
        let paired = membership
            .as_ref()
            .is_some_and(|membership| membership.boxes.len() > 1)
            || position.is_some_and(|setup| setup.role == Role::Member);
        let identity = link.as_ref().and_then(|link| Identity::read(link).ok());
        let origin = roster
            .as_ref()
            .and_then(|roster| roster.here.as_ref().map(|key| encode(key)))
            .or_else(|| identity.as_ref().map(|identity| identity.public.clone()));
        let signer = identity.map(|identity| identity.sign).or_else(|| {
            let local = origin.as_ref()?;
            link.as_ref()?
                .member_list()
                .ok()?
                .boxes
                .into_iter()
                .find(|member| &member.key == local)?
                .sign
        });
        if paired && (origin.is_none() || signer.is_none()) {
            return Err(Error::Message(
                "The computer’s backup identity is unavailable.",
            ));
        }
        let local = Standalone {
            origin,
            signer,
            setup: position.map(|setup| setup.position.id.clone()),
        };
        let accepted = match (&link, position) {
            (Some(link), Some(position)) if position.role == Role::Primary && !restoring => {
                let mut history = serde_json::Map::new();
                for envelope in link.leadership.values() {
                    for packet in checkpoint::packet(&files, envelope).map_err(|_| {
                        Error::Message("The accepted setup checkpoint is incomplete.")
                    })? {
                        history.insert(packet.d.clone(), serde_json::to_value(packet)?);
                    }
                }
                if let Some(docs) = link.extra.get("docs").and_then(Value::as_object) {
                    for value in docs.values() {
                        let Ok(envelope) = serde_json::from_value(value.clone()) else {
                            continue;
                        };
                        let Ok(doc) = documents::verify(envelope) else {
                            continue;
                        };
                        if local
                            .signer
                            .as_ref()
                            .is_some_and(|sign| *sign == encode(&doc.origin))
                            && matches!(
                                doc.kind,
                                Kind::Peers | Kind::Personalities | Kind::SetupShared
                            )
                        {
                            history.insert(
                                doc.envelope.d.clone(),
                                serde_json::to_value(doc.envelope)?,
                            );
                        }
                    }
                }
                let mut shared = serde_json::to_value(&link.shared)?
                    .as_object()
                    .cloned()
                    .expect("shared values serialize as an object");
                if link.shared_setup.as_ref() == Some(&position.position.id) {
                    shared.retain(|kind, _| {
                        declarations::data(kind).is_some_and(|data| {
                            matches!(data.backup, Backup::Storage(_) | Backup::Via { .. })
                        })
                    });
                } else {
                    shared.clear();
                }
                Some(json!({
                    "setup": position, "shared": shared, "outcomes": link.shared_outcomes,
                    "history": history.into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
                }))
            }
            _ => None,
        };
        let fingerprint = crate::model::fingerprint(&json!({
            "setup": position.map(|setup| &setup.position), "membership": membership,
            "origin": local.origin, "signer": local.signer, "paired": paired,
            "ownership": link.as_ref().map(assignments),
        }));
        let fences = BackupFences::read(&files.path("run"))
            .map_err(|error| Error::Detail(error.to_string()))?;
        Ok(Self {
            files,
            setup,
            local,
            link,
            paired,
            accepted,
            fingerprint,
            fences,
            roots,
            relay: None,
            approval: OnceCell::new(),
        })
    }

    pub fn with_relay(mut self, relay: Option<super::relay::mail::PeerMail>) -> Self {
        self.relay = relay;
        self
    }

    fn permits_id(&self, id: &str) -> bool {
        self.fences
            .permits(id, |owner| self.replacement_is_here(owner))
    }

    fn replacement_is_here(&self, owner: &str) -> bool {
        let Some(here) = self.local.origin.as_deref() else {
            return false;
        };
        if owner == here {
            return true;
        }
        let Some(link) = &self.link else { return false };
        let Ok(roster) = link.member_list() else {
            return false;
        };
        let matches = assignments(link);
        let mut key = owner;
        let mut visited = std::collections::HashSet::new();
        while roster.left.iter().any(|old| old == key) && visited.insert(key) {
            let Some(next) = matches
                .get(&format!("replacement:{key}"))
                .and_then(|record| record.get("computer"))
                .and_then(Value::as_str)
            else {
                return false;
            };
            if next == here {
                return true;
            }
            key = next;
        }
        false
    }

    fn can_recover(&self, scope: kuvo_peers::directory::RecordScope, id: &str) -> bool {
        self.permits_id(id)
            && (self.fences.owner(id).is_some()
                || self.relay.as_ref().is_none_or(|relay| {
                    relay
                        .view()
                        .announcing(scope, id)
                        .iter()
                        .all(|key| self.replacement_is_here(&encode(key)))
                }))
    }

    pub fn capture_from(
        files: Files,
        setup: SetupHandle,
        roots: super::inventory::Roots,
    ) -> Result<Self> {
        let _guard = StaleLock::acquire(
            &files.path("run/.link.lock"),
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(3),
            kuvo_store::locks::OnBusy::Refuse,
        )?;
        Self::read(files, setup, roots, false)
    }

    pub fn check_capture(&self) -> Result<()> {
        // Capture holds the accepted snapshot from the start of the operation.
        // Replica receipts and new peer announcements can arrive during a large
        // archive. Only a change of setup invalidates the selected destination.
        let link: Option<Link> = self.files.read(LINK).map_err(setup_error)?;
        let scope = link
            .as_ref()
            .and_then(|link| link.setup().ok())
            .map(|setup| &setup.position.id);
        if scope != self.local.setup.as_ref() {
            return Err(Error::Message("Backup membership changed during capture."));
        }
        Ok(())
    }
}

impl Authority for Boundary {
    fn capture(&self, source: Source, point: RestorePoint) -> Result<Manifest> {
        let mut manifest = self.local.capture(source, point)?;
        manifest.paired = self.paired;
        Ok(manifest)
    }

    fn capture_entry(&self, name: &str, _file: &Path) -> Result<bool> {
        Ok(super::inventory::references(name, &self.roots)
            .iter()
            .all(|(_, id)| self.permits_id(id)))
    }

    fn restore_entry(&self, _manifest: &Manifest, name: &str, _file: &Path) -> Result<bool> {
        Ok(super::inventory::references(name, &self.roots)
            .iter()
            .all(|(scope, id)| self.can_recover(*scope, id)))
    }

    fn insight_origins(
        &self,
        manifest: &Manifest,
    ) -> Result<Option<super::recovery::InsightOrigins>> {
        if !manifest.paired {
            return Ok(None);
        }
        let Some(own) = self.local.origin.clone() else {
            return Ok(None);
        };
        let link = self
            .link
            .as_ref()
            .ok_or(Error::Message("Wait for accepted backup membership."))?;
        let roster = link.member_list().map_err(setup_error)?;
        let ownership = assignments(link);
        let known: std::collections::HashSet<_> = roster
            .left
            .iter()
            .chain(roster.boxes.iter().map(|peer| &peer.key))
            .collect();
        let mut accepted: Vec<_> = roster
            .boxes
            .iter()
            .filter(|peer| peer.key != own)
            .map(|peer| peer.key.clone())
            .collect();
        let history = roster.extra.get("computerHistory").filter(|history| {
            history.get("setup").and_then(Value::as_str) == manifest.setup.as_deref()
        });
        for old in &roster.left {
            let matched = ownership
                .get(&format!("replacement:{old}"))
                .is_some_and(|record| {
                    record.get("source").is_some_and(Value::is_string)
                        && record
                            .get("computer")
                            .and_then(Value::as_str)
                            .is_some_and(|computer| {
                                computer != old && known.iter().any(|key| key.as_str() == computer)
                            })
                });
            let recorded =
                history
                    .and_then(|history| history.get("slots"))
                    .and_then(Value::as_array)
                    .is_some_and(|slots| {
                        slots.iter().any(|slot| {
                            slot.get("key").and_then(Value::as_str) == Some(old)
                                || slot.get("previous").and_then(Value::as_array).is_some_and(
                                    |keys| keys.iter().any(|key| key.as_str() == Some(old)),
                                )
                        })
                    });
            if matched || recorded {
                accepted.push(old.clone());
            }
        }
        Ok(Some(super::recovery::InsightOrigins { own, accepted }))
    }

    fn recover_queue(&self, _manifest: &Manifest, queue: &str, id: &str) -> Result<bool> {
        use kuvo_peers::directory::RecordScope::{Group, Session};
        Ok(self.permits_id(id)
            && match queue {
                "usr_you" => true,
                id if id.starts_with("ses_") => self.can_recover(Session, id),
                id if id.starts_with("grp_") => self.can_recover(Group, id),
                _ => false,
            })
    }

    fn fenced_sessions(&self, _manifest: &Manifest) -> Result<Vec<String>> {
        use kuvo_peers::directory::RecordScope::Session;
        let mut sessions: std::collections::BTreeSet<_> = self
            .fences
            .ids()
            .filter(|id| id.starts_with("ses_"))
            .map(str::to_owned)
            .collect();
        if let Some(relay) = &self.relay {
            sessions.extend(relay.view().announced_sessions());
        }
        Ok(sessions
            .into_iter()
            .filter(|id| !self.can_recover(Session, id))
            .collect())
    }

    fn accepted_snapshot(&self) -> Result<Option<Value>> {
        Ok(self.accepted.clone())
    }

    fn authorize_restore(&self, manifest: &Manifest) -> Result<()> {
        if !self.paired && !manifest.paired {
            self.local.authorize_restore(manifest)?;
        } else {
            if manifest
                .origin
                .as_ref()
                .zip(self.local.origin.as_ref())
                .is_some_and(|(old, own)| old == own)
                || manifest
                    .signer
                    .as_ref()
                    .zip(self.local.signer.as_ref())
                    .is_some_and(|(old, own)| old == own)
            {
                return Err(Error::Message(
                    "Restore onto a replacement with fresh membership keys.",
                ));
            }
            if self.approval.get().is_none() {
                let wait = || {
                    Error::Message(
                        "Enroll the replacement in the current setup and wait for its accepted membership.",
                    )
                };
                let link: Link = self
                    .files
                    .read(LINK)
                    .map_err(setup_error)?
                    .ok_or_else(wait)?;
                let position = link.setup().map_err(|_| wait())?;
                let (Some(origin), Some(setup), Some(own)) =
                    (&manifest.origin, &manifest.setup, &self.local.origin)
                else {
                    return Err(wait());
                };
                let members = link.member_list().map_err(|_| wait())?;
                if position.position.id != *setup
                    || position.role != Role::Member
                    || !members.boxes.iter().any(|member| &member.key == own)
                    || members.boxes.iter().any(|member| &member.key == origin)
                {
                    return Err(wait());
                }
                let relay = self.relay.as_ref().ok_or_else(wait)?;
                let request = Approval {
                    source: manifest.source.clone(),
                    origin: origin.clone(),
                    setup: setup.clone(),
                };
                let reply = relay.approval(&position.position.primary, &request)?;
                if reply.status != 200 {
                    return Err(Error::Message(
                        "Confirm the replacement on the main computer.",
                    ));
                }
                let expected = wire_boundary(&link)?;
                if reply.value.get("boundary").and_then(Value::as_str) != Some(expected.as_str()) {
                    return Err(Error::Message(
                        "Wait for the main computer’s ownership and membership to arrive.",
                    ));
                }
                let _ = self.approval.set(());
            }
        }
        let current = Self::read(
            self.files.clone(),
            self.setup.clone(),
            self.roots.clone(),
            true,
        )?;
        if current.fingerprint != self.fingerprint || current.fences != self.fences {
            return Err(Error::Message(
                "Ownership changed during restore. Verify the replacement again.",
            ));
        }
        Ok(())
    }

    fn confirm_capture(&self) -> Result<()> {
        self.check_capture()
    }
}

fn setup_error(error: crate::Error) -> Error {
    Error::Detail(error.to_string())
}

pub(super) fn assignments(link: &Link) -> serde_json::Map<String, Value> {
    let mut ownership = serde_json::Map::new();
    for kind in [
        SharedKind::OwnershipAssignments,
        SharedKind::OwnershipRecovery,
    ] {
        if let Some(records) = link.value(kind).and_then(Value::as_object) {
            ownership.extend(records.clone());
        }
    }
    ownership
}

pub(crate) fn match_replacement(
    link: &Link,
    own: &str,
    source: &Source,
    previous: &str,
    replacement: &str,
) -> Result<Option<Value>> {
    if previous == replacement
        || !super::url_token(previous, 43)
        || !super::url_token(replacement, 43)
    {
        return Err(Error::Message(
            "Choose the old computer and its new membership.",
        ));
    }
    let roster = link.member_list().map_err(setup_error)?;
    let live = |key: &str| roster.boxes.iter().any(|peer| peer.key == key);
    if !roster.left.iter().any(|key| key == previous)
        || live(previous)
        || !live(replacement)
        || own == replacement
    {
        return Err(Error::Message(
            "On the main computer, remove the old membership and add the replacement with fresh keys first.",
        ));
    }
    if !permits_replacement(&roster, previous, replacement) {
        return Err(Error::Message(
            "The selected replacement conflicts with the setup’s computer history.",
        ));
    }
    let key = format!("replacement:{previous}");
    let choice = json!({ "computer": replacement, "source": source });
    if assignments(link).get(&key) == Some(&choice) {
        return Ok(None);
    }
    let mut recovery = link
        .value(SharedKind::OwnershipRecovery)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    recovery.insert(key, choice);
    Ok(Some(Value::Object(recovery)))
}

fn permits_replacement(
    roster: &crate::model::Membership,
    previous: &str,
    replacement: &str,
) -> bool {
    let Some(slots) = roster
        .extra
        .get("computerHistory")
        .and_then(|history| history.get("slots"))
        .and_then(Value::as_array)
    else {
        return true;
    };
    let includes = |previous_list: &[Value]| {
        previous_list
            .iter()
            .any(|key| key.as_str() == Some(previous))
    };
    slots.iter().all(|slot| {
        let id = slot.get("key").and_then(Value::as_str);
        let list = slot
            .get("previous")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        if id == Some(replacement) {
            list.is_empty() || includes(list)
        } else {
            !includes(list)
        }
    })
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct Approval {
    pub source: Source,
    pub origin: String,
    pub setup: String,
}

pub(crate) fn proof(link: &Link, sender: &str, request: &Approval) -> Result<String> {
    let refused = || Error::Message("The main computer has no current match for this replacement.");
    let position = link.setup().map_err(|_| refused())?;
    let roster = link.member_list().map_err(|_| refused())?;
    let active = |member: &str| roster.boxes.iter().any(|peer| peer.key == member);
    if position.position.id != request.setup
        || !super::url_token(&request.origin, 43)
        || !active(sender)
        || active(&request.origin)
        || !roster.left.contains(&request.origin)
        || !permits_replacement(&roster, &request.origin, sender)
    {
        return Err(refused());
    }
    let ownership = assignments(link);
    let choice = ownership
        .get(&format!("replacement:{}", request.origin))
        .ok_or_else(refused)?;
    if choice.get("computer").and_then(Value::as_str) != Some(sender)
        || choice.get("source") != Some(&serde_json::to_value(&request.source)?)
    {
        return Err(refused());
    }
    wire_boundary(link)
}

fn wire_boundary(link: &Link) -> Result<String> {
    let setup = link.setup().map_err(setup_error)?;
    let mut roster = link.member_list().map_err(setup_error)?;
    if let Some(history) = roster
        .extra
        .get_mut("computerHistory")
        .and_then(Value::as_object_mut)
    {
        history.shift_remove("retiredNames");
    }
    let ownership = link.value(SharedKind::OwnershipRecovery).map_or_else(
        || {
            link.value(SharedKind::OwnershipAssignments)
                .cloned()
                .unwrap_or(Value::Null)
        },
        |_| Value::Object(assignments(link)),
    );
    Ok(crate::model::fingerprint(&json!({
        "assignments": ownership, "membership": roster, "epoch": setup.position.epoch,
    })))
}
