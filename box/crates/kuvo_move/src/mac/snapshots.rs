use super::{database::Rekey, invalid, paths::PathMap};
use crate::{Disk, Error, entries, metadata};
use kuvo_store::files;
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Repair engine paths inside its staged snapshot stores. The Git records in
/// repositories outside this private tree are handled by placement.
pub(super) fn rewrite(data: &Disk, map: &PathMap, changes: Vec<Rekey>) -> Result<(), Error> {
    let mut projects = BTreeMap::new();
    let mut snapshots = std::collections::BTreeSet::new();
    for change in changes {
        for (from, to) in change.projects {
            if projects
                .insert(from, to.clone())
                .is_some_and(|prior| prior != to)
            {
                return Err(invalid(
                    "The staged databases map one project to two destinations.",
                ));
            }
        }
        snapshots.extend(change.snapshots);
    }
    for (id, old) in snapshots {
        require_token(&id)?;
        let id_after = projects.get(&id).map_or(id.as_str(), String::as_str);
        let new = map
            .absolute(&old)
            .ok_or_else(|| invalid("A staged snapshot has no destination."))?;
        let from = data.path(Path::new("snapshot").join(&id).join(digest(&old)))?;
        let to = data.path(Path::new("snapshot").join(id_after).join(digest(&new)))?;
        rename(&from, &to)?;
    }
    for (from, to) in projects {
        require_token(&from)?;
        require_token(&to)?;
        for family in ["shell", "snapshot"] {
            let source = data.path(Path::new(family).join(&from))?;
            let target = data.path(Path::new(family).join(&to))?;
            if metadata(&source)?.is_none() {
                continue;
            }
            if metadata(&target)?.is_none() {
                rename(&source, &target)?;
                continue;
            }
            if !metadata(&source)?.is_some_and(|meta| meta.is_dir())
                || !metadata(&target)?.is_some_and(|meta| meta.is_dir())
            {
                return Err(Error::Busy(target));
            }
            for entry in entries(&source)? {
                rename(&entry.path(), &target.join(entry.file_name()))?;
            }
            files::remove_tree_synced(&source)?;
        }
    }
    let snapshot = data.path("snapshot")?;
    for project in directories(&snapshot)? {
        for repository in directories(&project)? {
            for name in ["config", "opencode.gitconfig", "objects/info/alternates"] {
                let file = Disk(repository.clone()).path(name)?;
                if metadata(&file)?.is_some_and(|meta| meta.is_file())
                    && let Some(text) = files::text_of(&file)
                    && let Some(next) = map.lines(&text)
                {
                    files::rewrite_mac_entry(&file, next.as_bytes())?;
                }
            }
        }
    }
    Ok(())
}

fn directories(path: &Path) -> Result<Vec<PathBuf>, Error> {
    if !metadata(path)?.is_some_and(|meta| meta.is_dir()) {
        return Ok(Vec::new());
    }
    let mut children = Vec::new();
    for entry in entries(path)? {
        if entry.file_type()?.is_dir() {
            children.push(entry.path());
        }
    }
    Ok(children)
}

fn require_token(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(
            "The staged database has an invalid project directory ID.",
        ));
    }
    Ok(())
}

fn digest(path: &str) -> String {
    hex::encode(Sha1::digest(path.as_bytes()))
}

fn rename(source: &Path, target: &Path) -> Result<(), Error> {
    if source == target || metadata(source)?.is_none() {
        return Ok(());
    }
    if metadata(target)?.is_some() {
        return Err(Error::Busy(target.into()));
    }
    let parent = target
        .parent()
        .ok_or_else(|| invalid("A staged snapshot has no containing folder."))?;
    files::create_private_dirs(parent)?;
    files::replace_synced(source, target)?;
    Ok(())
}
