//! Client creation and local deletion/restore through the queue's writer lock.

mod book;
mod edit;
mod fences;
mod hand;
mod importing;
mod offline;
mod peer_actions;
mod tools;

use super::{
    assignees::Assignees,
    attachments::staging::{ClientFiles, Staged},
    forward::{Forward, records::Act},
    labels,
    mentions::Index,
    migration,
    numbers::{Grant, Numbers},
    peer_reads::PeerReads,
    refs::{Reference, References},
    routes::{blocking, internal, local_work_queue_app, storage_refusal, typed, work_queue_client},
    status::{self, PRIORITIES, STATUSES},
    store::{self, Error, Item, QUEUE, Store, limit, owner_id, plain_name, queue_files},
    told, view,
};
use fences::{Compaction, Fence};
use http::{HeaderMap, StatusCode};
use jiff::Timestamp;
use kuvo_engine::{Engine, LocalSessions};
use kuvo_peers::{
    directory::RecordScope,
    ownership,
    relay::{RelayEvent, RelayHandle},
    setup_lists::Reader,
};
use kuvo_protocol::routes::{
    delete_kuvo_work_queue_items_item as delete, post_kuvo_work_queue_items as add,
    post_kuvo_work_queue_items_item_restore as restore,
};
use kuvo_server::{Access, Refusal, Registry};
use kuvo_setup::{QueueLabel, SetupHandle};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
    locks::OwnedLock,
};
use kuvo_wire::mail::Envelope;
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const CLIENT: &str = "Only Kuvo Code on your Mac or iPhone can do this: agents change the Work Queue with their tools.";
const RECEIPT: ObjectFormat = ObjectFormat::additive("Owned data");
const CREATION_FIELDS: [&str; 13] = [
    "owner",
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
    "removeInlineImages",
];

#[derive(Clone)]
struct Writer {
    store: Store,
    setup: SetupHandle,
    peers: Reader,
    refs: Arc<References>,
    numbers: Numbers,
    files: Arc<ClientFiles>,
    sessions: LocalSessions,
    engine: Engine,
    reads: PeerReads,
    forward: Forward,
    revisions: Arc<offline::Revisions>,
}

pub(super) fn register(
    registry: &mut Registry,
    store: Store,
    setup: SetupHandle,
    peers: Reader,
    refs: Arc<References>,
    sessions: LocalSessions,
    engine: Engine,
    relay: Option<RelayHandle>,
    reads: PeerReads,
    owners: ownership::Handle,
) -> Runtime {
    let writer = Writer {
        revisions: Arc::new(offline::Revisions::new()),
        numbers: Numbers::new(&store, setup.clone(), peers.clone()),
        files: Arc::new(ClientFiles::new()),
        forward: Forward::new(store.clone(), owners, relay.clone()),
        sessions,
        engine: engine.clone(),
        reads,
        store,
        setup,
        peers,
        refs,
    };
    let here = writer.clone();
    registry.register::<add::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        async move {
            if !work_queue_client(&call.request.caller) {
                return Err(Refusal::new(StatusCode::FORBIDDEN, CLIENT));
            }
            let body: Value = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
            if body.get("baseRevision").is_some() {
                let id = body["id"].as_str().unwrap_or_default().to_owned();
                return offline::reply(
                    here.ordinary_edit(
                        &call.request.caller,
                        &call.request.headers,
                        &id,
                        body,
                        true,
                        false,
                    )
                    .await?,
                );
            }
            typed(
                here.create(
                    &call.request.headers,
                    body,
                    local_work_queue_app(&call.request.caller),
                )
                .await?,
            )
        }
    });
    let here = writer.clone();
    registry.register::<delete::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        let name = super::routes::decode(call.param("item"));
        async move {
            if !work_queue_client(&call.request.caller) {
                return Err(Refusal::new(StatusCode::FORBIDDEN, CLIENT));
            }
            let body: Value = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
            if store::item_id(&name) && body.get("baseRevision").is_some() {
                return offline::reply(
                    here.ordinary_edit(
                        &call.request.caller,
                        &call.request.headers,
                        &name,
                        body,
                        false,
                        true,
                    )
                    .await?,
                );
            }
            typed(
                here.change_for_client(
                    name,
                    Removal::Delete,
                    local_work_queue_app(&call.request.caller),
                )
                .await?,
            )
        }
    });
    let here = writer.clone();
    registry.register::<restore::Spec, _, _>(Access::Any, move |call| {
        let here = here.clone();
        let name = super::routes::decode(call.param("item"));
        async move {
            if !work_queue_client(&call.request.caller) {
                return Err(Refusal::new(StatusCode::FORBIDDEN, CLIENT));
            }
            typed(
                here.change_for_client(
                    name,
                    Removal::Restore,
                    local_work_queue_app(&call.request.caller),
                )
                .await?,
            )
        }
    });
    writer.register_edits(registry);
    writer.register_import(registry);
    writer.register_tools(registry, engine);
    writer.start(relay)
}

