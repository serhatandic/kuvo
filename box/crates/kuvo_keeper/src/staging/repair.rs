//! Repair only the selected installed release, never another version or an
//! unsigned file supplied by a visiting Mac.
use super::{
    Contents, DigestFile, MAX_ENVELOPE, MAX_EXPANDED, MAX_PACK, contents, names, read_release,
};
use crate::{
    fleet,
    manifest::{self, SignedRelease},
};
use flate2::read::GzDecoder;
use kuvo_store::{files, format::ObjectFormat, home::Home, locks};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RECORD: ObjectFormat = ObjectFormat::additive("Repair record");

pub fn repair_selected(home: &Home, keys: &[&str], now: SystemTime) -> io::Result<Value> {
    files::create_private_dirs(&home.run())?;
    let record = home.run().join("repair.json");
    RECORD.read(&record).map_err(io::Error::other)?;
    let result = repair(home, keys, now)?;
    #[cfg(target_os = "linux")]
    if matches!(result["result"].as_str(), Some("sound" | "repaired"))
        && !home.run().join("wsl-task").exists()
        && crate::shell_keeper(&home.bin())
            .file_name()
            .is_some_and(|name| name == ".kuvo-start-keeper")
        && let Err(error) = crate::crontab::install(home)
    {
        eprintln!("Keeping the saved keeper's startup: {error}");
    }
    let at = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut kept = result.clone();
    kept["at"] = json!(at);
    files::write_secret(&record, &serde_json::to_vec(&kept)?)?;
    Ok(result)
}

/// Fetch only the selected release if its local signed repair source is lost.
/// Downloads do not hold the staging lock or disturb running work.
pub async fn repair_with_fetch(home: &Home, key: &str, feed: &str) -> io::Result<Value> {
    let result = repair_or_fetch(home, key, feed).await?;
    #[cfg(target_os = "linux")]
    if matches!(result["result"].as_str(), Some("sound" | "repaired")) {
        let task = crate::wsl_task::WslTask::for_home(home).await;
        if let Err(error) = match task {
            Ok(Some(task)) => task.repair(home).await,
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        } {
            eprintln!("Keeping the WSL startup task: {error}");
        }
    }
    Ok(result)
}

async fn repair_or_fetch(home: &Home, key: &str, feed: &str) -> io::Result<Value> {
    let first = repair_selected(home, &[key], SystemTime::now())?;
    if first["result"] != "damaged"
        && !(first["result"] == "none"
            && matches!(first["why"].as_str(), Some("noRelease" | "otherVersion")))
    {
        return Ok(first);
    }
    let Some(selected) = fleet::selected(home)? else {
        return Ok(first);
    };
    let fetcher = fleet::Fetcher::default();
    let address = format!(
        "{}/v{}/boxes.json",
        feed.trim_end_matches('/'),
        selected.version()
    );
    let Ok(Some(offered)) = fetcher.check(&address, key).await else {
        return Ok(first);
    };
    let candidate = offered.for_repair();
    let reference = candidate.local()?;
    if reference.target()? != selected {
        return Ok(first);
    }
    let folder = selected.folder(home.root());
    if !safe_folder(home, &folder) || !receipt_matches(&folder, &reference)? {
        return Ok(first);
    }
    let Ok(bytes) = candidate.download_or_held(home, key, &fetcher).await else {
        return Ok(first);
    };
    if !reference.matches_bytes(&bytes) {
        return Ok(first);
    }
    let lock = match locks::try_file_lock(&home.run().join(".release-staging.lock")) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(first),
        Err(error) => return Err(error),
    };
    if busy(home, SystemTime::now())
        || fleet::selected(home)?.as_ref() != Some(&selected)
        || !safe_folder(home, &folder)
        || !receipt_matches(&folder, &reference)?
    {
        return Ok(first);
    }
    files::write_secret(&folder.join("pack"), &bytes)?;
    files::write_secret(&folder.join("boxes.json"), candidate.envelope().as_bytes())?;
    drop(lock);
    repair_selected(home, &[key], SystemTime::now())
}

fn none(why: &str) -> Value {
    json!({"formatVersion":1,"result":"none","why":why})
}

fn busy(home: &Home, now: SystemTime) -> bool {
    let run = home.run();
    [
        "previous",
        "update-trial",
        ".rolling-back",
        "keeper-rollback",
        "switch-pending",
    ]
    .iter()
    .any(|name| run.join(name).exists())
        || crate::update_status::legacy_updating(home)
        || modified(&run.join("mac-installing")).is_some_and(|at| {
            now.duration_since(at)
                .map_or(true, |age| age < Duration::from_secs(3600))
        })
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.is_file())?
        .modified()
        .ok()
}

