//! Renew a Helper that predates the self-renewing Helper protocol.

use std::{
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt as _,
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use kuvo_os::{
    platform::{Account, Platform},
    process::{ProcessIdentity, ProcessState, Spawn},
};
use kuvo_store::{files, home::Home, locks};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

use super::{HELPER, LABEL, LaunchAgent, domain, output};

const RECORD: &str = "helper-handover.json";
const HELPER_PROGRAM: &str = "Contents/MacOS/Kuvo Helper";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Outcome {
    Handing,
    Handed,
    Stopped,
    Gone,
    Stayed,
    Failed,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    #[serde(default)]
    v: u8,
    bundle: String,
    copy: u64,
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    helper: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by: Option<u32>,
    at: u64,
    outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    still_old: Option<StillOld>,
}

#[derive(Serialize, Deserialize)]
struct StillOld {
    pid: u32,
    at: u64,
}

impl Receipt {
    fn read(home: &Home) -> io::Result<Option<Self>> {
        let Some(bytes) = files::read(&home.run().join(RECORD))? else {
            return Ok(None);
        };
        let record: Self = serde_json::from_slice(&bytes)?;
        if record.v > 1 {
            return Err(io::Error::other(
                "The Helper handover record uses an unsupported format.",
            ));
        }
        Ok(Some(record))
    }

    fn save(&self, home: &Home) -> io::Result<()> {
        files::write_secret(&home.run().join(RECORD), &serde_json::to_vec(self)?)
    }
}

pub(super) fn status(home: &Home) -> io::Result<serde_json::Value> {
    Ok(serde_json::to_value(Receipt::read(home)?)?)
}

pub fn watch_handover(home: Home) -> Option<JoinHandle<()>> {
    let (agent, real) = tools(&home).ok()??;
    drop(agent);
    let myself = std::process::id().to_string();
    let parent = output(Command::new("/bin/ps").args(["-o", "ppid=", "-p", &myself])).ok()?;
    let pid = String::from_utf8_lossy(&parent.stdout)
        .trim()
        .parse()
        .ok()?;
    let helper = ProcessIdentity::read(pid).ok()??;
    let shorter = (!real)
        .then(|| {
            std::env::var("KUVO_TEST_CODE_EVERY")
                .ok()?
                .parse::<u64>()
                .ok()
        })
        .flatten();
    let round = Duration::from_millis(shorter.filter(|ms| *ms > 0).unwrap_or(30_000));
    Some(tokio::spawn(async move {
        loop {
            match consider(&home, &helper) {
                Ok(true) => tokio::time::sleep(round).await,
                Ok(false) => return,
                Err(error) => {
                    eprintln!("Kuvo Helper handover: {error}");
                    return;
                }
            }
        }
    }))
}

/// Retry while the update or a previous handover is still active.
fn consider(home: &Home, helper: &ProcessIdentity) -> io::Result<bool> {
    if stopped(home) || !ours(home, helper)? {
        return Ok(false);
    }
    if updating(home) {
        return Ok(true);
    }
    let Some((bundle, copy)) = build(home)? else {
        return Ok(true);
    };
    if !renews(home) {
        return Ok(false);
    }
    let _lock = locks::try_file_lock(&home.run().join(".helper-handover.lock"))?;
    if let Some(mut ready) = Receipt::read(home)?
        && ready.bundle == bundle
        && ready.copy == copy
    {
        if ready.outcome == Outcome::Handing
            && ready.by.map_or_else(
                || now().saturating_sub(ready.at) < 120_000,
                |pid| command_line(pid).is_ok_and(|line| line.ends_with(" helper-hand-over")),
            )
        {
            return Ok(true);
        }
        if ready.still_old.is_none() {
            if ready.outcome == Outcome::Handing {
                ready.outcome = Outcome::Failed;
                ready.message = Some("The hand-over ended before it said how it went.".into());
            }
            ready.still_old = Some(StillOld {
                pid: helper.pid,
                at: now(),
            });
            ready.save(home)?;
            eprintln!(
                "Kuvo Helper is still the one from before the update after it was handed over once."
            );
        }
        return Ok(false);
    }
    let mut record = Receipt {
        v: 1,
        bundle,
        copy,
        pid: helper.pid,
        helper: Some(helper.clone()),
        by: None,
        at: now(),
        outcome: Outcome::Handing,
        message: None,
        still_old: None,
    };
    record.save(home)?;
    let mut start = Spawn::new(std::env::current_exe()?, std::env::vars_os().collect());
    start.args.push("helper-hand-over".into());
    start.cwd = Some(home.root().into());
    start
        .env
        .insert("KUVO_HOME".into(), home.root().as_os_str().into());
    match start.spawn() {
        Ok(mut child) => {
            record.by = Some(child.identity().pid);
            child.leave_running_on_drop();
        }
        Err(error) => {
            record.outcome = Outcome::Failed;
            record.message = Some(error.to_string());
        }
    }
    record.save(home)?;
    Ok(false)
}

pub fn run_handover(home: &Home) -> io::Result<()> {
    let mut record = {
        let lock = home.run().join(".helper-handover.lock");
        let deadline = Instant::now() + Duration::from_secs(1);
        let _lock = loop {
            match locks::try_file_lock(&lock) {
                Ok(lock) => break lock,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error),
            }
        };
        let Some(mut record) = Receipt::read(home)? else {
            return Ok(());
        };
        if record.outcome != Outcome::Handing
            || record.by.is_some_and(|by| by != std::process::id())
        {
            return Ok(());
        }
        record.by = Some(std::process::id());
        record.save(home)?;
        record
    };
    let result = perform(home, &record);
    let (outcome, message) = match result {
        Ok((outcome, message)) => (outcome, message.to_owned()),
        Err(error) => (
            if error.kind() == io::ErrorKind::Interrupted {
                Outcome::Stopped
            } else {
                Outcome::Failed
            },
            error.to_string(),
        ),
    };
    record.outcome = outcome;
    record.message = Some(message);
    record.save(home)
}

