//! macOS's Reduce Motion (System Settings › Accessibility › Display), followed into GPUI's
//! `App::set_reduce_motion` as it's turned on and off, so what moves (the bots, the spinners)
//! shows its still form while it's on. GPUI doesn't read it by itself.

use gpui::App;

/// Follows the system's setting from now on (on a Mac; elsewhere, motion stays on).
pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    mac::follow(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use gpui::App;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use objc2_foundation::NSNotification;

    fn reduced() -> bool {
        NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }

    pub fn follow(cx: &mut App) {
        cx.set_reduce_motion(reduced());
        // The system says the display's options changed; the setting is read again then.
        let (changed, heard) = async_channel::unbounded::<()>();
        let told = RcBlock::new(move |_: NonNull<NSNotification>| {
            let _ = changed.try_send(());
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the name is AppKit's own constant; no object or queue is named, so the block runs
        // where the system posts it, and only hands the news on.
        let observer = unsafe { center.addObserverForName_object_queue_usingBlock(Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification), None, None, &told) };
        // (Kept for as long as the app runs.)
        std::mem::forget(observer);
        cx.spawn(async move |cx| {
            while heard.recv().await.is_ok() {
                let reduce = reduced();
                cx.update(|cx| cx.set_reduce_motion(reduce));
            }
        })
        .detach();
    }
}
