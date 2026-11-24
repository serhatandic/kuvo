//! Account and path spelling. Filesystem authorization must resolve the actual object separately.

use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

pub type Environment = BTreeMap<OsString, OsString>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Mac,
    Linux,
    Wsl,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Self::Mac;
        #[cfg(windows)]
        return Self::Windows;
        #[cfg(target_os = "linux")]
        Self::linux(&std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default())
    }

    pub fn linux(release: &str) -> Self {
        let release = release.to_ascii_lowercase();
        if release.contains("microsoft") || release.contains("wsl") {
            Self::Wsl
        } else {
            Self::Linux
        }
    }

    pub fn is_windows_host(self) -> bool {
        matches!(self, Self::Windows | Self::Wsl)
    }

    pub fn prompted_folders(self) -> &'static [&'static str] {
        match self {
            Self::Mac => &[
                "Desktop",
                "Documents",
                "Downloads",
                "Library",
                "Movies",
                "Music",
                "Pictures",
            ],
            _ => &[],
        }
    }
}

pub fn env_value<'a>(platform: Platform, env: &'a Environment, key: &str) -> Option<&'a OsStr> {
    env.get(OsStr::new(key))
        .or_else(|| {
            (platform == Platform::Windows)
                .then(|| env.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)))
                .flatten()
                .map(|(_, value)| value)
        })
        .map(OsString::as_os_str)
}

#[derive(Debug, Eq, PartialEq)]
pub struct Account {
    pub name: OsString,
    pub home: PathBuf,
    pub shell: Option<PathBuf>,
}

impl Account {
    /// The system account, independent of HOME and USER on Unix.
    pub fn current() -> io::Result<Self> {
        system_account()
    }

    pub fn home_folder(&self, platform: Platform, env: &Environment) -> io::Result<PathBuf> {
        let value = env_value(
            platform,
            env,
            if platform == Platform::Windows {
                "USERPROFILE"
            } else {
                "HOME"
            },
        );
        let home = value
            .filter(|v| !v.is_empty())
            .unwrap_or(self.home.as_os_str());
        if platform == Platform::Windows {
            let wire = home
                .to_str()
                .and_then(windows_wire_path)
                .filter(|path| path.as_bytes()[1] == b':')
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Kuvo needs a local Windows user profile",
                    )
                })?;
            return Ok(PathBuf::from(wire.replace('/', "\\")));
        }
        Ok(PathBuf::from(home))
    }

    pub fn login_shell(&self, platform: Platform, env: &Environment) -> Option<PathBuf> {
        let given = env_value(platform, env, "SHELL")
            .map(Path::new)
            .filter(|path| path.is_absolute());
        match platform {
            Platform::Mac => Some(given.unwrap_or(Path::new("/bin/zsh")).to_owned()),
            Platform::Linux | Platform::Wsl => Some(
                given
                    .filter(|path| *path != Path::new("/bin/sh"))
                    .or(self.shell.as_deref())
                    .unwrap_or(Path::new("/bin/sh"))
                    .to_owned(),
            ),
            Platform::Windows => git_bash(env).ok(),
        }
    }
}

#[cfg(unix)]
fn system_account() -> io::Result<Account> {
    use std::{ffi::CStr, mem::MaybeUninit, os::unix::ffi::OsStrExt, ptr};
    let mut size = 4096;
    loop {
        let mut bytes = vec![0; size];
        let mut record = MaybeUninit::uninit();
        let mut found = ptr::null_mut();
        // SAFETY: record, buffer and result pointer remain writable throughout getpwuid_r.
        let error = unsafe {
            libc::getpwuid_r(
                rustix::process::getuid().as_raw(),
                record.as_mut_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                &mut found,
            )
        };
        if error == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        if found.is_null() {
            #[cfg(target_os = "linux")]
            if let Some(account) = directory_account()? {
                return Ok(account);
            }
            return Ok(Account {
                name: OsString::new(),
                home: env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "account has no home")
                })?,
                shell: None,
            });
        }
        // SAFETY: success with a non-null result initialized record and its NUL-terminated fields.
        let (name, home, shell) = unsafe {
            let record = record.assume_init();
            let copy = |field| OsStr::from_bytes(CStr::from_ptr(field).to_bytes()).to_owned();
            (
                copy(record.pw_name),
                PathBuf::from(copy(record.pw_dir)),
                PathBuf::from(copy(record.pw_shell)),
            )
        };
        return Ok(Account {
            name,
            home,
            shell: shell.is_absolute().then_some(shell),
        });
    }
}

