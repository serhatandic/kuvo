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
fn program_name(program: &str) -> String {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_owned()
}
#[cfg(not(windows))]
fn ours(args: &[String]) -> bool {
    args.first().is_some_and(|program| {
        let name = program_name(program);
        name.starts_with("language_server") || name == "agy"
    })
}
fn endpoints(args: &[String], ports: BTreeSet<u16>) -> Vec<Endpoint> {
    let Some(csrf) = flag(args, "--csrf_token").or_else(|| {
        args.first()
            .filter(|s| program_name(s) == "agy")
            .map(|_| "")
    }) else {
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
        && !out.iter().any(|e| e.port == port && !e.tls)
    {
        out.push(Endpoint {
            port,
            tls: false,
            csrf: csrf.into(),
        });
    }
    out
}
#[cfg(not(windows))]
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
#[cfg(not(windows))]
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
#[cfg(not(windows))]
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
            && flag(&args, "--gemini-dir") != Some(profile.as_ref())
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
fn mac_profile(command: &str, profile: &str) -> bool {
    assignment(command, "GEMINI_HOME=", profile, false)
        || assignment(command, "--app_data_dir=", profile, true)
        || assignment(command, "--app_data_dir ", profile, true)
}
#[cfg(target_os = "macos")]
fn mac_listening_ports(sockets: &str) -> BTreeSet<u16> {
    sockets
        .lines()
        .filter_map(|line| {
            line.strip_suffix(" (LISTEN)")?
                .rsplit_once(':')?
                .1
                .parse::<u16>()
                .ok()
        })
        .filter(|port| *port > 0)
        .collect()
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
        if !mac_profile(&full, &profile) {
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
        let ports = mac_listening_ports(&sockets);
        let out = endpoints(&args, ports);
        if !out.is_empty() {
            return out;
        }
    }
    vec![]
}
#[cfg(windows)]
fn windows(profile: &Path, fixture: Option<&Path>) -> Vec<Endpoint> {
    use crate::process::{self, ProcessState};
    let profile = match profile.canonicalize() {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    let within = |path: &Path| {
        fixture.is_none_or(|root| path.canonicalize().is_ok_and(|p| p.starts_with(root)))
    };
    let owns = |args: &[String]| {
        ["--gemini-dir", "--app_data_dir", "--app-data-dir"]
            .into_iter()
            .filter_map(|name| flag(args, name))
            .any(|value| {
                Path::new(value).canonicalize().is_ok_and(|path| {
                    path.to_string_lossy()
                        .eq_ignore_ascii_case(&profile.to_string_lossy())
                        || path.to_string_lossy().eq_ignore_ascii_case(
                            &profile.join("antigravity-acp").to_string_lossy(),
                        )
                })
            })
    };
    let candidate = |pid| {
        process::candidate_arguments(pid, |exe| {
            let name = program_name(&exe.to_string_lossy());
            within(exe) && (name == "agy" || name.starts_with("language_server"))
        })
        .ok()
        .flatten()
    };
    let parents = match process::process_parents() {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    let listeners = match process::tcp_listeners() {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    // Check only listeners, and only command lines of matching executables.
    let mut pids = BTreeSet::new();
    pids.extend(listeners.into_iter().map(|(pid, _)| pid));
    for pid in pids {
        let Some((identity, args)) = candidate(pid) else {
            continue;
        };
        let mut admitted = owns(&args);
        if !admitted
            && args
                .first()
                .is_some_and(|p| program_name(p).starts_with("language_server"))
        {
            // An ACP language server inherits its profile from the agy parent
            // launched with --gemini-dir. No other editor's process is admitted.
            let mut child = pid;
            let mut child_start = identity.start_identity.parse::<u64>().unwrap_or(0);
            for _ in 0..8 {
                let Some(&parent) = parents.get(&child) else {
                    break;
                };
                if parent == 0 || parent == child {
                    break;
                }
                let Some(start) = process::process_started(parent) else {
                    break;
                };
                if start > child_start {
                    break;
                }
                if let Some((parent_identity, parent_args)) = candidate(parent) {
                    if parent_args
                        .first()
                        .is_some_and(|p| program_name(p) == "agy")
                        && owns(&parent_args)
                        && parent_identity.state() == ProcessState::Live
                    {
                        admitted = true;
                        break;
                    }
                }
                child = parent;
                child_start = start;
            }
        }
        if !admitted || identity.state() != ProcessState::Live {
            continue;
        }
        // The listening table is sampled again after the profile proof, so a
        // recycled PID cannot lend a previous process's port to this candidate.
        let ports = process::tcp_listeners()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(owner, port)| (owner == pid && port > 0).then_some(port))
            .collect::<BTreeSet<_>>();
        if identity.state() != ProcessState::Live {
            continue;
        }
        let out = endpoints(&args, ports);
        if !out.is_empty() {
            return out;
        }
    }
    vec![]
}

pub async fn discover(profile: PathBuf, fixture: Option<PathBuf>) -> Vec<Endpoint> {
    if let Some(root) = fixture {
        #[cfg(windows)]
        return tokio::task::spawn_blocking(move || windows(&profile, Some(&root)))
            .await
            .unwrap_or_default();
        #[cfg(not(windows))]
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
    #[cfg(windows)]
    {
        return tokio::task::spawn_blocking(move || windows(&profile, None))
            .await
            .unwrap_or_default();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = profile;
        vec![]
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mac_fixture {
    use super::*;
    #[test]
    fn process_and_listening_socket_records_select_only_the_own_profile() {
        let profile = "/fixture/My Profile [a]+(work)/profile";
        for full in [
            format!(
                "/fixture/language_server --csrf_token fixture GEMINI_HOME={profile} PATH=/usr/bin"
            ),
            format!("/fixture/language_server --app_data_dir={profile} --csrf_token fixture"),
            format!("/fixture/language_server --app_data_dir \"{profile}\" --csrf_token fixture"),
        ] {
            assert!(mac_profile(&full, profile), "{full}");
        }
        assert!(!mac_profile(
            &format!("/fixture/language_server GEMINI_HOME={profile}-other PATH=/usr/bin"),
            profile
        ));
        assert!(!mac_profile(
            &format!("/fixture/language_server NOT_GEMINI_HOME={profile} PATH=/usr/bin"),
            profile
        ));
        let sockets = "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\nls 123 fixture 3u IPv4 0 0t0 TCP 127.0.0.1:4111 (LISTEN)\nls 123 fixture 4u IPv6 0 0t0 TCP [::1]:4222 (LISTEN)\nls 123 fixture 5u IPv4 0 0t0 TCP 127.0.0.1:4333->127.0.0.1:4444 (ESTABLISHED)";
        let ports = mac_listening_ports(sockets);
        assert_eq!(ports, BTreeSet::from([4111, 4222]));
        let language_server = vec!["/fixture/language_server".into()];
        assert!(endpoints(&language_server, ports.clone()).is_empty());
        let agy = vec!["/fixture/agy".into()];
        assert!(ours(&agy));
        assert_eq!(endpoints(&agy, ports.clone()).len(), 4);
        assert!(
            endpoints(&agy, ports.clone())
                .iter()
                .all(|e| e.csrf.is_empty())
        );
        let with_csrf = vec![
            "/fixture/language_server".into(),
            "--csrf_token".into(),
            "fixture".into(),
        ];
        assert!(ours(&with_csrf));
        assert!(
            endpoints(&with_csrf, ports)
                .iter()
                .all(|e| e.csrf == "fixture")
        );
    }
}
