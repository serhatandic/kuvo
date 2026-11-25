//! The local composer's durable handoff and receipt feed.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use http::StatusCode;
use http_body_util::BodyExt;
use kuvo_protocol::routes::{
    delete_kuvo_sessions_id_delivery_message as remove,
    get_kuvo_data_messages_operation as receipt,
    get_kuvo_data_messages_operation_events as receipt_events,
    get_kuvo_sessions_id_delivery as read, get_kuvo_sessions_id_delivery_events as events,
    post_kuvo_data_messages as addressed, post_kuvo_sessions_id_delivery as save,
    post_kuvo_sessions_id_delivery_message_ack as ack,
    post_kuvo_sessions_id_delivery_message_now as now,
    post_kuvo_sessions_id_delivery_message_steer as steer, post_kuvo_sessions_id_submit as submit,
};
use kuvo_server::{
    Access, Caller, JsonLine, LocalOrigin, Refusal, Registry, RouteCall, RouteReply, Transport,
};
use kuvo_wire::keys::{decode_exact, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time,
};

use crate::{
    Engine, LocalSessions, OwnedRequest, OwnerAction, Submitter, disk, queue, sessions,
    typed::{self, Draft},
};

const MAX_PENDING: usize = 200;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Outcome {
    Pending {
        message: String,
    },
    Committed {
        #[serde(rename = "acceptedAt")]
        accepted_at: u64,
    },
    Interrupted {
        message: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
struct Receipt {
    operation: String,
    owner: String,
    #[serde(flatten)]
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    submission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    digest: Option<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    Steer,
    Now,
}

impl Mode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Now => "now",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Operation {
    setup: String,
    record: String,
    operation: String,
    owner: String,
    body: Value,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    draft: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    delivery: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    scoped: bool,
    submitter: Submitter,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery_mode: Option<Mode>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Removed {
    id: String,
    draft: String,
    words: String,
    files: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt: Option<String>,
}

#[derive(Clone, Copy)]
enum Edit {
    TakeBack,
    Acknowledge,
    SetMode(Mode),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default)]
    format_version: u32,
    record: String,
    setup: String,
    digest: String,
    receipt: Receipt,
    #[serde(default)]
    at: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    draft: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    delivery: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    scoped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submitter: Option<Submitter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued_delivery: Option<Operation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queued_submission: Option<Operation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queued_message: Option<Operation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parked: Option<Operation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submission: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    removal_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removed_delivery: Option<Removed>,
}

impl Saved {
    fn author_matches(&self, viewer: &Submitter) -> bool {
        self.submitter
            .as_ref()
            .is_some_and(|author| author.author_matches(viewer))
    }

    fn queued(&self) -> Option<&Operation> {
        self.queued_delivery
            .as_ref()
            .or(self.queued_submission.as_ref())
            .or(self.queued_message.as_ref())
    }
}

fn hash(mut value: Value) -> String {
    value.sort_all_objects();
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.to_string()))
}

fn operation(setup: &str, session: &str, id: &str) -> String {
    hash(json!(["delivery-v1", setup, session, id]))
}

fn message_operation(setup: &str, session: &str, id: &str) -> String {
    hash(json!([setup, hash(json!([session, id]))]))
}

impl Operation {
    fn canonical_key(&self) -> Option<String> {
        let id = self.body["id"]
            .as_str()
            .filter(|id| crate::send::valid_id(id))?;
        if self.delivery {
            Some(operation(&self.setup, &self.record, id))
        } else if self.scoped {
            crate::submissions::key(&self.record, &self.setup, &self.owner, &self.body)
        } else {
            Some(message_operation(&self.setup, &self.record, id))
        }
    }
}

fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn store_error(_: io::Error) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "This computer couldn’t save the message receipt. Try again in a moment.",
    )
}

fn gone() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "There’s no such message operation here.",
    )
}

struct Target {
    setup: String,
    owner: String,
    name: Option<String>,
    submitter: Submitter,
}

impl Target {
    fn for_session<T>(
        engine: &Engine,
        call: &RouteCall<T>,
        session: &str,
    ) -> Result<Self, Refusal> {
        let submitter = Submitter::from_request(&call.request.caller)?;
        let scope = engine
            .ownership
            .as_ref()
            .map(|authority| authority.scope(session, None))
            .transpose()?;
        let setup = scope
            .as_ref()
            .and_then(|scope| scope.setup.clone())
            .unwrap_or_default();
        let computer =
            crate::query(&call.request.path, "computer").unwrap_or_else(|| "here".into());
        if computer == "here"
            || scope
                .as_ref()
                .and_then(|scope| scope.here)
                .is_some_and(|key| encode(&key) == computer)
        {
            return Ok(Self {
                setup,
                owner: "here".into(),
                name: None,
                submitter,
            });
        }
        if !valid_key(&computer) {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "Name the conversation’s computer.",
            ));
        }
        let owner = scope
            .and_then(|scope| scope.owner)
            .filter(|owner| owner.key.is_some_and(|key| encode(&key) == computer))
            .or_else(|| {
                engine
                    .ownership
                    .as_ref()?
                    .members()
                    .into_iter()
                    .find(|owner| owner.key.is_some_and(|key| encode(&key) == computer))
            })
            .filter(|_| !setup.is_empty())
            .ok_or_else(|| {
                Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There’s no such computer in this setup.",
                )
            })?;
        Ok(Self {
            setup,
            owner: computer,
            name: Some(owner.name),
            submitter,
        })
    }

    fn owns(&self, saved: &Saved) -> bool {
        self.setup == saved.setup
            && self.owner == saved.receipt.owner
            && saved.author_matches(&self.submitter)
    }
}

