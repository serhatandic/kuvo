use super::{migration, status};
use kuvo_store::{
    files,
    format::{self, Fields, ObjectFormat},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub(super) const QUEUE: ObjectFormat = ObjectFormat::additive("Work Queue owner file");
pub(super) const LABELS: ObjectFormat = ObjectFormat::additive("Work Queue labels");
pub(super) const COLORS: [&str; 10] = [
    "coral", "orange", "amber", "lime", "mint", "sky", "indigo", "violet", "pink", "graphite",
];

#[derive(Debug, Error)]
pub(super) enum Error {
    #[error(transparent)]
    Format(#[from] format::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub(super) fn exclusive(
    path: &Path,
    wait: std::time::Duration,
) -> io::Result<kuvo_store::locks::OwnedLock> {
    kuvo_store::locks::OwnedLock::acquire(path, wait).map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            return io::Error::new(
                io::ErrorKind::WouldBlock,
                "Kuvo Code is updating its link. Try again in a moment.",
            );
        }
        error
    })
}

#[derive(Clone)]
pub(super) struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(home: &Path) -> Self {
        Self {
            root: home.join("run").join("work-queue"),
        }
    }

    pub fn items(&self) -> Result<Vec<Item>, Error> {
        match migration::migrate(&self.root) {
            Err(error @ Error::Format(format::Error::Unsupported { .. })) => return Err(error),
            Err(error) => eprintln!(
                "Work Queue: its items couldn't be moved over to statuses yet ({error}): shown as they are, and tried again."
            ),
            Ok(()) => {}
        }
        let queues = self.root.join("queues");
        let mut items: Vec<Option<Item>> = Vec::new();
        let mut positions: HashMap<String, usize> = HashMap::new();
        for file in queue_files(&queues) {
            let Some(owner) = file
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| owner_id(name))
            else {
                continue;
            };
            let Some(fields) = read(&file, QUEUE)? else {
                continue;
            };
            let mut seen = HashSet::new();
            for item in array(fields.get("items"))
                .iter()
                .filter_map(|value| Item::read(owner, value))
            {
                if !seen.insert(item.id.clone()) {
                    continue;
                }
                if let Some(&index) = positions.get(&item.id) {
                    if items[index]
                        .as_ref()
                        .is_some_and(|other| other.updated.at > item.updated.at)
                    {
                        continue;
                    }
                    items[index] = None;
                }
                positions.insert(item.id.clone(), items.len());
                items.push(Some(item));
            }
        }
        Ok(items.into_iter().flatten().collect())
    }
}

#[derive(Clone, Serialize)]
pub(super) struct Stamp {
    pub at: f64,
    pub by: Value,
}

impl Stamp {
    fn read(value: Option<&Value>, fallback: &Self) -> Self {
        let Some(value) = value else {
            return fallback.clone();
        };
        let Some(at) = value.get("at").and_then(Value::as_f64) else {
            return fallback.clone();
        };
        Self {
            at,
            by: value
                .get("by")
                .and_then(who)
                .unwrap_or_else(|| fallback.by.clone()),
        }
    }
}

pub(super) fn who(value: &Value) -> Option<Value> {
    let fields = value.as_object()?;
    if fields.get("user").and_then(Value::as_bool) == Some(true) {
        return Some(json!({"user": true}));
    }
    if let (Some(session), Some(name)) = (value["session"].as_str(), value["name"].as_str()) {
        return Some(json!({"session": session, "name": name}));
    }
    let kept: Fields = fields
        .iter()
        .filter(|(key, value)| {
            key.encode_utf16().count() <= 40
                && match value {
                    Value::String(text) => text.encode_utf16().count() <= 200,
                    Value::Number(_) | Value::Bool(_) => true,
                    _ => false,
                }
        })
        .take(10)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    (!kept.is_empty()).then_some(Value::Object(kept))
}

pub(super) struct Item {
    pub id: String,
    pub number: Option<f64>,
    pub owner: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub order: f64,
    pub priority: String,
    pub labels: Vec<String>,
    pub created: Stamp,
    pub updated: Stamp,
    pub removed: Option<Stamp>,
    pub notes: Vec<Value>,
    pub events: Vec<Value>,
    pub files: Vec<Value>,
    pub links: Vec<Value>,
    pub sessions: Vec<Value>,
    pub handed: Vec<Value>,
    pub saved: Fields,
}

