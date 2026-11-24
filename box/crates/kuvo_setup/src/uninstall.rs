//! The stopped machine path has no resident or keeper to stop. Retain its home
//! and work and clear its local application identity and credential stores.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_os::process::ProcessIdentity;
use kuvo_store::{
    files::{remove_file_synced, remove_tree_synced},
    home::Home,
    layout::{self, Layout},
    locks::OwnedLock,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    Error,
    actor::{LINK, validate_disk},
    cleanup,
    model::{Link, Role},
    storage::Files,
};

const STOPPED: &str = "Kuvo Code is stopped on this computer and won't start again. Its folder stays, with your sessions and settings.";
const DESKTOP: &str = "Kuvo Code couldn’t delete this desktop’s local identity.";
const CREDENTIALS: &str =
    "Kuvo Code couldn’t clear all its keys and sign-ins. Run uninstall again.";
const MAIN_UNINSTALL: &str = "Choose another main computer first.";

const PROCESSES: &[(&str, &str)] = &[
    ("run/box.pid", "keeper"),
    ("run/engine.json", "engine"),
    ("run/agents.pid", "coding agents"),
    ("run/link.pid", "link"),
    ("run/serve.json", "server"),
    ("run/box-services.json", "box services"),
    ("run/browser.pid", "browser"),
];

pub struct Outcome {
    pub status: u8,
    pub value: Value,
    pub note: Option<String>,
}

pub fn run(home: &Home, check: bool) -> Outcome {
    let root = home.root();
    let retired_app = root
        .parent()
        .and_then(Path::file_name)
        .and_then(|part| part.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("run.kuvo.code"));
    if retired_app {
        return refusal(
            "Kuvo on this Mac runs in Kuvo Code's own folder: remove Kuvo Code itself (its background work goes with it).",
        );
    }
    match stopped_machine(home, check) {
        Ok(outcome) => outcome,
        Err(error) => refusal(&error.to_string()),
    }
}

pub(crate) fn log_cleanup(files: &Files, detail: impl std::fmt::Display) {
    use std::io::Write as _;
    let path = files.path("run/uninstall.log");
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    if let Ok(mut log) = options.open(path)
        && log.metadata().is_ok_and(|metadata| metadata.is_file())
    {
        let _ = writeln!(log, "{detail}");
    }
}