/// Retire the old watchdog's day-old attachment marker, keeping the Mac's visit
/// time without asking the engine (or stopping an unrecorded process).
fn retire_visit(home: &Home, now: SystemTime) -> io::Result<()> {
    let attached = home.run().join("attached");
    let Some(at) = modified(&attached) else {
        return Ok(());
    };
    if now
        .duration_since(at)
        .is_ok_and(|age| age > Duration::from_secs(23 * 3600))
    {
        let older = home.run().join("older-mac");
        if modified(&home.bin().join("box-version")).is_some_and(|version| at > version)
            && modified(&older).is_none_or(|seen| at > seen)
        {
            files::replace_synced(&attached, &older)?;
        } else {
            files::remove_file_synced(&attached)?;
        }
    }
    Ok(())
}

fn repair(home: &Home, keys: &[&str], now: SystemTime) -> io::Result<Value> {
    if busy(home, now) {
        return Ok(none("busy"));
    }
    retire_visit(home, now)?;
    let Some(selected) = fleet::selected(home)? else {
        return Ok(none("noRelease"));
    };
    let _lock = match locks::try_file_lock(&home.run().join(".release-staging.lock")) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(none("busy")),
        Err(error) => return Err(error),
    };
    if busy(home, now) || fleet::selected(home)?.as_ref() != Some(&selected) {
        return Ok(none("busy"));
    }
    let folder = selected.folder(home.root());
    if !safe_folder(home, &folder) {
        return Ok(none("noRelease"));
    }
    let envelope = folder.join("boxes.json");
    let pack = folder.join("pack");
    let Ok(signed) =
        read_release(&envelope, MAX_ENVELOPE).and_then(|bytes| SignedRelease::verify(&bytes, keys))
    else {
        return Ok(none("noRelease"));
    };
    if !signed.launches_rust() {
        return Ok(none("otherVersion"));
    }
    let reference = signed.reference(signed.pack_for(manifest::local_target()?)?);
    if reference.target()? != selected || !receipt_matches(&folder, &reference)? {
        return Ok(none("otherVersion"));
    }
    let Ok(bytes) = read_release(&pack, MAX_PACK) else {
        return Ok(none("noRelease"));
    };
    if manifest::sha256(&bytes) != reference.sha256 {
        // The pack is separate from the programs being used. Never remove the
        // installed release because the repair copy of its pack has gone bad.
        files::remove_file_synced(&pack)?;
        return Ok(json!({"formatVersion":1,"result":"damaged","version":reference.version}));
    }
    let Ok((packed_manifest, expected)) = packed_files(&bytes, &signed, &reference) else {
        return Ok(none("failed"));
    };
    let manifest = folder.join("box.json");
    let manifest_wrong = !fs::symlink_metadata(&manifest).is_ok_and(|meta| meta.is_file())
        || fs::read(&manifest).ok().as_ref() != Some(&packed_manifest);
    let mut wrong = BTreeMap::new();
    for (name, hash) in expected {
        let target = match name.strip_prefix("config/opencode/plugins/") {
            Some(plugin) => folder.join("bin/opencode-plugins").join(plugin),
            None => folder.join(&name),
        };
        let parent = target.parent().ok_or_else(super::outside)?;
        // Never traverse a replaced directory outside the selected release.
        if parent
            .ancestors()
            .take_while(|path| *path != folder)
            .any(|path| fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_dir()))
        {
            return Ok(none("failed"));
        }
        if hash_of(&target).as_deref() != Some(&hash) {
            wrong.insert(name, (target, hash));
        }
    }
    let mut repaired = Vec::new();
    if !wrong.is_empty() {
        let draft = tempfile::Builder::new()
            .prefix(".repair-")
            .tempdir_in(&folder)?;
        // The signed pack digest has already been checked. Extract only the
        // wrong files, checking each one's listed digest before publishing any.
        // An ordinary Mac visit whose files are sound writes no large copies.
        let Ok(replacements) = replacements(draft.path(), &bytes, &wrong) else {
            return Ok(none("failed"));
        };
        if busy(home, SystemTime::now()) || fleet::selected(home)?.as_ref() != Some(&selected) {
            return Ok(none("busy"));
        }
        for (name, staged) in replacements {
            let (target, _) = &wrong[&name];
            files::create_private_dirs(target.parent().ok_or_else(super::outside)?)?;
            files::replace_synced(&staged, target)?;
            repaired.push(name);
        }
    }
    if manifest_wrong {
        files::write_secret(&manifest, &packed_manifest)?;
        repaired.push("box.json".to_owned());
    }
    files::remove_tree_synced(&folder.join("bin/.own"))?;
    Ok(if repaired.is_empty() {
        json!({"formatVersion":1,"result":"sound","version":reference.version})
    } else {
        json!({"formatVersion":1,"result":"repaired","version":reference.version,"files":repaired})
    })
}

