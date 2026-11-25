use super::*;
use std::{
    fs::OpenOptions,
    mem::{MaybeUninit, size_of},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    ptr::{self, null, null_mut},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    },
    System::{
        Console::*,
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        JobObjects::*,
        SystemInformation::GetSystemDirectoryW,
        SystemServices::{JOB_OBJECT_QUERY, JOB_OBJECT_TERMINATE},
        Threading::*,
    },
};

pub(super) struct Owner {
    process: OwnedHandle,
    job: OwnedHandle,
    status: Option<ExitStatus>,
    recoverable: bool,
    leave_running: bool,
}

impl Owner {
    pub(super) fn send_signal(&self, signal: GroupSignal) -> io::Result<()> {
        match signal {
            GroupSignal::Kill | GroupSignal::Terminate => {
                // SAFETY: the open job contains only this worker's process tree.
                ok(unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unix process signal",
            )),
        }
    }
    pub(super) fn leave_running_on_drop(&mut self) {
        self.leave_running = true;
    }

    pub(super) fn status(&self) -> Option<ExitStatus> {
        self.status
    }

    pub(super) fn has_exited(&self) -> io::Result<bool> {
        self.wait_for(0)
    }

    fn wait_for(&self, millis: u32) -> io::Result<bool> {
        // SAFETY: the open process handle has synchronization access.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), millis) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub(super) fn stop(&mut self, grace: Duration) -> io::Result<ExitStatus> {
        let start = Instant::now();
        let mut active = self.active_processes()?;
        while active != 0 {
            let left = grace.saturating_sub(start.elapsed());
            if left.is_zero() {
                // SAFETY: the job contains only the owned process tree.
                ok(unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) })?;
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(10)));
            active = self.active_processes()?;
        }
        self.reap()
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        self.reap()?;
        // SAFETY: the job contains only the owned process tree.
        ok(unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) })?;
        Ok(self.status.expect("leader was reaped"))
    }

    fn active_processes(&self) -> io::Result<u32> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: the buffer matches the requested job accounting information class.
        ok(unsafe {
            QueryInformationJobObject(
                self.job.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                ptr::from_mut(&mut accounting).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        })?;
        Ok(accounting.ActiveProcesses)
    }

    fn reap(&mut self) -> io::Result<ExitStatus> {
        self.wait_for(INFINITE)?;
        let mut code = 0;
        // SAFETY: the process handle is open and code is writable.
        ok(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
        let status = ExitStatus::from_raw(code);
        self.status = Some(status);
        Ok(status)
    }
}

pub(super) fn signal_from_exit(_: ExitStatus) -> Option<String> {
    None
}

impl Drop for Owner {
    fn drop(&mut self) {
        if self.recoverable && !self.leave_running && self.status.is_none() {
            // SAFETY: the job contains only the worker created for this owner.
            unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) };
        }
    }
}