#[derive(Clone)]
pub(crate) struct Delivery {
    store: PathBuf,
    name: String,
    wake: mpsc::Sender<()>,
    changes: watch::Receiver<u64>,
    updates: watch::Sender<u64>,
    handing_off: watch::Receiver<Option<String>>,
    _task: Arc<Task>,
}

struct Task(JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn file(root: &Path, family: &str, key: &str) -> PathBuf {
    root.join(family).join(format!("{key}.json"))
}

fn delivery_record(saved: &Saved, key: &str) -> bool {
    saved.format_version <= 1
        && saved.receipt.operation == key
        && (saved.delivery && saved.receipt.owner == "here" || valid_key(&saved.receipt.owner))
}

async fn lookup(root: &Path, key: &str) -> io::Result<Option<Saved>> {
    if !valid_key(key) {
        return Ok(None);
    }
    for family in ["message-receipts", "outbox"] {
        if let Some(saved) = disk::read::<Saved>(&file(root, family, key)).await? {
            if saved.format_version > 1 {
                return Err(io::Error::other("Unsupported message receipt format."));
            }
            return Ok(delivery_record(&saved, key).then_some(saved));
        }
    }
    Ok(None)
}

async fn pending(root: &Path) -> io::Result<Vec<(Saved, Operation)>> {
    let mut list = match tokio::fs::read_dir(root.join("outbox")).await {
        Ok(list) => list,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut saved = Vec::new();
    while let Some(entry) = list.next_entry().await? {
        let name = entry.file_name();
        let Some(key) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|key| valid_key(key))
        else {
            continue;
        };
        let Some(found) = lookup(root, key).await.ok().flatten() else {
            continue;
        };
        if let Some(op) = found.queued().cloned()
            && matches!(found.receipt.outcome, Outcome::Pending { .. })
            && op.setup == found.setup
            && op.operation == key
            && op.owner == found.receipt.owner
            && op.record == found.record
            && found.submitter.as_ref() == Some(&op.submitter)
            && op.delivery == found.delivery
            && op.scoped == found.scoped
            && op.draft == found.draft
            && op.canonical_key().as_deref() == Some(key)
            && sessions::valid_id(&found.record)
        {
            saved.push((found, op));
        }
    }
    saved.sort_by(|(a, _), (b, _)| {
        a.at.cmp(&b.at)
            .then(a.receipt.operation.cmp(&b.receipt.operation))
    });
    Ok(saved)
}

async fn finish(
    engine: &Engine,
    list: &LocalSessions,
    root: &Path,
    mut saved: Saved,
    op: Operation,
) -> io::Result<bool> {
    let Some(id) = op.body["id"].as_str().map(str::to_owned) else {
        return Ok(false);
    };
    let result = if op.owner == "here" {
        apply_delivery(engine, list, op).await
    } else {
        contact_owner(engine, op).await
    };
    let (status, response) = match result {
        Ok(response) => (StatusCode::OK, response),
        Err(error) => {
            let response = error.reply();
            if response.status.is_server_error() {
                return Ok(false);
            }
            let status = response.status;
            let body = response
                .body
                .collect()
                .await
                .ok()
                .and_then(|body| serde_json::from_slice(&body.to_bytes()).ok())
                .unwrap_or_default();
            (status, body)
        }
    };
    if response["ambiguous"] == true || crate::session_move::refused(status, &response) {
        return Ok(false);
    }
    saved.receipt.outcome =
        if status == StatusCode::OK && (!saved.draft || response["message"] == id) {
            Outcome::Committed {
                accepted_at: response["acceptedAt"].as_u64().unwrap_or_else(disk::now),
            }
        } else {
            let message = response["answer"]["error"]
                .as_str()
                .or_else(|| response["message"].as_str())
                .unwrap_or("The message could not be sent.")
                .to_owned();
            Outcome::Interrupted { message }
        };
    saved.parked = saved
        .queued_submission
        .take()
        .filter(|_| matches!(saved.receipt.outcome, Outcome::Interrupted { .. }));
    saved.queued_delivery = None;
    saved.queued_message = None;
    let key = &saved.receipt.operation;
    let _writer = engine
        .decision_writer
        .acquire()
        .await
        .expect("the decision writer stays open");
    if !lookup(root, key)
        .await?
        .is_some_and(|current| !current.removed && current.queued().is_some())
    {
        return Ok(true);
    }
    disk::durable(file(root, "message-receipts", key), saved.clone()).await?;
    let _ = tokio::fs::remove_file(file(root, "outbox", key)).await;
    Ok(true)
}

async fn contact_owner(engine: &Engine, op: Operation) -> Result<Value, Refusal> {
    let unavailable = || {
        Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Waiting for the conversation’s computer.",
        )
    };
    let authority = engine.ownership.as_ref().ok_or_else(unavailable)?;
    let key = decode_exact(&op.owner).ok_or_else(unavailable)?;
    let scope = authority.scope(&op.record, None)?;
    if scope.held {
        return Err(unavailable());
    }
    if scope.setup.as_deref() != Some(&op.setup)
        || scope
            .owner
            .as_ref()
            .and_then(|owner| owner.key)
            .is_some_and(|owner| owner != key)
    {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "The conversation’s owner changed.",
        ));
    }
    let awaiting_announcement = op.delivery && scope.owner.is_none();
    let request = OwnedRequest {
        setup: op.setup,
        session: op.record,
        operation: op.operation,
        submitter: op.submitter,
        action: if op.scoped {
            OwnerAction::Submit { draft: op.body }
        } else if op.delivery {
            OwnerAction::Deliver {
                message: json!({"draft":op.body,"deliveryMode":op.delivery_mode}),
            }
        } else {
            OwnerAction::Deliver {
                message: json!({"prompt":op.body}),
            }
        },
    };
    let answer = authority
        .request(key, request, Duration::from_secs(12))
        .await?;
    let status = StatusCode::from_u16(answer.status).map_err(|_| unavailable())?;
    if awaiting_announcement && status == StatusCode::CONFLICT {
        return Err(unavailable());
    }
    if status.is_success() {
        Ok(answer.body)
    } else {
        Err(Refusal::json(status, answer.body))
    }
}