#[cfg(windows)]
fn system_account() -> io::Result<Account> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Profile, SHGetKnownFolderPath};
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn GetUserNameW(buffer: *mut u16, length: *mut u32) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(buffer: *mut std::ffi::c_void);
    }
    let mut name = [0; 257];
    let mut len = name.len() as u32;
    let mut profile = std::ptr::null_mut();
    // SAFETY: both calls receive writable output buffers of the specified size or pointer type.
    let (named, result) = unsafe {
        (
            GetUserNameW(name.as_mut_ptr(), &mut len),
            SHGetKnownFolderPath(&FOLDERID_Profile, 0, std::ptr::null_mut(), &mut profile),
        )
    };
    if result < 0 {
        return Err(io::Error::other(format!(
            "Windows profile lookup failed ({result:#x})"
        )));
    }
    // SAFETY: success returns a NUL-terminated string owned by the COM allocator.
    let home = unsafe {
        let len = (0..).find(|i| profile.add(*i).read() == 0).unwrap();
        let home = OsString::from_wide(std::slice::from_raw_parts(profile, len));
        CoTaskMemFree(profile.cast());
        home
    };
    let mut account = Account {
        name: if named == 0 {
            OsString::new()
        } else {
            OsString::from_wide(&name[..len as usize - 1])
        },
        home: home.into(),
        shell: None,
    };
    account.home = account.home_folder(Platform::Windows, &Environment::new())?;
    Ok(account)
}

/// The root of the frozen layout. File families and durability belong to kuvo_store.
pub fn box_home(account: &Account, platform: Platform, env: &Environment) -> io::Result<PathBuf> {
    match env_value(platform, env, "KUVO_HOME").filter(|path| !path.is_empty()) {
        Some(path) => Ok(path.into()),
        None => Ok(account.home_folder(platform, env)?.join(".kuvo-code")),
    }
}

pub fn windows_wire_path(native: &str) -> Option<String> {
    let wire = native.replace('\\', "/");
    let bytes = wire.as_bytes();
    let tail = if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":/" {
        &wire[3..]
    } else {
        let unc = wire.strip_prefix("//")?;
        if unc
            .split('/')
            .take(2)
            .filter(|part| !part.is_empty())
            .count()
            != 2
        {
            return None;
        }
        unc
    };
    if wire[2..].contains("//")
        || tail.chars().any(|c| c <= '\x1f' || ":<>\"|?*".contains(c))
        || tail
            .trim_end_matches('/')
            .split('/')
            .any(|part| part.ends_with(['.', ' ']))
    {
        return None;
    }
    Some(wire)
}

pub fn windows_native_path(wire: &str) -> Option<String> {
    (windows_wire_path(wire)?.as_str() == wire).then(|| wire.replace('/', "\\"))
}

/// Extend a validated wire path for Win32 calls that still enforce MAX_PATH.
pub fn windows_extended_path(wire: &str) -> Option<String> {
    let native = windows_native_path(wire)?;
    if let Some(share) = native.strip_prefix(r"\\") {
        Some(format!(r"\\?\UNC\{share}"))
    } else {
        Some(format!(r"\\?\{native}"))
    }
}