fn ok(result: i32) -> io::Result<()> {
    if result != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: callers pass a successful Win32 handle creation or duplication result.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

fn environment_block(env: &Environment) -> Vec<u16> {
    let mut sorted = std::collections::BTreeMap::new();
    for (key, value) in env {
        sorted.insert(key.to_string_lossy().to_uppercase(), (key, value));
    }
    let mut block = Vec::new();
    for (key, value) in sorted.into_values() {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(wide(value));
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn start_time(process: HANDLE) -> io::Result<String> {
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: process remains open and each output points to a separate FILETIME.
    ok(unsafe { GetProcessTimes(process, created, exited, kernel, user) })?;
    Ok(((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)).to_string())
}

pub(super) fn read_start(pid: u32) -> io::Result<Option<String>> {
    // SAFETY: OpenProcess validates the PID and requested access.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let process = owned(handle)?;
    let start = start_time(process.as_raw_handle())?;
    // SAFETY: process is a live handle with synchronization access.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_TIMEOUT => Ok(Some(start)),
        WAIT_OBJECT_0 => Ok(None),
        _ => Err(io::Error::last_os_error()),
    }
}

struct Attributes {
    // Pointer-sized storage gives the opaque structure the required alignment.
    _storage: Vec<MaybeUninit<usize>>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl Attributes {
    fn new() -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: a null buffer requests the required size for two attributes.
        unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size) };
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![MaybeUninit::<usize>::uninit(); size.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        // SAFETY: the list points into writable aligned storage of at least size bytes.
        ok(unsafe { InitializeProcThreadAttributeList(list, 2, 0, &mut size) })?;
        Ok(Self {
            _storage: storage,
            list,
        })
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the list was initialized and its backing storage remains alive.
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

pub(super) fn spawn(
    spec: &Spawn,
    recovery_name: Option<&str>,
    resident: bool,
) -> io::Result<ChildGroup> {
    let (program, mut line) = command_line(spec)?;
    let cwd = spec.cwd.as_deref().map(|path| wide(path.as_os_str()));
    let env = environment_block(&spec.env);

    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    if recovery_name.is_none() || resident {
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    }
    if resident {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
    }
    let name = recovery_name.map(|name| wide(OsStr::new(name)));
    // SAFETY: the name, if present, is NUL-terminated. The handle is non-inheritable.
    let job = owned(unsafe {
        CreateJobObjectW(null(), name.as_ref().map_or(null(), |name| name.as_ptr()))
    })?;
    // SAFETY: GetLastError reads the thread-local result from CreateJobObjectW.
    if name.is_some() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "worker job already exists",
        ));
    }
    // SAFETY: the information class matches the initialized structure and its length.
    ok(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            ptr::from_ref(&limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;

    let (stdin, input) = match &spec.stdin {
        Stdio::Pipe => {
            let (read, write) = io::pipe()?;
            (Some(write), duplicate(read.as_raw_handle())?)
        }
        mode => (None, inherit_or_null(mode, STD_INPUT_HANDLE, true)?),
    };
    let (stdout, stdout_handle) = output(&spec.stdout, STD_OUTPUT_HANDLE)?;
    let (stderr, error) = output(&spec.stderr, STD_ERROR_HANDLE)?;
    let job_list = [job.as_raw_handle()];
    let handles = [
        input.as_raw_handle(),
        stdout_handle.as_raw_handle(),
        error.as_raw_handle(),
    ];

    let attributes = Attributes::new()?;
    // SAFETY: both handle arrays remain alive through attribute deletion. The job is assigned
    // as part of process creation, and exactly these three standard handles may be inherited.
    unsafe {
        ok(UpdateProcThreadAttribute(
            attributes.list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            job_list.as_ptr().cast(),
            size_of::<[HANDLE; 1]>(),
            null_mut(),
            null(),
        ))?;
        ok(UpdateProcThreadAttribute(
            attributes.list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            size_of::<[HANDLE; 3]>(),
            null_mut(),
            null(),
        ))?;
    }

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.list;
    let mut result = PROCESS_INFORMATION::default();
    // SAFETY: all buffers below are NUL-terminated (the environment has two terminators), the
    // writable command line and result are exclusive, and startup carries the live attributes.
    ok(unsafe {
        CreateProcessW(
            program.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            env.as_ptr().cast(),
            cwd.as_ref().map_or(null(), |path| path.as_ptr()),
            &startup.StartupInfo,
            &mut result,
        )
    })?;
    let process = owned(result.hProcess)?;
    drop(owned(result.hThread)?);
    let identity = ProcessIdentity {
        pid: result.dwProcessId,
        start_identity: start_time(process.as_raw_handle())?,
    };
    Ok(ChildGroup {
        owner: Owner {
            process,
            job,
            status: None,
            recoverable: recovery_name.is_some() && !resident,
            leave_running: false,
        },
        identity,
        stdin,
        stdout,
        stderr,
    })
}

fn duplicate(source: HANDLE) -> io::Result<OwnedHandle> {
    let mut child = null_mut();
    // SAFETY: source is borrowed in this process and the output receives a separate owned handle.
    unsafe {
        let current = GetCurrentProcess();
        ok(DuplicateHandle(
            current,
            source,
            current,
            &mut child,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        ))?;
    }
    owned(child)
}

fn inherit_or_null(mode: &Stdio, which: STD_HANDLE, read: bool) -> io::Result<OwnedHandle> {
    if let Stdio::File(file) = mode {
        return duplicate(file.as_raw_handle());
    }
    if matches!(mode, Stdio::Inherit) {
        // SAFETY: the selector is one of the standard handle constants.
        let handle = unsafe { GetStdHandle(which) };
        if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
            return duplicate(handle);
        }
    }
    let file = OpenOptions::new().read(read).write(!read).open("NUL")?;
    duplicate(file.as_raw_handle())
}

fn output(mode: &Stdio, which: STD_HANDLE) -> io::Result<(Option<PipeReader>, OwnedHandle)> {
    Ok(match mode {
        Stdio::Pipe => {
            let (read, write) = io::pipe()?;
            (Some(read), duplicate(write.as_raw_handle())?)
        }
        mode => (None, inherit_or_null(mode, which, false)?),
    })
}

fn command_line(spec: &Spawn) -> io::Result<(Vec<u16>, Vec<u16>)> {
    let path = spec.executable.path();
    if !path.is_file() {
        return Err(invalid_path(path));
    }
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match (&spec.executable, extension.as_str()) {
        (Executable::Native(_), "exe" | "com") => {
            let mut line = Vec::new();
            append_arg(&mut line, path.as_os_str());
            for arg in &spec.args {
                line.push(b' ' as u16);
                append_arg(&mut line, arg);
            }
            line.push(0);
            Ok((wide(path.as_os_str()), line))
        }
        (Executable::CmdScript(_), "bat" | "cmd") => batch_line(path, &spec.args),
        _ => Err(invalid_path(path)),
    }
}

fn batch_line(script: &Path, args: &[OsString]) -> io::Result<(Vec<u16>, Vec<u16>)> {
    let mut system = [0; 260];
    // SAFETY: system is a writable UTF-16 buffer of the advertised size.
    let size = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
    if size == 0 || size >= system.len() {
        return Err(io::Error::last_os_error());
    }
    let program = PathBuf::from(OsString::from_wide(&system[..size])).join("cmd.exe");
    let mut line: Vec<u16> = "cmd.exe /d /v:off /s /c \"".encode_utf16().collect();
    for (i, arg) in std::iter::once(script.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .enumerate()
    {
        if arg
            .as_encoded_bytes()
            .iter()
            .any(|byte| matches!(byte, b'%' | b'"' | b'\r' | b'\n'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "batch arguments cannot contain quotes, percent signs or line breaks",
            ));
        }
        if i != 0 {
            line.push(b' ' as u16);
        }
        append_arg(&mut line, arg);
    }
    line.extend([b'"' as u16, 0]);
    Ok((wide(program.as_os_str()), line))
}

/// Quote for the Windows CRT argv parser (including runs of backslashes before quotes).
fn append_arg(line: &mut Vec<u16>, arg: &OsStr) {
    const QUOTE: u16 = b'"' as u16;
    const SLASH: u16 = b'\\' as u16;
    line.push(QUOTE);
    let mut slashes = 0;
    for value in arg.encode_wide() {
        match value {
            SLASH => slashes += 1,
            _ => {
                line.extend(std::iter::repeat_n(
                    SLASH,
                    if value == QUOTE {
                        slashes * 2 + 1
                    } else {
                        slashes
                    },
                ));
                slashes = 0;
                line.push(value);
            }
        }
    }
    line.extend(std::iter::repeat_n(SLASH, slashes * 2));
    line.push(QUOTE);
}

pub(super) fn executable(pid: u32) -> io::Result<PathBuf> {
    // SAFETY: OpenProcess validates the PID.
    let process = owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    let mut path = vec![0; 32_768];
    let mut length = path.len() as u32;
    // SAFETY: the process handle and the writable UTF-16 buffer are valid.
    ok(unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut length)
    })?;
    Ok(OsString::from_wide(&path[..length as usize]).into())
}

