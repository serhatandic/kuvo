//! Signed installer handoff for an identified, stopped released machine.

use std::{
    ffi::OsString,
    fs, io,
    path::Path,
    process::{Command, ExitCode, Stdio},
    time::Duration,
};

use kuvo_keeper::{
    manifest::{SignedRelease, UPDATE_KEY, local_target},
    staging::{self, MAX_ENVELOPE},
};
use kuvo_os::{
    platform::{Account, Platform},
    process::ProcessIdentity,
};
use kuvo_store::{
    files,
    home::Home,
    layout::{self, Layout},
    locks,
};
use serde_json::{Value, json};

const JOURNAL: &str = "released-machine.json";

pub fn run(args: &[OsString]) -> ExitCode {
    match upgrade(args) {
        Ok(started) => {
            println!("{}", json!({"result":"done", "started":started}));
            ExitCode::SUCCESS
        }
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::WouldBlock) =>
        {
            println!("{}", json!({"result":"waits", "message":error.to_string()}));
            ExitCode::from(4)
        }
        Err(error) => {
            println!(
                "{}",
                json!({"result":"failed", "message":error.to_string()})
            );
            ExitCode::FAILURE
        }
    }
}

fn upgrade(args: &[OsString]) -> Result<bool, Box<dyn std::error::Error>> {
    let [source, identity] = args else {
        return Err(io::Error::other(
            "Expected the signed release folder and saved machine identity.",
        )
        .into());
    };
    let identity = identity
        .to_str()
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| io::Error::other("The saved machine identity is invalid."))?;
    let source = Path::new(source);
    if !source.is_absolute() {
        return Err(io::Error::other("Send the full path to the release folder.").into());
    }
    let account = Account::current()?;
    let platform = Platform::current();
    let profile = account
        .home_folder(platform, &std::env::vars_os().collect())?
        .canonicalize()?;
    let home = Home::new(profile.join(".kuvo-code"))?;
    let wrong = || io::Error::other("This account does not hold the saved installation.");
    if !fs::symlink_metadata(home.root())?.is_dir() {
        return Err(wrong().into());
    }
    let id = home.root().join("machine-id");
    let id_info = fs::symlink_metadata(&id)?;
    if !id_info.is_file() || id_info.len() > 64 || fs::read_to_string(id)?.trim() != identity {
        return Err(wrong().into());
    }
    let installer = home.updates().join("install");
    for folder in [
        home.updates(),
        installer.clone(),
        home.run(),
        home.settings(),
        home.root().join("engine"),
        home.root().join("versions"),
    ] {
        regular(&folder, true)?;
    }
    regular(&home.run().join("link.json"), false)?;
    regular(&home.root().join(layout::FILE), false)?;
    if files::text_of(&home.settings().join("kind")).is_some_and(|kind| kind.trim() == "mac")
        || home.run().join("off").exists()
    {
        return Err(wrong().into());
    }

    let envelope = staging::read_release(&source.join("boxes.json"), MAX_ENVELOPE)?;
    let test_key = kuvo_keeper::prepare_command::is_scratch(&home)?
        .then(|| std::env::var("KUVO_TEST_RELEASE_KEY").ok())
        .flatten();
    let signed = SignedRelease::verify(&envelope, &[test_key.as_deref().unwrap_or(UPDATE_KEY)])?;
    if !signed.launches_rust() {
        return Err(io::Error::other("The selected release does not start Rust.").into());
    }
    let engine = home.root().join("engine").join(signed.engine());
    regular(&engine, true)?;
    regular(&engine.join("opencode"), false)?;
    let selection = signed
        .reference(signed.pack_for(local_target()?)?)
        .target()?;
    let target = selection.to_string();
    let attempt = json!({"formatVersion":1, "identity":identity, "selection":target});
    let journal = installer.join(JOURNAL);
    let lock = home.updates().join(".install-media.lock");
    regular(&lock, false)?;
    files::create_private_dirs(&home.updates())?;
    let _guard = locks::try_file_lock(&lock)?;
    regular(&journal, false)?;
    let saved: Option<Value> = files::read(&journal)?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?;
    if saved.as_ref().is_some_and(|saved| saved != &attempt) {
        return Err(io::Error::other("A different installation move is still pending.").into());
    }
    if saved.is_none() {
        let layout = layout::inspect(&home)?;
        if layout == Layout::Current
            && fs::read_link(home.bin()).is_ok_and(|path| path == Path::new(&target))
        {
            return Ok(false);
        }
        let installed = files::text_of(&home.bin().join("box-version"));
        if !matches!(
            layout,
            Layout::Unmarked
                | Layout::Older { version: 0 | 1 }
                | Layout::Interrupted {
                    version: 0,
                    upgrading_to: 1
                }
        ) || installed
            .as_deref()
            .is_some_and(|version| !matches!(version.trim(), "0.7.2" | "0.8.0-beta.1"))
            || fs::symlink_metadata(home.bin()).is_ok_and(|entry| !entry.is_dir())
            || !home.run().join("link.json").is_file() && !home.bin().join("kuvo-start").is_file()
        {
            return Err(io::Error::other(
                "This installation does not need the released-machine move.",
            )
            .into());
        }
        check_stopped(&home)?;
        staging::prepare_install_selection(&home, &signed, source, &envelope)?;
        files::write_secret(&journal, &serde_json::to_vec(&attempt)?)?;
    }
    let selected = fs::read_link(home.bin()).is_ok_and(|path| path == Path::new(&target));
    if !selected {
        check_stopped(&home)?;
        kuvo_move::open(&home, platform, Some(&profile), |_| {})?;
        staging::select_initial(&home, &selection)?;
    }
    let launched = Command::new("/bin/sh")
        .arg(home.bin().join("kuvo-start"))
        .arg("box")
        .env("HOME", &profile)
        .current_dir(home.root())
        .stdin(Stdio::null())
        .status()?;
    if !launched.success() {
        return Err(io::Error::other("The installed keeper couldn’t start.").into());
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(wait_for_resident(&home))?;
    files::remove_file_synced(&journal)?;
    if let Err(error) = files::remove_empty_dir_synced(&installer)
        && error.kind() != io::ErrorKind::DirectoryNotEmpty
    {
        return Err(error.into());
    }
    Ok(true)
}

