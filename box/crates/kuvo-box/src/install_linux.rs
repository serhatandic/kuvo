//! Fresh Linux installation from the signed media carried by kuvo-install.

use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::Duration,
};

use kuvo_keeper::{
    Release, crontab,
    manifest::{SignedRelease, UPDATE_KEY, local_target},
    staging,
};
use kuvo_move::install::{self, Verdict};
use kuvo_os::platform::{Account, Platform};
use kuvo_store::{files, home::Home, layout, locks};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::install_released::{check_stopped, regular, wait_for_resident};

const JOURNAL: &str = "fresh-machine.json";
const STAGED: &str = "fresh-machine-media";

#[derive(Deserialize, Serialize)]
struct Attempt {
    format_version: u8,
    selection: String,
    aside: String,
    entries: Vec<PathBuf>,
    preserved: bool,
    start_failed: bool,
}

impl Attempt {
    fn read(marker: &Path) -> io::Result<Option<Self>> {
        regular(marker, false)?;
        files::read(marker)?
            .map(|bytes| {
                let attempt: Self = serde_json::from_slice(&bytes)?;
                let valid_entry = |entry: &PathBuf| {
                    let mut parts = entry.components();
                    match (parts.next(), parts.next(), parts.next()) {
                        (Some(Component::Normal(name)), None, None) => {
                            name != "desktop"
                                && name != "trash"
                                && name != "updates"
                                && name != ".folder-upgrade.lock"
                        }
                        (Some(Component::Normal(folder)), Some(Component::Normal(name)), None) => {
                            folder == "updates"
                                && name != "install"
                                && name != "rehome"
                                && name != ".install-media.lock"
                        }
                        _ => false,
                    }
                };
                if attempt.format_version != 1
                    || !attempt.aside.starts_with("earlier-box-")
                    || Path::new(&attempt.aside)
                        .file_name()
                        .is_none_or(|name| name != attempt.aside.as_str())
                    || !attempt.entries.iter().all(valid_entry)
                {
                    return Err(io::Error::other(
                        "The interrupted installation marker is unreadable.",
                    ));
                }
                Ok(attempt)
            })
            .transpose()
    }

    fn write(&self, marker: &Path) -> io::Result<()> {
        files::write_secret(marker, &serde_json::to_vec(self)?)
    }
}

pub fn run(args: &[OsString]) -> ExitCode {
    match apply(args) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            println!(
                "{}",
                json!({"result":"failed", "message":error.to_string()})
            );
            ExitCode::FAILURE
        }
    }
}

