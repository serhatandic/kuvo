//! Backup side of removal. The setup owner journals the chosen point and
//! provides the verified source generation and destination.
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use kuvo_store::{files, home::Home};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{SetupHandle, storage::Files};

use super::{
    Authority, Backups, CaptureSelection, Destination, DestinationStore, DiskStore, Error, PointId,
    Reason, RecoveryKey, RestorePoint, Result, Source, boundary::Boundary, expired_points,
};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ConfiguredPoint {
    source: Source,
    id: PointId,
    at: i64,
}

#[derive(Deserialize)]
struct Configuration {
    version: u64,
    enabled: bool,
    source: Source,
    key: String,
    dest: Destination,
    disk: Option<u64>,
    projects: Vec<PathBuf>,
}

fn read_configuration(home: &Path) -> Result<Option<(PathBuf, Configuration, Value)>> {
    let file = home.join("backup/configuration.json");
    let Some(bytes) = files::read(&file)? else {
        return Ok(None);
    };
    let raw: Value = serde_json::from_slice(&bytes)?;
    let config: Configuration = serde_json::from_value(raw.clone())?;
    if config.version != 1 {
        return Err(Error::Message("Unsupported backup configuration."));
    }
    Ok(Some((file, config, raw)))
}

fn disk_store(config: &Configuration, erase_boundary: &Path) -> Result<DiskStore> {
    let Destination::Disk { path } = &config.dest else {
        return Err(Error::Message(
            "The verified pre-wipe backup could not be confirmed.",
        ));
    };
    std::fs::metadata(path)
        .map_err(|error| Error::Detail(format!("reading {}: {error}", path.display())))?;
    if files::file_stamp(path)?.map(|stamp| stamp.device) != config.disk {
        return Err(Error::Message("Reconnect the selected backup disk."));
    }
    DiskStore::open(path, erase_boundary, &config.source)
}

pub(crate) fn pre_wipe_backup_enabled(home: &Path) -> Result<bool> {
    Ok(read_configuration(home)?.is_some_and(|(_, config, _)| config.enabled))
}

pub(crate) fn uses_automatic_relay_backup(home: &Path) -> Result<bool> {
    Ok(read_configuration(home)?.is_some_and(|(_, config, _)| {
        config.enabled && matches!(config.dest, Destination::Computer { ssh: None, .. })
    }))
}

/// Capture and authenticate the source's configured backup while its ownership
/// view is still available. The saved point contains no recovery credential.
pub(crate) fn prepare_configured_disk(
    home: &Path,
    erase_boundary: &Path,
    authority: SetupHandle,
) -> Result<Option<ConfiguredPoint>> {
    if !pre_wipe_backup_enabled(home)? {
        return Ok(None);
    }
    let backups =
        Backups::with_environment(Home::new(home.to_owned())?, &std::env::vars_os().collect());
    let (_guard, scratch) = backups.scratch()?;
    let Some((file, config, mut raw)) =
        read_configuration(home)?.filter(|(_, config, _)| config.enabled)
    else {
        return Ok(None);
    };
    let created = (|| -> Result<RestorePoint> {
        let store = disk_store(&config, erase_boundary)?;
        let authority = Boundary::capture_from(
            Files::new(home.to_owned()),
            authority,
            backups.roots.clone(),
        )?;
        let at = jiff::Timestamp::now().as_millisecond();
        let id = PointId::generate(Reason::Wipe, at)?;
        let key = RecoveryKey::parse(&config.key)?;
        let point = backups.export_locked(
            &store,
            super::PointTarget {
                source: &config.source,
                key: &key,
                id: &id,
                at,
            },
            &authority,
            CaptureSelection {
                projects: &config.projects,
                attachments: None,
            },
            scratch.path(),
            || Ok(()),
        )?;
        raw["last"] = serde_json::to_value(&point)?;
        if let Some(fields) = raw.as_object_mut() {
            fields.shift_remove("error");
        }
        files::write_secret(&file, &serde_json::to_vec(&raw)?)?;
        for expired in expired_points(&store.list()?, at)? {
            store.erase(&expired)?;
        }
        Ok(point)
    })();
    match created {
        Ok(point) => Ok(Some(ConfiguredPoint {
            source: config.source,
            id: point.id,
            at: point.at,
        })),
        Err(error) => {
            raw["error"] = Value::String(error.to_string());
            files::write_secret(&file, &serde_json::to_vec(&raw)?)?;
            Err(error)
        }
    }
}

pub(crate) fn with_verified_configured_disk<T>(
    home: &Path,
    erase_boundary: &Path,
    point: Option<&ConfiguredPoint>,
    commit: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let verify = || -> Result<T> {
        let current = read_configuration(home)?.map(|(_, config, _)| config);
        let Some(point) = point else {
            if current.is_some_and(|config| config.enabled) {
                return Err(Error::Message(
                    "The verified pre-wipe backup could not be confirmed.",
                ));
            }
            return Ok(commit()?);
        };
        let config = current
            .filter(|config| config.source == point.source)
            .ok_or(Error::Message("The pre-wipe backup configuration changed."))?;
        let backups = Backups::new(Home::new(home.to_owned())?);
        let store = disk_store(&config, erase_boundary)?;
        backups.verify_for_wipe(
            &store,
            &point.source,
            &RecoveryKey::parse(&config.key)?,
            &point.id,
            || Ok(commit()?),
        )
    };
    verify().map_err(io::Error::other)
}

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
            super::PointTarget {
                source: plan.source,
                key: plan.key,
                id: plan.id,
                at: plan.at,
            },
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
