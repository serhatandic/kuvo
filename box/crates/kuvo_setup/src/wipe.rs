//! The last headless computer can erase its own installation after its signed
//! removal decision and process shutdown. The journal follows the old inode.
//! On Windows a folder can't be renamed while the box holds files in it, so the
//! resident hands the erase to a copy of itself outside the folder, and the next
//! box to start deletes that copy.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use kuvo_os::platform::{Account, Platform};
use kuvo_store::{
    files,
    home::Home,
    layout::{self, Layout},
    locks::OwnedLock,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    actor::{LINK, validate_disk},
    model::{self, Identity, Link, Role, SharedKind, Status},
    storage::Files,
    uninstall,
};

pub(crate) const UNSUPPORTED: &str = "This computer’s data layout cannot be wiped from here.";

#[derive(Deserialize, Serialize)]
pub(crate) struct LocalWipe {
    pub(crate) setup: String,
    pub(crate) operation: String,
}

impl LocalWipe {
    pub(crate) fn from(link: &Link) -> Option<Self> {
        let kept = link.extra.get("localWipe")?;
        if kept.get("primary").is_some()
            || kept.get("backup").is_some()
            || kept.get("generation").is_some()
            || kept.get("requiresPreparation").is_some()
        {
            return None;
        }
        let wipe: Self = serde_json::from_value(kept.clone()).ok()?;
        let setup = link.setup().ok()?;
        let own = Identity::read(link).ok()?;
        let roster = link.member_list().ok()?;
        let decision = link.shared_outcomes.get(&wipe.operation)?;
        let accepted = link.shared.get(&SharedKind::MembershipAndAccess)?;
        let saved = roster.extra.get("removals")?.get(&own.public)?;
        (setup.role == Role::Primary
            && setup.position.primary == own.public
            && setup.sign == own.sign
            && setup.position.id == wipe.setup
            && roster.boxes.len() == 1
            && model::safe_token(&wipe.operation, 22, 43)
            && decision.kind == SharedKind::MembershipAndAccess
            && decision.epoch == setup.position.epoch
            && decision.status == Status::Committed
            && decision.sequence == accepted.sequence
            && decision.proposal == model::fingerprint(&serde_json::json!({
                "kind": SharedKind::MembershipAndAccess, "value": accepted.value, "operation": wipe.operation,
            }))
            && saved["operation"] == wipe.operation
            && saved["local"] == true
            && saved["wipe"] == true
            && saved.get("plan").is_none()
            && saved.get("resolution").is_none())
        .then_some(wipe)
    }
}

fn boundary(home: &Path) -> io::Result<Option<PathBuf>> {
    let platform = Platform::current();
    if std::env::var_os("KUVO_BOX")
        .as_deref()
        .is_some_and(|v| v == "mac")
        || home.file_name().is_none_or(|part| part != ".kuvo-code")
    {
        return Ok(None);
    }
    let account = Account::current()?
        .home_folder(platform, &std::env::vars_os().collect())?
        .canonicalize()?;
    let actual = parent(home)?.canonicalize()?;
    Ok((account == actual).then_some(actual.join(".kuvo-code")))
}