fn apply(args: &[OsString]) -> Result<u8, Box<dyn std::error::Error>> {
    let [verb, source, name] = args else {
        return Err(
            io::Error::other("Expected an installer verb, release folder and home name.").into(),
        );
    };
    let source = Path::new(source);
    let name = Path::new(name);
    if !source.is_absolute() || name.file_name() != Some(name.as_os_str()) {
        return Err(io::Error::other("The release or Kuvo home has an invalid path.").into());
    }
    let account = Account::current()?;
    let profile = account
        .home_folder(Platform::Linux, &std::env::vars_os().collect())?
        .canonicalize()?;
    let home = Home::new(profile.join(name))?;
    let updates = home.updates();
    let install = updates.join("install");
    for directory in [home.root(), updates.as_path(), &install] {
        regular(directory, true)?;
    }
    let marker = install.join(JOURNAL);
    let judgement = install::judge(&home, None, Platform::Linux, None)?;
    let pending = Attempt::read(&marker)?.is_some() && judgement.verdict != Verdict::Waits;
    let verdict = if pending {
        Verdict::Fresh
    } else {
        judgement.verdict
    };
    if verb == "status" {
        let suggested = if pending {
            Some(install::Verb::New)
        } else {
            judgement.verb
        };
        println!(
            "{}",
            json!({"verdict":verdict, "verb":suggested, "status":judgement.status})
        );
        return Ok(0);
    }
    if verb == "move" {
        println!(
            "{}",
            json!({"result":"refused","verdict":verdict,
            "message":"There's nothing to move on this computer: Kuvo is where it belongs already.",
            "status":judgement.status})
        );
        return Ok(1);
    }
    if verb != "new" {
        return Ok(2);
    }
    if verdict != Verdict::Fresh {
        return Ok(refuse_new(verdict));
    }
    if name != Path::new(".kuvo-code") {
        return Ok(answer(
            "refused",
            verdict,
            "On Linux, Kuvo is always in ~/.kuvo-code.",
            1,
        ));
    }
    if !pending && let Some(code) = running_refusal(&home)? {
        return Ok(code);
    }
    let scratch = profile != account.home.canonicalize()?;
    let key = scratch
        .then(|| std::env::var("KUVO_TEST_RELEASE_KEY").ok())
        .flatten();
    let envelope = staging::read_release(&source.join("boxes.json"), staging::MAX_ENVELOPE)?;
    let signed = SignedRelease::verify(&envelope, &[key.as_deref().unwrap_or(UPDATE_KEY)])?;
    if !signed.launches_rust() {
        return Err(io::Error::other("The selected release does not start Rust.").into());
    }
    let selected = signed
        .reference(signed.pack_for(local_target()?)?)
        .target()?;
    files::create_private_dirs(&install)?;
    let install_lock = updates.join(".install-media.lock");
    regular(&install_lock, false)?;
    let _install = match locks::try_file_lock(&install_lock) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            return Ok(refuse_new(Verdict::Waits));
        }
        Err(error) => return Err(error.into()),
    };
    let staged = Home::new(install.join(STAGED))?;
    regular(staged.root(), true)?;
    let saved = Attempt::read(&marker)?;
    if let Some(attempt) = &saved {
        if attempt.selection != selected.to_string() {
            return Err(io::Error::other("A different installation is still pending.").into());
        }
    } else {
        let verdict = install::judge(&home, None, Platform::Linux, None)?.verdict;
        if verdict != Verdict::Fresh {
            return Ok(refuse_new(verdict));
        }
        if let Some(code) = running_refusal(&home)? {
            return Ok(code);
        }
    }
    let mut attempt = prepare(&home, &staged, &marker, &signed, source, &envelope, saved)?;
    let start = crontab::install(&home)?.is_some() || !scratch;
    let mut value = json!({"result":"done","verdict":"machine","started":start});
    if !attempt.entries.is_empty() {
        value["setAside"] = json!(home.root().join("trash").join(&attempt.aside));
    }
    if start && let Err(error) = start_keeper(&home, &profile) {
        attempt.start_failed = true;
        attempt.write(&marker)?;
        value["result"] = "failed".into();
        value["verdict"] = "fresh".into();
        value["started"] = false.into();
        value["message"] = format!(
            "Kuvo is installed on this computer, but couldn't be started: {error}. Try again."
        )
        .into();
        println!("{value}");
        return Ok(1);
    }
    // The selected home is ready. Retire earlier installer staging as well
    // so its unfinished-install marker cannot make this home look fresh.
    files::remove_tree_synced(&install)?;
    let result = install::judge(&home, None, Platform::Linux, None)?;
    value["verdict"] = json!(result.verdict);
    value["status"] = result.status;
    if !start {
        value["message"] = "It's made, and starts by itself only in ~/.kuvo-code.".into();
    }
    println!("{value}");
    Ok(0)
}

fn answer(result: &str, verdict: Verdict, message: &str, code: u8) -> u8 {
    println!(
        "{}",
        json!({"result":result, "verdict":verdict, "message":message})
    );
    code
}

fn refuse_new(verdict: Verdict) -> u8 {
    match verdict {
        Verdict::Waits => answer("waits", verdict, "Another move is under way.", 4),
        Verdict::Machine => answer(
            "refused",
            verdict,
            "This computer is set up as one of your machines already: Kuvo uses what's here.",
            1,
        ),
        _ => answer(
            "refused",
            verdict,
            "Kuvo on this computer is moving: that's finished first.",
            1,
        ),
    }
}

fn running_refusal(home: &Home) -> io::Result<Option<u8>> {
    match check_stopped(home) {
        Ok(()) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Some(answer(
            "refused",
            Verdict::Fresh,
            "Something is running in this computer's ~/.kuvo-code: Kuvo is installed there once that's stopped.",
            1,
        ))),
        Err(error) => Err(error),
    }
}

