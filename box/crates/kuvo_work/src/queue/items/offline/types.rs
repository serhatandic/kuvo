use super::history::Resolve;
use crate::queue::store::{item_id, owner_id};
use kuvo_server::Caller;
use kuvo_store::format::Fields;
use kuvo_wire::keys::decode_exact;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const MAX_BYTES: usize = 512 * 1024;
pub(super) const MAX_PENDING: usize = 200;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Target {
    kind: Kind,
    pub id: String,
    pub queue: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
enum Kind {
    #[serde(rename = "queueItems")]
    Queue,
}

impl Target {
    pub fn valid(&self) -> bool {
        item_id(&self.id)
            && owner_id(&self.queue)
            && self
                .computer
                .as_deref()
                .is_none_or(|key| decode_exact::<32>(key).is_some())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Edit {
    #[serde(rename = "ref")]
    pub target: Target,
    pub operation: String,
    pub base_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Fields>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve: Option<Resolve>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<Note>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Note {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Attachment {
    pub id: String,
    pub file: Value,
}

impl Edit {
    pub fn parse(value: Value) -> Option<Self> {
        value.get("baseRevision")?;
        for field in [
            "fields",
            "resolve",
            "create",
            "delete",
            "notes",
            "attachments",
        ] {
            if value.get(field).is_some_and(Value::is_null) {
                return None;
            }
        }
        let edit: Self = serde_json::from_value(value).ok()?;
        edit.valid().then_some(edit)
    }

    pub fn valid(&self) -> bool {
        if !self.target.valid()
            || !token(&self.operation)
            || self.create == Some(false)
            || self.delete == Some(false)
        {
            return false;
        }
        if self.create() {
            if self.base_revision.is_some() {
                return false;
            }
        } else if self
            .base_revision
            .as_deref()
            .is_none_or(|id| decode_exact::<32>(id).is_none())
        {
            return false;
        }
        if self.delete() || self.resolve.is_some() {
            return !self.create()
                && self.fields.is_none()
                && self.notes.is_none()
                && self.attachments.is_none()
                && !(self.delete() && self.resolve.is_some())
                && self
                    .resolve
                    .as_ref()
                    .is_none_or(|choice| decode_exact::<32>(&choice.id).is_some());
        }
        self.fields.as_ref().is_none_or(|fields| fields.len() <= 64)
            && self.notes.as_ref().is_none_or(|notes| {
                notes.len() <= 40
                    && notes.iter().all(|note| {
                        token(&note.id)
                            && !note.text.trim().is_empty()
                            && note.text.encode_utf16().count() <= 20_000
                    })
            })
            && self.attachments.as_ref().is_none_or(|attachments| {
                attachments.len() <= 50
                    && attachments.iter().all(|attachment| {
                        token(&attachment.id)
                            && attachment.file["uri"]
                                .as_str()
                                .is_some_and(|uri| uri.starts_with("data:"))
                    })
            })
            && (self.fields.is_some()
                || self.notes.as_ref().is_some_and(|notes| !notes.is_empty())
                || self
                    .attachments
                    .as_ref()
                    .is_some_and(|attachments| !attachments.is_empty()))
    }

    pub fn create(&self) -> bool {
        self.create == Some(true)
    }
    pub fn delete(&self) -> bool {
        self.delete == Some(true)
    }
}

pub(super) fn token(id: &str) -> bool {
    (1..=80).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn op(setup: &str, nonce: &str) -> String {
    key(json!([setup, nonce]))
}

pub(super) fn legacy_op(setup: &str, sender: &str, nonce: &str) -> String {
    key(json!([setup, sender, nonce]))
}

pub(super) fn key(value: Value) -> String {
    kuvo_wire::keys::encode(&kuvo_wire::json::digest(&value))
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(super) enum Submitter {
    Computer,
    Paired { key: String, pairing: String },
}

impl Submitter {
    pub fn of(caller: &Caller) -> Self {
        caller
            .pairing()
            .map_or(Self::Computer, |pairing| Self::Paired {
                key: kuvo_wire::keys::encode(&pairing.key),
                pairing: pairing.id.clone(),
            })
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Envelope {
    pub setup: String,
    pub nonce: String,
    pub edit: Edit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitter: Option<Submitter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_only: Option<bool>,
}

impl Envelope {
    pub fn valid(&self, sender: &str) -> bool {
        self.edit.valid()
            && token(&self.nonce)
            && (self.edit.operation == op(&self.setup, &self.nonce)
                || self.edit.operation == legacy_op(&self.setup, sender, &self.nonce))
    }
}

pub(super) fn status(receipt: &Value) -> u16 {
    match receipt["status"].as_str() {
        Some("pending") => 202,
        Some("capacity") => 413,
        Some("interrupted" | "deleted" | "refresh") => 409,
        _ => 200,
    }
}

pub(super) fn pending(operation: &str) -> Value {
    json!({"operation":operation,"status":"pending","message":"Saved here. Waiting for the current owner to accept it."})
}

pub(super) fn interrupted(operation: &str, message: &str) -> Value {
    json!({"operation":operation,"status":"interrupted","message":message})
}
