//! Own a child and its Unix process group or Windows job until the work finishes.

use crate::platform::{Environment, Executable};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    io::{self, PipeReader, PipeWriter},
    path::{Path, PathBuf},
    process::ExitStatus,
    time::Duration,
};

#[cfg(unix)]
#[path = "process/unix.rs"]
mod sys;
#[cfg(windows)]
#[path = "process/windows.rs"]
mod sys;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_identity: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Live,
    Stale,
    Unknown,
}

impl ProcessIdentity {
    /// None means the OS confirmed that the process is gone.
    pub fn read(pid: u32) -> io::Result<Option<Self>> {
        if pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "zero is not a process ID",
            ));
        }
        Ok(sys::read_start(pid)?.map(|start_identity| Self {
            pid,
            start_identity,
        }))
    }

    /// A one-shot notification of this exact process exiting. The wait belongs to a detached
    /// OS thread (not the runtime's blocking pool, so keeper handover need not wait for it).
    pub fn watch_exit(&self) -> io::Result<tokio::sync::oneshot::Receiver<io::Result<()>>> {
        let (send, receive) = tokio::sync::oneshot::channel();
        let wait = sys::exit_wait(self)?;
        std::thread::Builder::new()
            .name("process-exit".into())
            .spawn(move || {
                let _ = send.send(wait());
            })?;
        Ok(receive)
    }

    pub fn state(&self) -> ProcessState {
        if self.start_identity.is_empty() {
            return ProcessState::Unknown;
        }
        #[cfg(windows)]
        if self.start_identity.starts_with('0')
            || self.start_identity.len() > 20
            || !self
                .start_identity
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            return ProcessState::Unknown;
        }
        match Self::read(self.pid) {
            Ok(Some(current)) if current == *self => ProcessState::Live,
            Ok(_) => ProcessState::Stale,
            Err(_) => ProcessState::Unknown,
        }
    }
}

/// Recognize legacy PID-only records kept by the TypeScript box. Never signal
/// the process and never infer a match from a PID alone where it can be reused.
pub fn legacy_process_runs(pid: u32, name: &str) -> bool {
    if pid <= 1 || matches!(ProcessIdentity::read(pid), Ok(None)) {
        return false;
    }
    legacy_command_contains(pid, name)
}

#[cfg(target_os = "linux")]
fn legacy_command_contains(pid: u32, name: &str) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .is_ok_and(|line| String::from_utf8_lossy(&line).contains(name))
}

#[cfg(target_os = "macos")]
fn legacy_command_contains(pid: u32, name: &str) -> bool {
    if std::env::var("KUVO_BOX").as_deref() == Ok("mac")
        || (std::env::var_os("KUVO_AGENTS_HOME").is_some()
            && std::env::var_os("KUVO_AGENTS_REMOTE").is_none())
    {
        return true;
    }
    std::process::Command::new("/bin/ps")
        .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .map_or(true, |output| {
            !output.status.success() || String::from_utf8_lossy(&output.stdout).contains(name)
        })
}

#[cfg(windows)]
fn legacy_command_contains(_pid: u32, _name: &str) -> bool {
    // The TypeScript box never matched old PID-only records on Windows.
    false
}

/// Windows has no process groups: a command's work is its process tree. Each running
/// process's parent as Windows recorded it; that parent may since have exited and its PID
/// been reused, so compare start times before trusting a link.
#[cfg(windows)]
pub fn process_parents() -> io::Result<std::collections::HashMap<u32, u32>> {
    sys::parents()
}

/// When a running process started (Windows file time), none once it's gone.
#[cfg(windows)]
pub fn process_started(pid: u32) -> Option<u64> {
    sys::read_start(pid).ok().flatten()?.parse().ok()
}

/// Each listening TCP socket's owner and port, IPv4 and IPv6.
#[cfg(windows)]
pub fn tcp_listeners() -> io::Result<Vec<(u32, u16)>> {
    sys::tcp_listeners()
}

#[derive(Default, Debug)]
pub enum Stdio {
    #[default]
    Null,
    Pipe,
    Inherit,
    File(std::fs::File),
}

pub struct Spawn {
    pub executable: Executable,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Environment,
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
}

impl Spawn {
    pub fn new(program: impl Into<PathBuf>, env: Environment) -> Self {
        Self {
            executable: Executable::Native(program.into()),
            args: Vec::new(),
            cwd: None,
            env,
            stdin: Stdio::Null,
            stdout: Stdio::Null,
            stderr: Stdio::Null,
        }
    }

    pub fn spawn(&self) -> io::Result<ChildGroup> {
        self.launch(None, false)
    }

    /// Supervise this resident while leaving nested workers to their own owners.
    /// On Windows the job is kill-on-close, with silent breakaway for descendants.
    pub fn spawn_resident(&self, job_name: &str) -> io::Result<ChildGroup> {
        self.launch_named(job_name, true)
    }

    /// Start a worker that can be handed over to the next resident after it is recorded.
    /// The unique recovery name identifies the process tree on Windows.
    pub fn spawn_recoverable(&self, recovery_name: &str) -> io::Result<ChildGroup> {
        self.launch_named(recovery_name, false)
    }

