use std::{
    fs::{self, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Error, Item, Kind, Result, items, signed::Writer};

const DAY: u64 = 86_400_000;
const RETENTION: u64 = 365 * DAY;

pub(super) struct Activity {
    home: PathBuf,
    file: PathBuf,
    pruned: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Entry {
    id: String,
    at: u64,
    what: Action,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<Kind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    how: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Action {
    Used,
    Asked,
    Allowed,
    Declined,
    Refused,
    Added,
    Changed,
    Removed,
    Granted,
    Ungranted,
    Paused,
    Resumed,
    Shown,
}

#[derive(Default)]
pub(super) struct Filter {
    pub item: Option<String>,
    pub bot: Option<String>,
    pub session: Option<String>,
    pub before: Option<f64>,
    pub limit: Option<f64>,
}

impl Activity {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.into(),
            file: home.join("box/vault/activity.jsonl"),
            pruned: 0,
        }
    }

    pub fn note(&mut self, entry: Entry) -> Result<()> {
        let _writer = Writer::acquire(&self.home)?;
        self.append(&entry)
            .map_err(|error| Error::disk("Couldn't keep the vault's activity.", error))?;
        if entry.at.saturating_sub(self.pruned) > DAY {
            self.pruned = entry.at;
            if let Ok(lines) = self.read() {
                let mut kept = Vec::new();
                let mut pruned = false;
                for saved in lines {
                    if entry.at.saturating_sub(saved.at) >= RETENTION {
                        pruned = true;
                    } else {
                        write_entry(&mut kept, &saved).expect("vault entries serialize");
                    }
                }
                if pruned {
                    kuvo_store::files::write_secret(&self.file, &kept).map_err(|error| {
                        Error::disk("Couldn't prune the vault's activity.", error)
                    })?;
                }
            }
        }
        Ok(())
    }

    fn append(&self, entry: &Entry) -> std::io::Result<()> {
        kuvo_store::files::create_private_dirs(self.file.parent().expect("activity has a parent"))?;
        let mut options = OpenOptions::new();
        options.read(true).append(true).create(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&self.file)?;
        let mut bytes = Vec::new();
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut tail = [0];
            file.read_exact(&mut tail)?;
            if tail[0] != b'\n' {
                bytes.push(b'\n');
            }
        }
        write_entry(&mut bytes, entry)?;
        file.write_all(&bytes)
    }

    fn read(&self) -> Result<Vec<Entry>> {
        let Ok(bytes) = fs::read(&self.file) else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        for line in bytes.split(|byte| *byte == b'\n') {
            let Ok(Value::Object(saved)) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            super::signed::validate_version(saved.get("formatVersion"), 1, "Vault activity")?;
            if let Ok(entry) = serde_json::from_value(Value::Object(saved)) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    pub fn list(&self, filter: Filter, by: &str, now: u64) -> Result<Vec<Value>> {
        let mut entries = self.read()?;
        entries.retain(|entry| {
            let matches = |asked: Option<&str>, kept: Option<&str>| {
                asked
                    .filter(|asked| !asked.is_empty())
                    .is_none_or(|asked| kept == Some(asked))
            };
            now.saturating_sub(entry.at) < RETENTION
                && matches(filter.item.as_deref(), entry.item.as_deref())
                && matches(filter.bot.as_deref(), entry.bot.as_deref())
                && matches(filter.session.as_deref(), entry.session.as_deref())
                && filter
                    .before
                    .filter(|before| *before != 0.0)
                    .is_none_or(|before| (entry.at as f64) < before)
        });
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.at));
        let limit = filter
            .limit
            .filter(|limit| *limit != 0.0)
            .unwrap_or(100.0)
            .clamp(1.0, 500.0) as usize;
        Ok(entries
            .into_iter()
            .take(limit)
            .map(|mut entry| {
                let mine = entry.by.take();
                if mine.is_none() {
                    entry.client = None;
                }
                let mut shown = serde_json::to_value(entry).expect("vault entries serialize");
                if mine.as_deref() == Some(by) {
                    shown["mine"] = Value::Bool(true);
                }
                shown
            })
            .collect())
    }
}

impl Entry {
    pub fn agent(
        what: Action,
        item: Option<&Item>,
        caller: &super::identity::Identity,
        place: &super::broker::Where,
        how: Option<&str>,
        by: Option<&str>,
        now: u64,
    ) -> Result<Self> {
        Ok(Self {
            id: items::new_id("a_")?,
            at: now,
            what,
            item: item.map(|item| item.id.clone()),
            item_name: item.map(|item| item.name.clone()),
            kind: item.map(|item| item.kind),
            bot: caller.bot.as_ref().map(|bot| bot.id.clone()),
            bot_name: caller.bot.as_ref().map(|bot| bot.name.clone()),
            session: (!caller.root.is_empty()).then(|| caller.root.clone()),
            session_name: caller.name.clone(),
            client: by.map(|_| "Mac".into()),
            by: by.map(str::to_owned),
            site: place.site.clone(),
            program: place.program.clone(),
            how: how.map(str::to_owned),
            why: place.why.as_ref().map(|why| items::one_line(why, 300)),
        })
    }

    pub fn client(
        what: Action,
        item: Option<&Item>,
        bot: Option<String>,
        how: Option<String>,
        by: String,
        now: u64,
    ) -> Result<Self> {
        Ok(Self {
            id: items::new_id("a_")?,
            at: now,
            what,
            item: item.map(|item| item.id.clone()),
            item_name: item.map(|item| item.name.clone()),
            kind: item.map(|item| item.kind),
            bot,
            bot_name: None,
            session: None,
            session_name: None,
            client: Some("Mac".into()),
            by: Some(by),
            site: None,
            program: None,
            how,
            why: None,
        })
    }
}

fn write_entry(bytes: &mut Vec<u8>, entry: &Entry) -> std::io::Result<()> {
    let mut value = serde_json::to_value(entry)?;
    value["formatVersion"] = Value::from(1);
    serde_json::to_writer(&mut *bytes, &value)?;
    bytes.push(b'\n');
    Ok(())
}
