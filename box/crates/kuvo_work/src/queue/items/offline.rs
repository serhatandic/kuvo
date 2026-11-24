//! Edit a queue item from a saved revision and deliver drafts to its current keeper.

mod apply;
mod history;
mod outbox;
mod types;

use super::*;
use crate::queue::forward::records::Reply;
pub(super) use history::Revisions;
use kuvo_peers::ownership::PairingState;
use kuvo_server::Caller;
use kuvo_setup::{RecordEditAnswer, RecordEditCall, RecordEditFuture, RecordEdits};
use outbox::{Outbox as OwnedOutbox, Staged as StageResult};
use std::collections::{HashMap, HashSet};
use types::{Edit, Envelope as Draft, Submitter, Target};

const FEATURE: &str = "data.offline-edits";
const AUTHORITY: &str = "data.queued-device-authority";

pub(super) struct Routes(pub(super) Writer);

impl RecordEdits for Routes {
    fn perform(&self, caller: Caller, call: RecordEditCall) -> RecordEditFuture {
        let writer = self.0.clone();
        Box::pin(async move {
            use RecordEditCall::*;
            let outbox = OwnedOutbox::new(&writer.store);
            let answer = match call {
                Read(query) if query["kind"] == "queueItems" => {
                    let target = serde_json::from_value::<Target>(query)
                        .ok()
                        .filter(Target::valid)
                        .ok_or_else(|| bad("Name the record to edit."))?;
                    writer.read_edit(target, outbox).await?
                }
                Submit(body) if body["ref"]["kind"] == "queueItems" => {
                    let edit = Edit::parse(body).ok_or_else(|| bad("Send a supported edit with its operation id, record address and base revision (inline attachments only)."))?;
                    writer
                        .submit_edit(edit, Submitter::of(&caller), outbox)
                        .await?
                }
                Outbox => {
                    let setup = writer
                        .setup
                        .queue_setup()
                        .map_or_else(String::new, |setup| setup.id);
                    let entries = blocking(move || outbox.entries(&setup))
                        .await?
                        .iter()
                        .filter_map(|entry| entry.listed())
                        .collect::<Vec<_>>();
                    RecordEditAnswer {
                        status: 200,
                        value: json!({"entries":entries}),
                    }
                }
                Receipt(operation) => {
                    let setup = writer
                        .setup
                        .queue_setup()
                        .map_or_else(String::new, |setup| setup.id);
                    let result = blocking(move || outbox.read(&operation)).await?;
                    let Some(entry) = result.filter(|entry| entry.setup == setup) else {
                        return Ok(None);
                    };
                    RecordEditAnswer {
                        status: 200,
                        value: entry.recovered(),
                    }
                }
                _ => return Ok(None),
            };
            Ok(Some(answer))
        })
    }
}

#[derive(Clone)]
struct Identity {
    setup: String,
    here: String,
}

struct Location {
    key: String,
    peer: Option<[u8; 32]>,
    computer: Value,
}

impl Writer {
    pub(super) async fn ordinary_edit(
        &self,
        caller: &Caller,
        headers: &HeaderMap,
        id: &str,
        mut body: Value,
        create: bool,
        delete: bool,
    ) -> Result<RecordEditAnswer, Refusal> {
        let operation = nonce(headers, &body)?.to_owned();
        let target = json!({"kind":"queueItems","id":id,"queue":body["owner"]});
        let computer = body.get("computer").cloned();
        body["ref"] = target;
        if let Some(computer) = computer {
            body["ref"]["computer"] = computer;
        }
        body["operation"] = operation.into();
        if create {
            body["create"] = true.into();
        }
        if delete {
            body["delete"] = true.into();
        }
        let edit = Edit::parse(body).ok_or_else(|| bad("Send a supported edit with its operation id, record address and base revision (inline attachments only)."))?;
        self.submit_edit(edit, Submitter::of(caller), OwnedOutbox::new(&self.store))
            .await
    }

