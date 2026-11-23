//! Folders (PROTOCOL.md, `kuvo/folders`): this computer's folders, for a client to choose one, a
//! new one made, and every file under one (`kuvo/folders/tree`), for a project's file filter, "@"
//! and finding files.

use std::fs::{self, DirEntry};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use http::StatusCode;
use kuvo_os::platform::Platform;
use kuvo_server::Refusal;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::address::{Query, resolved};
use super::git;
use super::order::FinderOrder;

/// How many files a tree lists at most.
pub(crate) const TREE_LIMIT: usize = 20_000;
/// Folders a walk outside git skips: what builds and installs fill.
const HEAVY: &[&str] = &[
    ".git",
    "node_modules",
    "build",
    "DerivedData",
    ".build",
    "Pods",
    "dist",
    ".next",
];
/// What a Files panel's folder leaves out (`files=1`): version control's own folders.
const VERSION_CONTROL: &[&str] = &[".git", ".hg", ".svn"];
const NOT_A_FOLDER_PATH: &str = "Send a folder's full path, or one that starts with ~/.";
const STOPPED: &str = "Stopped: no one asks any longer.";

/// This computer's home folder and the folders in it the system asks you about first.
#[derive(Clone, Debug)]
pub struct Places {
    pub home: PathBuf,
    /// This box's own folder (`~/.kuvo-code`).
    pub box_home: PathBuf,
    pub platform: Platform,
}

impl Places {
    fn asks_about(&self, folder: &Path) -> &'static [&'static str] {
        if folder == self.home {
            self.platform.prompted_folders()
        } else {
            &[]
        }
    }

    /// The folder a request's `path` names (none or `~`: the home folder; `~/` starts there), or
    /// none for one that isn't a full path.
    fn asked_folder(&self, given: Option<&str>) -> Option<PathBuf> {
        let given = given.unwrap_or("").trim();
        let folder = match given {
            "" | "~" => self.home.clone(),
            _ => match given.strip_prefix("~/") {
                Some(inside) => self.home.join(inside),
                None => PathBuf::from(given),
            },
        };
        folder.is_absolute().then(|| resolved(&folder))
    }
}

/// Why a folder couldn't be read, by why: 404 for one that isn't there (or isn't a folder), 403
/// for one this box isn't let into, 500 for anything else, each with the system's reason.
pub(crate) fn refusal(at: &Path, error: &io::Error) -> Refusal {
    let at = at.display();
    let (status, message, reason) = match error.kind() {
        io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            format!("There's no folder {at} here."),
            "No such folder".to_owned(),
        ),
        io::ErrorKind::NotADirectory => (
            StatusCode::NOT_FOUND,
            format!("{at} isn't a folder."),
            "Not a folder".to_owned(),
        ),
        io::ErrorKind::PermissionDenied => (
            StatusCode::FORBIDDEN,
            format!("{at} can't be read here."),
            "Permission denied".to_owned(),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{at} can't be read here."),
            said(error),
        ),
    };
    Refusal::json(
        status,
        serde_json::json!({ "message": message, "reason": reason }),
    )
}

/// The system's reason for an error, as Node says it (libuv's words, capitalized), so a client
/// shows the same words from either box.
fn said(error: &io::Error) -> String {
    let words = platform::libuv_words(error).map_or_else(
        || {
            error
                .to_string()
                .split(" (os error")
                .next()
                .unwrap_or("")
                .to_owned()
        },
        str::to_owned,
    );
    let words = words.split(',').next().unwrap_or("").trim();
    let mut letters = words.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => "It couldn't be read".to_owned(),
    }
}