pub(super) struct Adopted {
    identity: ProcessIdentity,
    process: OwnedHandle,
    job: Option<OwnedHandle>,
}

impl Adopted {
    pub(super) fn has_job(&self) -> bool {
        self.job.is_some()
    }

    pub(super) fn open(
        identity: ProcessIdentity,
        recovery_name: Option<&str>,
    ) -> io::Result<Option<Self>> {
        // SAFETY: the system validates the PID and returns a stable process handle.
        let process = owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                0,
                identity.pid,
            )
        })?;
        if start_time(process.as_raw_handle())? != identity.start_identity {
            return Ok(None);
        }
        let job = if let Some(name) = recovery_name {
            let name = wide(OsStr::new(name));
            // SAFETY: name is NUL-terminated and the returned handle is non-inheritable.
            let job = owned(unsafe {
                OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, name.as_ptr())
            });
            match job {
                Ok(job) => {
                    let mut member = 0;
                    // SAFETY: both handles are open and member is writable.
                    ok(unsafe {
                        IsProcessInJob(process.as_raw_handle(), job.as_raw_handle(), &mut member)
                    })?;
                    if member == 0 {
                        return Err(io::Error::other(
                            "worker does not belong to the recorded job",
                        ));
                    }
                    Some(job)
                }
                Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        Ok(Some(Self {
            identity,
            process,
            job,
        }))
    }

    pub(super) fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    pub(super) fn stop(&mut self, _grace: Duration) -> io::Result<()> {
        let process = self.process.as_raw_handle();
        // SAFETY: the handle pins the adopted process and has synchronization access.
        match unsafe { WaitForSingleObject(process, 0) } {
            WAIT_OBJECT_0 => return Ok(()),
            WAIT_TIMEOUT => {}
            _ => return Err(io::Error::last_os_error()),
        }
        if let Some(job) = &self.job {
            // SAFETY: membership of the adopted process was verified at open.
            ok(unsafe { TerminateJobObject(job.as_raw_handle(), 1) })?;
        } else {
            // SAFETY: the pinned handle has termination rights and names the verified process.
            ok(unsafe { TerminateProcess(process, 1) })?;
        }
        // SAFETY: the process remains pinned and the wait is bounded.
        match unsafe { WaitForSingleObject(process, 5000) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(io::Error::other("the adopted worker did not exit")),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

pub(super) fn parents() -> io::Result<std::collections::HashMap<u32, u32>> {
    // SAFETY: a process snapshot takes no buffers.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut parents = std::collections::HashMap::new();
    // SAFETY: the snapshot is open and the entry states its own size.
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Ok(parents)
}

pub(super) fn tcp_listeners() -> io::Result<Vec<(u32, u16)>> {
    // The port is in network byte order in the low 16 bits.
    let port = |field: u32| u16::from_be(field as u16);
    let mut found = listeners(2, |row: &MIB_TCPROW_OWNER_PID| {
        (row.dwOwningPid, port(row.dwLocalPort))
    })?;
    found.extend(listeners(23, |row: &MIB_TCP6ROW_OWNER_PID| {
        (row.dwOwningPid, port(row.dwLocalPort))
    })?);
    Ok(found)
}

/// One address family's listening table: a count, then its rows.
fn listeners<Row>(family: u32, read: impl Fn(&Row) -> (u32, u16)) -> io::Result<Vec<(u32, u16)>> {
    let mut buffer: Vec<u64> = Vec::new();
    let mut size = 0;
    loop {
        // SAFETY: the buffer is writable for `size` bytes, as the last call asked.
        let status = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match status {
            NO_ERROR => break,
            ERROR_INSUFFICIENT_BUFFER => buffer.resize((size as usize).div_ceil(8), 0),
            error => return Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
    let Some(&first) = buffer.first() else {
        return Ok(Vec::new());
    };
    let count = (first as u32) as usize;
    let offset = size_of::<u32>().next_multiple_of(align_of::<Row>());
    if offset + count * size_of::<Row>() > buffer.len() * 8 {
        return Err(io::Error::other("a short TCP table"));
    }
    // SAFETY: the table holds `count` rows after its count, within the checked buffer.
    let rows = unsafe {
        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>().add(offset).cast::<Row>(), count)
    };
    Ok(rows.iter().map(read).collect())
}