pub struct Runtime {
    task: tokio::task::JoinHandle<()>,
    pub(super) edits: Arc<dyn kuvo_setup::RecordEdits>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Writer {
    fn start(self, relay: Option<RelayHandle>) -> Runtime {
        let mut incoming = relay.as_ref().and_then(RelayHandle::subscribe);
        let mut upkeep = tokio::time::interval(Duration::from_secs(2));
        upkeep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let edits = Arc::new(offline::Routes(self.clone()));
        let task = tokio::spawn(async move {
            let mut retry = std::collections::HashMap::new();
            let mut label_revision = None;
            let mut snapshots = told::Delivery::default();
            let mut handings = hand::Delivery::default();
            loop {
                let result = tokio::select! {
                    _ = upkeep.tick() => self.upkeep(relay.as_ref(), &mut label_revision, &mut snapshots, &mut retry, &mut handings).await,
                    message = next_mail(&mut incoming) => {
                        match (relay.as_ref(), message.as_deref()) {
                            (Some(relay), Some(RelayEvent::Mail { key, envelope, .. })) => {
                                self.received(relay, key, &Value::Object(envelope.0.clone()), &mut snapshots, &mut handings).await
                            }
                            _ => Ok(()),
                        }
                    }
                };
                if let Err(error) = result {
                    eprintln!("Work Queue exchange: {error}");
                }
            }
        });
        Runtime { task, edits }
    }

    async fn upkeep(
        &self,
        relay: Option<&RelayHandle>,
        label_revision: &mut Option<(String, u64)>,
        snapshots: &mut told::Delivery,
        retry: &mut std::collections::HashMap<String, i64>,
        handings: &mut hand::Delivery,
    ) -> Result<(), Error> {
        if let Some(setup) = self.setup.queue_setup()
            && let Some(accepted) = setup.labels
        {
            let revision = (setup.id, accepted.sequence);
            if label_revision.as_ref() != Some(&revision) {
                let store = self.store.clone();
                tokio::task::spawn_blocking(move || {
                    labels::strip(&store, &accepted.library.removed)
                })
                .await
                .map_err(io::Error::other)??;
                *label_revision = Some(revision);
            }
        }
        let now = Timestamp::now().as_millisecond();
        let local_max = self.local_max().await?;
        let grants = self.numbers.answer_own(local_max, now).await?;
        if grants.is_some() || self.waiting().await? {
            self.number_waiting(grants).await?;
        }
        self.retry_offline(relay, now, retry).await?;
        self.send_handings(relay, handings, now).await?;
        if let Some(relay) = relay {
            let local_max = self.local_max().await?;
            if let Some((main, ask)) = self.numbers.ask_to_send(local_max, now).await?
                && let Some(key) = kuvo_wire::keys::decode_exact(&main)
                && let Some(peer) = self
                    .peers
                    .view()
                    .roster
                    .member(&key)
                    .and_then(|member| member.host())
            {
                send(relay, peer, json!({"ask":ask}), now).await;
            }
            snapshots
                .publish(
                    &self.store,
                    &self.peers,
                    relay,
                    self.refs.clone(),
                    &self.sessions,
                    self.setup.queue_setup(),
                    now,
                )
                .await?;
        }
        Ok(())
    }

    async fn received(
        &self,
        relay: &RelayHandle,
        key: &[u8; 32],
        envelope: &Value,
        snapshots: &mut told::Delivery,
        handings: &mut hand::Delivery,
    ) -> Result<(), Error> {
        let queue = &envelope["workQueue"];
        if !queue.is_object()
            || queue["memory"].as_bool() == Some(true)
            || queue
                .get("formatVersion")
                .is_some_and(|version| !matches!(version.as_f64(), Some(0.0 | 1.0)))
        {
            return Ok(());
        }
        let Some(peer) = self
            .peers
            .view()
            .roster
            .member(key)
            .and_then(|member| member.host())
        else {
            return Ok(());
        };
        let by = kuvo_wire::keys::encode(key);
        let now = Timestamp::now().as_millisecond();
        self.receive_offline(relay, &peer, &by, queue, now).await?;
        if let Some(hand) = queue.get("hand")
            && let Some(answer) = self.receive_hand(&by, hand.clone(), now).await?
        {
            send(relay, peer.clone(), json!({"handed":answer}), now).await;
        }
        if let Some(answer) = queue.get("handed") {
            self.handed_back(*key, answer, handings, now).await?;
        }
        if queue.get("scopedAct").is_none()
            && self.forward.admits(*key)
            && let Some(act) = queue.get("act").and_then(Act::parse)
        {
            let id = act.id.clone();
            let answer = match self.respond_to_act(&by, act, now).await {
                Ok(answer) => answer,
                Err(error) => {
                    eprintln!("Work Queue forwarded action: {error}");
                    super::forward::records::Reply {
                        status: 503,
                        value: json!({"message":"This computer couldn’t persist the operation’s outcome. Check the record before retrying."}),
                    }
                }
            };
            if let Some(mails) = kuvo_peers::work_queue_mails(
                json!({"acted":{"id":id,"status":answer.status,"value":answer.value}}),
                now,
            ) {
                for mail in mails {
                    let _ = relay.sender.mail(peer.clone(), mail).await;
                }
            }
        }
        if let Some(had) = queue["had"].as_str() {
            snapshots.acknowledge(key, had);
        }
        if let Some(told) = queue.get("told").filter(|told| told.is_object()) {
            let store = self.store.clone();
            let sender = *key;
            let told = told.clone();
            if let Some(id) =
                tokio::task::spawn_blocking(move || told::keep(&store, &sender, &told, now))
                    .await
                    .map_err(io::Error::other)??
            {
                send(relay, peer.clone(), json!({"had":id}), now).await;
            }
        }
        if queue["ask"].is_object() && queue.get("to").is_none() && queue.get("for").is_none() {
            let local_max = self.local_max().await?;
            if let Some(block) = self
                .numbers
                .give(by, queue["ask"].clone(), local_max, now)
                .await?
            {
                send(relay, peer, json!({"give":block,"quorumGrant":true}), now).await;
            }
        } else if queue["give"].is_object() && queue.get("for").is_none() {
            let local_max = self.local_max().await?;
            if let Some(grant) = self
                .numbers
                .accept(by, queue["give"].clone(), local_max)
                .await?
            {
                self.number_waiting(Some(grant)).await?;
            }
        }
        Ok(())
    }

    async fn local_max(&self) -> Result<u64, Error> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.items().map(|items| highest(&items)))
            .await
            .map_err(io::Error::other)?
    }

    async fn waiting(&self) -> Result<bool, Error> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            store.items().map(|items| {
                items.iter().any(|item| {
                    item.removed.is_none()
                        && item.source().is_none()
                        && !item.saved.contains_key("handing")
                        && (item.number.is_none() || book::wanted(item).is_some())
                })
            })
        })
        .await
        .map_err(io::Error::other)?
    }

    fn may_number(&self, item: &Item) -> bool {
        item.source().is_none()
            && !item.saved.contains_key("handing")
            && self.admit(&item.owner).is_ok()
    }

    async fn number_waiting(&self, received: Option<Grant>) -> Result<(), Error> {
        let root = self.store.root.clone();
        let guard = tokio::task::spawn_blocking(move || {
            migration::migrate(&root)?;
            Ok::<_, Error>(store::exclusive(
                &root.join(".lock"),
                Duration::from_secs(3),
            )?)
        })
        .await
        .map_err(io::Error::other)??;
        let store = self.store.clone();
        let items = tokio::task::spawn_blocking(move || store.items())
            .await
            .map_err(io::Error::other)??;
        let mut book = book::Book::new(items);
        let now = Timestamp::now().as_millisecond();
        let grant = match received {
            Some(answer) => Some(answer),
            None => {
                let asked = self.numbers.asking_claims().await?;
                let claims = book
                    .items
                    .iter()
                    .filter(|item| item.removed.is_none() && self.may_number(item))
                    .filter_map(|item| {
                        let number = book::wanted(item)?;
                        (!book::flag(item, "backGranted")
                            && !super::numbers::contains(&asked, number))
                        .then_some((number, item.number.is_some()))
                    })
                    .collect::<Vec<_>>();
                let restores = claims
                    .iter()
                    .filter(|(_, returns)| *returns)
                    .map(|(number, _)| *number)
                    .collect();
                self.numbers
                    .claim(
                        claims.into_iter().map(|(number, _)| number).collect(),
                        restores,
                        highest(&book.items),
                        now,
                    )
                    .await?
            }
        };
        let (waiting, used) = book.settle(grant, |item| self.may_number(item), now);
        let assigned = self
            .numbers
            .allocate_with_used(waiting.len(), used, highest(&book.items), now)
            .await?;
        for (index, number) in waiting.into_iter().zip(assigned) {
            book::assign(book.mark(index), number);
        }
        let writer = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            book.save(&writer, now).map_err(|error| {
                io::Error::other(format!(
                    "Cannot persist Work Queue numbers (status {}).",
                    error.reply().status
                ))
            })
        })
        .await
        .map_err(io::Error::other)??;
        Ok(())
    }

    async fn create(
        &self,
        headers: &HeaderMap,
        body: Value,
        local: bool,
    ) -> Result<Value, Refusal> {
        let nonce = nonce(headers, &body)?;
        let setup = self.setup.queue_setup();
        let scope = setup.as_ref().map_or("", |setup| setup.id.as_str());
        let operation = key(json!([scope, nonce]));
        let id = format!("wqi_{}", &operation[..20]);
        let selected: Fields = CREATION_FIELDS
            .into_iter()
            .filter_map(|name| body.get(name).map(|value| (name.into(), value.clone())))
            .collect();
        let digest = key(Value::Object(selected));
        let now = Timestamp::now().as_millisecond();
        let guard = self.lock().await?;
        let writer = self.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let (all, compaction) = writer.changes(now)?;
            if let Some(receipt) = writer.receipt(&all, &id)? {
                if receipt["operation"].as_str() == Some(&operation) && receipt["digest"].as_str() == Some(&digest) {
                    compaction.finish(&writer, now)?;
                    return Ok(Prepared::Replay(receipt["answer"]["value"].clone()));
                }
                return Err(reused());
            }
            if all.iter().any(|item| item.id == id) {
                return Err(reused());
            }
            let assignees = Assignees::read(&writer.store);
            let assignee = assignee(&body, &assignees)?;
            let owner = body.get("owner").map(|owner| owner.as_str().filter(|owner| owner_id(owner))
                .ok_or_else(|| bad("That's no queue to add it to (owner): a conversation's address, or a group chat's id.")))
                .transpose()?.map(str::to_owned)
                .or_else(|| assignee.as_ref().and_then(|assignee| {
                    (assignee["kind"] == "bot").then(|| assignee["id"].as_str()).flatten()
                        .and_then(|id| assignees.bot_session(id)).map(str::to_owned)
                })).unwrap_or_else(|| "usr_you".into());
            writer.admit(&owner)?;
            let staged = writer.files.stage(&writer.store, body.get("files"), local)?;
            let item = writer.new_row(&id, &owner, &body, &all, &assignees, now, &json!({"user":true}))?;
            if let Some(images) = body.get("removeInlineImages") {
                let parsed = Item::read(&owner, &item).expect("new queue item");
                edit::pictures_to_remove(&parsed, images)?;
            }
            let document = writer.read_owner(&owner, now)?;
            Ok(Prepared::New { item, document, assignees, all, operation, digest, staged, compaction, body_changed: body.get("body").is_some() })
        }).await.map_err(internal)??;
        let Prepared::New {
            mut item,
            mut document,
            assignees,
            all,
            operation,
            digest,
            staged,
            compaction,
            body_changed,
        } = prepared
        else {
            let Prepared::Replay(value) = prepared else {
                unreachable!()
            };
            return Ok(value);
        };
        let assigned = self
            .numbers
            .allocate(1, highest(&all), now)
            .await
            .map_err(storage_refusal)?;
        if let Some(number) = assigned.first() {
            item["number"] = (*number).into();
        }
        let writer = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let owner = document.owner.clone();
            let published =
                writer
                    .files
                    .publish(&writer.store, &mut item, &staged, body_changed, now)?;
            let parsed = Item::read(&owner, &item).expect("new queue item");
            item["mentionIndex"] =
                json!(Index::rebuild(&writer.refs.mentions, &parsed, &all, None));
            let reader = Item::read(&owner, &item).expect("new queue item");
            let answer = view::mutation(&reader, &assignees);
            item["creationReceipt"] = json!({
                "operation":operation, "digest":digest, "answer":{"status":200,"value":answer}
            });
            document.items.push(item);
            document.save(&writer.store).map_err(storage_refusal)?;
            published.commit();
            compaction.finish(&writer, now)?;
            Ok(answer)
        })
        .await
        .map_err(internal)?
    }

    fn new_row(
        &self,
        id: &str,
        owner: &str,
        body: &Value,
        all: &[Item],
        assignees: &Assignees,
        now: i64,
        who: &Value,
    ) -> Result<Value, Refusal> {
        let title = text(
            body.get("title")
                .filter(|title| !title.is_null())
                .unwrap_or(&Value::String(String::new())),
            800,
            "The title",
        )?;
        let title = limit(&plain_name(&title), 200);
        if title.is_empty() {
            return Err(bad("Say what it is (title), in a line."));
        }
        if all
            .iter()
            .filter(|item| {
                item.owner == owner && item.removed.is_none() && !status::closed(&item.status)
            })
            .count()
            >= 2000
        {
            return Err(bad(
                "Your queue has 2000 open items already: mark some done or remove them first.",
            ));
        }
        let status = body
            .get("status")
            .map(status_of)
            .transpose()?
            .unwrap_or("todo");
        let position = place(&all, None, status, body["top"].as_bool() == Some(true));
        let label_book = labels::shown(&self.store, self.setup.queue_setup().as_ref())
            .map_err(storage_refusal)?;
        let labels = body
            .get("labels")
            .map(|given| labels_of(given, &label_book))
            .transpose()?
            .unwrap_or_default();
        let stamp = json!({"at":now,"by":who});
        let mut item = json!({
            "id":id, "owner":owner, "title":title, "body":"", "status":status, "order":position,
            "labels":labels, "notes":[], "files":[], "links":[], "sessions":[],
            "created":stamp, "updated":stamp, "mentionIndex":{"body":[],"notes":[],"targets":[]},
        });
        if status::closed(status) {
            item["doneAt"] = now.into();
        }
        if let Some(assignee) = assignee(body, assignees)? {
            item["assignee"] = assignee;
        }
        if let Some(value) = body.get("body") {
            item["body"] = text(value, 100_000, "The description (body)")?.into();
        }
        if let Some(priority) = body.get("priority").map(priority_of).transpose()?.flatten() {
            item["priority"] = priority.into();
        }
        if let Some(due) = body.get("due").map(due_of).transpose()?.flatten() {
            item["due"] = due.into();
        }
        Ok(item)
    }

    async fn change(&self, name: String, removal: Removal) -> Result<Value, Refusal> {
        self.change_as(name, removal, json!({"user":true}), None)
            .await
    }

    async fn change_as(
        &self,
        name: String,
        removal: Removal,
        who: Value,
        operation: Option<String>,
    ) -> Result<Value, Refusal> {
        let guard = self.lock().await?;
        let writer = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let now = Timestamp::now().as_millisecond();
            let Some(reference) = writer.refs.parse(&name) else {
                return Err(Refusal::new(StatusCode::NOT_FOUND, "There's no such item."));
            };
            let (all, compaction) = writer.changes(now)?;
            if let Some(answer) = operation.as_deref().and_then(|operation| tools::kept_answer(&all, operation)) {
                compaction.finish(&writer, now)?;
                return Ok(answer);
            }
            let Some(item) = writer.refs.find(&all, &reference) else {
                return Err(Refusal::new(
                    StatusCode::NOT_FOUND,
                    "There's no item like that.",
                ));
            };
            writer.admit(&item.owner)?;
            let mut file = writer.read_owner(&item.owner, now)?;
            let saved = file
                .items
                .iter_mut()
                .find(|saved| saved["id"] == item.id)
                .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "There's no item like that."))?;
            let stamp = json!({"at":now,"by":who});
            let agent = who["user"] != true;
            let display = tools::item_name(saved);
            match removal {
                Removal::Delete => {
                    if item.removed.is_some() {
                        return Err(if agent { bad(format!("{display} is deleted already.")) } else { bad("It's deleted already.") });
                    }
                    if let Some(handing) = saved.get("handing").filter(|entry| entry.is_object()) {
                        let here = writer.peers.view().roster.here.map(|key| kuvo_wire::keys::encode(&key));
                        if here.as_deref().is_some_and(|key| handing["key"].as_str() == Some(key)) {
                            return Err(bad("Wait for the local handoff to finish before deleting the item."));
                        }
                        if handing["released"].as_str().is_some_and(|proof| !proof.is_empty()) {
                            return Err(bad("That item’s new owner already accepted the handoff."));
                        }
                        let to = handing["name"].as_str().filter(|name| !name.is_empty())
                            .or_else(|| handing["to"].as_str()).unwrap_or_default();
                        let note = json!({"at":now,"by":who,
                            "text":format!("Its hand-off to {to} on another computer was called off.")});
                        let mut notes = store::array(saved.get("notes")).to_vec();
                        notes.push(note);
                        saved["notes"] = notes.into();
                    }
                    saved["removed"] = stamp.clone();
                    saved["sessions"] = json!([]);
                    saved.as_object_mut().expect("queue item").remove("handing");
                    append_event(saved, json!({"at":now,"by":who,"kind":"deleted"}));
                }
                Removal::Restore => {
                    if item.removed.is_none() {
                        return Err(if agent { bad(format!("{display} isn't deleted.")) } else { bad("It isn't deleted.") });
                    }
                    if agent && item.removed.as_ref().is_some_and(|stamp| stamp.by["user"] == true) {
                        return Err(tools::deleted(item));
                    }
                    if agent {
                        edit::refuse_handing(item)?;
                    }
                    if let Some(source) = item.source() {
                        let ref_of = |value: &Value| {
                            writer.refs.issue(value["ref"].as_str().unwrap_or_default())
                                .map(|issue| issue.key())
                                .unwrap_or_else(|| format!("{}:{}", value["kind"], value["ref"]).to_lowercase())
                        };
                        let source_key = ref_of(source);
                        if let Some(other) = all.iter().find(|other| other.id != item.id && other.removed.is_none()
                            && other.source().is_some_and(|other| {
                                ref_of(other) == source_key || source.get("id").is_some_and(|id| other["id"] == *id)
                            }))
                        {
                            return Err(bad(format!("Another item is {} now, in the queue of {}: one item for each issue, so this one stays deleted.",
                                source["ref"].as_str().unwrap_or_default(), other.owner)));
                        }
                    }
                    append_event(saved, json!({"at":now,"by":who,"kind":"restored"}));
                    saved.as_object_mut().expect("queue item").remove("removed");
                    let status = if status_of(&Value::String(item.status.clone())).is_ok() {
                        item.status.as_str()
                    } else {
                        saved["status"] = "todo".into();
                        append_event(saved, json!({"at":now,"by":{"source":"kuvo","name":"Kuvo Code"},
                            "kind":"moved","from":item.status,"to":"todo"}));
                        "todo"
                    };
                    saved["order"] = json!(place(&all, Some(&item.id), status, false));
                }
            }
            saved["updated"] = stamp;
            writer.observe_known(saved, &item.id);
            let response = Item::read(&item.owner, saved).expect("saved queue item");
            let answer = if agent {
                let answer = tools::said(match removal {
                    Removal::Delete => format!("Deleted {display}. Bring it back within 30 days with queue_remove and undo."),
                    Removal::Restore => format!("Brought back {display}."),
                });
                tools::keep_reply(saved, operation, &answer);
                answer
            } else {
                view::mutation(&response, &Assignees::read(&writer.store))
            };
            file.save(&writer.store).map_err(storage_refusal)?;
            compaction.finish(&writer, now)?;
            Ok(answer)
        })
        .await
        .map_err(internal)?
    }

    fn admit(&self, owner: &str) -> Result<(), Refusal> {
        let view = self.peers.view();
        if view.local.ownership.frozen.contains(owner) {
            let message = if owner.starts_with("grp_") {
                "This group chat is moving to another computer, with its Work Queue: try again once it's there."
            } else {
                "That conversation is moving to another computer, with its Work Queue: try again once it's there."
            };
            return Err(bad(message));
        }
        let scope = if owner.starts_with("ses_") {
            RecordScope::Session
        } else if owner.starts_with("grp_") {
            RecordScope::Group
        } else {
            return Ok(());
        };
        if let Some(here) = view
            .owner_of(scope, owner, self.peers.now())
            .filter(|owner| !owner.local)
        {
            return Err(bad(format!(
                "Open this queue on {} to change it.",
                here.name
            )));
        }
        Ok(())
    }

    async fn lock(&self) -> Result<OwnedLock, Refusal> {
        let root = self.store.root.clone();
        blocking(move || {
            files::create_private_dirs(&root)?;
            migration::migrate(&root)?;
            Ok(store::exclusive(
                &root.join(".lock"),
                Duration::from_secs(3),
            )?)
        })
        .await
    }

    fn strict_items(&self, now: i64) -> Result<(Vec<String>, Vec<Item>), Refusal> {
        let mut owners = Vec::new();
        for file in queue_files(&self.store.root.join("queues")) {
            if let Some(owner) = file
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| owner_id(name))
            {
                self.read_owner(owner, now)?;
                owners.push(owner.to_owned());
            }
        }
        Ok((owners, self.store.items().map_err(storage_refusal)?))
    }

    fn read_owner(&self, owner: &str, now: i64) -> Result<Document, Refusal> {
        let path = self.store.root.join("queues").join(format!("{owner}.json"));
        match QUEUE.read(&path) {
            Ok(None) => Ok(Document {
                owner: owner.into(),
                path,
                items: Vec::new(),
            }),
            Ok(Some(record)) => {
                if let Some(items) = record.fields.get("items").and_then(Value::as_array) {
                    return Ok(Document {
                        owner: owner.into(),
                        path,
                        items: items.clone(),
                    });
                }
                Err(aside(&path, owner, now))
            }
            Err(kuvo_store::format::Error::Unreadable { source, .. }) => {
                if source.downcast_ref::<io::Error>().is_some() {
                    Err(bad(format!(
                        "{owner}'s Work Queue file can't be read just now, so nothing was changed: try again."
                    )))
                } else {
                    Err(aside(&path, owner, now))
                }
            }
            Err(error) => Err(storage_refusal(error.into())),
        }
    }

    fn receipt(&self, all: &[Item], id: &str) -> Result<Option<Value>, Refusal> {
        if let Some(item) = self.refs.find(all, &Reference::Id(id)) {
            return Ok(Some(
                item.saved
                    .get("creationReceipt")
                    .cloned()
                    .unwrap_or(Value::Null),
            ));
        }
        for family in [Fence::Deleted, Fence::Departed] {
            if let Some(record) = self.fence(family, id)? {
                return Ok(Some(
                    record
                        .get("creationReceipt")
                        .cloned()
                        .unwrap_or(Value::Null),
                ));
            }
        }
        Ok(None)
    }
}

