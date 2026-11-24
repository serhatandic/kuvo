// Real Rust controller and holder, with this test binary standing in for a line-based agent.
// Each controller is a separate process, so killing one tests OS process ownership as well as replay.
use kuvo_agents::hold::{Controller, Directory, HoldKind, Limits, Output, Replay, Start};
use kuvo_os::platform::Environment;
use kuvo_store::home::Home;
use serde_json::json;
use std::{
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

const ASK: &str = r#"{"id":17,"method":"fixture/approve"}"#;
const REPLY: &str = r#"{"id":17,"result":{"allow":true}}"#;

#[test]
fn replacing_a_crashed_controller_keeps_the_live_turn_and_agent_pid() {
    let root = scratch();
    #[cfg(windows)]
    kuvo_store::files::write_atomic(
        &root.path().join("run/socket.key"),
        "ab".repeat(32).as_bytes(),
    )
    .unwrap();
    let old_release = root.path().join("old");
    let new_release = root.path().join("new");
    fs::create_dir_all(&old_release).unwrap();
    fs::create_dir_all(&new_release).unwrap();
    let binary = env::var_os("KUVO_HOLDER_TEST_BOX").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_kuvo-box")),
        PathBuf::from,
    );
    let binary = binary.canonicalize().unwrap();
    let old_holder = old_release.join(binary.file_name().unwrap());
    fs::hard_link(&binary, &old_holder)
        .or_else(|_| fs::copy(&binary, &old_holder).map(drop))
        .unwrap();
    let driver = env::current_exe().unwrap();
    let new_driver = new_release.join(driver.file_name().unwrap());
    fs::hard_link(&driver, &new_driver)
        .or_else(|_| fs::copy(&driver, &new_driver).map(drop))
        .unwrap();

    let mut original = probe(
        &driver,
        root.path(),
        "controller_probe",
        "original",
        &old_holder,
    );
    let mut original_output = BufReader::new(original.0.stdout.take().unwrap());
    let before = report(&mut original_output);
    assert_eq!(before["generation"], 1);
    assert!(before["child"].as_u64().is_some_and(|pid| pid > 0));
    original.0.kill().unwrap();
    assert!(!original.0.wait().unwrap().success());
    // A replacement from a different release directory discovers the original
    // Rust holder without access to the original controller's in-memory state.
    let mut successor = probe(
        &new_driver,
        root.path(),
        "controller_probe",
        "successor",
        &old_holder,
    );
    let mut successor_output = BufReader::new(successor.0.stdout.take().unwrap());
    let after = report(&mut successor_output);
    assert_eq!(after["generation"], 2);
    assert_eq!(after["child"], before["child"]);
    assert_eq!(after["holder"], before["holder"]);
    assert!(successor.0.wait().unwrap().success());
}

fn scratch() -> tempfile::TempDir {
    #[cfg(unix)]
    return tempfile::Builder::new()
        .prefix("kh-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(windows)]
    tempfile::Builder::new().prefix("kh-").tempdir().unwrap()
}

struct Probe(Child);

impl Drop for Probe {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn probe(executable: &Path, root: &Path, name: &str, mode: &str, holder: &Path) -> Probe {
    let mut command = Command::new(executable);
    command
        .env_clear()
        .envs(environment(root))
        .env("HOLDER_TEST_MODE", mode)
        .env("HOLDER_TEST_ROOT", root)
        .env("HOLDER_TEST_BINARY", holder)
        .args(["--exact", name, "--ignored", "--nocapture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    Probe(command.spawn().unwrap())
}

fn environment(root: &Path) -> Environment {
    let mut environment: Environment = env::vars_os()
        .filter(|(name, _)| {
            ["SystemRoot", "WINDIR"]
                .iter()
                .any(|key| name.eq_ignore_ascii_case(key))
        })
        .collect();
    for name in ["HOME", "USERPROFILE", "TMPDIR", "TEMP", "TMP"] {
        environment.insert(name.into(), root.as_os_str().into());
    }
    environment
}

fn report(reader: &mut impl BufRead) -> serde_json::Value {
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "Controller exited before reporting"
        );
        if let Some((_, json)) = line.split_once("HOLDER-REPORT ") {
            return serde_json::from_str(json).unwrap();
        }
    }
}

#[test]
#[ignore = "controller process launched by the scenario"]
fn controller_probe() {
    let root = PathBuf::from(env::var_os("HOLDER_TEST_ROOT").unwrap());
    let binary = PathBuf::from(env::var_os("HOLDER_TEST_BINARY").unwrap());
    let mode = env::var("HOLDER_TEST_MODE").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), drive(root, binary, mode))
            .await
            .unwrap();
    });
}

