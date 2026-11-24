//! Drive Windows enrollment in a private scratch home with no packaged starter.

#![cfg(windows)]

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;
use std::{
    fs,
    io::Write as _,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn enrollment_checks_the_home_layout_before_requiring_any_startup_files() {
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path().join("home");
    kuvo_os::windows::private_home(&home, true).unwrap();
    let marker = b"{\"version\":22}\n";
    fs::write(home.join("folder-version.json"), marker).unwrap();
    let expiry = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    let code = json!({
        "setup": format!("set_{}", URL_SAFE_NO_PAD.encode([1; 16])),
        "expires": expiry,
        "relay": "ws://127.0.0.1:1",
        "address": URL_SAFE_NO_PAD.encode([2; 16]),
        "primary": URL_SAFE_NO_PAD.encode([3; 32]),
        "token": URL_SAFE_NO_PAD.encode([4; 24]),
    });
    let binary =
        std::env::var_os("KUVO_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_kuvo-box").into());
    let mut process = Command::new(binary)
        .args(["install", "--join"])
        .env_clear()
        .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
        .env("KUVO_HOME", &home)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        process.stdin.take().unwrap(),
        "kuvo-add-1.{}",
        URL_SAFE_NO_PAD.encode(code.to_string())
    )
    .unwrap();
    let output = process.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("Kuvo folder layout 22 requires the move to layout 23"),
        "{error}"
    );
    assert_eq!(fs::read(home.join("folder-version.json")).unwrap(), marker);
    assert_eq!(fs::read_dir(&home).unwrap().count(), 1);
}
