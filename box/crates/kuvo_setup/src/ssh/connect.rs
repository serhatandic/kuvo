use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kuvo_os::platform::{Account, Platform};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use super::{
    AddError, Computer, Job, Progress, WindowsMode,
    address::Address,
    local_name,
    transport::{self, Connection, Ran},
};

mod reservation;
mod windows;

pub(super) use reservation::Reservation;
#[cfg(unix)]
pub(crate) use reservation::valid_token as is_reservation_token;

const WINDOWS: &str = "cmd.exe /d /c echo KUVO-WINDOWS-%COMPUTERNAME%";
const LIST_WSL: &str = "wsl.exe --list --verbose";
const WSL_READ: &str = "import base64,os,sys; script=base64.b64decode(sys.stdin.buffer.raw.readline(262144)); os.execv('/bin/sh',['sh','-c','kuvo_script=1; '+script.decode()])";
const OCCUPIED_ACCOUNT: i32 = 79;

pub(super) struct Destination {
    ssh: Connection,
    name: String,
    root: String,
    distro: Option<String>,
    target: Target,
    existing: Option<Existing>,
}

#[derive(Clone)]
pub(super) struct Existing {
    pub(super) identity: Option<String>,
    pub(super) key: Option<String>,
}

impl Existing {
    fn read(lines: &[&str]) -> Option<Self> {
        if !lines.iter().any(|line| line.trim() == "installed") {
            return None;
        }
        let identity = lines.iter().find_map(|line| {
            let id = line.strip_prefix("kuvo id ")?.trim();
            (id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then(|| id.to_owned())
        });
        let key = lines.iter().find_map(|line| {
            let card = serde_json::from_str::<Value>(line.strip_prefix("card ")?).ok()?;
            card["self"]["key"].as_str().map(str::to_owned)
        });
        Some(Self { identity, key })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Target {
    os: &'static str,
    cpu: &'static str,
}

pub(super) fn has_release(home: &Path) -> bool {
    // The installed release supplies the executable. A disposable fixture may
    // provide a freshly built binary while the keeper is not installed there.
    binary(
        home,
        Target {
            os: std::env::consts::OS,
            cpu: std::env::consts::ARCH,
        },
    )
    .is_some()
}

impl Destination {
    pub(super) async fn inspect(job: &Job, address: Address, name: &str) -> Result<Self, AddError> {
        let home = job.files.root().to_owned();
        let connection = tokio::task::spawn_blocking(move || Connection::open(home, address))
            .await
            .map_err(|_| "Couldn’t start OpenSSH.")?
            .map_err(|_| "Couldn’t start OpenSSH.")?;

        let mut first = run(job, &connection, "true".into(), Vec::new(), 30)
            .await
            .map_err(|_| unreachable(name))?;
        if first.status.code() == Some(255)
            && first.stderr.contains("Host key verification failed")
            && !first
                .stderr
                .contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        {
            let stop = job.stop.clone();
            let ssh = connection.clone();
            let keys = tokio::task::spawn_blocking(move || ssh.probe_keys(&stop))
                .await
                .map_err(|_| unreachable(name))?
                .map_err(|_| unreachable(name))?;
            if keys.is_empty() {
                return Err(format!("Couldn’t read {name}’s host key.").into());
            }
            let (answer, answered) = oneshot::channel();
            job.send(Progress::HostKey(keys.clone(), answer)).await?;
            answered.await.map_err(|_| "The Add was stopped.")?;
            // Recheck the submitter's grant after the interactive wait.
            job.admit().await?;
            let ssh = connection.clone();
            tokio::task::spawn_blocking(move || ssh.trust(&keys))
                .await
                .map_err(|_| unreachable(name))?
                .map_err(|_| format!("Kuvo couldn’t save the SSH host key for {name}."))?;
            first = run(job, &connection, "true".into(), Vec::new(), 30)
                .await
                .map_err(|_| unreachable(name))?;
        }
        check_connection(&first, name, &local_name(&job.setup))?;

        Ok(Self::inspect_platform(job, connection, name).await?)
    }

    async fn inspect_platform(
        job: &Job,
        connection: Connection,
        name: &str,
    ) -> Result<Self, String> {
        let root = scratch_root(job.files.root())?;
        let mut candidate = Self {
            ssh: connection,
            name: name.into(),
            root,
            distro: None,
            target: Target { os: "", cpu: "" },
            existing: None,
        };
        let probe = r#"uname -sm
if [ "$(uname -s)" = Darwin ]; then
  echo "macos $(sw_vers -productVersion 2>/dev/null)"
  if ! xcode-select -p >/dev/null 2>&1 || ! /usr/bin/git --version >/dev/null 2>&1; then echo no-tools; fi
elif ldd --version 2>&1 | grep -qi musl; then
  echo musl
fi
"#;
        let mut checked = candidate.shell(job, probe, Vec::new(), 30).await?;
        if !checked.status.success() {
            let win = run(job, &candidate.ssh, WINDOWS.into(), Vec::new(), 30).await?;
            let hostname = String::from_utf8_lossy(&win.stdout)
                .lines()
                .find_map(|line| {
                    line.trim().strip_prefix("KUVO-WINDOWS-").filter(|name| {
                        (1..=63).contains(&name.len())
                            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    })
                })
                .map(str::to_ascii_lowercase);
            let Some(hostname) = hostname else {
                return Err(unreachable(name));
            };
            candidate.name.clone_from(&hostname);
            if job.windows == WindowsMode::Native {
                return candidate.native(job, &hostname).await;
            }
            let listing = run(job, &candidate.ssh, LIST_WSL.into(), Vec::new(), 30).await?;
            let distro = distro(&listing.stdout).ok_or_else(|| {
                format!(
                    "Install Ubuntu with WSL 2 on {}, then open Ubuntu once and try again.",
                    hostname.to_ascii_lowercase()
                )
            })?;
            candidate.distro = Some(distro);
            checked = candidate.shell(job, probe, Vec::new(), 60).await?;
        }
        let data = String::from_utf8_lossy(&checked.stdout);
        let mut lines = data.lines();
        let system = lines.next().unwrap_or("");
        let (os, cpu) = system.split_once(' ').unwrap_or(("", ""));
        let os = match os {
            "Darwin" => "macos",
            "Linux" => "linux",
            _ => "",
        };
        let cpu = match cpu {
            "x86_64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            _ => "",
        };
        if !checked.status.success() || os.is_empty() || cpu.is_empty() {
            return Err(format!(
                "Kuvo Code runs on macOS, and on Linux with glibc, on x86-64 or ARM64; this computer is {}.",
                preferences_name(system, "something else")
            ));
        }
        let found: Vec<_> = lines.collect();
        if found.contains(&"musl") {
            return Err(format!(
                "Kuvo Code runs on macOS, and on Linux with glibc, on x86-64 or ARM64; this computer is {} (musl).",
                preferences_name(system, "something else")
            ));
        }
        if let Some(version) = found.iter().find_map(|line| line.strip_prefix("macos ")) {
            let major = version
                .split('.')
                .next()
                .and_then(|text| text.parse::<u32>().ok());
            if major.is_some_and(|major| major < 13) {
                return Err(format!(
                    "Kuvo Code runs on macOS 13 or later; that Mac has macOS {}.",
                    preferences_name(version, "an older one")
                ));
            }
        }
        if found.contains(&"no-tools") {
            return Err("That Mac needs a working git first. Run xcode-select --install on it (with Xcode, accept its license), then try again.".into());
        }
        candidate.target = Target { os, cpu };
        Ok(candidate)
    }

    pub(super) async fn inspect_existing(
        &self,
        account: Option<&mut Reservation<'_>>,
    ) -> Result<Option<Existing>, String> {
        let Some(account) = account else {
            return Ok(self.existing.clone());
        };
        let probe = format!(
            r#"
K="$HOME/{root}"
if [ -e "$K/run/setup-card.json" ] || [ -e "$K/run/link.json" ] || [ -e "$K/bin/kuvo-link.js" ]; then
  echo installed
  if [ -f "$K/machine-id" ] && [ ! -L "$K/machine-id" ]; then printf 'kuvo id %s\n' "$(head -c 34 "$K/machine-id")"; fi
  if [ -f "$K/run/setup-card.json" ] && [ ! -L "$K/run/setup-card.json" ]; then printf 'card %s\n' "$(head -c 200000 "$K/run/setup-card.json")"; fi
fi
if [ "$(uname -s)" = Darwin ] && [ -d "$HOME/Library/Application Support/run.kuvo.code" ]; then echo older; fi
"#,
            root = self.root
        );
        let ran = account.run(&probe, Vec::new(), 30).await?;
        if !ran.status.success() {
            return Err(unreachable(&self.name));
        }
        let data = String::from_utf8_lossy(&ran.stdout);
        let lines: Vec<_> = data.lines().collect();
        let existing = Existing::read(&lines);
        if existing.is_none() && lines.contains(&"older") {
            return Err(format!(
                "{} still has Kuvo Code 0.7’s sessions. Update it to Kuvo first — its sessions move over then — and add it again.",
                self.name
            ));
        }
        Ok(existing)
    }

    pub(super) async fn install(
        &self,
        job: &Job,
        account: Option<&mut Reservation<'_>>,
    ) -> Result<(), String> {
        let root = job.files.root().to_owned();
        let target = self.target;
        let path = binary(&root, target)
            .ok_or_else(|| format!("Kuvo Code has no files of its own to give {} yet. Update Kuvo Code here, then try again.", self.name))?;
        if self.target.os == "windows" {
            return self.install_native(job, path).await;
        }
        let binary = tokio::task::spawn_blocking(move || read_binary(&path)).await.map_err(|_| "Couldn’t read the Kuvo Code release.")?
            .ok_or_else(|| format!("Kuvo Code has no files of its own to give {} yet. Update Kuvo Code here, then try again.", self.name))?;
        let digest = hex::encode(Sha256::digest(&binary));
        let script = format!(
            r#"set -eu
umask 077
K="$HOME/{root}"
[ ! -L "$K" ] && [ ! -L "$K/bin" ] && [ ! -L "$K/run" ] || exit 78
[ ! -e "$K/run/setup-card.json" ] && [ ! -e "$K/run/link.json" ] && [ ! -e "$K/bin/kuvo-link.js" ] || exit 78
mkdir -p "$K/bin" "$K/run"
T=$(mktemp "$K/bin/.ssh-box.XXXXXXXX") || exit 1
trap 'rm -f -- "$T"' EXIT INT TERM
cat >"$T"
if command -v sha256sum >/dev/null 2>&1; then sum=$(sha256sum <"$T"); else sum=$(shasum -a 256 <"$T"); fi
[ "${{sum%% *}}" = '{digest}' ] || exit 1
chmod 700 "$T"
# Publish a new executable without replacing an existing installation.
ln "$T" "$K/bin/kuvo-box"
"#,
            root = self.root
        );
        let placed = account
            .ok_or("The SSH account is no longer reserved.")?
            .run(&script, binary, 180)
            .await?;
        if !placed.status.success() {
            return Err(format!(
                "Kuvo Code’s files couldn’t be put on {}.",
                self.name
            ));
        }
        Ok(())
    }

    pub(super) async fn enroll(
        &self,
        job: &Job,
        account: Option<&mut Reservation<'_>>,
        name: &str,
        code: &str,
    ) -> Result<(Computer, String), AddError> {
        if self.target.os == "windows" {
            return Ok(self.enroll_native(job, name, code).await?);
        }
        let quoted_name = name.replace('\'', "'\"'\"'");
        let account = account.ok_or("The SSH account is no longer reserved.")?;
        let script = format!(
            r#"K="$HOME/{}"
for state in "$K/run/setup-card.json" "$K/run/link.json" "$K/bin/kuvo-link.js"; do
  if [ -e "$state" ] || [ -L "$state" ]; then exit {OCCUPIED_ACCOUNT}; fi
done
KUVO_HOME="$K" KUVO_SSH_ACCOUNT_TOKEN='{}' exec "$K/bin/kuvo-box" ssh-enroll '{quoted_name}'
"#,
            self.root,
            account.token()
        );
        let mut input = code.as_bytes().to_vec();
        input.push(b'\n');
        let ran = account.run(&script, input, 90).await?;
        if ran.status.code() == Some(OCCUPIED_ACCOUNT) {
            return Err(AddError::occupied(name));
        }
        Ok(enrollment(ran, name)?)
    }

    pub(super) fn distro(&self) -> Option<String> {
        self.distro.clone()
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    async fn shell(
        &self,
        job: &Job,
        script: &str,
        input: Vec<u8>,
        seconds: u64,
    ) -> Result<Ran, String> {
        self.shell_with_stop(job.stop.clone(), script, input, seconds)
            .await
    }

    async fn shell_with_stop(
        &self,
        stop: Arc<AtomicBool>,
        script: &str,
        mut input: Vec<u8>,
        seconds: u64,
    ) -> Result<Ran, String> {
        let command = if let Some(distro) = &self.distro {
            let mut header = STANDARD.encode(script).into_bytes();
            header.push(b'\n');
            header.append(&mut input);
            input = header;
            format!("wsl.exe -d {distro} --cd ~ --exec /usr/bin/python3 -c \"{WSL_READ}\"")
        } else {
            transport::sh(script)
        };
        run_with_stop(&self.ssh, stop, command, input, seconds).await
    }
}

async fn run(
    job: &Job,
    ssh: &Connection,
    command: String,
    input: Vec<u8>,
    seconds: u64,
) -> Result<Ran, String> {
    run_with_stop(ssh, job.stop.clone(), command, input, seconds).await
}

async fn run_with_stop(
    ssh: &Connection,
    stop: Arc<AtomicBool>,
    command: String,
    input: Vec<u8>,
    seconds: u64,
) -> Result<Ran, String> {
    let ssh = ssh.clone();
    tokio::task::spawn_blocking(move || {
        ssh.run(&command, input, Duration::from_secs(seconds), &stop)
    })
    .await
    .map_err(|_| "The SSH command stopped.".to_owned())?
    .map_err(|_| "The SSH command could not complete.".to_owned())
}

fn binary(home: &Path, target: Target) -> Option<PathBuf> {
    if scratch(home)
        && let Some(path) = std::env::var_os("KUVO_TEST_SSH_BINARY").map(PathBuf::from)
        && path.is_absolute()
        && path.is_file()
    {
        return Some(path);
    }
    if target.os != std::env::consts::OS || target.cpu != std::env::consts::ARCH {
        return None;
    }
    let bin = home.join(if cfg!(windows) {
        "bin/kuvo-box.exe"
    } else {
        "bin/kuvo-box"
    });
    bin.is_file().then_some(bin)
}

fn scratch(home: &Path) -> bool {
    Account::current()
        .and_then(|account| {
            let env = std::env::vars_os().collect();
            let account_home = account.home_folder(Platform::current(), &env)?;
            Ok(account_home != account.home && home != account.home.join(".kuvo-code"))
        })
        .unwrap_or(false)
}

fn scratch_root(home: &Path) -> Result<String, String> {
    let Ok(root) = std::env::var("KUVO_TEST_SSH_ROOT") else {
        return Ok(".kuvo-code".into());
    };
    if scratch(home)
        && !matches!(root.as_str(), "" | "." | ".." | ".kuvo-code")
        && root
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        Ok(root)
    } else {
        Err("The test needs its own remote install folder.".into())
    }
}

fn distro(bytes: &[u8]) -> Option<String> {
    let body = if bytes.iter().take(30).any(|b| *b == 0) {
        String::from_utf16_lossy(
            &bytes
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let choices = body
        .lines()
        .filter_map(|line| {
            let pieces: Vec<_> = line
                .trim()
                .trim_start_matches(['*', '\u{feff}'])
                .split_whitespace()
                .collect();
            let name = *pieces.first()?;
            (pieces.len() >= 3
                && pieces.last() == Some(&"2")
                && !name.to_ascii_lowercase().starts_with("docker-desktop")
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
            .then_some(name)
        })
        .collect::<Vec<_>>();
    choices
        .iter()
        .copied()
        .find(|name| {
            let name = name.to_ascii_lowercase();
            name == "ubuntu" || name.starts_with("ubuntu-")
        })
        .or_else(|| choices.first().copied())
        .map(str::to_owned)
}

fn preferences_name(text: &str, fallback: &str) -> String {
    let text = crate::preferences::short_name(text, 60);
    if text.is_empty() {
        fallback.into()
    } else {
        text
    }
}

fn unreachable(name: &str) -> String {
    format!("Couldn’t reach {name}. Make sure it’s on and connected, then Try Again.")
}

fn check_connection(ran: &Ran, name: &str, here: &str) -> Result<(), AddError> {
    if ran.status.code() != Some(255) {
        return Ok(());
    }
    let said = &ran.stderr;
    if said.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        let host = said.lines().find_map(|line| {
            let host = line
                .trim_end()
                .strip_prefix("Host key for ")?
                .strip_suffix(" has changed and you have requested strict checking.")?;
            (!host.is_empty()
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:-[]".contains(&b)))
            .then_some(host)
        });
        let host = host.map_or_else(|| "and its address".into(), |host| format!("'{host}'"));
        return Err(AddError::titled(
            format!("{name}’s host key changed"),
            format!(
                "{name}’s host key has changed since {here} last connected. If you expect that (it was reinstalled, say), remove the old key in a terminal on {here} with ssh-keygen -R {host}, then try again. If not, someone may be intercepting the connection."
            ),
        ));
    }
    let lower = said.to_ascii_lowercase();
    if lower.contains("permission denied") || lower.contains("too many authentication failures") {
        return Err(AddError::titled(
            format!("{name} refused the connection"),
            format!(
                "{name} didn’t accept {here}’s SSH keys. Kuvo Code connects with the keys and SSH agent of {here}, and the SSH keys in its vault, without a password. If your key is in an app like 1Password or Secretive, or on a security key, unlock it, then try again."
            ),
        ));
    }
    Ok(())
}

fn enrollment(ran: Ran, name: &str) -> Result<(Computer, String), String> {
    if !ran.status.success() {
        return Err(format!("Kuvo Code on {name} couldn’t join your computers."));
    }
    #[derive(Deserialize)]
    struct Enrolled {
        key: String,
        identity: String,
    }
    let answer: Enrolled = serde_json::from_slice(&ran.stdout)
        .map_err(|_| format!("Kuvo Code on {name} couldn’t join your computers."))?;
    if answer.identity.len() != 32 || !answer.identity.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("Kuvo Code on {name} couldn’t join your computers."));
    }
    Ok((
        Computer {
            key: answer.key,
            name: name.into(),
        },
        answer.identity,
    ))
}

fn read_binary(path: &Path) -> Option<Vec<u8>> {
    let info = path.symlink_metadata().ok()?;
    (info.is_file() && info.len() <= 128 * 1024 * 1024)
        .then(|| fs::read(path).ok())
        .flatten()
}