#[cfg(unix)]
mod platform {
    /// libuv's message for an error number (`uv_strerror`), as Node's errors carry it.
    pub(super) fn libuv_words(error: &std::io::Error) -> Option<&'static str> {
        Some(match error.raw_os_error()? {
            libc::E2BIG => "argument list too long",
            libc::EACCES => "permission denied",
            libc::EAGAIN => "resource temporarily unavailable",
            libc::EBADF => "bad file descriptor",
            libc::EBUSY => "resource busy or locked",
            libc::ECANCELED => "operation canceled",
            libc::EEXIST => "file already exists",
            libc::EFAULT => "bad address in system call argument",
            libc::EFBIG => "file too large",
            libc::EILSEQ => "illegal byte sequence",
            libc::EINTR => "interrupted system call",
            libc::EINVAL => "invalid argument",
            libc::EIO => "i/o error",
            libc::EISDIR => "illegal operation on a directory",
            libc::ELOOP => "too many symbolic links encountered",
            libc::EMFILE => "too many open files",
            libc::ENAMETOOLONG => "name too long",
            libc::ENFILE => "file table overflow",
            libc::ENODEV => "no such device",
            libc::ENOENT => "no such file or directory",
            libc::ENOMEM => "not enough memory",
            libc::ENOSPC => "no space left on device",
            libc::ENOSYS => "function not implemented",
            libc::ENOTDIR => "not a directory",
            libc::ENOTEMPTY => "directory not empty",
            libc::ENOTSUP => "operation not supported on socket",
            libc::ENXIO => "no such device or address",
            libc::EOVERFLOW => "value too large for defined data type",
            libc::EPERM => "operation not permitted",
            libc::EROFS => "read-only file system",
            libc::ETIMEDOUT => "connection timed out",
            _ => return None,
        })
    }
}

#[cfg(not(unix))]
mod platform {
    use std::io::ErrorKind;

    /// libuv's message for the error libuv maps a Windows error to, by its kind.
    pub(super) fn libuv_words(error: &std::io::Error) -> Option<&'static str> {
        Some(match error.kind() {
            ErrorKind::FilesystemLoop => "too many symbolic links encountered",
            ErrorKind::InvalidFilename => "name too long",
            ErrorKind::ResourceBusy => "resource busy or locked",
            ErrorKind::OutOfMemory => "not enough memory",
            ErrorKind::TimedOut => "connection timed out",
            ErrorKind::InvalidInput => "invalid argument",
            _ => return None,
        })
    }
}

fn is_folder(at: &Path, entry: &DirEntry) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_dir() => true,
        Ok(kind) if kind.is_symlink() => {
            fs::metadata(at.join(entry.file_name())).is_ok_and(|m| m.is_dir())
        }
        _ => false,
    }
}

#[derive(Serialize)]
struct Listed {
    name: String,
    path: PathBuf,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    repository: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderList {
    path: PathBuf,
    home: PathBuf,
    parent: Option<PathBuf>,
    folders: Vec<Listed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_entries: Option<Vec<Listed>>,
}

/// `kuvo/folders?path=`: the folders in one, as a client shows them to choose one, ordered as Finder
/// orders names, hidden ones only with `hidden=1`; with `files=1`, its files too, as a Files panel
/// lists a folder, without version control's own folders and `.DS_Store`.
pub(crate) fn list(places: &Places, query: &Query) -> Result<FolderList, Refusal> {
    let at = places
        .asked_folder(query.get("path"))
        .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, NOT_A_FOLDER_PATH))?;
    let entries: Vec<DirEntry> = fs::read_dir(&at)
        .and_then(Iterator::collect)
        .map_err(|error| refusal(&at, &error))?;
    let hidden = query.get("hidden") == Some("1");
    let panel = query.get("files") == Some("1");
    let asks = places.asks_about(&at);
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for entry in &entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let version_control = VERSION_CONTROL.contains(&name.as_str());
        if is_folder(&at, entry) {
            if panel && version_control {
                continue;
            }
            let path = at.join(&name);
            let repository = !asks.contains(&name.as_str()) && path.join(".git").exists();
            folders.push(Listed {
                name,
                path,
                repository,
            });
        } else if panel && name != ".DS_Store" && !version_control {
            files.push(name);
        }
    }
    let order = FinderOrder::new();
    folders.sort_by(|a, b| order.compare(&a.name, &b.name));
    files.sort_by(|a, b| order.compare(a, b));
    let (files, file_entries) = if panel {
        let entries = files
            .iter()
            .map(|name| Listed {
                name: name.clone(),
                path: at.join(name),
                repository: false,
            })
            .collect();
        (Some(files), Some(entries))
    } else {
        (None, None)
    };
    Ok(FolderList {
        parent: at.parent().map(Path::to_owned),
        path: at,
        home: places.home.clone(),
        folders,
        files,
        file_entries,
    })
}