/// A Win32 spelling to check when canonicalization returned an extended path.
pub fn windows_conventional_path(path: &Path) -> Option<PathBuf> {
    let tail = path.to_str()?.strip_prefix(r"\\?\")?;
    let native = match tail.strip_prefix(r"UNC\") {
        Some(share) => format!(r"\\{share}"),
        None if tail.as_bytes().get(1) == Some(&b':') => tail.to_owned(),
        None => return None,
    };
    windows_wire_path(&native)?;
    Some(native.into())
}

/// Decode the file URL after the HTTP query layer has decoded its own escaping.
pub fn file_uri_path(platform: Platform, uri: &str) -> Option<PathBuf> {
    use percent_encoding::percent_decode_str;
    let rest = uri.strip_prefix("file://")?;
    let (authority, path) = rest.split_at(rest.find('/')?);
    let path = path.split(['?', '#']).next()?;
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }
        let quote = path.get(index + 1..index + 3)?;
        if !quote.bytes().all(|byte| byte.is_ascii_hexdigit())
            || quote.eq_ignore_ascii_case("2f")
            || (platform == Platform::Windows && quote.eq_ignore_ascii_case("5c"))
        {
            return None;
        }
        index += 3;
    }
    let path = percent_decode_str(path).decode_utf8().ok()?;
    if path.contains('\0') {
        return None;
    }
    let local = authority.is_empty() || authority.eq_ignore_ascii_case("localhost");
    if platform == Platform::Windows {
        let wire = if local {
            path[1..].to_owned()
        } else {
            format!("//{authority}{path}")
        };
        return if wire.encode_utf16().count() >= 260 {
            windows_extended_path(&wire)
        } else {
            windows_native_path(&wire)
        }
        .map(PathBuf::from);
    }
    local.then(|| PathBuf::from(path.as_ref()))
}

/// PATH on Windows accepts quoted semicolons, drops relative entries, and keeps case.
pub fn windows_path_folders(mut list: &str) -> impl Iterator<Item = String> {
    std::iter::from_fn(move || {
        while !list.is_empty() {
            let (part, remaining) = if let Some(quoted) = list.strip_prefix('"') {
                if let Some((part, rest)) = quoted.split_once('"')
                    && (rest.is_empty() || rest.starts_with(';'))
                {
                    (Some(part), rest.strip_prefix(';').unwrap_or(rest))
                } else {
                    (None, list.split_once(';').map_or("", |(_, rest)| rest))
                }
            } else {
                let (part, rest) = list.split_once(';').unwrap_or((list, ""));
                (Some(part.trim()), rest)
            };
            list = remaining;
            if let Some(path) = part.and_then(windows_wire_path) {
                return Some(path.replace('/', "\\"));
            }
        }
        None
    })
}

