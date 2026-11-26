use super::*;
use crate::queue::{
    forward::{records as forward_core, scoped},
    peer_reads::QueueTarget,
};

// Methods for the queue Writer. Both the client entry points and the receiver use the same local
// operations. Only the former may forward.

/// Who asked for a change.
pub(in crate::queue) enum Client {
    /// The app on this computer.
    Local,
    /// A paired device, with its grant for a change kept elsewhere (none when the operation it
    /// named can't be an operation id).
    Paired(Option<scoped::Device>),
    /// Anyone else: a change kept elsewhere is made there.
    Other,
}

impl Writer {
    /// Who `caller` is, for a change it asks of an item: its grant from what it sent, where it's
    /// a paired device.
    pub(in crate::queue) fn client(
        &self,
        caller: &kuvo_server::Caller,
        headers: &http::HeaderMap,
        body: Option<&Value>,
    ) -> Client {
        if local_work_queue_app(caller) {
            return Client::Local;
        }
        let Some(pairing) = caller.pairing().filter(|pairing| pairing.is_current()) else {
            return Client::Other;
        };
        let grant = self.setup.queue_setup().zip(scoped::operation(headers, body).ok());
        Client::Paired(grant.map(|(setup, operation)| scoped::Device {
            setup: setup.id,
            key: pairing.key,
            pairing: pairing.id.clone(),
            operation,
        }))
    }

    pub(super) async fn change_for_client(
        &self,
        reference: String,
        removal: Removal,
        client: Client,
    ) -> Result<Value, Refusal> {
        let restoring = matches!(removal, Removal::Restore);
        if let Some(target) = self
            .reads
            .forward_target(reference.clone(), restoring)
            .await?
        {
            let method = if restoring {
                forward_core::Method::Post
            } else {
                forward_core::Method::Delete
            };
            let action = restoring.then(|| "restore".to_owned());
            return self
                .forward_change(target, method, action, None, client)
                .await;
        }
        self.change(reference, removal).await
    }

    pub(super) async fn edit_for_client(
        &self,
        reference: String,
        fields: Value,
        client: Client,
    ) -> Result<Value, Refusal> {
        if let Some(target) = self.reads.forward_target(reference.clone(), false).await? {
            let allowed = [
                "title",
                "body",
                "status",
                "priority",
                "labels",
                "assignee",
                "due",
                "before",
                "after",
                "top",
                "files",
                "itemRelation",
            ];
            let fields = allowed
                .into_iter()
                .filter_map(|field| fields.get(field).map(|value| (field.into(), value.clone())))
                .collect::<Fields>();
            return self
                .forward_change(
                    target,
                    forward_core::Method::Patch,
                    None,
                    Some(fields.into()),
                    client,
                )
                .await;
        }
        self.edit(reference, fields, matches!(client, Client::Local)).await
    }

