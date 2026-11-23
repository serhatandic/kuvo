//! Backup side of removal. The setup owner journals the chosen point and
//! provides the verified source generation and destination.
use std::{collections::BTreeMap, path::PathBuf};

use super::{
    Authority, Backups, CaptureSelection, DestinationStore, Error, PointId, Reason, RecoveryKey,
    RestorePoint, Result, Source,
};

pub struct WipeExport<'a> {
    pub source: &'a Source,
    pub id: &'a PointId,
    pub at: i64,
    pub key: &'a RecoveryKey,
    pub projects: &'a [PathBuf],
    /// Archive entry names and base64url SHA-256 hashes from the source scan.
    pub attachment_hashes: Option<&'a BTreeMap<String, String>>,
}

impl Backups {
    /// Export a journaled removal point, or verify the same point after a retry.
    pub fn export_before_wipe(
        &self,
        store: &(impl DestinationStore + ?Sized),
        plan: WipeExport<'_>,
        authority: &impl Authority,
        check_source: impl FnOnce() -> Result<()>,
    ) -> Result<RestorePoint> {
        if plan.id.reason() != Reason::Wipe {
            return Err(Error::Message("Choose a pre-wipe backup point."));
        }
        let (_guard, scratch) = self.scratch()?;
        store.check_export_projects(plan.projects)?;
        self.export_locked(
            store,
            plan.source,
            plan.key,
            plan.id,
            plan.at,
            authority,
            CaptureSelection {
                projects: plan.projects,
                attachments: plan.attachment_hashes,
            },
            scratch.path(),
            check_source,
        )
    }
}
