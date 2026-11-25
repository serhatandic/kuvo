//! Preserve the address and creation receipt of items whose deleted rows expire.

use super::*;
use std::collections::HashSet;

pub(super) const RETAIN_DELETED: i64 = 30 * 86_400_000;

#[derive(Clone, Copy)]
pub(super) enum Fence {
    Deleted,
    Departed,
}

impl Fence {
    fn path(self, store: &Store, id: &str) -> PathBuf {
        let family = match self {
            Self::Deleted => "edit-deletions",
            Self::Departed => "edit-departures",
        };
        store
            .root
            .parent()
            .expect("Work Queue under run")
            .join("owned-data")
            .join(family)
            .join(format!("{}.json", key(json!([id]))))
    }
}

pub(super) struct Compaction {
    expired: HashSet<String>,
    owners: Vec<String>,
    sync_mentions: Option<std::collections::HashMap<String, u32>>,
}

impl Writer {
    pub(super) fn fence(&self, family: Fence, id: &str) -> Result<Option<Fields>, Refusal> {
        RECEIPT
            .read(&family.path(&self.store, id))
            .map(|record| record.map(|record| record.fields))
            .map_err(Error::from)
            .map_err(storage_refusal)
    }

    pub(super) fn note_departed(
        &self,
        item: &Item,
        queue: &str,
        computer: &str,
    ) -> Result<(), Refusal> {
        let path = Fence::Departed.path(&self.store, &item.id);
        self.fence(Fence::Departed, &item.id)?;
        let mut fields = Fields::from_iter([
            ("queue".into(), queue.into()),
            ("computer".into(), computer.into()),
        ]);
        if let Some(creation) = item.saved.get("creationReceipt") {
            fields.insert("creationReceipt".into(), creation.clone());
        }
        let bytes = serde_json::to_vec(&RECEIPT.encode(fields)).map_err(internal)?;
        let run = self.store.root.parent().expect("Work Queue under run");
        files::write_atomic_through(&path, &bytes, run).map_err(internal)
    }

    pub(super) fn changes(&self, now: i64) -> Result<(Vec<Item>, Compaction), Refusal> {
        let (owners, mut items) = self.strict_items(now)?;
        let mut expired = HashSet::new();
        for item in &items {
            if !item
                .removed
                .as_ref()
                .is_some_and(|stamp| now as f64 - stamp.at > RETAIN_DELETED as f64)
            {
                continue;
            }
            let path = Fence::Deleted.path(&self.store, &item.id);
            let mut record = Fields::from_iter([
                ("id".into(), item.id.clone().into()),
                ("queue".into(), item.owner.clone().into()),
                ("at".into(), item.saved["removed"]["at"].clone()),
            ]);
            if let Some(receipt) = item.saved.get("creationReceipt") {
                record.insert("creationReceipt".into(), receipt.clone());
            }
            // Validate any existing fence before replacing it. Retain the fence even if the mutation is refused.
            self.fence(Fence::Deleted, &item.id)?;
            let bytes = serde_json::to_vec(&RECEIPT.encode(record)).map_err(internal)?;
            let run = self.store.root.parent().expect("Work Queue under run");
            files::write_atomic_through(&path, &bytes, run)
                .map_err(Error::from)
                .map_err(storage_refusal)?;
            expired.insert(item.id.clone());
        }
        if !expired.is_empty() {
            items.retain_mut(|item| {
                if expired.contains(&item.id) {
                    return false;
                }
                prune(&mut item.saved, &expired);
                true
            });
        }
        let sync_mentions = crate::queue::mentions::before_numbering(&items);
        Ok((
            items,
            Compaction {
                expired,
                owners,
                sync_mentions,
            },
        ))
    }
    /// Only the rows whose missing or unbound index changed need writing. This
    /// runs on the disk worker, under the caller's existing queue lock.
    pub(super) fn sync_mentions(
        &self,
        now: i64,
        before: &std::collections::HashMap<String, u32>,
    ) -> Result<(), Refusal> {
        let all = self.store.items().map_err(storage_refusal)?;
        let updates = crate::queue::mentions::sync_indexes(&self.refs.mentions, &all, before);
        let mut documents = std::collections::BTreeMap::new();
        let mut refused = HashSet::new();
        for (at, index) in updates {
            let item = &all[at];
            if refused.contains(&item.owner) {
                continue;
            }
            if !documents.contains_key(&item.owner) && self.admit(&item.owner).is_err() {
                refused.insert(item.owner.clone());
                continue;
            }
            let document = self.document(&mut documents, &item.owner, now)?;
            if let Some(row) = document
                .items
                .iter_mut()
                .find(|row| row["id"] == item.id)
                .and_then(Value::as_object_mut)
            {
                row.insert("mentionIndex".into(), json!(index));
                row.remove("mentionLinks");
            }
        }
        for document in documents.into_values() {
            document.save(&self.store).map_err(storage_refusal)?;
        }
        Ok(())
    }
}

impl Compaction {
    pub(super) fn finish(self, writer: &Writer, now: i64) -> Result<(), Refusal> {
        if self.expired.is_empty() {
            return if let Some(before) = self.sync_mentions {
                writer.sync_mentions(now, &before)
            } else {
                Ok(())
            };
        }
        for owner in &self.owners {
            let mut document = writer.read_owner(owner, now)?;
            let before = document.items.len();
            document
                .items
                .retain(|row| !expired(&self.expired, &row["id"]));
            let mut changed = document.items.len() != before;
            for row in document.items.iter_mut().filter_map(Value::as_object_mut) {
                changed |= prune(row, &self.expired);
            }
            if changed {
                if document.items.is_empty() {
                    files::remove_file_synced(&document.path)
                        .map_err(Error::from)
                        .map_err(storage_refusal)?;
                } else {
                    document.save(&writer.store).map_err(storage_refusal)?;
                }
            }
        }
        let parent = writer.store.root.join("files");
        if let Ok(resolved) = parent.canonicalize()
            && resolved.parent() == writer.store.root.canonicalize().ok().as_deref()
            && resolved.file_name() == parent.file_name()
        {
            for id in self.expired {
                let path = parent.join(id);
                let result = match fs::symlink_metadata(&path) {
                    Ok(meta) if !meta.is_dir() => files::remove_file_synced(&path),
                    _ => files::remove_tree_synced(&path),
                };
                result.map_err(Error::from).map_err(storage_refusal)?;
            }
        }
        if let Some(before) = self.sync_mentions {
            writer.sync_mentions(now, &before)?;
        }
        Ok(())
    }
}

fn expired(ids: &HashSet<String>, id: &Value) -> bool {
    id.as_str().is_some_and(|id| ids.contains(id))
}

fn prune(row: &mut Fields, ids: &HashSet<String>) -> bool {
    let Some(links) = row.get_mut("relations").and_then(Value::as_array_mut) else {
        return false;
    };
    let before = links.len();
    links.retain(|link| !expired(ids, &link["item"]));
    if links.is_empty() {
        row.remove("relations");
        true
    } else {
        links.len() != before
    }
}
