//! Quiet reads of an item or attachment on the computer that keeps it.

use super::{
    assignees::Assignees,
    attachments,
    github::Seen,
    mentions,
    refs::{Reference, References},
    routes::blocking,
    sessions::Sessions,
    store::{Item, Store},
    told, view,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use http::StatusCode;
use kuvo_engine::LocalSessions;
use kuvo_peers::{
    directory::{ComputerKey, Member},
    memory_read::{Answer, Handle, Kind, Read, Standing},
    setup_lists::Reader,
};
use kuvo_server::Refusal;
use serde_json::{Value, json};
use std::sync::Arc;

const MOST_FILE: usize = 4_000_000;
const ATTACHMENTS: &str = "work-queue.attachments";

#[derive(Clone)]
pub struct PeerReads {
    pub(super) github: super::github::Reader,
    pub(super) restore_labels: super::labels::Restores,
    pub(super) store: Store,
    pub(super) refs: Arc<References>,
    pub(super) sessions: LocalSessions,
    pub(super) peers: Reader,
    pub(super) transport: Handle,
}

struct Target {
    key: ComputerKey,
    computer: Value,
    item: Value,
    deleted: bool,
}

pub(super) struct QueueTarget {
    pub key: ComputerKey,
    pub computer: Value,
    pub id: String,
    pub previous: Value,
}

impl PeerReads {
    pub(super) async fn forward_target(
        &self,
        name: String,
        restoring: bool,
    ) -> Result<Option<QueueTarget>, Refusal> {
        let here = self.clone();
        let lookup = name.clone();
        let (try_elsewhere, target) = blocking(move || {
            let Some(reference) = here.refs.parse(&lookup) else {
                return Ok((false, None));
            };
            let all = here.store.items()?;
            if here.refs.find(&all, &reference).is_some() {
                return Ok((false, None));
            }
            Ok((true, here.told(&all, &reference, restoring)?))
        })
        .await?;
        if let Some(target) = target {
            return Ok(Some(QueueTarget {
                key: target.key,
                computer: target.computer,
                id: target.item["id"].as_str().unwrap_or_default().to_owned(),
                previous: target.item,
            }));
        }
        if !try_elsewhere {
            return Ok(None);
        }
        Ok(self.seek(name).await?.map(|(peer, answer)| QueueTarget {
            key: peer.key,
            computer: computer(&peer),
            id: answer["item"]["id"].as_str().unwrap_or_default().to_owned(),
            previous: answer["item"].clone(),
        }))
    }

    pub(super) fn known_owner(
        &self,
        all: &[super::store::Item],
        id: &str,
    ) -> Result<Option<QueueTarget>, super::store::Error> {
        Ok(self
            .told(all, &Reference::Id(id), true)?
            .map(|target| QueueTarget {
                key: target.key,
                computer: target.computer,
                id: id.into(),
                previous: target.item,
            }))
    }

    pub fn serves_ownership(&self, read: &Read) -> bool {
        read.area.as_deref() == Some("ownership") && read.item == super::labels::RESTORE_ROUTE
    }

    /// The shared memory transport only calls this for a GET from an accepted computer.
    pub async fn answer(&self, read: Read) -> Answer {
        if read.area.is_some() {
            if self.serves_ownership(&read) {
                return self.restore_labels.answer_peer(read).await;
            }
            return Answer {
                status: 404,
                value: json!({"message":"There is no such read."}),
            };
        }
        if matches!(read.kind, Kind::Record) {
            self.refresh_local(&read.item).await;
        }
        let here = self.clone();
        match blocking(move || {
            let mut all = here.store.items()?;
            let Some(item) = here.refs.parse(&read.item).and_then(|reference| here.refs.find(&all, &reference)) else {
                return Ok(missing_here(&here.peers.view().roster.name));
            };
            match read.kind {
                Kind::Record => {
                    let id = item.id.clone();
                    mentions::recover(&here.refs.mentions, &mut all);
                    let item = here.refs.find(&all, &Reference::Id(&id)).expect("selected queue item");
                    let value = here.detail(item, &all)?;
                    Ok(Answer { status: 200, value })
                }
                Kind::File { name } => {
                    let Some(file) = attachments::read(&here.store, item, &name) else {
                        return Ok(Answer { status: 404, value: json!({"message":"There's no such file."}) });
                    };
                    if file.bytes.len() > MOST_FILE {
                        let size = file.bytes.len().div_ceil(100_000) as f64 / 10.0;
                        let computer = &here.peers.view().roster.name;
                        return Ok(Answer { status: 413, value: json!({"message":format!("{name} is {size:.1} MB: too big to bring from {computer} (4 MB at most). Open it there.")}) });
                    }
                    Ok(Answer { status: 200, value: json!({"type":file.media_type,"data":STANDARD.encode(file.bytes)}) })
                }
            }
        }).await {
            Ok(answer) => answer,
            Err(_) => Answer { status: 500, value: json!({"message":"The Work Queue couldn't be read."}) },
        }
    }

    async fn refresh_local(&self, name: &str) {
        let here = self.clone();
        let name = name.to_owned();
        let source = blocking(move || {
            let all = here.store.items()?;
            Ok(here
                .refs
                .parse(&name)
                .and_then(|reference| here.refs.find(&all, &reference))
                .filter(|item| item.removed.is_none())
                .and_then(|item| item.source())
                .map(|source| json!({"source":source})))
        })
        .await;
        if let Ok(Some(source)) = source {
            self.github
                .read_seen(self.refs.clone(), vec![source], super::github::WAIT)
                .await;
        }
    }

    fn detail(&self, item: &Item, all: &[Item]) -> Result<Value, super::store::Error> {
        Ok(view::detail(
            item,
            all,
            &self.refs,
            &Seen::read(&self.store)?,
            &Sessions::read(self.sessions.latest_record()),
            &Assignees::read(&self.store),
        ))
    }

    pub(super) async fn item(&self, name: String, client: bool) -> Result<Value, Refusal> {
        self.item_view(name, client, true).await
    }

    pub(super) async fn item_for_tool(&self, name: String) -> Result<Value, Refusal> {
        self.item_view(name, true, false).await
    }

    async fn item_view(&self, name: String, client: bool, refresh: bool) -> Result<Value, Refusal> {
        if refresh {
            self.refresh_local(&name).await;
        }
        let here = self.clone();
        let lookup = name.clone();
        let (local, target) = blocking(move || {
            let Some(reference) = here.refs.parse(&lookup) else {
                return Ok((None, None));
            };
            let mut all = here.store.items()?;
            mentions::recover(&here.refs.mentions, &mut all);
            if let Some(item) = here.refs.find(&all, &reference) {
                return Ok((Some(here.detail(item, &all)?), None));
            }
            let far = here.told(&all, &reference, client)?;
            Ok((None, far))
        })
        .await?;
        if let Some(local) = local {
            return Ok(local);
        }
        if self.refs.parse(&name).is_none() {
            return Err(no_item());
        }
        if let Some(mut target) = target {
            if client && self.transport.standing(target.key, told::FEATURE) == Standing::Ready {
                let id = target.item["id"].as_str().unwrap_or_default().to_owned();
                if let Some(answer) = self
                    .remote(target.key, id, Kind::Record)
                    .await
                    .filter(|answer| answer.status == 200 && answer.value["item"].is_object())
                {
                    return Ok(self.item_there(
                        answer.value["item"].clone(),
                        target.computer,
                        target.key,
                    ));
                }
            }
            if refresh && target.item.get("source").is_some() {
                self.github
                    .read_seen(
                        self.refs.clone(),
                        vec![target.item.clone()],
                        super::github::WAIT,
                    )
                    .await;
                let here = self.clone();
                if let Ok(seen) = blocking(move || Seen::read(&here.store)).await {
                    seen.apply(&self.refs, &mut target.item);
                }
            }
            let peer = self.peers.view();
            let away = peer.standing(target.key, self.peers.now()).away();
            let can = view::can_elsewhere(&target.item, &peer, away, client);
            let fields = target.item.as_object_mut().expect("peer's item summary");
            fields.remove("handedFrom");
            for key in ["notes", "events", "files", "links", "handed"] {
                fields.insert(key.into(), json!([]));
            }
            fields.insert("body".into(), "".into());
            fields.insert("told".into(), true.into());
            fields.insert("can".into(), can);
            return Ok(json!({"item":target.item}));
        }
        if !client {
            return Err(no_item());
        }
        match self.seek(name).await? {
            Some((peer, answer)) => {
                Ok(self.item_there(answer["item"].clone(), computer(&peer), peer.key))
            }
            None => Err(no_item()),
        }
    }

    pub(super) async fn file(
        &self,
        reference: String,
        name: String,
        client: bool,
    ) -> Result<attachments::Download, Refusal> {
        let here = self.clone();
        let requested = name.clone();
        let lookup = reference.clone();
        let (local, far) = blocking(move || {
            let Some(named) = here.refs.parse(&lookup) else {
                return Ok((None, None));
            };
            let all = here.store.items()?;
            if let Some(item) = here.refs.find(&all, &named) {
                return Ok((Some(attachments::read(&here.store, item, &requested)), None));
            }
            Ok((None, here.told(&all, &named, false)?))
        })
        .await?;
        let missing = || Refusal::new(StatusCode::NOT_FOUND, "There's no such file.");
        if let Some(local) = local {
            return local.ok_or_else(missing);
        }
        if !client
            || self.refs.parse(&reference).is_none()
            || name.is_empty()
            || name.encode_utf16().count() > 300
        {
            return Err(missing());
        }
        let (peer, computer, id) = if let Some(target) = far {
            (
                target.key,
                target.computer,
                target.item["id"].as_str().unwrap_or_default().to_owned(),
            )
        } else if let Some((peer, value)) = self.seek(reference).await? {
            (
                peer.key,
                computer(&peer),
                value["item"]["id"].as_str().unwrap_or_default().to_owned(),
            )
        } else {
            return Err(missing());
        };
        let display = computer["name"].as_str().unwrap_or_default();
        let failed = |status, message: String| {
            Refusal::json(status, json!({"message":message,"computer":computer}))
        };
        let kind = Kind::File { name };
        let answer = self.remote(peer, id, kind).await.ok_or_else(|| failed(StatusCode::SERVICE_UNAVAILABLE,
            format!("{display} isn't answering now (it may be asleep, off or offline). Try again once it's back.")))?;
        if answer.status != 200 {
            let mut value = answer.value;
            if !value.is_object() {
                value = json!({});
            }
            value["computer"] = computer;
            return Err(Refusal::json(
                StatusCode::from_u16(answer.status).unwrap_or(StatusCode::BAD_GATEWAY),
                value,
            ));
        }
        let parts = answer.value["type"]
            .as_str()
            .zip(answer.value["data"].as_str());
        let download = parts.and_then(|(media_type, data)| {
            let bytes = STANDARD.decode(data).ok()?;
            Some(attachments::Download {
                media_type: media_type.to_owned(),
                bytes,
            })
        });
        download.ok_or_else(|| {
            failed(
                StatusCode::BAD_GATEWAY,
                format!("{display} answered something this computer can't read."),
            )
        })
    }

    fn told(
        &self,
        all: &[Item],
        reference: &Reference<'_>,
        deleted: bool,
    ) -> Result<Option<Target>, super::store::Error> {
        let view = self.peers.view();
        let far = told::items_elsewhere(
            &self.store,
            all,
            &view,
            "all",
            &Sessions::default(),
            &[],
            self.peers.now(),
        )?;
        let live = far
            .items
            .into_iter()
            .filter(|item| self.refs.names_summary(item, reference));
        let removed = far
            .removed
            .into_iter()
            .filter(|item| deleted && self.refs.names_summary(item, reference));
        let target = |(deleted, item): (bool, Value)| {
            let computer = item["computer"].clone();
            let key = kuvo_wire::keys::decode_exact(computer["key"].as_str()?)?;
            Some(Target {
                key,
                computer,
                item,
                deleted,
            })
        };
        Ok(live
            .map(|item| (false, item))
            .chain(removed.map(|item| (true, item)))
            .filter_map(target)
            .min_by(|a, b| {
                a.deleted.cmp(&b.deleted).then_with(|| {
                    b.item["updated"]["at"]
                        .as_f64()
                        .unwrap_or_default()
                        .total_cmp(&a.item["updated"]["at"].as_f64().unwrap_or_default())
                })
            }))
    }

    async fn remote(&self, peer: ComputerKey, item: String, kind: Kind) -> Option<Answer> {
        self.transport
            .request(
                peer,
                Read {
                    area: None,
                    item,
                    kind,
                    body: None,
                },
            )
            .await
            .ok()
    }

    fn item_there(&self, mut row: Value, computer: Value, key: ComputerKey) -> Value {
        row["computer"] = computer;
        if row.get("source").is_some() {
            let fields = row.as_object_mut().expect("remote item object");
            fields.remove("number");
            fields.remove("numberPending");
        } else if row["number"].is_null() {
            row["numberPending"] = true.into();
        }
        if row["can"].is_object() {
            row["can"]["assign"] = false.into();
            row["can"]["attach"] = (row["can"]["attach"] == true
                && self.peers.view().advertises(&key, ATTACHMENTS))
            .into();
        }
        row["told"] = false.into();
        json!({"item":row})
    }

    async fn seek(&self, name: String) -> Result<Option<(Member, Value)>, Refusal> {
        let Some(reference) = self
            .refs
            .parse(&name)
            .filter(|_| name.encode_utf16().count() <= 300)
        else {
            return Ok(None);
        };
        let view = self.peers.view();
        let here = &view.roster.name;
        if view.roster.members.is_empty() {
            return Ok(None);
        }
        if !view.relay_connected {
            return Err(Refusal::json(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({
                    "message": format!("It isn't on {here}, and {here} isn't connected to your other computers now, so it can't tell whether it's on one of them. Try again in a moment."),
                    "computers": view.roster.members.iter().map(computer).collect::<Vec<_>>(),
                }),
            ));
        }
        let mut pending = tokio::task::JoinSet::new();
        let mut away = Vec::<Member>::new();
        let mut older = Vec::<Member>::new();
        let mut refused = Vec::<(Member, String)>::new();
        let mut deleted: Option<(Member, Value)> = None;
        for peer in &view.roster.members {
            if !view.advertises(&peer.key, told::FEATURE)
                || !view.advertises(&peer.key, kuvo_peers::memory_read::FEATURE)
            {
                older.push(peer.clone());
                continue;
            }
            if self.transport.standing(peer.key, told::FEATURE) != Standing::Ready {
                away.push(peer.clone());
                continue;
            }
            let here = self.clone();
            let name = name.clone();
            let peer = peer.clone();
            pending.spawn(async move {
                let reply = here.remote(peer.key, name, Kind::Record).await;
                (peer, reply)
            });
        }
        let changed = |value: &Value| value["item"]["updated"]["at"].as_f64().unwrap_or_default();
        while let Some(result) = pending.join_next().await {
            let Ok((peer, reply)) = result else {
                continue;
            };
            if let Some(Answer { status: 200, value }) = reply.as_ref()
                && let Some(item) = value.get("item").filter(|item| {
                    item["id"].as_str().is_some_and(super::store::item_id)
                        && item["owner"].as_str().is_some_and(super::store::owner_id)
                        && self.refs.names_summary(item, &reference)
                })
            {
                if !item["removed"].is_object() {
                    return Ok(Some((peer, value.clone())));
                }
                if deleted
                    .as_ref()
                    .is_none_or(|(_, earlier)| changed(value) > changed(earlier))
                {
                    deleted = Some((peer, value.clone()));
                }
                continue;
            }
            match reply {
                Some(answer) if answer.status == 404 => {}
                Some(answer) => refused.push((
                    peer,
                    super::store::limit(answer.value["message"].as_str().unwrap_or_default(), 300),
                )),
                None => away.push(peer),
            }
        }
        if away.is_empty() && older.is_empty() && refused.is_empty() {
            return Ok(deleted);
        }
        let mut why = Vec::new();
        let mut again = String::new();
        if !away.is_empty() {
            let names = listed(&away);
            if away.len() == 1 {
                why.push(format!(
                    "{names} isn't answering now (it may be asleep, off or offline)"
                ));
                again = format!(" Try again once {names} answers.");
            } else {
                why.push(format!(
                    "{names} aren't answering now (they may be asleep, off or offline)"
                ));
                again = " Try again once they answer.".into();
            }
        }
        if !older.is_empty() {
            let verb = if older.len() == 1 { "needs" } else { "need" };
            why.push(format!(
                "{} {verb} a newer Kuvo Code to be asked from {here}",
                listed(&older)
            ));
        }
        why.extend(refused.iter().map(|(peer, said)| {
            if said.is_empty() {
                format!("{} couldn't say", peer.name)
            } else {
                format!("{} couldn't say (\"{said}\")", peer.name)
            }
        }));
        let computers = away
            .iter()
            .chain(&older)
            .chain(refused.iter().map(|(peer, _)| peer))
            .map(computer)
            .collect::<Vec<_>>();
        Err(Refusal::json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "message": format!("It isn't on {here}, and {}, so it may be there.{again}", why.join(", and ")),
                "computers": computers,
            }),
        ))
    }
}

fn listed(peers: &[Member]) -> String {
    let Some((last, before)) = peers.split_last() else {
        return String::new();
    };
    if before.is_empty() {
        return last.name.clone();
    }
    format!(
        "{} and {}",
        before
            .iter()
            .map(|peer| peer.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        last.name
    )
}

fn computer(peer: &Member) -> Value {
    json!({"key":kuvo_wire::keys::encode(&peer.key), "name":peer.name, "kind":peer.kind})
}

fn no_item() -> Refusal {
    Refusal::new(StatusCode::NOT_FOUND, "There's no such item.")
}

fn missing_here(name: &str) -> Answer {
    Answer {
        status: 404,
        value: json!({"message":format!("It's no longer on {name}: it may have moved. Look again in a moment.")}),
    }
}
