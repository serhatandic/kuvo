//! Public OS adapter scenarios. Children are this test executable or a system shell.

use kuvo_os::{
    ipc::{Channel, Endpoint, windows_pipe_path},
    platform::{
        Account, Environment, Platform, box_home, join_path, windows_extended_path,
        windows_native_path, windows_wire_path,
    },
    process::{ChildGroup, ProcessIdentity, ProcessState, Spawn, Stdio},
    watch::{Backend, Notice, Options, Subscription, WatchHub},
};
use serde_json::Value;
use std::{
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const LIMIT: Duration = Duration::from_secs(10);
const ADDRESS_VECTORS: &str = include_str!("../../../test/fixtures/file-addresses/addresses.json");
const WINDOWS_VECTORS: &str =
    include_str!("../../../../protocol/vectors/file-addresses/windows.json");

fn scratch() -> TempDir {
    tempfile::Builder::new().prefix("rs13-").tempdir().unwrap()
}

fn bounded(condition: impl Fn() -> bool, description: &str) {
    let deadline = Instant::now() + LIMIT;
    while !condition() {
        assert!(Instant::now() < deadline, "{description}");
        std::thread::yield_now();
    }
}

#[test]
fn windows_paths_round_trip_exact_case_and_refuse_ambiguous_names() {
    let vectors: Value = serde_json::from_str(WINDOWS_VECTORS).unwrap();
    for case in vectors["paths"].as_array().unwrap() {
        let wire = case["wire"].as_str().unwrap();
        let native = case["native"].as_str().unwrap();
        assert_eq!(windows_wire_path(native).as_deref(), Some(wire), "{native}");
        assert_eq!(windows_native_path(wire).as_deref(), Some(native), "{wire}");
        if let Some(extended) = case["extended"].as_str() {
            assert_eq!(
                windows_extended_path(wire).as_deref(),
                Some(extended),
                "{wire}"
            );
            assert_eq!(
                kuvo_os::platform::windows_conventional_path(Path::new(extended)).as_deref(),
                Some(Path::new(native)),
            );
        }
    }
    for value in vectors["refused"].as_array().unwrap() {
        let path = value.as_str().unwrap();
        assert_eq!(windows_wire_path(path), None, "{path}");
        assert_eq!(windows_native_path(path), None, "{path}");
    }
}

#[cfg(unix)]
#[test]
fn file_addresses_point_to_the_exact_files_named_by_the_vectors() {
    use kuvo_os::platform::file_uri_path;
    let temp = scratch();
    let vectors: Value = serde_json::from_str(ADDRESS_VECTORS).unwrap();
    for case in vectors["addresses"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let encoded = case["encoded"].as_str().unwrap();
        let inner = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .unwrap();
        fs::write(temp.path().join(name), name).unwrap();
        let decoded = file_uri_path(
            Platform::current(),
            &format!("file://{}/{inner}", temp.path().display()),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(decoded).unwrap(), name);
    }
    assert_eq!(file_uri_path(Platform::current(), "file:///a%2fb"), None);
}

#[cfg(windows)]
#[test]
fn windows_file_uri_addresses_keep_native_paths() {
    use kuvo_os::platform::file_uri_path;
    let temp = scratch();
    let vectors: Value = serde_json::from_str(ADDRESS_VECTORS).unwrap();
    let root = windows_wire_path(temp.path().to_str().unwrap()).unwrap();
    for case in vectors["addresses"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let encoded = case["encoded"].as_str().unwrap();
        let inner = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .unwrap();
        let uri = format!("file:///{root}/{inner}");
        if name.contains('?') {
            assert_eq!(file_uri_path(Platform::Windows, &uri), None);
        } else {
            fs::write(temp.path().join(name), name).unwrap();
            assert_eq!(
                fs::read_to_string(file_uri_path(Platform::Windows, &uri).unwrap()).unwrap(),
                name
            );
        }
    }
    assert_eq!(
        file_uri_path(Platform::Windows, "file://Server/Share/some%20File").unwrap(),
        Path::new(r"\\Server\Share\some File")
    );
}

#[test]
fn profile_and_override_decide_where_the_frozen_home_lives() {
    let account = Account::current().unwrap();
    assert!(account.home.is_absolute());
    let mut env = Environment::new();
    env.insert("HOME".into(), "/shell-home".into());
    env.insert("userprofile".into(), r"d:\Users\Test Profile".into());
    let windows = box_home(&account, Platform::Windows, &env).unwrap();
    #[cfg(windows)]
    assert_eq!(windows, Path::new(r"d:\Users\Test Profile\.kuvo-code"));
    #[cfg(unix)]
    {
        // Use the Windows spelling only for the profile itself on a Unix filesystem.
        assert_eq!(
            windows.parent().unwrap(),
            Path::new(r"d:\Users\Test Profile")
        );
        assert_eq!(
            box_home(&account, Platform::current(), &env).unwrap(),
            Path::new("/shell-home/.kuvo-code")
        );
    }
    let temp = scratch();
    env.insert("KUVO_HOME".into(), temp.path().as_os_str().into());
    assert_eq!(
        box_home(&account, Platform::current(), &env).unwrap(),
        temp.path()
    );
    env.insert("USERPROFILE".into(), r"\\Server\Share\roaming".into());
    assert!(account.home_folder(Platform::Windows, &env).is_err());
}

#[test]
fn platform_facts_distinguish_the_wsl_kernel_and_native_windows() {
    assert_eq!(
        Platform::linux("6.6.87.2-microsoft-standard-WSL2"),
        Platform::Wsl
    );
    assert_eq!(Platform::linux("6.12.105+deb13-amd64"), Platform::Linux);
    assert!(Platform::Windows.is_windows_host());
    assert!(Platform::Wsl.is_windows_host());
    let running = Platform::current();
    #[cfg(target_os = "linux")]
    assert_eq!(
        running,
        Platform::linux(
            fs::read_to_string("/proc/sys/kernel/osrelease")
                .unwrap()
                .trim()
        )
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        (running, running.prompted_folders().contains(&"Documents")),
        (Platform::Mac, true)
    );
    #[cfg(windows)]
    assert_eq!(running, Platform::Windows);
}

#[test]
fn windows_path_merges_absolute_folders_and_preserves_case_and_quoted_semicolons() {
    let first = OsStr::new(r#"D:\Git\cmd;"C:\has;semi";relative;d:/git/cmd;D:\Git\cmd"#);
    let second = OsStr::new(r"relative;C:relative;\\Server\Share\bin;D:/Git/cmd");
    assert_eq!(
        join_path(Platform::Windows, [first, second]),
        r#"D:\Git\cmd;"C:\has;semi";d:\git\cmd;\\Server\Share\bin"#
    );
}

#[cfg(windows)]
#[test]
fn executable_search_uses_pathext_and_git_bash_from_its_own_installation() {
    use kuvo_os::platform::{Executable, git_bash, which};
    let temp = scratch();
    let prefix = temp.path().join("Git");
    let cmd = prefix.join("cmd");
    let bin = prefix.join("bin");
    fs::create_dir_all(&cmd).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(cmd.join("git.exe"), "").unwrap();
    fs::write(bin.join("bash.exe"), "").unwrap();
    fs::write(cmd.join("runner.cmd"), "").unwrap();
    fs::write(cmd.join("runner.exe"), "").unwrap();
    let env = Environment::from([
        ("Path".into(), cmd.as_os_str().to_owned()),
        ("Pathext".into(), ".CMD;.EXE".into()),
    ]);
    let shim = which(OsStr::new("runner"), &env, &[]).unwrap();
    assert!(matches!(shim, Executable::CmdScript(_)));
    assert!(shim.path().ends_with("runner.CMD"));
    assert_eq!(git_bash(&env).unwrap(), bin.join("bash.exe"));
    // Git's own mingw64\bin on PATH names the same installation.
    let mingw = prefix.join("mingw64").join("bin");
    fs::create_dir_all(&mingw).unwrap();
    fs::write(mingw.join("git.exe"), "").unwrap();
    let inner = Environment::from([("PATH".into(), mingw.as_os_str().to_owned())]);
    assert_eq!(git_bash(&inner).unwrap(), bin.join("bash.exe"));
    // The installation Git names for itself comes first.
    let own = temp.path().join("Own Git");
    fs::create_dir_all(own.join("bin")).unwrap();
    fs::write(own.join("bin").join("bash.exe"), "").unwrap();
    let mut named = env.clone();
    named.insert("GIT_INSTALL_ROOT".into(), own.as_os_str().to_owned());
    assert_eq!(git_bash(&named).unwrap(), own.join("bin").join("bash.exe"));
    fs::remove_file(bin.join("bash.exe")).unwrap();
    assert_eq!(
        git_bash(&env).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[cfg(unix)]
#[test]
fn login_shell_environment_filters_box_keys_and_appends_the_original_path() {
    use kuvo_os::platform::{login_environment, which};
    use std::os::unix::fs::PermissionsExt;
    let temp = scratch();
    let shell = temp.path().join("profile-shell");
    fs::write(&shell, "#!/bin/sh\ncase $1 in -ilc) exit 2;; esac\nexport PATH=/profile/tools TERM=from-profile\nprintf 'profile notice'\nexec /bin/sh -c \"$2\"\n").unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    let env = Environment::from([
        ("SHELL".into(), shell.as_os_str().into()),
        ("HOME".into(), temp.path().as_os_str().into()),
        (
            "PATH".into(),
            format!("{}:/profile/tools", temp.path().display()).into(),
        ),
        ("KUVO_SAMPLE_KEY".into(), "keep out of startup".into()),
    ]);
    let found = login_environment(&Account::current().unwrap(), &env, LIMIT, LIMIT)
        .unwrap()
        .unwrap();
    assert_eq!(found[OsStr::new("TERM")], "from-profile");
    assert_eq!(
        found[OsStr::new("PATH")],
        format!("/profile/tools:{}", temp.path().display()).as_str()
    );
    assert!(!found.contains_key(OsStr::new("KUVO_SAMPLE_KEY")));
    assert_eq!(
        which(OsStr::new("profile-shell"), &found, &[])
            .unwrap()
            .path(),
        shell
    );
}

#[cfg(windows)]
#[test]
fn windows_login_environment_uses_the_native_path_and_canonical_variable_names() {
    use kuvo_os::platform::login_environment;
    let env = Environment::from([
        ("Path".into(), r"D:\Tools;anything;D:/Tools".into()),
        ("TeRm".into(), "kuvo".into()),
    ]);
    let found = login_environment(&Account::current().unwrap(), &env, LIMIT, LIMIT)
        .unwrap()
        .unwrap();
    assert_eq!(found[OsStr::new("PATH")], r"D:\Tools");
    assert_eq!(found[OsStr::new("TERM")], "kuvo");
}

fn child_spec(mode: &str) -> Spawn {
    let mut child = Spawn::new(
        std::env::current_exe().unwrap(),
        Environment::from([("KUVO_OS_TEST_MODE".into(), mode.into())]),
    );
    child.args = ["--ignored", "--exact", "subprocess_entry", "--nocapture"]
        .into_iter()
        .map(Into::into)
        .collect();
    child.stdin = Stdio::Pipe;
    child.stdout = Stdio::Pipe;
    child.stderr = Stdio::Inherit;
    child
}

fn read_field(child: &mut ChildGroup, marker: &'static str) -> String {
    let mut lines = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let read = std::thread::spawn(move || {
        let mut line = String::new();
        while lines.read_line(&mut line).unwrap() > 0 {
            if let Some(value) = line.trim_end().strip_prefix(marker) {
                let _ = tx.send(value.to_owned());
                break;
            }
            line.clear();
        }
        lines.into_inner()
    });
    let result = rx.recv_timeout(LIMIT);
    if result.is_err() {
        let _ = child.stop(Duration::ZERO, |_| {});
    }
    child.stdout = Some(read.join().unwrap());
    result.unwrap_or_else(|error| panic!("waiting for child output {marker}: {error}"))
}

#[test]
fn child_only_receives_the_named_environment_and_its_pid_is_checked_with_start_identity() {
    let mut spec = child_spec("environment");
    spec.env.insert("EXPLICIT".into(), "provided".into());
    let mut child = spec.spawn().unwrap();
    assert_eq!(read_field(&mut child, "RESULT="), "provided:absent");
    let known = child.identity().clone();
    assert_eq!(
        ProcessIdentity::read(known.pid).unwrap(),
        Some(known.clone())
    );
    let different = ProcessIdentity {
        start_identity: "1".to_owned(),
        ..known.clone()
    };
    assert_eq!(different.state(), ProcessState::Stale);
    #[cfg(windows)]
    assert_eq!(
        ProcessIdentity {
            pid: known.pid,
            start_identity: "0".into()
        }
        .state(),
        ProcessState::Unknown
    );
    assert_eq!(known.state(), ProcessState::Live);
    child.stdin.as_mut().unwrap().write_all(b"q").unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(known.state(), ProcessState::Stale);
}

#[tokio::test]
async fn an_exit_watch_waits_for_the_exact_child_without_reaping_it() {
    let mut child = child_spec("environment").spawn().unwrap();
    let _ = read_field(&mut child, "RESULT=");
    let identity = child.identity().clone();
    let mut exited = identity.watch_exit().unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(100), &mut exited).await.is_err());
    child.stdin.as_mut().unwrap().write_all(b"q").unwrap();
    tokio::time::timeout(LIMIT, exited).await.unwrap().unwrap().unwrap();
    assert!(child.wait().unwrap().success());
    let stale = identity.watch_exit().unwrap();
    tokio::time::timeout(LIMIT, stale).await.unwrap().unwrap().unwrap();
}

#[cfg(unix)]
#[test]
fn process_group_gets_term_before_a_forced_stop() {
    let temp = scratch();
    let mut spec = Spawn::new("/bin/sh", Environment::new());
    spec.cwd = Some(temp.path().canonicalize().unwrap());
    spec.args = [
        "-c",
        "trap 'printf graceful > stopped; exit 42' TERM; printf 'READY=1\\n'; while :; do :; done",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    spec.stdout = Stdio::Pipe;
    let mut child = spec.spawn().unwrap();
    assert_eq!(read_field(&mut child, "READY="), "1");
    assert_eq!(child.stop(LIMIT, |_| {}).unwrap().code(), Some(42));
    assert_eq!(
        fs::read_to_string(temp.path().join("stopped")).unwrap(),
        "graceful"
    );
}

#[cfg(unix)]
#[test]
fn hard_stop_reaps_the_group_even_with_a_term_ignoring_child() {
    let mut spec = Spawn::new("/bin/sh", Environment::new());
    spec.args = [
        "-c",
        "trap '' TERM; /bin/sleep 1000 & printf 'DESC=%s\\n' \"$!\"; wait",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    spec.stdout = Stdio::Pipe;
    let mut child = spec.spawn().unwrap();
    let pid = read_field(&mut child, "DESC=").parse().unwrap();
    let descendant = ProcessIdentity::read(pid).unwrap().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        child.stop(Duration::ZERO, |_| {}).unwrap().signal(),
        Some(9)
    );
    bounded(
        || descendant.state() == ProcessState::Stale,
        "grandchild survived group stop",
    );
}

#[cfg(windows)]
#[test]
fn windows_headless_process_can_stop_gracefully_over_stdin() {
    let mut child = child_spec("environment").spawn().unwrap();
    assert_eq!(read_field(&mut child, "RESULT="), ":absent");
    let status = child
        .stop(LIMIT, |child| {
            child.stdin.as_mut().unwrap().write_all(b"q").unwrap()
        })
        .unwrap();
    assert!(status.success());
}

#[cfg(windows)]
#[test]
fn windows_job_closes_its_descendants_when_its_owner_is_terminated() {
    use std::process::{Command, Stdio as StdStdio};
    let spec = child_spec("job-owner");
    // Deliberately not a ChildGroup: terminating the owner must not itself terminate its child.
    let mut owner = Command::new(spec.executable.path())
        .args(&spec.args)
        .env_clear()
        .envs(spec.env)
        .stdin(StdStdio::piped())
        .stdout(StdStdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let mut out = BufReader::new(owner.stdout.take().unwrap());
    let pid = loop {
        line.clear();
        if out.read_line(&mut line).unwrap() == 0 {
            let status = owner.wait().unwrap();
            panic!("job owner exited before starting its descendant: {status}");
        }
        if let Some(value) = line.trim().strip_prefix("DESC=") {
            break value.parse().unwrap();
        }
    };
    let identity = ProcessIdentity::read(pid).unwrap().unwrap();
    drop(out);
    owner.kill().unwrap();
    owner.wait().unwrap();
    bounded(
        || identity.state() == ProcessState::Stale,
        "descendant survived job owner exit",
    );
}

#[cfg(windows)]
#[test]
fn windows_runs_an_explicit_cmd_shim_with_arguments_and_job_ownership() {
    use kuvo_os::platform::Executable;
    let temp = scratch();
    let script = temp.path().join("tool with spaces.cmd");
    fs::write(
        &script,
        b"@echo off\r\nsetlocal DisableDelayedExpansion\r\necho RESULT=%1\r\nexit /b 17\r\n",
    )
    .unwrap();
    let mut spawn = Spawn::new(&script, Environment::new());
    spawn.executable = Executable::CmdScript(script);
    spawn.args = vec!["two words & more".into()];
    spawn.stdout = Stdio::Pipe;
    let mut child = spawn.spawn().unwrap();
    assert_eq!(read_field(&mut child, "RESULT="), "\"two words & more\"");
    assert_eq!(child.wait().unwrap().code(), Some(17));
}

#[test]
#[ignore = "run only as the controlled child of a scenario"]
fn subprocess_entry() {
    let mode = std::env::var("KUVO_OS_TEST_MODE").unwrap();
    match mode.as_str() {
        "environment" => {
            println!(
                "RESULT={}:{}",
                std::env::var("EXPLICIT").unwrap_or_default(),
                if std::env::var_os("PATH").is_none() {
                    "absent"
                } else {
                    "inherited"
                }
            );
            let mut input = [0_u8; 1];
            std::io::stdin().read_exact(&mut input).unwrap();
            assert_eq!(input, [b'q']);
        }
        #[cfg(unix)]
        "path-reader" => {
            let env = Environment::from([
                ("SHELL".into(), std::env::var_os("TEST_PROFILE").unwrap()),
                ("HOME".into(), std::env::var_os("TEST_HOME").unwrap()),
            ]);
            let until = Duration::from_secs(60);
            let _ = kuvo_os::platform::login_environment(
                &Account::current().unwrap(),
                &env,
                until,
                until,
            );
        }
        #[cfg(windows)]
        "uncooperative" => {
            println!("READY=1");
            loop {
                std::thread::park();
            }
        }
        #[cfg(windows)]
        "job-owner" => {
            let mut child = child_spec("environment").spawn().unwrap();
            assert_eq!(read_field(&mut child, "RESULT="), ":absent");
            println!("DESC={}", child.identity().pid);
            let mut input = [0_u8; 1];
            let _ = std::io::stdin().read_exact(&mut input);
        }
        _ => panic!("unexpected child mode {mode}"),
    }
}

#[test]
fn windows_named_pipe_spelling_matches_the_key_sid_and_channel_on_the_wire() {
    let sid = "S-1-5-21-123-456-789-1001";
    let pipe =
        windows_pipe_path(sid, &[0x11; 32], &Channel::Holder("012345abcdef".into())).unwrap();
    assert_eq!(
        pipe,
        r"\\.\pipe\kuvo-0745ec8b84248d4f49e4847e9e4db493c8190e7416216b63d9c7c773016a0b1d"
    );
    assert_ne!(
        pipe,
        windows_pipe_path(sid, &[0x11; 32], &Channel::Ssh).unwrap()
    );
    assert!(windows_pipe_path(sid, &[0x11; 32], &Channel::Holder("a/b".into())).is_err());
}

#[tokio::test]
async fn ipc_exchanges_bytes_on_consecutive_connections_and_rejects_a_duplicate_listener() {
    let temp = scratch();
    let endpoint = Endpoint::new(
        &temp.path().join("endpoint").join("ssh"),
        &[0x12; 32],
        Channel::Ssh,
    )
    .unwrap();
    let other = if cfg!(windows) {
        Endpoint::new(endpoint.path(), &[0x13; 32], Channel::Ssh).unwrap()
    } else {
        Endpoint::new(
            &temp.path().join("endpoint").join("other"),
            &[0x13; 32],
            Channel::Ssh,
        )
        .unwrap()
    };

    tokio::time::timeout(LIMIT, async {
        let mut listener = endpoint.bind().unwrap();
        assert!(endpoint.bind().is_err());
        assert!(other.connect().await.is_err());
        for byte in [0x00, 0xba] {
            let mut client = endpoint.connect().await.unwrap();
            // Unix duplicate bind may have probed the existing listener with an empty connection.
            let exchange = async {
                loop {
                    let mut server = listener.accept().await.unwrap();
                    let mut request = [0_u8; 1];
                    if server.read_exact(&mut request).await.is_ok() {
                        assert_eq!(request, [byte]);
                        server.write_all(&[request[0], b'!']).await.unwrap();
                        break;
                    }
                }
            };
            let client_exchange = async {
                client.write_all(&[byte]).await.unwrap();
                let mut reply = [0_u8; 2];
                client.read_exact(&mut reply).await.unwrap();
                assert_eq!(reply, [byte, b'!']);
            };
            tokio::join!(exchange, client_exchange);
        }
        drop(listener);
        assert!(endpoint.connect().await.is_err());
        assert!(endpoint.bind().is_ok());
    })
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn unix_ipc_recovers_only_private_stale_sockets_and_accepts_legacy_holders() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    let temp = scratch();
    let folder = temp.path().join("ipc");
    fs::create_dir(&folder).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    let path = folder.join("ssh");
    drop(UnixListener::bind(&path).unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    let endpoint = Endpoint::new(&path, &[0; 32], Channel::Ssh).unwrap();
    assert_eq!(
        endpoint.connect().await.err().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        endpoint.bind().err().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        endpoint.connect().await.err().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let rebuilt = endpoint.bind().unwrap();
    assert_eq!(
        fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(rebuilt);
    assert!(!path.exists());

    let holder_path = folder.join("hold");
    let holder_server = UnixListener::bind(&holder_path).unwrap();
    fs::set_permissions(&holder_path, fs::Permissions::from_mode(0o755)).unwrap();
    let holder = Endpoint::new(
        &holder_path,
        &[0; 32],
        Channel::Holder("012345abcdef".into()),
    )
    .unwrap();
    let _client = holder.connect().await.unwrap();
    assert!(holder_server.accept().is_ok());
}

async fn changed(subscription: &mut Subscription, wanted: &Path) {
    tokio::time::timeout(LIMIT, async {
        loop {
            match subscription.recv().await.expect("watch closed") {
                Notice::Changed { paths } if paths.iter().any(|path| path == wanted) => return,
                Notice::Failed(error) => panic!("watch failed: {error}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no watch event for {}", wanted.display()));
}

#[tokio::test]
async fn recursive_and_shallow_watchers_share_folder_and_each_get_their_events() {
    let temp = scratch();
    let folder = temp.path().join("made");
    let hub = WatchHub::default();
    let mut shallow = hub
        .subscribe(
            &folder,
            Options {
                create: true,
                recursive: false,
            },
        )
        .await
        .unwrap();
    fs::create_dir(folder.join("nested")).unwrap();
    let mut deep = hub
        .subscribe(
            folder.join("."),
            Options {
                create: false,
                recursive: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(hub.watched_folders().await.unwrap(), 1);
    assert!(matches!(deep.recv().await, Some(Notice::Rescan)));
    fs::write(folder.join("nested").join("first"), "contents").unwrap();
    changed(&mut deep, Path::new("nested").join("first").as_path()).await;
    fs::write(folder.join("second"), "top level").unwrap();
    // The shallow subscriber must never receive a nested event even after the upgrade.
    tokio::time::timeout(LIMIT, async {
        loop {
            if let Notice::Changed { paths } = shallow.recv().await.unwrap() {
                assert!(paths.iter().all(|path| path.components().count() <= 1));
                if paths.iter().any(|path| path == Path::new("second")) {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();
    changed(&mut deep, Path::new("second")).await;
}

#[tokio::test(start_paused = true)]
async fn reconnect_reuses_the_watcher_during_grace_and_the_last_drop_releases_it() {
    let temp = scratch();
    let hub = WatchHub::new(Duration::from_millis(500), Duration::from_secs(1), false);
    let first = hub
        .subscribe(temp.path(), Options::default())
        .await
        .unwrap();
    drop(first);
    assert_eq!(hub.watched_folders().await.unwrap(), 1);
    tokio::time::advance(Duration::from_millis(499)).await;
    let second = hub
        .subscribe(temp.path(), Options::default())
        .await
        .unwrap();
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(hub.watched_folders().await.unwrap(), 1);
    drop(second);
    tokio::time::advance(Duration::from_millis(500)).await;
    let deadline = Instant::now() + LIMIT;
    while hub.watched_folders().await.unwrap() != 0 {
        assert!(
            Instant::now() < deadline,
            "the last subscriber left its watcher open"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn polling_delivers_same_size_changes_to_an_existing_file() {
    let temp = scratch();
    let path = temp.path().join("file");
    fs::write(&path, "first").unwrap();
    let hub = WatchHub::new(Duration::from_millis(500), Duration::from_millis(20), true);
    let mut watcher = hub
        .subscribe(temp.path(), Options::default())
        .await
        .unwrap();
    assert_eq!(watcher.backend(), Backend::Poll);
    fs::write(&path, "other").unwrap();
    changed(&mut watcher, Path::new("file")).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "run on WSL with KUVO_RS13_WSL_DRIVE pointing to a scratch folder on /mnt/<drive>"]
async fn watching_a_windows_drive_on_wsl_selects_polling_and_delivers_changes() {
    assert_eq!(Platform::current(), Platform::Wsl);
    let scratch = std::env::var_os("KUVO_RS13_WSL_DRIVE")
        .map(std::path::PathBuf::from)
        .unwrap();
    assert!(kuvo_os::platform::is_wsl_windows_drive(&scratch));
    let hub = WatchHub::new(Duration::from_millis(500), Duration::from_millis(20), false);
    let mut watcher = hub.subscribe(&scratch, Options::default()).await.unwrap();
    assert_eq!(watcher.backend(), Backend::Poll);
    fs::write(scratch.join("watched"), "windows drive").unwrap();
    changed(&mut watcher, Path::new("watched")).await;
}

#[cfg(target_os = "macos")]
#[test]
fn seatbelt_launch_uses_the_generated_profile_and_preserves_the_command() {
    let temp = scratch();
    let root = temp.path().canonicalize().unwrap();
    let profile = root.join("rules.sb");
    let guarded = root.join("guarded");
    fs::create_dir(&guarded).unwrap();
    fs::write(
        &profile,
        format!(
            "(version 1)\n(allow default)\n(deny file-write* (subpath \"{}\"))\n",
            guarded.display()
        ),
    )
    .unwrap();
    let mut command = Spawn::new("/bin/sh", Environment::new());
    command.cwd = Some(root.clone());
    command.args = [
        "-c",
        "printf '%s' \"$1\" > allowed; printf fail > guarded/denied",
        "sh",
        "with spaces",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let spec = kuvo_os::sandbox::seatbelt(command, &profile).unwrap();
    let status = spec.spawn().unwrap().wait().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("allowed")).unwrap(),
        "with spaces"
    );
    assert!(!status.success());
    assert!(!guarded.join("denied").exists());
}

#[cfg(windows)]
#[test]
fn native_windows_keeps_auto_unavailable() {
    assert!(!kuvo_os::sandbox::supported_platform());
    let command = child_spec("environment");
    assert_eq!(
        kuvo_os::sandbox::seatbelt(command, Path::new(r"C:\no-profile"))
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
}

#[cfg(windows)]
#[tokio::test]
async fn named_pipe_acl_is_protected_and_grants_access_only_to_its_account_sid() {
    use std::{os::windows::io::AsRawHandle, ptr::null_mut};
    use tokio::net::windows::named_pipe::ClientOptions;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo,
                SDDL_REVISION_1, SE_KERNEL_OBJECT,
            },
            DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        },
    };

    let key = [0x44; 32];
    let endpoint = Endpoint::new(Path::new("ignored on Windows"), &key, Channel::Ssh).unwrap();
    let _listener = endpoint.bind().unwrap();
    let client = ClientOptions::new().open(endpoint.path()).unwrap();
    let info = DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION;
    let mut descriptor = null_mut();
    // SAFETY: the pipe handle is open, and the buffer pointer is writable.
    let status = unsafe {
        GetSecurityInfo(
            client.as_raw_handle(),
            SE_KERNEL_OBJECT,
            info,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(status, 0, "GetSecurityInfo failed");

    let mut text = null_mut();
    let mut len = 0;
    // SAFETY: descriptor is the valid object returned by GetSecurityInfo; outputs are writable.
    let converted = unsafe {
        let success = ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            info,
            &mut text,
            &mut len,
        );
        LocalFree(descriptor);
        success
    };
    assert_ne!(converted, 0, "could not read the pipe's SDDL");
    // SAFETY: text points to len UTF-16 code units including the trailing NUL.
    let sddl = unsafe {
        let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, len as usize - 1));
        LocalFree(text.cast());
        sddl
    };
    let (owner, dacl) = sddl.strip_prefix("O:").unwrap().split_once("D:").unwrap();
    // The kernel maps GENERIC_ALL to FILE_ALL_ACCESS on the created pipe.
    assert_eq!(dacl, format!("P(A;;FA;;;{owner})"));
    assert_eq!(
        endpoint.path(),
        Path::new(&windows_pipe_path(owner, &key, &Channel::Ssh).unwrap())
    );
}

#[cfg(unix)]
#[test]
fn a_login_profile_is_stopped_if_the_process_that_reads_it_dies() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch();
    let shell = root.path().join("hanging-profile");
    fs::write(
        &shell,
        "#!/bin/sh\n/bin/sleep 12 &\nprintf '%s\\n' \"$$\" > profile.pid\nwait\n",
    )
    .unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    let mut spec = child_spec("path-reader");
    spec.env
        .insert("TEST_PROFILE".into(), shell.as_os_str().to_owned());
    spec.env
        .insert("TEST_HOME".into(), root.path().as_os_str().to_owned());
    let mut owner = spec.spawn().unwrap();
    let record = root.path().join("profile.pid");
    bounded(
        || fs::read_to_string(&record).is_ok_and(|text| text.ends_with('\n')),
        "login shell did not start",
    );
    let pid = fs::read_to_string(record).unwrap().trim().parse().unwrap();
    let profile = ProcessIdentity::read(pid).unwrap().unwrap();

    owner.stop(Duration::ZERO, |_| {}).unwrap();
    let deadline = Instant::now() + LIMIT;
    while profile.state() != ProcessState::Stale {
        assert!(
            Instant::now() < deadline,
            "login shell {pid} outlived its reader: {:?}; {:?}",
            ProcessIdentity::read(pid),
            std::process::Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "pid,ppid,pgid,stat,args"])
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        );
        std::thread::yield_now();
    }
}

#[cfg(windows)]
#[test]
fn windows_terminates_a_job_that_ignores_graceful_shutdown() {
    let mut child = child_spec("uncooperative").spawn().unwrap();
    assert_eq!(read_field(&mut child, "READY="), "1");
    let identity = child.identity().clone();
    assert_eq!(child.stop(Duration::ZERO, |_| {}).unwrap().code(), Some(1));
    assert_eq!(identity.state(), ProcessState::Stale);
}

#[cfg(unix)]
#[test]
fn a_descendant_has_time_to_finish_after_the_group_leader_exits() {
    let temp = scratch();
    let root = temp.path().canonicalize().unwrap();
    let mut spec = Spawn::new("/bin/sh", Environment::new());
    spec.cwd = Some(root.clone());
    spec.args = [
        "-c",
        r#"
        trap 'exit 42' TERM
        /bin/sh -c 'trap "while [ ! -e release ]; do :; done; printf done > descendant; exit 0" TERM
          printf "READY=1\n"; while :; do :; done' &
        wait
    "#,
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    spec.stdout = Stdio::Pipe;
    let mut child = spec.spawn().unwrap();
    assert_eq!(read_field(&mut child, "READY="), "1");
    let leader = child.identity().clone();
    let (send, done) = std::sync::mpsc::sync_channel(1);
    let stop = std::thread::spawn(move || {
        send.send(child.stop(LIMIT, |_| {})).unwrap();
    });
    bounded(
        || leader.state() == ProcessState::Stale,
        "leader did not handle TERM",
    );
    let premature = done.recv_timeout(Duration::from_millis(100));
    fs::write(root.join("release"), "").unwrap();
    assert!(
        matches!(premature, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        "stop returned before the descendant had a chance to finish"
    );
    assert_eq!(done.recv_timeout(LIMIT).unwrap().unwrap().code(), Some(42));
    stop.join().unwrap();
    assert_eq!(fs::read_to_string(root.join("descendant")).unwrap(), "done");
}

/// The resident ignores SIGPIPE (Rust does at startup); an agent it starts
/// must get the default, so a pipeline it runs ends quietly on a closed pipe.
#[cfg(unix)]
#[test]
fn a_child_starts_with_the_default_broken_pipe_signal() {
    let Some(perl) = ["/usr/bin/perl", "/bin/perl"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
    else {
        eprintln!("skipped: no perl to report the inherited disposition");
        return;
    };
    // SAFETY: this test process is single purpose; ignoring SIGPIPE here is
    // what the resident's own runtime does before it starts children.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let mut spec = Spawn::new(perl, Environment::new());
    spec.args = ["-e", "print 'PIPE=', ($SIG{PIPE} // 'DEFAULT'), \"\\n\""]
        .into_iter()
        .map(Into::into)
        .collect();
    spec.stdout = Stdio::Pipe;
    let mut child = spec.spawn().unwrap();
    assert_eq!(read_field(&mut child, "PIPE="), "DEFAULT");
    child.wait().unwrap();
}
