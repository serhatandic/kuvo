//! Canonical path resolution for artifacts (symlinks resolved, nearest existing parent for
//! paths not yet or no longer on disk), matching `artifacts/paths.ts`.

use std::path::{Path, PathBuf};

/// Strip Windows' verbatim `\\?\` prefix so canonical paths match Node's `realpathSync`.
pub(crate) fn clean_canonical(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{stripped}"));
        }
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

/// Where `path` really is through every symlink; for one not on disk, its nearest existing
/// ancestor with the remaining components appended.
pub fn real_of(path: &Path) -> Option<PathBuf> {
    let mut tail = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            let mut out = clean_canonical(real);
            for part in tail.into_iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        let name = at.file_name()?;
        let parent = at.parent()?;
        if parent == at {
            return None;
        }
        tail.push(name.to_os_string());
        at = parent;
    }
}

/// `path` spelled under the first of `folders` that contains it (through links), or as given.
pub fn spelled_in(path: &Path, folders: &[Option<&Path>]) -> PathBuf {
    let Some(real) = real_of(path) else {
        return path.to_path_buf();
    };
    for folder in folders.iter().flatten() {
        let Some(home) = real_of(folder) else {
            continue;
        };
        if real == home {
            return folder.to_path_buf();
        }
        if let Ok(rel) = real.strip_prefix(&home) {
            return folder.join(rel);
        }
    }
    path.to_path_buf()
}
