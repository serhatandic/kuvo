//! The box's home, on-disk encodings and durable storage primitives.

pub mod background;
pub mod declarations;
pub mod diff;
pub mod files;
pub mod format;
pub mod home;
pub mod layout;
pub mod locks;
pub mod release_version;
pub mod sqlite;

mod platform;
