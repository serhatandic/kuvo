//! Sparkle, the updater 0.7 had, driven from Rust. The bundle carries it in
//! `Contents/Frameworks/Sparkle.framework` and names its feed and key in Info.plist
//! (`scripts/build-desktop-app.sh`); it's loaded as the app opens, never linked, so a build run
//! outside a bundle (`cargo run`, the tests) has none and says it doesn't update itself.
//!
//! Sparkle's own windows are the update prompt (what's new, the download, Install and Relaunch),
//! as in 0.7. Its settings (automatic checks and downloads, when it last looked) are kept in the
//! app's defaults, where 0.7 kept them under the same bundle ID; so is "Get beta updates"
//! ([`super::BETAS_KEY`]), which this delegate turns into the feed's channels.

use std::ffi::c_void;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSBundle, NSDate, NSError, NSSet, NSString, NSUserDefaults};

use super::{BETAS_KEY, Can, Facts, NOT_THIS_BUILD, Updater, version, wants_betas};

/// What Sparkle says changes, watched so the page and the menu follow it.
const WATCHED: &[&str] = &["canCheckForUpdates", "sessionInProgress", "lastUpdateCheckDate", "automaticallyChecksForUpdates", "automaticallyDownloadsUpdates"];

pub struct Ivars {
    tell: async_channel::Sender<()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Delegate` doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KuvoUpdaterDelegate"]
    #[ivars = Ivars]
    /// Sparkle's delegate (`SPUUpdaterDelegate`, asked by `respondsToSelector:`), and the
    /// watcher of its updater's state.
    pub struct Delegate;

    impl Delegate {
        /// The feed's channels this copy reads: "beta" for those who get betas.
        #[unsafe(method_id(allowedChannelsForUpdater:))]
        fn allowed_channels(&self, _updater: &AnyObject) -> Retained<NSSet<NSString>> {
            let names: Vec<Retained<NSString>> = super::channels(gets_betas()).iter().map(|name| NSString::from_str(name)).collect();
            NSSet::from_retained_slice(&names)
        }

        #[unsafe(method(updater:didFindValidUpdate:))]
        fn found(&self, _updater: &AnyObject, item: &AnyObject) {
            say(&format!("{} is available", shown_version(item)));
        }

        #[unsafe(method(updater:didDownloadUpdate:))]
        fn downloaded(&self, _updater: &AnyObject, item: &AnyObject) {
            say(&format!("{} is downloaded", shown_version(item)));
        }

        /// Downloaded and checked on its own: Sparkle installs it as the app quits (left to it).
        #[unsafe(method(updater:willInstallUpdateOnQuit:immediateInstallationBlock:))]
        fn installs_on_quit(&self, _updater: &AnyObject, item: &AnyObject, _install: *mut AnyObject) -> Bool {
            say(&format!("{} is ready, and installs when Kuvo quits", shown_version(item)));
            self.tell();
            Bool::NO
        }

        #[unsafe(method(updater:willInstallUpdate:))]
        fn installing(&self, _updater: &AnyObject, item: &AnyObject) {
            say(&format!("installing {}", shown_version(item)));
        }

        #[unsafe(method(updater:didAbortWithError:))]
        fn aborted(&self, _updater: &AnyObject, error: &NSError) {
            // (Finding none is said this way too: SUNoUpdateError.)
            if error.code() == 1001 {
                say("up to date");
            } else {
                say(&format!("stopped: {}", error.localizedDescription()));
            }
        }

        #[unsafe(method(updater:didFinishUpdateCycleForUpdateCheck:error:))]
        fn finished(&self, _updater: &AnyObject, _check: isize, _error: Option<&NSError>) {
            self.tell();
        }

        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observed(&self, _key: Option<&NSString>, _object: Option<&AnyObject>, _change: Option<&AnyObject>, _context: *mut c_void) {
            self.tell();
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, tell: async_channel::Sender<()>) -> Retained<Delegate> {
        let this = Delegate::alloc(mtm).set_ivars(Ivars { tell });
        unsafe { msg_send![super(this), init] }
    }

    fn tell(&self) {
        self.ivars().tell.try_send(()).ok();
    }
}

/// What the app says of its updates, for a log (stderr: Console for an app, a file for a test).
fn say(what: &str) {
    eprintln!("kuvo: updates: {what}");
}

fn shown_version(item: &AnyObject) -> String {
    let shown: Option<Retained<NSString>> = unsafe { msg_send![item, displayVersionString] };
    shown.map_or_else(|| "an update".to_owned(), |shown| format!("Kuvo {shown}"))
}

/// "Get beta updates", as chosen (in 0.7 or here), else as this version goes.
fn gets_betas() -> bool {
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(BETAS_KEY);
    let chosen = defaults.objectForKey(&key).is_some().then(|| defaults.boolForKey(&key));
    wants_betas(chosen, version())
}

/// Sparkle, started: its controller (the standard update windows), its updater, and the delegate
/// they hold weakly, kept here for the app's life.
pub struct Sparkle {
    controller: Retained<AnyObject>,
    updater: Retained<AnyObject>,
    delegate: Retained<Delegate>,
}

impl Sparkle {
    /// Loads the bundle's Sparkle and starts it, telling `tell` whenever its state changes. Why
    /// it can't where it can't, in a sentence for the Updates page.
    pub fn start(tell: async_channel::Sender<()>) -> Result<Sparkle, String> {
        let mtm = MainThreadMarker::new().ok_or_else(|| NOT_THIS_BUILD.to_owned())?;
        let main = NSBundle::mainBundle();
        // Only a release's bundle names a feed (as 0.7's did).
        if main.objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL")).is_none() {
            return Err(NOT_THIS_BUILD.into());
        }
        let missing = "Kuvo’s updater is missing from the app. Download Kuvo again to get updates.";
        let frameworks = main.privateFrameworksPath().ok_or(missing)?;
        let framework = NSBundle::bundleWithPath(&NSString::from_str(&format!("{frameworks}/Sparkle.framework"))).ok_or(missing)?;
        if let Err(error) = unsafe { framework.loadAndReturnError() } {
            // (Why, in dyld's words: a signature it won't take, a file that isn't there.)
            let why: Retained<NSString> = unsafe { msg_send![&error, debugDescription] };
            say(&format!("Sparkle didn't load: {why}"));
            return Err(missing.into());
        }
        let class = AnyClass::get(c"SPUStandardUpdaterController").ok_or(missing)?;
        let delegate = Delegate::new(mtm, tell);
        let controller: Option<Retained<AnyObject>> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![allocated, initWithStartingUpdater: false, updaterDelegate: &*delegate, userDriverDelegate: std::ptr::null_mut::<AnyObject>()]
        };
        let controller = controller.ok_or(missing)?;
        let updater: Option<Retained<AnyObject>> = unsafe { msg_send![&controller, updater] };
        let updater = updater.ok_or(missing)?;
        for key in WATCHED {
            unsafe {
                let _: () = msg_send![&updater, addObserver: &*delegate, forKeyPath: &*NSString::from_str(key), options: 0usize, context: std::ptr::null_mut::<c_void>()];
            }
        }
        // (A feed or key it can't use is said in Sparkle's own window, as it starts.)
        unsafe {
            let _: () = msg_send![&controller, startUpdater];
        }
        Ok(Sparkle { controller, updater, delegate })
    }
}

impl Updater for Sparkle {
    fn facts(&self) -> Facts {
        let (checks, downloads, allows, can_check, busy): (bool, bool, bool, bool, bool) = unsafe {
            (
                msg_send![&self.updater, automaticallyChecksForUpdates],
                msg_send![&self.updater, automaticallyDownloadsUpdates],
                msg_send![&self.updater, allowsAutomaticUpdates],
                msg_send![&self.updater, canCheckForUpdates],
                msg_send![&self.updater, sessionInProgress],
            )
        };
        let checked: Option<Retained<NSDate>> = unsafe { msg_send![&self.updater, lastUpdateCheckDate] };
        Facts {
            version: version().to_owned(),
            can: Can::Update,
            can_check,
            busy,
            checks,
            downloads: allows.then_some(downloads),
            betas: gets_betas(),
            checked: checked.map(|date| date.timeIntervalSince1970() * 1000.0),
        }
    }

    fn check(&self) {
        unsafe {
            let _: () = msg_send![&self.controller, checkForUpdates: std::ptr::null_mut::<AnyObject>()];
        }
    }

    fn set_checks(&self, on: bool) {
        unsafe {
            let _: () = msg_send![&self.updater, setAutomaticallyChecksForUpdates: on];
        }
    }

    fn set_downloads(&self, on: bool) {
        unsafe {
            let _: () = msg_send![&self.updater, setAutomaticallyDownloadsUpdates: on];
        }
    }

    fn set_betas(&self, on: bool) {
        let before = gets_betas();
        NSUserDefaults::standardUserDefaults().setBool_forKey(on, &NSString::from_str(BETAS_KEY));
        // On: the newest beta now, not at the next daily check.
        if on && !before {
            unsafe {
                let _: () = msg_send![&self.updater, checkForUpdatesInBackground];
            }
        }
        self.delegate.tell();
    }
}

impl Drop for Sparkle {
    fn drop(&mut self) {
        for key in WATCHED {
            unsafe {
                let _: () = msg_send![&self.updater, removeObserver: &*self.delegate, forKeyPath: &*NSString::from_str(key)];
            }
        }
    }
}
