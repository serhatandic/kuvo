//! OS-level sandbox launch. Policy construction and the box's CLI dispatch live with their callers.

use crate::{
    platform::{Executable, Platform},
    process::Spawn,
};
use std::{io, path::Path};

pub fn supported_platform() -> bool {
    matches!(Platform::current(), Platform::Mac | Platform::Linux)
}

/// Wrap an existing absolute executable with the rendered Seatbelt policy.
pub fn seatbelt(mut command: Spawn, profile: &Path) -> io::Result<Spawn> {
    if Platform::current() != Platform::Mac {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Seatbelt is only available on macOS",
        ));
    }
    if !profile.is_absolute() || !command.executable.path().is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Seatbelt needs absolute profile and executable paths",
        ));
    }
    command.args.splice(
        ..0,
        [
            "-f".into(),
            profile.as_os_str().to_owned(),
            command.executable.path().as_os_str().to_owned(),
        ],
    );
    command.executable = Executable::Native("/usr/bin/sandbox-exec".into());
    Ok(command)
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::*;
