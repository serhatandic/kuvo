//! Keep files left by an earlier installation until the new move commits.
use super::{EntryIdentity, invalid, stamp};
use crate::{Disk, Error, entries, metadata};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

#[derive(Default, Deserialize, Serialize)]
pub(in crate::mac) struct Destination {
    had_trash: bool,
    entries: Vec<Entry>,
}

#[derive(Deserialize, Serialize)]
struct Entry {
    relative: PathBuf,
    identity: EntryIdentity,
}

const NEW: &str = "updates/rehome/new";

impl Destination {
    pub(super) fn capture(disk: &Disk, stamp: u64) -> Result<Self, Error> {
        Self::plan(disk, stamp, false)
    }

    pub(in crate::mac) fn fresh(disk: &Disk, stamp: u64) -> Result<Self, Error> {
        Self::plan(disk, stamp, true)
    }

    fn plan(disk: &Disk, stamp: u64, fresh: bool) -> Result<Self, Error> {
        let trash = disk.path("trash")?;
        let mut plan = Self {
            had_trash: metadata(&trash)?.is_some(),
            entries: Vec::new(),
        };
        let mut candidates = Vec::new();
        for base in ["", "updates"] {
            let folder = if base.is_empty() {
                disk.0.clone()
            } else {
                disk.path(base)?
            };
            for item in entries(&folder)? {
                let relative = Path::new(base).join(item.file_name());
                if archived_name(&relative).is_some() {
                    candidates.push(relative);
                }
            }
        }
        candidates.sort();
        if !fresh && metadata(&disk.path(NEW)?)?.is_some() {
            candidates.push(NEW.into());
        }
        let mut old_trash = entries(&trash)?;
        old_trash.sort_by_key(std::fs::DirEntry::file_name);
        candidates.extend(
            old_trash
                .into_iter()
                .filter(|entry| {
                    !fresh
                        || !entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("earlier-box-")
                })
                .map(|entry| Path::new("trash").join(entry.file_name())),
        );
        for relative in candidates {
            if let Some(meta) = metadata(&disk.path(&relative)?)? {
                plan.entries.push(Entry {
                    relative,
                    identity: EntryIdentity::from(&meta),
                });
            }
        }
        if !plan.entries.is_empty() && metadata(&disk.path(aside(stamp)?)?)?.is_some() {
            return Err(invalid("A kept copy already uses this move's name."));
        }
        Ok(plan)
    }

    pub(super) fn contains(&self, name: &Path) -> bool {
        self.entries.iter().any(|entry| entry.relative == name)
    }

    pub(super) fn validate(&self) -> Result<(), Error> {
        let mut seen = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if archived_name(&entry.relative).is_none() || !seen.insert(&entry.relative) {
                return Err(invalid(
                    "A saved destination path does not belong to the move.",
                ));
            }
        }
        Ok(())
    }

    pub(in crate::mac) fn take(
        &self,
        disk: &Disk,
        time: u64,
        mut first: impl FnMut(),
    ) -> Result<Option<PathBuf>, Error> {
        let aside = aside(time)?;
        for (index, entry) in self.entries.iter().enumerate() {
            let (original, kept) = entry.paths(disk, &aside)?;
            match (metadata(&original)?, metadata(&kept)?) {
                (Some(meta), None) if EntryIdentity::from(&meta) == entry.identity => {
                    files::create_private_dirs(kept.parent().expect("kept copy has a parent"))?;
                    files::place_mac_entry(&original, &kept)?;
                    if index == 0 {
                        first();
                    }
                }
                (None, Some(meta)) if EntryIdentity::from(&meta) == entry.identity => {}
                _ => return Err(Error::Busy(original)),
            }
        }
        (!self.entries.is_empty())
            .then(|| disk.path(&aside))
            .transpose()
            .map_err(Error::from)
    }

    pub(super) fn undo(&self, disk: &Disk, time: u64) -> Result<(), Error> {
        let aside = aside(time)?;
        for entry in self.entries.iter().rev() {
            let (original, kept) = entry.paths(disk, &aside)?;
            if let Some(meta) = metadata(&kept)?
                && EntryIdentity::from(&meta) == entry.identity
            {
                if metadata(&original)?.is_some() {
                    return Err(Error::Busy(original));
                }
                files::place_mac_entry(&kept, &original)?;
            }
        }
        if !self.entries.is_empty() {
            for relative in [aside.join("trash"), aside.join("updates"), aside] {
                let _ = files::remove_empty_dir_synced(&disk.path(relative)?);
            }
            if !self.had_trash {
                let _ = files::remove_empty_dir_synced(&disk.path("trash")?);
            }
        }
        Ok(())
    }

    pub(super) fn actions(&self, disk: &Disk, time: u64) -> Result<Vec<Value>, Error> {
        let aside = aside(time)?;
        let mut made = std::collections::BTreeSet::new();
        let mut actions = Vec::new();
        if !self.entries.is_empty() && !self.had_trash {
            actions.push(
                json!({"act":"made","path":disk.path("trash")?, "folder":true, "formatVersion":1}),
            );
        }
        for entry in &self.entries {
            let (original, kept) = entry.paths(disk, &aside)?;
            let parent = kept.parent().expect("kept copy has a parent");
            let root = disk.path(&aside)?;
            if made.insert(root.clone()) {
                actions.push(json!({"act":"made","path":root,"folder":true,"formatVersion":1}));
            }
            if parent != root && made.insert(parent.to_owned()) {
                actions.push(json!({"act":"made","path":parent,"folder":true,"formatVersion":1}));
            }
            actions.push(json!({"act":"rename","from":original,"to":kept,"formatVersion":1}));
        }
        Ok(actions)
    }
}

impl Entry {
    fn paths(&self, disk: &Disk, aside: &Path) -> Result<(PathBuf, PathBuf), Error> {
        let kept = archived_name(&self.relative)
            .ok_or_else(|| invalid("A saved destination path does not belong to the move."))?;
        Ok((disk.path(&self.relative)?, disk.path(aside.join(kept))?))
    }
}

fn aside(time: u64) -> Result<PathBuf, Error> {
    Ok(Path::new("trash").join(format!("earlier-box-{}", stamp(time)?)))
}

fn archived_name(path: &Path) -> Option<&Path> {
    if path == Path::new(NEW) {
        return Some(Path::new("new-box-mark"));
    }
    let mut parts = path.components();
    let Component::Normal(top) = parts.next()? else {
        return None;
    };
    match parts.next() {
        None if top == "updates"
            || top == "desktop"
            || top == "trash"
            || top.to_string_lossy().starts_with(".folder-upgrade.lock") =>
        {
            None
        }
        None => Some(path),
        Some(Component::Normal(child))
            if parts.next().is_none()
                && ((top == "updates" && child != "rehome" && child != "install")
                    || top == "trash") =>
        {
            Some(path)
        }
        _ => None,
    }
}
