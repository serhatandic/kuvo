//! Verify a target pack and publish its complete installable directory in one rename.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use flate2::read::GzDecoder;
use kuvo_store::{files, format::ObjectFormat, home::Home, locks};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    Release, SELECTOR,
    engine_pack::{EnginePack, check_installer_engine},
    manifest::{ReleaseRef, SignedRelease, sha256},
};

pub const MAX_ENVELOPE: u64 = 1_000_000;
pub const MAX_PACK: u64 = 100_000_000;
const MAX_EXPANDED: u64 = 500_000_000;
const DAMAGED: &str = "The release's files aren't what it says.";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contents {
    kuvo: String,
    version: String,
    engine: String,
    launch: String,
    files: BTreeMap<String, String>,
    #[serde(default)]
    bundles: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    engines: Vec<String>,
    #[serde(default)]
    downgrade_safe: bool,
}

pub fn read_release(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let unreadable = || io::Error::other("That release can't be read.");
    let file = File::open(path).map_err(|_| unreadable())?;
    let metadata = file.metadata().map_err(|_| unreadable())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(unreadable());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unreadable())?;
    if bytes.len() as u64 > limit {
        return Err(unreadable());
    }
    Ok(bytes)
}

/// Verify the signed media carried by the installer before reporting its status.
pub fn check_install_media(home: &Home, source: &Path, key: &str) -> io::Result<()> {
    let checked: io::Result<(SignedRelease, &str)> = (|| {
        let envelope = read_release(&source.join("boxes.json"), MAX_ENVELOPE)?;
        let signed = SignedRelease::verify(&envelope, &[key])?;
        if !signed.launches_rust() {
            return Err(damaged());
        }
        let target = crate::manifest::local_target()?;
        if let Some(engine) = signed.engine_build(target)
            && !reqwest::Url::parse(&engine.url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        {
            return Err(damaged());
        }
        let pack = signed.pack_for(target)?;
        let archive = File::open(source.join(pack.filename()?))?;
        let metadata = archive.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_PACK {
            return Err(damaged());
        }
        let mut digest = Sha256::new();
        let size = io::copy(&mut archive.take(MAX_PACK + 1), &mut digest)?;
        if size > MAX_PACK || pack.sha256 != format!("{:x}", digest.finalize()) {
            return Err(damaged());
        }
        Ok((signed, target))
    })();
    let (signed, target) = checked.map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "This copy of Kuvo is damaged. Download it again, then try again.",
        )
    })?;
    check_installer_engine(home, &signed, target)
}

/// Probe the new programs in a private folder and leave the selected release in place.
pub fn prepare(
    home: &Home,
    signed: &SignedRelease,
    target: &str,
    bytes: &[u8],
    envelope: &[u8],
) -> io::Result<ReleaseRef> {
    prepare_pack(home, signed, target, bytes, envelope, StageFor::Fleet)
}

enum StageFor {
    Fleet,
    Install,
}

fn prepare_pack(
    home: &Home,
    signed: &SignedRelease,
    target: &str,
    bytes: &[u8],
    envelope: &[u8],
    purpose: StageFor,
) -> io::Result<ReleaseRef> {
    let pack = signed.pack_for(target)?;
    if bytes.len() as u64 > MAX_PACK || sha256(bytes) != pack.sha256 {
        return Err(damaged());
    }
    let reference = signed.reference(pack);
    let release = reference.target()?;
    let versions = home.root().join("versions");
    files::create_private_dirs(&versions)?;
    files::create_private_dirs(&home.run())?;
    // Serialize stage publication and its ready fact. Download and signature admission
    // are outside the lock and the running release never waits on it.
    let _lock = locks::try_file_lock(&home.run().join(".release-staging.lock"))?;
    for entry in fs::read_dir(&versions)? {
        let entry = entry?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(".staging-") || name.starts_with(".probe-"))
            && entry.file_type()?.is_dir()
        {
            files::remove_tree_synced(&entry.path())?;
        }
    }
    let draft = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(&versions)?;
    let (contents, actual) = unpack_checked(draft.path(), signed, &reference, bytes)?;
    if matches!(purpose, StageFor::Install) && !actual.contains_key("bin/kuvo-start") {
        return Err(io::Error::other("The selected release has no keeper."));
    }
    if matches!(purpose, StageFor::Fleet) {
        check_downgrade(home, &contents)?;
    }
    let binary = format!("bin/kuvo-box{}", std::env::consts::EXE_SUFFIX);
    let probe = tempfile::Builder::new()
        .prefix(".probe-")
        .tempdir_in(&versions)?;
    let engine = EnginePack::find_or_fetch(home, signed, target, probe.path())?;
    let cwd = Some(home.root());
    check_engine(engine.executable(), cwd, "The new engine doesn't run here.")?;
    probe_program(
        &draft.path().join(&binary),
        &["selftest", "--engine", &contents.engine],
        cwd,
        "The new box doesn't run with its engine here.",
        Duration::from_secs(10),
    )?;
    let bin = draft.path().join("bin");
    let plugins = draft.path().join("config/opencode/plugins");
    if plugins.exists() {
        fs::rename(plugins, bin.join("opencode-plugins"))?;
        fs::remove_dir_all(draft.path().join("config"))?;
    } else {
        files::create_private_dirs(&bin.join("opencode-plugins"))?;
    }
    if !bin.join(".kuvo-boot").exists() {
        files::write_secret(&bin.join(".kuvo-boot"), format!("{release}\n").as_bytes())?;
    }
    let receipt = json!({
        "formatVersion": 1,
        "version": reference.version,
        "sha256": reference.sha256,
        "engine": contents.engine,
        "files": contents.files,
        "engines": contents.engines,
        "downgradeSafe": contents.downgrade_safe,
    });
    let receipt = serde_json::to_vec(&receipt)?;
    files::write_secret(&bin.join(".installed.json"), &receipt)?;
    engine.publish(home)?;
    publish(home, draft.path(), &release, &receipt, &actual)?;
    let folder = release.folder(home.root());
    files::write_secret(&folder.join("pack"), bytes)?;
    files::write_secret(&folder.join("boxes.json"), envelope)?;
    if matches!(purpose, StageFor::Fleet) {
        files::write_secret(
            &home.run().join("fleet-ready.json"),
            &serde_json::to_vec(&reference)?,
        )?;
        if let Err(error) = crate::retirement::sweep(home, &release) {
            eprintln!("Sweeping superseded Kuvo releases: {error}");
        }
    }
    Ok(reference)
}

