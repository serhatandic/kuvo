//! Platform facts, process ownership, local IPC, file watching and sandbox launchers.

pub mod disk_usage;
pub mod ipc;
pub mod keychain;
pub mod platform;
pub mod process;
pub mod usage_server;
pub mod sandbox;
pub mod watch;
pub mod watch_live;

#[cfg(windows)]
pub mod windows;

pub mod trash;
pub mod usage_computer;
