//! What macOS does for your settings that GPUI doesn't: the Dock's count, the alert sound, which
//! editors are on this Mac, and this app's identifier.

use std::path::PathBuf;

use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::runtime::Bool;
use objc2_app_kit::{NSApplication, NSWorkspace};
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNNotificationSound, UNUserNotificationCenter};

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

/// Asked once a run, as the first notification goes (macOS asks you only the first time).
static ASKED: AtomicBool = AtomicBool::new(false);

/// Posts a notification: `sound`, macOS's notification sound, which Focus and your choices in
/// System Settings › Notifications silence with the banner. Nothing outside an app bundle (the
/// framework stops a program that isn't one). It may wait on macOS: call it off the UI's thread.
pub fn post(tag: &str, title: &str, body: &str, sound: bool) {
    if bundle_id().is_none() {
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    if !ASKED.swap(true, Ordering::SeqCst) {
        let answered = RcBlock::new(|_: Bool, _: *mut NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &answered);
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    if sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(tag), &content, None);
    center.addNotificationRequest_withCompletionHandler(&request, None);
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
