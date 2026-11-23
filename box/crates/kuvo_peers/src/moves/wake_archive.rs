//! The directory member beside move.json. Deferred links can only name
//! entries already reconstructed within the private staging directory.

use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use kuvo_store::files;
use tar::Entry;

use super::session_files::checked_path;

pub(super) fn measure(folder: &Path, limit: u64) -> io::Result<u64> {
    if !fs::symlink_metadata(folder)?.is_dir() {
        return Err(invalid_tree());
    }
    let root = folder.canonicalize()?;
    let mut pending = vec![folder.to_owned()];
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if meta.is_file() {
            bytes = bytes.saturating_add(meta.len());
            if bytes > limit {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "The move exceeds the bundle size limit.",
                ));
            }
        } else if meta.is_symlink() {
            check_relative(folder, &path, &fs::read_link(&path)?)?;
            resolve_inside(&root, &path)?;
        } else {
            return Err(invalid_tree());
        }
    }
    Ok(bytes)
}

pub(super) struct Staging<'a> {
    label: &'a str,
    root: &'a Path,
    kept: HashSet<PathBuf>,
    links: Vec<Link>,
    has_folder: bool,
}

enum Link {
    Symbolic { path: PathBuf, relative: PathBuf },
    Hard { path: PathBuf, target: PathBuf },
}

impl<'a> Staging<'a> {
    pub(super) fn new(label: &'a str, root: &'a Path) -> Self {
        Self {
            label,
            root,
            kept: HashSet::new(),
            links: Vec::new(),
            has_folder: false,
        }
    }

    pub(super) fn accept(
        &mut self,
        name: &str,
        entry: &mut Entry<'_, impl Read>,
    ) -> io::Result<bool> {
        let kind = entry.header().entry_type();
        if name == self.label {
            if self.has_folder || !kind.is_dir() || entry.size() != 0 {
                return Err(invalid_tree());
            }
            files::create_private_dirs(self.root)?;
            self.has_folder = true;
            return Ok(true);
        }
        let Some(relative) = member(self.label, name) else {
            return Ok(false);
        };
        let path = checked_path(self.root, Path::new(relative))?;
        if !self.kept.insert(path.clone()) {
            return Err(invalid_tree());
        }
        if kind.is_dir() && entry.size() == 0 {
            files::create_private_dirs(&path)?;
        } else if kind.is_file() {
            files::write_generated_new(&path, |out| {
                io::copy(entry, out)?;
                Ok(())
            })?;
        } else if entry.size() == 0 && (kind.is_symlink() || kind.is_hard_link()) {
            let link = entry.link_name()?.ok_or_else(invalid_tree)?;
            if kind.is_symlink() {
                check_relative(self.root, &path, &link)?;
                self.links.push(Link::Symbolic {
                    path,
                    relative: link.into_owned(),
                });
            } else {
                let target = link
                    .to_str()
                    .and_then(|link| member(self.label, link))
                    .ok_or_else(invalid_tree)?;
                let target = checked_path(self.root, Path::new(target))?;
                self.links.push(Link::Hard { path, target });
            }
        } else {
            return Err(invalid_tree());
        }
        Ok(true)
    }

    pub(super) fn seal(self) -> io::Result<()> {
        if !self.has_folder {
            return Err(invalid_tree());
        }
        let root = self.root.canonicalize()?;
        let mut pending = self.links;
        while !pending.is_empty() {
            let mut later = Vec::new();
            let mut progressed = false;
            for link in pending {
                if link.restore(&root)? {
                    progressed = true;
                } else {
                    later.push(link);
                }
            }
            if !progressed {
                return Err(invalid_tree());
            }
            pending = later;
        }
        Ok(())
    }
}

impl Link {
    fn restore(&self, root: &Path) -> io::Result<bool> {
        let (path, target) = match self {
            Self::Symbolic { path, relative } => {
                let from = path.parent().ok_or_else(invalid_tree)?;
                (path, from.join(relative))
            }
            Self::Hard { path, target } => (path, target.clone()),
        };
        let resolved = match resolve_inside(root, &target) {
            Ok(resolved) => resolved,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let parent = path.parent().ok_or_else(invalid_tree)?;
        files::create_private_dirs(parent)?;
        if !parent.canonicalize()?.starts_with(root) {
            return Err(invalid_tree());
        }
        let meta = resolved.metadata()?;
        match self {
            Self::Symbolic { relative, .. } => {
                files::restore_symbolic_link(path, relative, meta.is_dir())?
            }
            Self::Hard { .. } if meta.is_file() => files::restore_hard_link(path, &resolved)?,
            Self::Hard { .. } => return Err(invalid_tree()),
        }
        Ok(true)
    }
}

fn member<'a>(prefix: &str, entry: &'a str) -> Option<&'a str> {
    entry.strip_prefix(prefix)?.strip_prefix('/')
}

fn check_relative(root: &Path, path: &Path, to: &Path) -> io::Result<()> {
    let here = path.parent().ok_or_else(invalid_tree)?;
    let mut depth = here
        .strip_prefix(root)
        .map_err(|_| invalid_tree())?
        .components()
        .count();
    for part in to.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(_) => depth += 1,
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return Err(invalid_tree()),
        }
    }
    Ok(())
}

fn resolve_inside(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let target = path.canonicalize()?;
    if !target.starts_with(root) || target == root {
        return Err(invalid_tree());
    }
    Ok(target)
}

fn invalid_tree() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "It brought a wake-up folder with a link or entry that would not work on this computer.",
    )
}