pub fn join_path<'a>(platform: Platform, lists: impl IntoIterator<Item = &'a OsStr>) -> OsString {
    if platform == Platform::Windows {
        let mut seen = std::collections::HashSet::new();
        let folders = lists
            .into_iter()
            .flat_map(|list| windows_path_folders(list.to_str().unwrap_or_default()))
            .filter(|folder| seen.insert(folder.clone()))
            .map(|folder| {
                if folder.contains(';') {
                    format!("\"{folder}\"")
                } else {
                    folder
                }
            });
        return folders.collect::<Vec<_>>().join(";").into();
    }
    let mut seen = std::collections::HashSet::new();
    let folders = lists
        .into_iter()
        .flat_map(env::split_paths)
        .filter(|folder| {
            !folder.as_os_str().is_empty()
                && (platform != Platform::Wsl || !is_wsl_windows_drive(folder))
                && seen.insert(folder.clone())
        });
    env::join_paths(folders).expect("PATH items already split")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Executable {
    Native(PathBuf),
    /// An explicitly resolved cmd/bat file. Invoke through the Windows script launcher.
    CmdScript(PathBuf),
}

impl Executable {
    pub fn path(&self) -> &Path {
        match self {
            Self::Native(path) | Self::CmdScript(path) => path,
        }
    }
}

/// Search the supplied PATH followed by extras. No implicit search of the current directory.
pub fn which(name: &OsStr, env: &Environment, extra: &[PathBuf]) -> Option<Executable> {
    let platform = Platform::current();
    let path = Path::new(name);
    let list = env_value(platform, env, "PATH").unwrap_or_default();
    let folders: Vec<_> = if platform == Platform::Windows {
        windows_path_folders(list.to_str()?)
            .map(PathBuf::from)
            .collect()
    } else {
        env::split_paths(list)
            .filter(|folder| !folder.as_os_str().is_empty())
            .collect()
    };
    let bases = if path.is_absolute() {
        vec![path.to_owned()]
    } else if path.file_name() == Some(name) && !name.is_empty() {
        folders
            .iter()
            .chain(extra)
            .filter(|folder| !folder.as_os_str().is_empty())
            .filter(|folder| platform != Platform::Windows || folder.is_absolute())
            .filter_map(|folder| {
                let path = std::path::absolute(folder.join(name)).ok()?;
                (platform != Platform::Wsl || !is_wsl_windows_drive(&path)).then_some(path)
            })
            .collect()
    } else {
        return None;
    };
    if platform != Platform::Windows {
        #[cfg(unix)]
        return bases
            .into_iter()
            .find(|path| {
                path.is_file() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
            })
            .map(Executable::Native);
    }
    let extensions = env_value(platform, env, "PATHEXT")
        .and_then(OsStr::to_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD");
    for base in bases {
        let candidates = if base.extension().is_some() {
            vec![base]
        } else {
            extensions
                .split(';')
                .filter_map(|ext| {
                    let ext = ext.trim().strip_prefix('.')?;
                    (!ext.is_empty() && ext.bytes().all(|byte| byte.is_ascii_alphanumeric()))
                        .then(|| base.with_extension(ext))
                })
                .collect()
        };
        for path in candidates {
            let kind = match path
                .extension()
                .and_then(OsStr::to_str)
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("exe" | "com") => Executable::Native,
                Some("cmd" | "bat") => Executable::CmdScript,
                _ => continue,
            };
            if path.is_file() {
                return Some(kind(path));
            }
        }
    }
    None
}

pub fn is_wsl_windows_drive(path: &Path) -> bool {
    path.strip_prefix("/mnt")
        .ok()
        .and_then(|path| path.iter().next())
        .is_some_and(|drive| {
            drive.as_encoded_bytes().len() == 1 && drive.as_encoded_bytes()[0].is_ascii_alphabetic()
        })
}

/// Git for Windows supplies the shell. In particular, System32\bash.exe is the WSL launcher.
pub fn git_bash(env: &Environment) -> io::Result<PathBuf> {
    let platform = Platform::Windows;
    let on_path = env_value(platform, env, "PATH")
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let candidates = windows_path_folders(on_path)
        .map(PathBuf::from)
        .filter(|path| path.join("git.exe").is_file())
        .filter_map(|path| {
            path.parent()
                .map(|parent| parent.join("bin").join("bash.exe"))
        })
        .chain(
            ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
                .into_iter()
                .filter_map(|key| {
                    let root = windows_wire_path(env_value(platform, env, key)?.to_str()?)?;
                    let root = PathBuf::from(root.replace('/', "\\"));
                    let installed = if key == "LOCALAPPDATA" {
                        root.join("Programs")
                    } else {
                        root
                    };
                    Some(installed.join("Git").join("bin").join("bash.exe"))
                }),
        );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Git for Windows is required for Kuvo shell commands",
            )
        })
}

/// Run shell source through Git for Windows. Git Bash takes its working
/// directory from the process, so a Windows path never becomes shell source.
#[cfg(windows)]
pub fn git_bash_command(
    env: &Environment,
    folder: &Path,
    source: &str,
) -> io::Result<std::process::Command> {
    use std::os::windows::process::CommandExt as _;
    let folder = folder.canonicalize()?;
    let folder = windows_conventional_path(&folder)
        .and_then(|path| windows_wire_path(path.to_str()?))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Kuvo needs an absolute Windows shell folder",
            )
        })?;
    let configs = env_value(Platform::Windows, env, "GIT_CONFIG_COUNT")
        .and_then(OsStr::to_str)
        .and_then(|count| count.parse::<u16>().ok())
        .unwrap_or(0);
    let mut command = std::process::Command::new(git_bash(env)?);
    command
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .env_clear()
        .envs(
            env.iter()
                .filter(|(name, _)| !name.as_encoded_bytes().starts_with(b"=")),
        )
        .env_remove("BASH_ENV")
        .env("MSYS", "winsymlinks:nativestrict")
        .env("GIT_CONFIG_COUNT", (u32::from(configs) + 1).to_string())
        .env(format!("GIT_CONFIG_KEY_{configs}"), "core.longpaths")
        .env(format!("GIT_CONFIG_VALUE_{configs}"), "true")
        .args(["--noprofile", "--norc", "-c"])
        // CreateProcess cannot take every working directory that Bash supports.
        // Keep the Windows path separate from the shell source.
        .arg(format!(
            "cd -- \"$1\" || exit\nshift\n{}",
            source.replace("\r\n", "\n")
        ))
        .args(["kuvo", &folder]);
    Ok(command)
}

