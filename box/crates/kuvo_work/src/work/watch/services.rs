//! Which of the commands a session left running are services, listening on TCP ports (a dev
//! server): its finish doesn't wait on those. Each command leads a process group of its own, so
//! the ports are found by group: `lsof` on macOS, `/proc` on Linux. Windows has no process groups
//! of this kind; nothing there is found to listen yet.

use std::collections::BTreeMap;

/// The TCP ports each of `groups` listens on.
pub async fn listening_ports(groups: &[u32]) -> BTreeMap<u32, Vec<u16>> {
    let mut wanted: Vec<u32> = groups.iter().copied().filter(|group| *group > 1).collect();
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() {
        return BTreeMap::new();
    }
    found_in(wanted).await
}

fn add(found: &mut BTreeMap<u32, Vec<u16>>, group: u32, port: u16) {
    let ports = found.entry(group).or_default();
    if !ports.contains(&port) {
        ports.push(port);
        ports.sort_unstable();
    }
}

/// `lsof -F gn`: a `g<group>` line before each process's files, an `n<address>` line for each.
#[cfg(target_os = "macos")]
async fn found_in(groups: Vec<u32>) -> BTreeMap<u32, Vec<u16>> {
    let named: Vec<String> = groups.iter().map(u32::to_string).collect();
    let mut command = tokio::process::Command::new("lsof");
    command.args([
        "-nP",
        "-a",
        "-g",
        &named.join(","),
        "-iTCP",
        "-sTCP:LISTEN",
        "-F",
        "gn",
    ]);
    command
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let run = tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await;
    // (It exits 1 when nothing listens: what it printed says.)
    let printed = run
        .ok()
        .and_then(Result::ok)
        .map(|output| output.stdout)
        .unwrap_or_default();
    let (mut found, mut group) = (BTreeMap::new(), 0);
    for line in String::from_utf8_lossy(&printed).lines() {
        if let Some(named) = line.strip_prefix('g') {
            group = named.parse().unwrap_or(0);
        }
        let port = line
            .strip_prefix('n')
            .and_then(|address| address.rsplit_once(':')?.1.parse::<u16>().ok());
        if let Some(port) = port.filter(|port| *port > 0 && groups.contains(&group)) {
            add(&mut found, group, port);
        }
    }
    found
}

/// The listening sockets (`/proc/net/tcp`, state 0A) by their inodes, and each process in one of
/// the groups (`/proc/<pid>/stat`'s fifth field) holding one (`/proc/<pid>/fd`).
#[cfg(target_os = "linux")]
async fn found_in(groups: Vec<u32>) -> BTreeMap<u32, Vec<u16>> {
    tokio::task::spawn_blocking(move || from_proc(&groups, std::path::Path::new("/proc")))
        .await
        .unwrap_or_default()
}

#[cfg(any(target_os = "linux", test))]
fn from_proc(groups: &[u32], proc: &std::path::Path) -> BTreeMap<u32, Vec<u16>> {
    use std::{collections::HashMap, fs};
    let mut listening: HashMap<String, u16> = HashMap::new();
    for table in ["tcp", "tcp6"] {
        let text = fs::read_to_string(proc.join("net").join(table)).unwrap_or_default();
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10 || fields[3] != "0A" {
                continue;
            }
            let port = fields[1]
                .rsplit(':')
                .next()
                .and_then(|port| u16::from_str_radix(port, 16).ok());
            if let Some(port) = port {
                listening.insert(fields[9].to_owned(), port);
            }
        }
    }
    let mut found = BTreeMap::new();
    if listening.is_empty() {
        return found;
    }
    let pids = fs::read_dir(proc).into_iter().flatten().flatten();
    for pid in pids.filter(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    }) {
        let stat = fs::read_to_string(pid.path().join("stat")).unwrap_or_default();
        // (After the name, which may hold spaces and parentheses: state, parent, group.)
        let after = stat
            .rfind(')')
            .map_or("", |end| stat.get(end + 2..).unwrap_or_default());
        let Some(group) = after
            .split(' ')
            .nth(2)
            .and_then(|group| group.parse::<u32>().ok())
        else {
            continue;
        };
        if !groups.contains(&group) {
            continue;
        }
        for fd in fs::read_dir(pid.path().join("fd"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let target = fs::read_link(fd.path()).unwrap_or_default();
            let target = target.to_string_lossy();
            let inode = target
                .strip_prefix("socket:[")
                .and_then(|rest| rest.strip_suffix(']'));
            if let Some(port) = inode.and_then(|inode| listening.get(inode)) {
                add(&mut found, group, *port);
            }
        }
    }
    found
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
async fn found_in(_groups: Vec<u32>) -> BTreeMap<u32, Vec<u16>> {
    BTreeMap::new()
}

#[cfg(test)]
mod tests {
    use super::from_proc;

    #[test]
    fn a_group_holding_a_listening_socket_is_a_service() {
        let proc = tempfile::tempdir().unwrap();
        let root = proc.path();
        std::fs::create_dir_all(root.join("net")).unwrap();
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1\n";
        std::fs::write(root.join("net/tcp"), table).unwrap();
        std::fs::create_dir_all(root.join("77/fd")).unwrap();
        std::fs::write(root.join("77/stat"), "77 (vite (dev)) S 1 70 70 0").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("socket:[4242]", root.join("77/fd/3")).unwrap();
        let found = from_proc(&[70], root);
        #[cfg(unix)]
        assert_eq!(found.get(&70), Some(&vec![5173]));
        assert!(from_proc(&[71], root).is_empty());
    }
}
