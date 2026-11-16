//! What the tile beside System Settings needs from macOS itself, which GPUI doesn't give: where
//! System Settings' window is, and moving, showing, hiding and never focusing the tile's own
//! window. macOS only; reads no window's contents or title (frames are given without Screen
//! Recording).

use std::ffi::c_void;

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::CGRect;
use core_graphics::window::{copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID};
use gpui::{Bounds, Pixels, Point, Window, point, px, size};
use objc2::encode::{Encode, Encoding};
use objc2::msg_send;
use objc2::runtime::AnyObject;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// System Settings' program, by where macOS keeps it.
const SETTINGS: &str = "/System Settings.app/Contents/MacOS/System Settings";

/// Whether process `pid` is System Settings.
fn is_settings(pid: i64) -> bool {
    let Ok(pid) = libc::c_int::try_from(pid) else { return false };
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the kernel writes at most `path.len()` bytes into `path`, and says how many.
    let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast::<c_void>(), path.len() as u32) };
    length > 0 && String::from_utf8_lossy(&path[..length as usize]).ends_with(SETTINGS)
}

/// System Settings' window on screen, if it has one: its frame as Quartz gives it, from the top
/// left of the main display.
pub fn settings_window() -> Option<Bounds<Pixels>> {
    let windows = copy_window_info(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID)?;
    for item in windows.get_all_values() {
        // SAFETY: each item of the window list is a dictionary, kept alive by the list.
        let window: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(item as CFDictionaryRef) };
        // SAFETY: the keys are constants of CoreGraphics' own.
        let key = |key: CFStringRef| unsafe { CFString::wrap_under_get_rule(key) };
        let number = |named: CFStringRef| window.find(key(named)).and_then(|value| value.downcast::<CFNumber>()).and_then(|number| number.to_i64());
        // SAFETY: as above.
        if number(unsafe { kCGWindowLayer }) != Some(0) || !number(unsafe { kCGWindowOwnerPID }).is_some_and(is_settings) {
            continue;
        }
        // SAFETY: as above.
        let Some(frame) = window.find(key(unsafe { kCGWindowBounds })).and_then(|value| value.downcast::<CFDictionary>()).and_then(|bounds| CGRect::from_dict_representation(&bounds)) else { continue };
        if frame.size.width > 200. {
            return Some(Bounds::new(point(px(frame.origin.x as f32), px(frame.origin.y as f32)), size(px(frame.size.width as f32), px(frame.size.height as f32))));
        }
    }
    None
}

/// A point as AppKit takes it.
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

// SAFETY: two doubles, laid out and named as CoreGraphics' `CGPoint`.
unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}

/// The NSWindow (here the tile's NSPanel) GPUI drew `window` in, where it has one.
fn native(window: &Window) -> Option<*mut AnyObject> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return None };
    let view = handle.ns_view.as_ptr().cast::<AnyObject>();
    // SAFETY: `view` is the window's live NSView, on the main thread (GPUI's windows only exist
    // there).
    let native: *mut AnyObject = unsafe { msg_send![view, window] };
    (!native.is_null()).then_some(native)
}

/// Moves `window` so its top left is at `to` (from the top left of the main display, as
/// [`settings_window`] gives frames), `main_height` being that display's height: AppKit counts
/// from its bottom.
pub fn move_window(window: &Window, to: Point<Pixels>, main_height: Pixels) {
    let Some(native) = native(window) else { return };
    let top_left = CGPoint { x: f64::from(f32::from(to.x)), y: f64::from(f32::from(main_height) - f32::from(to.y)) };
    // SAFETY: a live NSWindow on the main thread, which takes a point.
    unsafe {
        let _: () = msg_send![native, setFrameTopLeftPoint: top_left];
    }
}

/// Keeps the tile's panel from becoming the key window when it's clicked or dragged from: the
/// keyboard stays with System Settings (or Kuvo), and AppKit's Touch Bar, which watches the key
/// window's views, isn't set watching the tile's. (It was, on a Mac with a Touch Bar: when the
/// tile went, AppKit later stopped watching views already gone, and the app was stopped with
/// "removeObserver … not registered".) GPUI's windows all say they can become key; an NSPanel
/// that `becomesKeyOnlyIfNeeded` only does on a click for a view that needs the keyboard, which
/// GPUI's doesn't ask for. AppKit may still make it key on its own (with every other window of
/// Kuvo's minimized, say): hiding it before it's let go is what covers that (`close_tile`). The
/// mouse still reaches it: the icon is dragged and the buttons pressed as before (with the plain
/// arrow over it: GPUI sets the pointer's shape only for the key window).
pub fn never_key(window: &Window) {
    let Some(native) = native(window) else { return };
    // SAFETY: a live NSWindow on the main thread; only an NSPanel is sent the panel's setter.
    unsafe {
        let panel: bool = msg_send![native, isKindOfClass: objc2::class!(NSPanel)];
        if panel {
            let _: () = msg_send![native, setBecomesKeyOnlyIfNeeded: true];
        }
    }
}

/// Shows or hides `window` where it is, without making it key or bringing Kuvo forward.
pub fn set_shown(window: &Window, shown: bool) {
    let Some(native) = native(window) else { return };
    let nothing = std::ptr::null_mut::<AnyObject>();
    // SAFETY: a live NSWindow on the main thread.
    unsafe {
        if shown {
            let _: () = msg_send![native, orderFront: nothing];
        } else {
            let _: () = msg_send![native, orderOut: nothing];
        }
    }
}
