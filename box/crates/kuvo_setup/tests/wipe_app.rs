#![cfg(target_os = "macos")]

use std::{cell::Cell, fs, io, os::unix::fs::MetadataExt, path::Path, path::PathBuf};

use kuvo_setup::wipe::{AppWipeKeychain, AppWipeKind, resume_app_wipe};
use kuvo_store::files;
use serde_json::{Value, json};

const SETUP: &str = "set_aaaaaaaaaaaaaaaaaaaaaa";
const OPERATION: &str = "bbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    account: tempfile::TempDir,
    app: PathBuf,
    receipts: PathBuf,
    detached: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let account = tempfile::tempdir().unwrap();
        let parent = account
            .path()
            .canonicalize()
            .unwrap()
            .join("Library/Application Support");
        let app = parent.join("run.kuvo.code.wipe-test");
        files::write_secret(&app.join("box/run/work"), b"keep before Keychain cleanup").unwrap();
        let receipts = parent.join(".run.kuvo.code.wipe-test-removal-receipts");
        let detached = receipts.join(format!("erase-{OPERATION}"));
        Self {
            account,
            app,
            receipts,
            detached,
        }
    }

    fn write_phase(&self, name: &str, phase: &str, source: &Path) {
        let info = source.metadata().unwrap();
        let id = format!("{}:{}", info.dev(), info.ino());
        files::write_secret(&self.receipts.join(name), &serde_json::to_vec(&json!({
            "setup": SETUP, "operation": OPERATION, "source": id, "phase": phase, "formatVersion": 1,
        })).unwrap()).unwrap();
    }

    fn receipt(&self, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.receipts.join(name)).unwrap()).unwrap()
    }
}

struct Keychain<'a> {
    fixture: &'a Fixture,
    calls: Cell<u32>,
}

impl AppWipeKeychain for Keychain<'_> {
    fn clear_app_link_keys(&self, app: &Path) -> io::Result<()> {
        assert_eq!(app, self.fixture.app);
        assert!(!app.exists());
        assert_eq!(self.fixture.receipt("app-member-wipe")["phase"], "detached");
        assert_eq!(
            fs::read(self.fixture.detached.join("box/run/work"))?,
            b"keep before Keychain cleanup"
        );
        let prior = self.calls.replace(self.calls.get() + 1);
        if prior == 0 {
            return Err(io::Error::other("keychain stopped"));
        }
        Ok(())
    }
}

#[test]
fn a_member_wipe_retries_keychain_cleanup_after_detach_before_erasing_data() {
    let fixture = Fixture::new();
    let keychain = Keychain {
        fixture: &fixture,
        calls: Cell::new(0),
    };
    let resume = || {
        resume_app_wipe(
            &fixture.app,
            fixture.account.path(),
            AppWipeKind::Member,
            &keychain,
        )
    };
    fixture.write_phase("app-member-wipe", "stopping", &fixture.app);
    assert!(!resume().unwrap());
    assert!(fixture.app.exists());
    assert_eq!(keychain.calls.get(), 0);

    fixture.write_phase("app-member-wipe", "erasing", &fixture.app);
    assert_eq!(resume().unwrap_err().to_string(), "keychain stopped");
    assert!(!fixture.app.exists());
    assert!(fixture.detached.join("box/run/work").exists());

    fs::create_dir(&fixture.app).unwrap();
    assert!(resume().is_err());
    assert_eq!(keychain.calls.get(), 1);
    fs::remove_dir(&fixture.app).unwrap();

    assert!(resume().unwrap());
    assert_eq!(keychain.calls.get(), 2);
    assert!(!fixture.detached.exists());
    assert!(resume().unwrap());
    assert_eq!(keychain.calls.get(), 2);
    assert_eq!(
        fixture.receipt("app-member-wipe"),
        json!({
            "setup": SETUP, "operation": OPERATION, "formatVersion": 1,
        })
    );
    assert!(!fixture.receipts.join("last-setup-wipe").exists());
}

#[test]
fn an_approved_last_app_wipe_resumes_with_keychain_cleanup_already_complete() {
    let fixture = Fixture::new();
    let keychain = Keychain {
        fixture: &fixture,
        calls: Cell::new(0),
    };
    fixture.write_phase("last-setup-wipe", "erasing", &fixture.app);
    assert!(
        resume_app_wipe(
            &fixture.app,
            fixture.account.path(),
            AppWipeKind::LastComputer,
            &keychain
        )
        .unwrap()
    );
    assert!(!fixture.app.exists());
    assert!(!fixture.detached.exists());
    assert_eq!(keychain.calls.get(), 0);
}