fn stopped_machine(home: &Home, check: bool) -> Result<Outcome, Error> {
    let root = home.root();
    if !home.bin().join("kuvo-start").is_file() && !home.run().is_dir() {
        return Ok(refusal("There’s no Kuvo Code box in this folder."));
    }
    if !fs::symlink_metadata(root).is_ok_and(|entry| entry.is_dir())
        || !fs::symlink_metadata(home.run()).is_ok_and(|entry| entry.is_dir())
        || layout::inspect(home)? != Layout::Current
    {
        return Ok(refusal(
            "Kuvo Code couldn’t confirm this installation’s folder.",
        ));
    }
    let files = Files::new(root.canonicalize().map_err(|source| at(root, source))?);
    if check {
        return run_check(&files, root).map(|value| Outcome {
            status: 0,
            value,
            note: None,
        });
    }
    validate_disk(&files)?;
    if let Some(link) = files.read::<Link>(LINK)?
        && needs_handover(&link)?
    {
        return Ok(refusal(MAIN_UNINSTALL));
    }
    let owner = files.path("run/.client-owner");
    let Ok(_owner) = OwnedLock::acquire(&owner, Duration::ZERO) else {
        return Ok(refusal(
            "Kuvo Code is still running. Stop it before uninstalling.",
        ));
    };
    let _link = claim(&files, "run/.link.lock")?;
    if layout::inspect(home)? != Layout::Current {
        return Ok(refusal(
            "Kuvo Code couldn’t confirm this installation’s folder.",
        ));
    }
    validate_disk(&files)?;
    let running = live_processes(&files)?;
    if !running.is_empty() {
        return Ok(refusal(
            "Kuvo Code is still running. Stop it before uninstalling.",
        ));
    }
    if files.path("bin/kuvo-start").exists() || files.path("run/wsl-task").exists() {
        return Ok(refusal(
            "Kuvo Code couldn’t confirm its startup entries were removed.",
        ));
    }
    let link: Option<Link> = files.read(LINK)?;
    if let Some(link) = link {
        // A member may have joined since the read before we obtained the lock.
        if needs_handover(&link)? {
            return Ok(refusal(MAIN_UNINSTALL));
        }
        if let Ok(setup) = link.setup() {
            let shared = link.member_list()?;
            if setup.role == Role::Member || shared.devices().next().is_some() {
                return Ok(refusal(
                    "Kuvo Code couldn’t confirm that your main computer revoked this desktop’s access.",
                ));
            }
        }
        if link
            .extra
            .get("phones")
            .and_then(Value::as_array)
            .is_some_and(|phones| !phones.is_empty())
            || ["localWipe", "localUnlink", "removalSource"]
                .iter()
                .any(|field| link.extra.get(*field).is_some_and(|value| !value.is_null()))
        {
            return Ok(refusal(
                "Kuvo Code couldn’t confirm this computer’s removal had finished.",
            ));
        }
    }
    if files.path("run/host.json").exists()
        || files.path("box/kind").is_file()
            && fs::read_to_string(files.path("box/kind")).is_ok_and(|text| text.trim() == "mac")
    {
        return Ok(refusal(
            "Kuvo Code couldn’t confirm it had stopped this Mac’s background work.",
        ));
    }
    if let Some(backup) = files.read::<Value>("backup/configuration.json")?
        && backup["enabled"] != false
    {
        return Ok(refusal(
            "The verified pre-wipe backup could not be confirmed.",
        ));
    }
    files.write_bytes("run/off", b"")?;
    for needed in ["run/serve.needed", "run/link.needed"] {
        remove_file_synced(&files.path(needed))
            .map_err(|source| at(&files.path(needed), source))?;
    }
    let identity = files.path("desktop");
    let desktop = remove_tree_synced(&identity);
    let credentials = cleanup::clear(&files).and_then(|()| {
        remove_file_synced(&files.path("run/launch.env"))?;
        remove_file_synced(&files.path("run/launch.notes"))
    });
    for error in [&desktop, &credentials]
        .into_iter()
        .filter_map(|result| result.as_ref().err())
    {
        eprintln!("uninstall cleanup: {error}");
    }
    let problem = match (desktop, credentials) {
        (Err(_), _) => Some(DESKTOP),
        (_, Err(_)) => Some(CREDENTIALS),
        (Ok(()), Ok(())) => None,
    };
    let note = problem.map_or_else(
        || STOPPED.to_owned(),
        |problem| format!("{STOPPED} {problem}"),
    );
    let complete = problem.is_none();
    Ok(Outcome {
        status: u8::from(!complete),
        value: json!({"result": if complete { "uninstalled" } else { "unfinished" }, "stopped": true,
            "launchAgent": false, "crontab": false, "startsAgain": [], "left": [root], "message": note}),
        note: Some(note),
    })
}

fn needs_handover(link: &Link) -> Result<bool, Error> {
    let Ok(setup) = link.setup() else {
        return Ok(false);
    };
    Ok(setup.role == Role::Primary
        && link
            .member_list()?
            .boxes
            .iter()
            .any(|peer| peer.key != setup.position.primary))
}

fn run_check(files: &Files, home: &Path) -> Result<Value, Error> {
    let link: Link = files.read(LINK)?.unwrap_or_default();
    let computers = link
        .member_list()
        .map_or(0, |roster| roster.boxes.len().saturating_sub(1));
    let phones = link
        .extra
        .get("phones")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let kind = fs::read_to_string(files.path("box/kind")).ok();
    let running = live_processes(files)?;
    let (bots, wakes) = scheduled_work(files)?;
    Ok(
        json!({"folder": home, "kind": kind.as_deref().map(str::trim).unwrap_or("machine"),
        "running": running, "starts": {"launchAgent": false, "crontab": false, "checked": false},
        "bots": bots, "wakes": wakes, "phones": phones, "computers": computers}),
    )
}