pub(crate) async fn receive(
    engine: &Engine,
    list: &LocalSessions,
    request: OwnedRequest,
    message: Value,
) -> Result<Value, Refusal> {
    let bad = || {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Use the original owner and stable message ID.",
        )
    };
    let authority = engine.ownership.as_ref().ok_or_else(bad)?;
    let scope = authority.scope(&request.session, None)?;
    if scope.held {
        return Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Waiting for the conversation’s computer.",
        ));
    }
    if scope.setup.as_deref() != Some(&request.setup)
        || !scope.owner.is_some_and(|owner| owner.local)
    {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "The conversation’s owner changed.",
        ));
    }
    if let Some(prompt) = message["prompt"].as_object() {
        let id = prompt
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| crate::send::valid_id(id))
            .ok_or_else(bad)?;
        if request.operation != message_operation(&request.setup, &request.session, id)
            || !valid_addressed(&message["prompt"])
        {
            return Err(bad());
        }
        request.submitter.authorize(authority.as_ref())?;
        let caller = request.submitter.caller(authority.clone(), true);
        return crate::send::send_draft(engine, list, &request.session, prompt.clone(), &caller)
            .await;
    }
    let id = message["draft"]["id"]
        .as_str()
        .filter(|id| crate::send::valid_id(id))
        .ok_or_else(bad)?;
    if request.operation != operation(&request.setup, &request.session, id) {
        return Err(bad());
    }
    let delivery_mode =
        serde_json::from_value(message["deliveryMode"].clone()).map_err(|_| bad())?;
    apply_delivery(
        engine,
        list,
        Operation {
            setup: request.setup,
            record: request.session,
            operation: request.operation,
            owner: "here".into(),
            body: message["draft"].clone(),
            draft: true,
            delivery: true,
            scoped: false,
            submitter: request.submitter,
            delivery_mode,
        },
    )
    .await
}

async fn apply_delivery(
    engine: &Engine,
    list: &LocalSessions,
    op: Operation,
) -> Result<Value, Refusal> {
    let caller = if let Some(authority) = &engine.ownership {
        let scope = authority.scope(&op.record, None)?;
        if scope.held {
            return Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Waiting for the conversation’s computer.",
            ));
        }
        if scope.setup.as_deref().unwrap_or_default() != op.setup
            || scope.owner.as_ref().is_some_and(|owner| !owner.local)
        {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "The conversation’s owner changed.",
            ));
        }
        op.submitter.authorize(authority.as_ref())?;
        op.submitter.caller(authority.clone(), true)
    } else {
        if op.submitter != Submitter::Computer {
            return Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Checking the submitting device’s access.",
            ));
        }
        Caller::Local(Transport::Http {
            origin: LocalOrigin::App,
            internal: true,
        })
    };
    let draft = serde_json::from_value::<Draft>(op.body).map_err(|_| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            "Send a draft with its stable id, text and message fields.",
        )
    })?;
    let author = typed::author(&caller)?;
    typed::submit(
        engine,
        list,
        &op.record,
        draft,
        typed::DraftContext {
            caller: &caller,
            author: &author,
            delivery_mode: op.delivery_mode,
            submission: None,
            request: typed::DraftRequest::Durable,
        },
    )
    .await
}