async fn drive(root: PathBuf, binary: PathBuf, mode: String) {
    let directory = Directory::for_home(&Home::new(root.clone()).unwrap());
    if mode == "original" {
        let mut active = directory
            .start(
                &binary,
                Start {
                    command: vec![
                        env::current_exe().unwrap(),
                        "--exact".into(),
                        "agent_probe".into(),
                        "--ignored".into(),
                        "--nocapture".into(),
                    ],
                    cwd: Some(root.clone()),
                    env: environment(&root),
                    kind: HoldKind::JsonRpc,
                    meta: json!({"kind":"codex","provider":"codex-cli","turns":{
                        "thread":{"session":"ses_held","from":0},
                    }})
                    .as_object()
                    .unwrap()
                    .clone(),
                    portable_requests: None,
                    limits: Limits {
                        orphan: Some(Duration::from_secs(8)),
                        linger: Some(Duration::from_secs(3)),
                        ..Limits::default()
                    },
                },
            )
            .await
            .unwrap();
        active.input.write("begin").await.unwrap();
        while let Output::Line { sequence, line, .. } = active.output.next().await.unwrap() {
            if line == ASK {
                active.input.acknowledge(sequence).await.unwrap();
                break;
            }
        }
        active.input.prepare(0).await.unwrap();
        assert!(matches!(
            active.output.next().await.unwrap(),
            Output::Prepared { prepared: true }
        ));
        announce(&active);
        std::future::pending::<()>().await;
    } else {
        let records = directory.records().await.unwrap();
        let [record] = records.as_slice() else {
            panic!("Expected one live Rust holder")
        };
        let turns = record.turns().unwrap();
        assert_eq!(turns["thread"].session, "ses_held");
        let mut active = directory
            .adopt(record, Replay::Complete(turns["thread"].from))
            .await
            .unwrap();
        let mut before = 0;
        loop {
            match active.output.next().await.unwrap() {
                Output::Line { line, .. } if line == "before" => before += 1,
                Output::Line { line, again, .. } if line == ASK => {
                    assert!(!again);
                    break;
                }
                Output::Line { .. } => {}
                frame => panic!("Unexpected holder output: {frame:?}"),
            }
        }
        assert_eq!(before, 1);
        active.input.unpin().await.unwrap();
        active.input.write(REPLY).await.unwrap();
        assert!(
            matches!(active.output.next().await.unwrap(), Output::Line { line, .. } if line == "after")
        );
        active.input.end().await.unwrap();
        loop {
            match active.output.next().await.unwrap() {
                Output::Exited { exit } => {
                    assert_eq!(exit.code, Some(0));
                    break;
                }
                Output::Line { .. } => {}
                frame => panic!("Unexpected holder output: {frame:?}"),
            }
        }
        active.input.bye().await.unwrap();
        assert_eq!(
            active.output.next().await.unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        announce(&active);
    }
}

fn announce(controller: &Controller) {
    println!(
        "HOLDER-REPORT {}",
        json!({
            "holder": controller.welcome.pid,
            "child": controller.welcome.child,
            "generation": controller.welcome.generation,
        })
    );
}

#[test]
#[ignore = "agent process launched by the scenario"]
fn agent_probe() {
    let lines = std::io::stdin().lock().lines();
    for (index, line) in lines.enumerate() {
        match index {
            0 => {
                assert_eq!(line.unwrap(), "begin");
                println!("\nbefore");
                println!("{ASK}");
            }
            1 => {
                assert_eq!(line.unwrap(), REPLY);
                println!("after");
            }
            _ => panic!("Unexpected agent input"),
        }
    }
}

#[test]
fn scans_only_live_rust_holders_and_leaves_typescript_records_alone() {
    let root = scratch();
    let folder = root.path().join("run/agents/hold");
    fs::create_dir_all(&folder).unwrap();
    let pid = std::process::id();
    let mut stale = json!({
        "proto":"hold/1","holder":"kuvo-box/1","id":"aaaaaaaaaaaa","kind":"raw",
        "pid":pid,"child":null,"program":"fixture","cwd":null,"started":1,"bridge":null,
        "generation":1,"exited":null,"meta":{},"key":"fixture-only-key",
        "process":{"pid":pid,"startIdentity":"1"},
    });
    let stale_record = folder.join("aaaaaaaaaaaa.json");
    let stale_journal = folder.join("aaaaaaaaaaaa.journal");
    fs::write(&stale_record, stale.to_string()).unwrap();
    fs::write(&stale_journal, "left by the old holder\n").unwrap();

    stale["id"] = json!("bbbbbbbbbbbb");
    stale["holder"] = json!("00112233aabbccdd");
    stale.as_object_mut().unwrap().remove("process");
    let legacy_record = folder.join("bbbbbbbbbbbb.json");
    fs::write(&legacy_record, stale.to_string()).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let directory = Directory::for_home(&Home::new(root.path().into()).unwrap());
    assert!(runtime.block_on(directory.records()).unwrap().is_empty());
    assert_eq!(stale_record.exists(), cfg!(windows));
    assert_eq!(stale_journal.exists(), cfg!(windows));
    assert_eq!(
        fs::read_to_string(legacy_record).unwrap(),
        stale.to_string()
    );
}
