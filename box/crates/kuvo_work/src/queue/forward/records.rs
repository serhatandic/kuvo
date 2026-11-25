//! Durable receipts and ordinary tracked Work Queue requests.

use crate::queue::store::{self, Error, Store};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
    locks::{OnBusy, StaleLock},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io, path::PathBuf, time::Duration};

const REQUEST: ObjectFormat = ObjectFormat::additive("Work Queue forwarded request");
const ANSWER: ObjectFormat = ObjectFormat::additive("Work Queue forwarded answer");
const RECEIPTS: ObjectFormat = ObjectFormat::additive("Work Queue forwarded action receipts");
const MOST_RECEIPTS: usize = 2_000;
const MEMORY: i64 = 600_000;
const CLOCK_AHEAD: i64 = 60_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(in crate::queue) struct Reply {
    pub status: u16,
    #[serde(default)]
    pub value: Value,
}

#[derive(Deserialize, Serialize)]
pub(in crate::queue) struct Act {
    pub id: String,
    pub at: i64,
    pub item: String,
    pub method: Method,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Agent>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub(in crate::queue) enum Method {
    Get,
    Patch,
    Delete,
    Post,
}

pub(in crate::queue) enum RequestKind<'a> {
    Read,
    OfflineRead,
    LabelRestore,
    LabelEdit,
    Archive { session: &'a str, action: &'a str },
    Agent,
    File(&'a str),
    Edit,
    Delete,
    Restore,
}

impl Act {
    pub fn parse(raw: &Value) -> Option<Self> {
        if !legacy(raw) {
            return None;
        }
        let act: Self = serde_json::from_value(raw.clone()).ok()?;
        (valid_id(&act.id) && short(&act.item) && act.kind().is_some()).then_some(act)
    }

    pub fn kind(&self) -> Option<RequestKind<'_>> {
        if let Some(agent) = &self.agent {
            return (self.area.as_deref() == Some("queue-agent") && agent.valid(self))
                .then_some(RequestKind::Agent);
        }
        if let Some(area) = self.area.as_deref() {
            if area != "ownership" || self.action.is_some() {
                return None;
            }
            return match (self.method, self.item.as_str()) {
                (Method::Get, "offline-edit") => Some(RequestKind::OfflineRead),
                (Method::Post, crate::queue::labels::EDIT_ROUTE) => Some(RequestKind::LabelEdit),
                (Method::Post, crate::queue::labels::RESTORE_ROUTE) => {
                    Some(RequestKind::LabelRestore)
                }
                (Method::Post, route) => {
                    let (session, action) =
                        route.strip_prefix("kuvo/peer-archive/")?.split_once('/')?;
                    let suffix = session.strip_prefix("ses")?;
                    if !(1..=95).contains(&suffix.len())
                        || !suffix
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                        || !matches!(action, "unarchive" | "delete" | "delete-all")
                    {
                        return None;
                    }
                    Some(RequestKind::Archive { session, action })
                }
                _ => None,
            };
        }
        Some(match (self.method, self.action.as_deref()) {
            (Method::Get, None) => RequestKind::Read,
            (Method::Get, Some("file")) => {
                RequestKind::File(self.name.as_deref().filter(|name| short(name))?)
            }
            (Method::Patch, None) => RequestKind::Edit,
            (Method::Delete, None) => RequestKind::Delete,
            (Method::Post, Some("restore")) => RequestKind::Restore,
            _ => return None,
        })
    }

    pub fn receipt(&self) -> ReceiptRef {
        ReceiptRef {
            id: self.id.clone(),
            at: self.at,
        }
    }

    pub fn current(&self, now: i64) -> bool {
        now.saturating_sub(self.at) <= MEMORY && self.at.saturating_sub(now) <= CLOCK_AHEAD
    }
}

fn short(text: &str) -> bool {
    !text.is_empty() && text.encode_utf16().count() <= 300
}

pub(in crate::queue) fn valid_id(id: &str) -> bool {
    id.strip_prefix("wqa_").is_some_and(|id| {
        (16..=40).contains(&id.len())
            && id
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
    })
}

fn legacy(value: &Value) -> bool {
    value
        .get("formatVersion")
        .is_none_or(|version| matches!(version.as_f64(), Some(0.0 | 1.0)))
}

#[derive(Deserialize, Serialize)]
pub(in crate::queue) struct Spool {
    pub to: String,
    pub act: Act,
}

impl Spool {
    pub fn write(&self, store: &Store) -> Result<(), Error> {
        let fields = serde_json::from_value::<Fields>(json!(self)).map_err(io::Error::other)?;
        store::write(&request_path(store, &self.act.id), REQUEST, fields).map_err(Into::into)
    }