async fn next_mail(
    incoming: &mut Option<tokio::sync::broadcast::Receiver<Arc<RelayEvent>>>,
) -> Option<Arc<RelayEvent>> {
    while let Some(receiver) = incoming.as_mut() {
        match receiver.recv().await {
            Ok(event) => return Some(event),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => *incoming = None,
        }
    }
    std::future::pending().await
}

async fn send(relay: &RelayHandle, peer: kuvo_wire::keys::Host, value: Value, now: i64) {
    let mail = Envelope(Fields::from_iter([
        ("workQueue".into(), value),
        ("at".into(), now.into()),
    ]));
    let _ = relay.sender.mail(peer, mail).await;
}

fn highest(items: &[Item]) -> u64 {
    items
        .iter()
        .filter_map(|item| item.number)
        .fold(0.0, f64::max) as u64
}

enum Removal {
    Delete,
    Restore,
}

enum Prepared {
    Replay(Value),
    New {
        item: Value,
        document: Document,
        assignees: Assignees,
        all: Vec<Item>,
        operation: String,
        digest: String,
        staged: Staged,
        compaction: Compaction,
        body_changed: bool,
    },
}

struct Document {
    owner: String,
    path: PathBuf,
    items: Vec<Value>,
}

impl Document {
    fn save(self, store: &Store) -> Result<(), Error> {
        let items = self
            .items
            .iter()
            .filter_map(|row| Item::read(&self.owner, row))
            .map(Item::normalized)
            .collect::<Vec<_>>();
        let record = QUEUE.encode(Fields::from_iter([
            ("owner".into(), self.owner.into()),
            ("items".into(), items.into()),
        ]));
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        let run = store.root.parent().expect("Work Queue under run");
        files::write_atomic_through(&self.path, &bytes, run).map_err(Into::into)
    }
}

