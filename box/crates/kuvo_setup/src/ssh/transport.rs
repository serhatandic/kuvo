use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use kuvo_os::{
    platform::{Account, Environment, Executable, Platform, login_environment, which},
    process::{Spawn, Stdio},
};
use kuvo_store::{files, locks::OwnedLock};
use sha2::{Digest, Sha256};

use super::address::Address;

const BASE: &[&str] = &[
    "BatchMode=yes",
    "RemoteCommand=none",
    "RequestTTY=no",
    "ConnectTimeout=15",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
    "ClearAllForwardings=yes",
    "ForwardAgent=no",
    "ForwardX11=no",
    "UpdateHostKeys=no",
    "ControlMaster=no",
    "ControlPath=none",
];

#[derive(Clone)]
pub(super) struct Connection {
    program: Executable,
    env: Environment,
    account: PathBuf,
    system_account: bool,
    home: PathBuf,
    address: Address,
}

pub(super) struct Ran {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

#[derive(Clone)]
pub(super) struct HostKey {
    pub kind: String,
    pub fingerprint: String,
    line: String,
}

impl Connection {
    pub(super) fn open(home: PathBuf, address: Address) -> io::Result<Self> {
        let platform = Platform::current();
        let account = Account::current()?;
        let mut env: Environment = std::env::vars_os()
            .filter(|(name, _)| !name.as_encoded_bytes().starts_with(b"="))
            .collect();
        let account_home = account.home_folder(platform, &env)?;
        let test = env
            .get(OsStr::new("KUVO_TEST_SSH"))
            .map(PathBuf::from)
            .filter(|path| {
                path.is_absolute()
                    && home != account.home.join(".kuvo-code")
                    && account_home != account.home
            });
        let program = if let Some(path) = test {
            Executable::Native(path)
        } else if platform == Platform::Mac {
            Executable::Native("/usr/bin/ssh".into())
        } else {
            which(OsStr::new("ssh"), &env, &[])
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "OpenSSH is unavailable"))?
        };
        if !env.contains_key(OsStr::new("SSH_AUTH_SOCK"))
            && account_home == account.home
            && let Ok(Some(login)) = login_environment(
                &account,
                &env,
                Duration::from_secs(2),
                Duration::from_secs(2),
            )
            && let Some(agent) = login.get(OsStr::new("SSH_AUTH_SOCK"))
        {
            env.insert("SSH_AUTH_SOCK".into(), agent.clone());
        }
        Ok(Self {
            program,
            env,
            system_account: account_home == account.home,
            account: account_home,
            home,
            address,
        })
    }

    pub(super) fn known(&self) -> PathBuf {
        self.home.join("ssh/known_hosts")
    }

    pub(super) fn probe_keys(&self, stop: &AtomicBool) -> io::Result<Vec<HostKey>> {
        let folder = self.home.join("run/ssh-scan");
        files::create_private_dirs(&folder)?;
        let file = folder.join(kuvo_wire::keys::encode(
            &crate::random::<9>().map_err(io::Error::other)?,
        ));
        let _ = self.call(
            [
                "StrictHostKeyChecking=accept-new".into(),
                format!("UserKnownHostsFile={}", quote(&file)),
                format!("GlobalKnownHostsFile={}", null_file()),
                "HashKnownHosts=no".into(),
                "CheckHostIP=no".into(),
                "PubkeyAuthentication=no".into(),
                "PasswordAuthentication=no".into(),
                "KbdInteractiveAuthentication=no".into(),
                "GSSAPIAuthentication=no".into(),
                "HostbasedAuthentication=no".into(),
                "IdentityAgent=none".into(),
            ],
            "true",
            Vec::new(),
            Duration::from_secs(60),
            stop,
        );
        let text = fs::read_to_string(&file).unwrap_or_default();
        let _ = files::remove_file_synced(&file);
        let _ = fs::remove_dir(&folder);
        Ok(text.lines().filter_map(host_key).take(8).collect())
    }

    pub(super) fn trust(&self, keys: &[HostKey]) -> io::Result<()> {
        let path = self.known();
        let _lock = OwnedLock::acquire(
            &self.home.join("ssh/.known_hosts.lock"),
            Duration::from_secs(3),
        )?;
        let kept = fs::read_to_string(&path).or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(String::new())
            } else {
                Err(error)
            }
        })?;
        let mut lines: Vec<_> = kept
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        for key in keys {
            if !lines.contains(&key.line.as_str()) {
                lines.push(&key.line);
            }
        }
        files::write_atomic(&path, format!("{}\n", lines.join("\n")).as_bytes())
    }

    pub(super) fn run(
        &self,
        command: &str,
        input: Vec<u8>,
        limit: Duration,
        stop: &AtomicBool,
    ) -> io::Result<Ran> {
        self.call(self.trusted_options(), command, input, limit, stop)
    }

    fn trusted_options(&self) -> [String; 2] {
        let ssh = self.account.join(".ssh");
        let hosts = [
            ssh.join("known_hosts"),
            ssh.join("known_hosts2"),
            self.known(),
        ];
        let hosts = hosts
            .iter()
            .map(|path| quote(path))
            .collect::<Vec<_>>()
            .join(" ");
        [
            "StrictHostKeyChecking=yes".into(),
            format!("UserKnownHostsFile={hosts}"),
        ]
    }

    pub(super) fn copy(
        &self,
        source: &Path,
        remote: &str,
        limit: Duration,
        stop: &AtomicBool,
    ) -> io::Result<Ran> {
        let program = if Platform::current() == Platform::Mac {
            Executable::Native("/usr/bin/scp".into())
        } else {
            which(OsStr::new("scp"), &self.env, &[]).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "OpenSSH scp is unavailable")
            })?
        };
        let name = source.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        let mut spawn = self.command(program, self.trusted_options());
        spawn.cwd = source.parent().map(Path::to_owned);
        spawn.args.extend([
            "-s".into(),
            "--".into(),
            Path::new(".").join(name).into_os_string(),
            format!("{}:{remote}", self.address.as_str()).into(),
        ]);
        execute(spawn, Vec::new(), limit, stop)
    }

    fn call(
        &self,
        options: impl IntoIterator<Item = String>,
        command: &str,
        input: Vec<u8>,
        limit: Duration,
        stop: &AtomicBool,
    ) -> io::Result<Ran> {
        let mut spawn = self.command(self.program.clone(), options);
        spawn
            .args
            .extend(["--".into(), self.address.as_str().into(), command.into()]);
        execute(spawn, input, limit, stop)
    }

    fn command(&self, program: Executable, options: impl IntoIterator<Item = String>) -> Spawn {
        let mut spawn = Spawn::new(program.path(), self.env.clone());
        spawn.executable = program;
        let config = self.account.join(".ssh/config");
        if !self.system_account && config.is_file() {
            spawn.args.extend(["-F".into(), config.into_os_string()]);
        }
        for option in BASE.iter().map(|value| value.to_string()).chain(options) {
            spawn.args.extend(["-o".into(), OsString::from(option)]);
        }
        spawn
    }
}