fn unpack_checked(
    folder: &Path,
    signed: &SignedRelease,
    reference: &ReleaseRef,
    bytes: &[u8],
) -> io::Result<(Contents, BTreeMap<String, String>)> {
    let actual = unpack(folder, bytes)?;
    let manifest_path = folder.join("box.json");
    let manifest = read_release(&manifest_path, 10_000_000)?;
    let contents = contents(&manifest)?;
    if contents.kuvo != "box/1"
        || contents.launch != "rust"
        || contents.version != reference.version
        || contents.engine != signed.engine()
    {
        return Err(damaged());
    }
    let expected = names(&contents)?;
    if expected != actual {
        return Err(damaged());
    }
    let binary = format!("bin/kuvo-box{}", std::env::consts::EXE_SUFFIX);
    if !actual.contains_key(&binary)
        || fs::read_to_string(folder.join("bin/box-version"))
            .map_err(|_| damaged())?
            .trim()
            != reference.version
    {
        return Err(damaged());
    }
    Ok((contents, actual))
}

/// The released-app move consumes a sealed release tree and its pinned engine
/// beneath the installer's folder. The separate install lock outlives the move.
#[cfg(target_os = "macos")]
pub fn prepare_install(home: &Home, signed: &SignedRelease, source: &Path) -> io::Result<Home> {
    let target = crate::manifest::local_target()?;
    let pack = signed.pack_for(target)?;
    let bytes = read_release(&source.join(pack.filename()?), MAX_PACK)?;
    if sha256(&bytes) != pack.sha256 {
        return Err(damaged());
    }
    let reference = signed.reference(pack);
    let install = home.updates().join("install");
    files::create_private_dirs(&install)?;
    let published = Home::new(install.join(&pack.sha256))?;
    // The move checks its recorded inputs again before resuming. Never displace
    // a tree that a cut-short move may still be using.
    if published.root().join("release/box.json").is_file() {
        return Ok(published);
    }
    let draft = tempfile::Builder::new()
        .prefix(".media-")
        .tempdir_in(&install)?;
    let media = Home::new(draft.path().to_owned())?;
    let release = media.root().join("release");
    files::create_private_dirs(&release)?;
    unpack_checked(&release, signed, &reference, &bytes)?;
    let probe = tempfile::Builder::new()
        .prefix(".probe-")
        .tempdir_in(&install)?;
    let engine = EnginePack::find_or_fetch(&media, signed, target, probe.path())?;
    check_engine(
        engine.executable(),
        None,
        "Kuvo's engine doesn't run on this Mac.",
    )?;
    probe_program(
        &release.join("bin/kuvo-box"),
        &["selftest", "--engine", signed.engine()],
        None,
        "The new box doesn't run with its engine here.",
        Duration::from_secs(10),
    )?;
    // The engine downloader's temporary file belongs to the install folder.
    // Seal it before publishing the pair that the move will open.
    engine.publish(&Home::new(install.clone())?)?;
    files::sync_tree(draft.path(), home.root())?;
    files::replace_synced(draft.path(), published.root())?;
    Ok(published)
}

