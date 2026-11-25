//! The backup service journals review decisions; kept rows enter through the live queue writer.
use super::*;
use kuvo_setup::backup::{Error as BackupError, QueueReview, Result as BackupResult};

impl QueueReview for Writer {
    fn can_keep(&self, item: &Value) -> BackupResult<bool> {
        let Some(item) = recovered(item) else {
            return Ok(false);
        };
        self.reviewable(&item).map_err(backup_error)
    }

    fn settle(&self, given: Value, root: &Path, marker: &str, keep: bool) -> BackupResult<()> {
        let item = recovered(&given).ok_or(BackupError::Message(
            "That item is unavailable in this restore point.",
        ))?;
        self.settle_review(item, root, marker, keep)
            .map_err(backup_error)
    }
}

fn recovered(row: &Value) -> Option<Item> {
    let owner = row["owner"].as_str().filter(|id| owner_id(id))?;
    Item::read(owner, row)
}

fn backup_error(refusal: Refusal) -> BackupError {
    use http_body_util::BodyExt;
    let message = tokio::runtime::Handle::current()
        .block_on(refusal.reply().body.collect())
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body.to_bytes()).ok())
        .and_then(|value| value["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "The Work Queue cannot be changed just now.".into());
    BackupError::Detail(message)
}

impl Writer {
    fn reviewable(&self, item: &Item) -> Result<bool, Refusal> {
        if item.removed.is_some()
            || self.admit(&item.owner).is_err()
            || self.fence(Fence::Deleted, &item.id)?.is_some()
            || self.fence(Fence::Departed, &item.id)?.is_some()
        {
            return Ok(false);
        }
        let far = told::items_elsewhere(
            &self.store,
            &[],
            &self.peers.view(),
            "all",
            &super::super::sessions::Sessions::default(),
            &[],
            self.peers.now(),
        )
        .map_err(storage_refusal)?;
        Ok(!far
            .items
            .iter()
            .chain(&far.removed)
            .any(|row| row["id"] == item.id))
    }

    fn settle_review(
        &self,
        mut item: Item,
        recovered: &Path,
        marker: &str,
        keep: bool,
    ) -> Result<(), Refusal> {
        let _guard = self.lock_here()?;
        let now = Timestamp::now().as_millisecond();
        let (all, compaction) = self.changes(now)?;
        let existing = all.iter().find(|row| row.id == item.id);
        if let Some(previous) = existing.and_then(|row| row.saved.get("restoreReview")) {
            if keep && previous == marker {
                compaction.finish(self, now)?;
                self.retell.again();
                return Ok(());
            }
            return Err(bad("This item was already kept under another review."));
        }
        if !keep {
            return compaction.finish(self, now);
        }
        if existing.is_some() || !self.reviewable(&item)? {
            return Err(bad(
                "The item’s owner or deletion conflicts with keeping this copy.",
            ));
        }
        let source = Store::new(&recovered.join("home"));
        item.files = super::super::attachments::restore_files(&source, &self.store, &item)
            .map_err(internal)?;
        item.saved.remove("handing");
        let owner = item.owner.clone();
        let mut row = item.normalized();
        row["restoreReview"] = marker.into();
        let mut document = self.read_owner(&owner, now)?;
        document.items.push(row);
        document.save(&self.store).map_err(storage_refusal)?;
        compaction.finish(self, now)?;
        self.retell.again();
        Ok(())
    }
}
