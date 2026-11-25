//! A verified removal can erase its installation after its writers stop. The
//! durable journal follows the directory's original inode.
//! On Windows a folder can't be renamed while the box holds files in it, so the
//! resident hands the erase to a copy of itself outside the folder, and the next
//! box to start deletes that copy.

#[cfg(target_os = "macos")]
mod app;
pub(crate) mod member;

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
        let own = Identity::read(link).ok()?;
        Self::accepted(link, &own.public, &own.sign)
    }

    fn accepted(link: &Link, key: &str, sign: &str) -> Option<Self> {
        let kept = link.extra.get("localWipe")?;
        if kept.get("primary").is_some()
            || kept.get("generation").is_some()
            || kept.get("requiresPreparation").is_some()
        {
            return None;
        }
        let wipe: Self = serde_json::from_value(kept.clone()).ok()?;
        let setup = link.setup().ok()?;
        let roster = link.member_list().ok()?;
        let decision = link.shared_outcomes.get(&wipe.operation)?;
        let accepted = link.shared.get(&SharedKind::MembershipAndAccess)?;
        let saved = roster.extra.get("removals")?.get(key)?;
        (setup.role == Role::Primary
            && setup.position.primary == key
            && setup.sign == sign
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

/// This computer can be wiped from here: its folder is the account's own, or the Mac app's.
pub(crate) fn supported_here(home: &Path) -> bool {
    #[cfg(target_os = "macos")]
    if std::env::var_os("KUVO_BOX").is_some_and(|kind| kind == "mac") {
        return app::Target::for_box(home).ok().flatten().is_some();
    }
    boundary(home).ok().flatten().is_some()
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

pub(crate) fn supports_member(home: &Path) -> io::Result<bool> {
    // A Windows member's erase isn't handed to a copy outside its folder yet.
    if cfg!(windows) {
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    if let Some(target) = app::Target::for_box(home)? {
        return Ok(target.check_layout(&app::Target::settings()?).is_ok());
    }
    let Some(root) = boundary(home)? else {
        return Ok(false);
    };
    Ok(fs::symlink_metadata(root)?.is_dir())
}

pub(crate) fn check(home: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    if let Some(target) = app::Target::for_box(home)? {
        return target.check_layout(&app::Target::settings()?);
    }
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

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ReinstallReceipt {
    setup: String,
    operation: String,
}

/// What an installer finds of an earlier wipe at its target.
pub enum InstallerWipe {
    None,
    Waiting,
    #[cfg(target_os = "macos")]
    MemberReview,
    Complete(ReinstallReceipt),
}

#[cfg(target_os = "macos")]
pub fn launch_app_wipe(home: &Path) -> io::Result<bool> {
    let Some(target) = app::Target::for_box(home)? else {
        return Ok(false);
    };
    if !target.pending()? {
        return Ok(false);
    }
    target.launch_worker()?;
    Ok(true)
}

#[cfg(target_os = "macos")]
pub async fn app_install_state(
    folder: &Path,
    account: &Path,
    keychain: &dyn AppWipeKeychain,
) -> io::Result<InstallerWipe> {
    match app::Target::for_folder(folder, account)? {
        Some(target) => target.installer_wipe(keychain).await,
        None => Ok(InstallerWipe::None),
    }
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

impl AppWipeKind {
    fn marker(self) -> &'static str {
        match self {
            Self::LastComputer => "last-setup-wipe",
            Self::Member => "app-member-wipe",
        }
    }
}

#[cfg(target_os = "macos")]
pub async fn finish_prepared_app_wipe(
    home: &Path,
    keychain: &dyn AppWipeKeychain,
) -> io::Result<bool> {
    match app::Target::for_box(home)? {
        Some(target) => target.finish(keychain, member::Report::Wipe).await,
        None => Ok(false),
    }
}

#[cfg(target_os = "macos")]
pub(crate) async fn prepare_app_wipe(
    home: &Path,
    link: &mut Link,
    authority: crate::SetupHandle,
) -> Result<(), crate::Error> {
    if let Some((kind, decision, member)) = app_decision(link)
        && let Some(target) = app::Target::for_box(home).map_err(app_error)?
        && !target.retained(kind, &decision).map_err(app_error)?
    {
        let settings = app::Target::settings().map_err(app_error)?;
        target.check_layout(&settings).map_err(app_error)?;
        let kept = target.box_files();
        prepare_configured_backup(
            &kept,
            target.root(),
            link,
            authority,
            model::ComputerKind::Mac,
        )
        .await
        .map_err(app_error)?;
        let Some(wipe) = link
            .extra
            .get_mut("localWipe")
            .and_then(Value::as_object_mut)
        else {
            return Err(crate::Error::Invalid("app-owned wipe approval"));
        };
        if app::helper_keeps_keys(&kept) && wipe.get("hostKeysCleared") != Some(&Value::Bool(true))
        {
            crate::mac_host::clear_integration_keys(&kept).await?;
            wipe.insert("hostKeysCleared".into(), Value::Bool(true));
        }
        wipe.insert("prepared".into(), Value::Bool(true));
        crate::actor::write_link(&kept, link, model::ComputerKind::Mac)?;
        target
            .prepare(kind, decision, member.as_ref(), link, &settings)
            .map_err(app_error)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn app_error(error: io::Error) -> crate::Error {
    crate::Error::Installer(error.to_string())
}

pub(crate) fn ready_for_shutdown(home: &Path, link: &Link) -> bool {
    if !link.extra.contains_key("localWipe") {
        return false;
    }
    #[cfg(target_os = "macos")]
    if std::env::var_os("KUVO_BOX").is_some_and(|kind| kind == "mac") {
        return app_decision(link)
            .zip(app::Target::for_box(home).ok().flatten())
            .is_some_and(|((kind, removal, _), target)| {
                target.retained(kind, &removal).unwrap_or(false)
            });
    }
    is_prepared(link)
        && (LocalWipe::from(link).is_some() || member_approval(link).is_some())
        && boundary(home).ok().flatten().is_some()
}

#[cfg(target_os = "macos")]
fn app_decision(link: &Link) -> Option<(AppWipeKind, LocalWipe, Option<member::MemberWipe>)> {
    if !link.extra.contains_key("localWipe") {
        return None;
    }
    if let Some(last) = LocalWipe::from(link) {
        return Some((AppWipeKind::LastComputer, last, None));
    }
    let own = Identity::read(link).ok()?;
    let member = member::MemberWipe::from(link, &own.public)?;
    let decision = LocalWipe {
        setup: member.setup.clone(),
        operation: member.operation.clone(),
    };
    Some((AppWipeKind::Member, decision, Some(member)))
}

fn member_approval(link: &Link) -> Option<member::MemberWipe> {
    if !is_prepared(link) {
        return None;
    }
    member::MemberWipe::from(link, &Identity::read(link).ok()?.public)
}

fn is_prepared(link: &Link) -> bool {
    link.extra
        .get("localWipe")
        .is_some_and(|wipe| wipe["prepared"] == true)
}

fn saved_backup(link: &Link) -> io::Result<Option<crate::backup::ConfiguredPoint>> {
    link.extra
        .get("localWipe")
        .and_then(|wipe| wipe.get("backup"))
        .map(|point| serde_json::from_value(point.clone()).map_err(io::Error::from))
        .transpose()
}

async fn prepare_configured_backup(
    kept: &Files,
    erase_boundary: &Path,
    link: &mut Link,
    authority: crate::SetupHandle,
    kind: model::ComputerKind,
) -> io::Result<()> {
    if saved_backup(link)?.is_none() {
        let home = kept.root().to_owned();
        let boundary = erase_boundary.to_owned();
        let point = tokio::task::spawn_blocking(move || {
            crate::backup::prepare_configured_disk(&home, &boundary, authority)
        })
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;
        if let Some(point) = point {
            let mut next = link.clone();
            next.extra["localWipe"]["backup"] = serde_json::to_value(point)?;
            crate::actor::write_link(kept, &next, kind).map_err(io::Error::other)?;
            *link = next;
        }
    }
    crate::backup::with_verified_configured_disk(
        kept.root(),
        erase_boundary,
        saved_backup(link)?.as_ref(),
        || Ok(()),
    )
}

pub(crate) async fn prepare_headless(
    home: &Path,
    link: &mut Link,
    authority: crate::SetupHandle,
) -> io::Result<()> {
    if !link.extra.contains_key("localWipe") || is_prepared(link) {
        return Ok(());
    }
    let Some(root) = boundary(home)? else {
        return Ok(());
    };
    let own = Identity::read(link).map_err(io::Error::other)?;
    let member = member::MemberWipe::from(link, &own.public);
    if member.is_none() && LocalWipe::from(link).is_none() {
        return Ok(());
    }
    check(&root)?;
    let marker = journal_path(&root)?;
    let receipts = parent(&marker)?;
    let _guard = OwnedLock::acquire(&parent(&root)?.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
    let kept = Files::new(root.clone());
    prepare_configured_backup(&kept, &root, link, authority, model::ComputerKind::Machine).await?;
    if let Some(member) = &member {
        member::Completion::prepare(receipts, link, member)?;
    }
    let mut next = link.clone();
    next.extra["localWipe"]["prepared"] = Value::Bool(true);
    crate::actor::write_link(&kept, &next, model::ComputerKind::Machine)
        .map_err(io::Error::other)?;
    *link = next;
    Ok(())
}

pub fn headless_member_pending(home: &Path) -> io::Result<bool> {
    let Some(root) = boundary(home)? else {
        return Ok(false);
    };
    Ok(Files::new(root)
        .read::<Link>(LINK)
        .map_err(io::Error::other)?
        .as_ref()
        .and_then(member_approval)
        .is_some())
}

pub async fn finish_headless_member(home: &Path) -> io::Result<bool> {
    let Some(root) = boundary(home)? else {
        return Ok(false);
    };
    if !headless_member_pending(&root)? {
        return Ok(false);
    }
    let marker = journal_path(&root)?;
    let receipts = parent(&marker)?;
    let guard = OwnedLock::acquire(&parent(&root)?.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
    check(&root)?;
    let kept = Files::new(root.clone());
    let owner = OwnedLock::acquire(&kept.path("run/.client-owner"), Duration::ZERO)?;
    let link = OwnedLock::acquire(&kept.path("run/.link.lock"), Duration::ZERO)?;
    validate_disk(&kept).map_err(io::Error::other)?;
    let Some(candidate) = kept.read::<Link>(LINK).map_err(io::Error::other)? else {
        return Ok(false);
    };
    let Some(wipe) = member_approval(&candidate) else {
        return Ok(false);
    };
    let completion = member::Completion::read(receipts, &wipe.setup, &wipe.operation)?
        .ok_or_else(|| io::Error::other("The wipe’s completion receipt is unavailable."))?;
    check(&root)?;
    crate::backup::with_verified_configured_disk(
        &root,
        &root,
        saved_backup(&candidate)?.as_ref(),
        || {
            kept.write_bytes("run/off", b"").map_err(io::Error::other)?;
            files::remove_tree_synced(&root)
        },
    )?;
    drop((owner, link, guard));
    completion
        .publish(&root, receipts, member::Report::Wipe)
        .await?;
    Ok(true)
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
    let marker = receipts.join(kind.marker());
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
                clear_detached_app_identity(&root, keychain)?;
            }
            Ok(())
        },
    )?;
    Ok(true)
}

fn clear_detached_app_identity(root: &Path, keychain: &dyn AppWipeKeychain) -> io::Result<()> {
    if identity(root)?.is_some() {
        return Err(io::Error::other(
            "The app-owned Box’s identity could not be removed safely.",
        ));
    }
    keychain.clear_app_link_keys(root)
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

impl InstallerWipe {
    pub fn waiting(&self) -> bool {
        self.pending_message().is_some()
    }

    pub fn pending_message(&self) -> Option<&'static str> {
        match self {
            Self::Waiting => Some("This computer’s wipe hasn’t finished."),
            #[cfg(target_os = "macos")]
            Self::MemberReview => Some(
                "This computer’s wipe needs review on the main Box. If the source is offline, restart the original app’s Box to continue.",
            ),
            Self::None | Self::Complete(_) => None,
        }
    }

    pub fn receipt(&self) -> Option<&ReinstallReceipt> {
        match self {
            Self::Complete(receipt) => Some(receipt),
            _ => None,
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

/// Installer status can resume the existing erase. The recorded operation and
/// directory identity remain the sole authority in `finish`; a marker alone
/// can make the installer wait, but cannot authorize deletion.
pub fn installer_wipe(home: &Path, resume: bool) -> io::Result<InstallerWipe> {
    let Some(root) = boundary(home)? else {
        return Ok(InstallerWipe::None);
    };
    let marker = journal_path(&root)?;
    let records = Files::new(root.clone());
    let mut journal = read_journal(&marker)?;
    if journal.as_ref().is_some_and(|kept| kept.phase.is_some()) || pending_hint(&records) {
        if !resume {
            return Ok(InstallerWipe::Waiting);
        }
        match finish(&root) {
            Ok(true) => journal = read_journal(&marker)?,
            Ok(false) => return Ok(InstallerWipe::Waiting),
            Err(error) => {
                // A live owner or an incomplete erase leaves its receipt for retry.
                eprintln!("finishing the installation's earlier wipe: {error}");
                return Ok(InstallerWipe::Waiting);
            }
        }
    }
    Ok(match journal {
        None => InstallerWipe::None,
        Some(kept) if kept.phase.is_some() => InstallerWipe::Waiting,
        Some(kept) => InstallerWipe::Complete(ReinstallReceipt {
            setup: kept.setup,
            operation: kept.operation,
        }),
    })
}

/// Only a successful reinstall consumes the completed receipt it observed.
pub fn finish_reinstall(home: &Path, receipt: &ReinstallReceipt) -> io::Result<()> {
    let Some(root) = boundary(home)? else {
        return Ok(());
    };
    let marker = journal_path(&root)?;
    let observed = || -> io::Result<bool> {
        Ok(read_journal(&marker)?.is_some_and(|kept| {
            kept.phase.is_none()
                && kept.setup == receipt.setup
                && kept.operation == receipt.operation
        }))
    };
    if observed()? {
        let _guard =
            OwnedLock::acquire(&parent(&root)?.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
        if observed()? {
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
    #[cfg(windows)]
    forget_erasers(receipts);
    let records = Files::new(root.clone());
    let pending = || pending_link(&records);
    let read = || read_journal(&marker);
    let current = read()?;
    if current.as_ref().is_none_or(|kept| kept.phase.is_none()) && pending()?.is_none() {
        return Ok(current.is_some() && !root.exists());
    }
    let _boundary = OwnedLock::acquire(&parent.join(".kuvo-code-wipe.lock"), Duration::ZERO)?;
    let kept = read()?;
    let save = |journal: &Journal| -> io::Result<()> {
        files::create_private_dirs(receipts)?;
        // Windows makes the folder with the profile's ACL; close it as Unix makes it 0700.
        #[cfg(windows)]
        files::private_folder(receipts)?;
        private(receipts, true)?;
        files::write_atomic_through(&marker, &serde_json::to_vec(journal)?, parent)
    };
    if kept
        .as_ref()
        .is_none_or(|kept| matches!(kept.phase, None | Some(Phase::Stopping)))
    {
        check(&root)?;
        let owner = OwnedLock::acquire(&records.path("run/.client-owner"), Duration::ZERO)?;
        let link = OwnedLock::acquire(&records.path("run/.link.lock"), Duration::ZERO)?;
        validate_disk(&records).map_err(io::Error::other)?;
        let Some(candidate) = records.read::<Link>(LINK).map_err(io::Error::other)? else {
            return Ok(false);
        };
        let Some(wipe) = LocalWipe::from(&candidate) else {
            return Ok(false);
        };
        let backup = saved_backup(&candidate)?;
        if backup.is_none()
            && !is_prepared(&candidate)
            && crate::backup::pre_wipe_backup_enabled(&root).map_err(io::Error::other)?
        {
            return Ok(false);
        }
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
        crate::backup::with_verified_configured_disk(&root, &root, backup.as_ref(), || {
            records
                .write_bytes("run/off", b"")
                .map_err(io::Error::other)?;
            save(&approval)?;
            erase_approved(&root, receipts, approval, save, || {
                // Drop guards by token, leaving any replacement alone.
                drop((owner, link));
                Ok(())
            })
        })?;
        return Ok(true);
    }
    let Some(journal) = kept else {
        return Ok(false);
    };
    erase_approved(&root, receipts, journal, save, || Ok(()))?;
    Ok(true)
}

fn erase_approved(
    root: &Path,
    receipts: &Path,
    mut journal: Journal,
    save: impl Fn(&Journal) -> io::Result<()>,
    after_detach: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut after_detach = Some(after_detach);
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
            // Windows can't rename a folder holding open files, these locks included.
            // The wipe lock beside the folder still keeps other boxes out.
            #[cfg(windows)]
            if let Some(release) = after_detach.take() {
                release()?;
            }
            files::replace_synced(root, &target)?;
        }
        journal.phase = Some(Phase::Detached);
        save(&journal)?;
    }
    if let Some(after_detach) = after_detach {
        after_detach()?;
    }
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
