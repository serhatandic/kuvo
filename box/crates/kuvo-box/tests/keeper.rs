//! Drive the real keeper and resident in scratch installs. A damaged executable
//! stands in for a bad pack. The independent holder stands in for row 4.1.

use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use kuvo_keeper::SELECTOR;
use kuvo_os::process::{ProcessIdentity, ProcessState};
use serde_json::{Value, json};

const INITIAL: &str = "versions/0.8.0-beta.1-123456789abc/bin";
const GOOD: &str = "versions/0.8.0-beta.2-123456789abc/bin";
const BAD: &str = "versions/0.8.0-beta.3-ba9876543210/bin";

fn until<T>(duration: Duration, description: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + duration;
    loop {
        if let Some(result) = check() {
            return result;
        }
        assert!(Instant::now() < until, "{description}");
        thread::sleep(Duration::from_millis(100));
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Computer {
    scratch: tempfile::TempDir,
    home: PathBuf,
    binary: PathBuf,
}

impl Computer {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let home = scratch.path().join("state");
        #[cfg(windows)]
        kuvo_os::windows::private_home(&home, true).unwrap();
        Self::initialize(&home);
        fs::create_dir_all(home.join("run")).unwrap();
        fs::create_dir_all(home.join("box")).unwrap();
        fs::create_dir(scratch.path().join("profile")).unwrap();
        let binary = std::env::var_os("KUVO_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_kuvo-box").into())
            .into();
        Self {
            scratch,
            home,
            binary,
        }
    }

    fn initialize(home: &std::path::Path) {
        // The installer commits an empty home's layout before publishing its first pack.
        kuvo_store::layout::open(&kuvo_store::home::Home::new(home.to_owned()).unwrap()).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        let profile = self.scratch.path().join("profile");
        command
            .env_clear()
            .env("KUVO_HOME", &self.home)
            .env("HOME", &profile)
            .env("USERPROFILE", &profile)
            .env("APPDATA", &profile)
            .env("LOCALAPPDATA", &profile)
            .env("TEMP", &profile)
            .env("TMP", &profile)
            .env("TMPDIR", &profile)
            .stdin(Stdio::null());
        #[cfg(windows)]
        command
            .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
            // The unsigned cross-build can bring its runtime DLLs beside the binary.
            .env("PATH", self.binary.parent().unwrap());
        command
    }

    fn stage(&self, target: &str, version: &str, good: bool) {
        let bin = self.home.join(target);
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            bin.join("../box.json"),
            json!({"kuvo":"box/1","version":version,"launch":"rust"}).to_string(),
        )
        .unwrap();
        let executable = bin.join(format!("kuvo-box{}", std::env::consts::EXE_SUFFIX));
        if good {
            fs::copy(&self.binary, executable).unwrap();
        } else {
            fs::write(executable, "damaged executable").unwrap();
        }
    }

    fn control(&self, options: &[&str]) {
        let finished = self.command().arg("keep").args(options).output().unwrap();
        assert!(
            finished.status.success(),
            "{options:?}: {}",
            String::from_utf8_lossy(&finished.stderr)
        );
    }

    fn keeper(&self) -> ChildGuard {
        ChildGuard(self.command().arg("keep").spawn().unwrap())
    }

    fn wait_for_keeper(&self, keeper: &ChildGuard) {
        until(
            Duration::from_secs(20),
            "keeper claims this scratch home",
            || {
                let pid = fs::read_to_string(self.home.join("run/box.pid")).ok()?;
                (pid.trim().parse::<u32>().ok()? == keeper.0.id()).then_some(())
            },
        );
    }

    fn ready(&self, after: Option<&ProcessIdentity>) -> ProcessIdentity {
        until(
            Duration::from_secs(30),
            "resident answers authenticated kuvo/info",
            || {
                let pid = fs::read_to_string(self.home.join("run/link.pid")).ok()?;
                let identity = ProcessIdentity::read(pid.parse().ok()?).ok()??;
                if after == Some(&identity) {
                    return None;
                }
                let reply = self.command().args(["resident-info", &pid]).output().ok()?;
                if !reply.status.success() {
                    return None;
                }
                let body: Value = serde_json::from_slice(&reply.stdout).ok()?;
                assert_eq!(body["serverBuild"], env!("CARGO_PKG_VERSION"));
                assert_eq!(
                    fs::read(self.home.join("run/link.version")).unwrap(),
                    env!("CARGO_PKG_VERSION").as_bytes()
                );
                Some(identity)
            },
        )
    }

    fn wait_exit(keeper: &mut ChildGuard) {
        until(
            Duration::from_secs(20),
            "keeper exits after stopping the resident",
            || keeper.0.try_wait().unwrap(),
        );
    }
}