/// `POST kuvo/folders {"path"}`: a new folder, in one that's there, as a folder panel's New Folder
/// makes one (`~/` starts at the home folder).
pub(crate) fn make(places: &Places, body: &[u8]) -> Result<PathBuf, Refusal> {
    let body: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let given = body
        .get("path")
        .and_then(|path| path.as_str())
        .unwrap_or("")
        .trim();
    let folder = given
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(given), |inside| places.home.join(inside));
    if !folder.is_absolute() {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Send the new folder's full path, or one that starts with ~/.",
        ));
    }
    let at = resolved(&folder);
    let parent = at.parent().unwrap_or(&at).display().to_string();
    // Platform: a Mac's disk usually ignores case, so "Trees" is taken by "trees" there.
    if at.exists() {
        let name = at
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!("There's already something called {name} there."),
        ));
    }
    match fs::create_dir(&at) {
        Ok(()) => Ok(at),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Refusal::new(
            StatusCode::NOT_FOUND,
            format!("There's no folder {parent} here."),
        )),
        Err(_) => Err(Refusal::new(
            StatusCode::FORBIDDEN,
            format!("A folder can't be made in {parent} here."),
        )),
    }
}

/// Every file under a folder, by its path there.
#[derive(Serialize)]
pub(crate) struct FolderTree {
    pub(crate) path: PathBuf,
    pub(crate) files: Vec<String>,
    git: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified: Option<Vec<Option<u64>>>,
}

/// `kuvo/folders/tree?path=`: every file under a named folder, at most `limit`: git's list in a
/// repository (`git`), else (and where git lists nothing there) a walk that skips the heavy folders
/// and doesn't follow links; `dated=1`, when each last changed. Refused as `kuvo/folders` refuses.
pub(crate) async fn tree(
    places: &Places,
    query: &Query,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<FolderTree, Refusal> {
    // (Named, never the home folder by default: a walk of all of it is never what's meant.)
    let named = query
        .get("path")
        .is_some_and(|path| !path.trim().is_empty());
    let at = named
        .then(|| places.asked_folder(query.get("path")))
        .flatten()
        .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, NOT_A_FOLDER_PATH))?;
    if let Err(error) = fs::read_dir(&at) {
        return Err(refusal(&at, &error));
    }
    let listed = tokio::select! {
        listed = git::names(&at, limit) => listed,
        () = cancel.cancelled() => return Err(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, STOPPED)),
    };
    let dated = query.get("dated") == Some("1");
    let (files, git) = match listed {
        Some(listed) if !listed.names.is_empty() || !listed.complete => (listed.names, true),
        _ => {
            let asks = places.asks_about(&at);
            let at = at.clone();
            let cancel = cancel.clone();
            let walked = tokio::task::spawn_blocking(move || walk(&at, asks, limit, &cancel))
                .await
                .map_err(|error| {
                    Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                })?;
            (walked, false)
        }
    };
    if cancel.is_cancelled() {
        return Err(Refusal::new(StatusCode::SERVICE_UNAVAILABLE, STOPPED));
    }
    let modified = dated.then(|| {
        files
            .iter()
            .map(|name| changed_at(&at.join(name)))
            .collect()
    });
    Ok(FolderTree {
        path: at,
        files,
        git,
        modified,
    })
}

/// When a file last changed, in whole milliseconds since 1970.
fn changed_at(file: &Path) -> Option<u64> {
    let since = fs::metadata(file)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?;
    Some((since.as_nanos() as f64 / 1e6).round() as u64)
}

fn walk(top: &Path, asks: &[&str], limit: usize, cancel: &CancellationToken) -> Vec<String> {
    fn inside(
        top: &Path,
        folder: &str,
        asks: &[&str],
        limit: usize,
        cancel: &CancellationToken,
        files: &mut Vec<String>,
    ) -> bool {
        let Ok(entries) = fs::read_dir(if folder.is_empty() {
            top.to_owned()
        } else {
            top.join(folder)
        }) else {
            return true;
        };
        for entry in entries.flatten() {
            if files.len() >= limit || cancel.is_cancelled() {
                return false;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if HEAVY.contains(&name.as_str())
                || (folder.is_empty() && asks.contains(&name.as_str()))
            {
                continue;
            }
            let path = if folder.is_empty() {
                name
            } else {
                format!("{folder}/{name}")
            };
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if !inside(top, &path, asks, limit, cancel, files) {
                    return false;
                }
            } else {
                files.push(path);
            }
        }
        true
    }
    let mut files = Vec::new();
    inside(top, "", asks, limit, cancel, &mut files);
    files
}
