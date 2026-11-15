//! Where this app keeps its device key (what your computers know it by, and its relay secret): the
//! system's keychain on a Mac, the app's alone; elsewhere a file only you can read in Kuvo Code's
//! own folder, until there's a keyring to use. Never logged, never in the kept list of computers.

use std::path::PathBuf;

use kuvo_box::link::{Keeper, Kept};

/// The Mac's keychain: one generic password of the app's own.
#[cfg(target_os = "macos")]
pub struct Keychain {
    service: String,
}

#[cfg(target_os = "macos")]
const ACCOUNT: &str = "link-identity";

#[cfg(target_os = "macos")]
impl Keychain {
    /// `service`: the item's service name (the app's own; a test's is another).
    pub fn new(service: impl Into<String>) -> Keychain {
        Keychain { service: service.into() }
    }
}

#[cfg(target_os = "macos")]
impl Keeper for Keychain {
    fn read(&self) -> Kept {
        /// `errSecItemNotFound`: nothing's kept. Anything else (locked, refused) isn't "nothing".
        const NOT_FOUND: i32 = -25300;
        match security_framework::passwords::get_generic_password(&self.service, ACCOUNT) {
            Ok(bytes) => String::from_utf8(bytes).map_or(Kept::Unreadable, Kept::Found),
            Err(error) if error.code() == NOT_FOUND => Kept::Nothing,
            Err(_) => Kept::Unreadable,
        }
    }

    fn write(&self, kept: &str) -> bool {
        security_framework::passwords::set_generic_password(&self.service, ACCOUNT, kept.as_bytes()).is_ok()
    }

    fn forget(&self) {
        let _ = security_framework::passwords::delete_generic_password(&self.service, ACCOUNT);
    }
}

/// A file only you can read (`0600`), in a folder only you can open.
pub struct PrivateFile {
    path: PathBuf,
}

impl PrivateFile {
    pub fn new(path: impl Into<PathBuf>) -> PrivateFile {
        PrivateFile { path: path.into() }
    }
}

impl Keeper for PrivateFile {
    fn read(&self) -> Kept {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Kept::Found(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Kept::Nothing,
            Err(_) => Kept::Unreadable,
        }
    }

    fn write(&self, kept: &str) -> bool {
        kuvo_box::boxes::write_private(&self.path, kept.as_bytes()).is_ok()
    }

    fn forget(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The app's own folder for what it keeps as a client, under `home`: beside its box's on a Mac,
/// in `~/.kuvo-code` elsewhere. Nothing of a box's is in it.
pub fn folder(home: &std::path::Path) -> PathBuf {
    folder_of(home, kuvo_box::install::REAL_ROOT, kuvo_box::install::REAL_APP)
}

/// The same for a copy of the app (a demo's, a test's): under its own bundle ID on a Mac, in its
/// own root elsewhere, never the real app's.
pub fn folder_of(home: &std::path::Path, root: &str, app_id: &str) -> PathBuf {
    if cfg!(target_os = "macos") { home.join("Library/Application Support").join(app_id).join("desktop") } else { home.join(root).join("desktop") }
}