    fn identity(&self) -> Option<Identity> {
        Some(Identity {
            setup: self.setup.queue_setup()?.id,
            here: kuvo_wire::keys::encode(&self.peers.view().roster.here?),
        })
    }

    fn owns_editor(&self, target: &Target, here: &str, item: Option<&Item>) -> bool {
        let queue = item.map_or(target.queue.as_str(), |item| item.owner.as_str());
        (queue != "usr_you"
            || item.is_some()
            || target.computer.as_deref().is_none_or(|key| key == here))
            && item.is_none_or(|item| !item.saved.contains_key("handing"))
            && self.admit(queue).is_ok()
    }

    async fn edit_location(
        &self,
        target: Target,
        here: String,
    ) -> Result<Option<Location>, Refusal> {
        let writer = self.clone();
        blocking(move || {
            let all = writer.store.items()?;
            let view = writer.peers.view();
            let from_key = |key| {
                let computer = view.roster.member(&key)?;
                Some(Location {
                    key: kuvo_wire::keys::encode(&key),
                    peer: Some(key), computer: json!({"key":kuvo_wire::keys::encode(&key),"name":computer.name,"kind":computer.kind}),
                })
            };
            if let Some(item) = all.iter().find(|item| item.id == target.id) {
                if writer.owns_editor(&target, &here, Some(item)) {
                    return Ok(Some(Location { key: here, peer: None, computer: Value::Null }));
                }
            }
            if let Some(known) = writer.reads.known_owner(&all, &target.id)? {
                return Ok(Some(Location { key: kuvo_wire::keys::encode(&known.key), peer: Some(known.key), computer: known.computer }));
            }
            if let Some(key) = target.computer.as_deref().filter(|key| *key != here).and_then(kuvo_wire::keys::decode_exact) {
                return Ok(from_key(key));
            }
            let remote = if target.queue.starts_with("ses_") { Some(RecordScope::Session) }
                else if target.queue.starts_with("grp_") { Some(RecordScope::Group) } else { None };
            if let Some(owner) = remote.and_then(|scope| view.owner_of(scope, &target.queue, writer.peers.now()))
                && !owner.local
            {
                return Ok(owner.key.and_then(from_key));
            }
            Ok(Some(Location { key: here, peer: None, computer: Value::Null }))
        }).await
    }