/// Select the verified release that a Mac move or fresh making just installed.
/// Run before starting the Helper, while the installer still owns its lock.
#[cfg(target_os = "macos")]
pub fn select_install(
    home: &Home,
    signed: &SignedRelease,
    source: &Path,
    envelope: &[u8],
) -> io::Result<()> {
    let target = crate::manifest::local_target()?;
    let pack = signed.pack_for(target)?;
    let reference = signed.reference(pack);
    let selection = reference.target()?;
    let selected = home.bin();
    let entry = fs::symlink_metadata(&selected)?;
    if entry.file_type().is_symlink()
        && fs::read_link(&selected)? == Path::new(&selection.to_string())
    {
        return selection.validate(home.root());
    }
    let changed = || io::Error::other("The selected installation changed while it was made.");
    if !entry.is_dir() {
        return Err(changed());
    }
    let installed = read_release(&home.root().join("box.json"), 10_000_000)?;
    let current = contents(&installed)?;
    if current.launch != "rust"
        || current.version != reference.version
        || current.engine != signed.engine()
    {
        return Err(changed());
    }
    prepare_install_selection(home, signed, source, envelope)?;
    if installed != read_release(&selection.folder(home.root()).join("box.json"), 10_000_000)? {
        return Err(changed());
    }

    select_initial(home, &selection)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn prepare_install_selection(
    home: &Home,
    signed: &SignedRelease,
    source: &Path,
    envelope: &[u8],
) -> io::Result<Release> {
    let target = crate::manifest::local_target()?;
    let pack = signed.pack_for(target)?;
    let bytes = read_release(&source.join(pack.filename()?), MAX_PACK)?;
    prepare_pack(home, signed, target, &bytes, envelope, StageFor::Install)?.target()
}

/// Select the staged release before the installed keeper starts.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn select_initial(home: &Home, selection: &Release) -> io::Result<()> {
    selection.validate(home.root())?;
    let keeper = selection.folder(home.root()).join("bin/kuvo-start");
    if !fs::symlink_metadata(keeper).is_ok_and(|entry| entry.is_file()) {
        return Err(io::Error::other("The selected release has no keeper."));
    }
    let selected = home.bin();
    let target = selection.to_string();
    match fs::symlink_metadata(&selected) {
        Ok(entry)
            if entry.file_type().is_symlink()
                && fs::read_link(&selected)? == Path::new(&target) =>
        {
            return Ok(());
        }
        Ok(entry) if entry.is_dir() => {}
        Ok(_) => {
            return Err(io::Error::other(
                "The selected installation changed while it was made.",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(target, &selected)?;
            return files::sync_parent(&selected);
        }
        Err(error) => return Err(error),
    }
    let install = home.updates().join("install");
    files::create_private_dirs(&install)?;
    let draft = tempfile::Builder::new()
        .prefix(".selection-")
        .tempdir_in(install)?;
    let next = draft.path().join("bin");
    std::os::unix::fs::symlink(target, &next)?;
    files::exchange_entries(&next, &selected)?;
    files::remove_tree_synced(&next)
}

fn check_downgrade(home: &Home, next: &Contents) -> io::Result<()> {
    let read = |path: &Path, field: &str| -> io::Result<Option<String>> {
        files::read(path)?
            .map(|bytes| {
                serde_json::from_slice::<serde_json::Value>(&bytes)
                    .map(|entry| entry[field].as_str().map(str::to_owned))
            })
            .transpose()
            .map(Option::flatten)
            .map_err(io::Error::from)
    };
    let selected = Release::read(&home.root().join(SELECTOR))?.map_or_else(
        || home.bin().join("../box.json"),
        |release| release.folder(home.root()).join("box.json"),
    );
    let current = match read(&home.run().join("engine.json"), "version")? {
        Some(running) => Some(running),
        None => read(&selected, "engine")?,
    };
    if current.is_some_and(|version| {
        version != next.engine && !(next.downgrade_safe && next.engines.contains(&version))
    }) {
        return Err(io::Error::other(
            "The new engine hasn't been checked for going back to the engine running here.",
        ));
    }
    Ok(())
}

pub(crate) fn check_engine(program: &Path, cwd: Option<&Path>, failure: &str) -> io::Result<()> {
    // The signed SHA-512 pins the download. The probe only checks that it runs.
    probe_program(
        program,
        &["--version"],
        cwd,
        failure,
        Duration::from_secs(120),
    )
}

fn probe_program(
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    failure: &str,
    timeout: Duration,
) -> io::Result<()> {
    // These commands report an exit status and can finish before the keeper can
    // read a process start identity. The installer itself owns and reaps them.
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .map_err(|error| io::Error::other(format!("{failure} {error}")))?;
    let until = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(io::Error::other(failure)),
            Ok(None) if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            result => {
                if child.kill().is_ok() {
                    let _ = child.wait();
                }
                return result.and(Err(io::Error::new(io::ErrorKind::TimedOut, failure)));
            }
        }
    }
}

