//! Pending edits, saved baselines and terminal receipts survive process exit.

use super::types::*;
use crate::queue::store::{self, Error, Store};
use kuvo_store::{
    files,
    format::{Fields, ObjectFormat},
    locks::{OnBusy, StaleLock},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io, path::PathBuf, time::Duration};

const RECORD: ObjectFormat = ObjectFormat::additive("Owned data");

#[derive(Clone)]
pub(super) struct Outbox {
    root: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Entry {
    pub setup: String,
    pub owner: String,
    pub digest: String,
    #[serde(rename = "ref")]
    pub reference: Target,
    pub receipt: Value,
    pub queued_edit: Option<Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitter: Option<Submitter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery: Option<Fields>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<Attachment>>,
}

pub(super) enum Staged {
    Saved(Box<Entry>),
    Conflict,
    Full,
}

impl Outbox {
    pub fn new(store: &Store) -> Self {
        Self {
            root: store
                .root
                .parent()
                .expect("queue under run")
                .join("owned-data"),
        }
    }

    pub fn stage(
        &self,
        setup: &str,
        here: &str,
        owner: String,
        mut edit: Edit,
        submitter: Submitter,
    ) -> Result<Staged, Error> {
        let _guard = self.lock()?;
        let nonce = edit.operation.clone();
        let preferred = op(setup, &nonce);
        let legacy = legacy_op(setup, here, &nonce);
        let previous = self.read(&preferred)?.or(self.read(&legacy)?);
        let operation = previous
            .as_ref()
            .and_then(|before| before.receipt["operation"].as_str())
            .unwrap_or(&preferred)
            .to_owned();
        edit.operation = operation.clone();
        let digest = key(json!({"setup":setup,"nonce":nonce,"edit":edit}));
        if let Some(previous) = previous {
            return Ok(if previous.digest == digest {
                Staged::Saved(Box::new(previous))
            } else {
                Staged::Conflict
            });
        }
        if let Some(path) = self.path("edit-outbox", &operation)
            && path.try_exists()?
        {
            return Ok(Staged::Conflict);
        }
        let mut count = 0;
        for file in store::queue_files(&self.root.join("edit-outbox")) {
            if let Some(record) = store::read(&file, RECORD)?
                && (record.contains_key("queuedEdit") || record.contains_key("envelope"))
            {
                count += 1;
            }
        }
        if count >= MAX_PENDING {
            return Ok(Staged::Full);
        }
        let entry = Entry {
            setup: setup.into(),
            owner,
            digest,
            reference: edit.target.clone(),
            receipt: pending(&operation),
            queued_edit: Some(Envelope {
                setup: setup.into(),
                nonce,
                edit,
                submitter: Some(submitter.clone()),
                replay_only: None,
            }),
            submitter: Some(submitter),
            recovery: None,
            attachments: None,
        };
        self.write(&operation, &entry)?;
        Ok(Staged::Saved(Box::new(entry)))
    }

    pub fn read(&self, operation: &str) -> Result<Option<Entry>, Error> {
        let Some(path) = self.path("edit-outbox", operation) else {
            return Ok(None);
        };
        let Some(mut fields) = store::read(&path, RECORD)? else {
            return Ok(None);
        };
        if !fields
            .get("ref")
            .is_some_and(|value| value["kind"] == "queueItems")
        {
            return Ok(None);
        }
        if !fields.contains_key("queuedEdit")
            && let Some(old) = fields.remove("envelope")
        {
            fields.insert("queuedEdit".into(), old);
        }
        Ok(serde_json::from_value(Value::Object(fields)).ok())
    }

    pub fn entries(&self, setup: &str) -> Result<Vec<Entry>, Error> {
        let mut items = Vec::new();
        for file in store::queue_files(&self.root.join("edit-outbox")) {
            let Some(operation) = file.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if let Some(entry) = self.read(operation)?.filter(|entry| entry.setup == setup) {
                items.push(entry);
            }
        }
        Ok(items)
    }