impl Drop for Computer {
    fn drop(&mut self) {
        if thread::panicking()
            && let Ok(log) = fs::read_to_string(self.home.join("run/serve.log"))
        {
            eprintln!("scratch resident log:\n{log}");
        }
        // The keeper guard is dropped first on panic. Off re-adopts any
        // surviving resident from this test's records and shuts it down.
        let _ = self.command().args(["keep", "off"]).status();
    }
}

#[test]
fn a_keeper_restart_recovers_the_resident_and_pause_ends_the_start() {
    let computer = Computer::new();
    computer.stage(INITIAL, "0.8.0-beta.1", true);
    computer.control(&["select", INITIAL]);
    let mut keeper = computer.keeper();
    computer.wait_for_keeper(&keeper);
    let resident = computer.ready(None);

    let successor = ChildGuard(
        computer
            .command()
            .args(["keep", "handover"])
            .spawn()
            .unwrap(),
    );
    Computer::wait_exit(&mut keeper);
    computer.wait_for_keeper(&successor);
    assert_eq!(
        computer.ready(None),
        resident,
        "planned handover leaves the resident running"
    );
    let mut keeper = successor;
    keeper.0.kill().unwrap();
    Computer::wait_exit(&mut keeper);
    #[cfg(unix)]
    assert_eq!(
        resident.state(),
        ProcessState::Live,
        "keeper exit leaves the Unix resident"
    );
    #[cfg(windows)]
    until(
        Duration::from_secs(10),
        "keeper's job handle closure stops its resident",
        || (resident.state() == ProcessState::Stale).then_some(()),
    );

    let mut keeper = computer.keeper();
    computer.wait_for_keeper(&keeper);
    #[cfg(unix)]
    assert_eq!(
        computer.ready(None),
        resident,
        "the restarted keeper adopts the same resident"
    );
    #[cfg(windows)]
    let resident = computer.ready(Some(&resident));

    #[cfg(unix)]
    {
        let refused = computer.command().args(["keep", "pause"]).output().unwrap();
        assert!(!refused.status.success());
        assert!(!computer.home.join("run/paused").exists());
        fs::write(computer.home.join("box/kind"), "mac\n").unwrap();
    }
    computer.control(&["pause"]);
    Computer::wait_exit(&mut keeper);
    assert_eq!(resident.state(), ProcessState::Stale);
    assert!(computer.home.join("run/paused").is_file());
    let mut resumed = computer.keeper();
    computer.wait_for_keeper(&resumed);
    let again = computer.ready(Some(&resident));
    assert!(!computer.home.join("run/paused").exists());
    fs::write(computer.home.join("run/link.needed"), "").unwrap();
    computer.control(&["off"]);
    Computer::wait_exit(&mut resumed);
    assert_eq!(again.state(), ProcessState::Stale);
    assert!(!computer.home.join("run/link.needed").exists());
    assert!(computer.home.join("run/off").is_file());
}

