//! macOS discovery for the app being moved. Only processes identified by
//! their executable or bundle are eligible to receive a signal.
use crate::Error;
use kuvo_os::{
    platform::Account,
    process::{ProcessIdentity, ProcessState},
};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Toucher {
    pid: u32,
    command: String,
    executable: PathBuf,
    written: Vec<PathBuf>,
}

struct Discovery<'a> {
    source: &'a Path,
    actual: PathBuf,
    ancestors: BTreeSet<u32>,
}

impl<'a> Discovery<'a> {
    fn new(source: &'a Path) -> Result<Self, Error> {
        let actual = source.canonicalize()?;
        let list = Command::new("/bin/ps")
            .env_clear()
            .args(["-axo", "pid=,ppid="])
            .output()?;
        let parents: HashMap<_, _> = String::from_utf8_lossy(&list.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_ascii_whitespace();
                Some((
                    fields.next()?.parse::<u32>().ok()?,
                    fields.next()?.parse::<u32>().ok()?,
                ))
            })
            .collect();
        let mut ancestors = BTreeSet::new();
        let mut pid = std::process::id();
        while pid > 1 && ancestors.insert(pid) {
            let Some(parent) = parents.get(&pid) else {
                break;
            };
            pid = *parent;
        }
        Ok(Self {
            source,
            actual,
            ancestors,
        })
    }

    fn scan(&self) -> Result<Vec<Toucher>, Error> {
        let result = Command::new("/usr/sbin/lsof")
            .env_clear()
            .args(["-n", "-P", "-w", "-F0pcfan", "-u"])
            .arg(Account::current()?.name)
            .output()?;
        let mut found = Vec::new();
        let mut executable = false;
        let mut write = false;
        for field in result.stdout.split(|byte| *byte == 0) {
            let field = std::str::from_utf8(field.trim_ascii_start()).unwrap_or_default();
            let Some((kind, value)) = field.split_at_checked(1) else {
                continue;
            };
            if kind == "p" {
                found.push(Toucher {
                    pid: value.parse().unwrap_or_default(),
                    ..Toucher::default()
                });
                continue;
            }
            let Some(item) = found.last_mut() else {
                continue;
            };
            match kind {
                "c" => item.command = value.into(),
                "f" => {
                    executable = value == "txt";
                    write = false;
                }
                "a" => write = matches!(value, "w" | "u"),
                "n" => {
                    let file = Path::new(value);
                    if item.executable.as_os_str().is_empty() && executable {
                        item.executable = file.into();
                    }
                    if write && self.contains(file) {
                        item.written.push(file.into());
                    }
                }
                _ => {}
            }
        }
        found.retain(|item| item.pid > 1 && !self.ancestors.contains(&item.pid));
        Ok(found)
    }

    fn contains(&self, file: &Path) -> bool {
        file.starts_with(self.source) || file.starts_with(&self.actual)
    }

    fn earlier(
        &self,
        process: &Toucher,
        mover: &Path,
        mover_id: &str,
        program: Option<&str>,
    ) -> bool {
        let executable = &process.executable;
        if [self.source, self.actual.as_path()]
            .into_iter()
            .any(|folder| {
                let in_programs = ["engine", "box"]
                    .iter()
                    .any(|name| executable.starts_with(folder.join(name)));
                in_programs && !executable.starts_with(folder.join("box/artifacts"))
            })
        {
            return true;
        }
        let Some(bundle) = app_of(executable) else {
            return false;
        };
        if bundle == mover || bundle.canonicalize().is_ok_and(|actual| actual == mover) {
            return false;
        }
        program
            .zip(plist(&bundle, "CFBundleExecutable").as_deref())
            .is_some_and(|(now, before)| {
                now != before && plist(&bundle, "CFBundleIdentifier").as_deref() == Some(mover_id)
            })
    }
}

fn app_of(program: &Path) -> Option<PathBuf> {
    let mut bundle = PathBuf::new();
    for part in program.components() {
        bundle.push(part);
        if bundle.extension().is_some_and(|ext| ext == "app") {
            return Some(bundle);
        }
    }
    None
}