    fn launch_named(&self, recovery_name: &str, resident: bool) -> io::Result<ChildGroup> {
        if recovery_name.is_empty()
            || !recovery_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid recovery name",
            ));
        }
        self.launch(Some(recovery_name), resident)
    }

    fn launch(&self, recovery_name: Option<&str>, resident: bool) -> io::Result<ChildGroup> {
        let valid = |value: &OsStr| !value.as_encoded_bytes().contains(&0);
        if !self.executable.path().is_absolute()
            || self.cwd.as_deref().is_some_and(|path| !path.is_absolute())
            || self.args.iter().any(|arg| !valid(arg))
            || self.env.iter().any(|(name, value)| {
                name.is_empty()
                    || !valid(name)
                    || name.as_encoded_bytes().contains(&b'=')
                    || !valid(value)
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "process requires an absolute executable and valid arguments, directory and environment",
            ));
        }
        sys::spawn(self, recovery_name, resident)
    }
}

pub struct ChildGroup {
    owner: sys::Owner,
    identity: ProcessIdentity,
    pub stdin: Option<PipeWriter>,
    pub stdout: Option<PipeReader>,
    pub stderr: Option<PipeReader>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupSignal {
    Hangup,
    Interrupt,
    Quit,
    Kill,
    Terminate,
    User1,
    User2,
}

impl GroupSignal {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "SIGHUP" => Self::Hangup,
            "SIGINT" => Self::Interrupt,
            "SIGQUIT" => Self::Quit,
            "SIGKILL" => Self::Kill,
            "SIGTERM" => Self::Terminate,
            "SIGUSR1" => Self::User1,
            "SIGUSR2" => Self::User2,
            _ => return None,
        })
    }

    pub fn name_from_exit(status: ExitStatus) -> Option<String> {
        sys::signal_from_exit(status)
    }
}

impl ChildGroup {
    pub fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    /// Keep the worker alive when the resident exits. Call only after publishing
    /// the recovery record. Explicit stop and wait still own the process tree.
    pub fn leave_running_on_drop(&mut self) {
        self.owner.leave_running_on_drop();
    }

    /// Request application-level shutdown before waiting. Unix also sends SIGTERM to the group.
    /// On Windows, a headless process should exit through its own protocol or stdin EOF.
    /// Call on a blocking worker in an async application.
    pub fn stop(
        &mut self,
        grace: Duration,
        request: impl FnOnce(&mut Self),
    ) -> io::Result<ExitStatus> {
        if let Some(status) = self.owner.status() {
            return Ok(status);
        }
        request(self);
        self.stdin.take();
        self.owner.stop(grace)
    }

    /// Wait for the leader's exit and clean up remaining group members. This call blocks.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.stdin.take();
        self.owner.wait()
    }

    /// True once the leader has exited, without giving up ownership of its group.
    pub fn has_exited(&self) -> io::Result<bool> {
        self.owner.has_exited()
    }

    pub fn signal(&self, signal: GroupSignal) -> io::Result<()> {
        self.owner.send_signal(signal)
    }
}

pub(crate) fn invalid_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("not an executable path: {}", path.display()),
    )
}

/// A worker started by an earlier resident. Its executable is checked before
/// opening it and its start identity guards against recycled process IDs.
pub struct AdoptedProcess(sys::Adopted);

impl AdoptedProcess {
    pub fn open(
        pid: u32,
        expected_start: Option<&str>,
        matches_executable: impl FnOnce(&Path) -> bool,
        recovery_name: Option<&str>,
    ) -> io::Result<Option<Self>> {
        let Some(identity) = ProcessIdentity::read(pid)? else {
            return Ok(None);
        };
        if expected_start.is_some_and(|start| start != identity.start_identity) {
            return Ok(None);
        }
        let executable = match sys::executable(pid) {
            Ok(path) => path,
            Err(_) if identity.state() == ProcessState::Stale => return Ok(None),
            Err(error) => return Err(error),
        };
        if !matches_executable(&executable) || identity.state() != ProcessState::Live {
            return Ok(None);
        }
        sys::Adopted::open(identity, recovery_name).map(|worker| worker.map(Self))
    }

    pub fn identity(&self) -> &ProcessIdentity {
        self.0.identity()
    }

    /// On Windows, retain the job before asking the previous supervisor to leave.
    /// An open process handle alone does not keep the job alive.
    pub fn open_resident(
        identity: &ProcessIdentity,
        matches_executable: impl FnOnce(&Path) -> bool,
        job_name: &str,
    ) -> io::Result<Option<Self>> {
        let resident = Self::open(
            identity.pid,
            Some(&identity.start_identity),
            matches_executable,
            Some(job_name),
        )?;
        #[cfg(windows)]
        let resident = resident.filter(|resident| resident.0.has_job());
        Ok(resident)
    }

    pub fn stop(&mut self, grace: Duration) -> io::Result<()> {
        self.0.stop(grace)
    }

    /// Ask it to stop (SIGTERM), without waiting for it to.
    #[cfg(unix)]
    pub fn terminate(&self) -> io::Result<()> {
        self.0.terminate()
    }
}
