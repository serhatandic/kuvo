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

/// The app's own folder for what it keeps as a client, under `home`: `<root>/desktop` on every
/// computer (`~/.kuvo-code/desktop`; a copy of the app's, a demo's or a test's, in its own root).
/// Never in `~/Library/Application Support/<bundle ID>`, which on a Mac is the earlier Kuvo Code's
/// folder (0.7's), only ever read by the move from it. Nothing of a box's is in it.
pub fn folder_of(home: &std::path::Path, root: &str) -> PathBuf {
    home.join(root).join("desktop")
}

/// The link's log, in the app's own folder ([`kuvo_box::link::log`]).
pub const LINK_LOG: &str = "link.log";

/// Your computers, as this app follows them: what it keeps of them (`boxes.json`) in its own
/// folder under `home`, this device's key with `keeper`, `nearest` the box on this computer.
pub fn boxes(home: &std::path::Path, root: &str, nearest: Option<std::sync::Arc<dyn kuvo_box::transport::Transport>>, keeper: std::sync::Arc<dyn kuvo_box::link::Keeper>, dial: std::sync::Arc<dyn kuvo_box::link::Dial>) -> kuvo_box::boxes::Boxes {
    // How the link to your other computers fares (each connection lost, and why), for a report:
    // `link.log` beside them, never on screen.
    kuvo_box::link::log::to_file(folder_of(home, root).join(LINK_LOG));
    kuvo_box::link::log::note(format!("Kuvo {} started", option_env!("KUVO_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))));
    kuvo_box::boxes::Boxes::new(kuvo_box::boxes::Options {
        nearest,
        keeper,
        dial,
        name: kuvo_box::boxes::device_name(),
        // (The bundle's version where it was built into one: scripts/build-desktop-app.sh.)
        app: option_env!("KUVO_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")).to_owned(),
        kept: Some(folder_of(home, root).join("boxes.json")),
        timing: Default::default(),
    })
}
