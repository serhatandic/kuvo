//! Native filesystem and shell scenarios. These use a scratch NTFS folder
//! and the machine's real Git for Windows installation.

#![cfg(windows)]

use kuvo_os::{
    platform::{Environment, git_bash, git_bash_spawn},
    process::{Spawn, Stdio, process_parents, process_started, tcp_listeners},
    windows::{private_folder, private_home, system_directory},
};
use std::{
    env, fs,
    io::{BufRead, BufReader, ErrorKind, Read},
    net::TcpListener,
    path::Path,
    process::Command,
};

#[derive(Debug)]
struct Reply {
    success: bool,
    stdout: Vec<u8>,
}

fn bash(environment: &Environment, folder: &Path, source: &str) -> Reply {
    let mut spawn = git_bash_spawn(environment.clone(), folder, source).unwrap();
    spawn.stdout = Stdio::Pipe;
    spawn.stderr = Stdio::Inherit;
    let mut child = spawn.spawn().unwrap();
    let mut stdout = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
    let success = child.wait().unwrap().success();
    Reply { success, stdout }
}

#[test]
fn an_installation_refuses_an_acl_other_accounts_can_read() {
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path().join(".kuvo-code");
    assert_eq!(
        private_home(&home, false).unwrap_err().kind(),
        ErrorKind::NotFound,
    );
    private_home(&home, true).unwrap();
    private_home(&home, false).unwrap();
    fs::write(home.join("private"), "test").unwrap();
    let grant = Command::new(system_directory().unwrap().join("icacls.exe"))
        .arg(&home)
        .args(["/grant", "*S-1-1-0:(OI)(CI)(RX)"])
        .output()
        .unwrap();
    assert!(grant.status.success(), "{grant:?}");
    for create in [false, true] {
        assert_eq!(
            private_home(&home, create).unwrap_err().kind(),
            ErrorKind::PermissionDenied,
        );
        assert_eq!(fs::read_to_string(home.join("private")).unwrap(), "test");
    }
}

#[test]
fn git_bash_runs_crlf_commands_and_git_tracks_a_long_path_with_spaces() {
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path().join("private");
    private_home(&home, true).unwrap();
    let folder = home
        .join("Folder with spaces".repeat(7))
        .join("Another folder".repeat(10));
    assert!(folder.as_os_str().len() > 260);
    fs::create_dir_all(&folder).unwrap();
    let mut environment: kuvo_os::platform::Environment = env::vars_os().collect();
    if let Err(error) = git_bash(&environment) {
        assert_eq!(
            error.to_string(),
            "Git for Windows is required for Kuvo shell commands"
        );
        panic!("This Windows scenario requires Git for Windows");
    }
    let reply = bash(
        &environment,
        &folder,
        "printf '%s\n' 'First line' > 'File With Spaces.txt'\r\n",
    );
    assert!(reply.success, "{reply:?}");
    let file = folder.join("File With Spaces.txt");
    assert_eq!(fs::read(&file).unwrap(), b"First line\n");

    // Native Git can track long paths, but Windows won't start it in a
    // working directory beyond MAX_PATH. Run it from this project's root.
    let relative = file
        .strip_prefix(&home)
        .unwrap()
        .to_str()
        .unwrap()
        .replace('\\', "/");
    environment.insert("TEST_LONG_PATH".into(), relative.as_str().into());
    let source = concat!(
        "git init --quiet &&\r\n",
        "git add -- \"$TEST_LONG_PATH\" &&\r\n",
        "git ls-files\r\n"
    );
    let reply = bash(&environment, &home, source);
    assert!(reply.success, "{reply:?}");
    assert_eq!(String::from_utf8_lossy(&reply.stdout).trim_end(), relative);

    let link = bash(
        &environment,
        &folder,
        "ln -s 'File With Spaces.txt' 'Project link'",
    );
    if link.success {
        assert!(
            fs::symlink_metadata(folder.join("Project link"))
                .unwrap()
                .is_symlink()
        );
    } else {
        assert!(
            fs::symlink_metadata(folder.join("Project link")).is_err(),
            "{link:?}"
        );
    }
}

#[test]
fn listening_sockets_name_their_process_and_its_parent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let own = std::process::id();
    assert!(tcp_listeners().unwrap().contains(&(own, port)));

    // A child that listens names this process as its parent, and started after it.
    let powershell = system_directory()
        .unwrap()
        .join(r"WindowsPowerShell\v1.0\powershell.exe");
    let environment = env::vars_os().filter(|(name, _)| !name.as_encoded_bytes().contains(&b'='));
    let mut spawn = Spawn::new(powershell, environment.collect());
    spawn.args = [
        "-NoProfile",
        "-Command",
        "$l=[Net.Sockets.TcpListener]::new([Net.IPAddress]::IPv6Loopback,0);$l.Start();\
         [Console]::WriteLine($l.LocalEndpoint.Port);Start-Sleep 60",
    ]
    .map(Into::into)
    .to_vec();
    spawn.stdout = Stdio::Pipe;
    let mut child = spawn.spawn().unwrap();
    let pid = child.identity().pid;
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port: u16 = line.trim().parse().unwrap();
    assert!(tcp_listeners().unwrap().contains(&(pid, port)));
    assert_eq!(process_parents().unwrap().get(&pid), Some(&own));
    assert!(process_started(own).unwrap() <= process_started(pid).unwrap());
    child.stop(std::time::Duration::ZERO, |_| {}).unwrap();
    assert_eq!(process_started(pid), None);
}

#[test]
fn a_secret_folder_is_closed_to_others_and_refused_when_another_account_owns_it() {
    let scratch = tempfile::tempdir().unwrap();
    let folder = scratch.path().join("keys");
    fs::create_dir(&folder).unwrap();
    let icacls = system_directory().unwrap().join("icacls.exe");
    let acl = |args: &[&str]| {
        let reply = Command::new(&icacls).arg(&folder).args(args).output().unwrap();
        assert!(reply.status.success(), "{reply:?}");
        String::from_utf8_lossy(&reply.stdout).into_owned()
    };
    acl(&["/grant", "*S-1-1-0:(OI)(CI)(RX)"]);
    private_folder(&folder).unwrap();
    // Only this account is left, with nothing inherited from the scratch folder.
    let listed = acl(&[]);
    let entries: Vec<_> = listed.lines().filter(|line| line.contains(":(")).collect();
    assert_eq!(entries.len(), 1, "{listed}");
    assert!(entries[0].contains("(OI)(CI)(F)") && !entries[0].contains("(I)"), "{listed}");
    // A private one stays as it is; one another account owns is refused.
    private_folder(&folder).unwrap();
    acl(&["/grant", "*S-1-5-19:(OI)(CI)(F)"]);
    acl(&["/setowner", "*S-1-5-19"]);
    assert_eq!(
        private_folder(&folder).unwrap_err().kind(),
        ErrorKind::PermissionDenied
    );
}
