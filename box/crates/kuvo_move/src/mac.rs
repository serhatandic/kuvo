//! Prepare an unpublished copy of an older Mac app home.

use crate::{Disk, Error, entries, metadata};
use kuvo_store::{files, format, home::Home, layout};
use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

mod artifacts;
mod check;
mod counts;
mod database;
mod engine;
mod letters;
mod paths;
mod preferences;
mod publication;
mod snapshots;
mod source;

pub use publication::{CutoverStep, Outcome, Run, blocks_open, settle_started};

const STAGE: &str = "updates/rehome/rust-stage";
const BESIDE: &[(&str, &str)] = &[
    ("opencode/data", "data"),
    ("opencode/config", "config"),
    ("opencode/state", "state"),
    ("opencode/cache", "cache"),
    ("opencode/workspace", "workspace"),
    ("integrations", "integrations"),
    ("bots", "bots"),
    ("wakes", "wakes"),
    ("groups", "groups"),
    ("Chats", "chats"),
    ("agents", "agents"),
    ("phone-link.json", "phone-link.json"),
    ("bots.json", "bots.json"),
    ("opencode-files", "opencode-files"),
    ("Attachments", "attachments"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppStep {
    Clone,
    Import,
    Database,
    Rewrite,
    Check,
}

#[derive(Debug)]
pub struct StagedApp {
    pub path: PathBuf,
    pub dropped_letters: usize,
    pub dropped_waiting_messages: u64,
    counts: counts::Comparison,
    leftovers: Vec<String>,
}

/// Prepare the source and the pinned engine's database for the Mac import.
/// The caller owns the installer's staging and the app must be stopped.
/// This entry point never publishes the home layout.
pub fn stage(
    destination: &Home,
    account: &Path,
    bundle: &Path,
    release: &Path,
    engine_dir: &Path,
    completed: impl FnMut(AppStep),
) -> Result<StagedApp, Error> {
    let _guard = layout::lock_for_move(destination, Duration::from_secs(3))?;
    prepare(destination, account, bundle, release, engine_dir, completed)
}

fn prepare(
    destination: &Home,
    account: &Path,
    bundle: &Path,
    release: &Path,
    engine_dir: &Path,
    mut completed: impl FnMut(AppStep),
) -> Result<StagedApp, Error> {
    if [account, bundle, release, engine_dir]
        .iter()
        .any(|path| !path.is_absolute())
    {
        return Err(invalid("The app move needs absolute paths."));
    }
    let id = bundle_id(bundle)?;
    let source = account.join("Library/Application Support").join(id);
    let origin = source.canonicalize()?;
    let target = destination.root().canonicalize()?;
    if origin.starts_with(&target) || target.starts_with(&origin) {
        return Err(invalid("The app and destination folders overlap."));
    }
    let disk = Disk(target);
    let layout = layout::inspect(destination)?;
    if !matches!(
        layout,
        layout::Layout::Unmarked | layout::Layout::Older { version: 0 }
    ) {
        return Err(Error::UnknownHome(destination.root().into()));
    }
    for entry in entries(&disk.0)? {
        let name = entry.file_name();
        if name != OsStr::new("updates")
            && name != OsStr::new("desktop")
            && name != OsStr::new(layout::FILE)
            && !name.to_string_lossy().starts_with(".folder-upgrade.lock")
        {
            return Err(Error::UnknownHome(destination.root().into()));
        }
    }
    let journal = disk.path("updates/rehome/journal.json")?;
    if metadata(&journal)?.is_some() {
        return Err(Error::PreviousMove(journal));
    }
    source::validate_legacy(&source)?;
    let source_box = source.join("box");
    let source_disk = Disk(source_box.clone());
    if metadata(&source_box)?.is_some_and(|meta| !meta.is_dir()) {
        return Err(invalid("The app's box path must be a directory."));
    }
    source::require_quiet(&source_disk)?;
    let input = source::stamp(&source)?;
    engine::check_marker_path(&source_box)?;
    let engine = checked_engine(release, engine_dir)?;

    let staging = disk.path(STAGE)?;
    let release = release.canonicalize()?;
    let engine_dir = engine_dir.canonicalize()?;
    if [&release, &engine_dir]
        .iter()
        .any(|path| path.starts_with(&staging))
    {
        return Err(invalid(
            "The release and engine must be outside the app staging tree.",
        ));
    }
    // An unpublished copy is always rebuilt. That keeps a previous run's
    // partly migrated database out of any later attempt.
    files::remove_tree_synced(&staging)?;
    files::create_private_dirs(&staging)?;
    let staging_disk = Disk(staging.to_owned());
    for entry in entries(&source_box)? {
        let name = entry.file_name();
        if name == OsStr::new("run") {
            clone_run(&entry.path(), &staging.join("run"))?;
        } else if name != OsStr::new("bin") && name != OsStr::new(layout::FILE) {
            clone(&entry.path(), &staging.join(name), false)?;
        }
    }
    for (from, to) in BESIDE {
        let path = source.join(from);
        if metadata(&path)?.is_some() {
            clone(&path, &staging.join(to), false)?;
        }
    }
    clone(&release.join("bin"), &staging.join("bin"), true)?;
    for entry in entries(&source_box.join("bin"))? {
        let path = staging.join("bin").join(entry.file_name());
        if metadata(&path)?.is_none() && !entry.file_type()?.is_symlink() {
            clone(&entry.path(), &path, false)?;
        }
    }
    clone(&engine_dir, &staging.join("engine"), true)?;
    files::sync_tree(&staging, destination.root().canonicalize()?.as_path())?;
    completed(AppStep::Clone);
    let before = counts::read(&staging)?;

    let plugin = "config/opencode/plugins/kuvo-code.js";
    let release_plugin = release.join(plugin);
    if metadata(&release_plugin)?.is_some_and(|meta| meta.is_file()) {
        let path = staging_disk.path(plugin)?;
        files::remove_file_synced(&path)?;
        clone(&release_plugin, &path, true)?;
    }
    preferences::take(&source, &staging_disk, account, destination.root())?;
    let dropped_letters = letters::upgrade(&source, &staging_disk)?;
    let welcome = staging_disk.path("run/welcomed.json")?;
    if metadata(&welcome)?.is_none() {
        files::write_new(&welcome, br#"{"moved":true,"formatVersion":1}"#)?;
    }
    files::sync_tree(&staging, &disk.0)?;
    completed(AppStep::Import);

    let staged_engine = staging.join("engine").join(engine);
    let data = staging.join("data");
    let work = disk.path("updates/rehome/rust-engine-check")?;
    files::remove_tree_synced(&work)?;
    let result = engine::migrate(&staged_engine, &data, &work);
    files::remove_tree_synced(&work)?;
    let dropped_waiting_messages = result?;
    files::sync_tree(&staging, &disk.0)?;
    source::require_quiet(&source_disk)?;
    completed(AppStep::Database);
    let mut placements: Vec<_> = BESIDE
        .iter()
        .map(|(from, to)| (source.join(from), destination.root().join(to)))
        .collect();
    placements.push((source_box, destination.root().into()));
    let map = paths::PathMap::new(
        placements
            .iter()
            .map(|(from, to)| (from.as_path(), to.as_path())),
    );
    let databases = engine::databases(&data)?;
    let changes = databases
        .iter()
        .map(|file| database::rewrite(file, &map))
        .collect::<Result<_, _>>()?;
    snapshots::rewrite(&Disk(data.join("opencode")), &map, changes)?;
    for name in [
        "config/opencode/opencode.json",
        "box/projects.json",
        "box/worktrees.json",
    ] {
        let file = staging_disk.path(name)?;
        if metadata(&file)?.is_some_and(|meta| meta.is_file())
            && let Some(bytes) = files::read(&file)?
            && let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && map.json(&mut value)
        {
            let next = paths::serialize_json(&value, &bytes).map_err(|source| Error::Input {
                path: file.clone(),
                source,
            })?;
            files::rewrite_mac_entry(&file, &next)?;
        }
    }
    artifacts::rewrite(&staging_disk, &map)?;
    paths::repoint_links(&staging, &map)?;
    files::sync_tree(&staging, &disk.0)?;
    completed(AppStep::Rewrite);
    let result = check::validate(&staged_engine, &data, &work);
    files::remove_tree_synced(&work)?;
    result?;
    let counts = counts::Comparison::new(before, counts::read(&staging)?)?;
    let leftovers = databases
        .iter()
        .map(|file| database::leftovers(file, &map))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    source::require_quiet(&source_disk)?;
    if source::stamp(&source)? != input {
        return Err(invalid("The app's data changed during the staged move."));
    }
    completed(AppStep::Check);
    Ok(StagedApp {
        path: staging,
        dropped_letters,
        dropped_waiting_messages,
        counts,
        leftovers,
    })
}

fn bundle_id(bundle: &Path) -> Result<String, Error> {
    let result = Command::new("/usr/bin/plutil")
        .env_clear()
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()?;
    let id = String::from_utf8_lossy(&result.stdout);
    let id = id.trim();
    if !result.status.success()
        || !id.starts_with("run.kuvo.code")
        || id.ends_with('.')
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.'))
    {
        return Err(invalid("The app bundle has an invalid identifier."));
    }
    Ok(id.to_owned())
}

fn checked_engine(release: &Path, engine_dir: &Path) -> Result<PathBuf, Error> {
    let manifest = format::BOX_FILES.read(&release.join("box.json"))?;
    let version = manifest
        .as_ref()
        .and_then(|record| record.fields.get("engine"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.as_bytes()[0].is_ascii_alphanumeric()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
        })
        .ok_or_else(|| invalid("The release does not name a valid engine."))?;
    for path in [
        release.join("bin/kuvo-start"),
        release.join("bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper"),
        engine_dir.join(version).join("opencode"),
    ] {
        if !fs::metadata(&path)?.is_file() {
            return Err(Error::UnknownHome(path));
        }
    }
    Ok(Path::new(version).join("opencode"))
}

fn clone(from: &Path, to: &Path, installed: bool) -> Result<(), Error> {
    if metadata(to)?.is_some() {
        // Placement merges and their path rewrites must be planned together.
        return Err(Error::Busy(to.into()));
    }
    files::clone_mac_entry(from, to, installed)?;
    Ok(())
}

fn clone_run(from: &Path, to: &Path) -> Result<(), Error> {
    if !metadata(from)?.is_some_and(|meta| meta.is_dir()) {
        return Err(invalid("The app's run path must be a directory."));
    }
    files::create_private_dirs(to)?;
    for entry in entries(from)? {
        let name = entry.file_name();
        let kind = entry.file_type()?;
        if !process_record(&name.to_string_lossy())
            && (kind.is_file() || kind.is_dir() || kind.is_symlink())
        {
            clone(&entry.path(), &to.join(name), false)?;
        }
    }
    Ok(())
}

fn process_record(name: &str) -> bool {
    name.starts_with("keeper.")
        || name.ends_with(".pid")
        || name.ends_with(".lock")
        || matches!(
            name,
            "serve.json"
                | "served.json"
                | "host.json"
                | "engine.json"
                | "bridge.json"
                | "box-services.json"
                | ".box-services-start"
                | "usage-attribution.json"
                | "link.version"
                | "link.needed"
                | ".updating"
                | "socket.key"
                | ".system-asking"
                | "engine-output"
                | "engine-stopped"
                | "engine.retry"
                | "agents-stopped"
                | "agents.retry"
                | "agents.now"
                | "agents-renewing"
                | "repair.json"
                | "update-trial"
                | "update-restore"
        )
}

fn invalid(message: &str) -> Error {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}
