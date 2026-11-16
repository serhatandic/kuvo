//! Making sure it's you before what lets something use what's yours from now on (Always Allow, a
//! vault item given from now on, who can use it, seeing what a secret keeps): Touch ID, or this
//! Mac's password, as the old Mac app asked (LocalAuthentication's `deviceOwnerAuthentication`).
//! Every such step goes through [`ConfirmYou`], which tests stand in for. Where the app can ask, it
//! says so to each box with its settings requests (`kuvo_box::client::set_confirms_you`), which
//! offers those steps only then.

use std::sync::Arc;

use gpui::{App, Global};

/// What asks. `reason` completes macOS's "Kuvo is trying to …" ("let Scout use “GitHub” from now
/// on"); true once it's you. It waits for the answer: called off the main thread.
#[derive(Clone)]
pub struct ConfirmYou(pub Arc<dyn Fn(&str) -> bool + Send + Sync>);

impl Global for ConfirmYou {}

impl ConfirmYou {
    /// This computer's own check, where the app has one (macOS); none elsewhere yet.
    pub fn here() -> Option<ConfirmYou> {
        #[cfg(target_os = "macos")]
        {
            Some(ConfirmYou(Arc::new(mac::ask)))
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    /// The app's, where it can ask.
    pub fn of(cx: &App) -> Option<ConfirmYou> {
        cx.try_global::<ConfirmYou>().cloned()
    }
}

/// Sets the app up to make sure it's you where it can, and says so to boxes.
pub fn init(cx: &mut App) {
    if let Some(confirm) = ConfirmYou::here() {
        cx.set_global(confirm);
        kuvo_box::client::set_confirms_you(true);
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSString;

    #[link(name = "LocalAuthentication", kind = "framework")]
    unsafe extern "C" {}

    /// `LAPolicyDeviceOwnerAuthentication`: Touch ID, or the Mac's password.
    const DEVICE_OWNER: isize = 2;

    /// Asks, and waits for the answer (macOS answers on a queue of its own).
    pub fn ask(reason: &str) -> bool {
        let Some(class) = AnyClass::get(c"LAContext") else { return false };
        let context: Option<Retained<AnyObject>> = unsafe { msg_send![class, new] };
        let Some(context) = context else { return false };
        let (tell, heard) = std::sync::mpsc::channel::<bool>();
        let answered = RcBlock::new(move |ok: Bool, _: *mut AnyObject| {
            let _ = tell.send(ok.as_bool());
        });
        let reason = NSString::from_str(reason);
        unsafe {
            let _: () = msg_send![&*context, evaluatePolicy: DEVICE_OWNER, localizedReason: &*reason, reply: &*answered];
        }
        heard.recv().unwrap_or(false)
    }
}