    async fn forward_change(
        &self,
        target: QueueTarget,
        method: forward_core::Method,
        action: Option<String>,
        mut body: Option<Value>,
        client: Client,
    ) -> Result<Value, Refusal> {
        let device = match client {
            Client::Local => None,
            Client::Paired(Some(device)) => Some(device),
            Client::Paired(None) => {
                return Err(Refusal::json(
                    StatusCode::CONFLICT,
                    json!({"message":scoped::NO_GRANT,"computer":target.computer}),
                ));
            }
            Client::Other => {
                return Err(Refusal::json(
                    StatusCode::CONFLICT,
                    json!({"message":format!("Update {} before forwarding this operation from a paired device.", target.computer["name"].as_str().unwrap_or_default()),"computer":target.computer}),
                ));
            }
        };
        if body
            .as_ref()
            .and_then(|body| body["files"].as_array())
            .is_some_and(|files| !files.is_empty())
            && !self
                .peers
                .view()
                .advertises(&target.key, "work-queue.attachments")
        {
            return Err(Refusal::json(
                StatusCode::CONFLICT,
                json!({"message":format!("{} needs an update before it can keep attached files. Update it and try again.", target.computer["name"].as_str().unwrap_or_default()),"computer":target.computer}),
            ));
        }
        if let Some(fields) = body.as_mut()
            && fields["files"]
                .as_array()
                .is_some_and(|files| !files.is_empty())
        {
            let given = fields["files"].clone();
            let name = target.computer["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let files = self.files.clone();
            match tokio::task::spawn_blocking(move || files.for_peer(&given, &name))
                .await
                .map_err(internal)?
            {
                Ok(carried) => fields["files"] = carried,
                Err(error) => {
                    use http_body_util::BodyExt;
                    let reply = error.reply();
                    let bytes = reply.body.collect().await.map_err(internal)?.to_bytes();
                    let mut value: Value = serde_json::from_slice(&bytes).map_err(internal)?;
                    value["computer"] = target.computer;
                    return Err(Refusal::json(reply.status, value));
                }
            }
        }
        let relation = body
            .as_ref()
            .is_some_and(|body| body.get("itemRelation").is_some());
        let deleted = matches!(method, forward_core::Method::Delete);
        let mut result = self
            .forward
            .call(
                &target,
                tools::item_name(&target.previous),
                method,
                action,
                body,
                device,
            )
            .await?;
        if relation && result["itemRelationApplied"] != true {
            return Err(Refusal::json(
                StatusCode::CONFLICT,
                json!({"message":format!("Update Kuvo on {} to link items there, then try again.", target.computer["name"].as_str().unwrap_or_default()),"computer":target.computer}),
            ));
        }
        if result["item"]["id"] == target.id {
            let store = self.store.clone();
            let item = result["item"].clone();
            let key = target.key;
            let previous = target.previous;
            let _ = tokio::task::spawn_blocking(move || {
                told::apply_reply(&store, &key, &item, deleted, &previous)
            })
            .await;
        }
        if result["item"].is_object() {
            result["item"]["computer"] = target.computer;
            if result["item"]["can"].is_object() {
                result["item"]["can"]["assign"] = false.into();
            }
        }
        Ok(result)
    }

    /// A paired device's change to one of this computer's items, sent on by `peer` with the
    /// device's grant (TypeScript `takeScopedAct`). Decided once for its operation and kept, so a
    /// retry is answered the same. None: it isn't admitted here (the item isn't here, or the
    /// device's access is being checked), so nothing is answered and nothing kept.
    pub(super) async fn respond_to_scoped(
        &self,
        peer: &str,
        given: scoped::Given,
        now: i64,
    ) -> Result<Option<forward_core::Reply>, Error> {
        let here = self.peers.view().roster.here.map(|key| kuvo_wire::keys::encode(&key));
        let setup = self.setup.queue_setup().map(|setup| setup.id);
        if setup.as_deref() != Some(given.grant.setup.as_str())
            || here.as_deref() != Some(given.owner.as_str())
            || given.act.area.is_some()
            || !store::item_id(&given.act.item)
        {
            return Ok(None);
        }
        async fn off<T: Send + 'static>(
            work: impl FnOnce() -> Result<T, Error> + Send + 'static,
        ) -> Result<T, Error> {
            tokio::task::spawn_blocking(work).await.map_err(io::Error::other)?
        }
        let store = self.store.clone();
        let id = given.act.item.clone();
        let item = off(move || Ok(store.items()?.into_iter().find(|item| item.id == id))).await?;
        let Some(item) = item.filter(|item| !item.saved.contains_key("handing")) else {
            return Ok(None);
        };
        let run = self.store.root.parent().expect("Work Queue under run").to_owned();
        let ledger = std::sync::Arc::new(scoped::Ledger::new(&run, &given, &item.id, &item.owner));
        let kept = ledger.clone();
        if let scoped::Earlier::Answer(answer) = off(move || Ok(kept.earlier()?)).await? {
            return Ok(Some(answer));
        }
        let (key, pairing) = given.grant.device().expect("a read grant names its device");
        let veto = match self.forward.pairing_state(&key, pairing) {
            _ if given.replay_only => Some(scoped::refused(scoped::REVOKED)),
            ownership::PairingState::Revoked => Some(scoped::refused(scoped::REVOKED)),
            ownership::PairingState::Checking => return Ok(None),
            ownership::PairingState::Allowed => None,
        };
        if veto.is_none() && self.admit(&item.owner).is_err() {
            return Ok(None);
        }
        if let Some(veto) = veto.or_else(|| given.expired(now).then(scoped::expired)) {
            let kept = ledger.clone();
            let answer = veto.clone();
            off(move || Ok(kept.decide(&answer, now)?)).await?;
            return Ok(Some(veto));
        }
        let kept = ledger.clone();
        off(move || Ok(kept.claim()?)).await?;
        let answer = self.respond_to_act(peer, given.act, now).await?;
        let decided = answer.clone();
        off(move || Ok(ledger.decide(&decided, now)?)).await?;
        Ok(Some(answer))
    }

