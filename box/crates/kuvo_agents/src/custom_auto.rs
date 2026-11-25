//! Custom Auto model-list storage. It is separate from the approval mode.
//! The turn handover owner decides when and how to use a list.
use kuvo_store::{files, locks::OwnedLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entry {
    #[serde(rename = "providerID")]
    pub provider: String,
    #[serde(rename = "modelID")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct List {
    pub id: String,
    pub name: String,
    pub entries: Vec<Entry>,
}
impl List {
    pub fn revision(&self) -> String {
        hex::encode(Sha256::digest(
            serde_json::to_vec(self).expect("model list"),
        ))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Provider,
    Model,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limit {
    pub key: String,
    pub until: i64,
    pub scope: Scope,
    pub known: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    pub lists: Vec<List>,
    pub limits: Vec<Limit>,
}
impl Default for Saved {
    fn default() -> Self {
        Self {
            lists: vec![List {
                id: "auto".into(),
                name: "Custom Auto".into(),
                entries: vec![],
            }],
            limits: vec![],
        }
    }
}
pub struct Store {
    file: PathBuf,
    lock: PathBuf,
}
#[derive(Debug)]
pub enum Error {
    Stale,
    Invalid(&'static str),
    Disk(io::Error),
}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Disk(value)
    }
}
impl Store {
    pub fn new(home: &Path) -> Self {
        Self {
            file: home.join("agents/auto.json"),
            lock: home.join("run/auto.lock"),
        }
    }
    fn load(&self) -> Saved {
        let Some(raw) = files::read(&self.file)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            return Saved::default();
        };
        let mut kept = Saved::default();
        if let Some(lists) = raw["lists"].as_array() {
            let mut seen = BTreeSet::new();
            let lists = lists
                .iter()
                .filter_map(|l| {
                    let id = l["id"].as_str()?.to_owned();
                    if !valid_id(&id) || !seen.insert(id.clone()) {
                        return None;
                    }
                    Some(List {
                        id,
                        name: clean_name(l["name"].as_str().unwrap_or_default()),
                        entries: clean_entries(&l["entries"]),
                    })
                })
                .collect::<Vec<_>>();
            if !lists.is_empty() {
                kept.lists = lists;
            }
        } else {
            kept.lists[0].entries = clean_entries(&raw["entries"]);
        }
        kept.limits = raw["limits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| serde_json::from_value::<Limit>(v.clone()).ok())
            .filter(|l| !l.key.is_empty())
            .collect();
        kept
    }
    fn save(&self, kept: &Saved) -> io::Result<()> {
        files::write_secret(&self.file, &serde_json::to_vec_pretty(kept)?)
    }
    fn locked(&self) -> io::Result<OwnedLock> {
        OwnedLock::acquire(&self.lock, Duration::from_secs(15))
    }
    pub fn read(&self, now: i64) -> io::Result<Saved> {
        let _guard = self.locked()?;
        let mut kept = self.load();
        let before = kept.limits.len();
        kept.limits.retain(|l| l.until > now);
        if kept.limits.len() != before {
            self.save(&kept)?;
        }
        Ok(kept)
    }
    pub fn change(
        &self,
        id: &str,
        revision: Option<&str>,
        name: Option<&str>,
        entries: Option<Vec<Entry>>,
        add: bool,
        now: i64,
    ) -> Result<Saved, Error> {
        if !valid_id(id) || matches!(id, "order" | "limits") {
            return Err(Error::Invalid("Invalid list id."));
        }
        if name.is_some_and(|s| s.trim().is_empty()) {
            return Err(Error::Invalid("Send a name."));
        }
        let _guard = self.locked()?;
        let mut kept = self.load();
        kept.limits.retain(|l| l.until > now);
        let position = kept.lists.iter().position(|l| l.id == id);
        if add && position.is_some()
            || !add && position.is_none_or(|i| revision != Some(&kept.lists[i].revision()))
        {
            return Err(Error::Stale);
        }
        if let Some(i) = position {
            let list = &mut kept.lists[i];
            if let Some(name) = name {
                list.name = clean_name(name);
            }
            if let Some(entries) = entries {
                list.entries = fold(entries, |_| None);
            }
        } else {
            kept.lists.push(List {
                id: id.into(),
                name: clean_name(name.unwrap_or_default()),
                entries: fold(entries.unwrap_or_default(), |_| None),
            });
        }
        self.save(&kept)?;
        Ok(kept)
    }
    pub fn reorder(&self, ids: &[String], was: &[String], now: i64) -> Result<Saved, Error> {
        let _guard = self.locked()?;
        let mut kept = self.load();
        kept.limits.retain(|l| l.until > now);
        let actual = kept.lists.iter().map(|l| l.id.clone()).collect::<Vec<_>>();
        if was != actual {
            return Err(Error::Stale);
        }
        if ids.len() != actual.len()
            || ids.iter().collect::<BTreeSet<_>>().len() != actual.len()
            || ids.iter().any(|id| !actual.contains(id))
        {
            return Err(Error::Invalid("Send every list id once."));
        }
        kept.lists = ids
            .iter()
            .map(|id| {
                kept.lists
                    .iter()
                    .find(|l| l.id == *id)
                    .expect("validated id")
                    .clone()
            })
            .collect();
        self.save(&kept)?;
        Ok(kept)
    }
    pub fn remove(&self, id: &str, revision: Option<&str>, now: i64) -> Result<Saved, Error> {
        let _guard = self.locked()?;
        let mut kept = self.load();
        kept.limits.retain(|l| l.until > now);
        if kept
            .lists
            .iter()
            .find(|l| l.id == id)
            .is_none_or(|l| revision != Some(&l.revision()))
        {
            return Err(Error::Stale);
        }
        if kept.lists.len() == 1 {
            return Err(Error::Invalid("Keep one list at least."));
        }
        kept.lists.retain(|l| l.id != id);
        self.save(&kept)?;
        Ok(kept)
    }
    pub fn clear(&self, provider: Option<&str>, now: i64) -> Result<(), Error> {
        let _guard = self.locked()?;
        let mut kept = self.load();
        if let Some(provider) = provider {
            kept.limits.retain(|l| {
                l.until > now && l.key != provider && !l.key.starts_with(&format!("{provider}/"))
            });
        } else {
            kept.limits.clear();
        }
        self.save(&kept)?;
        Ok(())
    }
}
pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn clean_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "Custom Auto".into()
    } else {
        let mut count = 0;
        trimmed
            .chars()
            .take_while(|c| {
                count += c.len_utf16();
                count <= 60
            })
            .collect()
    }
}
pub fn clean_entries(given: &Value) -> Vec<Entry> {
    fold(
        given
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| serde_json::from_value::<Entry>(v.clone()).ok())
            .map(|mut e| {
                if e.variant.as_deref() == Some("") {
                    e.variant = None;
                }
                e
            })
            .collect(),
        |_| None,
    )
}
pub fn fold(entries: Vec<Entry>, alias: impl Fn(&Entry) -> Option<String>) -> Vec<Entry> {
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter(|e| e.provider != "auto")
        .filter_map(|mut e| {
            if let Some(id) = alias(&e) {
                e.model = id;
            }
            seen.insert(e.clone()).then_some(e)
        })
        .collect()
}
