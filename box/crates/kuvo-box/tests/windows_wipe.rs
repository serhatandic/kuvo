//! The last Windows computer wipes the installation its program runs from: a copy outside
//! it erases it, and the reinstall at the same path deletes the copy. A scratch profile stands in for
//! the account's own.

#![cfg(windows)]

use std::{fs, path::Path, process::Stdio, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, Command},
};

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn prepare(home: &Path) {
    kuvo_os::windows::private_home(home, true).unwrap();
    kuvo_store::layout::publish(&kuvo_store::home::Home::new(home.to_path_buf()).unwrap()).unwrap();
    fs::create_dir_all(home.join("run")).unwrap();
    fs::write(
        home.join("run/link.json"),
        r#"{"name":"Last computer","phones":[]}"#,
    )
    .unwrap();
}

async fn start(program: &Path, profile: &Path, home: &Path) -> (Child, Value) {
    let mut child = Command::new(program)
        .arg("serve")
        .env_clear()
        .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
        .env("USERPROFILE", profile)
        .env("KUVO_HOME", home)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let size = stdout.read_line(&mut line).await.unwrap();
    assert_ne!(size, 0, "the resident exited during startup");
    (child, read(&home.join("run/serve.json")))
}

async fn request(record: &Value, method: &str, path: &str, body: Value) -> (u16, Value) {
    let port = record["port"].as_u64().unwrap() as u16;
    let auth = STANDARD.encode(format!("opencode:{}", record["password"].as_str().unwrap()));
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let body = body.to_string();
    let request = format!(
        "{method} /{path} HTTP/1.0\r\nHost: localhost\r\nAuthorization: Basic {auth}\r\nx-kuvo-route-version: 1\r\nx-kuvo-from: mac\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await.unwrap();
    let reply = String::from_utf8(reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_last_computer_erases_itself_from_outside_and_the_reinstall_deletes_the_eraser() {
    let scratch = tempfile::tempdir().unwrap();
    let profile = scratch.path().canonicalize().unwrap();
    let profile = kuvo_os::platform::windows_conventional_path(&profile).unwrap();
    let home = profile.join(".kuvo-code");
    let outside = profile.join("untouched.txt");
    fs::write(&outside, "outside the installation").unwrap();
    prepare(&home);
    fs::create_dir_all(home.join("data")).unwrap();
    fs::write(home.join("data/fixture.txt"), "erase this work").unwrap();

    // The installed box runs from the folder it erases.
    let built = std::path::PathBuf::from(
        std::env::var_os("KUVO_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_kuvo-box").into()),
    );
    fs::create_dir_all(home.join("bin")).unwrap();
    for entry in fs::read_dir(built.parent().unwrap()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "dll") {
            fs::copy(&path, home.join("bin").join(path.file_name().unwrap())).unwrap();
        }
    }
    let installed = home.join("bin/kuvo-box.exe");
    fs::copy(&built, &installed).unwrap();

    let (mut resident, record) = start(&installed, &profile, &home).await;
    let (status, before) = request(&record, "GET", "kuvo/setup", Value::Null).await;
    assert_eq!(status, 200, "{before}");
    let (setup, key) = (
        before["setup"].clone(),
        before["you"]["key"].as_str().unwrap().to_owned(),
    );
    let operation = STANDARD
        .encode([90; 16])
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "");
    let route = format!("kuvo/computers/{key}/unlink");
    let (status, answer) = request(&record, "POST", &route, json!({"operation": operation})).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["removal"]["state"], "wipe-pending");
    assert_eq!(answer["offer"], "reinstall-local");

    let ended = tokio::time::timeout(Duration::from_secs(30), resident.wait()).await;
    assert!(ended.expect("the resident kept running").unwrap().success());
    // A copy of the program outside the folder erases it once the resident has exited.
    let receipts = profile.join(".kuvo-code-removal-receipts");
    let marker = receipts.join("last-setup-wipe");
    let erased = json!({"setup": setup, "operation": operation, "formatVersion": 1});
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !(marker.exists() && read(&marker) == erased) {
        assert!(
            std::time::Instant::now() < deadline,
            "the old installation was not erased"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!home.exists());
    assert!(!receipts.join(format!("erase-{operation}")).exists());
    assert!(kuvo_os::windows::is_private(&receipts).unwrap());
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "outside the installation"
    );
    let eraser = fs::read_dir(&receipts)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .find(|name| name.starts_with("eraser-"))
        .expect("the eraser's copy");
    let pid: u32 = eraser["eraser-".len()..].parse().unwrap();
    while kuvo_os::process::ProcessIdentity::read(pid)
        .unwrap()
        .is_some()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the eraser kept running"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The reinstall at the same path starts fresh and deletes the eraser's copy.
    prepare(&home);
    let (_next, record) = start(&built, &profile, &home).await;
    let (status, after) = request(&record, "GET", "kuvo/setup", Value::Null).await;
    assert_eq!(status, 200, "{after}");
    assert_ne!(after["you"]["key"].as_str().unwrap(), key);
    assert!(!receipts.join(eraser).exists());
    assert_eq!(read(&marker), erased);
}

#[tokio::test]
async fn a_wipe_waits_until_the_sign_in_task_is_removed() {
    let scratch = tempfile::tempdir().unwrap();
    let profile = scratch.path().canonicalize().unwrap();
    let profile = kuvo_os::platform::windows_conventional_path(&profile).unwrap();
    let home = profile.join(".kuvo-code");
    prepare(&home);
    // Only the task's registration matters here: its program never runs in the test.
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::write(home.join("bin").join(kuvo_keeper::windows::STARTER), "").unwrap();
    let task = kuvo_keeper::windows::StartupTask::for_home(&home).unwrap();
    assert!(task.register().unwrap());
    let built = std::path::PathBuf::from(
        std::env::var_os("KUVO_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_kuvo-box").into()),
    );
    let (_resident, record) = start(&built, &profile, &home).await;
    let (_, before) = request(&record, "GET", "kuvo/setup", Value::Null).await;
    let route = format!(
        "kuvo/computers/{}/unlink",
        before["you"]["key"].as_str().unwrap()
    );
    let operation = "W".repeat(22);
    let answer = request(&record, "POST", &route, json!({"operation": operation})).await;
    task.remove().unwrap();
    assert!(!task.registered().unwrap());
    assert_eq!(answer.0, 409, "{}", answer.1);
    assert!(home.join("run/link.json").exists());
}