impl Item {
    pub(super) fn read(owner: &str, value: &Value) -> Option<Self> {
        let saved = value.as_object()?;
        let id = value["id"].as_str().filter(|id| item_id(id))?.to_owned();
        let title = value["title"].as_str()?.to_owned();
        let number = positive_integer(&value["number"]);
        let (status, labels) = match value["status"].as_str().filter(|status| !status.is_empty()) {
            Some(status) => {
                let mut seen = HashSet::new();
                let labels = array(saved.get("labels"))
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|id| label_id(id) && seen.insert(*id))
                    .take(20)
                    .map(str::to_owned)
                    .collect();
                (limit(status, 40), labels)
            }
            None => {
                let (status, label) = status::category(value["category"].as_str(), None);
                (
                    status.into(),
                    label.into_iter().map(str::to_owned).collect(),
                )
            }
        };
        let user = Stamp {
            at: 0.0,
            by: json!({"user": true}),
        };
        let created = Stamp::read(saved.get("created"), &user);
        let updated = Stamp::read(saved.get("updated"), &created);
        let removed = value["removed"]["at"]
            .as_f64()
            .map(|_| Stamp::read(saved.get("removed"), &user));
        let list = |key, valid: fn(&Value) -> bool| -> Vec<Value> {
            array(saved.get(key))
                .iter()
                .filter(|entry| valid(entry))
                .cloned()
                .collect()
        };
        let mut events = list("events", |entry| {
            entry["at"].is_number()
                && entry["kind"]
                    .as_str()
                    .is_some_and(|kind| !kind.is_empty() && kind.encode_utf16().count() <= 40)
                && who(&entry["by"]).is_some()
                && entry["by"].to_string().encode_utf16().count() <= 500
                && entry.to_string().encode_utf16().count() <= 2000
        });
        events.drain(..events.len().saturating_sub(50));
        Some(Self {
            id,
            number,
            owner: owner.into(),
            title,
            status,
            labels,
            created,
            updated,
            removed,
            events,
            body: value["body"].as_str().unwrap_or_default().into(),
            order: value["order"].as_f64().unwrap_or_default(),
            priority: value["priority"]
                .as_str()
                .filter(|id| status::PRIORITIES.iter().any(|priority| priority.id == *id))
                .unwrap_or("normal")
                .into(),
            notes: list("notes", |entry| {
                entry["at"].is_number() && entry["text"].is_string() && who(&entry["by"]).is_some()
            }),
            files: list("files", |entry| {
                entry["name"].as_str().is_some_and(file_name) && entry["size"].is_number()
            }),
            links: list("links", |entry| entry["url"].is_string()),
            sessions: list("sessions", |entry| {
                entry["session"].as_str().is_some_and(session_id)
            }),
            handed: list("handed", |entry| {
                entry["from"].is_string() && entry["to"].is_string()
            }),
            saved: saved.clone(),
        })
    }

    /// A changed queue writes the same checked fields it reads, keeping newer fields.
    pub(super) fn normalized(mut self) -> Value {
        for (key, value) in [
            ("id", self.id.into()),
            ("owner", self.owner.into()),
            ("title", self.title.into()),
            ("body", self.body.into()),
            ("status", self.status.into()),
            ("order", self.order.into()),
            ("labels", self.labels.into()),
            ("notes", self.notes.into()),
            ("files", self.files.into()),
            ("links", self.links.into()),
            ("sessions", self.sessions.into()),
            ("created", json!(self.created)),
            ("updated", json!(self.updated)),
        ] {
            self.saved.insert(key.into(), value);
        }
        let number = self.number.map(Value::from);
        let priority = (self.priority != "normal").then(|| self.priority.into());
        let removed = self.removed.map(|removed| json!(removed));
        for (key, value) in [
            ("number", number),
            ("priority", priority),
            ("removed", removed),
            ("category", None),
        ] {
            if let Some(value) = value {
                self.saved.insert(key.into(), value);
            } else {
                self.saved.remove(key);
            }
        }
        for (key, checked) in [("events", self.events), ("handed", self.handed)] {
            if self.saved.get(key).is_some_and(Value::is_array) {
                self.saved.insert(key.into(), checked.into());
            } else {
                self.saved.remove(key);
            }
        }
        for name in ["wanted", "importedAs"] {
            if self
                .saved
                .get(name)
                .is_some_and(|value| positive_integer(value).is_none())
            {
                self.saved.remove(name);
            }
        }
        for name in ["backGranted", "stays"] {
            if self.saved.get(name).is_some_and(|value| *value != true) {
                self.saved.remove(name);
            }
        }
        if self
            .saved
            .get("doneAt")
            .is_some_and(|value| !value.is_number())
        {
            self.saved.remove("doneAt");
        }
        if self
            .saved
            .get("handedBy")
            .is_some_and(|value| !value.as_str().is_some_and(owner_id))
        {
            self.saved.remove("handedBy");
        }
        if let Some(fields) = self
            .saved
            .get_mut("progress")
            .and_then(Value::as_object_mut)
        {
            fields.retain(|key, value| {
                (key == "note" && value.is_string()) || (key == "value" && value.is_number())
            });
        } else {
            self.saved.remove("progress");
        }
        if self
            .saved
            .get("source")
            .is_some_and(|source| !source["kind"].is_string() || !source["ref"].is_string())
        {
            self.saved.remove("source");
        }
        Value::Object(self.saved)
    }

    pub fn source(&self) -> Option<&Value> {
        self.saved
            .get("source")
            .filter(|source| source["kind"].is_string() && source["ref"].is_string())
    }

    pub fn handed_by(&self) -> Option<&str> {
        self.saved
            .get("handedBy")
            .and_then(Value::as_str)
            .filter(|id| owner_id(id))
    }
}

