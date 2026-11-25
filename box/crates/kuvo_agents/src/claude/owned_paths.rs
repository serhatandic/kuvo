//! Claude Code's own conversation files count as part of the working area.

use std::{
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
};

pub(super) fn owns_path(config: &Path, directory: &Path, given: &Path) -> bool {
    let path = if given.is_absolute() {
        given.to_owned()
    } else {
        directory.join(given)
    };
    if background_output(&path) {
        return true;
    }
    let (Some(config), Some(path)) = (resolve(config), resolve(&path)) else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(config) else {
        return false;
    };
    let mut parts = relative.iter();
    match parts.next().and_then(OsStr::to_str) {
        Some("plans") => {
            parts
                .next()
                .is_some_and(|name| Path::new(name).extension() == Some(OsStr::new("md")))
                && parts.next().is_none()
        }
        Some("projects") => {
            let Some(_project) = parts.next() else {
                return false;
            };
            match parts.next().and_then(OsStr::to_str) {
                Some("memory") => parts.next().is_some(),
                Some(_session) => {
                    parts.next() == Some(OsStr::new("tool-results")) && parts.next().is_some()
                }
                None => false,
            }
        }
        _ => false,
    }
}

fn resolve(path: &Path) -> Option<PathBuf> {
    if path.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    let mut missing = Vec::new();
    for candidate in path.ancestors() {
        match fs::canonicalize(candidate) {
            Ok(mut resolved) => {
                for name in missing.into_iter().rev() {
                    resolved.push(name);
                }
                return Some(resolved);
            }
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && fs::symlink_metadata(candidate).is_err() =>
            {
                missing.push(candidate.file_name()?);
            }
            Err(_) => return None,
        }
    }
    None
}

/// A background command's output, in /tmp's `claude-<uid>` where Claude Code has no temporary
/// folder of its own (in the sandbox): `<project>/<session>/tasks/<file>`.
fn background_output(path: &Path) -> bool {
    let Some(uid) = kuvo_os::platform::user_id() else {
        return false;
    };
    let Some(text) = path.to_str() else {
        return false;
    };
    ["/tmp", "/private/tmp"].into_iter().any(|tmp| {
        text.strip_prefix(&format!("{tmp}/claude-{uid}/"))
            .is_some_and(|rest| {
                let parts = rest.split('/').collect::<Vec<_>>();
                matches!(parts.as_slice(), [project, session, "tasks", file]
                    if ![project, session, file].iter().any(|part| part.is_empty() || **part == ".."))
            })
    })
}