fn start_keeper(home: &Home, profile: &Path) -> io::Result<()> {
    let started = Command::new("/bin/sh")
        .arg(home.bin().join("kuvo-start"))
        .arg("box")
        .env("HOME", profile)
        .current_dir(home.root())
        .stdin(Stdio::null())
        .status()?;
    if !started.success() {
        return Err(io::Error::other("its keeper didn't start"));
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(wait_for_resident(home))
}

fn prepare(
    home: &Home,
    staged: &Home,
    marker: &Path,
    signed: &SignedRelease,
    source: &Path,
    envelope: &[u8],
    saved: Option<Attempt>,
) -> Result<Attempt, Box<dyn std::error::Error>> {
    let _move = layout::lock_for_move(home, Duration::from_secs(3))?;
    layout::inspect(home)?;
    let mut attempt = if let Some(attempt) = saved
        && !(attempt.start_failed && check_stopped(home).is_ok())
    {
        attempt
    } else {
        check_stopped(home)?;
        files::remove_tree_synced(staged.root())?;
        let selection = staging::prepare_install_selection(staged, signed, source, envelope)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(home.root())? {
            let name = entry?.file_name();
            if name != "desktop"
                && name != "trash"
                && name != "updates"
                && name != ".folder-upgrade.lock"
            {
                entries.push(PathBuf::from(name));
            }
        }
        for entry in fs::read_dir(home.updates())? {
            let name = entry?.file_name();
            if name != "install" && name != "rehome" && name != ".install-media.lock" {
                entries.push(Path::new("updates").join(name));
            }
        }
        entries.sort();
        let ago = jiff::Timestamp::now()
            .strftime("%Y-%m-%dT%H-%M-%S-%3fZ")
            .to_string();
        let attempt = Attempt {
            format_version: 1,
            selection: selection.to_string(),
            aside: format!("earlier-box-{ago}-{}", std::process::id()),
            entries,
            preserved: false,
            start_failed: false,
        };
        attempt.write(marker)?;
        attempt
    };
    let aside = home.root().join("trash").join(&attempt.aside);
    if !attempt.preserved {
        regular(&home.root().join("trash"), true)?;
        regular(&aside, true)?;
        regular(&aside.join("updates"), true)?;
        for entry in &attempt.entries {
            let original = home.root().join(entry);
            let saved = aside.join(entry);
            if has_entry(&saved)? {
                if has_entry(&original)? {
                    return Err(io::Error::other(
                        "The files changed while Kuvo was setting them aside.",
                    )
                    .into());
                }
                continue;
            }
            files::create_private_dirs(saved.parent().expect("saved parent"))?;
            files::replace_synced(&original, &saved)?;
        }
        attempt.preserved = true;
        attempt.write(marker)?;
    }
    for folder in ["engine", "versions"] {
        let from = staged.root().join(folder);
        let to = home.root().join(folder);
        if has_entry(&from)? {
            if has_entry(&to)? {
                return Err(
                    io::Error::other("The files changed while Kuvo was being installed.").into(),
                );
            }
            files::replace_synced(&from, &to)?;
        }
    }
    let selection: Release = attempt.selection.parse()?;
    staging::select_initial(home, &selection)?;
    layout::publish(home)?;
    prepare_launch(home)?;
    Ok(attempt)
}

fn has_entry(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn prepare_launch(home: &Home) -> Result<(), Box<dyn std::error::Error>> {
    let run = home.run();
    regular(&run, true)?;
    files::create_private_dirs(&run)?;
    let launch = run.join("launch.env");
    regular(&launch, false)?;
    if !has_entry(&launch)? {
        let mut env = String::from("OPENCODE_CLIENT=desktop\n");
        for (name, value) in std::env::vars_os() {
            if let (Some(name), Some(value)) = (name.to_str(), value.to_str())
                && (matches!(name, "PATH" | "SHELL" | "LANG") || name.starts_with("LC_"))
                && !value.contains('\n')
            {
                env.push_str(&format!("{name}={value}\n"));
            }
        }
        files::write_secret(&launch, env.as_bytes())?;
    }
    regular(&home.root().join("machine-id"), false)?;
    kuvo_setup::installer::ensure_machine_id(home)?;
    Ok(())
}
