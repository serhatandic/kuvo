//! The retained snapshot a computer last sent to the other setup members.

use super::{
    labels, status,
    store::{self, Error, Store},
};
use kuvo_peers::{
    directory::ComputerKey, reach::Standing, relay::RelayHandle, session::ComputerKind,
    setup_lists::View, work_queue_mails,
};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
    locks::{OnBusy, StaleLock},
};
use kuvo_wire::keys::encode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::PathBuf,
    time::{Duration, SystemTime},
};

const FORMAT: ObjectFormat = ObjectFormat::additive("Work Queue peer summary");
pub(super) use kuvo_peers::WORK_QUEUE_SNAPSHOTS as FEATURE;
pub(super) const MOST_ITEMS: usize = 1500;
pub(super) const MOST_DONE: usize = 100;

pub(super) type StatusCounts = BTreeMap<String, usize>;

#[derive(Deserialize, Serialize)]
pub(super) struct Told {
    pub id: String,
    pub at: i64,
    pub items: Vec<Value>,
    // Counts of live rows excluded from the summary, not the rows themselves.
    #[serde(
        default,
        rename = "omittedByStatus",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub omitted_by_status: StatusCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<Vec<Value>>,
    pub owners: Vec<Value>,
    pub labels: Vec<kuvo_setup::QueueLabel>,
    #[serde(flatten)]
    pub numbering: Numbering,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Numbering {
    pub kind: ComputerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<Authority>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Authority {
    pub key: String,
    pub kind: ComputerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    #[serde(flatten)]
    told: Told,
    heard: i64,
    latest_at: Option<i64>,
}

fn path(store: &Store, sender: &ComputerKey) -> PathBuf {
    store
        .root
        .join("peers")
        .join(format!("{}.json", encode(sender)))
}

fn read(store: &Store, sender: &ComputerKey) -> Result<Option<Kept>, Error> {
    Ok(store::read(&path(store, sender), FORMAT)?
        .and_then(|fields| serde_json::from_value(Value::Object(fields)).ok()))
}

/// Numbers in the saved views are used to distinguish a prior import from a collision.
pub(super) fn known_numbers(
    store: &Store,
) -> Result<std::collections::BTreeMap<u64, String>, Error> {
    let mut found = std::collections::BTreeMap::new();
    for sender in store::queue_files(&store.root.join("peers"))
        .iter()
        .filter_map(|file| kuvo_wire::keys::decode_exact(file.file_stem()?.to_str()?))
    {
        if let Some(kept) = read(store, &sender)? {
            for item in kept
                .told
                .items
                .into_iter()
                .filter(|item| item["source"].is_null())
            {
                if let (Some(number), Some(title)) = (
                    store::positive_integer(&item["number"]),
                    item["title"].as_str(),
                ) {
                    found.insert(number as u64, title.into());
                }
            }
        }
    }
    Ok(found)
}

/// Keep late mail only if the sender reset its clock. Acknowledge duplicates as well.
pub(super) fn keep(
    store: &Store,
    sender: &ComputerKey,
    given: &Value,
    now: i64,
) -> Result<Option<String>, Error> {
    let Some(told) = incoming(given, now) else {
        return Ok(None);
    };
    let _lock = StaleLock::acquire(
        &store.root.join(".told.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    let before = read(store, sender)?;
    if let Some(before) = &before
        && before.told.at > told.at
        && before.told.at - told.at < 600_000
        && before.told.id != told.id
    {
        return Ok(None);
    }
    let id = told.id.clone();
    let latest_at = before.map_or(told.at, |before| {
        told.at
            .max(before.told.at)
            .max(before.latest_at.unwrap_or(told.at))
    });
    let kept = Kept {
        told,
        heard: now,
        latest_at: Some(latest_at),
    };
    let fields = serde_json::from_value::<Fields>(json!(kept)).map_err(io::Error::other)?;
    let bytes = serde_json::to_vec(&FORMAT.encode(fields)).map_err(io::Error::other)?;
    files::write_secret(&path(store, sender), &bytes)?;
    Ok(Some(id))
}

fn incoming(raw: &Value, now: i64) -> Option<Told> {
    let id = raw["id"].as_str()?;
    if !id.strip_prefix("wqt_").is_some_and(|token| {
        (8..=40).contains(&token.len())
            && token
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
    }) {
        return None;
    }
    let at = raw["at"].as_i64()?.min(now);
    let incoming_items = raw["items"].as_array()?;
    let incoming_labels = raw["labels"].as_array()?;
    let mut ids = HashSet::<String>::new();
    let mut rows = |array: &[Value], removed: bool| {
        array
            .iter()
            .take(MOST_ITEMS)
            .filter_map(|item| summary(item, at, removed))
            .filter(|item| {
                item["id"]
                    .as_str()
                    .is_some_and(|id| ids.insert(id.to_owned()))
            })
            .collect::<Vec<_>>()
    };
    let items = rows(incoming_items, false);
    let removed = raw["removed"].as_array().map(|removed| rows(removed, true));
    let owners = store::array(raw.get("owners"))
        .iter()
        .take(MOST_ITEMS)
        .filter_map(owner)
        .collect();
    let labels = incoming_labels
        .iter()
        .take(60)
        .filter_map(|raw| {
            labels::clean_label(
                raw["id"].as_str()?,
                raw["name"].as_str()?,
                raw["color"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    let kind = kind(&raw["kind"]);
    let primary = raw.get("primary").and_then(|value| {
        let key = value["key"].as_str()?;
        kuvo_wire::keys::decode_exact::<32>(key)?;
        let kind = match value["kind"].as_str()? {
            "mac" => ComputerKind::Mac,
            "machine" => ComputerKind::Machine,
            _ => return None,
        };
        Some(Authority {
            key: key.into(),
            kind,
            since: positive_time(&value["since"]),
        })
    });
    Some(Told {
        id: id.into(),
        at,
        items,
        omitted_by_status: status::STATUSES
            .iter()
            .filter_map(|status| {
                let count = raw["omittedByStatus"][status.id].as_u64()?;
                (count > 0 && count <= u32::MAX as u64)
                    .then(|| (status.id.to_owned(), count as usize))
            })
            .collect(),
        removed,
        owners,
        labels,
        numbering: Numbering {
            kind,
            primary,
            since: positive_time(&raw["since"]),
            top: number(&raw["top"]),
        },
    })
}

fn kind(value: &Value) -> ComputerKind {
    if value.as_str() == Some("mac") {
        ComputerKind::Mac
    } else {
        ComputerKind::Machine
    }
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| (1..=999_999).contains(n))
}

fn positive_time(value: &Value) -> Option<i64> {
    value.as_i64().filter(|time| *time > 0)
}

fn words(value: &Value, max: usize) -> String {
    store::limit(&store::plain_name(value.as_str().unwrap_or_default()), max)
}

fn owner(raw: &Value) -> Option<Value> {
    let id = raw["owner"].as_str().filter(|id| store::owner_id(id))?;
    let kind = raw["kind"]
        .as_str()
        .filter(|kind| matches!(*kind, "bot" | "you" | "group"))
        .unwrap_or("session");
    let title = raw["title"].is_string().then(|| words(&raw["title"], 200));
    let mut row = json!({"owner":id,"kind":kind,"title":title});
    if let Some(id) = raw["bot"]["id"]
        .as_str()
        .filter(|id| valid_token(id, "bot_"))
    {
        row["bot"] = json!({"id":id,"name":words(&raw["bot"]["name"], 100)});
    }
    if let Some(id) = raw["startedBy"]
        .as_str()
        .filter(|id| valid_token(id, "ses_"))
    {
        row["startedBy"] = id.into();
    }
    Some(row)
}

fn valid_token(id: &str, begins: &str) -> bool {
    id.strip_prefix(begins).is_some_and(|tail| {
        (1..=60).contains(&tail.len()) && tail.bytes().all(|ch| ch.is_ascii_alphanumeric())
    })
}

fn at(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0).max(0.0)
}

fn stamp(raw: &Value) -> Value {
    let by = raw
        .get("by")
        .and_then(store::who)
        .unwrap_or_else(|| json!({"name":"someone else"}));
    json!({"at":at(&raw["at"]),"by":by})
}

fn summary(raw: &Value, latest: i64, removed: bool) -> Option<Value> {
    let id = raw["id"].as_str().filter(|id| store::item_id(id))?;
    let owner = raw["owner"].as_str().filter(|id| store::owner_id(id))?;
    if raw["removed"].is_object() != removed {
        return None;
    }
    let title = words(&raw["title"], 200);
    let title = if title.is_empty() { "Untitled" } else { &title };
    let status = raw["status"]
        .as_str()
        .filter(|id| status::STATUSES.iter().any(|status| status.id == *id))
        .unwrap_or("todo");
    let priority = raw["priority"]
        .as_str()
        .filter(|id| status::PRIORITIES.iter().any(|priority| priority.id == *id))
        .unwrap_or("normal");
    let labels = store::array(raw.get("labels"))
        .iter()
        .filter_map(Value::as_str)
        .filter(|id| store::label_id(id))
        .take(20)
        .collect::<Vec<_>>();
    let sessions = store::array(raw.get("sessions")).iter().filter_map(|entry| {
        let id = entry["session"].as_str().filter(|id| valid_token(id, "ses_"))?;
        let title = words(&entry["title"], 200);
        Some(json!({"session":id,"title":if title.is_empty() { "New Session" } else { &title },"working":entry["working"] == true}))
    }).take(20).collect::<Vec<_>>();
    let count = |key: &str| raw[key].as_u64().unwrap_or_default();
    let mut row = json!({"id":id,"owner":owner,"title":title,"status":status,"priority":priority,"order":raw["order"].as_f64().unwrap_or_default(),"labels":labels,"sessions":sessions,
        "created":stamp(&raw["created"]), "updated":stamp(&raw["updated"]), "fileCount":count("fileCount"), "noteCount":count("noteCount"), "linkCount":count("linkCount")});
    if raw["source"]["kind"].is_string() && raw["source"]["ref"].is_string() {
        let source = ["kind", "ref", "url", "host", "id", "name"]
            .into_iter()
            .filter_map(|key| {
                let text = raw["source"][key].as_str()?;
                if key == "url"
                    && raw["source"]["kind"]
                        .as_str()
                        .is_some_and(|kind| kind.starts_with("github"))
                    && !text.starts_with("https://github.com/")
                {
                    return None;
                }
                Some((key.to_owned(), Value::String(store::limit(text, 2000))))
            })
            .collect::<Map<_, _>>();
        row["source"] = source.into();
        let seen = &raw["seen"];
        if let Some(state) = seen["state"].as_str().filter(|state| {
            (1..=20).contains(&state.len())
                && state
                    .bytes()
                    .all(|ch| ch.is_ascii_lowercase() || ch == b'_')
        }) {
            let mut read = json!({"state":state,"at":at(&seen["at"]).min(latest as f64)});
            if seen["checked"].is_number() {
                read["checked"] = json!(at(&seen["checked"]).min(latest as f64));
            }
            if seen["gone"] == true {
                read["gone"] = true.into();
            }
            row["seen"] = read;
            if raw["keptIn"]
                .as_str()
                .is_some_and(|id| status::STATUSES.iter().any(|status| status.id == id))
            {
                row["keptIn"] = raw["keptIn"].clone();
            }
        }
    } else if let Some(number) = number(&raw["number"]) {
        row["number"] = number.into();
    } else {
        row["numberPending"] = true.into();
    }
    if let Some(aliases) = raw["numberAliases"].as_array() {
        let sanitized: Vec<_> = aliases.iter().filter_map(|alias| {
            let setup = alias["setup"].as_str()?;
            let number = number(&alias["number"])?;
            let token = setup.strip_prefix("set_")?;
            if token.len() != 22 || !token.bytes().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-')) { return None; }
            let name = words(&alias["name"], 100);
            Some(json!({"setup":setup,"name":if name.is_empty(){"Earlier setup"}else{&name},"number":number}))
        }).take(20).collect();
        row["numberAliases"] = json!(sanitized);
    }
    let mut progress = Map::new();
    if let Some(value) = raw["progress"]["value"]
        .as_f64()
        .filter(|value| (0.0..=1.0).contains(value))
    {
        progress.insert("value".into(), json!(value));
    }
    if raw["progress"]["note"].is_string() {
        progress.insert("note".into(), words(&raw["progress"]["note"], 300).into());
    }
    if !progress.is_empty() {
        row["progress"] = progress.into();
    }
    if let Some(wanted) = number(&raw["wanted"]) {
        row["wanted"] = wanted.into();
    }
    if raw["doneAt"].is_number() {
        row["doneAt"] = json!(at(&raw["doneAt"]));
    }
    if removed {
        row["removed"] = stamp(&raw["removed"]);
    }
    if let (Some(kind @ ("agent" | "bot")), Some(id)) = (
        raw["assignee"]["kind"].as_str(),
        raw["assignee"]["id"].as_str(),
    ) {
        let name = words(&raw["assignee"]["name"], 100);
        row["assignee"] =
            json!({"kind":kind,"id":id,"name":if name.is_empty() { id } else { &name }});
    }
    if let Some(due) = raw["due"].as_str().map(str::trim).filter(|due| {
        due.parse::<jiff::civil::Date>()
            .is_ok_and(|date| date.to_string() == *due)
    }) {
        row["due"] = due.into();
    }
    if let Some(note) = raw["lastNote"]["text"].as_str() {
        let mut whole = stamp(&raw["lastNote"]);
        whole["text"] = store::limit(note, 500).into();
        row["lastNote"] = whole;
    }
    if let Some(id) = raw["handedBy"].as_str().filter(|id| store::owner_id(id)) {
        row["handedBy"] = id.into();
    }
    if let Some(id) = raw["handing"]["to"]
        .as_str()
        .filter(|id| store::owner_id(id))
    {
        row["handing"] = json!({"to":id,"name":words(&raw["handing"]["name"], 100)});
    }
    for (field, valid, max) in [
        ("handedFrom", store::owner_id as fn(&str) -> bool, 20),
        ("mentionTargets", store::item_id, usize::MAX),
    ] {
        if let Some(values) = raw[field].as_array() {
            let mut seen = HashSet::new();
            row[field] = json!(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|id| valid(id) && seen.insert(*id))
                    .take(max)
                    .collect::<Vec<_>>()
            );
        }
    }
    Some(row)
}

#[derive(Default)]
pub(super) struct Elsewhere {
    pub items: Vec<Value>,
    pub removed: Vec<Value>,
    pub owners: Vec<Value>,
    pub labels: Vec<kuvo_setup::QueueLabel>,
    pub computers: Vec<Value>,
    pub pending: Vec<Value>,
    pub omitted_by_computer: BTreeMap<String, StatusCounts>,
}

pub(super) fn items_elsewhere(
    store: &Store,
    local: &[store::Item],
    view: &View,
    owner_id: &str,
    sessions: &super::sessions::Sessions,
    retired_labels: &[String],
    now: i64,
) -> Result<Elsewhere, Error> {
    let live: HashSet<_> = local
        .iter()
        .filter(|item| item.removed.is_none())
        .map(|item| item.id.as_str())
        .collect();
    let deleted: HashSet<_> = local
        .iter()
        .filter(|item| item.removed.is_some())
        .map(|item| item.id.as_str())
        .collect();
    let mut merged = Elsewhere::default();
    let mut seen = HashSet::new();
    let mut owners = HashSet::new();
    let mut labels = HashSet::new();
    let mut index = HashMap::new();
    let mut items: Vec<(i64, Value)> = Vec::new();
    for other in &view.roster.members {
        let Some(kept) = read(store, &other.key)? else {
            continue;
        };
        seen.insert(other.key);
        let key = encode(&other.key);
        let computer = json!({"key":key,"name":other.name,"kind":other.kind});
        // A Mac's or machine's roster name follows accepted setup changes rather than the cached row.
        let standing = view.standing(other.key, now);
        let mut copy = computer.clone();
        copy["at"] = kept.told.at.into();
        if let Standing::Away { last_seen } = standing {
            copy["away"] = last_seen.unwrap_or(kept.heard).into();
        }
        merged.computers.push(copy);
        if owner_id == "all" && !kept.told.omitted_by_status.is_empty() {
            merged
                .omitted_by_computer
                .insert(key, kept.told.omitted_by_status);
        }
        for mut item in kept
            .told
            .items
            .into_iter()
            .chain(kept.told.removed.into_iter().flatten())
        {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            if live.contains(id)
                || (deleted.contains(id)
                    && (item.get("removed").is_some() || item.get("handing").is_some()))
            {
                continue;
            }
            let id = id.to_owned();
            let newer = |lhs: &Value, a: i64, rhs: &Value, b: i64| {
                let changed_at =
                    |item: &Value| at(&item["updated"]["at"]).max(at(&item["removed"]["at"]));
                changed_at(lhs) > changed_at(rhs) || (changed_at(lhs) == changed_at(rhs) && a > b)
            };
            item["computer"] = computer.clone();
            if standing.away()
                && let Some(sessions) = item["sessions"].as_array_mut()
            {
                for session in sessions {
                    session["working"] = false.into();
                }
            }
            if let Some(&position) = index.get(&id) {
                let (at, item_at): &mut (i64, Value) = &mut items[position];
                if newer(&item, kept.told.at, item_at, *at) {
                    *at = kept.told.at;
                    *item_at = item;
                }
            } else {
                index.insert(id, items.len());
                items.push((kept.told.at, item));
            }
        }
        for owner in kept.told.owners {
            if let Some(id) = owner["owner"]
                .as_str()
                .filter(|id| owners.insert(id.to_string()))
            {
                let _ = id;
                merged.owners.push(owner);
            }
        }
        for label in kept.told.labels {
            if !retired_labels.contains(&label.id) && labels.insert(label.id.clone()) {
                merged.labels.push(label);
            }
        }
    }
    let locally_visible = |item: &Value| {
        owner_id == "all"
            || item["owner"] == owner_id
            || item["handedBy"] == owner_id
            || store::array(item.get("handedFrom"))
                .iter()
                .any(|id| id == owner_id)
            || (owner_id.starts_with("ses_")
                && item["owner"]
                    .as_str()
                    .is_some_and(|owned| sessions.under(owner_id, owned)))
            || (item.get("removed").is_none()
                && store::array(item.get("sessions"))
                    .iter()
                    .any(|session| session["session"] == owner_id))
    };
    let mut named = HashSet::new();
    for (_, item) in items.into_iter().filter(|(_, item)| locally_visible(item)) {
        if let Some(owner) = item["owner"].as_str() {
            named.insert(owner.to_string());
        }
        if let Some(owner) = item["handedBy"].as_str() {
            named.insert(owner.to_string());
        }
        if item.get("removed").is_some() {
            merged.removed.push(item);
        } else {
            merged.items.push(item);
        }
    }
    merged
        .owners
        .retain(|row| row["owner"].as_str().is_some_and(|id| named.contains(id)));
    merged.pending = view
        .pending(FEATURE, |key| seen.contains(key))
        .into_iter()
        .map(|entry| json!(entry))
        .collect();
    Ok(merged)
}

/// Editions of the local snapshot, sent on the normal peer-mail path.
#[derive(Default)]
pub(super) struct Delivery {
    sent: HashMap<ComputerKey, DeliveryTo>,
    current: Option<Snapshot>,
}

struct Snapshot {
    told: Told,
    marks: Vec<Option<SystemTime>>,
}

struct DeliveryTo {
    id: String,
    sent: i64,
    kept: bool,
}

impl Delivery {
    pub fn acknowledge(&mut self, peer: &ComputerKey, id: &str) {
        if let Some(sent) = self.sent.get_mut(peer).filter(|sent| sent.id == id) {
            sent.kept = true;
        }
    }

    pub fn due(&self, peer: &ComputerKey, id: &str, now: i64) -> bool {
        let Some(before) = self.sent.get(peer) else {
            return true;
        };
        now.saturating_sub(before.sent) >= 15_000
            && (before.id != id || (!before.kept && now.saturating_sub(before.sent) >= 60_000))
    }

    pub fn sent(&mut self, peer: ComputerKey, id: String, now: i64) {
        self.sent.insert(
            peer,
            DeliveryTo {
                id,
                sent: now,
                kept: false,
            },
        );
    }

    pub async fn publish(
        &mut self,
        reads: &super::peer_reads::PeerReads,
        relay: &RelayHandle,
        setup: Option<kuvo_setup::QueueSetup>,
        now: i64,
    ) -> Result<(), Error> {
        let super::peer_reads::PeerReads {
            store,
            peers,
            refs,
            sessions,
            ..
        } = reads;
        let view = peers.view();
        let targets: Vec<_> = view
            .roster
            .members
            .iter()
            .filter(|peer| {
                view.advertises(&peer.key, FEATURE) && !view.standing(peer.key, now).away()
            })
            .filter_map(|peer| peer.host())
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        let store = store.clone();
        let sessions = sessions.clone();
        let refs = refs.clone();
        let before = self
            .current
            .as_ref()
            .map(|before| (before.told.at, before.marks.clone()));
        let changed = tokio::task::spawn_blocking(move || {
            let marks = [
                "queues",
                "labels.json",
                "numbers.json",
                "peers",
                "seen.json",
            ]
            .map(|name| {
                std::fs::metadata(store.root.join(name))
                    .and_then(|file| file.modified())
                    .ok()
            })
            .to_vec();
            if before
                .as_ref()
                .is_some_and(|(at, before)| before == &marks && now.saturating_sub(*at) <= 60_000)
            {
                return Ok::<_, Error>(None);
            }
            let sessions = super::sessions::Sessions::read(sessions.latest_record());
            let told = outgoing(&store, &refs, &sessions, &view, setup.as_ref(), now)?;
            Ok(Some(Snapshot { told, marks }))
        })
        .await
        .map_err(io::Error::other)??;
        if let Some(latest) = changed {
            self.current = Some(latest);
        }
        let Some(told) = self.current.as_ref().map(|current| &current.told) else {
            return Ok(());
        };
        let id = told.id.clone();
        let targets: Vec<_> = targets
            .into_iter()
            .filter(|peer| self.due(&peer.key, &id, now))
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        let Some(mails) = fitted(told, now) else {
            return Ok(());
        };
        for target in targets {
            let mut sent = true;
            for mail in &mails {
                if relay
                    .sender
                    .mail(target.clone(), mail.clone())
                    .await
                    .is_err()
                {
                    sent = false;
                    break;
                }
            }
            if sent {
                self.sent(target.key, id.clone(), now);
            }
        }
        Ok(())
    }
}

fn fitted(told: &Told, now: i64) -> Option<Vec<kuvo_wire::mail::Envelope>> {
    let mut body = serde_json::to_value(told).ok()?;
    let send = |body: &Value| work_queue_mails(json!({"told":body}), now);
    if let Some(full) = send(&body) {
        return Some(full);
    }
    if let Some(removed) = &told.removed {
        for most in [500, 100, 20, 0]
            .into_iter()
            .filter(|most| *most < removed.len())
        {
            if most == 0 {
                body.as_object_mut()?.remove("removed");
            } else {
                body["removed"] = json!(&removed[..most]);
            }
            if let Some(fitted) = send(&body) {
                return Some(fitted);
            }
        }
    }
    for most in [1000, 500, 200, 50]
        .into_iter()
        .filter(|most| *most < told.items.len())
    {
        body["items"] = json!(&told.items[..most]);
        let mut omitted = told.omitted_by_status.clone();
        count_omitted(&mut omitted, &told.items[most..]);
        body["omittedByStatus"] = json!(omitted);
        if let Some(fitted) = send(&body) {
            return Some(fitted);
        }
    }
    None
}

fn count_omitted<'a>(counts: &mut StatusCounts, rows: impl IntoIterator<Item = &'a Value>) {
    for row in rows {
        if let Some(status) = row["status"].as_str() {
            *counts.entry(status.to_owned()).or_default() += 1;
        }
    }
}

pub(super) fn outgoing(
    store: &Store,
    refs: &super::refs::References,
    sessions: &super::sessions::Sessions,
    view: &View,
    setup: Option<&kuvo_setup::QueueSetup>,
    now: i64,
) -> Result<Told, Error> {
    use super::{
        assignees::Assignees,
        github::Seen,
        view::{owner, summary},
    };
    use sha2::{Digest, Sha256};
    let all = store.items()?;
    let owners = Assignees::read(store);
    let seen = Seen::read(store)?;
    let now_date = jiff::Zoned::now().date();
    let mut open = Vec::new();
    let mut done = Vec::new();
    let mut removed = Vec::new();
    for item in &all {
        let mut row = summary(item, now_date, refs, &seen, sessions, &owners);
        if row.get("seen").is_some()
            && let Some(checked) = item
                .source()
                .and_then(|source| seen.last_checked(refs, source))
        {
            row["seen"]["checked"] = checked;
        }
        let fields = row.as_object_mut().expect("queue summary");
        for local in ["can", "overdue", "doneAs", "struck", "closesOn"] {
            fields.remove(local);
        }
        if let Some(assignee) = fields.get_mut("assignee").and_then(Value::as_object_mut) {
            assignee.remove("look");
        }
        if let Some(note) = fields.get_mut("lastNote").and_then(Value::as_object_mut)
            && let Some(text) = note["text"].as_str()
        {
            note.insert("text".into(), store::limit(text, 500).into());
        }
        let mut handed_from = HashSet::new();
        let handed: Vec<_> = item
            .handed
            .iter()
            .filter_map(|hand| hand["from"].as_str())
            .filter(|id| handed_from.insert(*id))
            .collect();
        if !handed.is_empty() {
            fields.insert("handedFrom".into(), json!(handed));
        }
        let mention_targets = store::array(
            item.saved
                .get("mentionIndex")
                .and_then(|index| index.get("targets")),
        );
        if !mention_targets.is_empty() {
            fields.insert("mentionTargets".into(), json!(mention_targets));
        }
        if let Some(removed_at) = item.removed.as_ref().map(|stamp| stamp.at) {
            removed.push((removed_at, row));
        } else if row["status"].as_str().is_some_and(status::closed) {
            done.push((at(&row["doneAt"]), row));
        } else {
            open.push((item.updated.at, row));
        }
    }
    open.sort_by(|a, b| b.0.total_cmp(&a.0));
    done.sort_by(|a, b| b.0.total_cmp(&a.0));
    removed.sort_by(|a, b| b.0.total_cmp(&a.0));
    let most_open = MOST_ITEMS - done.len().min(MOST_DONE);
    let mut omitted_by_status = StatusCounts::new();
    count_omitted(
        &mut omitted_by_status,
        open.iter()
            .skip(most_open)
            .chain(done.iter().skip(MOST_DONE))
            .map(|(_, row)| row),
    );
    let items = open
        .into_iter()
        .take(most_open)
        .chain(done.into_iter().take(MOST_DONE))
        .map(|(_, row)| row)
        .collect::<Vec<_>>();
    let removed = removed
        .into_iter()
        .take(MOST_ITEMS)
        .map(|(_, row)| row)
        .collect::<Vec<_>>();
    let mut named = HashSet::new();
    let owner_rows = items
        .iter()
        .chain(&removed)
        .flat_map(|row| {
            [row["owner"].as_str(), row["handedBy"].as_str()]
                .into_iter()
                .flatten()
        })
        .filter(|id| named.insert(*id))
        .map(|id| json!(owner(id, sessions, &owners)))
        .collect::<Vec<_>>();
    let kind = if cfg!(target_os = "macos") {
        ComputerKind::Mac
    } else {
        ComputerKind::Machine
    };
    let numbers = store::read(
        &store.root.join("numbers.json"),
        ObjectFormat::additive("Work Queue number ledger"),
    )?
    .unwrap_or_default();
    let top = numbers
        .get("next")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .saturating_sub(1)
        .max(
            ["leases", "used"]
                .iter()
                .flat_map(|field| store::array(numbers.get(*field)))
                .filter_map(|range| range[1].as_u64())
                .max()
                .unwrap_or_default(),
        )
        .max(
            all.iter()
                .filter_map(|item| item.number)
                .fold(0.0, f64::max) as u64,
        );
    let since = numbers
        .get("since")
        .and_then(positive_time)
        .unwrap_or_else(|| {
            let birth = store.root.parent().and_then(fs_birth).unwrap_or(now);
            all.iter()
                .map(|item| item.created.at as i64)
                .filter(|at| *at > 0)
                .fold(now.min(birth), i64::min)
        });
    let primary = setup.and_then(|setup| {
        let key = setup.primary.clone();
        if setup.main_here {
            return Some(Authority {
                key,
                kind,
                since: Some(since),
            });
        }
        let decoded = kuvo_wire::keys::decode_exact::<32>(&key)?;
        let member = view.roster.member(&decoded)?;
        let since = read(store, &decoded)
            .ok()
            .flatten()
            .and_then(|read| read.told.numbering.since);
        Some(Authority {
            key,
            kind: member.kind,
            since,
        })
    });
    let labels = labels::shown(store, None)?;
    let removed = (!removed.is_empty()).then_some(removed);
    let numbering = Numbering {
        kind,
        since: Some(since),
        top: Some(top),
        primary,
    };
    let mut report = Told {
        id: String::new(),
        at: now,
        items,
        omitted_by_status,
        removed,
        owners: owner_rows,
        labels,
        numbering,
    };
    let mut content = serde_json::to_value(&report).map_err(io::Error::other)?;
    let fields = content.as_object_mut().expect("snapshot object");
    fields.remove("id");
    fields.remove("at");
    if let Some(rows) = fields.get_mut("items").and_then(Value::as_array_mut) {
        for item in rows {
            if let Some(seen) = item.get_mut("seen").and_then(Value::as_object_mut) {
                seen.remove("checked");
            }
        }
    }
    let bytes = kuvo_wire::json::ordinary(&content);
    report.id = format!("wqt_{}", &encode(&Sha256::digest(bytes.as_bytes()))[..22]);
    Ok(report)
}

fn fs_birth(path: &std::path::Path) -> Option<i64> {
    let since = std::fs::metadata(path)
        .ok()?
        .created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// Apply the keeper's acknowledged mutation to its saved snapshot, unless a newer one arrived first.
pub(super) fn apply_reply(
    store: &Store,
    sender: &ComputerKey,
    item: &Value,
    deleted: bool,
    before: &Value,
) -> Result<(), Error> {
    let Some(fresh) = summary(item, i64::MAX, deleted) else {
        return Ok(());
    };
    let Some(id) = fresh["id"].as_str() else {
        return Ok(());
    };
    let _lock = StaleLock::acquire(
        &store.root.join(".told.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    let file = path(store, sender);
    let Some(mut fields) = store::read(&file, FORMAT)? else {
        return Ok(());
    };
    let formerly = store::array(fields.get("items"));
    let formerly_deleted = store::array(fields.get("removed"));
    let previous = formerly
        .iter()
        .chain(formerly_deleted)
        .find(|row| row["id"] == id);
    let changed = |row: &Value| at(&row["updated"]["at"]).max(at(&row["removed"]["at"]));
    if previous.is_some_and(|previous| changed(previous) > changed(&fresh)) {
        return Ok(());
    }
    // A direct lookup can reach rows that were counted but not listed. Move
    // such a row out of the aggregate when adding its acknowledged summary.
    let mut omitted = fields
        .get("omittedByStatus")
        .and_then(|value| serde_json::from_value::<StatusCounts>(value.clone()).ok())
        .unwrap_or_default();
    if previous.is_none() && !omitted.is_empty() {
        let snapshot_at = at(fields.get("at").unwrap_or(&Value::Null));
        if changed(&fresh) <= snapshot_at {
            return Ok(());
        }
        if before.get("removed").is_none()
            && changed(before) <= snapshot_at
            && let Some(count) = before["status"]
                .as_str()
                .and_then(|status| omitted.get_mut(status))
        {
            *count = count.saturating_sub(1);
        }
        omitted.retain(|_, count| *count > 0);
    }
    let mut items: Vec<_> = formerly
        .iter()
        .filter(|item| item["id"] != id)
        .cloned()
        .collect();
    let mut removed: Vec<_> = formerly_deleted
        .iter()
        .filter(|item| item["id"] != id)
        .cloned()
        .collect();
    if deleted {
        removed.insert(0, fresh);
        removed.truncate(MOST_ITEMS);
    } else if let Some(previous) = previous.and_then(Value::as_object) {
        let mut kept = previous.clone();
        let unchanged = |key: &str| {
            matches!(key, "sessions" | "progress" | "handedFrom") && previous.contains_key(key)
        };
        kept.extend(
            fresh
                .as_object()
                .expect("clean peer item")
                .iter()
                .filter(|(key, _)| !unchanged(key))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        kept.remove("removed");
        items.push(kept.into());
    } else {
        items.push(fresh);
    }
    fields.insert("items".into(), json!(items));
    fields.insert("removed".into(), json!(removed));
    if omitted.is_empty() {
        fields.remove("omittedByStatus");
    } else {
        fields.insert("omittedByStatus".into(), json!(omitted));
    }
    let bytes = serde_json::to_vec(&FORMAT.encode(fields)).map_err(io::Error::other)?;
    files::write_secret(&file, &bytes)?;
    Ok(())
}
