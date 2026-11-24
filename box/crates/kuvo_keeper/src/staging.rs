//! Verify a target pack and publish its complete installable directory in one rename.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};

use flate2::read::GzDecoder;
use kuvo_os::{
    platform::Environment,
    process::{Spawn, Stdio},
};
use kuvo_store::{files, format::ObjectFormat, home::Home, locks};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    Release, SELECTOR,
    engine_pack::EnginePack,
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

/// Probe the new programs in a private folder and leave the selected release in place.
pub fn prepare(
    home: &Home,
    signed: &SignedRelease,
    target: &str,
    bytes: &[u8],
    envelope: &[u8],
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
    check_downgrade(home, &contents)?;
    let binary = format!("bin/kuvo-box{}", std::env::consts::EXE_SUFFIX);
    let probe = tempfile::Builder::new()
        .prefix(".probe-")
        .tempdir_in(&versions)?;
    let engine = EnginePack::find_or_fetch(home, signed, target, probe.path())?;
    check_program(
        engine.executable(),
        &["--version"],
        probe.path(),
        "The new engine doesn't run here.",
    )?;
    check_program(
        &draft.path().join(&binary),
        &["selftest", "--engine", &contents.engine],
        probe.path(),
        "The new box doesn't run with its engine here.",
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
    files::write_secret(
        &home.run().join("fleet-ready.json"),
        &serde_json::to_vec(&reference)?,
    )?;
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
    check_program(
        engine.executable(),
        &["--version"],
        probe.path(),
        "The new engine doesn't run here.",
    )?;
    check_program(
        &release.join("bin/kuvo-box"),
        &["selftest", "--engine", signed.engine()],
        probe.path(),
        "The new box doesn't run with its engine here.",
    )?;
    // The engine downloader's temporary file belongs to the install folder.
    // Seal it before publishing the pair that the move will open.
    engine.publish(&Home::new(install.clone())?)?;
    files::sync_tree(draft.path(), home.root())?;
    files::replace_synced(draft.path(), published.root())?;
    Ok(published)
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

fn check_program(program: &Path, args: &[&str], folder: &Path, failure: &str) -> io::Result<()> {
    probe_program(program, args, folder, failure, None)
}

pub(crate) fn check_engine_version(program: &Path, folder: &Path, version: &str) -> io::Result<()> {
    probe_program(
        program,
        &["--version"],
        folder,
        "The engine doesn't report the pinned version.",
        Some(version),
    )
}

fn probe_program(
    program: &Path,
    args: &[&str],
    folder: &Path,
    failure: &str,
    version: Option<&str>,
) -> io::Result<()> {
    let mut env: Environment = std::env::vars_os()
        .filter(|(key, _)| {
            ["PATH", "SystemRoot", "WINDIR"]
                .iter()
                .any(|name| key.eq_ignore_ascii_case(std::ffi::OsStr::new(name)))
        })
        .collect();
    for key in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TMPDIR",
        "TEMP",
        "TMP",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "KUVO_HOME",
    ] {
        env.insert(key.into(), folder.as_os_str().into());
    }
    let mut command = Spawn::new(program, env);
    command.cwd = Some(folder.into());
    command.args = args.iter().map(|arg| (*arg).into()).collect();
    let capture = if let Some(version) = version {
        let output = tempfile::tempfile_in(folder)?;
        command.stdout = Stdio::File(output.try_clone()?);
        Some((output, version))
    } else {
        None
    };
    let mut child = command
        .spawn()
        .map_err(|error| io::Error::other(format!("{failure} {error}")))?;
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if child.has_exited()? {
            if !child.wait()?.success() {
                return Err(io::Error::other(failure));
            }
            if let Some((mut output, version)) = capture {
                output.seek(SeekFrom::Start(0))?;
                let mut bytes = Vec::new();
                output.take(129).read_to_end(&mut bytes)?;
                if bytes.len() > 128 || std::str::from_utf8(&bytes).map(str::trim) != Ok(version) {
                    return Err(io::Error::other(failure));
                }
            }
            return Ok(());
        }
        if Instant::now() >= until {
            child.stop(Duration::from_millis(100), |_| {})?;
            return Err(io::Error::new(io::ErrorKind::TimedOut, failure));
        }
        std::thread::sleep(Duration::from_millis(20));
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