fn aside(path: &Path, owner: &str, now: i64) -> Refusal {
    let folder = path.parent().expect("queue folder").join("aside");
    let aside = folder.join(format!("{owner}-{now}.json"));
    let kept = files::create_private_dirs(&folder).and_then(|()| fs::rename(path, &aside));
    if kept.is_ok() {
        bad(format!(
            "{owner}'s Work Queue file can't be read, so nothing was changed. It's kept at {} for you to look at; try again to carry on with an empty queue for {owner}.",
            aside.display()
        ))
    } else {
        bad(format!(
            "{owner}'s Work Queue file can't be read, so nothing was changed, and it couldn't be moved aside: look at {}.",
            path.display()
        ))
    }
}

fn nonce<'a>(headers: &'a HeaderMap, body: &'a Value) -> Result<&'a str, Refusal> {
    let invalid = || bad("Send a valid operation id.");
    let header = headers
        .get("x-kuvo-operation")
        .map(|value| value.to_str().map_err(|_| invalid()))
        .transpose()?;
    let field = body
        .get("operation")
        .map(|value| value.as_str().ok_or_else(invalid))
        .transpose()?;
    if let (Some(header), Some(field)) = (header, field)
        && header != field
    {
        return Err(invalid());
    }
    header
        .or(field)
        .filter(|id| {
            (1..=80).contains(&id.len())
                && id
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
        })
        .ok_or_else(invalid)
}

