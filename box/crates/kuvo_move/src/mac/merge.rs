//! Plan name collisions while both copies are still private.
use crate::{Error, entries, metadata};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Merged {
    pub source: PathBuf,
    pub placed: PathBuf,
}

pub(super) fn clone(
    from: &Path,
    staging: &Path,
    destination: &Path,
    merged: &mut Vec<Merged>,
) -> Result<(), Error> {
    let Some(there) = metadata(staging)? else {
        files::clone_mac_entry(from, staging, false)?;
        return Ok(());
    };
    let incoming = metadata(from)?.ok_or_else(|| Error::UnknownHome(from.into()))?;
    if !there.is_dir() || !incoming.is_dir() {
        return Err(Error::Busy(destination.into()));
    }
    let mut children = entries(from)?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let name = child.file_name();
        merge_entry(
            &child.path(),
            &staging.join(&name),
            &destination.join(&name),
            merged,
        )?;
    }
    Ok(())
}

fn merge_entry(
    from: &Path,
    staging: &Path,
    destination: &Path,
    merged: &mut Vec<Merged>,
) -> Result<(), Error> {
    let Some(there) = metadata(staging)? else {
        files::clone_mac_entry(from, staging, false)?;
        merged.push(Merged {
            source: from.into(),
            placed: destination.into(),
        });
        return Ok(());
    };
    let incoming = metadata(from)?.ok_or_else(|| Error::UnknownHome(from.into()))?;
    if there.is_dir() && incoming.is_dir() {
        return clone(from, staging, destination, merged);
    }
    let stem = staging
        .file_stem()
        .ok_or_else(|| super::invalid("A merged entry needs a name."))?;
    let extension = staging.extension();
    for number in 2.. {
        let mut name = OsString::from(stem);
        name.push(format!(" {number}"));
        if let Some(extension) = extension {
            name.push(".");
            name.push(extension);
        }
        let next = staging.with_file_name(&name);
        if metadata(&next)?.is_none() {
            files::clone_mac_entry(from, &next, false)?;
            merged.push(Merged {
                source: from.into(),
                placed: destination.with_file_name(name),
            });
            return Ok(());
        }
    }
    unreachable!("the next unused copy has a name")
}