    async fn read_edit(
        &self,
        target: Target,
        outbox: OwnedOutbox,
    ) -> Result<RecordEditAnswer, Refusal> {
        let unknown = || {
            Refusal::new(
                StatusCode::CONFLICT,
                "This computer could not identify the owner.",
            )
        };
        let id = self.identity().ok_or_else(unknown)?;
        let location = self
            .edit_location(target.clone(), id.here.clone())
            .await?
            .ok_or_else(unknown)?;
        let Some(peer) = location.peer else {
            let mut answer = self.edit_view(target, id.clone()).await?;
            if answer.status == 200 {
                answer.value["owner"] = id.here.into();
                answer.value["stale"] = false.into();
            }
            return Ok(RecordEditAnswer {
                status: answer.status,
                value: answer.value,
            });
        };
        let cache = types::key(json!([id.setup, location.key, "queueItems", target.id]));
        if self.peers.view().advertises(&peer, FEATURE)
            && let Ok(mut found) = self
                .forward
                .read_edit(
                    peer,
                    location.computer,
                    json!({"ref":target,"setup":id.setup}),
                )
                .await
            && found["revision"].is_string()
            && self.identity().is_some_and(|now| now.setup == id.setup)
            && self
                .edit_location(target.clone(), id.here)
                .await?
                .is_some_and(|again| again.key == location.key)
        {
            let value = found.clone();
            blocking(move || outbox.cache(&cache, &value)).await?;
            found["owner"] = location.key.into();
            found["stale"] = false.into();
            return Ok(RecordEditAnswer {
                status: 200,
                value: found,
            });
        }
        if let Some(mut value) = blocking(move || outbox.snapshot(&cache)).await? {
            value["owner"] = location.key.into();
            value["stale"] = true.into();
            return Ok(RecordEditAnswer { status: 200, value });
        }
        Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "There is no saved revision here yet.",
        ))
    }

    async fn edit_view(&self, target: Target, id: Identity) -> Result<Reply, Refusal> {
        let _guard = self.lock().await?;
        let writer = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = _guard;
            let now = Timestamp::now().as_millisecond();
            let (all, compaction) = writer.changes(now)?;
            let item = all.iter().find(|item| item.id == target.id);
            let deletion = if item.is_none() {
                writer.fence(Fence::Deleted, &target.id)?
            } else {
                None
            };
            let target = fence_target(&target, deletion.as_ref());
            if !writer.owns_editor(&target, &id.here, item) {
                compaction.finish(&writer, now)?;
                return Ok(Reply {
                    status: 409,
                    value: json!({"message":"This computer could not confirm the current record."}),
                });
            }
            let value = if let Some(item) = item {
                let mut row = Value::Object(item.saved.clone());
                let history = writer.revisions.observe_item(&mut row, &item.id, None);
                row["offlineEdits"] = json!(history);
                if row.as_object() != Some(&item.saved) {
                    writer.save_edit_row(&item.owner, &item.id, row, now)?;
                }
                history.view()
            } else {
                writer
                    .revisions
                    .absent(&target.id, deletion.is_some())
                    .view()
            };
            compaction.finish(&writer, now)?;
            Ok(Reply { status: 200, value })
        })
        .await
        .map_err(internal)?
    }

    fn save_edit_row(&self, owner: &str, id: &str, row: Value, now: i64) -> Result<(), Refusal> {
        let mut document = self.read_owner(owner, now)?;
        if let Some(item) = document.items.iter_mut().find(|item| item["id"] == id) {
            *item = row;
        } else {
            document.items.push(row);
        }
        document.save(&self.store).map_err(storage_refusal)
    }

    async fn submit_edit(
        &self,
        edit: Edit,
        submitter: Submitter,
        outbox: OwnedOutbox,
    ) -> Result<RecordEditAnswer, Refusal> {
        let id = self.identity().ok_or_else(|| {
            Refusal::new(
                StatusCode::CONFLICT,
                "Join a setup before saving an offline edit.",
            )
        })?;
        let location = self
            .edit_location(edit.target.clone(), id.here.clone())
            .await?
            .ok_or_else(|| {
                Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The owner could not be identified or too many edits are waiting.",
                )
            })?;
        if self
            .identity()
            .is_none_or(|current| current.setup != id.setup || current.here != id.here)
        {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "The setup changed before this edit could be saved.",
            ));
        }
        let saved = outbox.clone();
        let setup = id.setup.clone();
        let here = id.here.clone();
        let owner = location.key.clone();
        let stage = blocking(move || saved.stage(&setup, &here, owner, edit, submitter)).await?;
        let StageResult::Saved(entry) = stage else {
            return Err(match stage {
                StageResult::Conflict => {
                    Refusal::new(StatusCode::CONFLICT, "That operation id is already in use.")
                }
                _ => Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The owner could not be identified or too many edits are waiting.",
                ),
            });
        };
        let mut receipt = entry.receipt;
        if location.peer.is_none()
            && entry.owner == id.here
            && let Some(draft) = entry.queued_edit
        {
            if let Some(applied) = self.apply_draft(draft, id.here.clone()).await? {
                let kept = applied.clone();
                blocking(move || outbox.finish(&id.setup, &id.here, kept)).await?;
                receipt = applied;
            }
        }
        Ok(RecordEditAnswer {
            status: types::status(&receipt),
            value: receipt,
        })
    }

    pub(super) async fn offline_read(&self, body: Value, sender: &str) -> Result<Reply, Error> {
        let Some(identity) = self.identity().filter(|id| body["setup"] == id.setup) else {
            return Ok(interrupted_read());
        };
        let Some(target) = serde_json::from_value::<Target>(body["ref"].clone())
            .ok()
            .filter(Target::valid)
        else {
            return Ok(Reply {
                status: 400,
                value: json!({"message":"Name the record and its setup."}),
            });
        };
        if !self.accepts_editor(&identity.setup, sender) {
            return Ok(interrupted_read());
        }
        Ok(self
            .edit_view(target, identity)
            .await
            .unwrap_or_else(|_| interrupted_read()))
    }

    fn accepts_editor(&self, setup: &str, sender: &str) -> bool {
        self.identity().is_some_and(|id| {
            id.setup == setup
                && (sender == id.here
                    || kuvo_wire::keys::decode_exact(sender)
                        .is_some_and(|key| self.forward.admits(key)))
        })
    }

    fn authority(&self, draft: &Draft) -> PairingState {
        if draft.replay_only == Some(true) {
            return PairingState::Revoked;
        }
        match &draft.submitter {
            Some(Submitter::Computer) => PairingState::Allowed,
            Some(Submitter::Paired { key, pairing }) => kuvo_wire::keys::decode_exact(key)
                .map_or(PairingState::Revoked, |key| {
                    self.forward.pairing_state(&key, pairing)
                }),
            None => PairingState::Revoked,
        }
    }

    pub(super) async fn retry_offline(
        &self,
        relay: Option<&RelayHandle>,
        now: i64,
        retry: &mut HashMap<String, i64>,
    ) -> Result<(), Error> {
        let Some(id) = self.identity() else {
            return Ok(());
        };
        let outbox = OwnedOutbox::new(&self.store);
        let saved = outbox.clone();
        let setup = id.setup.clone();
        let entries = tokio::task::spawn_blocking(move || saved.entries(&setup))
            .await
            .map_err(io::Error::other)??;
        let mut remaining = HashSet::new();
        for mut entry in entries {
            let Some(mut draft) = entry.queued_edit.take() else {
                continue;
            };
            let operation = draft.edit.operation.clone();
            remaining.insert(operation.clone());
            if retry.get(&operation).is_some_and(|last| now - last < 5_000) {
                continue;
            }
            let authority = self.authority(&draft);
            if matches!(authority, PairingState::Checking) {
                continue;
            }
            let location = self
                .edit_location(draft.edit.target.clone(), id.here.clone())
                .await
                .ok()
                .flatten();
            let Some(location) = location else { continue };
            if entry.owner != location.key {
                let saved = outbox.clone();
                let setup = id.setup.clone();
                let operation = operation.clone();
                let next = location.key.clone();
                if !tokio::task::spawn_blocking(move || {
                    saved.readdress(&setup, &operation, &entry.owner, &next)
                })
                .await
                .map_err(io::Error::other)??
                {
                    continue;
                }
            }
            if let Some(peer) = location.peer {
                let view = self.peers.view();
                let Some(relay) = relay else { continue };
                let Some(host) = view.roster.member(&peer).and_then(|member| member.host()) else {
                    continue;
                };
                if !self.forward.admits(peer)
                    || !view.advertises(&peer, FEATURE)
                    || !view.advertises(&peer, AUTHORITY)
                {
                    continue;
                }
                if !matches!(authority, PairingState::Allowed) {
                    draft.replay_only = Some(true);
                }
                if let Some(mails) =
                    kuvo_peers::work_queue_mails(json!({"scopedOfflineEdit":draft}), now)
                {
                    for mail in mails {
                        if relay.sender.mail(host.clone(), mail).await.is_err() {
                            break;
                        }
                    }
                    retry.insert(operation, now);
                }
            } else {
                retry.insert(operation, now);
                if let Ok(Some(receipt)) = self.apply_draft(draft, id.here.clone()).await {
                    let setup = id.setup.clone();
                    let own = id.here.clone();
                    let saved = outbox.clone();
                    tokio::task::spawn_blocking(move || saved.finish(&setup, &own, receipt))
                        .await
                        .map_err(io::Error::other)??;
                }
            }
        }
        retry.retain(|operation, _| remaining.contains(operation));
        Ok(())
    }

    pub(super) async fn receive_offline(
        &self,
        relay: &RelayHandle,
        peer: &kuvo_wire::keys::Host,
        sender: &str,
        queue: &Value,
        now: i64,
    ) -> Result<(), Error> {
        if let Some(mut draft) = queue
            .get("scopedOfflineEdit")
            .filter(|_| queue.get("offlineEdit").is_none())
            .or_else(|| {
                queue
                    .get("offlineEdit")
                    .filter(|_| queue.get("scopedOfflineEdit").is_none())
            })
            .filter(|raw| {
                raw.get("formatVersion")
                    .is_none_or(|version| matches!(version.as_f64(), Some(0.0 | 1.0)))
            })
            .filter(|raw| raw.to_string().len() <= types::MAX_BYTES + 512)
            .and_then(|raw| serde_json::from_value::<Draft>(raw.clone()).ok())
            .filter(|draft| draft.valid(sender) && self.accepts_editor(&draft.setup, sender))
        {
            if queue.get("offlineEdit").is_some() {
                draft.submitter = None;
            }
            let setup = draft.setup.clone();
            if let Ok(Some(receipt)) = self.apply_draft(draft, sender.into()).await
                && self.accepts_editor(&setup, sender)
                && let Some(mails) = kuvo_peers::work_queue_mails(
                    json!({"offlineEditReceipt":{"setup":setup,"receipt":receipt}}),
                    now,
                )
            {
                for mail in mails {
                    if relay.sender.mail(peer.clone(), mail).await.is_err() {
                        break;
                    }
                }
            }
        }
        if let Some(answer) = queue.get("offlineEditReceipt") {
            if let Some(setup) = answer["setup"]
                .as_str()
                .filter(|setup| self.accepts_editor(setup, sender))
                && valid_receipt(&answer["receipt"])
            {
                let setup = setup.to_owned();
                let sender = sender.to_owned();
                let receipt = answer["receipt"].clone();
                let outbox = OwnedOutbox::new(&self.store);
                tokio::task::spawn_blocking(move || outbox.finish(&setup, &sender, receipt))
                    .await
                    .map_err(io::Error::other)??;
            }
        }
        Ok(())
    }
}

