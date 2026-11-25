//! Work Queue access to the current number lease and the accepted label library.

use std::time::Duration;

use http::StatusCode;
use kuvo_server::{Caller, Refusal};
use kuvo_wire::keys::encode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::{sync::oneshot, time::Instant};

use super::{Command, Owner, Response, SetupHandle};
use crate::{
    Error, Receipt, SharedKind,
    edits::Submitter,
    model::{Link, Role, safe_token},
    random, routes,
};

const WAIT: Duration = Duration::from_secs(12);
const LABELS: SharedKind = SharedKind::QueueLabels;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct QueueNumberLease {
    pub floor: u64,
    pub high: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueNumberAllocation {
    pub sequence: u64,
    pub fields: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueLabel {
    pub id: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueLabelLibrary {
    pub labels: Vec<QueueLabel>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedQueueLabels {
    pub sequence: u64,
    pub library: QueueLabelLibrary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSetup {
    pub id: String,
    pub epoch: u64,
    pub primary: String,
    pub main_here: bool,
    pub merging: bool,
    pub checkpointed: bool,
    pub removed_computers: Vec<String>,
    pub allocation: Option<QueueNumberAllocation>,
    pub labels: Option<AcceptedQueueLabels>,
}

pub(super) fn accepted(link: &Link) -> Option<QueueSetup> {
    let setup = link.setup().ok()?;
    let entry = |kind| {
        link.value(kind)
            .and_then(|value| Some((value, link.shared.get(&kind)?.sequence)))
    };
    Some(QueueSetup {
        id: setup.position.id.clone(),
        epoch: setup.position.epoch,
        primary: setup.position.primary.clone(),
        main_here: setup.role == Role::Primary,
        checkpointed: setup.checkpointed,
        removed_computers: link
            .member_list()
            .ok()
            .map(|members| members.left)
            .unwrap_or_default(),
        merging: link
            .extra
            .get("setupMerge")
            .is_some_and(|value| !value.is_null()),
        allocation: entry(SharedKind::QueueNumberAllocation).and_then(|(value, sequence)| {
            Some(QueueNumberAllocation {
                sequence,
                fields: value.as_object()?.clone(),
            })
        }),
        labels: entry(LABELS).and_then(|(value, sequence)| {
            let library = serde_json::from_value(value.clone()).ok()?;
            Some(AcceptedQueueLabels { sequence, library })
        }),
    })
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Save {
    setup: String,
    epoch: u64,
    revision: u64,
    by: Submitter,
    library: QueueLabelLibrary,
}

pub(super) struct Pending {
    main: String,
    epoch: u64,
    deadline: Instant,
    reply: oneshot::Sender<Result<Response, Error>>,
}

impl SetupHandle {
    pub fn queue_setup(&self) -> Option<QueueSetup> {
        self.views.borrow().work_queue.clone()
    }

    pub async fn queue_number_lease(&self) -> Result<Option<QueueNumberLease>, Error> {
        let reply = self.request_command(Command::QueueNumberLease).await?;
        serde_json::from_value(reply.value).map_err(|_| Error::Invalid("Work Queue number lease"))
    }

    /// Save the whole prepared label library against the revision it was read from.
    /// A member asks the accepted main to sign and publish it.
    pub async fn save_queue_labels(
        &self,
        caller: &Caller,
        setup: &QueueSetup,
        library: QueueLabelLibrary,
    ) -> Result<Receipt, Refusal> {
        let by = routes::submitter(caller)?;
        self.save_queue_labels_as(by, setup, library).await
    }

    /// Called by the resident after its local holder or active tool has admitted the request.
    pub async fn save_queue_labels_on_this_computer(
        &self,
        setup: &QueueSetup,
        library: QueueLabelLibrary,
    ) -> Result<Receipt, Refusal> {
        self.save_queue_labels_as(Submitter::Computer, setup, library)
            .await
    }

    pub async fn may_save_queue_labels(&self) -> bool {
        // The setup command checks the same write lease for both authority operations.
        self.may_switch_fleet().await
    }

    async fn save_queue_labels_as(
        &self,
        by: Submitter,
        setup: &QueueSetup,
        library: QueueLabelLibrary,
    ) -> Result<Receipt, Refusal> {
        let change = Save {
            setup: setup.id.clone(),
            epoch: setup.epoch,
            revision: setup.labels.as_ref().map_or(0, |labels| labels.sequence),
            by,
            library,
        };
        let result = tokio::time::timeout(
            WAIT + Duration::from_secs(2),
            self.request(Command::QueueLabelsSave(change)),
        )
        .await
        .map_err(|_| unavailable())??;
        if result.status == 200 {
            return serde_json::from_value(result.value).map_err(|_| unavailable());
        }
        Err(Refusal::json(
            StatusCode::from_u16(result.status).unwrap_or(StatusCode::CONFLICT),
            result.value,
        ))
    }
}

impl Owner {
    pub(super) fn work_queue_number_lease(&self) -> Option<QueueNumberLease> {
        let setup = self.link.setup().ok()?;
        if setup.role != Role::Primary || self.can_write().is_err() {
            return None;
        }
        if self.roster().is_ok_and(|roster| roster.boxes.len() == 1) {
            let had_peers = self.roster().is_ok_and(|roster| !roster.left.is_empty());
            let floor = if had_peers {
                self.lease
                    .seen
                    .as_ref()
                    .filter(|seen| seen.setup == setup.position.id)
                    .map_or(0, |seen| seen.high)
            } else {
                0
            };
            let high =
                crate::lease::reserve_local_numbers(&self.files, &setup.position.id, floor).ok()?;
            return Some(QueueNumberLease { floor, high });
        }
        let seen = self
            .lease
            .seen
            .as_ref()
            .filter(|seen| seen.setup == setup.position.id)?;
        let (floor, high) = seen.number_bounds()?;
        Some(QueueNumberLease { floor, high })
    }

    pub(super) async fn ask_queue_labels(
        &mut self,
        change: Save,
        reply: oneshot::Sender<Result<Response, Error>>,
    ) {
        let Ok(setup) = self.link.setup() else {
            let _ = reply.send(Err(Error::Conflict));
            return;
        };
        if !same_setup(&change, setup) {
            let _ = reply.send(Err(Error::Conflict));
            return;
        }
        if setup.role == Role::Primary {
            let result = self.keep_queue_labels(change).await;
            let _ = reply.send(result);
            return;
        }
        if !change.by.current(&self.link) {
            let _ = reply.send(Ok(denied()));
            return;
        }
        let main = setup.position.primary.clone();
        let epoch = setup.position.epoch;
        self.expire_queue_labels();
        let nonce = random::<16>().map(|nonce| encode(&nonce));
        let Ok(nonce) = nonce else {
            let _ = reply.send(Ok(waiting()));
            return;
        };
        if self.pending_queue_labels.len() >= 32
            || !self
                .send_authority(
                    &main,
                    json!({"kind":"queue-label-save", "request":nonce, "change":change}),
                )
                .await
        {
            let _ = reply.send(Ok(waiting()));
            return;
        }
        self.pending_queue_labels.insert(
            nonce,
            Pending {
                main,
                epoch,
                deadline: Instant::now() + WAIT,
                reply,
            },
        );
    }

    fn expire_queue_labels(&mut self) {
        self.pending_queue_labels
            .retain(|_, pending| pending.deadline > Instant::now() && !pending.reply.is_closed());
    }

    pub(super) async fn receive_queue_labels_save(
        &mut self,
        member: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let setup = self.link.setup()?;
        let roster = self.roster()?;
        if setup.role != Role::Primary
            || value["epoch"].as_u64() != Some(setup.position.epoch)
            || !roster.boxes.iter().any(|peer| peer.key == member)
            || self.link.disabled(&roster, member)
        {
            return Ok(());
        }
        let Some(nonce) = value["request"]
            .as_str()
            .filter(|nonce| safe_token(nonce, 22, 22))
        else {
            return Ok(());
        };
        let change: Save = serde_json::from_value(value["change"].clone())
            .map_err(|_| Error::Invalid("forwarded Work Queue labels"))?;
        let result = self.keep_queue_labels(change).await;
        let result = result.unwrap_or_else(|error| failed(&error));
        self.send_authority(
            member,
            json!({
                "kind":"queue-label-saved", "request":nonce,
                "status":result.status, "value":result.value,
            }),
        )
        .await;
        Ok(())
    }

    pub(super) fn receive_queue_labels_saved(
        &mut self,
        main: &str,
        value: &Value,
    ) -> Result<(), Error> {
        let setup = self.link.setup()?;
        if setup.role != Role::Member
            || setup.position.primary != main
            || value["epoch"].as_u64() != Some(setup.position.epoch)
        {
            return Ok(());
        }
        let Some(nonce) = value["request"].as_str() else {
            return Ok(());
        };
        if !self.pending_queue_labels.get(nonce).is_some_and(|pending| {
            pending.main == main
                && pending.epoch == setup.position.epoch
                && pending.deadline > Instant::now()
        }) {
            return Ok(());
        }
        let response = match value["status"].as_u64() {
            Some(200) if serde_json::from_value::<Receipt>(value["value"].clone()).is_ok() => {
                Response::ok(value["value"].clone())
            }
            Some(status @ (400 | 403 | 409 | 503)) if value["value"]["message"].is_string() => {
                Response {
                    status: status as u16,
                    value: value["value"].clone(),
                }
            }
            _ => waiting(),
        };
        if let Some(pending) = self.pending_queue_labels.remove(nonce) {
            let _ = pending.reply.send(Ok(response));
        }
        Ok(())
    }

    async fn keep_queue_labels(&mut self, change: Save) -> Result<Response, Error> {
        let setup = self.link.setup()?;
        if !same_setup(&change, setup) {
            return Err(Error::Conflict);
        }
        if !change.by.current(&self.link) {
            return Ok(denied());
        }
        if !change.library.labels.iter().all(|label| {
            label_id(&label.id)
                && (1..=40).contains(&label.name.encode_utf16().count())
                && (1..=20).contains(&label.color.len())
        }) || !change.library.removed.iter().all(|id| label_id(id))
        {
            return Err(Error::Invalid("Work Queue label library"));
        }
        let receipt = self
            .save(LABELS, json!(change.library), Some(change.revision), None)
            .await?;
        Ok(Response::ok(json!(receipt)))
    }
}

fn label_id(id: &str) -> bool {
    id.strip_prefix("lbl_")
        .or_else(|| id.strip_prefix("cat_"))
        .is_some_and(|id| {
            (1..=40).contains(&id.len())
                && id
                    .bytes()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-')
        })
}

fn same_setup(change: &Save, setup: &crate::model::Setup) -> bool {
    setup.position.id == change.setup && setup.position.epoch == change.epoch
}

fn unavailable() -> Refusal {
    let waiting = waiting();
    Refusal::json(StatusCode::CONFLICT, waiting.value)
}

fn waiting() -> Response {
    Response {
        status: 409,
        value: json!({"message": "The main computer could not save the label change."}),
    }
}

fn denied() -> Response {
    Response {
        status: 403,
        value: json!({"message": "The submitting device no longer has access."}),
    }
}

fn failed(error: &Error) -> Response {
    let (status, message) = match error {
        Error::Invalid(_) => (400, error.to_string()),
        Error::DeviceRemoved | Error::DevicePermission => (403, error.to_string()),
        Error::Conflict | Error::Capacity => (409, error.to_string()),
        _ => (503, error.to_string()),
    };
    Response {
        status,
        value: json!({"message": message}),
    }
}
