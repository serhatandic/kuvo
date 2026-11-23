use super::{Plan, invalid};
use crate::{Disk, Error, entries, metadata};
use kuvo_store::files;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Repairs {
    edits: Vec<Edit>,
    outside: Vec<Original>,
}

#[derive(Deserialize, Serialize)]
struct Original {
    path: PathBuf,
    was: String,
}

#[derive(Deserialize, Serialize)]
struct Edit {
    original: Original,
    next: String,
}

impl Repairs {
    pub(super) fn validate(&self) -> Result<(), Error> {
        for path in self
            .edits
            .iter()
            .map(|edit| &edit.original.path)
            .chain(self.outside.iter().map(|entry| &entry.path))
        {
            if !path.is_absolute()
                || path.components().any(|part| part == Component::ParentDir)
                || !matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some(".git" | "gitdir")
                )
            {
                return Err(invalid(
                    "An invalid Git record appears in the move journal.",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn apply(&self, mut first: impl FnMut()) -> Result<(), Error> {
        for (index, edit) in self.edits.iter().enumerate() {
            let path = &edit.original.path;
            let read = read_record(path)?.ok_or_else(|| Error::Busy(path.clone()))?;
            if read == edit.next {
                continue;
            }
            if read != edit.original.was {
                return Err(Error::Busy(path.clone()));
            }
            files::rewrite_mac_entry(path, edit.next.as_bytes())?;
            if index == 0 {
                first();
            }
        }
        Ok(())
    }

    pub(super) fn actions(&self) -> impl Iterator<Item = Value> {
        self.outside.iter().map(|record| {
            json!({
                "act":"wrote", "path":record.path, "was":record.was, "formatVersion":1
            })
        })
    }

    fn record(&mut self, link: &Location, was: &str, next: String) {
        if was != next && !self.edits.iter().any(|edit| edit.original.path == link.to) {
            self.edits.push(Edit {
                original: Original {
                    path: link.to.clone(),
                    was: was.into(),
                },
                next,
            });
        }
        if link.staged == link.to {
            self.keep(&link.to, was);
        }
    }

    fn keep(&mut self, path: &Path, was: &str) {
        if !self.outside.iter().any(|record| record.path == path) {
            self.outside.push(Original {
                path: path.into(),
                was: was.into(),
            });
        }
    }
}

struct Mapping {
    old: PathBuf,
    staged: PathBuf,
    to: PathBuf,
}

struct Location {
    old: PathBuf,
    staged: PathBuf,
    to: PathBuf,
}

impl Location {
    fn child(&self, child: &Path) -> Self {
        Self {
            old: self.old.join(child),
            staged: self.staged.join(child),
            to: self.to.join(child),
        }
    }
}

pub(super) fn plan(plan: &Plan, placed: &[usize], disk: &Disk) -> Result<Repairs, Error> {
    let target = disk.0.canonicalize()?;
    let mut map = Vec::new();
    for &index in placed {
        let entry = &plan.placements[index];
        if entry.installed || !metadata(&entry.staged)?.is_some_and(|meta| meta.is_dir()) {
            continue;
        }
        let Some(name) = entry.to.file_name() else {
            continue;
        };
        let to = target.join(name);
        let Ok(old) = entry.from.canonicalize() else {
            continue;
        };
        map.push(Mapping {
            old,
            staged: entry.staged.clone(),
            to,
        });
    }
    let mut candidates = Vec::new();
    for entry in &map {
        walk(&entry.staged, 6, &mut candidates)?;
    }
    let mut repairs = Repairs::default();
    let mut seen = HashSet::new();
    for entry in candidates {
        let Some(location) = map.iter().find_map(|entry_map| {
            let suffix = entry.strip_prefix(&entry_map.staged).ok()?;
            Some(resolve(&entry_map.old.join(suffix), &map))
        }) else {
            continue;
        };
        if metadata(&location.staged)?.is_some_and(|meta| meta.is_dir()) {
            for record in entries(&location.staged.join("worktrees"))? {
                if !record.file_type()?.is_dir() {
                    continue;
                }
                let record = location.child(&Path::new("worktrees").join(record.file_name()));
                let back = record.child(Path::new("gitdir"));
                if let Some(text) = read_record(&back.staged)? {
                    let link = resolve(&absolute(&record.old, text.trim()), &map);
                    repair(&link, &record, &map, &mut repairs, &mut seen)?;
                }
            }
        } else if let Some(text) = read_record(&location.staged)?
            && let Some(reference) = gitdir(&text)
        {
            let parent = location
                .old
                .parent()
                .ok_or_else(|| invalid("A worktree needs a parent."))?;
            let record = resolve(&absolute(parent, reference), &map);
            repair(&location, &record, &map, &mut repairs, &mut seen)?;
        }
    }
    repairs.validate()?;
    Ok(repairs)
}

fn repair(
    link: &Location,
    record: &Location,
    map: &[Mapping],
    repairs: &mut Repairs,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), Error> {
    if link.to.file_name().and_then(|name| name.to_str()) != Some(".git")
        || !seen.insert(link.to.clone())
    {
        return Ok(());
    }
    let back = record.child(Path::new("gitdir"));
    let Some(original_link) = read_record(&link.staged)? else {
        return Ok(());
    };
    let Some(original_back) = read_record(&back.staged)? else {
        return Ok(());
    };
    let Some(reference) = gitdir(&original_link) else {
        return Ok(());
    };
    let folder = link
        .old
        .parent()
        .ok_or_else(|| invalid("A worktree needs a parent."))?;
    // The reciprocal records authorize the repair. A copied .git file alone
    // cannot change an unrelated repository or a worktree it no longer owns.
    if located(&absolute(folder, reference)) != located(&record.old)
        || located(&absolute(&record.old, original_back.trim())) != located(&link.old)
    {
        return Ok(());
    }
    let worktree = link
        .to
        .parent()
        .ok_or_else(|| invalid("A worktree needs a parent."))?;
    let next_back = projected_name(original_back.trim(), &record.to, &link.to);
    repairs.record(
        &back,
        &original_back,
        original_back.replacen(original_back.trim(), &next_back, 1),
    );
    let next = projected_name(reference, worktree, &record.to);
    repairs.record(
        link,
        &original_link,
        original_link.replacen(reference, &next, 1),
    );
    if back.staged == back.to {
        // The previous worktree remains in the source. Keep the record of
        // what Git's repair observed, even when its forward link stays put.
        let old = resolve(&absolute(&record.old, original_back.trim()), &[]);
        if !map.iter().any(|item| old.to.starts_with(&item.to))
            && let Some(text) = read_record(&old.to)?
        {
            repairs.keep(&old.to, &text);
        }
    }
    Ok(())
}

fn resolve(path: &Path, map: &[Mapping]) -> Location {
    let old = located(path);
    if let Some((item, suffix)) = map.iter().find_map(|item| {
        old.strip_prefix(&item.old)
            .ok()
            .map(|suffix| (item, suffix))
    }) {
        return Location {
            old: old.clone(),
            staged: item.staged.join(suffix),
            to: item.to.join(suffix),
        };
    }
    Location {
        old: old.clone(),
        staged: old.clone(),
        to: old,
    }
}

fn located(path: &Path) -> PathBuf {
    // Keep the final component as a directory entry, so a symlink cannot
    // turn a Git-record read or a write into an operation on its target.
    if let Some((parent, name)) = path.parent().zip(path.file_name())
        && let Ok(parent) = parent.canonicalize()
    {
        return parent.join(name);
    }
    path.into()
}

fn read_record(path: &Path) -> Result<Option<String>, Error> {
    if !metadata(path)?.is_some_and(|meta| meta.is_file() && meta.len() <= 64 * 1024) {
        return Ok(None);
    }
    Ok(files::read(path)?.and_then(|bytes| String::from_utf8(bytes).ok()))
}

fn gitdir(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix("gitdir:").map(str::trim))
        .filter(|value| !value.is_empty())
}

fn absolute(base: &Path, value: &str) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in base.join(value).components() {
        match part {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            _ => clean.push(part.as_os_str()),
        }
    }
    clean
}

fn projected_name(previous: &str, base: &Path, target: &Path) -> String {
    if Path::new(previous).is_absolute() {
        return target.to_string_lossy().into();
    }
    let mut lhs = base.components().peekable();
    let mut rhs = target.components().peekable();
    while lhs.peek().is_some() && lhs.peek() == rhs.peek() {
        lhs.next();
        rhs.next();
    }
    lhs.map(|_| Component::ParentDir)
        .chain(rhs)
        .collect::<PathBuf>()
        .to_string_lossy()
        .into()
}

fn walk(folder: &Path, depth: u8, found: &mut Vec<PathBuf>) -> Result<(), Error> {
    let git = folder.join(".git");
    if metadata(&git)?.is_some_and(|meta| meta.is_dir() || meta.is_file()) {
        found.push(git);
        return Ok(());
    }
    if depth == 0 {
        return Ok(());
    }
    let mut children = entries(folder)?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for entry in children {
        if entry.file_type()?.is_dir()
            && !matches!(
                entry.file_name().to_str(),
                Some(
                    "node_modules"
                        | ".git"
                        | "snapshot"
                        | "log"
                        | "engine"
                        | "cache"
                        | "kept"
                        | "staging"
                )
            )
        {
            walk(&entry.path(), depth - 1, found)?;
        }
    }
    Ok(())
}
