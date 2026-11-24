//! The one line portable installations may put in the account's crontab.

use std::{
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::Output,
    thread,
    time::{Duration, Instant},
};

use kuvo_os::{
    platform::{Account, Environment, Platform},
    process::{Spawn, Stdio},
};
use kuvo_store::home::Home;

const TAG: &[u8] = b"# kuvo-code: starts its engine again after a restart";
const MAX_TABLE: u64 = 2_000_000;

/// A scratch home can use a local stand-in. The real crontab belongs only to
/// the canonical installation in the system account.
pub fn uninstall(home: &Home, check: bool) -> io::Result<Option<bool>> {
    let account = Account::current()?;
    let mut env: Environment = std::env::vars_os().collect();
    let profile = account
        .home_folder(Platform::current(), &env)?
        .canonicalize()?;
    if home.root().canonicalize()? != profile.join(".kuvo-code") {
        return Ok(None);
    }
    let executable = if profile == account.home.canonicalize()? {
        ["/usr/bin/crontab", "/usr/sbin/crontab", "/bin/crontab"]
            .into_iter()
            .map(PathBuf::from)
            .find(|file| file.is_file())
    } else {
        env.get(std::ffi::OsStr::new("KUVO_TEST_UNINSTALL_TOOLS"))
            .map(PathBuf::from)
            .and_then(|path| path.canonicalize().ok())
            .filter(|path| path.starts_with(&profile))
            .map(|path| path.join("crontab"))
            .filter(|file| fs::symlink_metadata(file).is_ok_and(|entry| entry.is_file()))
    };
    let Some(executable) = executable else {
        return Ok(None);
    };
    env.insert("LC_ALL".into(), "C".into());
    let mut task = Spawn::new(executable, env);
    task.cwd = Some(profile);
    task.stdout = Stdio::Pipe;
    task.stderr = Stdio::Pipe;
    let listed = run(&mut task, "-l", None)?;
    if !listed.status.success() {
        if String::from_utf8_lossy(&listed.stderr)
            .to_ascii_lowercase()
            .contains("no crontab")
        {
            return Ok(Some(false));
        }
        return Err(io::Error::other(
            "Kuvo couldn’t read the account's crontab.",
        ));
    }
    let mut kept = Vec::with_capacity(listed.stdout.len());
    let mut found = false;
    for line in listed.stdout.split_inclusive(|byte| *byte == b'\n') {
        if line.windows(TAG.len()).any(|part| part == TAG) {
            found = true;
        } else {
            kept.extend_from_slice(line);
        }
    }
    if found && !check {
        if !kept.is_empty() && !kept.ends_with(b"\n") {
            kept.push(b'\n');
        }
        if !run(&mut task, "-", Some(kept))?.status.success() {
            return Err(io::Error::other(
                "Kuvo couldn’t remove its line in the crontab.",
            ));
        }
    }
    Ok(Some(found))
}

fn run(task: &mut Spawn, arg: &str, input: Option<Vec<u8>>) -> io::Result<Output> {
    task.args = vec![arg.into()];
    task.stdin = if input.is_some() {
        Stdio::Pipe
    } else {
        Stdio::Null
    };
    let mut child = task.spawn()?;
    thread::scope(|scope| {
        let stdout = scope.spawn(reader(child.stdout.take().expect("stdout pipe")));
        let stderr = scope.spawn(reader(child.stderr.take().expect("stderr pipe")));
        let writer = input
            .zip(child.stdin.take())
            .map(|(input, mut pipe)| scope.spawn(move || pipe.write_all(&input)));
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            match child.has_exited() {
                Ok(true) => break child.wait(),
                Ok(false) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                result => {
                    let _ = child.stop(Duration::from_secs(1), |_| {});
                    break result.and(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Kuvo’s crontab command took too long.",
                    )));
                }
            }
        }?;
        let failed = |_| io::Error::other("Kuvo couldn’t read the crontab command's reply.");
        if let Some(writer) = writer {
            writer.join().map_err(failed)??;
        }
        Ok(Output {
            status,
            stdout: stdout.join().map_err(failed)??,
            stderr: stderr.join().map_err(failed)??,
        })
    })
}

fn reader(pipe: impl Read) -> impl FnOnce() -> io::Result<Vec<u8>> {
    move || {
        let mut bytes = Vec::new();
        pipe.take(MAX_TABLE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_TABLE {
            return Err(io::Error::other("The account's crontab is too large."));
        }
        Ok(bytes)
    }
}
