//! Installer requests share the setup's authenticated `act` / `acted` mail.
//! The originating request alone holds the installer token. The main only sees its hash.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use kuvo_wire::keys::encode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{sync::oneshot, task::JoinSet, time::Instant};

use super::{Command, Owner, Response, SetupHandle};
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

pub(crate) enum AddRequest {
    Checklist,
    Offer {
        invite: String,
        selection: Selection,
    },
    Ssh(Action),
}

impl AddRequest {
    fn allowed_for(&self) -> Duration {
        Duration::from_secs(match self {
            Self::Checklist => 20,
            Self::Offer { .. } => 45,
            Self::Ssh(Action::Page) => 20,
            Self::Ssh(_) => 45,
        })
    }

    fn to_act(&self, at: u64, id: String, position: &Position, by: Submitter) -> Act {
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
        };
        let actor = match by {
            Submitter::Computer => None,
            Submitter::Paired { key, pairing } => Some(Device { key, pairing }),
        };
        Act {
            id,
            at,
            area: AREA.into(),
            method: method.into(),
            item,
            body: Scope {
                setup: position.id.clone(),
                epoch: position.epoch,
                actor,
                input,
            },
        }
    }

    fn mutates(&self) -> bool {
        match self {
            Self::Checklist => false,
            Self::Offer { .. } => true,
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
    body: Scope,
}

impl Act {
    fn request(&self) -> Option<AddRequest> {
        if self.area != AREA
            || !self
                .id
                .strip_prefix("wqa_")
                .is_some_and(|id| safe_token(id, 16, 40))
        {
            return None;
        }
        match (self.method.as_str(), self.item.as_str()) {
            ("GET", "credentials") => Some(AddRequest::Checklist),
            ("POST", "code") => {
                let given = self.body.input.as_ref()?;
                Some(AddRequest::Offer {
                    invite: given["invite"]
                        .as_str()
                        .filter(|id| safe_token(id, 43, 43))?
                        .into(),
                    selection: Selection::read(given.get("credentials").cloned())?,
                })
            }
            _ => {
                Action::read(&self.method, &self.item, self.body.input.clone()).map(AddRequest::Ssh)
            }
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
    target: String,
    act: Act,
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
            .request_command(Command::AddViaMain(
                AddRequest::Offer {
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
    pub(super) async fn begin_add_request(
        &mut self,
        request: AddRequest,
        by: Submitter,
        reply: Reply,
    ) -> Result<(), Error> {
        let Ok(setup) = self.link.setup() else {
            let _ = reply.send(Ok(refused(409, CHANGED)));
            return Ok(());
        };
        if !by.current(&self.link) {
            let _ = reply.send(Ok(refused(403, UNPAIRED)));
            return Ok(());
        }
        let position = setup.position.clone();
        if setup.role == Role::Primary {
            if self.forwarding.jobs.len() >= 32 {
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
        let sent = Instant::now();
        let deadline = sent + request.allowed_for();
        let id = format!("wqa_{}", encode(&random::<16>()?));
        let act = request.to_act(lease::wall(), id.clone(), &position, by);
        let spool = if request.mutates() {
            let path = format!("run/work-queue/forward/{id}.json");
            if self
                .files
                .write(
                    &path,
                    &json!({"formatVersion": 1, "to": position.primary, "act": act}),
                )
                .is_err()
            {
                let _ = reply.send(Ok(refused(503, NO_MAIN)));
                return Ok(());
            }
            Some(path)
        } else {
            None
        };
        self.send(&target, json!({"act": act, "at": act.at})).await;
        self.forwarding.pending.insert(
            id,
            Pending {
                target: position.primary,
                act,
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
        request: AddRequest,
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
                AddRequest::Checklist => {
                    PreparedRequest::Checklist(Inventory::read(&files).await.checklist())
                }
                AddRequest::Ssh(action) => match ssh {
                    Some(ssh) => PreparedRequest::Ssh(ssh.execute(action, by.clone()).await),
                    None => PreparedRequest::Failed(refused(
                        503,
                        "Kuvo’s local server hasn’t started yet. Try again in a moment.",
                    )),
                },
                AddRequest::Offer { invite, selection } if selection.empty() => {
                    PreparedRequest::Offer { invite, seed: None }
                }
                AddRequest::Offer { invite, selection } => {
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
        let Some(request) = act.request() else {
            return Ok(());
        };
        let now = lease::wall();
        if now.saturating_sub(act.at) > (request.allowed_for() + RETRY).as_millis() as u64
            || act.at > now + 15_000
            || self.forwarding.jobs.len() >= 32
        {
            return Ok(());
        }
        let by = act.body.by();
        let Some(setup) = self
            .link
            .setup()
            .ok()
            .filter(|setup| setup.role == Role::Primary && act.body.in_setup(&setup.position))
        else {
            self.add_answer(from, &act.id, refused(409, CHANGED)).await;
            return Ok(());
        };
        let position = setup.position.clone();
        if !by.current(&self.link) {
            self.add_answer(from, &act.id, refused(403, UNPAIRED)).await;
            return Ok(());
        }
        if let AddRequest::Offer { invite, selection } = &request
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
        let cache = if matches!(&request, AddRequest::Ssh(action) if action.changes()) {
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

    pub(super) fn receive_add_answer(&mut self, from: &str, value: &Value) {
        let Some(id) = value["id"].as_str() else {
            return;
        };
        let matches = self.forwarding.pending.get(id).is_some_and(|pending| {
            pending.target == from
                && self.link.setup().is_ok_and(|setup| {
                    setup.role == Role::Member
                        && setup.position.primary == from
                        && pending.act.body.in_setup(&setup.position)
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

    pub(super) async fn retry_add_requests(&mut self) {
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
                setup.role == Role::Member
                    && setup.position.primary == pending.target
                    && pending.act.body.in_setup(&setup.position)
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
            if let Some(peer) = self.host(&pending.target) {
                self.send(&peer, json!({"act": pending.act, "at": pending.act.at}))
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