fn publish(
    home: &Home,
    draft: &Path,
    release: &Release,
    receipt: &[u8],
    hashes: &BTreeMap<String, String>,
) -> io::Result<()> {
    let folder = release.folder(home.root());
    if folder.try_exists()? {
        // A retry after the durable rename may have lost only the ready fact.
        // A different full digest with the same filename prefix must never replace it.
        if fs::read(folder.join("bin/.installed.json"))? != receipt {
            return Err(io::Error::other(
                "A different release is already staged at this location.",
            ));
        }
        if fs::read(folder.join("box.json"))? != fs::read(draft.join("box.json"))? {
            return Err(damaged());
        }
        for (name, hash) in hashes {
            let file = match name.strip_prefix("config/opencode/plugins/") {
                Some(plugin) => folder.join("bin/opencode-plugins").join(plugin),
                None => folder.join(name),
            };
            if !fs::symlink_metadata(&file)?.is_file() {
                return Err(damaged());
            }
            let mut digest = Sha256::new();
            io::copy(&mut File::open(file)?, &mut digest)?;
            if format!("{:x}", digest.finalize()) != *hash {
                return Err(damaged());
            }
        }
    } else {
        files::sync_tree(draft, home.root())?;
        files::replace_synced(draft, &folder)?;
    }
    release.validate(home.root())
}

fn contents(bytes: &[u8]) -> io::Result<Contents> {
    let record = ObjectFormat::additive("Box pack file list")
        .decode(serde_json::from_slice(bytes)?)
        .map_err(io::Error::other)?;
    serde_json::from_value(serde_json::Value::Object(record.fields)).map_err(io::Error::other)
}

fn names(contents: &Contents) -> io::Result<BTreeMap<String, String>> {
    let mut expected = contents.files.clone();
    if expected.keys().any(|name| !ordinary_file(name)) {
        return Err(outside());
    }
    for (bundle, members) in &contents.bundles {
        let app = bundle.strip_prefix("bin/").ok_or_else(outside)?;
        if !app.ends_with(".app")
            || !app.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !app
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._ -".contains(&b))
        {
            return Err(outside());
        }
        for (name, digest) in members {
            if !name.split('/').all(bundle_part) {
                return Err(outside());
            }
            expected.insert(format!("{bundle}/{name}"), digest.clone());
        }
    }
    Ok(expected)
}

fn ordinary_file(path: &str) -> bool {
    let name = ["bin/runtime/", "bin/", "config/opencode/plugins/"]
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix));
    name.is_some_and(|name| {
        !matches!(name, "" | "." | ".." | ".installed.json")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    })
}

fn bundle_part(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+ -".contains(&b))
}

fn unpack(folder: &Path, bytes: &[u8]) -> io::Result<BTreeMap<String, String>> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes).take(MAX_EXPANDED));
    let mut hashes = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let raw = entry.path_bytes().into_owned();
        let raw = std::str::from_utf8(&raw).map_err(|_| outside())?;
        let name = raw.strip_prefix("./").unwrap_or(raw).trim_end_matches('/');
        let kind = entry.header().entry_type();
        if name.is_empty() || name == "." {
            if kind.is_dir() {
                continue;
            }
            return Err(outside());
        }
        if !name.split('/').all(safe_component) {
            return Err(outside());
        }
        // Directory entries are implicit and no permissions come from them.
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(outside());
        }
        let path = folder.join(name);
        let parent = path.parent().ok_or_else(outside)?;
        files::create_private_dirs(parent)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut digest = Sha256::new();
        let size = io::copy(&mut entry, &mut DigestFile(&mut file, &mut digest))?;
        if size != entry.size() {
            return Err(damaged());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = 0o600 | (entry.header().mode()? & 0o111);
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        if name != "box.json" {
            hashes.insert(name.to_string(), format!("{:x}", digest.finalize()));
        }
    }
    Ok(hashes)
}

fn safe_component(part: &str) -> bool {
    // Use one spelling on every platform, including Windows' alternate streams
    // and device names, where an apparently relative path can escape the draft.
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|suffix| suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit())
        });
    !part.is_empty()
        && !part.ends_with(['.', ' '])
        && !device
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+ -".contains(&b))
}

struct DigestFile<'a>(&'a mut File, &'a mut Sha256);

impl Write for DigestFile<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.0.write(bytes)?;
        self.1.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

fn damaged() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, DAMAGED)
}

fn outside() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "The release names a file outside Kuvo’s folder.",
    )
}
