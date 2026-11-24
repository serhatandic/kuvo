//! A verified removal can erase its installation after its writers stop. The
//! durable journal follows the directory's original inode.

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
    if platform == Platform::Windows
        || std::env::var_os("KUVO_BOX")
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

#[derive(Serialize)]
pub struct ReinstallReceipt {
    setup: String,
    operation: String,
}

pub enum InstallState {
    Pending,
    Erased(ReinstallReceipt),
}

/// The box supplies the vault operation. The path is the original App Support
/// folder even after it has been detached. Clearing the same identity again is safe.
pub trait AppWipeKeychain {
    fn clear_app_link_keys(&self, app_folder: &Path) -> io::Result<()>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppWipeKind {
    LastComputer,
    Member,
}

/// Recover the erasing or detached phase of an app-owned wipe. For a member,
/// finish clearing the app identity after detaching and before deleting its data.
pub fn resume_app_wipe(
    app_folder: &Path,
    account_home: &Path,
    kind: AppWipeKind,
    keychain: &dyn AppWipeKeychain,
) -> io::Result<bool> {
    let Some(root) = app_boundary(app_folder, account_home)? else {
        return Ok(false);
    };
    let parent = parent(&root)?;
    let id = root
        .file_name()
        .and_then(|id| id.to_str())
        .ok_or_else(|| io::Error::other(UNSUPPORTED))?;
    let receipts = parent.join(format!(".{id}-removal-receipts"));
    let marker = receipts.join(match kind {
        AppWipeKind::LastComputer => "last-setup-wipe",
        AppWipeKind::Member => "app-member-wipe",
    });
    let Some(journal) = read_journal(&marker)? else {
        return Ok(false);
    };
    if !matches!(journal.phase, Some(Phase::Erasing | Phase::Detached)) {
        return Ok(journal.phase.is_none() && !root.exists());
    }
    let _guard = OwnedLock::acquire(&parent.join(format!("{id}-wipe.lock")), Duration::ZERO)?;
    let Some(journal) = read_journal(&marker)? else {
        return Ok(false);
    };
    if !matches!(journal.phase, Some(Phase::Erasing | Phase::Detached)) {
        return Ok(journal.phase.is_none() && !root.exists());
    }
    erase_approved(
        &root,
        &receipts,
        journal,
        |journal| files::write_atomic_through(&marker, &serde_json::to_vec(journal)?, parent),
        || {
            if kind == AppWipeKind::Member {
                if identity(&root)?.is_some() {
                    return Err(io::Error::other(
                        "The app-owned Box’s identity could not be removed safely.",
                    ));
                }
                keychain.clear_app_link_keys(&root)?;
            }
            Ok(())
        },
    )?;
    Ok(true)
}

fn app_boundary(folder: &Path, account: &Path) -> io::Result<Option<PathBuf>> {
    if Platform::current() != Platform::Mac {
        return Ok(None);
    }
    let Some(id) = folder.file_name().and_then(|id| id.to_str()) else {
        return Ok(None);
    };
    let valid_id = id.strip_prefix("run.kuvo.code").is_some_and(|suffix| {
        suffix.is_empty()
            || suffix.strip_prefix('.').is_some_and(|parts| {
                parts.split('.').all(|part| {
                    !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            })
    });
    if !valid_id {
        return Ok(None);
    }
    let support = account.canonicalize()?.join("Library/Application Support");
    let actual = match parent(folder)?.canonicalize() {
        Ok(actual) => actual,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok((actual == support).then(|| support.join(id)))
}

impl InstallState {
    pub fn pending_message(&self) -> Option<&'static str> {
        match self {
            Self::Pending => Some("This computer’s wipe hasn’t finished."),
            Self::Erased(_) => None,
        }
    }
}

fn journal_path(root: &Path) -> io::Result<PathBuf> {
    Ok(parent(root)?
        .join(".kuvo-code-removal-receipts")
        .join("last-setup-wipe"))
}

fn read_journal(marker: &Path) -> io::Result<Option<Journal>> {
    if !private(parent(marker)?, true)? || !private(marker, false)? {
        return Ok(None);
    }
    let bytes = fs::read(marker)?;
    if bytes.len() > 220 {
        return Err(io::Error::other("Invalid directory wipe receipt"));
    }
    let kept: Journal = serde_json::from_slice(&bytes)?;
    let source = kept
        .source
        .as_deref()
        .and_then(|id| id.split_once(':'))
        .is_some_and(|(dev, file)| dev.parse::<u64>().is_ok() && file.parse::<u128>().is_ok());
    if kept.format_version > 1
        || !valid_removal(&kept.setup, &kept.operation)
        || kept.phase.is_some() != source
        || kept.phase.is_some() != kept.source.is_some()
    {
        return Err(io::Error::other("Invalid directory wipe receipt"));
    }
    Ok(Some(kept))
}

fn valid_removal(setup: &str, operation: &str) -> bool {
    setup
        .strip_prefix("set_")
        .is_some_and(|part| model::safe_token(part, 22, 22))
        && model::safe_token(operation, 22, 43)
}

/// An older link may fence installation before an authenticated erase grant
/// is available. The hint never authorizes deletion.
fn pending_hint(records: &Files) -> bool {
    let Ok(Some(raw)) = records.read::<Value>(LINK) else {
        return false;
    };
    let matches_setup = |kept: &Value| {
        kept["setup"]
            .as_str()
            .zip(kept["operation"].as_str())
            .is_some_and(|(setup, operation)| {
                raw["setup"]["id"] == setup && valid_removal(setup, operation)
            })
    };
    let wipe = &raw["localWipe"];
    match raw["setup"]["role"].as_str() {
        Some("primary") => matches_setup(wipe) && wipe.get("primary").is_none(),
        Some("member") => {
            matches_setup(&raw["removalSource"])
                || (matches_setup(wipe) && wipe["primary"].is_object())
        }
        _ => false,
    }
}

pub fn install_state(home: &Path) -> io::Result<Option<InstallState>> {
    let Some(root) = boundary(home)? else {
        return Ok(None);
    };
    let marker = journal_path(&root)?;
    let records = Files::new(root.clone());
    let mut journal = read_journal(&marker)?;
    if journal.as_ref().is_some_and(|kept| kept.phase.is_some()) || pending_hint(&records) {
        match finish(&root) {
            Ok(true) => journal = read_journal(&marker)?,
            Ok(false) => return Ok(Some(InstallState::Pending)),
            Err(error) => {
                eprintln!("finishing the installation's earlier wipe: {error}");
                return Ok(Some(InstallState::Pending));
            }
        }
    }
    Ok(journal.map(|kept| {
        if kept.phase.is_some() {
            InstallState::Pending
        } else {
            InstallState::Erased(ReinstallReceipt {
                setup: kept.setup,
                operation: kept.operation,
            })
        }
    }))
}

pub fn finish_reinstall(home: &Path) -> io::Result<()> {
    let Some(root) = boundary(home)? else {
        return Ok(());
    };
    let marker = journal_path(&root)?;
    if read_journal(&marker)?.is_some_and(|kept| kept.phase.is_none()) {
        let _guard =
            OwnedLock::acquire(&parent(&root)?.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
        if read_journal(&marker)?.is_some_and(|kept| kept.phase.is_none()) {
            files::remove_file_synced(&marker)?;
        }
    }
    Ok(())
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

/// Complete an already-authorized wipe when no other resident owns the home.
/// Returning true tells startup that it should leave the erased home alone.
pub fn finish(home: &Path) -> io::Result<bool> {
    let Some(root) = boundary(home)? else {
        return Ok(false);
    };
    let parent = parent(&root)?;
    let marker = journal_path(&root)?;
    let receipts = marker
        .parent()
        .ok_or_else(|| io::Error::other(UNSUPPORTED))?;
    let records = Files::new(root.clone());
    let pending = || pending_link(&records);
    let read = || read_journal(&marker);
    let current = read()?;
    if current.as_ref().is_none_or(|kept| kept.phase.is_none()) && pending()?.is_none() {
        return Ok(current.is_some() && !root.exists());
    }
    let _boundary = OwnedLock::acquire(&parent.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
    let mut kept = read()?;
    let save = |journal: &Journal| -> io::Result<()> {
        files::create_private_dirs(receipts)?;
        private(receipts, true)?;
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
    let Some(journal) = kept else {
        return Ok(false);
    };
    erase_approved(&root, receipts, journal, save, || {
        // OwnedLock drops only its own token, leaving any replacement alone.
        drop(guard);
        Ok(())
    })?;
    Ok(true)
}

fn erase_approved(
    root: &Path,
    receipts: &Path,
    mut journal: Journal,
    save: impl Fn(&Journal) -> io::Result<()>,
    after_detach: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let target = receipts.join(format!("erase-{}", journal.operation));
    let old = identity(&target)?;
    if old
        .as_ref()
        .is_some_and(|found| Some(found) != journal.source.as_ref())
    {
        return Err(io::Error::other("The folder to erase has changed."));
    }
    journal.format_version = 1;
    if journal.phase == Some(Phase::Erasing) {
        if old.is_none() {
            if identity(root)? != journal.source {
                return Err(io::Error::other("The folder to erase has changed."));
            }
            files::replace_synced(root, &target)?;
        }
        journal.phase = Some(Phase::Detached);
        save(&journal)?;
    }
    after_detach()?;
    files::remove_tree_synced(&target)?;
    journal.source = None;
    journal.phase = None;
    save(&journal)?;
    Ok(())
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

#[cfg(unix)]
fn private(path: &Path, directory: bool) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    match fs::symlink_metadata(path) {
        Ok(info)
            if (if directory {
                info.is_dir()
            } else {
                info.is_file()
            }) && info.mode() & 0o077 == 0
                && info.uid() == fs::metadata(parent(path)?)?.uid() =>
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

#[cfg(windows)]
fn private(_path: &Path, _directory: bool) -> io::Result<bool> {
    // A native keeper must perform the detach after the executable releases it.
    Err(io::Error::other(UNSUPPORTED))
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent().ok_or_else(|| io::Error::other(UNSUPPORTED))
}
