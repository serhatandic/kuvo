//! What a conversation's commands asked for from inside the sandbox: its folder's `inbox`, the one
//! place they may write (`kuvo-wake`'s wake-ups, watches and cancels go there). Moved out of their
//! reach before anything in it is read, then taken in as plain files only, by the names
//! `kuvo-wake` gives them.

use std::{
    fs,
    io::Read,
    path::Path,
    time::{Duration, SystemTime},
};

use super::folder::{WAKE_LIMIT, WATCH_LIMIT, letters, names};

const LARGEST: u64 = 65_536;
const SUMMARY: u64 = 300;

/// A plain file a command left that's its own: not a link, nor another name for one, and small.
fn plain(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > LARGEST {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink() == 1
    }
    #[cfg(not(unix))]
    true
}

fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn head(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(limit)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn place(folder: &Path, name: &str, bytes: &[u8]) -> bool {
    let fresh = folder.join(format!(".new-{name}"));
    let placed = fs::write(&fresh, bytes).is_ok() && fs::rename(&fresh, folder.join(name)).is_ok();
    if !placed {
        let _ = fs::remove_file(&fresh);
    }
    placed
}

fn one_line(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&byte| {
            if matches!(byte, b'\n' | b'\r' | b'\t') {
                b' '
            } else {
                byte
            }
        })
        .collect()
}

/// Takes in what waits in `folder`'s inbox (`now`, seconds, for a new watch's first look).
pub fn take(folder: &Path, now: u64) {
    let inbox = folder.join("inbox");
    let Ok(meta) = fs::symlink_metadata(&inbox) else {
        return;
    };
    // Nothing asked, or something being written there now (`kuvo-wake` builds each in a `.new-`
    // of its own, then puts it in place): left for the next round.
    if !meta.file_type().is_symlink() {
        let Ok(entries) = fs::read_dir(&inbox) else {
            return;
        };
        let mut any = false;
        for entry in entries.flatten() {
            any = true;
            let fresh = entry.file_name().to_string_lossy().starts_with(".new-")
                && entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|at| SystemTime::now().duration_since(at).ok())
                    .is_none_or(|age| age < Duration::from_secs(60));
            if fresh {
                return;
            }
        }
        if !any {
            return;
        }
    }
    let took = folder.join(".took");
    let taking = folder.join(".taking");
    let _ = fs::remove_dir_all(&took);
    let _ = fs::remove_dir_all(&taking);
    // The inbox itself is moved, so a link put where it was moves as the link.
    if fs::rename(&inbox, &taking).is_err() {
        return;
    }
    if real_dir(&taking) && kuvo_store::files::create_private_dirs(&took).is_ok() {
        for part in ["wakes", "summaries", "watches", "cancel"] {
            let _ = fs::rename(taking.join(part), took.join(part));
        }
        let wakes = folder.join("wakes");
        let watches = folder.join("watches");
        let _ = fs::create_dir_all(&wakes);
        let _ = fs::create_dir_all(&watches);
        if real_dir(&took.join("cancel")) {
            for name in names(&took.join("cancel")) {
                if !letters(&name) {
                    continue;
                }
                for wake in names(&wakes) {
                    let ends = wake.ends_with(&format!("-{name}"));
                    let inside = wake.contains(&format!("-{name}-"));
                    if (ends || inside) && wakes.join(&wake).is_file() {
                        let _ = fs::remove_file(wakes.join(&wake));
                    }
                }
                let _ = fs::remove_file(folder.join("summaries").join(&name));
                let _ = fs::remove_dir_all(watches.join(&name));
            }
        }
        if real_dir(&took.join("wakes")) {
            for name in names(&took.join("wakes")) {
                let allowed = name.starts_with(|ch: char| ch.is_ascii_digit())
                    && name.contains('-')
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'@' | b'-'));
                let file = took.join("wakes").join(&name);
                if !allowed || !plain(&file) {
                    continue;
                }
                if names(&wakes).len() >= WAKE_LIMIT {
                    break;
                }
                if let Some(bytes) = head(&file, LARGEST) {
                    place(&wakes, &name, &bytes);
                }
            }
        }
        if real_dir(&took.join("summaries")) {
            let summaries = folder.join("summaries");
            let _ = fs::create_dir_all(&summaries);
            for name in names(&took.join("summaries")) {
                let file = took.join("summaries").join(&name);
                if !letters(&name) || !plain(&file) {
                    continue;
                }
                match head(&file, LARGEST) {
                    Some(bytes) if bytes.len() as u64 <= SUMMARY => {
                        place(&summaries, &name, &one_line(&bytes));
                    }
                    _ => {}
                }
            }
        }
        if real_dir(&took.join("watches")) {
            for name in names(&took.join("watches")) {
                let given = took.join("watches").join(&name);
                let into = watches.join(&name);
                if !letters(&name) || !real_dir(&given) || fs::symlink_metadata(&into).is_ok() {
                    continue;
                }
                if !["every", "check", "note"]
                    .iter()
                    .all(|part| plain(&given.join(part)))
                {
                    continue;
                }
                if names(&watches).len() >= WATCH_LIMIT {
                    break;
                }
                let copied = fs::create_dir(&into).is_ok()
                    && ["every", "check", "note"].iter().all(|part| {
                        head(&given.join(part), LARGEST)
                            .is_some_and(|bytes| fs::write(into.join(part), bytes).is_ok())
                    })
                    && fs::write(into.join("next"), now.to_string()).is_ok();
                if !copied {
                    let _ = fs::remove_dir_all(&into);
                    continue;
                }
                let summary = given.join("summary");
                if plain(&summary)
                    && let Some(bytes) =
                        head(&summary, LARGEST).filter(|b| b.len() as u64 <= SUMMARY)
                {
                    let _ = fs::write(into.join("summary"), one_line(&bytes));
                }
            }
        }
    }
    let _ = fs::remove_dir_all(&took);
    let _ = fs::remove_dir_all(&taking);
    let _ = fs::remove_file(&taking);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_files_by_kuvo_wakes_names_are_taken_in() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path();
        let inbox = folder.join("inbox");
        fs::create_dir_all(inbox.join("wakes")).unwrap();
        fs::create_dir_all(inbox.join("summaries")).unwrap();
        fs::create_dir_all(inbox.join("watches/Wt1")).unwrap();
        fs::create_dir_all(inbox.join("cancel")).unwrap();
        fs::create_dir_all(folder.join("wakes")).unwrap();
        fs::write(folder.join("wakes/100-Old-1d@0900"), "old").unwrap();
        fs::write(inbox.join("cancel/Old"), "").unwrap();
        fs::write(inbox.join("wakes/200-Ab1"), "hello").unwrap();
        fs::write(inbox.join("wakes/bad name"), "no").unwrap();
        fs::write(inbox.join("summaries/Ab1"), "a\nline").unwrap();
        for part in ["every", "check", "note"] {
            fs::write(inbox.join("watches/Wt1").join(part), part).unwrap();
        }
        take(folder, 42);
        assert_eq!(names(&folder.join("wakes")), ["200-Ab1"]);
        assert_eq!(
            fs::read_to_string(folder.join("summaries/Ab1")).unwrap(),
            "a line"
        );
        assert_eq!(
            fs::read_to_string(folder.join("watches/Wt1/next")).unwrap(),
            "42"
        );
        assert!(!inbox.exists());
        assert!(!folder.join(".took").exists());
    }
}
