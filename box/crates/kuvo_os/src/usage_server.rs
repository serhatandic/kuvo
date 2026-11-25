//! Discover an already running Antigravity server for exactly one owned
//! profile. The service only connects to literal loopback endpoints.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
pub struct Endpoint {
    pub port: u16,
    pub tls: bool,
    pub csrf: String,
}
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, arg)| {
        if arg == name {
            args.get(i + 1).map(String::as_str)
        } else {
            arg.strip_prefix(name).and_then(|s| s.strip_prefix('='))
        }
    })
}
fn ours(args: &[String]) -> bool {
    args.first()
        .is_some_and(|program| program.contains("language_server") || program.ends_with("/agy"))
}
fn endpoints(args: &[String], ports: BTreeSet<u16>) -> Vec<Endpoint> {
    let Some(csrf) = flag(args, "--csrf_token")
        .or_else(|| args.first().filter(|s| s.ends_with("/agy")).map(|_| ""))
    else {
        return vec![];
    };
    let mut out = ports
        .iter()
        .flat_map(|port| {
            [
                Endpoint {
                    port: *port,
                    tls: true,
                    csrf: csrf.into(),
                },
                Endpoint {
                    port: *port,
                    tls: false,
                    csrf: csrf.into(),
                },
            ]
        })
        .collect::<Vec<_>>();
    if let Some(port) = flag(args, "--extension_server_port")
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| ports.contains(p))
    {
        if !out.iter().any(|e| e.port == port && !e.tls) {
            out.push(Endpoint {
                port,
                tls: false,
                csrf: csrf.into(),
            });
        }
    }
    out
}
fn nul(file: &Path) -> Option<Vec<String>> {
    Some(
        std::fs::read(file)
            .ok()?
            .split(|b| *b == 0)
            .filter(|bytes| !bytes.is_empty())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect(),
    )
}
fn proc_ports(folder: &Path) -> BTreeSet<u16> {
    let fds = std::fs::read_dir(folder.join("fd"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .filter_map(|target| {
            target
                .to_str()
                .and_then(|s| s.strip_prefix("socket:[")?.strip_suffix(']'))
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    let mut ports = BTreeSet::new();
    for table in ["tcp", "tcp6"] {
        let Ok(text) = std::fs::read_to_string(folder.join("net").join(table)) else {
            continue;
        };
        for line in text.lines() {
            let cols = line.split_whitespace().collect::<Vec<_>>();
            if cols.get(3) != Some(&"0A") || cols.get(9).is_none_or(|inode| !fds.contains(*inode)) {
                continue;
            }
            if let Some(port) = cols
                .get(1)
                .and_then(|address| address.rsplit_once(':'))
                .and_then(|(_, port)| u16::from_str_radix(port, 16).ok())
                .filter(|p| *p > 0)
            {
                ports.insert(port);
            }
        }
    }
    ports
}
// The fixture root is supplied explicitly by a scratch-home caller; production
// Linux reads its own /proc. No process is started in either case.
fn from_proc(root: &Path, profile: &Path) -> Vec<Endpoint> {
    let profile = profile.to_string_lossy();
    let home = format!("GEMINI_HOME={profile}");
    let Ok(entries) = std::fs::read_dir(root) else {
        return vec![];
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let folder = entry.path();
        let Some(args) = nul(&folder.join("cmdline")) else {
            continue;
        };
        if !ours(&args) {
            continue;
        }
        if flag(&args, "--app_data_dir") != Some(profile.as_ref())
            && !nul(&folder.join("environ")).is_some_and(|env| env.contains(&home))
        {
            continue;
        }
        let out = endpoints(&args, proc_ports(&folder));
        if !out.is_empty() {
            return out;
        }
    }
    vec![]
}
#[cfg(target_os = "macos")]
fn assignment(command: &str, prefix: &str, profile: &str, flag: bool) -> bool {
    command.match_indices(prefix).any(|(index, _)| {
        if index > 0
            && !command[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            return false;
        }
        let value = &command[index + prefix.len()..];
        let value = value
            .strip_prefix('"')
            .or_else(|| value.strip_prefix('\''))
            .unwrap_or(value);
        let Some(rest) = value.strip_prefix(profile) else {
            return false;
        };
        let rest = rest
            .strip_prefix('"')
            .or_else(|| rest.strip_prefix('\''))
            .unwrap_or(rest);
        if rest.is_empty() {
            return true;
        }
        if !rest.chars().next().is_some_and(char::is_whitespace) {
            return false;
        }
        let next = rest.trim_start();
        if flag && next.starts_with("--") {
            return true;
        }
        next.split_whitespace()
            .next()
            .and_then(|arg| arg.split_once('='))
            .is_some_and(|(name, _)| {
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
    })
}
#[cfg(target_os = "macos")]
async fn output(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let out = tokio::time::timeout(std::time::Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    if out.stdout.len() > 2 * 1024 * 1024 {
        return None;
    }
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
#[cfg(target_os = "macos")]
async fn mac(profile: &Path) -> Vec<Endpoint> {
    let profile = profile.to_string_lossy();
    let Some(list) = output("/bin/ps", &["axww", "-o", "pid=,command="]).await else {
        return vec![];
    };
    for line in list.lines() {
        let line = line.trim();
        let Some((pid, command)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if pid.parse::<u32>().is_err()
            || (!command.contains("language_server") && !command.contains("/agy"))
        {
            continue;
        }
        // Only read environment of an admitted candidate, not every process.
        let Some(full) = output("/bin/ps", &["eww", "-p", pid, "-o", "command="]).await else {
            continue;
        };
        if !assignment(&full, "GEMINI_HOME=", &profile, false)
            && !assignment(&full, "--app_data_dir=", &profile, true)
            && !assignment(&full, "--app_data_dir ", &profile, true)
        {
            continue;
        }
        let Some(exe) = output("/bin/ps", &["-p", pid, "-o", "comm="]).await else {
            continue;
        };
        let args = std::iter::once(exe.trim().to_owned())
            .chain(command.split_whitespace().skip(1).map(str::to_owned))
            .collect::<Vec<_>>();
        if !ours(&args) {
            continue;
        }
        let Some(sockets) = output(
            "/usr/sbin/lsof",
            &["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", pid],
        )
        .await
        else {
            continue;
        };
        let ports = sockets
            .lines()
            .filter_map(|line| {
                line.strip_suffix(" (LISTEN)")?
                    .rsplit_once(':')?
                    .1
                    .parse::<u16>()
                    .ok()
            })
            .filter(|port| *port > 0)
            .collect();
        let out = endpoints(&args, ports);
        if !out.is_empty() {
            return out;
        }
    }
    vec![]
}
pub async fn discover(profile: PathBuf, fixture: Option<PathBuf>) -> Vec<Endpoint> {
    if let Some(root) = fixture {
        return tokio::task::spawn_blocking(move || from_proc(&root, &profile))
            .await
            .unwrap_or_default();
    }
    #[cfg(target_os = "linux")]
    {
        return tokio::task::spawn_blocking(move || from_proc(Path::new("/proc"), &profile))
            .await
            .unwrap_or_default();
    }
    #[cfg(target_os = "macos")]
    {
        return mac(&profile).await;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = profile;
        vec![]
    }
}