#[derive(Deserialize)]
struct MoveRecord {
    #[serde(rename = "id")]
    _id: String,
    #[serde(rename = "thing")]
    _thing: String,
    #[serde(rename = "peer")]
    _peer: String,
    role: MoveRole,
    stage: Option<String>,
    sessions: Vec<String>,
    #[serde(default)]
    failed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum MoveRole {
    From,
    To,
}

fn scheduled_work(files: &Files) -> Result<(usize, usize), Error> {
    let mut away = HashSet::new();
    for name in files.names("run/moves")? {
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if !named(id, "mov_") {
            continue;
        }
        let Ok(Some(record)) = files.read::<Value>(&format!("run/moves/{name}")) else {
            continue;
        };
        if record
            .get("formatVersion")
            .is_some_and(|version| !matches!(version.as_u64(), Some(0 | 1)))
        {
            return Err(Error::Version("move journal"));
        }
        let Ok(record) = serde_json::from_value::<MoveRecord>(record) else {
            continue;
        };
        let switched = matches!(record.stage.as_deref(), Some("switching" | "finishing"));
        if !record.failed && matches!(record.role, MoveRole::From) == switched {
            away.extend(record.sessions);
        }
    }
    let mut bots = 0;
    let mut wakes = 0;
    for collection in ["bots", "wakes"] {
        for name in files.names(collection)? {
            let folder = format!("{collection}/{name}");
            let Ok(session) = fs::read_to_string(files.path(&format!("{folder}/session"))) else {
                continue;
            };
            let session = session.trim();
            if session.is_empty() {
                continue;
            }
            if collection == "bots"
                && named(&name, "bot_")
                && let Ok(Some(bot)) = files.read::<Value>(&format!("{folder}/bot.json"))
                && bot["kuvo"] == 1
                && bot["id"] == name
                && bot["name"].is_string()
            {
                bots += 1;
            }
            if away.contains(session) {
                continue;
            }
            wakes += files
                .names(&format!("{folder}/wakes"))?
                .into_iter()
                .filter(|name| {
                    let due = name.split('-').next().unwrap_or_default();
                    let decimal = due.split_once('.');
                    let digits = |part: &str| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    };
                    decimal.map_or_else(
                        || digits(due),
                        |(whole, fraction)| digits(whole) && digits(fraction),
                    )
                })
                .count();
            wakes += files
                .names(&format!("{folder}/watches"))?
                .into_iter()
                .filter(|name| {
                    fs::read_to_string(files.path(&format!("{folder}/watches/{name}/check")))
                        .is_ok_and(|check| !check.is_empty())
                })
                .count();
        }
    }
    Ok((bots, wakes))
}

fn named(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

pub(crate) fn live_processes(files: &Files) -> Result<Vec<&'static str>, Error> {
    let mut running = Vec::new();
    for (file, role) in PROCESSES {
        let Some(record) = files.read::<Value>(file)? else {
            continue;
        };
        let Some(pid) = record
            .as_u64()
            .or_else(|| record["pid"].as_u64())
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 1)
        else {
            continue;
        };
        if pid != std::process::id()
            && ProcessIdentity::read(pid)
                .map_err(|source| at(&files.path(file), source))?
                .is_some()
        {
            running.push(*role);
        }
    }
    if !files.names("run/agents/hold")?.is_empty() {
        running.push("coding agents' processes");
    }
    Ok(running)
}

fn claim(files: &Files, path: &str) -> Result<OwnedLock, Error> {
    let path = files.path(path);
    OwnedLock::acquire(&path, Duration::from_secs(3)).map_err(|source| at(&path, source))
}

fn at(path: &Path, source: io::Error) -> Error {
    Error::File {
        action: "checking uninstall at",
        path: PathBuf::from(path),
        source,
    }
}

fn refusal(message: &str) -> Outcome {
    Outcome {
        status: 1,
        value: json!({"result": "refused", "message": message}),
        note: Some(message.into()),
    }
}
