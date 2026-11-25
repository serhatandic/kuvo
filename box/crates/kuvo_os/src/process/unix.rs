use super::*;
use rustix::process::{Pid, Signal, kill_process_group};
use std::{
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio as ChildStdio},
    thread,
    time::Instant,
};

pub(super) struct Owner {
    child: Child,
    status: Option<ExitStatus>,
    leave_running: bool,
}

impl Owner {
    pub(super) fn send_signal(&self, signal: GroupSignal) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        self.signal(match signal {
            GroupSignal::Hangup => Signal::HUP,
            GroupSignal::Interrupt => Signal::INT,
            GroupSignal::Quit => Signal::QUIT,
            GroupSignal::Kill => Signal::KILL,
            GroupSignal::Terminate => Signal::TERM,
            GroupSignal::User1 => Signal::USR1,
            GroupSignal::User2 => Signal::USR2,
        })
    }
    pub(super) fn leave_running_on_drop(&mut self) {
        self.leave_running = true;
    }

    pub(super) fn status(&self) -> Option<ExitStatus> {
        self.status
    }

    fn signal(&self, signal: Signal) -> io::Result<()> {
        // The leader is still our unreaped child, so its PID (also the PGID) cannot be recycled.
        let pid = Pid::from_raw(self.child.id() as i32).expect("positive child PID");
        match kill_process_group(pid, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            // Darwin can reject a group signal while its last process is exiting, before
            // waitid observes the exit. A single-PID signal still works on our pinned leader.
            #[cfg(target_os = "macos")]
            Err(rustix::io::Errno::PERM) if !group_has_live_members(self.child.id())? => {
                match rustix::process::kill_process(pid, signal) {
                    Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                    Err(error) => Err(error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn has_exited(&self) -> io::Result<bool> {
        if self.status.is_some() {
            return Ok(true);
        }
        self.observe_exit(libc::WEXITED | libc::WNOHANG | libc::WNOWAIT)
    }

    fn observe_exit(&self, options: libc::c_int) -> io::Result<bool> {
        // SAFETY: zeroed siginfo is a valid writable output buffer for waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: PID is our child and info is valid for the duration of the call.
            let result = unsafe { libc::waitid(libc::P_PID, self.child.id(), &mut info, options) };
            if result == 0 {
                // SAFETY: successful waitid initializes the child status, or leaves it zeroed.
                return Ok(unsafe { info.si_pid() } != 0);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    pub(super) fn stop(&mut self, grace: Duration) -> io::Result<ExitStatus> {
        self.has_exited()?;
        self.signal(Signal::TERM)?;
        let start = Instant::now();
        loop {
            if self.has_exited()? && !group_has_live_members(self.child.id())? {
                break;
            }
            let left = grace.saturating_sub(start.elapsed());
            if left.is_zero() {
                break;
            }
            thread::sleep(left.min(Duration::from_millis(10)));
        }
        self.finish()
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        self.observe_exit(libc::WEXITED | libc::WNOWAIT)?;
        self.finish()
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        // If another component unexpectedly reaped the leader, there is no longer a safe PGID.
        self.has_exited()?;
        self.signal(Signal::KILL)?;
        let status = self.child.wait()?;
        self.status = Some(status);
        Ok(status)
    }
}

pub(super) fn signal_from_exit(status: ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    let signal = status.signal()?;
    Some(
        match signal {
            libc::SIGHUP => "SIGHUP",
            libc::SIGINT => "SIGINT",
            libc::SIGQUIT => "SIGQUIT",
            libc::SIGILL => "SIGILL",
            libc::SIGTRAP => "SIGTRAP",
            libc::SIGABRT => "SIGABRT",
            libc::SIGBUS => "SIGBUS",
            libc::SIGFPE => "SIGFPE",
            libc::SIGKILL => "SIGKILL",
            libc::SIGUSR1 => "SIGUSR1",
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGUSR2 => "SIGUSR2",
            libc::SIGPIPE => "SIGPIPE",
            libc::SIGALRM => "SIGALRM",
            libc::SIGTERM => "SIGTERM",
            _ => return None,
        }
        .into(),
    )
}

impl Drop for Owner {
    fn drop(&mut self) {
        if !self.leave_running && self.status.is_none() {
            let _ = self.finish();
        }
    }
}

pub(super) fn spawn(
    spec: &Spawn,
    _recovery_name: Option<&str>,
    _resident: bool,
) -> io::Result<ChildGroup> {
    if !matches!(spec.executable, Executable::Native(_)) {
        return Err(invalid_path(spec.executable.path()));
    }
    let mut command = Command::new(spec.executable.path());
    command.args(&spec.args).env_clear().envs(&spec.env);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let (stdin, child_stdin) = match &spec.stdin {
        Stdio::Pipe => {
            let (read, write) = io::pipe()?;
            (Some(write), read.into())
        }
        Stdio::Null => (None, ChildStdio::null()),
        Stdio::Inherit => (None, ChildStdio::inherit()),
        Stdio::File(file) => (None, file.try_clone()?.into()),
    };
    let (stdout, child_stdout) = output(&spec.stdout)?;
    let (stderr, child_stderr) = output(&spec.stderr)?;
    command
        .stdin(child_stdin)
        .stdout(child_stdout)
        .stderr(child_stderr);
    // SAFETY: setsid is async-signal-safe. It gives the child a new session and process group.
    unsafe {
        command.pre_exec(|| rustix::process::setsid().map(|_| ()).map_err(Into::into));
    }
    let owner = Owner {
        child: command.spawn()?,
        status: None,
        leave_running: false,
    };
    let pid = owner.child.id();
    // The process may have already exited; its unreaped record still holds the start stamp.
    let identity = ProcessIdentity {
        pid,
        start_identity: start_record(pid)?
            .map(|(start, _)| start)
            .ok_or_else(|| io::Error::other("child start identity is unavailable"))?,
    };
    Ok(ChildGroup {
        owner,
        identity,
        stdin,
        stdout,
        stderr,
    })
}

fn output(mode: &Stdio) -> io::Result<(Option<PipeReader>, ChildStdio)> {
    Ok(match mode {
        Stdio::Pipe => {
            let (read, write) = io::pipe()?;
            (Some(read), write.into())
        }
        Stdio::Null => (None, ChildStdio::null()),
        Stdio::Inherit => (None, ChildStdio::inherit()),
        Stdio::File(file) => (None, file.try_clone()?.into()),
    })
}

pub(super) fn read_start(pid: u32) -> io::Result<Option<String>> {
    Ok(start_record(pid)?.and_then(|(start, live)| live.then_some(start)))
}

#[cfg(target_os = "linux")]
fn group_has_live_members(group: u32) -> io::Result<bool> {
    for process in std::fs::read_dir("/proc")? {
        let process = process?;
        let name = process.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        if pid == group || pid > i32::MAX as u32 {
            continue;
        }
        // SAFETY: getpgid only queries a PID. Kernel threads can return group zero.
        let candidate_group = unsafe { libc::getpgid(pid as i32) };
        if candidate_group == group as i32 {
            match linux_stat(pid) {
                Ok(Some(stat)) if stat.group == group && stat.live => return Ok(true),
                Err(error) if error.kind() != io::ErrorKind::PermissionDenied => return Err(error),
                _ => {}
            }
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn group_has_live_members(group: u32) -> io::Result<bool> {
    let mut members = vec![0_i32; 32];
    loop {
        // SAFETY: members is writable for its advertised length, and the group leader is pinned.
        let count = unsafe {
            libc::proc_listpgrppids(
                group as i32,
                members.as_mut_ptr().cast(),
                size_of_val(members.as_slice()) as i32,
            )
        };
        if count <= 0 {
            return Ok(false);
        }
        let count = count as usize;
        for &pid in &members[..count.min(members.len())] {
            if pid > 0 && pid as u32 != group && !matches!(read_start(pid as u32), Ok(None)) {
                return Ok(true);
            }
        }
        if count < members.len() {
            return Ok(false);
        }
        members.resize(members.len() * 2, 0);
    }
}

#[cfg(target_os = "linux")]
struct LinuxStat {
    group: u32,
    start: u64,
    live: bool,
}

#[cfg(target_os = "linux")]
fn linux_stat(pid: u32) -> io::Result<Option<LinuxStat>> {
    let stat = match std::fs::read(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ESRCH) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let end = stat
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or_else(|| io::Error::other("invalid process stat"))?;
    let mut fields = stat[end + 1..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty());
    let state = fields
        .next()
        .ok_or_else(|| io::Error::other("missing process state"))?;
    let group = fields
        .nth(1)
        .and_then(|field| std::str::from_utf8(field).ok())
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| io::Error::other("missing process group ID"))?;
    let start = fields
        .nth(16)
        .and_then(|field| std::str::from_utf8(field).ok())
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| io::Error::other("missing process start time"))?;
    Ok(Some(LinuxStat {
        group,
        start,
        live: !matches!(state, b"Z" | b"X" | b"x"),
    }))
}

#[cfg(target_os = "linux")]
fn start_record(pid: u32) -> io::Result<Option<(String, bool)>> {
    let Some(stat) = linux_stat(pid)? else {
        return Ok(None);
    };
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(Some((format!("{}:{}", boot.trim(), stat.start), stat.live)))
}

#[cfg(target_os = "macos")]
fn start_record(pid: u32) -> io::Result<Option<(String, bool)>> {
    use std::mem::{MaybeUninit, size_of};
    let pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid process ID"))?;
    let mut record = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the output buffer has the advertised length and type for PROC_PIDTBSDINFO.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            record.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        // SAFETY: signal 0 only checks existence.
        let exists = unsafe { libc::kill(pid, 0) };
        return if exists == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    // SAFETY: proc_pidinfo returned the full size of the initialized structure.
    let record = unsafe { record.assume_init() };
    Ok(Some((
        format!("{}:{:06}", record.pbi_start_tvsec, record.pbi_start_tvusec),
        record.pbi_status != libc::SZOMB,
    )))
}

#[cfg(target_os = "linux")]
pub(super) fn executable(pid: u32) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
}

#[cfg(target_os = "macos")]
pub(super) fn executable(pid: u32) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    let mut path = [0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the output buffer is writable for the advertised length.
    let read = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if read <= 0 {
        return Err(io::Error::last_os_error());
    }
    let end = path
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(read as usize);
    Ok(PathBuf::from(OsStr::from_bytes(&path[..end])))
}

pub(super) struct Adopted {
    identity: ProcessIdentity,
    #[cfg(target_os = "linux")]
    pidfd: Option<std::os::fd::OwnedFd>,
}

impl Adopted {
    pub(super) fn open(
        identity: ProcessIdentity,
        _recovery_name: Option<&str>,
    ) -> io::Result<Option<Self>> {
        #[cfg(target_os = "linux")]
        let pidfd = match rustix::process::pidfd_open(
            Pid::from_raw(identity.pid as i32).expect("validated PID"),
            rustix::process::PidfdFlags::empty(),
        ) {
            Ok(fd) => Some(fd),
            Err(rustix::io::Errno::NOSYS) => None,
            Err(rustix::io::Errno::SRCH) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok((identity.state() == ProcessState::Live).then_some(Self {
            identity,
            #[cfg(target_os = "linux")]
            pidfd,
        }))
    }

    pub(super) fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    fn signal(&self, signal: Signal) -> io::Result<()> {
        match self.identity.state() {
            ProcessState::Stale => return Ok(()),
            ProcessState::Unknown => return Err(io::Error::other("engine identity unavailable")),
            ProcessState::Live => {}
        }
        let pid = Pid::from_raw(self.identity.pid as i32).expect("validated PID");
        #[cfg(target_os = "linux")]
        let result = match &self.pidfd {
            Some(fd) => rustix::process::pidfd_send_signal(fd, signal),
            None => rustix::process::kill_process(pid, signal),
        };
        #[cfg(target_os = "macos")]
        let result = rustix::process::kill_process(pid, signal);
        match result {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn wait(&self, duration: Duration) -> bool {
        let until = Instant::now() + duration;
        loop {
            if self.identity.state() == ProcessState::Stale {
                return true;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            thread::sleep(left.min(Duration::from_millis(25)));
        }
    }

    pub(super) fn terminate(&self) -> io::Result<()> {
        self.signal(Signal::TERM)
    }

    pub(super) fn stop(&mut self, grace: Duration) -> io::Result<()> {
        self.signal(Signal::TERM)?;
        if !self.wait(grace) {
            self.signal(Signal::KILL)?;
            if !self.wait(Duration::from_secs(5)) {
                return Err(io::Error::other("the adopted worker did not exit"));
            }
        }
        Ok(())
    }
}