    pub fn read(store: &Store, id: &str) -> Result<Option<Self>, Error> {
        if !valid_id(id) {
            return Ok(None);
        }
        let Some(fields) = store::read(&request_path(store, id), REQUEST)? else {
            return Ok(None);
        };
        if fields.contains_key("scopedAct") {
            return Ok(None);
        }
        Ok(serde_json::from_value(Value::Object(fields)).ok())
    }

    pub fn matches(&self, to: &str, id: &str) -> bool {
        self.to == to && self.act.id == id
    }
}

fn request_path(store: &Store, id: &str) -> PathBuf {
    store.root.join("forward").join(format!("{id}.json"))
}
fn answer_path(store: &Store, id: &str) -> PathBuf {
    store.root.join("forward").join(format!("{id}.answer.json"))
}

pub(in crate::queue) fn keep_answer(store: &Store, key: &str, raw: &Value) -> Result<(), Error> {
    let Some(id) = raw["id"].as_str().filter(|id| valid_id(id)) else {
        return Ok(());
    };
    if !Spool::read(store, id)?.is_some_and(|spool| spool.matches(key, id)) {
        return Ok(());
    }
    let Ok(answer) = serde_json::from_value::<Reply>(raw.clone()) else {
        return Ok(());
    };
    let fields = serde_json::from_value::<Fields>(json!(answer)).map_err(io::Error::other)?;
    store::write(&answer_path(store, id), ANSWER, fields).map_err(Into::into)
}

pub(in crate::queue) fn forget(store: &Store, id: &str) {
    let _ = std::fs::remove_file(request_path(store, id));
    let _ = std::fs::remove_file(answer_path(store, id));
}

#[derive(Deserialize, Serialize)]
pub(in crate::queue) struct Receipt {
    until: i64,
    pub answer: Option<Reply>,
}

pub(in crate::queue) struct ReceiptRef {
    pub id: String,
    pub at: i64,
}

/// Claim before changing anything. A receipt without an outcome survives process exit.
pub(in crate::queue) fn receipt(
    store: &Store,
    peer: &str,
    act: &ReceiptRef,
    now: i64,
    answer: Option<Reply>,
) -> Result<Option<Receipt>, Error> {
    let _lock = StaleLock::acquire(
        &store.root.join(".acted.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Refuse,
    )?;
    let file = store.root.join("acted.json");
    let mut entries = RECEIPTS
        .read(&file)?
        .map(|record| record.fields)
        .unwrap_or_default();
    entries.retain(|_, entry| entry["until"].as_i64().is_some_and(|until| until >= now));
    let key = format!("{peer} {}", act.id);
    if answer.is_none()
        && let Some(before) = entries.get(&key)
    {
        return serde_json::from_value(before.clone())
            .map(Some)
            .map_err(|error| io::Error::other(error).into());
    }
    let until = entries
        .remove(&key)
        .and_then(|entry| entry["until"].as_i64())
        .unwrap_or_else(|| now.max(act.at).saturating_add(MEMORY));
    entries.insert(key, json!(Receipt { until, answer }));
    if entries.len() > MOST_RECEIPTS {
        let remove = entries
            .keys()
            .take(entries.len() - MOST_RECEIPTS)
            .cloned()
            .collect::<Vec<_>>();
        for key in remove {
            entries.remove(&key);
        }
    }
    let bytes = serde_json::to_vec(&RECEIPTS.encode(entries)).map_err(io::Error::other)?;
    files::write_atomic_through(
        &file,
        &bytes,
        store.root.parent().expect("Work Queue under run"),
    )?;
    Ok(None)
}

#[derive(Clone, Deserialize, Serialize)]
pub(in crate::queue) struct Agent {
    pub tool: Tool,
    pub owner: String,
    pub session: String,
    pub name: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub(in crate::queue) enum Tool {
    #[serde(rename = "queue_update")]
    Update,
    #[serde(rename = "queue_remove")]
    Remove,
    #[serde(rename = "queue_hand")]
    Hand,
}

impl Agent {
    fn valid(&self, act: &Act) -> bool {
        if !self.session.starts_with("ses_")
            || !store::owner_id(&self.session)
            || (self.owner != self.session
                && !(self.owner.starts_with("grp_") && store::owner_id(&self.owner)))
            || self.name.trim().is_empty()
            || self.name.encode_utf16().count() > 200
            || !store::item_id(&act.item)
        {
            return false;
        }
        matches!(
            (self.tool, act.method, act.action.as_deref()),
            (Tool::Update | Tool::Hand, Method::Patch, None)
                | (Tool::Remove, Method::Delete, None)
                | (Tool::Remove, Method::Post, Some("restore"))
        )
    }
}