impl Delivery {
    fn start(engine: Engine, list: LocalSessions, name: String) -> Self {
        let store = engine.home.join("run/owned-data");
        let root = store.clone();
        let (wake, mut nudge) = mpsc::channel::<()>(1);
        let (updates, changes) = watch::channel(0u64);
        let worker_updates = updates.clone();
        let (active, handing_off) = watch::channel(None);
        let task = tokio::spawn(async move {
            let mut tick = timer_fires::interval(Timer::DeliveryRecovery, Duration::from_secs(5));
            tick.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => {},
                    received = nudge.recv() => {
                        if received.is_none() { break; }
                    },
                }
                let Ok(waiting) = pending(&root).await else {
                    continue;
                };
                let mut deferred = std::collections::BTreeSet::new();
                for (_, candidate) in waiting {
                    if deferred.contains(&candidate.record) {
                        continue;
                    }
                    let current = {
                        let _writer = engine
                            .decision_writer
                            .acquire()
                            .await
                            .expect("the decision writer stays open");
                        match lookup(&root, &candidate.operation).await {
                            Ok(Some(saved)) => saved.queued().cloned().map(|op| {
                                active.send_replace(Some(op.operation.clone()));
                                (saved, op)
                            }),
                            _ => None,
                        }
                    };
                    let Some((saved, op)) = current else {
                        continue;
                    };
                    // A failure at the head can recover on the next tick without reordering.
                    let result = finish(&engine, &list, &root, saved, op).await;
                    active.send_replace(None);
                    match result {
                        Ok(true) => {
                            worker_updates.send_modify(|change| *change = change.wrapping_add(1))
                        }
                        _ => {
                            deferred.insert(candidate.record);
                        }
                    }
                }
            }
        });
        Self {
            store,
            name,
            wake,
            changes,
            updates,
            handing_off,
            _task: Arc::new(Task(task)),
        }
    }

    async fn save(
        &self,
        engine: &Engine,
        session: &str,
        body: Value,
        target: Target,
        kind: Form<'_>,
    ) -> Result<Saved, Refusal> {
        let Target {
            setup,
            owner,
            name,
            submitter,
        } = target;
        let id = body["id"].as_str().ok_or_else(gone)?;
        let (key, digest, submission) = match kind {
            Form::Delivery => (
                operation(&setup, session, id),
                hash(json!({"owner":owner,"body":body,"submitter":submitter})),
                None,
            ),
            Form::Addressed => (
                message_operation(&setup, session, id),
                hash(body.clone()),
                None,
            ),
            Form::Scoped { through } => (
                crate::submissions::key(session, &setup, &owner, &body).ok_or_else(gone)?,
                crate::submissions::intent(&body, &submitter, through),
                body["submission"].as_str().map(str::to_owned),
            ),
        };
        let _writer = engine
            .decision_writer
            .acquire()
            .await
            .expect("the decision writer stays open");
        if let Some(prior) = lookup(&self.store, &key).await.map_err(store_error)? {
            if prior.digest == digest {
                return Ok(prior);
            }
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "That message id is already used.",
            ));
        }
        if delivery_destination_changed(engine, session, &owner)? {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "The conversation’s owner changed.",
            ));
        }
        let rows = pending(&self.store).await.map_err(store_error)?;
        if rows.len() >= MAX_PENDING {
            return Err(Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Too many messages are waiting for their computers.",
            ));
        }
        let at = rows.last().map_or(disk::now(), |(prior, _)| {
            disk::now().max(prior.at.saturating_add(1))
        });
        let receipt = Receipt {
            operation: key.clone(),
            owner: owner.clone(),
            outcome: Outcome::Pending {
                message: format!(
                    "Unsent. Delivered when {} accepts it.",
                    name.as_deref().unwrap_or(&self.name)
                ),
            },
            submission: None,
            session: None,
            digest: None,
        };
        let delivery = matches!(kind, Form::Delivery);
        let scoped = matches!(kind, Form::Scoped { .. });
        let op = Operation {
            setup: setup.clone(),
            record: session.to_owned(),
            operation: key.clone(),
            owner,
            body,
            draft: delivery || scoped,
            delivery,
            scoped,
            submitter: submitter.clone(),
            delivery_mode: None,
        };
        let saved = Saved {
            format_version: 1,
            record: session.to_owned(),
            setup,
            digest,
            receipt,
            at,
            draft: delivery || scoped,
            delivery,
            scoped,
            submitter: Some(submitter),
            queued_delivery: delivery.then(|| op.clone()),
            queued_submission: scoped.then(|| op.clone()),
            queued_message: (!delivery && !scoped).then_some(op),
            parked: None,
            submission,
            removed: false,
            removal_token: None,
            removed_delivery: None,
        };
        disk::durable(file(&self.store, "outbox", &key), saved.clone())
            .await
            .map_err(store_error)?;
        self.changed();
        Ok(saved)
    }

    fn changed(&self) {
        self.updates
            .send_modify(|change| *change = change.wrapping_add(1));
        let _ = self.wake.try_send(());
    }

    async fn edit(
        &self,
        engine: &Engine,
        call: &RouteCall<()>,
        edit: Edit,
    ) -> Result<Value, Refusal> {
        let session = sessions::id(call)?;
        let target = Target::for_session(engine, call, &session)?;
        let missing = || Refusal::new(StatusCode::NOT_FOUND, "That message isn’t waiting here.");
        let id = call.param("message").ok_or_else(missing)?;
        let key = operation(&target.setup, &session, id);
        let token = crate::query(&call.request.path, "receipt");
        let _writer = engine
            .decision_writer
            .acquire()
            .await
            .expect("the decision writer stays open");
        let mut saved = lookup(&self.store, &key)
            .await
            .map_err(store_error)?
            .filter(|saved| target.owns(saved))
            .ok_or_else(missing)?;
        let terminal = file(&self.store, "message-receipts", &key);
        let outbox = file(&self.store, "outbox", &key);
        if matches!(edit, Edit::Acknowledge) {
            if token.as_deref().is_none_or(str::is_empty) || token != saved.removal_token {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There’s no such removal here.",
                ));
            }
            if saved.removed_delivery.take().is_some() {
                disk::durable(terminal, saved).await.map_err(store_error)?;
            }
            return Ok(json!({}));
        }
        if matches!(edit, Edit::TakeBack) && saved.removed && token == saved.removal_token {
            return Ok(json!({ "removed": saved.removed_delivery }));
        }
        let op = saved
            .queued_delivery
            .as_mut()
            .filter(|_| !saved.removed)
            .ok_or_else(missing)?;
        if target.owner == "here" && self.handing_off.borrow().as_deref() == Some(&key) {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "The message is being handed to the agent.",
            ));
        }
        let answer = match edit {
            Edit::SetMode(mode) => {
                op.delivery_mode = Some(mode);
                disk::durable(outbox, saved).await.map_err(store_error)?;
                json!({})
            }
            Edit::TakeBack => {
                let words = op.body["quotedText"]
                    .as_str()
                    .or_else(|| op.body["text"].as_str())
                    .unwrap_or_default();
                let removed = Removed {
                    id: id.to_owned(),
                    draft: words.to_owned(),
                    words: words.to_owned(),
                    files: op.body["message"]["files"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                    receipt: token.clone().filter(|token| !token.is_empty()),
                };
                saved.queued_delivery = None;
                saved.removed = true;
                saved.removal_token = token;
                saved.removed_delivery = removed.receipt.is_some().then(|| removed.clone());
                saved.receipt.outcome = Outcome::Interrupted {
                    message: "The message was removed.".into(),
                };
                disk::durable(terminal, saved).await.map_err(store_error)?;
                let _ = tokio::fs::remove_file(outbox).await;
                json!({ "removed": removed })
            }
            Edit::Acknowledge => unreachable!("an acknowledgement was handled above"),
        };
        self.changed();
        Ok(answer)
    }

    async fn receipt(
        &self,
        engine: &Engine,
        key: &str,
        viewer: &Submitter,
    ) -> Result<Receipt, Refusal> {
        let saved = lookup(&self.store, key)
            .await
            .map_err(store_error)?
            .ok_or_else(gone)?;
        let setup = engine
            .ownership
            .as_ref()
            .map(|authority| authority.scope(&saved.record, None))
            .transpose()?
            .and_then(|scope| scope.setup)
            .unwrap_or_default();
        if setup != saved.setup || (saved.scoped || saved.delivery) && !saved.author_matches(viewer)
        {
            return Err(gone());
        }
        let mut receipt = saved.receipt;
        if saved.submission.is_some() {
            receipt.submission = saved.submission;
            receipt.session = Some(saved.record);
            receipt.digest = Some(saved.digest);
        }
        Ok(receipt)
    }

    async fn queue(
        &self,
        engine: Option<&Engine>,
        session: &str,
        target: &Target,
    ) -> Result<Value, Refusal> {
        let engine = engine.filter(|_| target.owner == "here");
        let mut shown = if let Some(engine) = engine {
            queue::snapshot(engine, session, true).await?
        } else {
            json!({"paused":false,"held":[],"items":[]})
        };
        let mut items = shown["items"]
            .as_array_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        let ids = items
            .iter()
            .filter_map(|row| row["id"].as_str().map(str::to_owned))
            .collect::<std::collections::BTreeSet<_>>();
        for (_, op) in pending(&self.store)
            .await
            .map_err(store_error)?
            .into_iter()
            .filter(|(saved, op)| op.record == session && target.owns(saved))
        {
            let id = &op.body["id"];
            if id.as_str().is_some_and(|id| ids.contains(id)) {
                continue;
            }
            let words = op.body["quotedText"]
                .as_str()
                .or_else(|| op.body["text"].as_str())
                .unwrap_or_default();
            let files = op.body["message"]["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file["name"].as_str())
                .collect::<Vec<_>>();
            items.push(json!({"id":id,"words":words,"files":files,"from":null,"on":"box","delivery":"queue","held":false}));
        }
        shown["items"] = json!(items);
        Ok(shown)
    }
}