fn execute(
    mut spawn: Spawn,
    input: Vec<u8>,
    limit: Duration,
    stop: &AtomicBool,
) -> io::Result<Ran> {
    if stop.load(Ordering::Acquire) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    spawn.stdin = Stdio::Pipe;
    spawn.stdout = Stdio::Pipe;
    spawn.stderr = Stdio::Pipe;
    let mut child = spawn.spawn()?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    std::thread::scope(move |scope| {
        let send = scope.spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(&input);
            }
        });
        let out = scope.spawn(move || read(stdout));
        let err = scope.spawn(move || read(stderr));
        let deadline = Instant::now() + limit;
        let status = loop {
            if child.has_exited()? {
                break child.wait()?;
            }
            if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                child.stop(Duration::from_millis(500), |_| {})?;
                return Err(if stop.load(Ordering::Acquire) {
                    io::ErrorKind::Interrupted
                } else {
                    io::ErrorKind::TimedOut
                }
                .into());
            }
            std::thread::sleep(Duration::from_millis(40));
        };
        send.join()
            .map_err(|_| io::Error::other("SSH input worker stopped"))?;
        let stdout = out
            .join()
            .map_err(|_| io::Error::other("SSH output worker stopped"))??;
        let stderr = err
            .join()
            .map_err(|_| io::Error::other("SSH error worker stopped"))??;
        Ok(Ran {
            status,
            stdout,
            stderr: String::from_utf8_lossy(&stderr).into(),
        })
    })
}

pub(super) fn sh(script: &str) -> String {
    format!(
        "exec sh -c 'kuvo_script=$(printf %s {} | base64 -d) || exit 127; eval \"$kuvo_script\"'",
        STANDARD.encode(script)
    )
}

fn quote(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

fn null_file() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

fn host_key(line: &str) -> Option<HostKey> {
    let mut parts = line.split_ascii_whitespace();
    let host = parts.next()?;
    let kind = parts.next()?;
    let blob = parts.next()?;
    if host.starts_with(['@', '#'])
        || host.contains(['\r', '\n'])
        || !kind
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-@._".contains(&b))
    {
        return None;
    }
    let decoded = STANDARD.decode(blob).ok()?;
    let kind_text = match kind {
        "ssh-ed25519" => "ED25519",
        "ssh-rsa" => "RSA",
        "ssh-dss" => "DSA",
        other if other.starts_with("ecdsa-") => "ECDSA",
        other => other,
    };
    Some(HostKey {
        kind: kind_text.into(),
        fingerprint: format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(decoded))),
        line: format!("{host} {kind} {blob}"),
    })
}

fn read(pipe: Option<impl Read>) -> io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    if let Some(mut pipe) = pipe {
        let mut buffer = [0u8; 8192];
        loop {
            let len = pipe.read(&mut buffer)?;
            if len == 0 {
                break;
            }
            let room = (4_000_000usize).saturating_sub(kept.len()).min(len);
            kept.extend_from_slice(&buffer[..room]);
        }
    }
    Ok(kept)
}
