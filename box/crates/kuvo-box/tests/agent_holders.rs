// Real Rust controller and holder, with this test binary standing in for a line-based agent.
// Each controller is a separate process, so killing one tests OS process ownership as well as replay.
use kuvo_agents::{
    accounts::{Account, AgentKind},
    hold::{Controller, Directory, HoldKind, Limits, Output, Record, Start, TurnNote},
    roster::{Agent, Factory},
    runtime::{AdapterCall, Adoption, Runtime},
};
use kuvo_os::platform::Environment;
use kuvo_store::home::Home;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
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
    let runtime = Runtime::for_home(&Home::new(root.clone()).unwrap());
    if mode == "original" {
        let mut active = runtime
            .holders()
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
        let factories = HashMap::from([(
            AgentKind::Codex,
            Arc::new(FixtureFactory) as Arc<dyn Factory>,
        )]);
        let (roster, failures) = runtime.activate(&factories).await.unwrap();
        assert_eq!(roster.all().len(), 1);
        assert!(failures.is_empty(), "{failures:?}");
    }
}

struct FixtureFactory;

impl Factory for FixtureFactory {
    fn create(&self, account: &Account, home: Option<PathBuf>) -> std::io::Result<Arc<dyn Agent>> {
        assert_eq!(account.id, "default");
        assert!(home.is_none());
        Ok(Arc::new(FixtureAdapter))
    }
}

struct FixtureAdapter;

impl Agent for FixtureAdapter {
    fn probe_account(
        &self,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<kuvo_agents::account_check::AccountStatus, kuvo_agents::account_check::CheckFailure>,
    > {
        Box::pin(async { Err(kuvo_agents::account_check::CheckFailure::TimedOut) })
    }

    fn models(&self, _: bool) -> AdapterCall<'_, kuvo_agents::catalog::Models> {
        Box::pin(async { Ok(kuvo_agents::catalog::Models::default()) })
    }

    fn shutdown(&self) -> AdapterCall<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl Adoption for FixtureAdapter {
    fn accepts(&self, _: &Record, turns: &BTreeMap<String, TurnNote>) -> bool {
        turns.contains_key("thread")
    }

    fn attach<'a>(
        &'a self,
        account: &'a Account,
        _: Record,
        turns: BTreeMap<String, TurnNote>,
        mut active: Controller,
    ) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            assert_eq!(account.provider().to_string(), "codex-cli");
            assert_eq!(turns["thread"].session, "ses_held");
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
            Ok(())
        })
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

#[cfg(feature = "agent-runtime-fixture")]
#[test]
fn keeper_release_switch_adopts_the_residents_running_holder() {
    keeper_switch_holds_turn(false);
}

#[cfg(feature = "agent-runtime-fixture")]
#[test]
fn keeper_rollback_adopts_the_residents_running_holder() {
    keeper_switch_holds_turn(true);
}

