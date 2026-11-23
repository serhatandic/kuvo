//! Filesystem operations whose contracts depend on the host OS.

#[cfg(unix)]
#[path = "platform/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "platform/windows.rs"]
mod imp;

pub(crate) use imp::*;
