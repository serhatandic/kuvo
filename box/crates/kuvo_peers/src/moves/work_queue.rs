//! The conversation transfer worker calls the Work Queue keeper through this port.

use super::manifest::{FileProof, QueueFile};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io, path::Path};

pub struct StagedQueue {
    pub carried: Value,
    pub snapshot: QueueSnapshot,
}

pub struct QueueSnapshot {
    pub files: Vec<QueueFile>,
    pub fingerprint: String,
    pub records: Vec<String>,
}

/// Old manifests can promise the size without a digest. New manifests must
/// give both. In both cases the verified archive supplies the actual digest.
pub struct ExpectedQueueFile {
    pub relative: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct QueuePlacement {
    pub items: Vec<PlacedItem>,
    pub deletions: Vec<Value>,
}

impl QueuePlacement {
    pub fn records(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|item| item.id.clone())
            .chain(
                self.deletions
                    .iter()
                    .filter_map(|item| item["id"].as_str().map(str::to_owned)),
            )
            .collect()
    }
}

#[derive(Deserialize, Serialize)]
pub struct PlacedItem {
    pub id: String,
    pub owner: String,
}

pub trait WorkQueueTransfer: Send + Sync {
    fn stage(
        &self,
        owners: &[String],
        own: Option<&str>,
        directory: &Path,
        allowance: u64,
    ) -> io::Result<Option<StagedQueue>>;

    fn source_fingerprint(
        &self,
        owners: &[String],
        own: Option<&str>,
    ) -> io::Result<Option<String>>;

    fn expected_files(
        &self,
        carried: &Value,
        owners: &[String],
        require_proof: bool,
    ) -> io::Result<Vec<ExpectedQueueFile>>;

    fn install(
        &self,
        carried: Value,
        owners: &[String],
        staged: &Path,
        files: &[QueueFile],
        movement: &str,
        journal: &mut dyn FnMut(QueuePlacement) -> io::Result<()>,
    ) -> io::Result<()>;

    fn verify(&self, files: &[QueueFile]) -> io::Result<()>;

    fn finish(
        &self,
        movement: &str,
        placement: &QueuePlacement,
        files: &[QueueFile],
        roll_back: bool,
    ) -> io::Result<()>;

    fn retire(&self, owners: &[String]) -> io::Result<()>;
}

impl ExpectedQueueFile {
    pub fn installed(self, sha256: String) -> QueueFile {
        QueueFile {
            relative: self.relative,
            proof: FileProof {
                size: self.size,
                sha256,
            },
        }
    }
}