    pub(super) async fn respond_to_act(
        &self,
        peer: &str,
        act: forward_core::Act,
        now: i64,
    ) -> Result<forward_core::Reply, Error> {
        use forward_core::{Reply, RequestKind};
        use http_body_util::BodyExt;
        let Some(kind) = act.kind() else {
            return Ok(Reply {
                status: 400,
                value: json!({"message":"That isn't a Work Queue operation."}),
            });
        };
        // Hosts can use the legacy tracked GET while a Rust client uses the shared quiet reader.
        let read = match kind {
            RequestKind::Read => Some(kuvo_peers::memory_read::Kind::Record),
            RequestKind::File(name) => Some(kuvo_peers::memory_read::Kind::File {
                name: name.to_owned(),
            }),
            _ => None,
        };
        if !act.current(now) {
            let view = self.peers.view();
            let here = &view.roster.name;
            let there = kuvo_wire::keys::decode_exact(peer)
                .and_then(|key| view.roster.member(&key))
                .map_or("", |peer| peer.name.as_str());
            return Ok(Reply {
                status: 409,
                value: json!({"message":format!("{here}'s clock and {there}'s are too far apart to take it: check both computers' time.")}),
            });
        }
        if matches!(kind, RequestKind::OfflineRead) {
            return self.offline_read(act.body.unwrap_or_default(), peer).await;
        }
        if let Some(kind) = read {
            let result = self
                .reads
                .answer(kuvo_peers::memory_read::Read {
                    area: None,
                    item: act.item,
                    kind,
                    body: None,
                })
                .await;
            return Ok(Reply {
                status: result.status,
                value: result.value,
            });
        }
        let store = self.store.clone();
        let id = act.id.clone();
        let sender = peer.to_owned();
        let at = act.at;
        let action_for_receipt = act.receipt();
        if let Some(before) = tokio::task::spawn_blocking(move || {
            forward_core::receipt(&store, &sender, &action_for_receipt, now, None)
        })
        .await
        .map_err(io::Error::other)??
        {
            let here = self.peers.view().roster.name.clone();
            if let Some(answer) = before.answer {
                return Ok(answer);
            }
            if !matches!(kind, RequestKind::Agent) || !id.starts_with("wqa_tool_") {
                return Ok(Reply {
                    status: 409,
                    value: json!({"message":format!("{here} stopped while it was doing this, so it can't tell whether it was done. Look again in a moment.")}),
                });
            }
        }
        let result = match kind {
            RequestKind::Edit => {
                self.edit(act.item, act.body.unwrap_or_default(), false)
                    .await
            }
            RequestKind::Agent => Ok(self.peer_tool(act).await),
            RequestKind::LabelEdit => labels::peer_edit(
                &self.store,
                &self.setup,
                act.body.unwrap_or_default()["value"].clone(),
            )
            .await
            .inspect(|_| self.retell.again()),
            RequestKind::LabelRestore => {
                let value = act.body.unwrap_or_default()["value"].clone();
                if value.get("dryRun").is_some_and(|given| given != false) {
                    Err(bad("Update the box and retry the preview."))
                } else {
                    self.reads.restore_labels.here(value, false).await
                }
            }
            RequestKind::Archive { session, action } => {
                let owned = self
                    .owners
                    .scope(session, None)
                    .is_ok_and(|scope| !scope.held && scope.owner.is_some_and(|owner| owner.local));
                if !owned {
                    Err(Refusal::new(
                        StatusCode::CONFLICT,
                        "This computer couldn’t confirm it owns that record.",
                    ))
                } else if let Some((settings, dispatch)) = self
                    .archives
                    .get()
                    .and_then(|(settings, dispatch)| Some((settings, dispatch.upgrade()?)))
                {
                    match act
                        .body
                        .as_ref()
                        .and_then(|body| body["value"]["was"].as_str())
                    {
                        Some(was) => {
                            settings
                                .owner_action(dispatch.clone(), session, action, was)
                                .await
                        }
                        None => Err(Refusal::new(StatusCode::FORBIDDEN, "")),
                    }
                } else {
                    Err(Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "The session list isn’t ready.",
                    ))
                }
            }
            RequestKind::Delete => self.change(act.item, Removal::Delete).await,
            RequestKind::Restore => self.change(act.item, Removal::Restore).await,
            _ => unreachable!("mutation already selected"),
        };
        let answer = match result {
            Ok(value) => Reply { status: 200, value },
            Err(error) => {
                let reply = error.reply();
                if reply.status == StatusCode::NOT_FOUND {
                    let here = self.peers.view().roster.name.clone();
                    Reply {
                        status: 404,
                        value: json!({"message":format!("It's no longer on {here}: it may have moved. Look again in a moment.")}),
                    }
                } else {
                    let data = reply
                        .body
                        .collect()
                        .await
                        .map_err(io::Error::other)?
                        .to_bytes();
                    let value = serde_json::from_slice(&data).map_err(io::Error::other)?;
                    Reply {
                        status: reply.status.as_u16(),
                        value,
                    }
                }
            }
        };
        let store = self.store.clone();
        let reply = answer.clone();
        let peer = peer.to_owned();
        let receipt_ref = forward_core::ReceiptRef { id, at };
        tokio::task::spawn_blocking(move || {
            forward_core::receipt(&store, &peer, &receipt_ref, now, Some(reply))
        })
        .await
        .map_err(io::Error::other)??;
        Ok(answer)
    }
}
