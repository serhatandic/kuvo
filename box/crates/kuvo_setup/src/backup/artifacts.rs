//! Keep the bytes promised by the latest artifact journal records.
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{Error, Result, archive, inventory::Expected, recovery::Unavailable};

#[derive(Default)]
pub(super) struct Inventory {
    copies: BTreeMap<String, Expected>,
    pruned: Vec<Unavailable>,
}

impl Inventory {
    pub fn from_logs(logs: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let mut versions = serde_json::Map::new();
        for log in logs {
            for line in BufReader::new(archive::regular(&log)?).lines() {
                let line = line?;
                let Ok(saved) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(saved) = saved.as_object() else {
                    continue;
                };
                kuvo_store::format::check_version(
                    "Artifact index",
                    saved.get("formatVersion"),
                    0,
                    1,
                )
                .map_err(|error| Error::Detail(error.to_string()))?;
                let (Some(id), Some(version)) = (
                    saved
                        .get("a")
                        .and_then(|a| a.get("id"))
                        .and_then(Value::as_str),
                    saved.get("v").filter(|v| v.is_object()),
                ) else {
                    continue;
                };
                if let Some(number) = version.get("n").filter(|number| number.is_number()) {
                    versions.insert(format!("artifacts/{id}/versions/{number}"), version.clone());
                }
            }
        }
        let mut inventory = Self::default();
        for (entry, version) in versions {
            match version.get("kept").and_then(Value::as_bool) {
                Some(true) => inventory.require(version)?,
                Some(false) if version.get("status").and_then(Value::as_str) != Some("deleted") => {
                    inventory.pruned.push(Unavailable {
                        entry,
                        reason: "artifact version bytes were not kept at the restore point".into(),
                    });
                }
                _ => (),
            }
        }
        Ok(inventory)
    }

    fn require(&mut self, version: Value) -> Result<()> {
        let invalid = || Error::Message("A retained artifact version has an invalid inventory.");
        let hex = version
            .get("hash")
            .and_then(Value::as_str)
            .and_then(|hash| hash.strip_prefix("sha256:"))
            .filter(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or_else(invalid)?;
        let size = version
            .get("size")
            .and_then(Value::as_u64)
            .filter(|size| *size <= 9_007_199_254_740_991)
            .ok_or_else(invalid)?;
        let name = format!("artifacts/copies/{}/{hex}", &hex[..2]);
        let found = self.copies.entry(name).or_default();
        if found.size.is_some_and(|previous| previous != size) {
            return Err(Error::Message(
                "Retained artifact versions disagree about their byte count.",
            ));
        }
        found.size = Some(size);
        found.digest = Some(URL_SAFE_NO_PAD.encode(hex::decode(hex).map_err(|_| invalid())?));
        Ok(())
    }

    pub fn copies(&self) -> impl Iterator<Item = (&str, &Expected)> {
        self.copies
            .iter()
            .map(|(name, expected)| (name.as_str(), expected))
    }

    pub fn prune_report(self) -> Vec<Unavailable> {
        self.pruned
    }
}

pub(super) fn matches(file: &Path, expected: &Expected) -> Result<bool> {
    let mut file = archive::regular(file)?;
    if let Some(size) = expected.size
        && file.metadata()?.len() != size
    {
        return Ok(false);
    }
    if let Some(digest) = &expected.digest {
        let mut hash = Sha256::new();
        io::copy(&mut file, &mut hash)?;
        return Ok(URL_SAFE_NO_PAD.encode(hash.finalize()) == *digest);
    }
    Ok(true)
}