pub(crate) async fn listing(
    engine: &Engine,
    session: &str,
    setup: &str,
    viewer: &Submitter,
    query: &Value,
    keys: Vec<String>,
) -> io::Result<Vec<Value>> {
    let root = engine.home.join("run/owned-data");
    let requested = query["submission"].as_str();
    let after = query["after"].as_str().unwrap_or_default();
    let mut outcomes = Vec::new();
    for key in keys {
        let Some(saved) = lookup(&root, &key).await? else {
            if requested.is_none() {
                return Err(io::Error::other("Receipt changed while reading its index."));
            }
            continue;
        };
        let Some(submission) = saved.submission.as_deref() else {
            continue;
        };
        let Some((original_owner, id)) = crate::submissions::listing_id(submission, setup) else {
            continue;
        };
        if !saved.scoped
            || saved.setup != setup
            || saved.record != session
            || submission <= after
            || original_owner != saved.receipt.owner
            || hash(json!(["draft-submission", setup, session, submission])) != key
            || !saved.author_matches(viewer)
            || query["digest"]
                .as_str()
                .is_some_and(|digest| digest != saved.digest)
        {
            continue;
        }
        let mut row = json!({"submission":submission,"originalOwner":original_owner,
            "digest":saved.digest,"submitter":saved.submitter,"evidence":"listing"});
        match saved.receipt.outcome {
            Outcome::Committed { accepted_at } => {
                row["status"] = json!("committed");
                row["acceptedAt"] = json!(accepted_at);
                row["effect"] = json!({"message":id});
            }
            Outcome::Pending { .. } => row["status"] = json!("pending"),
            Outcome::Interrupted { .. } => row["status"] = json!("unknown"),
        }
        if let Some(draft) = saved.queued_submission.or(saved.parked) {
            row["draftAvailable"] = json!(true);
            if requested.is_some() {
                row["draft"] = draft.body;
            }
        }
        outcomes.push(row);
    }
    Ok(outcomes)
}

