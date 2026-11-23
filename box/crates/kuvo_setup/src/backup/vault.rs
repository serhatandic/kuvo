//! Separate vault points. The vault remains the only authority for values and
//! for creating new items. Its platform backend never enters a computer backup.
use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Backups, DestinationStore, Error, PointId, Reason, RecoveryKey, Result, Source, archive,
    context,
};

/// Implemented by the vault owner with Helper Keychain on Mac and the unlocked
/// wrapped-key file store elsewhere.
pub trait VaultBackend: Send + Sync {
    /// Whether this resident currently owns the vault service.
    fn holds_vault(&self) -> bool;
    fn state(&self) -> Result<VaultState>;
    fn items(&self) -> Result<Vec<VaultItem>>;
    fn values(&self, id: &str) -> Result<BTreeMap<String, String>>;
    /// Create a new item with a new ID and no inherited grants or usage.
    fn import_item(&self, item: VaultExportItem) -> Result<()>;
}

pub struct VaultState {
    pub made: bool,
    pub locked: bool,
    pub tampered: bool,
    pub generations: BTreeMap<String, u64>,
}

impl VaultState {
    fn healthy(&self) -> bool {
        self.made && !self.locked && !self.tampered
    }
}

#[derive(Deserialize, Serialize)]
pub struct VaultItem {
    pub id: String,
    pub name: String,
    pub fields: Vec<String>,
    #[serde(flatten)]
    pub metadata: serde_json::Map<String, Value>,
}

// On import, old exports need only a name and values. The vault backend
// applies its ordinary creation validation to all optional metadata.
#[derive(Deserialize, Serialize)]
pub struct VaultExportItem {
    pub name: String,
    pub values: BTreeMap<String, String>,
    #[serde(flatten)]
    pub metadata: serde_json::Map<String, Value>,
}

#[derive(Deserialize, Serialize)]
struct Point {
    version: u64,
    id: PointId,
    source: Source,
    items: Vec<VaultExportItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    id: PointId,
    source: Source,
    recovery_key: String,
}

#[derive(Serialize)]
pub struct Imported {
    imported: usize,
    skipped: Vec<String>,
    grants: &'static str,
}

fn healthy(vault: &dyn VaultBackend) -> Result<VaultState> {
    let state = vault.state()?;
    if !state.healthy() {
        return Err(Error::Message("Open an unlocked, healthy vault first."));
    }
    Ok(state)
}

impl Backups {
    pub fn export_vault(
        &self,
        store: &(impl DestinationStore + ?Sized),
        source: &Source,
        vault: &dyn VaultBackend,
        now: i64,
    ) -> Result<Exported> {
        let (_guard, scratch) = self.scratch()?;
        let before = healthy(vault)?;
        let mut items = Vec::new();
        for VaultItem {
            id,
            name,
            fields,
            mut metadata,
        } in vault.items()?
        {
            let values = vault.values(&id)?;
            if !fields.iter().all(|field| values.contains_key(field)) {
                return Err(changed());
            }
            metadata.insert("fields".into(), serde_json::to_value(fields)?);
            metadata.insert("id".into(), Value::String(id));
            items.push(VaultExportItem {
                name,
                values,
                metadata,
            });
        }
        let after = vault.state()?;
        if after.tampered || after.generations != before.generations {
            return Err(changed());
        }
        let key = RecoveryKey::generate()?;
        let id = PointId::generate(Reason::Vault, now)?;
        let aad = context(source, &id);
        let point = Point {
            version: 1,
            id: id.clone(),
            source: source.clone(),
            items,
        };
        let mut draft = tempfile::NamedTempFile::new_in(scratch.path())?;
        archive::seal(
            draft.as_file_mut(),
            &key,
            &aad,
            [archive::Entry::json("vault.json", &point)],
        )?;
        store.put(&id, draft.path())?;
        if let Err(error) = store
            .get(&id, scratch.path())
            .and_then(|file| archive::authenticate(&file, &key, &aad, None))
        {
            let _ = store.erase(&id);
            return Err(error);
        }
        Ok(Exported {
            id,
            source: source.clone(),
            recovery_key: key.expose(),
        })
    }

    pub fn import_vault(
        &self,
        store: &(impl DestinationStore + ?Sized),
        source: &Source,
        id: &PointId,
        key: &RecoveryKey,
        vault: &dyn VaultBackend,
    ) -> Result<Imported> {
        let (_guard, scratch) = self.scratch()?;
        healthy(vault)?;
        if id.reason() != Reason::Vault {
            return Err(Error::Message("Choose a vault export."));
        }
        let file = store.get(id, scratch.path())?;
        let point: Point = archive::single_json(
            &file,
            key,
            &context(source, id),
            scratch.path(),
            "vault.json",
        )?;
        if point.version != 1 || point.source != *source || point.id != *id {
            return Err(Error::Message("The export does not match that vault."));
        }
        let mut existing: HashSet<_> = vault
            .items()?
            .iter()
            .map(|item| item.name.to_lowercase())
            .collect();
        let mut result = Imported {
            imported: 0,
            skipped: Vec::new(),
            grants: "approval required",
        };
        for mut item in point.items {
            if existing.insert(item.name.to_lowercase()) {
                for field in ["id", "grants", "used"] {
                    item.metadata.shift_remove(field);
                }
                vault.import_item(item)?;
                result.imported += 1;
            } else {
                result.skipped.push(item.name);
            }
        }
        Ok(result)
    }
}

fn changed() -> Error {
    Error::Message("The vault changed or a value was unavailable. Try exporting again.")
}