fn packed_files(
    bytes: &[u8],
    signed: &SignedRelease,
    reference: &manifest::ReleaseRef,
) -> io::Result<(Vec<u8>, BTreeMap<String, String>)> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes).take(MAX_EXPANDED));
    for entry in archive.entries()? {
        let entry = entry?;
        if !matches!(entry.path_bytes().as_ref(), b"box.json" | b"./box.json") {
            continue;
        }
        if !entry.header().entry_type().is_file() || entry.size() > 10_000_000 {
            return Err(super::damaged());
        }
        let mut record = Vec::new();
        entry.take(10_000_001).read_to_end(&mut record)?;
        let given: Contents = contents(&record)?;
        if given.kuvo != "box/1"
            || given.launch != "rust"
            || given.version != reference.version
            || given.engine != signed.engine()
        {
            return Err(super::damaged());
        }
        let expected = names(&given)?;
        let version_hash = manifest::sha256(format!("{}\n", reference.version).as_bytes());
        let plain_hash = manifest::sha256(reference.version.as_bytes());
        if !expected.contains_key(&format!("bin/kuvo-box{}", std::env::consts::EXE_SUFFIX))
            || expected
                .get("bin/box-version")
                .is_none_or(|hash| hash != &version_hash && hash != &plain_hash)
        {
            return Err(super::damaged());
        }
        return Ok((record, expected));
    }
    Err(super::damaged())
}

fn replacements(
    draft: &Path,
    bytes: &[u8],
    wrong: &BTreeMap<String, (PathBuf, String)>,
) -> io::Result<BTreeMap<String, PathBuf>> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes).take(MAX_EXPANDED));
    let mut kept = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let raw = entry.path_bytes().into_owned();
        let raw = std::str::from_utf8(&raw).map_err(|_| super::outside())?;
        let name = raw.strip_prefix("./").unwrap_or(raw);
        let Some((_, expected)) = wrong.get(name) else {
            continue;
        };
        if !entry.header().entry_type().is_file() || kept.contains_key(name) {
            return Err(super::damaged());
        }
        let staged = draft.join(kept.len().to_string());
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        let mut digest = Sha256::new();
        let size = io::copy(&mut entry, &mut DigestFile(&mut file, &mut digest))?;
        if size != entry.size() || format!("{:x}", digest.finalize()) != *expected {
            return Err(super::damaged());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(
                0o600 | (entry.header().mode()? & 0o111),
            ))?;
        }
        file.sync_all()?;
        kept.insert(name.to_owned(), staged);
        if kept.len() == wrong.len() {
            break;
        }
    }
    if kept.len() != wrong.len() {
        return Err(super::damaged());
    }
    Ok(kept)
}

fn safe_folder(home: &Home, folder: &Path) -> bool {
    [
        home.root().join("versions"),
        folder.to_owned(),
        folder.join("bin"),
    ]
    .iter()
    .all(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()))
}

fn receipt_matches(folder: &Path, reference: &manifest::ReleaseRef) -> io::Result<bool> {
    Ok(kuvo_store::format::INSTALLED
        .read(&folder.join("bin/.installed.json"))
        .map_err(io::Error::other)?
        .is_none_or(|receipt| {
            receipt.fields.get("sha256").and_then(Value::as_str) == Some(&reference.sha256)
                && receipt.fields.get("version").and_then(Value::as_str) == Some(&reference.version)
        }))
}

fn hash_of(file: &Path) -> Option<String> {
    if !fs::symlink_metadata(file).ok()?.is_file() {
        return None;
    }
    let mut digest = Sha256::new();
    io::copy(&mut fs::File::open(file).ok()?, &mut digest).ok()?;
    Some(format!("{:x}", digest.finalize()))
}
