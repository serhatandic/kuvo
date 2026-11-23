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

#[derive(Default, Clone, Copy, Debug)]
pub enum Stdio {
    #[default]
    Null,
    Pipe,
    Inherit,
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
        sys::spawn(self)
    }
}

pub struct ChildGroup {
    owner: sys::Owner,
    identity: ProcessIdentity,
    pub stdin: Option<PipeWriter>,
    pub stdout: Option<PipeReader>,
    pub stderr: Option<PipeReader>,
}

impl ChildGroup {
    pub fn identity(&self) -> &ProcessIdentity {
        &self.identity
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
}

pub(crate) fn invalid_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("not an executable path: {}", path.display()),
    )
}