#[test]
fn a_ready_release_stays_selected_and_a_damaged_release_rolls_code_back() {
    let computer = Computer::new();
    computer.stage(INITIAL, "0.8.0-beta.1", true);
    computer.stage(GOOD, "0.8.0-beta.2", true);
    computer.stage(BAD, "0.8.0-beta.3", false);
    // Crash before replacing the selector: the intent matches the current release.
    computer.control(&["select", INITIAL]);
    fs::write(computer.home.join("run/previous"), INITIAL).unwrap();
    let mut keeper = computer.keeper();
    let first = computer.ready(None);
    assert!(!computer.home.join("run/previous").exists());
    let selector = computer.home.join(SELECTOR);
    let mut reader = File::open(&selector).unwrap();
    computer.control(&["select", GOOD]);
    let mut old_contents = String::new();
    reader.read_to_string(&mut old_contents).unwrap();
    assert_eq!(old_contents, INITIAL);
    assert_eq!(fs::read_to_string(&selector).unwrap(), GOOD);
    let second = computer.ready(Some(&first));
    until(Duration::from_secs(10), "healthy trial commits", || {
        (!computer.home.join("run/previous").exists()).then_some(())
    });
    eprintln!("good release confirmed");

    computer.control(&["select", BAD]);
    let racing = computer
        .command()
        .args(["keep", "select", INITIAL])
        .output()
        .unwrap();
    assert!(
        !racing.status.success(),
        "a second switch cannot displace a pending trial"
    );
    until(
        Duration::from_secs(25),
        "trial stops the old resident",
        || (second.state() == ProcessState::Stale).then_some(()),
    );
    let notes = computer.home.join("data/notes");
    fs::create_dir_all(notes.parent().unwrap()).unwrap();
    fs::write(&notes, "written during trial\n").unwrap();
    eprintln!("bad release selected, resident stopped and a new write made");
    until(Duration::from_secs(135), "bad release rolls back", || {
        (fs::read_to_string(&selector).ok()?.as_str() == GOOD).then_some(())
    });
    assert_eq!(computer.ready(Some(&second)).state(), ProcessState::Live);
    assert_eq!(
        fs::read_to_string(&notes).unwrap(),
        "written during trial\n"
    );
    let refusal = fs::read_to_string(computer.home.join("run/update-cannot")).unwrap();
    let parts: Vec<_> = refusal.split_whitespace().collect();
    assert!(parts[0].parse::<u64>().unwrap() > 0);
    assert_eq!(&parts[1..], ["0.8.0-beta.3", "ba9876543210"]);
    reader.seek(SeekFrom::Start(0)).unwrap();
    old_contents.clear();
    reader.read_to_string(&mut old_contents).unwrap();
    assert_eq!(old_contents, INITIAL);
    computer.control(&["off"]);
    Computer::wait_exit(&mut keeper);
}