fn key(value: Value) -> String {
    kuvo_wire::keys::encode(&kuvo_wire::json::digest(&value))
}

fn event(item: &mut Value, now: i64, kind: &str) {
    append_event(item, json!({"at":now,"by":{"user":true},"kind":kind}));
}

pub(super) fn append_event(item: &mut Value, event: Value) {
    let mut events = store::array(item.get("events")).to_vec();
    let last = events.last_mut();
    let reorder = |event: &Value| event["kind"] == "moved" && event["from"] == event["to"];
    if let Some(last) = last
        && let (Some(before), Some(after)) = (last["at"].as_f64(), event["at"].as_f64())
        && after >= before
        && last["by"] == event["by"]
        && ["notes", "handed"].iter().all(|field| {
            !store::array(item.get(*field))
                .iter()
                .any(|entry| entry["at"].as_f64().is_some_and(|at| at >= before))
        })
    {
        if after - before < 10.0 * 60_000.0
            && reorder(last)
            && reorder(&event)
            && last["to"] == event["to"]
        {
            last["at"] = event["at"].clone();
            if event["top"] == true {
                last["top"] = true.into();
            } else if let Some(fields) = last.as_object_mut() {
                fields.remove("top");
            }
        } else if after - before < 2.0 * 60_000.0
            && last["kind"] == "stopped"
            && last.get("session").is_none()
            && event["kind"] == "started"
        {
            events.pop();
        } else {
            events.push(event);
        }
    } else {
        events.push(event);
    }
    events.drain(..events.len().saturating_sub(50));
    item["events"] = events.into();
}