fn perform(home: &Home, record: &Receipt) -> io::Result<(Outcome, &'static str)> {
    let Some((agent, _)) = tools(home)? else {
        return Err(io::Error::other(
            "Only Kuvo on a Mac in ~/.kuvo-code runs under Kuvo Helper.",
        ));
    };
    LaunchAgent::automatic(home, true)?;
    while updating(home) {
        LaunchAgent::automatic(home, true)?;
        thread::sleep(Duration::from_millis(100));
    }
    if build(home)?.as_ref() != Some(&(record.bundle.clone(), record.copy)) {
        return Err(io::Error::other(
            "Kuvo changed releases before the Helper handover.",
        ));
    }
    let Some(helper) = record
        .helper
        .as_ref()
        .filter(|helper| helper.pid == record.pid)
    else {
        return Err(io::Error::other(
            "The Helper handover is missing its process identity.",
        ));
    };
    if !ours(home, helper)? {
        return Ok((
            Outcome::Gone,
            "Kuvo Helper from before the update had gone already: nothing was done.",
        ));
    }
    LaunchAgent::automatic(home, true)?;
    let _ = output(Command::new("/bin/kill").args(["-TERM", &record.pid.to_string()]))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while helper.state() != ProcessState::Stale {
        if Instant::now() >= deadline {
            return Ok((
                Outcome::Stayed,
                "Kuvo Helper from before the update didn't stop: it moves to the new one at your next login.",
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    let service = format!("{}/{LABEL}", domain()?);
    let expected = format!("pid = {}", record.pid);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let shown = agent.run(["print", &service])?;
        if !String::from_utf8_lossy(&shown.stdout)
            .lines()
            .any(|line| line.trim() == expected)
        {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let app = files::text_of(&agent.plist).and_then(|text| {
        let (_, value) = text.split_once("<key>AssociatedBundleIdentifiers</key>")?;
        let (_, value) = value.split_once("<string>")?;
        let (id, _) = value.split_once("</string>")?;
        (!id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)))
        .then(|| id.to_owned())
    });
    let message = agent.start_in(home, app.as_deref().unwrap_or("run.kuvo.code"), true)?;
    Ok((Outcome::Handed, message.message()))
}

fn tools(home: &Home) -> io::Result<Option<(LaunchAgent, bool)>> {
    let account = Account::current()?;
    let profile = account
        .home_folder(Platform::Mac, &std::env::vars_os().collect())?
        .canonicalize()?;
    let root = home.root().canonicalize()?;
    let expected = profile.join(".kuvo-code").canonicalize()?;
    if root != expected
        || files::text_of(&home.root().join("box/kind"))
            .as_deref()
            .map(str::trim)
            != Some("mac")
    {
        return Ok(None);
    }
    let real = profile == account.home.canonicalize()?;
    Ok(LaunchAgent::for_account(&profile, real).map(|agent| (agent, real)))
}

fn build(home: &Home) -> io::Result<Option<(String, u64)>> {
    let bundle = home.bin().join(HELPER);
    let mut seal = Sha256::new();
    let mut copy = None;
    for part in [
        HELPER_PROGRAM,
        "Contents/Info.plist",
        "Contents/_CodeSignature/CodeResources",
    ] {
        let mut file = match File::open(bundle.join(part)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if part == HELPER_PROGRAM {
            copy = Some(file.metadata()?.ino());
        }
        let mut buffer = [0; 8192];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            seal.update(&buffer[..n]);
        }
    }
    Ok(copy.map(|copy| (format!("{:x}", seal.finalize()), copy)))
}

fn renews(home: &Home) -> bool {
    files::text_of(&home.bin().join(HELPER).join("Contents/Info.plist"))
        .and_then(|text| {
            let (_, next) = text.split_once("<key>KuvoHelperRenews</key>")?;
            Some(
                next.trim_start()
                    .strip_prefix("<true")
                    .is_some_and(|next| next.trim_start().starts_with("/>")),
            )
        })
        .unwrap_or(false)
}

fn command_line(pid: u32) -> io::Result<String> {
    let shown =
        output(Command::new("/bin/ps").args(["-ww", "-o", "command=", "-p", &pid.to_string()]))?;
    if !shown.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&shown.stdout).trim().to_owned())
}

fn ours(home: &Home, helper: &ProcessIdentity) -> io::Result<bool> {
    if helper.pid <= 1 || helper.state() != ProcessState::Live {
        return Ok(false);
    }
    let program = home.bin().join(HELPER).join(HELPER_PROGRAM);
    Ok(
        command_line(helper.pid)?.contains(&format!("{} keep", program.display()))
            && helper.state() == ProcessState::Live,
    )
}

fn stopped(home: &Home) -> bool {
    LaunchAgent::automatic(home, true).is_err()
}

fn updating(home: &Home) -> bool {
    let run = home.run();
    if run.join("previous").exists() {
        return true;
    }
    let lock = run.join(".updating");
    if let Some(pid) = files::text_of(&lock.join("pid")).and_then(|pid| pid.trim().parse().ok()) {
        return command_line(pid)
            .is_ok_and(|line| line.contains("kuvo-link") || line.contains("kuvo-box update"));
    }
    fs::metadata(lock)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|elapsed| elapsed < Duration::from_secs(60))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
