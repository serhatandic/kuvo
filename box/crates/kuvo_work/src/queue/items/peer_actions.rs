use super::*;
use crate::queue::{forward::records as forward_core, peer_reads::QueueTarget};

// Methods for the queue Writer. Both the client entry points and the receiver use the same local
// operations. Only the former may forward.

impl Writer {
    pub(super) async fn change_for_client(
        &self,
        reference: String,
        removal: Removal,
        local: bool,
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
                .forward_change(target, method, action, None, local)
                .await;
        }
        self.change(reference, removal).await
    }

    pub(super) async fn edit_for_client(
        &self,
        reference: String,
        fields: Value,
        local: bool,
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
                    local,
                )
                .await;
        }
        self.edit(reference, fields, local).await
    }

    async fn forward_change(
        &self,
        target: QueueTarget,
        method: forward_core::Method,
        action: Option<String>,
        body: Option<Value>,
        local: bool,
    ) -> Result<Value, Refusal> {
        if !local {
            return Err(Refusal::json(
                StatusCode::CONFLICT,
                json!({"message":format!("Update {} before forwarding this operation from a paired device.", target.computer["name"].as_str().unwrap_or_default()),"computer":target.computer}),
            ));
        }
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
        let relation = body
            .as_ref()
            .is_some_and(|body| body.get("itemRelation").is_some());
        let deleted = matches!(method, forward_core::Method::Delete);
        let mut result = self
            .forward
            .call(
                target.key,
                target.computer.clone(),
                target.id.clone(),
                method,
                action,
                body,
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
            RequestKind::LabelRestore => {
                let value = act.body.unwrap_or_default()["value"].clone();
                if value.get("dryRun").is_some_and(|given| given != false) {
                    Err(bad("Update the box and retry the preview."))
                } else {
                    self.reads.restore_labels.here(value, false).await
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
