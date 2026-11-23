//! The move.json record beside the conversation and attachment bytes in a move bundle.

use std::{collections::BTreeMap, path::Path};

use kuvo_store::{files, format};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Kind;

const FORMAT: format::ObjectFormat = format::ObjectFormat {
    family: "Move bundle manifest",
    current: 3,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Version(#[from] format::Error),
    #[error("What came isn’t a move.")]
    Invalid(#[source] serde_json::Error),
    #[error("Saving the move manifest: {0}")]
    Write(#[from] std::io::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileProof {
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionFile {
    pub from: String,
    pub relative: String,
    #[serde(flatten)]
    pub proof: FileProof,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueFile {
    pub relative: String,
    #[serde(flatten)]
    pub proof: FileProof,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lane {
    pub sessions: Vec<String>,
    pub root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Origin {
    pub place: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub group: Value,
    pub messages: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub kind: Kind,
    pub thing: String,
    pub name: String,
    /// OpenCode owns exported session and message JSON.
    pub exports: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<Group>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queues: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane: Option<Lane>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_queue: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_evidence: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_notes: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_files: Option<Vec<SessionFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<Origin>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Manifest {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let fields = serde_json::from_slice(bytes).map_err(Error::Invalid)?;
        let object = FORMAT.decode(Value::Object(fields))?;
        serde_json::from_value(object.fields.into()).map_err(Error::Invalid)
    }

    /// Use the current encoding for the fields this bundle carries. A plain
    /// bundle remains readable by computers from before notes and evidence.
    pub fn write(&self, path: &Path) -> Result<(), Error> {
        let fields = serde_json::to_value(self).map_err(Error::Invalid)?;
        let Value::Object(fields) = fields else {
            unreachable!("a manifest serializes to an object");
        };
        let current = if self.forward_evidence.is_some() {
            FORMAT.current
        } else if self.session_notes.is_some() {
            2
        } else {
            1
        };
        let format = format::ObjectFormat { current, ..FORMAT };
        let bytes = serde_json::to_vec(&format.encode(fields)).map_err(Error::Invalid)?;
        files::write_atomic(path, &bytes)?;
        Ok(())
    }
}