fn plist(bundle: &Path, key: &str) -> Option<String> {
    let output = Command::new("/usr/bin/plutil")
        .env_clear()
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(super) fn quiesce(source: &Path, bundle: &Path, id: &str, real: bool) -> Result<(), Error> {
    let discovery = Discovery::new(source)?;
    stop_keeper(&discovery, real)?;
    let mover = bundle.canonicalize()?;
    let executable = plist(&mover, "CFBundleExecutable");
    let active = discovery.scan()?;
    let is_earlier = |item: &Toucher| discovery.earlier(item, &mover, id, executable.as_deref());
    let predecessors: Vec<_> = active.iter().filter(|item| is_earlier(item)).collect();
    if predecessors.is_empty() {
        return refuse_writers(&active);
    }
    if real {
        let uid = Command::new("/usr/bin/id").env_clear().arg("-u").output()?;
        let target = format!(
            "gui/{}/{id}.box",
            String::from_utf8_lossy(&uid.stdout).trim()
        );
        let _ = Command::new("/bin/launchctl")
            .env_clear()
            .args(["bootout", &target])
            .output();
    }
    let mut identities = HashMap::new();
    for item in predecessors {
        if let Some(identity) = ProcessIdentity::read(item.pid)? {
            identities.insert(item.pid, identity);
        }
    }
    // Verify the executable again after recording the process lifetime. A PID
    // recycled between discovery and signalling cannot authorize a signal.
    let mut live = Vec::new();
    for item in discovery.scan()?.iter().filter(|item| is_earlier(item)) {
        if let Some(identity) = identities.remove(&item.pid)
            && identity.state() == ProcessState::Live
        {
            eprintln!(
                "Stopping what still runs of the earlier Kuvo Code: {} {}",
                item.pid,
                item.executable.display()
            );
            live.push(identity);
        }
    }
    for (signal, wait) in [
        ("-TERM", Duration::from_secs(15)),
        ("-KILL", Duration::from_secs(5)),
    ] {
        live.retain(|item| item.state() == ProcessState::Live);
        if live.is_empty() {
            break;
        }
        for item in &live {
            if item.state() == ProcessState::Live {
                let _ = Command::new("/bin/kill")
                    .env_clear()
                    .args([signal, &item.pid.to_string()])
                    .output();
            }
        }
        let began = Instant::now();
        while began.elapsed() < wait && live.iter().any(|item| item.state() == ProcessState::Live) {
            thread::sleep(Duration::from_millis(100));
        }
    }
    let active = discovery.scan()?;
    if active.iter().any(is_earlier) {
        return Err(super::invalid(
            "An earlier Kuvo Code is still running in the background on this Mac and didn't stop. Restart this Mac, then try again.",
        ));
    }
    refuse_writers(&active)
}

#[derive(Deserialize)]
struct Keeper {
    pid: u32,
    url: Option<String>,
    password: Option<String>,
}

fn stop_keeper(discovery: &Discovery<'_>, real: bool) -> Result<(), Error> {
    let account = discovery
        .source
        .ancestors()
        .nth(3)
        .ok_or_else(|| super::invalid("The app folder needs an account."))?;
    let file = crate::Disk(discovery.source.into()).path("box/run/keeper.json")?;
    if !crate::metadata(&file)?.is_some_and(|entry| entry.is_file()) {
        return Ok(());
    }
    let Some(keeper) = crate::json_at::<Keeper>(&file).ok().flatten() else {
        return Ok(());
    };
    if keeper.pid <= 1 || discovery.ancestors.contains(&keeper.pid) {
        return Ok(());
    }
    let Some(identity) = ProcessIdentity::read(keeper.pid)? else {
        return Ok(());
    };
    let command = command_line(keeper.pid)?;
    let predecessor = if real {
        command.contains("/Contents/MacOS/Kuvo Code")
            || command.contains(&*discovery.source.to_string_lossy())
    } else {
        command.contains(&*account.to_string_lossy())
    };
    if !command.split_ascii_whitespace().any(|arg| arg == "--box")
        || !predecessor
        || identity.state() != ProcessState::Live
    {
        return Ok(());
    }
    if let Some((url, password)) = keeper
        .url
        .as_deref()
        .and_then(keeper_control)
        .zip(keeper.password)
    {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(std::io::Error::other)?;
        let _ = client.post(url).bearer_auth(password).send();
    }
    let wait = |duration: Duration| {
        let began = Instant::now();
        while began.elapsed() < duration && identity.state() == ProcessState::Live {
            thread::sleep(Duration::from_millis(100));
        }
    };
    wait(Duration::from_secs(30));
    if identity.state() == ProcessState::Live {
        let _ = Command::new("/bin/kill")
            .env_clear()
            .args(["-TERM", &keeper.pid.to_string()])
            .output();
        wait(Duration::from_secs(15));
    }
    if identity.state() == ProcessState::Live {
        return Err(super::invalid(
            "An earlier Kuvo Code is still running in the background on this Mac and didn't stop. Restart this Mac, then try again.",
        ));
    }
    Ok(())
}

fn keeper_control(address: &str) -> Option<String> {
    let port = address
        .strip_prefix("http://127.0.0.1:")?
        .trim_end_matches('/');
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    Some(format!("http://127.0.0.1:{port}/stop"))
}

pub(crate) fn command_line(pid: u32) -> std::io::Result<String> {
    let output = Command::new("/bin/ps")
        .env_clear()
        .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(super) fn no_open_writers(source: &Path) -> Result<(), Error> {
    refuse_writers(&Discovery::new(source)?.scan()?)
}

fn refuse_writers(active: &[Toucher]) -> Result<(), Error> {
    let mut names = Vec::new();
    for item in active.iter().filter(|item| !item.written.is_empty()) {
        let name = app_of(&item.executable)
            .and_then(|bundle| {
                plist(&bundle, "CFBundleDisplayName").or_else(|| plist(&bundle, "CFBundleName"))
            })
            .unwrap_or_else(|| format!("“{}”", item.command));
        if !names.contains(&name) {
            names.push(name);
        }
        eprintln!(
            "Writing in the earlier Kuvo Code's folder: {} {}: {}",
            item.pid,
            item.executable.display(),
            item.written
                .iter()
                .take(3)
                .map(|file| file.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let (subject, verb, pronoun) = match names.split_last() {
        None => return Ok(()),
        Some((last, [])) => (last.clone(), "is", "it"),
        Some((last, rest)) => (format!("{} and {last}", rest.join(", ")), "are", "them"),
    };
    Err(super::invalid(&format!(
        "{subject} {verb} still using your earlier Kuvo Code's files. Quit {pronoun}, then try again."
    )))
}