pub(crate) async fn settle_listing(
    engine: &Engine,
    session: &str,
    setup: &str,
    report: &Value,
) -> io::Result<()> {
    if report["evidence"] != "owner"
        || report["status"] != "committed"
        || report["conflict"] == true
    {
        return Ok(());
    }
    let (Some(submission), Some(accepted_at)) =
        (report["submission"].as_str(), report["acceptedAt"].as_u64())
    else {
        return Ok(());
    };
    let key = hash(json!(["draft-submission", setup, session, submission]));
    let root = engine.home.join("run/owned-data");
    let _writer = engine
        .decision_writer
        .acquire()
        .await
        .expect("the decision writer stays open");
    let Some(mut saved) = lookup(&root, &key).await? else {
        return Ok(());
    };
    if !saved.scoped
        || saved.setup != setup
        || saved.record != session
        || saved.removed
        || saved.digest != report["digest"]
        || saved.receipt.owner != report["originalOwner"]
        || matches!(saved.receipt.outcome, Outcome::Committed { .. })
    {
        return Ok(());
    }
    saved.receipt.outcome = Outcome::Committed { accepted_at };
    saved.queued_submission = None;
    saved.parked = None;
    disk::durable(file(&root, "message-receipts", &key), saved).await?;
    let _ = tokio::fs::remove_file(file(&root, "outbox", &key)).await;
    Ok(())
}

fn delivery_destination_changed(
    engine: &Engine,
    session: &str,
    owner: &str,
) -> Result<bool, Refusal> {
    if owner == "here" {
        return Ok(false);
    }
    let Some(authority) = &engine.ownership else {
        return Ok(true);
    };
    let scope = authority.scope(session, None)?;
    Ok(scope
        .owner
        .and_then(|owner| owner.key)
        .is_some_and(|key| encode(&key) != owner))
}

enum Form<'a> {
    Delivery,
    Addressed,
    Scoped { through: &'a str },
}

