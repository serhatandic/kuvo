//! Run stopped uninstall against the real Windows Task Scheduler.

#![cfg(windows)]

use kuvo_store::{home::Home, layout};
use std::{fs, process::Command};

#[test]
fn a_stopped_home_without_a_sign_in_task_can_be_uninstalled() {
    let scratch = tempfile::tempdir().unwrap();
    let home = Home::new(scratch.path().join("home")).unwrap();
    kuvo_os::windows::private_home(home.root(), true).unwrap();
    layout::open(&home).unwrap();
    fs::create_dir(home.run()).unwrap();

    let binary =
        std::env::var_os("KUVO_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_kuvo-box").into());
    for _ in 0..2 {
        let result = Command::new(&binary)
            .arg("uninstall")
            .env_clear()
            .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
            .env("KUVO_HOME", home.root())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let answer: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(answer["result"], "uninstalled");
        assert_eq!(answer["stopped"], true);
        assert!(home.run().join("off").is_file());
    }
}
