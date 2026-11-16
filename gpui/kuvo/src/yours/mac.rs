//! What macOS does for your settings that GPUI doesn't: the Dock's count, the alert sound, which
//! editors are on this Mac, and this app's identifier.

use std::path::PathBuf;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSBeep, NSWorkspace};
use objc2_foundation::{NSBundle, NSString};

/// The count on the Dock icon; none at 0. Only from the main thread (where the app's views run);
/// elsewhere it's left as it is.
pub fn set_dock_count(count: usize) {
    let Some(main) = MainThreadMarker::new() else { return };
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    NSApplication::sharedApplication(main).dockTile().setBadgeLabel(label.as_deref());
}

/// Whether Kuvo is the active app (only known on the main thread; elsewhere taken as not).
pub fn app_active() -> bool {
    MainThreadMarker::new().is_some_and(|main| NSApplication::sharedApplication(main).isActive())
}

pub fn play_alert() {
    NSBeep();
}

/// Where the app with this bundle identifier is, where it's on this Mac.
pub fn app_with_id(id: &str) -> Option<PathBuf> {
    let url = NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(id))?;
    url.path().map(|path| PathBuf::from(path.to_string()))
}

/// This app's bundle identifier (none for a build run outside a bundle).
pub fn bundle_id() -> Option<String> {
    NSBundle::mainBundle().bundleIdentifier().map(|id| id.to_string())
}
