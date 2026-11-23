use std::time::Duration;

use kuvo_wire::keys::{decode_exact, encode};
use serde_json::{Value, json};
use tokio::{sync::oneshot, time::Instant};

use super::{Owner, Response};
use crate::{
    Error, edits,
    lease::Action,
    model::{Role, safe_token},
    random,
};

pub(super) enum HandoverReply {
    Local(oneshot::Sender<Result<Response, Error>>),
    Member { key: String, request: String },
}

pub(super) struct Handover {
    target: String,
    epoch: u64,
    token: String,
    expires: Instant,
    submitter: edits::Submitter,
    reply: HandoverReply,
}

impl Owner {
    pub(super) async fn receive_authority(
        &mut self,
        from: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let setup = self.link.setup()?;
        if value.get("id").and_then(Value::as_str) != Some(&setup.position.id) {
            return Ok(());
        }
        let epoch = value.get("epoch").and_then(Value::as_u64);
        match value.get("kind").and_then(Value::as_str) {
            Some("available" | "relay-leadership") if setup.role == Role::Primary => {
                if let Some(peer) = self.host(from) {
                    let tip = epoch != Some(setup.position.epoch)
                        || value.get("wantsTip") == Some(&Value::Bool(true));
                    self.publish_to(peer, tip).await?;
                }
            }
            Some("handover-request")
                if setup.role == Role::Primary && epoch == Some(setup.position.epoch) =>
            {
                let request = value
                    .get("token")
                    .and_then(Value::as_str)
                    .filter(|id| safe_token(id, 22, 22));
                let target = value
                    .get("primary")
                    .and_then(Value::as_str)
                    .filter(|key| decode_exact::<32>(key).is_some());
                let submitter = value
                    .get("submitter")
                    .and_then(|value| serde_json::from_value(value.clone()).ok());
                if let (Some(request), Some(target), Some(submitter)) = (request, target, submitter)
                {
                    self.begin_handover(
                        target.into(),
                        epoch,
                        submitter,
                        HandoverReply::Member {
                            key: from.into(),
                            request: request.into(),
                        },
                    )
                    .await;
                }
            }
            Some("handover-reply") if setup.role == Role::Member => {
                if self.forwarded_handover.as_ref().is_some_and(|pending| {
                    pending.target == from
                        && value.get("token").and_then(Value::as_str) == Some(&pending.token)
                        && value.get("proposedEpoch").and_then(Value::as_u64) == Some(pending.epoch)
                }) && let Some(pending) = self.forwarded_handover.take()
                {
                    let status = value
                        .get("status")
                        .and_then(Value::as_u64)
                        .and_then(|status| u16::try_from(status).ok());
                    let response = match status {
                        Some(status @ (200 | 202 | 403 | 404 | 409 | 503)) => Ok(Response {
                            status,
                            value: value["value"].clone(),
                        }),
                        _ => Err(Error::Conflict),
                    };
                    self.answer_handover(pending.reply, pending.epoch, response)
                        .await;
                }
            }
            Some("handover-ready")
                if setup.role == Role::Member
                    && setup.position.primary == from
                    && epoch == Some(setup.position.epoch)
                    && !self.provisional(&self.own()?.public)
                    && !self.link.disabled(&self.roster()?, &self.own()?.public)
                    && self.link.leadership.contains_key(&setup.position.epoch) =>
            {
                self.send_authority(from, json!({"kind": "handover-confirm", "token": value["token"], "sign": self.own()?.sign})).await;
            }
            Some("handover-confirm")
                if setup.role == Role::Primary && epoch == Some(setup.position.epoch) =>
            {
                let sign = self
                    .roster()?
                    .boxes
                    .into_iter()
                    .find(|peer| peer.key == from)
                    .and_then(|peer| peer.sign);
                if self.pending_handover.as_ref().is_some_and(|pending| {
                    pending.target == from
                        && value.get("token").and_then(Value::as_str) == Some(&pending.token)
                        && sign.as_deref() == value.get("sign").and_then(Value::as_str)
                        && pending.expires > Instant::now()
                }) && let Some(pending) = self.pending_handover.take()
                {
                    let result = if pending.submitter.current(&self.link) {
                        self.finish_handover(&pending.target, pending.epoch).await
                    } else {
                        Ok(handover_denied())
                    };
                    self.answer_handover(pending.reply, pending.epoch, result)
                        .await;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) async fn expire_handover(&mut self) {
        let now = Instant::now();
        let due: Vec<_> = [&mut self.pending_handover, &mut self.forwarded_handover]
            .into_iter()
            .filter_map(|slot| {
                slot.as_ref()
                    .is_some_and(|pending| pending.expires <= now)
                    .then(|| slot.take())
                    .flatten()
            })
            .collect();
        for pending in due {
            self.answer_handover(pending.reply, pending.epoch, Ok(Response {
                status: 503, value: json!({"message": "The chosen computer hasn't caught up with the main computer yet."}),
            })).await;
        }
    }

    async fn answer_handover(
        &self,
        reply: HandoverReply,
        epoch: u64,
        response: Result<Response, Error>,
    ) {
        match reply {
            HandoverReply::Local(reply) => {
                let _ = reply.send(response);
            }
            HandoverReply::Member { key, request } => {
                let status = match response.as_ref() {
                    Ok(response) => response.status,
                    Err(Error::Conflict) => 409,
                    Err(_) => 503,
                };
                let body = response.map_or_else(
                    |error| json!({"message": error.to_string()}),
                    |response| response.value,
                );
                self.send_authority(&key, json!({
                    "kind": "handover-reply", "token": request, "status": status, "value": body, "proposedEpoch": epoch,
                })).await;
            }
        }
    }

    pub(super) async fn begin_handover(
        &mut self,
        target: String,
        expected_epoch: Option<u64>,
        submitter: edits::Submitter,
        reply: HandoverReply,
    ) {
        let epoch = self.link.setup().map_or(0, |setup| setup.position.epoch);
        let result = self
            .prepare_handover(&target, expected_epoch, &submitter)
            .await;
        match result {
            Ok(HandoverStart::Answer(response)) => {
                self.answer_handover(reply, epoch, Ok(response)).await
            }
            Ok(HandoverStart::Check {
                peer,
                token,
                forwarded,
            }) => {
                let expires = Instant::now() + Duration::from_secs(if forwarded { 25 } else { 20 });
                let pending = Handover {
                    target: peer,
                    epoch,
                    token,
                    expires,
                    submitter,
                    reply,
                };
                if forwarded {
                    self.forwarded_handover = Some(pending);
                } else {
                    self.pending_handover = Some(pending);
                }
            }
            Err(error) => self.answer_handover(reply, epoch, Err(error)).await,
        }
    }

    async fn prepare_handover(
        &self,
        target: &str,
        expected_epoch: Option<u64>,
        submitter: &edits::Submitter,
    ) -> Result<HandoverStart, Error> {
        let setup = self.link.setup()?;
        if !submitter.current(&self.link) {
            return Ok(HandoverStart::Answer(handover_denied()));
        }
        if target == setup.position.primary {
            return Ok(HandoverStart::Answer(Response::ok(
                json!({"primary": target, "epoch": setup.position.epoch}),
            )));
        }
        if expected_epoch.is_some_and(|epoch| epoch != setup.position.epoch) {
            return Err(Error::Conflict);
        }
        let token = encode(&random::<16>()?);
        let forwarded = setup.role == Role::Member;
        let peer = if forwarded {
            if self.forwarded_handover.is_some() {
                return Err(Error::Conflict);
            }
            let primary = &setup.position.primary;
            if !self.send_authority(primary, json!({"kind": "handover-request", "token": token, "primary": target, "submitter": submitter})).await {
                return Err(Error::Fenced);
            }
            primary.clone()
        } else {
            self.can_write()?;
            if self.pending_handover.is_some() {
                return Err(Error::Conflict);
            }
            if self.provisional(target)
                || !self
                    .roster()?
                    .boxes
                    .iter()
                    .any(|peer| peer.key == target && peer.sign.is_some())
            {
                return Ok(HandoverStart::Answer(Response {
                    status: 404,
                    value: json!({"message": "Choose a member of this setup."}),
                }));
            }
            if !self
                .send_authority(target, json!({"kind": "handover-ready", "token": token}))
                .await
            {
                return Err(Error::Fenced);
            }
            target.to_owned()
        };
        Ok(HandoverStart::Check {
            peer,
            token,
            forwarded,
        })
    }

    async fn finish_handover(&mut self, target: &str, epoch: u64) -> Result<Response, Error> {
        self.can_write()?;
        let setup = self.link.setup()?;
        if epoch != setup.position.epoch {
            return Err(Error::Conflict);
        }
        let own = self.identity.as_ref().ok_or(Error::Conflict)?;
        let term = self.lease.seen.as_ref().ok_or(Error::Conflict)?.term;
        let result = self
            .lease
            .request(
                &self.files,
                &self.link,
                own,
                Action::Transfer,
                term,
                Some(target),
            )
            .await?;
        if result.is_some_and(|(lease, _)| lease.holder == target) {
            self.next = Instant::now();
            self.refresh();
            return Ok(Response {
                status: 202,
                value: json!({"primary": target, "epoch": setup.position.epoch + 1}),
            });
        }
        Err(Error::Fenced)
    }
}

enum HandoverStart {
    Answer(Response),
    Check {
        peer: String,
        token: String,
        forwarded: bool,
    },
}

fn handover_denied() -> Response {
    Response {
        status: 403,
        value: json!({"message": "Use a client paired with this setup to change its leadership."}),
    }
}