    pub fn finish(&self, setup: &str, owner: &str, receipt: Value) -> Result<(), Error> {
        let Some(operation) = receipt["operation"].as_str().map(str::to_owned) else {
            return Ok(());
        };
        let _guard = self.lock()?;
        let Some(mut kept) = self
            .read(&operation)?
            .filter(|entry| entry.setup == setup && entry.owner == owner)
        else {
            return Ok(());
        };
        let Some(queued) = kept.queued_edit.take() else {
            return Ok(());
        };
        let mut recovery = Fields::new();
        if matches!(
            receipt["status"].as_str(),
            Some("refresh" | "deleted" | "interrupted" | "capacity")
        ) {
            for (field, value) in queued
                .edit
                .fields
                .into_iter()
                .flatten()
                .filter(|(_, value)| value.is_string())
            {
                recovery.insert(field, value);
            }
            for note in queued.edit.notes.into_iter().flatten() {
                recovery.insert(format!("note:{}", note.id), note.text.into());
            }
            kept.attachments = queued.edit.attachments.filter(|files| !files.is_empty());
        }
        kept.receipt = receipt;
        kept.recovery = Some(recovery);
        self.write(&operation, &kept)
    }

    pub fn readdress(
        &self,
        setup: &str,
        operation: &str,
        was: &str,
        next: &str,
    ) -> Result<bool, Error> {
        let _guard = self.lock()?;
        let Some(mut entry) = self.read(operation)?.filter(|entry| {
            entry.setup == setup && entry.owner == was && entry.queued_edit.is_some()
        }) else {
            return Ok(false);
        };
        entry.owner = next.into();
        self.write(operation, &entry)?;
        Ok(true)
    }

    pub fn cache(&self, key: &str, value: &Value) -> Result<(), Error> {
        let Some(file) = self.path("edit-snapshots", key) else {
            return Ok(());
        };
        let fields = Fields::from_iter([("value".into(), value.clone())]);
        store::write(&file, RECORD, fields).map_err(Into::into)
    }

    pub fn snapshot(&self, key: &str) -> Result<Option<Value>, Error> {
        let Some(path) = self.path("edit-snapshots", key) else {
            return Ok(None);
        };
        Ok(store::read(&path, RECORD)?.and_then(|mut record| record.remove("value")))
    }

    fn path(&self, family: &str, key: &str) -> Option<PathBuf> {
        kuvo_wire::keys::decode_exact::<32>(key)?;
        Some(self.root.join(family).join(format!("{key}.json")))
    }

    fn lock(&self) -> Result<Option<StaleLock>, Error> {
        files::create_private_dirs(&self.root)?;
        StaleLock::acquire(
            &self.root.join(".edits.lock"),
            Duration::from_secs(10),
            Duration::from_secs(3),
            OnBusy::Refuse,
        )
        .map_err(Into::into)
    }

    fn write(&self, operation: &str, entry: &Entry) -> Result<(), Error> {
        let path = self
            .path("edit-outbox", operation)
            .ok_or_else(|| io::Error::other("invalid edit operation"))?;
        let mut fields = serde_json::to_value(entry)
            .map_err(io::Error::other)?
            .as_object()
            .cloned()
            .expect("owned edit entry");
        if entry.queued_edit.is_none() {
            fields.remove("queuedEdit");
        }
        store::write(&path, RECORD, fields).map_err(Into::into)
    }
}

impl Entry {
    pub fn listed(&self) -> Option<Value> {
        (self.queued_edit.is_some()
            || self
                .recovery
                .as_ref()
                .is_some_and(|fields| !fields.is_empty())
            || self.attachments.is_some())
        .then(|| json!({"ref":self.reference,"owner":self.owner,"receipt":self.receipt}))
    }

    pub fn recovered(self) -> Value {
        let mut answer =
            json!({"receipt":self.receipt,"recovery":self.recovery.unwrap_or_default()});
        if let Some(attachments) = self.attachments {
            answer["attachments"] = json!(attachments);
        }
        answer
    }
}