/// Capture the environment of a terminal's login shell, then try a non-interactive login.
/// A slow profile is stopped as a group. This synchronous call belongs on a blocking worker.
pub fn login_environment(
    account: &Account,
    env: &Environment,
    interactive_timeout: std::time::Duration,
    login_timeout: std::time::Duration,
) -> io::Result<Option<Environment>> {
    let platform = Platform::current();
    let inherited = env_value(platform, env, "PATH").unwrap_or_default();
    #[cfg(windows)]
    {
        let _ = (account, interactive_timeout, login_timeout);
        let path = join_path(platform, [inherited]);
        if path.is_empty() {
            return Ok(None);
        }
        let mut result: Environment = env
            .iter()
            .map(|(key, val)| (key.to_string_lossy().to_uppercase().into(), val.clone()))
            .collect();
        result.insert("PATH".into(), path);
        Ok(Some(result))
    }
    #[cfg(unix)]
    {
        let shell = account
            .login_shell(platform, env)
            .expect("Unix login shell");
        if shell
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| matches!(name, "nu" | "csh" | "tcsh"))
        {
            return Ok(None);
        }
        let cwd = account.home_folder(platform, env)?;
        let mut kept: Environment = env
            .iter()
            .filter(|(key, _)| {
                key.to_str().is_some_and(|key| {
                    matches!(
                        key,
                        "HOME"
                            | "USER"
                            | "LOGNAME"
                            | "LANG"
                            | "TERM"
                            | "TMPDIR"
                            | "PATH"
                            | "SSH_AUTH_SOCK"
                    ) || key.strip_prefix("LC_").is_some_and(|suffix| {
                        !suffix.is_empty()
                            && suffix
                                .bytes()
                                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                    })
                })
            })
            .map(|(key, val)| (key.clone(), val.clone()))
            .collect();
        kept.insert("SHELL".into(), shell.as_os_str().to_owned());
        kept.insert("DISABLE_AUTO_UPDATE".into(), "true".into());
        kept.entry("HOME".into())
            .or_insert_with(|| cwd.as_os_str().to_owned());
        for (flags, duration) in [("-ilc", interactive_timeout), ("-lc", login_timeout)] {
            if let Some(mut result) = capture_login(&shell, &cwd, kept.clone(), flags, duration)? {
                let path = join_path(
                    platform,
                    [
                        result
                            .get(OsStr::new("PATH"))
                            .map_or(OsStr::new(""), OsString::as_os_str),
                        inherited,
                    ],
                );
                result.insert("PATH".into(), path);
                return Ok(Some(result));
            }
        }
        Ok(None)
    }
}

#[cfg(unix)]
fn capture_login(
    shell: &Path,
    cwd: &Path,
    env: Environment,
    flags: &str,
    timeout: std::time::Duration,
) -> io::Result<Option<Environment>> {
    use crate::process::{Spawn, Stdio};
    use std::os::unix::ffi::OsStrExt;

    const START: &[u8] = b"\0__KUVO_PATH__\0";
    const END: &[u8] = b"\0__KUVO_END__\0";
    const READ_ENV: &str =
        "printf '\\0__KUVO_PATH__\\0'; /usr/bin/env -0; printf '\\0__KUVO_END__\\0'";
    // After exec, the login shell remains the session leader. The monitor can safely kill its
    // own group if the owner disappears and its control pipe closes.
    const WATCH: &str = "exec 3<&0 </dev/null; (read -r ignored <&3; kill -KILL 0) </dev/null >/dev/null 2>&1 & exec 3<&-; exec \"$0\" \"$@\"";
    let mut spawn = Spawn::new("/bin/sh", env);
    spawn.args = vec![
        "-c".into(),
        WATCH.into(),
        shell.as_os_str().to_owned(),
        flags.into(),
        READ_ENV.into(),
    ];
    spawn.cwd = Some(cwd.to_owned());
    spawn.stdin = Stdio::Pipe;
    spawn.stdout = Stdio::Pipe;
    let mut marker = None;
    Ok(capture_output(&spawn, timeout, |content, old_len| {
        if marker.is_none() {
            let scan = old_len.saturating_sub(START.len());
            marker = content[scan..]
                .windows(START.len())
                .position(|window| window == START)
                .map(|index| scan + index + START.len());
        }
        let first = marker?;
        let scan = first.max(old_len.saturating_sub(END.len()));
        let last = scan
            + content[scan..]
                .windows(END.len())
                .position(|window| window == END)?;
        let vars: Environment = content[first..last]
            .split(|byte| *byte == 0)
            .filter_map(|line| {
                let eq = line.iter().position(|byte| *byte == b'=')?;
                (eq > 0).then(|| {
                    (
                        OsStr::from_bytes(&line[..eq]).to_owned(),
                        OsStr::from_bytes(&line[eq + 1..]).to_owned(),
                    )
                })
            })
            .collect();
        Some(vars)
    })?
    .filter(|vars| {
        vars.get(OsStr::new("PATH"))
            .is_some_and(|path| !path.is_empty())
    }))
}

