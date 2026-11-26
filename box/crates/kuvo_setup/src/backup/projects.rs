//! The index records project directories and links separately from file bytes.
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    Error, Result,
    archive::{self, Contents, Entry},
    inventory::{Roots, stat},
    recovery::{Unavailable, output},
};

pub(super) const INDEX: &str = "meta/projects.json";

#[derive(Deserialize, Serialize)]
struct Index {
    entries: Vec<Record>,
    omissions: Vec<Unavailable>,
}

#[derive(Deserialize, Serialize)]
struct Record {
    name: String,
    #[serde(flatten)]
    kind: Kind,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Kind {
    File { mode: u16 },
    Directory { mode: u16 },
    Link { target: String },
}

pub(super) fn selected(
    given: Option<serde_json::Value>,
    roots: &Roots,
    account: &Path,
) -> Result<Vec<PathBuf>> {
    let Some(given) = given else {
        return Ok(Vec::new());
    };
    let projects = given
        .as_array()
        .filter(|projects| projects.len() <= 100)
        .ok_or(Error::Message("Choose up to 100 project folders."))?;
    let mut chosen = Vec::new();
    for value in projects {
        let path = value
            .as_str()
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .ok_or(Error::Message(
                "Choose project folders by their absolute paths.",
            ))?;
        let real = canonical(path)?;
        let tmp = std::env::temp_dir();
        let mut standalone = [account, tmp.as_path()]
            .into_iter()
            .map(|path| canonical(path).unwrap_or_else(|_| path.to_path_buf()));
        if !real.is_dir()
            || standalone.any(|root| root.starts_with(&real))
            || roots.locations().any(|root| {
                let root = canonical(root).unwrap_or_else(|_| root.to_path_buf());
                real.starts_with(&root) || root.starts_with(&real)
            })
        {
            return Err(Error::Message(
                "Choose individual project folders outside Kuvo’s data.",
            ));
        }
        if !chosen.contains(&real) {
            chosen.push(real);
        }
    }
    Ok(chosen)
}

fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let real = path.canonicalize()?;
    #[cfg(windows)]
    let real = kuvo_os::platform::windows_conventional_path(&real).unwrap_or(real);
    Ok(real)
}

pub(super) fn capture(projects: &[PathBuf], entries: &mut Vec<Entry>) -> Result<()> {
    if projects.is_empty() {
        return Ok(());
    }
    let mut index = Index {
        entries: Vec::new(),
        omissions: Vec::new(),
    };
    for (number, project) in projects.iter().enumerate() {
        if canonical(project)? != *project || !project.is_dir() {
            return Err(Error::Message("A selected project folder is unavailable."));
        }
        collect(project, format!("projects/{number}"), &mut index, entries)?;
    }
    entries.push(Entry::json(INDEX, &index)?);
    Ok(())
}

fn collect(path: &Path, name: String, index: &mut Index, entries: &mut Vec<Entry>) -> Result<()> {
    let omit = |reason: &str| Unavailable {
        entry: name.clone(),
        reason: reason.into(),
    };
    if !archive::valid_name(&name) {
        index.omissions.push(omit("unsupported path"));
        return Ok(());
    }
    let meta = fs::symlink_metadata(path)?;
    let kind = if meta.is_symlink() {
        let target = fs::read_link(path)?;
        let Some(target) = target.to_str().filter(|target| safe_link(&name, target)) else {
            index.omissions.push(omit("unsafe symbolic link"));
            return Ok(());
        };
        Kind::Link {
            target: target.into(),
        }
    } else if meta.is_dir() {
        Kind::Directory { mode: mode(&meta) }
    } else if meta.is_file() {
        entries.push(Entry {
            name: name.clone(),
            contents: Contents::File {
                path: path.into(),
                size: None,
                digest: None,
            },
        });
        Kind::File { mode: mode(&meta) }
    } else {
        index.omissions.push(omit("unsupported filesystem entry"));
        return Ok(());
    };
    index.entries.push(Record {
        name: name.clone(),
        kind,
    });
    if meta.is_dir() {
        let mut children = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_unstable_by(|a, b| {
            a.file_name()
                .to_string_lossy()
                .encode_utf16()
                .cmp(b.file_name().to_string_lossy().encode_utf16())
        });
        for child in children {
            collect(
                &child.path(),
                format!("{name}/{}", child.file_name().to_string_lossy()),
                index,
                entries,
            )?;
        }
    }
    Ok(())
}

