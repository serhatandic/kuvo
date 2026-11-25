//! Platform facts, process ownership, local IPC, file watching and sandbox launchers.

pub mod ipc;
pub mod platform;
pub mod process;
pub mod sandbox;
pub mod watch;

#[cfg(windows)]
pub mod windows;

pub mod trash;