pub(super) fn regular(path: &Path, directory: bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(entry)
            if if directory {
                entry.is_dir()
            } else {
                entry.is_file()
            } =>
        {
            Ok(())
        }
        Ok(_) => Err(io::Error::other(format!(
            "{} is outside the saved installation.",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) async fn wait_for_resident(home: &Home) -> io::Result<()> {
    let file = home.run().join("serve.json");
    let listen = async {
        loop {
            if let Some(bytes) = files::read(&file)?
                && let Ok(record) = serde_json::from_slice::<kuvo_server::ServeRecord>(&bytes)
                && kuvo_keeper::resident_info(home.root(), record.pid)
                    .await?
                    .is_some()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), listen)
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "The installed keeper did not answer.",
            )
        })?
}

pub(super) fn check_stopped(home: &Home) -> io::Result<()> {
    if home.run().join(".launching").exists() || home.run().join("update-trial").exists() {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Another installation change is under way.",
        ));
    }
    for file in [
        "box.pid",
        "link.pid",
        "engine.json",
        "bridge.json",
        "agents.pid",
        "serve.json",
    ] {
        let path = home.run().join(file);
        regular(&path, false)?;
        let Some(bytes) = files::read(&path)? else {
            continue;
        };
        let value: Value = serde_json::from_slice(&bytes)?;
        let pid = value
            .as_u64()
            .or_else(|| value["pid"].as_u64())
            .and_then(|pid| u32::try_from(pid).ok());
        if let Some(pid) = pid.filter(|pid| *pid > 1)
            && ProcessIdentity::read(pid)?.is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Kuvo is still running on this computer.",
            ));
        }
    }
    Ok(())
}