#[cfg(feature = "agent-runtime-fixture")]
fn keeper_switch_holds_turn(resume_rollback: bool) {
    use kuvo_os::process::{ProcessIdentity, ProcessState};

    const OLD: &str = "versions/0.8.0-beta.1-123456789abc/bin";
    const NEW: &str = "versions/0.8.0-beta.2-123456789abc/bin";
    let scratch = scratch();
    let root = scratch.path().join("state");
    #[cfg(windows)]
    kuvo_os::windows::private_home(&root, true).unwrap();
    kuvo_store::layout::open(&Home::new(root.clone()).unwrap()).unwrap();
    let binary = env::var_os("KUVO_HOLDER_TEST_BOX")
        .map_or_else(
            || PathBuf::from(env!("CARGO_BIN_EXE_kuvo-box")),
            PathBuf::from,
        )
        .canonicalize()
        .unwrap();
    for (release, version) in [(OLD, "0.8.0-beta.1"), (NEW, "0.8.0-beta.2")] {
        let bin = root.join(release);
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            bin.join("../box.json"),
            json!({"kuvo":"box/1","version":version,"launch":"rust"}).to_string(),
        )
        .unwrap();
        let executable = bin.join(binary.file_name().unwrap());
        // The test harness exits without serving when the keeper launches it.
        let program = if resume_rollback && release == NEW {
            env::current_exe().unwrap()
        } else {
            binary.clone()
        };
        fs::hard_link(&program, &executable)
            .or_else(|_| fs::copy(&program, &executable).map(drop))
            .unwrap();
        #[cfg(windows)]
        {
            let unwind = binary.with_file_name("libunwind.dll");
            if unwind.is_file() {
                fs::copy(unwind, bin.join("libunwind.dll")).unwrap();
            }
        }
    }
    fs::create_dir_all(root.join("run")).unwrap();
    fs::write(
        root.join("run/agent-fixture-program"),
        env::current_exe().unwrap().to_str().unwrap(),
    )
    .unwrap();
    let session = root.join("agents/sessions/ses_keeper_fixture");
    fs::create_dir_all(&session).unwrap();
    fs::write(
        session.join("session.json"),
        json!({
            "id":"ses_keeper_fixture", "projectID":"fixture", "agent":"build",
            "location":{"directory":root}, "time":{"created":1,"updated":1},
            "model":{"providerID":"codex-cli","id":"stand-in"},
        })
        .to_string(),
    )
    .unwrap();
    let profile = scratch.path().join("profile");
    fs::create_dir(&profile).unwrap();

    let keeper = KeeperFixture {
        binary,
        root,
        profile,
    };
    keeper.control(&["select", OLD]);
    let _worker = Probe(keeper.command().arg("keep").spawn().unwrap());
    let first = fixture_until("the first release to publish its ready resident", || {
        let id = fs::read_to_string(keeper.root.join("run/link.pid")).ok()?;
        let identity = ProcessIdentity::read(id.trim().parse().ok()?).ok()??;
        keeper.ready(&id).then_some(identity)
    });

    fs::write(keeper.root.join("run/agent-fixture-begin"), b"").unwrap();
    let started = fixture_until("the first resident to start a held turn", || {
        fixture_checkpoint(&keeper.root, "agent-fixture-started")
    });
    assert_eq!(started["resident"], first.pid);
    assert_eq!(started["generation"], 1);

    keeper.control(&["select", NEW]);
    if resume_rollback {
        fixture_until(
            "the failed release to run after the first resident exits",
            || {
                let log = fs::read_to_string(keeper.root.join("run/serve.log")).ok()?;
                (first.state() == ProcessState::Stale
                    && (log.contains("running 0 tests") || log.contains("Unrecognized option:")))
                .then_some(())
            },
        );
        assert_eq!(
            fs::read_to_string(keeper.root.join("run/previous"))
                .unwrap()
                .trim(),
            OLD
        );
        // Replay the durable decision left when rollback was interrupted.
        kuvo_store::files::write_secret(&keeper.root.join("run/keeper-rollback"), NEW.as_bytes())
            .unwrap();
    }
    let finished = fixture_until("the replacement resident to finish the held turn", || {
        fixture_checkpoint(&keeper.root, "agent-fixture-finished")
    });
    let second = finished["resident"].as_u64().unwrap() as u32;
    assert_ne!(second, first.pid);
    assert_eq!(finished["agent"], started["agent"]);
    assert_eq!(finished["holder"], started["holder"]);
    assert_eq!(finished["generation"], 2);
    let db =
        kuvo_store::sqlite::open_index(&session.join("messages-v1.sqlite"), Duration::from_secs(1))
            .unwrap();
    let assistant: String = db
        .query_row(
            "SELECT body FROM message WHERE id = 'msg_keeper_fixture_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let assistant: serde_json::Value = serde_json::from_str(&assistant).unwrap();
    assert_eq!(assistant["content"][0]["text"], "before after");
    let completed: String = db
        .query_row(
            "SELECT body FROM message WHERE id = 'msg_keeper_fixture_2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let completed: serde_json::Value = serde_json::from_str(&completed).unwrap();
    assert_eq!(completed["outcome"], "succeeded");
    drop(db);
    let expected = if resume_rollback { OLD } else { NEW };
    fixture_until(
        "the replacement resident to answer and the keeper to settle its trial",
        || {
            (keeper.ready(&second.to_string())
                && !keeper.root.join("run/previous").exists()
                && fs::read_to_string(keeper.root.join(kuvo_keeper::SELECTOR))
                    .ok()?
                    .as_str()
                    == expected)
                .then_some(())
        },
    );
    if resume_rollback {
        let refusal = fs::read_to_string(keeper.root.join("run/update-cannot")).unwrap();
        assert_eq!(
            refusal.split_whitespace().skip(1).collect::<Vec<_>>(),
            ["0.8.0-beta.2", "123456789abc"]
        );
        assert!(!keeper.root.join("run/keeper-rollback").exists());
    }
    keeper.control(&["off"]);
}

#[cfg(feature = "agent-runtime-fixture")]
struct KeeperFixture {
    binary: PathBuf,
    root: PathBuf,
    profile: PathBuf,
}

#[cfg(feature = "agent-runtime-fixture")]
impl KeeperFixture {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .env_clear()
            .envs(environment(&self.profile))
            .env("KUVO_HOME", &self.root)
            .stdin(Stdio::null());
        command
    }

    fn control(&self, args: &[&str]) {
        let result = self.command().arg("keep").args(args).output().unwrap();
        assert!(
            result.status.success(),
            "keeper {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn ready(&self, pid: &str) -> bool {
        self.command()
            .args(["resident-info", pid.trim()])
            .output()
            .is_ok_and(|result| result.status.success())
    }
}

#[cfg(feature = "agent-runtime-fixture")]
impl Drop for KeeperFixture {
    fn drop(&mut self) {
        let _ = self.command().args(["keep", "off"]).output();
    }
}

#[cfg(feature = "agent-runtime-fixture")]
fn fixture_until<T>(what: &str, mut read: impl FnMut() -> Option<T>) -> T {
    let until = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = read() {
            return value;
        }
        assert!(
            std::time::Instant::now() < until,
            "Timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(feature = "agent-runtime-fixture")]
fn fixture_checkpoint(root: &Path, name: &str) -> Option<serde_json::Value> {
    serde_json::from_slice(&fs::read(root.join("run").join(name)).ok()?).ok()
}

#[test]
#[ignore = "agent process launched by the scenario"]
fn rpc_agent_probe() {
    let mut lines = std::io::stdin().lock().lines();
    let first: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(first["jsonrpc"], "2.0");
    assert_eq!(first["method"], "initialize");
    println!(r#"{{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{{"tool":"edit"}}}}"#);
    println!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"step":1}}}}"#);
    let answer: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(answer["id"], "p1");
    println!(
        "{}",
        json!({"jsonrpc":"2.0","id":first["id"],"result":{"saw":answer["result"]["outcome"]}})
    );
}

