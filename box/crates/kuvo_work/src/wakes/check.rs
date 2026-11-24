//! A watch's check, run as `kuvo-wake` runs one: a shell command in the folder its conversation
//! works in, for at most a minute, in the box's sandbox while that's on. What it prints is the
//! reading; a run that's no reading (the network down, out of time) is said why.
//!
//! A check's status alone can't tell an outage from "nothing there": in `curl -s … | grep -o …`
//! it's grep's, 1 both times. So its curl, wget and gh say how they did, and a run is no reading
//! when it ran out of time, couldn't run or was killed; when a curl or wget in it failed (an error
//! page is a reading); or when it printed nothing and a gh in it failed, or it exited 2 or more.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::ExitStatus,
    time::{Duration, Instant},
};

use kuvo_os::process::{Spawn, Stdio};

const LIMIT: Duration = Duration::from_secs(60);
const GRACE: Duration = Duration::from_secs(5);
const OUTPUT: usize = 4_000;

/// Before a check: its curl, wget and gh note their failures (not an error page, checks still
/// pending, or what reads them stopping early) in the file `KUVO_WATCH_FAILED` names. A curl that
/// writes out its own result (`-w '%{http_code}'`) never does.
const NOTING: &str = r#"kuvo_ran() { case $1 in 0|141|$3|$4) ;; *) echo "$2 $1" >>"$KUVO_WATCH_FAILED" ;; esac; return $1; }
curl() { command curl "$@"; kuvo_s=$?; for kuvo_a; do case $kuvo_a in --write-out*|-w*|-[!-]*w*) return $kuvo_s ;; esac; done; kuvo_ran $kuvo_s curl 22 23; }
wget() { command wget "$@"; kuvo_ran $? wget 3 8; }
gh() { command gh "$@"; kuvo_ran $? gh 8 8; }
"#;

/// The box's sandbox, as checks see it: off, on (with its launcher), or set up but not on yet
/// (checks wait rather than run without it).
#[derive(Clone, Debug)]
pub enum Fence {
    Off,
    On(PathBuf),
    NotReady,
}