#[cfg(target_os = "linux")]
fn directory_account() -> io::Result<Option<Account>> {
    use crate::process::{Spawn, Stdio};
    use std::os::unix::ffi::OsStrExt;
    // Static musl reads the password file directly. The host's getent also sees its NSS directory.
    let uid = rustix::process::getuid().as_raw().to_string();
    let mut getent = Spawn::new("/usr/bin/getent", Environment::new());
    getent.args = vec!["passwd".into(), uid.clone().into()];
    getent.stdout = Stdio::Pipe;
    capture_output(&getent, std::time::Duration::from_secs(2), |content, _| {
        let end = content.iter().position(|byte| *byte == b'\n')?;
        let mut fields = content[..end]
            .split(|byte| *byte == b':')
            .map(OsStr::from_bytes);
        let name = fields.next()?.to_owned();
        if fields.nth(1)? != OsStr::new(&uid) {
            return None;
        }
        let home = PathBuf::from(fields.nth(2)?);
        let shell = PathBuf::from(fields.next()?);
        Some(Account {
            name,
            home,
            shell: shell.is_absolute().then_some(shell),
        })
    })
}

#[cfg(unix)]
fn capture_output<T>(
    spec: &crate::process::Spawn,
    timeout: std::time::Duration,
    mut scan: impl FnMut(&[u8], usize) -> Option<T>,
) -> io::Result<Option<T>> {
    use rustix::{
        event::{PollFd, PollFlags, Timespec, poll},
        fs::{OFlags, fcntl_getfl, fcntl_setfl},
    };
    use std::{io::Read, time::Instant};

    const LIMIT: usize = 2 * 1024 * 1024;
    let mut child = match spec.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut out = child.stdout.take().expect("configured child output");
    fcntl_setfl(&out, fcntl_getfl(&out)? | OFlags::NONBLOCK)?;
    let mut content = Vec::new();
    let mut block = [0_u8; 8192];
    let started = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Ok(None);
        }
        let until = Timespec::try_from(remaining).map_err(io::Error::other)?;
        let mut fds = [PollFd::new(&out, PollFlags::IN)];
        match poll(&mut fds, Some(&until)) {
            Ok(0) => return Ok(None),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let old_len = content.len();
        match out.read(&mut block) {
            Ok(0) => return Ok(None),
            Ok(size) => content.extend_from_slice(&block[..size]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
        if content.len() > LIMIT {
            return Ok(None);
        }
        if let Some(result) = scan(&content, old_len) {
            return Ok(Some(result));
        }
    }
}

/// Free bytes available to the current account on the filesystem containing a folder.
#[cfg(unix)]
pub fn available_space(folder: &Path) -> io::Result<u64> {
    let stats = rustix::fs::statvfs(folder)?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(windows)]
pub fn available_space(folder: &Path) -> io::Result<u64> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let path: Vec<_> = folder.as_os_str().encode_wide().chain([0]).collect();
    let mut bytes = 0;
    // SAFETY: path is NUL-terminated and bytes points to a writable u64. The
    // other output parameters are optional.
    let ok =
        unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut bytes, ptr::null_mut(), ptr::null_mut()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(bytes)
}