fn inline(file: &Value, local: bool) -> bool {
    file["uri"]
        .as_str()
        .is_some_and(|uri| (local && uri.starts_with("file:")) || uri.starts_with("data:"))
        || file["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("data:"))
}

fn valid_addressed(body: &Value) -> bool {
    body["id"].as_str().is_some_and(crate::send::valid_id)
        && (body["words"].is_string() || body["text"].is_string())
        && body.get("command").is_none()
        && body["delivery"] != "now"
        && body.to_string().len() <= 128 * 1024
        && body.get("files").is_none_or(|files| {
            files
                .as_array()
                .is_some_and(|files| files.iter().all(|file| inline(file, false)))
        })
}

fn draft_reply<R>(saved: Saved, body: &Value) -> Result<RouteReply<R>, Refusal>
where
    R: kuvo_protocol::RouteSpec + kuvo_protocol::JsonStatus<200> + kuvo_protocol::JsonStatus<202>,
    <R as kuvo_protocol::JsonStatus<200>>::Body: serde::de::DeserializeOwned,
    <R as kuvo_protocol::JsonStatus<202>>::Body: serde::de::DeserializeOwned,
{
    match saved.receipt.outcome {
        Outcome::Pending { .. } => Ok(RouteReply::json::<202>(sessions::typed(
            serde_json::to_value(saved.receipt)
                .map_err(io::Error::other)
                .map_err(store_error)?,
        )?)),
        Outcome::Committed { accepted_at } => {
            Ok(RouteReply::json::<200>(sessions::typed(json!({
                "answer":{"send":{"text":body["text"],"quoted":true}},"message":body["id"],"acceptedAt":accepted_at
            }))?))
        }
        Outcome::Interrupted { message } => Err(Refusal::json(
            StatusCode::CONFLICT,
            json!({"message":message,"ambiguous":true}),
        )),
    }
}

impl Delivery {
    pub(crate) async fn scoped(
        &self,
        engine: &Engine,
        session: &str,
        body: Value,
        scope: &crate::OwnerScope,
        submitter: Submitter,
    ) -> Result<RouteReply<submit::Spec>, Refusal> {
        let through = scope.here.map(|key| encode(&key)).ok_or_else(gone)?;
        let setup = scope.setup.clone().ok_or_else(gone)?;
        let origin = body["submission"]
            .as_str()
            .and_then(|submission| submission.split('.').nth(1))
            .ok_or_else(gone)?;
        let key = crate::submissions::key(session, &setup, origin, &body).ok_or_else(gone)?;
        if let Some(prior) = lookup(&self.store, &key).await.map_err(store_error)? {
            if prior.digest == crate::submissions::intent(&body, &submitter, &through) {
                return draft_reply(prior, &body);
            }
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                "That message id is already used.",
            ));
        }
        let owner = scope.owner.as_ref()
            .filter(|owner| owner.key.is_some_and(|key| encode(&key) == origin))
            .ok_or_else(|| Refusal::json(StatusCode::CONFLICT, json!({"ambiguous":true,"message":"Look up that submission on its original computer."})))?;
        if !owner
            .features
            .iter()
            .any(|feature| feature == "typed.submission-recovery")
        {
            return Err(Refusal::new(
                StatusCode::CONFLICT,
                format!(
                    "Open the conversation on {} to submit this draft.",
                    owner.name
                ),
            ));
        }
        let target = Target {
            setup,
            owner: origin.into(),
            name: Some(owner.name.clone()),
            submitter,
        };
        if !body["id"].as_str().is_some_and(crate::send::valid_id)
            || body["text"]
                .as_str()
                .is_none_or(|text| text.trim_start().starts_with(['/', '!']))
            || body["side"] == true
            || body["message"]["delivery"] == "now"
            || body.to_string().len() > 128 * 1024
            || body["message"].get("files").is_some_and(|files| {
                !files
                    .as_array()
                    .is_some_and(|files| files.iter().all(|file| inline(file, false)))
            })
        {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "Send a message with a stable id and inline attachments under 128 KiB total. Send commands and Send Now on the session’s computer.",
            ));
        }
        let saved = self
            .save(
                engine,
                session,
                body.clone(),
                target,
                Form::Scoped { through: &through },
            )
            .await?;
        draft_reply(saved, &body)
    }
}

fn key<T>(call: &RouteCall<T>) -> Result<String, Refusal> {
    call.param("operation")
        .filter(|key| valid_key(key))
        .map(str::to_owned)
        .ok_or_else(gone)
}

fn register_edit<R>(routes: &mut Registry, engine: &Engine, delivery: &Delivery, edit: Edit)
where
    R: kuvo_protocol::RouteSpec<Body = ()> + kuvo_protocol::JsonStatus<200, Body = Value>,
{
    let store = delivery.clone();
    let engine = engine.clone();
    routes.register::<R, _, _>(Access::Any, move |call| {
        let store = store.clone();
        let engine = engine.clone();
        async move {
            Ok(RouteReply::json::<200>(
                store.edit(&engine, &call, edit).await?,
            ))
        }
    });
}