impl Fence {
    pub fn of(home: &Path) -> Self {
        if !home.join("run").is_dir() {
            return Self::Off;
        }
        let launcher = home.join("bin").join("kuvo-fence");
        if home.join("run/fence/on").is_file() && executable(&launcher) {
            Self::On(launcher)
        } else if home.join("run/fence/ready").is_file() {
            Self::NotReady
        } else {
            Self::Off
        }
    }
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

/// One run: what it printed (up to 4,000 bytes, line ends at its end left off), or why it's no
/// reading.
#[derive(Debug, Eq, PartialEq)]
pub enum Reading {
    Printed(String),
    None(String),
}

/// Runs a watch's check once (`watch`, its folder; `owner`, the folder of the conversation it's
/// for). Blocking: call it on a worker.
pub fn run(
    watch: &Path,
    owner: &Path,
    fence: &Fence,
    env: &kuvo_os::platform::Environment,
) -> Reading {
    if matches!(fence, Fence::NotReady) {
        return Reading::None("the sandbox isn't ready yet".into());
    }
    let directory = PathBuf::from(super::folder::text(&owner.join("directory")));
    let directory = if directory.is_absolute() && directory.is_dir() {
        directory
    } else if matches!(fence, Fence::On(_)) {
        return Reading::None("its conversation's folder isn't there".into());
    } else {
        owner.to_owned()
    };
    let check = super::folder::text(&watch.join("check"));
    let scratch = match tempfile::Builder::new().prefix(".run-").tempdir_in(watch) {
        Ok(scratch) => scratch,
        Err(_) => return Reading::None("the check didn't finish".into()),
    };
    let (out, failed) = (scratch.path().join("out"), scratch.path().join("failed"));
    let status = run_shell(
        &format!("{NOTING}{check}"),
        &directory,
        &out,
        &failed,
        fence,
        owner,
        env,
    );
    let printed = fs::read(&out).unwrap_or_default();
    let printed = String::from_utf8_lossy(&printed[..printed.len().min(OUTPUT)]);
    let printed = printed.trim_end_matches('\n').to_owned();
    let noted = fs::read_to_string(&failed).unwrap_or_default();
    let first = noted.lines().next().map(str::to_owned);
    let lost = noted
        .lines()
        .find(|line| !line.starts_with("gh "))
        .map(str::to_owned);
    let words = |line: &str| {
        let (tool, code) = line.rsplit_once(' ').unwrap_or((line, ""));
        (tool.to_owned(), code.to_owned())
    };
    let why = match status {
        None => Some("the check didn't finish".to_owned()),
        Some(124) => Some("the check ran out of time (a minute)".to_owned()),
        Some(code @ 125..=128) => Some(format!("the check couldn't run (exit {code})")),
        Some(code) if code > 128 && code != 141 => {
            Some(format!("the check was stopped (signal {})", code - 128))
        }
        Some(code) => {
            if let Some(line) = lost {
                let (tool, exit) = words(&line);
                Some(format!("{tool} failed (exit {exit})"))
            } else if !printed.is_empty() {
                None
            } else if let Some(line) = first {
                let (tool, exit) = words(&line);
                Some(format!(
                    "{tool} failed (exit {exit}), and the check printed nothing"
                ))
            } else if code >= 2 {
                Some(format!(
                    "the check failed (exit {code}) without printing anything"
                ))
            } else {
                None
            }
        }
    };
    match why {
        Some(why) => Reading::None(why),
        None => Reading::Printed(printed),
    }
}

fn shell() -> Option<PathBuf> {
    if cfg!(unix) {
        return Some(PathBuf::from("/bin/sh"));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|folder| folder.join("sh.exe"))
        .find(|candidate| candidate.is_file())
}

/// The script's status, as `timeout` gives it: 124 when out of time, 128 and the signal when
/// killed; none when it couldn't be read.
fn run_shell(
    script: &str,
    directory: &Path,
    out: &Path,
    failed: &Path,
    fence: &Fence,
    owner: &Path,
    base: &kuvo_os::platform::Environment,
) -> Option<i32> {
    let mut env = base.clone();
    env.insert("KUVO_WATCH_FAILED".into(), failed.as_os_str().to_owned());
    let program = match fence {
        Fence::On(launcher) => {
            env.insert("KUVO_FENCE_WORK".into(), directory.as_os_str().to_owned());
            env.insert("KUVO_FENCE_SHELL".into(), "/bin/sh".into());
            env.insert(
                "OPENCODE_SESSION_ID".into(),
                super::folder::text(&owner.join("session")).into(),
            );
            launcher.clone()
        }
        _ => match shell() {
            Some(shell) => shell,
            None => return Some(127),
        },
    };
    let mut spawn = Spawn::new(program, env);
    spawn.args = vec!["-c".into(), OsString::from(script), "sh".into()];
    spawn.cwd = Some(directory.to_owned());
    spawn.stdout = Stdio::File(fs::File::create(out).ok()?);
    let Ok(mut child) = spawn.spawn() else {
        return Some(127);
    };
    let started = Instant::now();
    loop {
        match child.has_exited() {
            Ok(true) => return child.wait().ok().map(code),
            Ok(false) => {}
            Err(_) => return None,
        }
        if started.elapsed() >= LIMIT {
            let _ = child.stop(GRACE, |_| {});
            return Some(124);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

/// What a check's output sums to, as `kuvo-wake` keeps it (`2/` and POSIX `cksum`'s line), so a
/// watch a shell box kept goes on comparing alike.
pub fn sum(output: &str) -> String {
    let bytes = output.as_bytes();
    let mut crc: u32 = 0;
    let mut feed = |byte: u8| {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    };
    for &byte in bytes {
        feed(byte);
    }
    let mut length = bytes.len();
    while length > 0 {
        feed((length & 0xff) as u8);
        length >>= 8;
    }
    format!("2/{} {}", !crc, bytes.len())
}

/// The environment a check runs with: the box's own, with what the engine was given taken back
/// (`KUVO_OWN_<name>`), and never the engine's password or settings.
pub fn environment() -> kuvo_os::platform::Environment {
    let mut env: kuvo_os::platform::Environment = std::env::vars_os().collect();
    let own: Vec<(OsString, OsString)> = env
        .iter()
        .filter_map(|(name, value)| {
            let name = name.to_str()?.strip_prefix("KUVO_OWN_")?;
            Some((OsString::from(name), value.clone()))
        })
        .collect();
    for (name, value) in own {
        env.remove(&OsString::from(format!(
            "KUVO_OWN_{}",
            name.to_string_lossy()
        )));
        if value.is_empty() {
            env.remove(&name);
        } else {
            env.insert(name, value);
        }
    }
    for name in [
        "OPENCODE_PASSWORD",
        "OPENCODE_SERVER_PASSWORD",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_CONFIG",
    ] {
        env.remove(&OsString::from(name));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_match_posix_cksum() {
        assert_eq!(sum(""), "2/4294967295 0");
        assert_eq!(sum("hello"), "2/3287646509 5");
    }

    #[cfg(unix)]
    #[test]
    fn a_reading_or_why_not() {
        let folder = tempfile::tempdir().unwrap();
        let owner = folder.path().join("owner");
        let watch = owner.join("watches/abc");
        fs::create_dir_all(&watch).unwrap();
        let env = environment();
        let ask = |check: &str| {
            fs::write(watch.join("check"), check).unwrap();
            run(&watch, &owner, &Fence::Off, &env)
        };
        assert_eq!(ask("printf 'up\\n\\n'"), Reading::Printed("up".into()));
        assert_eq!(ask("exit 1"), Reading::Printed(String::new()));
        assert_eq!(
            ask("exit 3"),
            Reading::None("the check failed (exit 3) without printing anything".into())
        );
        assert_eq!(
            ask("kuvo_ran 6 curl 22 23; echo page"),
            Reading::None("curl failed (exit 6)".into())
        );
        assert_eq!(
            ask("kuvo_ran 1 gh 8 8"),
            Reading::None("gh failed (exit 1), and the check printed nothing".into())
        );
        assert_eq!(
            ask("kill -9 $$"),
            Reading::None("the check was stopped (signal 9)".into())
        );
        assert_eq!(
            run(&watch, &owner, &Fence::NotReady, &env),
            Reading::None("the sandbox isn't ready yet".into())
        );
    }
}