pub(crate) fn check(home: &Path) -> io::Result<()> {
    let Some(root) = boundary(home)? else {
        return Err(io::Error::other(UNSUPPORTED));
    };
    let folder = Home::new(root.clone())?;
    if !fs::symlink_metadata(&root)?.is_dir()
        || !fs::symlink_metadata(folder.run())?.is_dir()
        || layout::inspect(&folder).map_err(io::Error::other)? != Layout::Current
    {
        return Err(io::Error::other(UNSUPPORTED));
    }
    for marker in [
        "bin/kuvo-start",
        "run/keeper.json",
        "run/wsl-task",
        "run/host.json",
    ] {
        match fs::symlink_metadata(root.join(marker)) {
            Ok(_) => {
                return Err(io::Error::other(
                    "Kuvo Code couldn’t confirm its startup entries were removed.",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    #[cfg(windows)]
    if kuvo_keeper::windows::StartupTask::for_home(&root)?.registered()? {
        return Err(io::Error::other(
            "Kuvo Code couldn’t confirm its startup entries were removed.",
        ));
    }
    if std::env::var_os("KUVO_KEYS_ON_HOST").is_some_and(|value| value == "1")
        || std::env::var_os("KUVO_HOST_URL").is_some()
        || fs::read_to_string(root.join("box/kind")).is_ok_and(|kind| kind.trim() == "mac")
    {
        return Err(io::Error::other(UNSUPPORTED));
    }
    let records = Files::new(root);
    if let Some(backup) = records
        .read::<Value>("backup/configuration.json")
        .map_err(io::Error::other)?
        && backup["enabled"] != false
    {
        return Err(io::Error::other(
            "The verified pre-wipe backup could not be confirmed.",
        ));
    }
    let running = uninstall::live_processes(&records).map_err(io::Error::other)?;
    if !running.is_empty() {
        return Err(io::Error::other(
            "Kuvo Code is still running. Stop it before wiping this computer.",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Phase {
    Stopping,
    Erasing,
    Detached,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    setup: String,
    operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<Phase>,
    #[serde(default)]
    format_version: u8,
}

pub fn pending(home: &Path) -> io::Result<bool> {
    let Some(root) = boundary(home)? else {
        return Ok(false);
    };
    pending_link(&Files::new(root)).map(|wipe| wipe.is_some())
}

fn pending_link(records: &Files) -> io::Result<Option<LocalWipe>> {
    let Ok(Some(raw)) = records.read::<Value>(LINK) else {
        return Ok(None);
    };
    if !raw.get("localWipe").is_some_and(Value::is_object) {
        return Ok(None);
    }
    let link: Link = serde_json::from_value(raw)?;
    Ok(LocalWipe::from(&link))
}

fn read_journal(receipts: &Path) -> io::Result<Option<Journal>> {
    let marker = receipts.join("last-setup-wipe");
    if !private(receipts, true)? || !private(&marker, false)? {
        return Ok(None);
    }
    let bytes = fs::read(&marker)?;
    if bytes.len() > 220 {
        return Err(io::Error::other("Invalid directory wipe receipt"));
    }
    let kept: Journal = serde_json::from_slice(&bytes)?;
    let setup = kept
        .setup
        .strip_prefix("set_")
        .is_some_and(|part| model::safe_token(part, 22, 22));
    let source = kept
        .source
        .as_deref()
        .and_then(|id| id.split_once(':'))
        .is_some_and(|(dev, file)| dev.parse::<u64>().is_ok() && file.parse::<u128>().is_ok());
    if kept.format_version > 1
        || !setup
        || !model::safe_token(&kept.operation, 22, 43)
        || kept.phase.is_some() != source
        || kept.phase.is_some() != kept.source.is_some()
    {
        return Err(io::Error::other("Invalid directory wipe receipt"));
    }
    Ok(Some(kept))
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ReinstallReceipt {
    setup: String,
    operation: String,
}

pub enum InstallerWipe {
    None,
    Waiting,
    Complete(ReinstallReceipt),
}

impl InstallerWipe {
    pub fn waiting(&self) -> bool {
        matches!(self, Self::Waiting)
    }

    pub fn receipt(&self) -> Option<&ReinstallReceipt> {
        match self {
            Self::Complete(receipt) => Some(receipt),
            _ => None,
        }
    }
}

/// Installer status can resume the existing erase. The recorded operation and
/// directory identity remain the sole authority in `finish`; a marker alone
/// can make the installer wait, but cannot authorize deletion.
pub fn installer_wipe(home: &Path, resume: bool) -> io::Result<InstallerWipe> {
    let Some(root) = boundary(home)? else {
        return Ok(InstallerWipe::None);
    };
    let receipts = parent(&root)?.join(".kuvo-code-removal-receipts");
    if resume {
        // A live owner or an incomplete erase leaves its receipt for retry.
        let _ = finish(&root);
    }
    let kept = read_journal(&receipts)?;
    let records = Files::new(root);
    let pending = records
        .read::<Value>(LINK)
        .ok()
        .flatten()
        .is_some_and(|link| {
            let setup = &link["setup"];
            let wipe = &link["localWipe"];
            setup["role"] == "primary"
                && wipe.get("primary").is_none()
                && setup["id"].as_str().is_some_and(|id| {
                    id.strip_prefix("set_")
                        .is_some_and(|part| model::safe_token(part, 22, 22))
                        && wipe["setup"] == id
                })
                && wipe["operation"]
                    .as_str()
                    .is_some_and(|operation| model::safe_token(operation, 22, 43))
        });
    if pending || kept.as_ref().is_some_and(|journal| journal.phase.is_some()) {
        return Ok(InstallerWipe::Waiting);
    }
    Ok(kept.map_or(InstallerWipe::None, |journal| {
        InstallerWipe::Complete(ReinstallReceipt {
            setup: journal.setup,
            operation: journal.operation,
        })
    }))
}

/// Only a successful reinstall consumes the completed receipt it observed.
pub fn finish_reinstall(home: &Path, receipt: &ReinstallReceipt) -> io::Result<()> {
    let Some(root) = boundary(home)? else {
        return Ok(());
    };
    let parent = parent(&root)?;
    let receipts = parent.join(".kuvo-code-removal-receipts");
    let _lock = OwnedLock::acquire(&parent.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
    if read_journal(&receipts)?.is_some_and(|journal| {
        journal.phase.is_none()
            && journal.setup == receipt.setup
            && journal.operation == receipt.operation
    }) {
        files::remove_file_synced(&receipts.join("last-setup-wipe"))?;
    }
    Ok(())
}

/// Complete an already-authorized wipe when no other resident owns the home.
/// Returning true tells startup that it should leave the erased home alone.
pub fn finish(home: &Path) -> io::Result<bool> {
    let Some(root) = boundary(home)? else {
        return Ok(false);
    };
    let parent = parent(&root)?;
    let receipts = parent.join(".kuvo-code-removal-receipts");
    let marker = receipts.join("last-setup-wipe");
    #[cfg(windows)]
    forget_erasers(&receipts);
    let records = Files::new(root.clone());
    let pending = || pending_link(&records);
    let read = || read_journal(&receipts);
    let current = read()?;
    if current.as_ref().is_none_or(|kept| kept.phase.is_none()) && pending()?.is_none() {
        return Ok(current.is_some() && !root.exists());
    }
    let _boundary = OwnedLock::acquire(&parent.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
    let mut kept = read()?;
    let save = |journal: &Journal| -> io::Result<()> {
        files::create_private_dirs(&receipts)?;
        // Windows makes the folder with the profile's ACL; close it as Unix makes it 0700.
        #[cfg(windows)]
        files::private_folder(&receipts)?;
        private(&receipts, true)?;
        files::write_atomic_through(&marker, &serde_json::to_vec(journal)?, parent)
    };
    let mut guard = None;
    if kept
        .as_ref()
        .is_none_or(|kept| matches!(kept.phase, None | Some(Phase::Stopping)))
    {
        check(&root)?;
        let owner = OwnedLock::acquire(&records.path("run/.client-owner"), Duration::ZERO)?;
        let link = OwnedLock::acquire(&records.path("run/.link.lock"), Duration::ZERO)?;
        validate_disk(&records).map_err(io::Error::other)?;
        let Some(wipe) = pending()? else {
            return Ok(false);
        };
        check(&root)?;
        let source =
            identity(&root)?.ok_or_else(|| io::Error::other("The folder to erase is missing."))?;
        if kept.as_ref().is_some_and(|kept| {
            kept.phase.is_some()
                && (kept.setup != wipe.setup
                    || kept.operation != wipe.operation
                    || kept.source.as_ref() != Some(&source))
        }) {
            return Err(io::Error::other(
                "A different directory erase is still pending.",
            ));
        }
        let approval = Journal {
            setup: wipe.setup,
            operation: wipe.operation,
            source: Some(source),
            phase: Some(Phase::Erasing),
            format_version: 1,
        };
        records
            .write_bytes("run/off", b"")
            .map_err(io::Error::other)?;
        save(&approval)?;
        guard = Some((owner, link));
        kept = Some(approval);
    }
    let Some(mut journal) = kept else {
        return Ok(false);
    };
    let target = receipts.join(format!("erase-{}", journal.operation));
    let old = identity(&target)?;
    if old
        .as_ref()
        .is_some_and(|found| Some(found) != journal.source.as_ref())
    {
        return Err(io::Error::other("The folder to erase has changed."));
    }
    if journal.phase == Some(Phase::Erasing) {
        if old.is_none() {
            if identity(&root)? != journal.source {
                return Err(io::Error::other("The folder to erase has changed."));
            }
            // Windows can't rename a folder holding open files, these locks included.
            // The wipe lock beside the folder still keeps other boxes out.
            #[cfg(windows)]
            drop(guard.take());
            files::replace_synced(&root, &target)?;
        }
        journal.phase = Some(Phase::Detached);
        save(&journal)?;
    }
    // Neither an installer nor another startup should remove locks from a new
    // installation at the now-vacant path. OwnedLock drops only its own token.
    drop(guard);
    files::remove_tree_synced(&target)?;
    journal.source = None;
    journal.phase = None;
    save(&journal)?;
    Ok(true)
}

fn identity(path: &Path) -> io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => (),
        Ok(_) => return Err(io::Error::other("The folder to erase must be a directory.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    Ok(files::file_stamp(path)?.map(|stamp| format!("{}:{}", stamp.device, stamp.file)))
}

fn private(path: &Path, directory: bool) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(info)
            if (if directory {
                info.is_dir()
            } else {
                info.is_file()
            }) && closed(path, &info)? =>
        {
            Ok(true)
        }
        Ok(_) => Err(io::Error::other(
            "The wipe receipt folder must be private to this account.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn closed(path: &Path, info: &fs::Metadata) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(info.mode() & 0o077 == 0 && info.uid() == fs::metadata(parent(path)?)?.uid())
}

#[cfg(windows)]
fn closed(path: &Path, _info: &fs::Metadata) -> io::Result<bool> {
    kuvo_os::windows::is_private(path)
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent().ok_or_else(|| io::Error::other(UNSUPPORTED))
}

/// Hand an authorized wipe to a copy of this program outside the folder it erases, which
/// starts once this process has exited. Its own copy is deleted by the next box to start.
#[cfg(windows)]
pub fn hand_over(home: &Path) -> io::Result<bool> {
    use std::os::windows::process::CommandExt as _;
    let Some(root) = boundary(home)? else {
        return Ok(false);
    };
    let receipts = parent(&root)?.join(".kuvo-code-removal-receipts");
    files::create_private_dirs(&receipts)?;
    files::private_folder(&receipts)?;
    let eraser = receipts.join(format!("{ERASER}{}", std::process::id()));
    fs::create_dir_all(&eraser)?;
    let program = std::env::current_exe()?;
    for entry in fs::read_dir(parent(&program)?)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
        {
            fs::copy(&path, eraser.join(path.file_name().unwrap_or_default()))?;
        }
    }
    let copy = eraser.join("kuvo-box.exe");
    fs::copy(&program, &copy)?;
    let mut command = std::process::Command::new(copy);
    command
        .arg("wipe-member")
        .env("KUVO_HOME", &root)
        .env(AFTER, std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, DETACHED_PROCESS,
    };
    // Outlive this process's job when it allows that; a test harness may not.
    let flags = CREATE_NO_WINDOW | DETACHED_PROCESS;
    command
        .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| command.creation_flags(flags).spawn())?;
    Ok(true)
}

#[cfg(windows)]
const ERASER: &str = "eraser-";
/// The process an eraser waits for before it starts.
#[cfg(windows)]
pub const AFTER: &str = "KUVO_WIPE_AFTER";

/// Wait for the resident that handed over the erase to exit.
#[cfg(windows)]
pub fn wait_for_hand_over() {
    use kuvo_os::process::ProcessIdentity;
    let Some(pid) = std::env::var(AFTER).ok().and_then(|pid| pid.parse().ok()) else {
        return;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while ProcessIdentity::read(pid).is_ok_and(|live| live.is_some())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Erasers that have finished: each one's program is free once it has exited.
#[cfg(windows)]
fn forget_erasers(receipts: &Path) {
    let own = format!("{ERASER}{}", std::process::id());
    for entry in fs::read_dir(receipts).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(ERASER) && name != own)
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}
