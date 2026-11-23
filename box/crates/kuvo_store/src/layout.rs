use crate::{files, format, home::Home, locks::OwnedLock};
use serde_json::{Value, json};
use std::{fs, io, path::Path, time::Duration};
use thiserror::Error;

pub const VERSION: u32 = 23;
pub const FILE: &str = "folder-version.json";
const LOCK: &str = ".folder-upgrade.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Current,
    Unmarked,
    Older { version: u32 },
    Interrupted { version: u32, upgrading_to: u32 },
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Format(#[from] format::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("Kuvo folder layout {version} requires the move to layout {VERSION}.")]
    MoveRequired { version: u32 },
    #[error("Kuvo folder layout {version} has an unfinished move to layout {upgrading_to}.")]
    ResumeMove { version: u32, upgrading_to: u32 },
}

pub fn inspect(home: &Home) -> Result<Layout, Error> {
    let path = home.root().join(FILE);
    let Some(bytes) = files::read(&path)? else {
        return match fs::symlink_metadata(&path) {
            // A dangling link is a marker we could not read.
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("cannot read {}", path.display()),
            )
            .into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Layout::Unmarked),
            Err(error) => Err(error.into()),
        };
    };
    let value: Value =
        serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap_or(Value::Null);
    let family = format!("Kuvo folder at {}", home.root().display());
    let found = value.get("version");
    let found = found.ok_or_else(|| format::unsupported(&family, None, VERSION))?;
    let version = format::check_version(&family, Some(found), 0, VERSION)?;
    if let Some(found) = value.get("upgradingTo") {
        let upgrading_to = format::check_version(&family, Some(found), 0, VERSION)?;
        if upgrading_to != version + 1 {
            return Err(format::unsupported(&family, Some(found), VERSION).into());
        }
        return Ok(Layout::Interrupted {
            version,
            upgrading_to,
        });
    }
    Ok(if version == VERSION {
        Layout::Current
    } else {
        Layout::Older { version }
    })
}

/// Report the committed version to a backup reader, including legacy layouts.
pub fn backup_version(home: &Home) -> Result<u32, Error> {
    match inspect(home)? {
        Layout::Current => Ok(VERSION),
        Layout::Unmarked => Ok(0),
        Layout::Older { version } => Ok(version),
        Layout::Interrupted {
            version,
            upgrading_to,
        } => Err(Error::ResumeMove {
            version,
            upgrading_to,
        }),
    }
}

/// A current home opens read-only here. Only an empty home can be initialized
/// by the resident. The move owns every existing older home and its checkpoints.
pub fn open(home: &Home) -> Result<(), Error> {
    match inspect(home)? {
        Layout::Current => return Ok(()),
        Layout::Unmarked => {}
        other => return require_current(other),
    }
    if !empty(home.root())? {
        return require_current(inspect(home)?);
    }
    let _guard = lock_for_move(home, Duration::from_secs(3))?;
    let inside = inspect(home)?;
    if inside != Layout::Unmarked || !empty(home.root())? {
        return require_current(inside);
    }
    publish(home)
}

fn require_current(layout: Layout) -> Result<(), Error> {
    match layout {
        Layout::Current => Ok(()),
        Layout::Unmarked => Err(Error::MoveRequired { version: 0 }),
        Layout::Older { version } => Err(Error::MoveRequired { version }),
        Layout::Interrupted {
            version,
            upgrading_to,
        } => Err(Error::ResumeMove {
            version,
            upgrading_to,
        }),
    }
}

fn empty(root: &Path) -> io::Result<bool> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    for entry in entries {
        match entry?.file_name().to_str() {
            Some(LOCK | FILE) => (),
            Some(name)
                if name.starts_with(".folder-upgrade.lock.pending-")
                    || name.starts_with(files::DRAFT_PREFIX) =>
            {
                ()
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Serialize publishing the frozen layout with a fresh opener or another mover.
pub fn lock_for_move(home: &Home, wait: Duration) -> io::Result<OwnedLock> {
    OwnedLock::acquire(&home.root().join(LOCK), wait)
}

/// Publish a frozen checkpoint after the move has safely placed its staged data.
/// The caller owns the move's writer lock. It must never use this on a future layout.
pub fn publish(home: &Home) -> Result<(), Error> {
    inspect(home)?;
    let path = home.root().join(FILE);
    let bytes = format!("{}\n", json!({"version": VERSION}));
    files::write_atomic_through(
        &path,
        bytes.as_bytes(),
        home.root().parent().unwrap_or(home.root()),
    )?;
    Ok(())
}