fn place(all: &[Item], excluding: Option<&str>, status: &str, top: bool) -> f64 {
    let orders = all
        .iter()
        .filter(|item| {
            item.removed.is_none() && item.status == status && excluding != Some(item.id.as_str())
        })
        .map(|item| item.order);
    if top {
        orders.reduce(f64::min).map_or(0.0, |n| n - 1.0)
    } else {
        orders.reduce(f64::max).map_or(0.0, |n| n + 1.0)
    }
}

fn bad(message: impl Into<String>) -> Refusal {
    Refusal::new(StatusCode::BAD_REQUEST, message)
}

fn reused() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "That operation id is in use or its item has moved.",
    )
}

fn text(value: &Value, limit: usize, name: &str) -> Result<String, Refusal> {
    let value = value
        .as_str()
        .ok_or_else(|| bad(format!("{name} should be text.")))?;
    if value.encode_utf16().count() > limit {
        let shown = if limit == 100_000 {
            "100,000".into()
        } else {
            limit.to_string()
        };
        return Err(bad(format!(
            "{name} is over {shown} characters: make it shorter."
        )));
    }
    Ok(value.into())
}

pub(super) fn status_of(value: &Value) -> Result<&'static str, Refusal> {
    let words = value.as_str().unwrap_or_default().trim().to_lowercase();
    let words = words
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '_' | '-'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let alias = match words.as_str() {
        "next" | "to do" => "todo",
        "later" => "backlog",
        "cancelled" => "canceled",
        "waiting on you" | "needs user" => "needs-you",
        "review" => "in-review",
        "doing" => "in-progress",
        "blocked" => "waiting",
        _ => "",
    };
    STATUSES
        .iter()
        .find(|status| status.id == alias || status.id.replace('-', " ") == words)
        .map(|status| status.id)
        .ok_or_else(|| {
            let display = value.as_str().unwrap_or_default();
            let list = STATUSES
                .iter()
                .map(|status| status.name)
                .collect::<Vec<_>>()
                .join(", ");
            bad(format!("There's no status {display}: it's one of {list}."))
        })
}

