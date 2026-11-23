//! Add and unlink requests share the setup's authenticated forwarding mail.
//! The originating request alone holds the installer token. The main only sees its hash.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use hmac::{Hmac, Mac};
use kuvo_wire::{
    json::canonical,
    keys::{decode_exact, encode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::{sync::oneshot, task::JoinSet, time::Instant};

use super::{Command, Owner, Response, SetupHandle, Unlink};
use crate::{
    Error,
    edits::Submitter,
    installer::{self, Device},
    lease,
    model::{Position, Role, safe_token},
    random,
    seed::source::{Consent, Inventory, Prepared, Selection},
    ssh::Action,
    storage::Files,
};

const AREA: &str = "addComputer";
const CHANGED: &str = "The setup changed. Refresh it and try again.";
const NO_MAIN: &str = "This computer can't reach your main computer.";
const UNPAIRED: &str = "Pair this device before adding a computer.";
const RETRY: Duration = Duration::from_secs(5);

type Reply = oneshot::Sender<Result<Response, Error>>;

pub(crate) enum ComputerRequest {
    Checklist,
    Offer {
        invite: String,
        selection: Selection,
    },
    Ssh(Action),
    Unlink {
        key: String,
        request: Unlink,
    },
}

impl ComputerRequest {
    fn allowed_for(&self) -> Duration {
        Duration::from_secs(match self {
            Self::Checklist => 20,
            Self::Offer { .. } => 45,
            Self::Ssh(Action::Page) => 20,
            Self::Ssh(_) | Self::Unlink { .. } => 45,
        })
    }

    fn outgoing(&self, at: u64, id: String, position: &Position, by: Submitter) -> Outgoing {
        if let Self::Unlink { key, request } = self {
            let grant = matches!(by, Submitter::Paired { .. }).then(|| ForwardGrant {
                setup: position.id.clone(),
                operation: request.operation().unwrap_or(&id).to_owned(),
                submitter: by,
            });
            return Outgoing {
                act: Act {
                    id,
                    at,
                    area: "computers".into(),
                    method: "DELETE".into(),
                    item: key.clone(),
                    body: json!(request),
                },
                grant,
            };
        }
        let (method, item, input) = match self {
            Self::Checklist => ("GET", "credentials".into(), None),
            Self::Offer { invite, selection } => (
                "POST",
                "code".into(),
                Some(json!({
                    "invite": invite, "credentials": selection.ids(),
                })),
            ),
            Self::Ssh(action) => action.wire(),
            Self::Unlink { .. } => unreachable!(),
        };
        let actor = match by {
            Submitter::Computer => None,
            Submitter::Paired { key, pairing } => Some(Device { key, pairing }),
        };
        Outgoing {
            act: Act {
                id,
                at,
                area: AREA.into(),
                method: method.into(),
                item,
                body: json!(Scope {
                    setup: position.id.clone(),
                    epoch: position.epoch,
                    actor,
                    input
                }),
            },
            grant: None,
        }
    }

    fn mutates(&self) -> bool {
        match self {
            Self::Checklist => false,
            Self::Offer { .. } | Self::Unlink { .. } => true,
            Self::Ssh(action) => action.changes(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Act {
    id: String,
    at: u64,
    area: String,
    method: String,
    item: String,
    #[serde(default)]
    body: Value,
}

impl Act {
    fn valid_id(&self) -> bool {
        self.id
            .strip_prefix("wqa_")
            .is_some_and(|id| safe_token(id, 16, 40))
    }

    fn addition(&self, scope: &Scope) -> Option<ComputerRequest> {
        match (self.method.as_str(), self.item.as_str()) {
            ("GET", "credentials") => Some(ComputerRequest::Checklist),
            ("POST", "code") => {
                let given = scope.input.as_ref()?;
                Some(ComputerRequest::Offer {
                    invite: given["invite"]
                        .as_str()
                        .filter(|id| safe_token(id, 43, 43))?
                        .into(),
                    selection: Selection::read(given.get("credentials").cloned())?,
                })
            }
            _ => Action::read(&self.method, &self.item, scope.input.clone())
                .map(ComputerRequest::Ssh),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct ForwardGrant {
    setup: String,
    operation: String,
    submitter: Submitter,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopedAct {
    act: Act,
    grant: ForwardGrant,
    owner: String,
    #[serde(default)]
    replay_only: bool,
}

struct Outgoing {
    act: Act,
    grant: Option<ForwardGrant>,
}

impl Outgoing {
    fn envelope(&self, to: &str) -> Value {
        let act = &self.act;
        match &self.grant {
            Some(grant) => {
                json!({"scopedAct": {"act": act, "grant": grant, "owner": to, "replayOnly": false}, "at": act.at})
            }
            None => json!({"act": act, "at": act.at}),
        }
    }

    fn spool(&self, to: &str, key: &[u8; 32]) -> Value {
        let act = &self.act;
        match &self.grant {
            Some(grant) => {
                let mut mac =
                    Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key size");
                mac.update(b"kuvo-relay-scoped-spool-v1\0");
                mac.update(canonical(&json!([to, [act, grant]])).as_bytes());
                let seal = encode(&mac.finalize().into_bytes());
                json!({"formatVersion": 1, "to": to, "scopedAct": act, "grant": grant, "seal": seal})
            }
            None => json!({"formatVersion": 1, "to": to, "act": act}),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Scope {
    setup: String,
    epoch: u64,
    actor: Option<Device>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
}

impl Scope {
    fn by(&self) -> Submitter {
        match &self.actor {
            Some(actor) => Submitter::Paired {
                key: actor.key.clone(),
                pairing: actor.pairing.clone(),
            },
            None => Submitter::Computer,
        }
    }

    fn in_setup(&self, position: &Position) -> bool {
        self.setup == position.id && self.epoch == position.epoch
    }
}

struct Pending {
    position: Position,
    outgoing: Outgoing,
    deadline: Instant,
    next: Instant,
    reply: Reply,
    spool: Option<String>,
}

impl Pending {
    fn finish(self, files: &Files, result: Response) {
        if let Some(path) = self.spool {
            if let Err(error) = files.remove(&path) {
                eprintln!("clearing installer request: {error}");
            }
        }
        let _ = self.reply.send(Ok(result));
    }
}

#[derive(Default)]
pub(super) struct Forwarding {
    pub(super) jobs: JoinSet<Finished>,
    active: HashSet<(String, String)>,
    pending: HashMap<String, Pending>,
    completed: HashMap<(String, String), Cached>,
}

struct Cached {
    digest: String,
    expires: Instant,
    answer: Response,
}

enum Where {
    Local(Reply),
    Peer {
        from: String,
        request: String,
        cache: Option<String>,
    },
}

pub(super) struct Finished {
    destination: Where,
    position: Position,
    by: Submitter,
    result: PreparedRequest,
}

enum PreparedRequest {
    Checklist(Value),
    Ssh(Response),
    Offer {
        invite: String,
        seed: Option<Prepared>,
    },
    Failed(Response),
}

impl SetupHandle {
    pub(crate) async fn installer_on_main(
        &self,
        selection: Selection,
        by: Submitter,
    ) -> Result<Response, Error> {
        let token = encode(&random::<24>()?);
        let offer = self
            .request_command(Command::ComputerViaMain(
                ComputerRequest::Offer {
                    invite: installer::hash(&token),
                    selection,
                },
                by,
            ))
            .await?;
        installer::complete_offer(offer, token)
    }
}

impl Owner {
    pub(super) async fn begin_computer_request(
        &mut self,
        mut request: ComputerRequest,
        by: Submitter,
        reply: Reply,
    ) -> Result<(), Error> {
        let Ok(setup) = self.link.setup() else {
            let _ = reply.send(Ok(refused(409, CHANGED)));
            return Ok(());
        };
        if !by.current(&self.link) {
            let message = match request {
                ComputerRequest::Unlink { .. } => "The submitting device no longer has access.",
                _ => UNPAIRED,
            };
            let _ = reply.send(Ok(refused(403, message)));
            return Ok(());
        }
        let position = setup.position.clone();
        if setup.role == Role::Primary {
            if let ComputerRequest::Unlink { key, request } = request {
                let _ = reply.send(self.unlink_computer(key, request, by).await);
            } else if self.forwarding.jobs.len() >= 32 {
                let _ = reply.send(Ok(refused(503, "Too many installer requests are waiting.")));
            } else {
                self.prepare_add(request, position, by, Where::Local(reply));
            }
            return Ok(());
        }
        let Some(target) = self
            .host(&position.primary)
            .filter(|_| self.forwarding.pending.len() < 32)
        else {
            let _ = reply.send(Ok(refused(503, NO_MAIN)));
            return Ok(());
        };
        if let ComputerRequest::Unlink { key, request } = &mut request {
            let refused = if !request.valid() {
                Some(refused(400, "Send a valid removal request."))
            } else if decode_exact::<32>(key).is_none() {
                Some(refused(404, "That isn’t one of your computers."))
            } else if request.requests_wipe() {
                Some(refused(409, "Wipe preparation is unavailable."))
            } else if self.link.disabled(&self.roster()?, &self.own()?.public) {
                Some(refused(409, CHANGED))
            } else {
                None
            };
            if let Some(answer) = refused {
                let _ = reply.send(Ok(answer));
                return Ok(());
            }
            request.bind_operation()?;
        }
        let sent = Instant::now();
        let deadline = sent + request.allowed_for();
        let id = format!("wqa_{}", encode(&random::<16>()?));
        let outgoing = request.outgoing(lease::wall(), id.clone(), &position, by);
        let spool = if request.mutates() {
            let path = format!("run/work-queue/forward/{id}.json");
            if self
                .files
                .write(&path, &outgoing.spool(&position.primary, &self.own()?.key))
                .is_err()
            {
                let _ = reply.send(Ok(refused(503, NO_MAIN)));
                return Ok(());
            }
            Some(path)
        } else {
            None
        };
        self.send(&target, outgoing.envelope(&position.primary))
            .await;
        self.forwarding.pending.insert(
            id,
            Pending {
                position,
                outgoing,
                deadline,
                next: sent + RETRY,
                reply,
                spool,
            },
        );
        Ok(())
    }

    fn prepare_add(
        &mut self,
        request: ComputerRequest,
        position: Position,
        by: Submitter,
        destination: Where,
    ) {
        let files = self.files.clone();
        let ssh = self
            .ssh
            .borrow()
            .as_ref()
            .and_then(std::sync::Weak::upgrade);
        self.forwarding.jobs.spawn(async move {
            let result = match request {
                ComputerRequest::Checklist => {
                    PreparedRequest::Checklist(Inventory::read(&files).await.checklist())
                }
                ComputerRequest::Ssh(action) => match ssh {
                    Some(ssh) => PreparedRequest::Ssh(ssh.execute(action, by.clone()).await),
                    None => PreparedRequest::Failed(refused(
                        503,
                        "Kuvo’s local server hasn’t started yet. Try again in a moment.",
                    )),
                },
                ComputerRequest::Offer { invite, selection } if selection.empty() => {
                    PreparedRequest::Offer { invite, seed: None }
                }
                ComputerRequest::Offer { invite, selection } => {
                    let inventory = Inventory::read(&files).await;
                    match inventory.capture(&files, &selection) {
                        Ok(seed) => PreparedRequest::Offer {
                            invite,
                            seed: Some(Prepared {
                                digest: selection.digest(),
                                seed,
                            }),
                        },
                        Err((status, message)) => PreparedRequest::Failed(refused(status, message)),
                    }
                }
                ComputerRequest::Unlink { .. } => unreachable!("the setup owner commits unlink"),
            };
            Finished {
                destination,
                position,
                by,
                result,
            }
        });
    }

    pub(super) async fn receive_add_request(
        &mut self,
        from: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let Ok(act) = serde_json::from_value::<Act>(value.clone()) else {
            return Ok(());
        };
        if act.area != AREA || !act.valid_id() {
            return Ok(());
        }
        let Ok(scope) = serde_json::from_value::<Scope>(act.body.clone()) else {
            return Ok(());
        };
        let Some(request) = act.addition(&scope) else {
            return Ok(());
        };
        let now = lease::wall();
        if now.saturating_sub(act.at) > (request.allowed_for() + RETRY).as_millis() as u64
            || act.at > now + 15_000
            || self.forwarding.jobs.len() >= 32
        {
            return Ok(());
        }
        let by = scope.by();
        let Some(setup) = self
            .link
            .setup()
            .ok()
            .filter(|setup| setup.role == Role::Primary && scope.in_setup(&setup.position))
        else {
            self.add_answer(from, &act.id, refused(409, CHANGED)).await;
            return Ok(());
        };
        let position = setup.position.clone();
        if !by.current(&self.link) {
            self.add_answer(from, &act.id, refused(403, UNPAIRED)).await;
            return Ok(());
        }
        if let ComputerRequest::Offer { invite, selection } = &request
            && self.link.installer_invites.contains_key(invite)
        {
            let consent = if selection.empty() {
                Consent::None
            } else {
                Consent::Kept(selection.digest())
            };
            let answer = self
                .issue_installer_offer(invite.clone(), consent, by)
                .await?;
            self.add_answer(from, &act.id, answer).await;
            return Ok(());
        }
        let cache = if matches!(&request, ComputerRequest::Ssh(action) if action.changes()) {
            let wire = serde_json::to_string(&act).expect("forwarded SSH request serializes");
            let digest = installer::hash(&wire);
            if let Some(old) = self
                .forwarding
                .completed
                .get(&(from.into(), act.id.clone()))
            {
                let answer = if old.digest == digest {
                    old.answer.clone()
                } else {
                    refused(
                        409,
                        "That operation id is already bound to another change or owner.",
                    )
                };
                self.add_answer(from, &act.id, answer).await;
                return Ok(());
            }
            Some(digest)
        } else {
            None
        };
        if self.forwarding.active.insert((from.into(), act.id.clone())) {
            self.prepare_add(
                request,
                position,
                by,
                Where::Peer {
                    from: from.into(),
                    request: act.id,
                    cache,
                },
            );
        }
        Ok(())
    }

    pub(super) async fn receive_forwarded_unlink(
        &mut self,
        from: &str,
        unscoped: Option<&Value>,
        scoped: Option<&Value>,
    ) -> Result<(), Error> {
        let parsed = match (unscoped, scoped) {
            (Some(value), None) => parse_unlink_act(value).map(|act| (act, Submitter::Computer)),
            (None, Some(value)) => {
                let readable_version = |value: &Value| {
                    value
                        .get("formatVersion")
                        .is_none_or(|version| matches!(version.as_u64(), Some(0 | 1)))
                };
                if !readable_version(value) || !readable_version(&value["grant"]) {
                    return Ok(());
                }
                serde_json::from_value::<ScopedAct>(value.clone())
                    .ok()
                    .filter(|given| {
                        !given.replay_only
                            && matches!(given.grant.submitter, Submitter::Paired { .. })
                            && safe_token(&given.grant.operation, 1, 80)
                            && self.own().is_ok_and(|own| own.public == given.owner)
                            && self
                                .link
                                .setup()
                                .is_ok_and(|setup| setup.position.id == given.grant.setup)
                            && value["act"].get("action").is_none()
                    })
                    .map(|given| (given.act, given.grant.submitter))
            }
            _ => None,
        };
        let Some((act, by)) = parsed else {
            return Ok(());
        };
        let now = lease::wall();
        if act.area != "computers"
            || act.method != "DELETE"
            || !act.valid_id()
            || now.saturating_sub(act.at) > 50_000
            || act.at > now + 15_000
            || !self
                .link
                .setup()
                .is_ok_and(|setup| setup.role == Role::Primary)
        {
            return Ok(());
        }
        let Some(peer) = self.host(from) else {
            return Ok(());
        };
        let answer = match serde_json::from_value::<Option<Unlink>>(act.body) {
            Ok(request) => self
                .unlink_computer(act.item, request.unwrap_or_default(), by)
                .await
                .unwrap_or_else(|error| refused(503, &error.to_string())),
            Err(_) => refused(400, "Send a valid removal request."),
        };
        self.send(&peer, json!({"acted": {"id": act.id, "status": answer.status, "value": answer.value}, "at": lease::wall()})).await;
        Ok(())
    }

    pub(super) fn receive_computer_answer(&mut self, from: &str, value: &Value) {
        let Some(id) = value["id"].as_str() else {
            return;
        };
        let matches = self.forwarding.pending.get(id).is_some_and(|pending| {
            pending.position.primary == from
                && self.link.setup().is_ok_and(|setup| {
                    setup.role == Role::Member
                        && setup.position.primary == from
                        && pending.position == setup.position
                })
        });
        if matches && let Some(pending) = self.forwarding.pending.remove(id) {
            let status = value["status"]
                .as_u64()
                .and_then(|status| u16::try_from(status).ok());
            let answer = match status {
                Some(status @ (200 | 201 | 202 | 400 | 403 | 404 | 405 | 409 | 503)) => Response {
                    status,
                    value: value["value"].clone(),
                },
                _ => refused(503, NO_MAIN),
            };
            pending.finish(&self.files, answer);
        }
    }

    pub(super) async fn finish_add_request(&mut self, finished: Finished) {
        let Finished {
            destination,
            position,
            by,
            result,
        } = finished;
        let stable = self
            .link
            .setup()
            .is_ok_and(|setup| setup.role == Role::Primary && setup.position == position);
        let still_member = match &destination {
            Where::Peer { from, .. } => self.host(from).is_some(),
            Where::Local(_) => true,
        };
        let response = if !stable || !still_member {
            Ok(refused(409, CHANGED))
        } else if !by.current(&self.link) {
            Ok(refused(403, UNPAIRED))
        } else {
            match result {
                PreparedRequest::Checklist(value) => Ok(Response::ok(value)),
                PreparedRequest::Ssh(answer) => Ok(answer),
                PreparedRequest::Offer { invite, seed } => {
                    let consent = seed.map_or(Consent::None, Consent::Selected);
                    self.issue_installer_offer(invite, consent, by).await
                }
                PreparedRequest::Failed(answer) => Ok(answer),
            }
        };
        match destination {
            Where::Local(reply) => {
                let _ = reply.send(response);
            }
            Where::Peer {
                from,
                request,
                cache,
            } => {
                self.forwarding
                    .active
                    .remove(&(from.clone(), request.clone()));
                let response = response.unwrap_or_else(|error| refused(503, &error.to_string()));
                if let Some(digest) = cache
                    && self.forwarding.completed.len() < 64
                {
                    self.forwarding.completed.insert(
                        (from.clone(), request.clone()),
                        Cached {
                            digest,
                            expires: Instant::now() + Duration::from_secs(50),
                            answer: response.clone(),
                        },
                    );
                }
                self.add_answer(&from, &request, response).await;
            }
        }
    }

    async fn add_answer(&self, from: &str, id: &str, answer: Response) {
        if let Some(peer) = self.host(from) {
            self.send(&peer, json!({"acted": {"id": id, "status": answer.status, "value": answer.value}, "at": lease::wall()})).await;
        }
    }

    pub(super) async fn retry_computer_requests(&mut self) {
        let now = Instant::now();
        self.forwarding
            .completed
            .retain(|_, entry| entry.expires > now);
        let due: Vec<_> = self
            .forwarding
            .pending
            .iter()
            .filter(|(_, pending)| pending.next <= now || pending.reply.is_closed())
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(mut pending) = self.forwarding.pending.remove(&id) else {
                continue;
            };
            let current = self.link.setup().is_ok_and(|setup| {
                setup.role == Role::Member && setup.position == pending.position
            });
            if pending.reply.is_closed() || pending.deadline <= now || !current {
                let answer = if current {
                    refused(503, NO_MAIN)
                } else {
                    refused(409, CHANGED)
                };
                pending.finish(&self.files, answer);
                continue;
            }
            if let Some(peer) = self.host(&pending.position.primary) {
                self.send(&peer, pending.outgoing.envelope(&pending.position.primary))
                    .await;
            }
            pending.next = now + RETRY;
            self.forwarding.pending.insert(id, pending);
        }
    }
}

fn refused(status: u16, message: &str) -> Response {
    Response {
        status,
        value: json!({"message": message}),
    }
}

fn parse_unlink_act(value: &Value) -> Option<Act> {
    if value.get("action").is_some() {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}