pub(super) fn positive_integer(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|number| *number > 0.0 && number.fract() == 0.0)
}

pub(super) fn array(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

pub(super) fn owner_id(id: &str) -> bool {
    id == "usr_you"
        || session_id(id)
        || id.strip_prefix("grp_").is_some_and(|tail| {
            bounded_ascii(tail, 1, 60, |byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit()
            })
        })
}

fn session_id(id: &str) -> bool {
    id.strip_prefix("ses_")
        .is_some_and(|tail| bounded_ascii(tail, 1, 60, |byte| byte.is_ascii_alphanumeric()))
}

pub(super) fn item_id(id: &str) -> bool {
    id.strip_prefix("wqi_").is_some_and(|tail| {
        bounded_ascii(tail, 20, 20, |byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
    })
}

pub(super) fn label_id(id: &str) -> bool {
    id.strip_prefix("lbl_")
        .or_else(|| id.strip_prefix("cat_"))
        .is_some_and(|tail| {
            bounded_ascii(tail, 1, 40, |byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
            })
        })
}

fn bounded_ascii(text: &str, min: usize, max: usize, test: impl Fn(u8) -> bool) -> bool {
    (min..=max).contains(&text.len()) && text.bytes().all(test)
}

pub(super) fn file_name(name: &str) -> bool {
    !matches!(name, "" | "." | "..")
        && name.encode_utf16().count() <= 200
        && !name.contains(['/', '\\', '\0'])
}

pub(super) fn plain_name(text: &str) -> String {
    text.split(|ch: char| ch.is_whitespace() || matches!(ch, '\0'..='\u{001f}' | '\u{007f}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

pub(super) fn limit(text: &str, units: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|ch| {
            used += ch.len_utf16();
            used <= units
        })
        .collect()
}

pub(super) fn queue_files(folder: &Path) -> Vec<PathBuf> {
    let mut names = files::list_of(folder);
    names.retain(|name| name.to_str().is_some_and(|name| name.ends_with(".json")));
    names.sort();
    names.into_iter().map(|name| folder.join(name)).collect()
}

pub(super) fn read(path: &Path, schema: ObjectFormat) -> Result<Option<Fields>, Error> {
    match schema.read(path) {
        Ok(record) => Ok(record.map(|record| record.fields)),
        Err(error @ format::Error::Unsupported { .. }) => Err(error.into()),
        Err(_) => Ok(None),
    }
}

pub(super) fn write(path: &Path, schema: ObjectFormat, fields: Fields) -> io::Result<()> {
    files::write_atomic(path, &serde_json::to_vec(&schema.encode(fields))?)
}