async fn wait(incoming: &mut tokio::sync::mpsc::Receiver<kuvo_agents::rpc::Incoming>) -> kuvo_agents::rpc::Incoming {
    tokio::time::timeout(Duration::from_secs(20), incoming.recv())
        .await
        .unwrap()
        .unwrap()
}

/// The shared JSON-RPC connection over a real holder: a request, the agent's
/// own request answered in between, a notification, and the agent's exit.
#[test]
fn json_rpc_runs_through_a_real_holder() {
    use kuvo_agents::rpc::{Framing, Incoming, Options, Rpc};

    let root = scratch();
    #[cfg(windows)]
    kuvo_store::files::write_atomic(
        &root.path().join("run/socket.key"),
        "ab".repeat(32).as_bytes(),
    )
    .unwrap();
    let binary = env::var_os("KUVO_HOLDER_TEST_BOX").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_kuvo-box")),
        PathBuf::from,
    );
    let home = root.path().to_owned();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let runtime = Runtime::for_home(&Home::new(home.clone()).unwrap());
            let controller = runtime
                .holders()
                .start(
                    &binary,
                    Start {
                        command: vec![
                            env::current_exe().unwrap(),
                            "--exact".into(),
                            "rpc_agent_probe".into(),
                            "--ignored".into(),
                            "--nocapture".into(),
                            "--quiet".into(),
                        ],
                        cwd: Some(home.clone()),
                        env: environment(&home),
                        kind: HoldKind::JsonRpc,
                        meta: json!({"kind":"grok","provider":"grok-cli","turns":{}})
                            .as_object()
                            .unwrap()
                            .clone(),
                        portable_requests: None,
                        limits: Limits::default(),
                    },
                )
                .await
                .unwrap();
            let (rpc, mut incoming) = Rpc::over(
                controller,
                Options {
                    framing: Framing::JsonRpc2,
                    filter: Some(Box::new(|line: &str| line.starts_with('{'))),
                    ..Options::default()
                },
            );
            let asking = tokio::spawn({
                let rpc = rpc.clone();
                async move { rpc.request("initialize", None, Some(Duration::from_secs(20))).await }
            });
            let Incoming::Request { id, method, .. } = wait(&mut incoming).await else {
                panic!("the agent's request came first");
            };
            assert_eq!(method, "session/request_permission");
            rpc.respond(id, Ok(json!({"outcome":"allow"}))).await;
            assert!(matches!(
                wait(&mut incoming).await,
                Incoming::Notification { method, .. } if method == "session/update"
            ));
            assert_eq!(asking.await.unwrap().unwrap(), json!({"saw":"allow"}));
            assert!(matches!(
                wait(&mut incoming).await,
                Incoming::Exited { code: 0, .. }
            ));
        });
}
