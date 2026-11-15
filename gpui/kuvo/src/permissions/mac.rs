//! The two things the tile beside System Settings needs from macOS itself, which GPUI doesn't
//! give: where System Settings' window is, and moving the tile's own window there. macOS only;
//! reads no window's contents or title (frames are given without Screen Recording).

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
        let number = |named: CFStringRef| window.find(&key(named)).and_then(|value| value.downcast::<CFNumber>()).and_then(|number| number.to_i64());
        // SAFETY: as above.
        if number(unsafe { kCGWindowLayer }) != Some(0) || !number(unsafe { kCGWindowOwnerPID }).is_some_and(is_settings) {
            continue;
        }
        // SAFETY: as above.
        let Some(frame) = window.find(&key(unsafe { kCGWindowBounds })).and_then(|value| value.downcast::<CFDictionary>()).and_then(|bounds| CGRect::from_dict_representation(&bounds)) else { continue };
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

/// Moves `window` so its top left is at `to` (from the top left of the main display, as
/// [`settings_window`] gives frames), `main_height` being that display's height: AppKit counts
/// from its bottom.
pub fn move_window(window: &Window, to: Point<Pixels>, main_height: Pixels) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return };
    let view = handle.ns_view.as_ptr().cast::<AnyObject>();
    let top_left = CGPoint { x: f64::from(f32::from(to.x)), y: f64::from(f32::from(main_height) - f32::from(to.y)) };
    // SAFETY: `view` is the window's live NSView, on the main thread (GPUI's windows only exist
    // there); its window, where it has one, takes a point.
    unsafe {
        let native: *mut AnyObject = msg_send![view, window];
        if !native.is_null() {
            let _: () = msg_send![native, setFrameTopLeftPoint: top_left];
        }
    }
}
