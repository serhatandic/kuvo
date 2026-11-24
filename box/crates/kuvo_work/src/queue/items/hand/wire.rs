//! The Work Queue handoff letters and the saved placement they carry.

use super::*;
use kuvo_peers::directory::ComputerKey;
use serde::{Deserialize, Serialize};

pub(in crate::queue::items) const HAND_LIFE: i64 = 30 * 86_400_000;
pub(in crate::queue::items) const MOST_HANDED_FILES: usize = 4_000_000;
pub(in crate::queue::items) const MOVE_FEATURE: &str = "moves.two-computers";
const SEEN: ObjectFormat = ObjectFormat::additive("Work Queue handoff receipts");

#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub(in crate::queue::items) struct HandId(String);

impl TryFrom<String> for HandId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.strip_prefix("wqh_").is_some_and(|token| {
            (8..=40).contains(&token.len())
                && token
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
        }) {
            Ok(Self(value))
        } else {
            Err("invalid Work Queue handoff id")
        }
    }
}

impl From<HandId> for String {
    fn from(id: HandId) -> Self {
        id.0
    }
}

impl HandId {
    pub fn new() -> io::Result<Self> {
        let mut random = [0; 12];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        Ok(Self(format!("wqh_{}", kuvo_wire::keys::encode(&random))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(in crate::queue::items) struct Handing {
    pub id: HandId,
    pub to: String,
    pub key: String,
    pub name: String,
    pub at: i64,
    pub by: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
}

impl Handing {
    pub fn read(item: &Item) -> Option<Self> {
        let state: Self = serde_json::from_value(item.saved.get("handing")?.clone()).ok()?;
        (store::session_id(&state.to) && state.peer().is_some()).then_some(state)
    }

    pub fn peer(&self) -> Option<ComputerKey> {
        kuvo_wire::keys::decode_exact(&self.key)
    }
}

#[derive(Deserialize)]
pub(in crate::queue::items) struct Hand {
    pub id: HandId,
    pub to: String,
    pub release: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub(in crate::queue::items) struct Prepared {
    pub proof: String,
    pub payload: Value,
}

impl Prepared {
    pub fn path(store: &Store, sender: &str, id: &HandId) -> PathBuf {
        store
            .root
            .parent()
            .expect("Work Queue under run")
            .join("owned-data")
            .join("queue-handoffs")
            .join(format!("{}.json", key(json!([sender, id]))))
    }

    pub fn read(path: &Path) -> Result<Option<Self>, Error> {
        store::read(path, RECEIPT)?
            .map(|fields| serde_json::from_value(Value::Object(fields)).map_err(io::Error::from))
            .transpose()
            .map_err(Error::from)
    }

    pub fn save(&self, store: &Store, path: &Path) -> Result<(), Error> {
        let fields = serde_json::from_value(json!(self)).map_err(io::Error::from)?;
        let bytes = serde_json::to_vec(&RECEIPT.encode(fields)).map_err(io::Error::from)?;
        let run = store.root.parent().expect("Work Queue under run");
        files::write_atomic_through(path, &bytes, run)?;
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
pub(in crate::queue::items) struct Answer {
    pub id: HandId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retry: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<String>,
}

impl Answer {
    pub fn accepted(id: HandId) -> Self {
        Self {
            id,
            ok: Some(true),
            why: None,
            prepared: None,
            retry: false,
        }
    }

    pub fn refused(id: HandId, why: impl Into<String>, retry: bool) -> Self {
        Self {
            id,
            ok: Some(false),
            why: Some(why.into()),
            prepared: None,
            retry,
        }
    }

    pub fn prepared(id: HandId, proof: String) -> Self {
        Self {
            id,
            ok: None,
            why: None,
            prepared: Some(proof),
            retry: true,
        }
    }
}

pub(in crate::queue::items) fn seen(store: &Store, id: &HandId) -> Result<bool, Error> {
    let seen = store::read(&store.root.join("handed.json"), SEEN)?.unwrap_or_default();
    Ok(seen.contains_key(id.as_str()))
}

pub(in crate::queue::items) fn remember(store: &Store, id: &HandId, now: i64) -> Result<(), Error> {
    use kuvo_store::locks::{OnBusy, StaleLock};
    let _lock = StaleLock::acquire(
        &store.root.join(".handed.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        OnBusy::Proceed,
    )?;
    let path = store.root.join("handed.json");
    let mut seen = store::read(&path, SEEN)?.unwrap_or_default();
    seen.insert(id.as_str().into(), now.into());
    let bytes = serde_json::to_vec(&SEEN.encode(seen)).map_err(io::Error::from)?;
    files::write_atomic_through(&path, &bytes, &store.root)?;
    Ok(())
}