fn safe_link(name: &str, target: &str) -> bool {
    if target.is_empty() || target.contains(['\\', '\0']) || Path::new(target).is_absolute() {
        return false;
    }
    // Traversing a symlink and then stepping up could leave the project.
    let mut up = name.split('/').count().saturating_sub(3);
    let mut named = false;
    for part in target.split('/') {
        match part {
            "" | "." => (),
            ".." if !named && up > 0 => up -= 1,
            ".." => return false,
            _ if cfg!(windows) && part.contains(':') => return false,
            _ => named = true,
        }
    }
    true
}

pub(super) struct RestorePlan {
    index: Index,
}

impl RestorePlan {
    pub fn read(entries: &BTreeMap<String, PathBuf>) -> Result<Option<Self>> {
        let Some(file) = entries.get(INDEX) else {
            return Ok(None);
        };
        let index: Index = serde_json::from_reader(archive::regular(file)?)
            .map_err(|_| Error::Message("Invalid project index."))?;
        let mut names = HashSet::new();
        let mut directories = HashSet::new();
        for record in &index.entries {
            if !archive::valid_name(&record.name)
                || !record
                    .name
                    .strip_prefix("projects/")
                    .and_then(|rest| rest.split('/').next())
                    .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
                || !names.insert(&record.name)
            {
                return Err(Error::Message("Invalid project path."));
            }
            match &record.kind {
                Kind::File { mode } if *mode <= 0o777 => {
                    if !entries.contains_key(&record.name) {
                        return Err(Error::Message("Missing project file."));
                    }
                }
                Kind::Directory { mode } if *mode <= 0o777 => {
                    directories.insert(record.name.as_str());
                }
                Kind::Link { target } if safe_link(&record.name, target) => (),
                _ => return Err(Error::Message("Invalid project entry.")),
            }
        }
        for name in names {
            let parent = name.rsplit_once('/').expect("project record has parent").0;
            if parent != "projects" && !directories.contains(parent) {
                return Err(Error::Message("Invalid project parent."));
            }
        }
        Ok(Some(Self { index }))
    }

    pub fn finish(self, recovery: &Path) -> Result<Vec<Unavailable>> {
        let mut folders = Vec::new();
        for record in self.index.entries {
            let target = output(recovery, &record.name)?;
            match record.kind {
                Kind::File { mode } => set_mode(&target, mode)?,
                Kind::Directory { mode } => {
                    kuvo_store::files::create_private_dirs(&target)?;
                    folders.push((target, mode));
                }
                Kind::Link { target: link } => {
                    match stat(&target)? {
                        None => {
                            kuvo_store::files::create_private_dirs(
                                target.parent().expect("project path has parent"),
                            )?;
                            symlink(Path::new(&link), &target)?;
                        }
                        Some(info)
                            if info.is_symlink() && fs::read_link(&target)? == Path::new(&link) => {}
                        _ => return Err(Error::Message("Conflicting recovered project link.")),
                    }
                    kuvo_store::files::sync_parent(&target)?;
                }
            }
        }
        folders.sort_by_key(|(path, _)| std::cmp::Reverse(path.as_os_str().len()));
        for (target, mode) in folders {
            set_mode(&target, mode)?;
            kuvo_store::files::sync_parent(&target)?;
        }
        Ok(self.index.omissions)
    }
}

#[cfg(unix)]
fn mode(info: &fs::Metadata) -> u16 {
    use std::os::unix::fs::PermissionsExt;
    (info.permissions().mode() & 0o777) as u16
}

#[cfg(windows)]
fn mode(info: &fs::Metadata) -> u16 {
    if info.permissions().readonly() {
        0o444
    } else {
        0o666
    }
}

fn set_mode(path: &Path, mode: u16) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let file = fs::File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        file.set_permissions(fs::Permissions::from_mode(mode.into()))?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    {
        if !fs::symlink_metadata(path)?.is_symlink() {
            let mut permissions = fs::metadata(path)?.permissions();
            permissions.set_readonly(mode & 0o200 == 0);
            fs::set_permissions(path, permissions)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    if target
        .parent()
        .is_some_and(|parent| parent.join(source).is_dir())
    {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    }
}