fn fence_target(target: &Target, deletion: Option<&Fields>) -> Target {
    let mut target = target.clone();
    if let Some(queue) = deletion
        .and_then(|record| record.get("queue"))
        .and_then(Value::as_str)
        .filter(|queue| owner_id(queue))
    {
        target.queue = queue.to_owned();
    }
    target
}

pub(super) fn reply<R>(answer: RecordEditAnswer) -> Result<kuvo_server::RouteReply<R>, Refusal>
where
    R: kuvo_protocol::JsonStatus<200> + kuvo_protocol::JsonStatus<202>,
    <R as kuvo_protocol::JsonStatus<200>>::Body: serde::de::DeserializeOwned,
    <R as kuvo_protocol::JsonStatus<202>>::Body: serde::de::DeserializeOwned,
{
    match answer.status {
        200 => typed(answer.value),
        202 => {
            let body = serde_json::from_value(answer.value).map_err(internal)?;
            Ok(kuvo_server::RouteReply::json::<202>(body))
        }
        status => Err(Refusal::json(
            StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            answer.value,
        )),
    }
}

fn valid_receipt(receipt: &Value) -> bool {
    let hash = |value: &Value| {
        value
            .as_str()
            .is_some_and(|id| kuvo_wire::keys::decode_exact::<32>(id).is_some())
    };
    if !hash(&receipt["operation"]) {
        return false;
    }
    match receipt["status"].as_str() {
        Some("interrupted" | "capacity") => receipt["message"].is_string(),
        Some("committed" | "choice" | "deleted" | "refresh") => {
            hash(&receipt["revision"])
                && receipt["choices"]
                    .as_array()
                    .is_some_and(|choices| choices.iter().all(hash))
        }
        _ => false,
    }
}

fn interrupted_read() -> Reply {
    Reply {
        status: 409,
        value: json!({"message":"This computer could not confirm the current record."}),
    }
}