fn priority_of(value: &Value) -> Result<Option<&'static str>, Refusal> {
    if value.is_null() {
        return Ok(None);
    }
    let name = value.as_str().unwrap_or_default().trim().to_lowercase();
    if matches!(name.as_str(), "normal" | "medium" | "none") {
        return Ok(None);
    }
    PRIORITIES
        .iter()
        .find(|priority| priority.id == name)
        .map(|priority| Some(priority.id))
        .ok_or_else(|| {
            bad(format!(
                "There's no priority {}: it's one of Urgent, High, Normal, Low.",
                value.as_str().unwrap_or_default()
            ))
        })
}

fn due_of(value: &Value) -> Result<Option<String>, Refusal> {
    if value.is_null() || value.as_str() == Some("") {
        return Ok(None);
    }
    let valid = value.as_str().map(str::trim).filter(|text| {
        text.len() == 10
            && text
                .parse::<jiff::civil::Date>()
                .is_ok_and(|day| day.to_string() == *text)
    });
    valid
        .map(|text| Some(text.to_owned()))
        .ok_or_else(|| bad("due is a day, like 2026-11-30 (or null for none)."))
}

fn labels_of(value: &Value, labels: &[QueueLabel]) -> Result<Vec<String>, Refusal> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let mut list = Vec::new();
    for item in value
        .as_array()
        .map_or_else(|| std::slice::from_ref(value), Vec::as_slice)
    {
        let wanted = item.as_str().unwrap_or_default().trim().to_lowercase();
        let label = labels
            .iter()
            .find(|label| label.id == wanted)
            .or_else(|| {
                labels
                    .iter()
                    .find(|label| label.name.to_lowercase() == wanted)
            })
            .ok_or_else(|| {
                bad(format!(
                    "There's no label {}: queue_labels lists them, and makes one.",
                    item.as_str().unwrap_or_default()
                ))
            })?;
        if !list.contains(&label.id) {
            list.push(label.id.clone());
        }
    }
    if list.len() > 10 {
        return Err(bad("An item carries 10 labels at most."));
    }
    Ok(list)
}

fn assignee(body: &Value, assignees: &Assignees) -> Result<Option<Value>, Refusal> {
    let Some(value) = body
        .get("assignee")
        .filter(|assignee| !assignee.is_null() && assignee.as_str() != Some(""))
    else {
        return Ok(None);
    };
    let all = assignees.rows();
    let found = if value.is_object() {
        all.iter()
            .find(|row| row["id"] == value["id"] && row["kind"] == value["kind"])
    } else if let Some(text) = value.as_str() {
        let text = text.trim();
        all.iter().find(|row| row["id"] == text).or_else(|| {
            all.iter().find(|row| {
                row["name"].as_str().is_some_and(|name| {
                    name.to_lowercase() == text.trim_start_matches('&').to_lowercase()
                })
            })
        })
    } else {
        None
    };
    if let Some(row) = found {
        return Ok(Some(json!({"id":row["id"],"kind":row["kind"]})));
    }
    let names = all
        .iter()
        .filter_map(|row| row["name"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let requested = value
        .as_str()
        .or_else(|| value["id"].as_str())
        .unwrap_or_default();
    Err(bad(format!(
        "There's no bot or coding agent {requested} here to assign it to. It can be: {names}."
    )))
}