#[cfg(unix)]
#[test]
fn the_shell_keeper_confirms_a_rust_switch_and_rolls_back_without_ending_a_holder() {
    use std::os::unix::{fs, process::CommandExt};

    let mut computer = Computer::new();
    computer.home = computer.scratch.path().join("profile/.kuvo-code");
    Computer::initialize(&computer.home);
    std::fs::create_dir_all(computer.home.join("run")).unwrap();
    for (target, version) in [
        (INITIAL, "0.8.0-beta.1"),
        (GOOD, "0.8.0-beta.2"),
        (BAD, "0.8.0-beta.3"),
    ] {
        computer.stage(target, version, target != BAD);
        let bin = computer.home.join(target);
        std::fs::create_dir(bin.join("opencode-plugins")).unwrap();
        std::fs::write(
            bin.join("kuvo-start"),
            include_str!("../../../src/keeper/kuvo-start").replace("@ENGINE@", "2.2.6"),
        )
        .unwrap();
        let launch = bin.join("kuvo-setsid");
        std::fs::write(&launch, include_str!("../../../src/keeper/kuvo-setsid")).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(launch, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(bin.join("kuvo-agents.js"), "// old bridge").unwrap();
        std::fs::write(bin.join("kuvo-link.js"), "// old link").unwrap();
    }
    let engine = computer.home.join("engine/2.2.6/opencode");
    std::fs::create_dir_all(engine.parent().unwrap()).unwrap();
    std::fs::write(
        &engine,
        "#!/bin/sh\nprintf called > \"$KUVO_HOME/run/split-worker\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(engine, std::fs::Permissions::from_mode(0o700)).unwrap();
    for file in ["link.needed", "serve.needed", "engine.now"] {
        std::fs::write(computer.home.join("run").join(file), "").unwrap();
    }
    fs::symlink(INITIAL, computer.home.join("bin")).unwrap();

    let command = |verb: &str| {
        let mut command = Command::new("/bin/sh");
        command
            .env_clear()
            .env("HOME", computer.scratch.path().join("profile"))
            .env("PATH", "/usr/bin:/bin")
            .env("KUVO_TEST_RELAY", "ws://127.0.0.1:9")
            .env("KUVO_TEST_KEEP_ROUND", "1")
            .stdin(Stdio::null())
            .arg(computer.home.join("bin/kuvo-start"))
            .arg(verb);
        command
    };
    struct ShellStop(Command);
    impl Drop for ShellStop {
        fn drop(&mut self) {
            let _ = self.0.status();
        }
    }
    let mut keeper = ChildGuard(command("keep").process_group(0).spawn().unwrap());
    let _off = ShellStop(command("off"));
    let first = computer.ready(None);

    // A real, detached stand-in for an agent's holder, with its live turn on stdin.
    let holder_file = computer.home.join("run/holder-reply");
    let holder_script = computer.home.join("run/kuvo-hold-1234.js");
    std::fs::write(&holder_script, "/bin/cat\n").unwrap();
    let mut holder = ChildGuard(
        Command::new("/bin/sh")
            .arg(&holder_script)
            .args(["hold", "deadbeef", "fixture"])
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(File::create(&holder_file).unwrap())
            .spawn()
            .unwrap(),
    );
    let holders = computer.home.join("run/agents/hold");
    std::fs::create_dir_all(&holders).unwrap();
    std::fs::write(holders.join("deadbeef.json"), format!(
        "{{\"proto\":\"hold/1\",\"holder\":\"fixture\",\"id\":\"deadbeef\",\"kind\":\"codex\",\"pid\":{},\"fixture\":true}}",
        holder.0.id()
    )).unwrap();
    let mut turn = holder.0.stdin.take().unwrap();
    use std::io::Write as _;
    turn.write_all(b"turn before switch\n").unwrap();
    until(Duration::from_secs(5), "holder received the turn", || {
        (std::fs::read_to_string(&holder_file).ok()? == "turn before switch\n").then_some(())
    });

    std::fs::write(computer.home.join("run/previous"), INITIAL).unwrap();
    let switched = command("stop")
        .args(["--switch", GOOD, "keep"])
        .output()
        .unwrap();
    assert!(
        switched.status.success(),
        "{}",
        String::from_utf8_lossy(&switched.stderr)
    );
    let second = computer.ready(Some(&first));
    until(
        Duration::from_secs(10),
        "shell start guard confirms the Rust generation",
        || (!computer.home.join("run/previous").exists()).then_some(()),
    );
    assert_eq!(
        std::fs::read_link(computer.home.join("bin")).unwrap(),
        PathBuf::from(GOOD)
    );
    assert_eq!(keeper.0.try_wait().unwrap(), None);
    assert!(!computer.home.join("run/split-worker").exists());
    assert!(!computer.home.join("run/agents.pid").exists());

    turn.write_all(b"turn after switch\n").unwrap();
    until(
        Duration::from_secs(5),
        "holder answered after switch",
        || {
            (std::fs::read_to_string(&holder_file).ok()?
                == "turn before switch\nturn after switch\n")
                .then_some(())
        },
    );
    eprintln!("shell keeper confirmed the Rust switch, holder still answering");

    std::fs::write(computer.home.join("run/previous"), GOOD).unwrap();
    let switched = command("stop")
        .args(["--switch", BAD, "keep"])
        .output()
        .unwrap();
    assert!(
        switched.status.success(),
        "{}",
        String::from_utf8_lossy(&switched.stderr)
    );
    let notes = computer.home.join("data/notes");
    std::fs::write(&notes, "from the shell trial").unwrap();
    until(
        Duration::from_secs(140),
        "shell guard rolls back the damaged Rust release",
        || {
            (std::fs::read_link(computer.home.join("bin")).ok()? == PathBuf::from(GOOD))
                .then_some(())
        },
    );
    let restored = computer.ready(Some(&second));
    assert_eq!(
        std::fs::read_to_string(notes).unwrap(),
        "from the shell trial"
    );
    turn.write_all(b"turn after rollback\n").unwrap();
    drop(turn);
    assert!(holder.0.wait().unwrap().success());
    assert_eq!(
        std::fs::read_to_string(holder_file).unwrap(),
        "turn before switch\nturn after switch\nturn after rollback\n"
    );
    assert!(command("off").status().unwrap().success());
    Computer::wait_exit(&mut keeper);
    assert_eq!(restored.state(), ProcessState::Stale);
}

#[cfg(unix)]
#[test]
fn a_legacy_manifest_starts_the_split_link_when_the_marker_asks_for_it() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};

    let computer = Computer::new();
    let home = computer.scratch.path().join("profile/.kuvo-code");
    let run = home.join("run");
    let bin = home.join("bin");
    let engine = home.join("engine/2.2.6");
    for folder in [&run, &bin, &engine] {
        fs::create_dir_all(folder).unwrap();
    }
    let keeper = bin.join("kuvo-start");
    fs::write(
        &keeper,
        include_str!("../../../src/keeper/kuvo-start").replace("@ENGINE@", "2.2.6"),
    )
    .unwrap();
    fs::write(
        home.join("box.json"),
        r#"{"kuvo":"box/1","files":{"launch":"rust"}}"#,
    )
    .unwrap();
    fs::write(
        bin.join("kuvo-link.js"),
        "// Kuvo folder layout 23 123456789abc\n",
    )
    .unwrap();
    fs::write(home.join("folder-version.json"), "{\"version\":23}\n").unwrap();
    fs::write(run.join("link.json"), "{}").unwrap();
    let say = engine.join("opencode");
    fs::write(&say, "#!/bin/sh\nfor last do :; done\nif [ \"$last\" = run ]; then printf '%s' \"$*\" > \"$KUVO_HOME/run/link-started\"; fi\n").unwrap();
    let setsid = bin.join("kuvo-setsid");
    fs::write(&setsid, include_str!("../../../src/keeper/kuvo-setsid")).unwrap();
    for executable in [say, setsid] {
        fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let command = |verb| {
        let mut command = Command::new("/bin/sh");
        command
            .env_clear()
            .env("HOME", computer.scratch.path().join("profile"))
            .env("PATH", "/usr/bin:/bin")
            .env("KUVO_TEST_KEEP_ROUND", "1")
            .stdin(Stdio::null())
            .arg(&keeper)
            .arg(verb);
        command
    };
    let mut keeper = ChildGuard(command("keep").process_group(0).spawn().unwrap());
    until(
        Duration::from_secs(10),
        "legacy keeper claims the home",
        || {
            (fs::read_to_string(run.join("box.pid")).ok()?.trim() == keeper.0.id().to_string())
                .then_some(())
        },
    );
    assert!(!run.join("link-started").exists());
    fs::write(run.join("link.needed"), "").unwrap();
    let invocation = until(
        Duration::from_secs(10),
        "marker starts the legacy split link",
        || {
            fs::read_to_string(run.join("link-started"))
                .ok()
                .filter(|s| !s.is_empty())
        },
    );
    assert_eq!(
        invocation,
        format!(
            "--config=/dev/null --no-env-file {}/kuvo-link.js run",
            bin.display()
        )
    );
    assert!(command("off").status().unwrap().success());
    Computer::wait_exit(&mut keeper);
}

#[test]
fn a_keeper_finishes_a_rollback_that_was_decided_before_it_restarted() {
    let computer = Computer::new();
    computer.stage(INITIAL, "0.8.0-beta.1", true);
    computer.stage(GOOD, "0.8.0-beta.2", true);
    computer.control(&["select", INITIAL]);
    computer.control(&["select", GOOD]);
    // Flags left by a crash after deciding the rollback, before sealing the selector.
    fs::write(computer.home.join("run/keeper-rollback"), GOOD).unwrap();
    fs::write(computer.home.join("run/update-engine-hold.json"), "{}").unwrap();
    let mut keeper = computer.keeper();
    computer.ready(None);
    assert_eq!(
        fs::read_to_string(computer.home.join(SELECTOR)).unwrap(),
        INITIAL
    );
    for file in ["keeper-rollback", "previous", "update-engine-hold.json"] {
        assert!(!computer.home.join("run").join(file).exists(), "{file}");
    }
    computer.control(&["off"]);
    Computer::wait_exit(&mut keeper);
}

#[test]
fn a_future_layout_never_opens_a_resident_or_changes_the_layout_marker() {
    let computer = Computer::new();
    computer.stage(INITIAL, "0.8.0-beta.1", true);
    computer.control(&["select", INITIAL]);
    let marker = computer.home.join("folder-version.json");
    let future = b"{\"version\":2147483647}\n";
    fs::write(&marker, future).unwrap();
    let mut keeper = computer.keeper();
    computer.wait_for_keeper(&keeper);
    until(
        Duration::from_secs(10),
        "resident reports a future layout",
        || {
            let log = fs::read_to_string(computer.home.join("run/serve.log")).ok()?;
            log.contains("2147483647").then_some(())
        },
    );
    assert!(!computer.home.join("run/link.pid").exists());
    assert!(!computer.home.join("run/serve.json").exists());
    assert_eq!(fs::read(marker).unwrap(), future);
    computer.control(&["off"]);
    Computer::wait_exit(&mut keeper);
}