pub(crate) fn register(engine: &Engine, list: LocalSessions, routes: &mut Registry) -> Delivery {
    let name = std::fs::read(engine.home.join("run/link.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|link| link["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "this computer".into());
    let delivery = Delivery::start(engine.clone(), list, name);
    register_edit::<remove::Spec>(routes, engine, &delivery, Edit::TakeBack);
    register_edit::<ack::Spec>(routes, engine, &delivery, Edit::Acknowledge);
    register_edit::<steer::Spec>(routes, engine, &delivery, Edit::SetMode(Mode::Steer));
    register_edit::<now::Spec>(routes, engine, &delivery, Edit::SetMode(Mode::Now));
    let store = delivery.clone();
    let here = engine.clone();
    routes.register::<save::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        let here = here.clone();
        async move {
            let session = sessions::id(&call)?;
            let target = Target::for_session(&here, &call, &session)?;
            let draft = typed::read_draft(&call)?;
            let body: Value = serde_json::from_slice(&call.request.body).map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, "Send a draft with its stable id, text and message fields."))?;
            let files = body["message"].get("files");
            let stable = draft.id.strip_prefix("msg_").is_some_and(|id| {
                (1..=80).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
            let limit = if target.owner == "here" { 140 * 1024 * 1024 } else { 5 * 1024 * 1024 };
            if !stable || body["side"] == true || body.to_string().len() > limit || files.is_some_and(|files| !files.as_array().is_some_and(|files| files.iter().all(|file| inline(file, target.owner == "here")))) {
                return Err(Refusal::new(StatusCode::BAD_REQUEST, "Send a draft with its stable id, text and message fields. Attachments for another computer need to fit in a 5 MiB draft."));
            }
            let saved = store.save(&here, &session, body.clone(), target, Form::Delivery).await?;
            draft_reply(saved, &body)
        }
    });
    let store = delivery.clone();
    let here = engine.clone();
    routes.register::<addressed::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        let here = here.clone();
        async move {
            let bad = || Refusal::new(StatusCode::BAD_REQUEST, "Name the conversation and its computer.");
            let session = crate::query(&call.request.path, "session").filter(|id| sessions::valid_id(id)).ok_or_else(bad)?;
            if crate::query(&call.request.path, "group").is_some()
                || !crate::query(&call.request.path, "computer").is_some_and(|computer| valid_key(&computer))
            {
                return Err(bad());
            }
            let changed = || Refusal::new(StatusCode::CONFLICT, "Open the conversation on its current computer to send this message.");
            let target = Target::for_session(&here, &call, &session).map_err(|error| {
                if error.reply().status == StatusCode::NOT_FOUND { changed() } else {
                    Refusal::new(StatusCode::FORBIDDEN, "Open this from a paired Kuvo Code client.")
                }
            })?;
            if target.owner == "here" { return Err(changed()); }
            if !here.ownership.as_ref().map(|authority| authority.scope(&session, None)).transpose()?
                .and_then(|scope| scope.owner).and_then(|owner| owner.key)
                .is_some_and(|key| encode(&key) == target.owner)
            {
                return Err(changed());
            }
            let body = serde_json::from_slice::<Value>(&call.request.body).ok().filter(valid_addressed)
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Send a message with a stable id and inline attachments under 128 KiB total. Send commands and Send Now on the session’s computer."))?;
            let saved = store.save(&here, &session, body, target, Form::Addressed).await?;
            let value = serde_json::to_value(&saved.receipt).map_err(io::Error::other).map_err(store_error)?;
            match saved.receipt.outcome {
                Outcome::Pending { .. } => Ok(RouteReply::json::<202>(sessions::typed(value)?)),
                Outcome::Committed { .. } => Ok(RouteReply::json::<200>(sessions::typed(value)?)),
                Outcome::Interrupted { .. } => Err(Refusal::json(StatusCode::CONFLICT, value)),
            }
        }
    });
    let store = delivery.clone();
    let here = engine.clone();
    routes.register::<receipt::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        let here = here.clone();
        async move {
            let viewer = Submitter::from_request(&call.request.caller).map_err(|_| gone())?;
            let receipt = store.receipt(&here, &key(&call)?, &viewer).await?;
            Ok(RouteReply::json::<200>(sessions::typed(
                serde_json::to_value(receipt)
                    .map_err(io::Error::other)
                    .map_err(store_error)?,
            )?))
        }
    });
    let store = delivery.clone();
    let here = engine.clone();
    routes.register::<receipt_events::Spec, _, _>(Access::Any, move |call| {
        let mut store = store.clone();
        let here = here.clone();
        async move {
            let viewer = Submitter::from_request(&call.request.caller).map_err(|_| gone())?;
            let key = key(&call)?;
            let initial = store.receipt(&here, &key, &viewer).await?;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut current = initial;
                let mut last = Value::Null;
                let mut tick = timer_fires::interval(Timer::DeliveryFollow, Duration::from_secs(1));
                loop {
                    let value = serde_json::to_value(&current).map_err(io::Error::other)?;
                    if value != last {
                        yield JsonLine::new(serde_json::from_value(value.clone()).map_err(io::Error::other)?)?;
                        last = value;
                    }
                    tokio::select! {
                        changed = store.changes.changed() => if changed.is_err() { break; },
                        () = call.cancel.cancelled() => break,
                        _ = tick.tick() => {},
                    }
                    let Ok(next) = store.receipt(&here, &key, &viewer).await else { break; };
                    current = next;
                }
            }))
        }
    });
    let store = delivery.clone();
    let here = engine.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let store = store.clone();
        let here = here.clone();
        async move {
            let session = sessions::id(&call)?;
            let target = Target::for_session(&here, &call, &session)?;
            let snapshot = store.queue(None, &session, &target).await?;
            Ok(RouteReply::json::<200>(sessions::typed(snapshot)?))
        }
    });
    let here = engine.clone();
    let store = delivery.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let mut store = store.clone();
        let here = here.clone();
        async move {
            let session = sessions::id(&call)?;
            let target = Target::for_session(&here, &call, &session)?;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut fresh = store.queue(None, &session, &target).await.map_err(|error| io::Error::other(error.reply().status.to_string()))?;
                let mut last = Value::Null;
                let mut tick = timer_fires::interval(Timer::DeliveryFollow, Duration::from_secs(1));
                loop {
                    if fresh != last {
                        yield JsonLine::new(serde_json::from_value(fresh.clone()).map_err(io::Error::other)?)?;
                        last = fresh.clone();
                    }
                    tokio::select! {
                        changed = store.changes.changed() => if changed.is_err() { break; },
                        () = call.cancel.cancelled() => break,
                        _ = tick.tick() => {},
                    }
                    if let Ok(queue) = store.queue(Some(&here), &session, &target).await { fresh = queue; }
                }
            }))
        }
    });
    delivery
}
