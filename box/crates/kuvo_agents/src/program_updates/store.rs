//! Side-by-side versioned staging for coding-agent programs.
//!
//! Each installed version is staged into `<home>/run/agents/programs/<program>/v_<key>/` so
//! running agent processes never execute from (or lock, on Windows) the user's installed binary.
//! Holders take a shared OS lock on `v_<key>.lease` for the life of the process (including across
//! resident restarts); once a newer version is active and the last lease drops, the old folder is
//! renamed aside and removed.

use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use kuvo_store::{files, locks};
use sha2::{Digest, Sha256};

use super::versions::npm_global_prefix;

const VERSION_PREFIX: &str = "v_";
const STAGING_PREFIX: &str = ".staging-";
const RETIRED_PREFIX: &str = ".retired-";
const LEASE_SUFFIX: &str = ".lease";
const LEASES_LOCK: &str = "leases.lock";
const INSTALL_LOCK: &str = "install.lock";
const CURRENT_FILE: &str = "current";
const READY_MARKER: &str = ".ready";
const FULL_HASH_LIMIT: u64 = 256 * 1024;
const EDGE_HASH_BYTES: usize = 4096;

pub fn program_root(home_root: &Path, program: &str) -> PathBuf {
    home_root.join("run/agents/programs").join(program)
}

/// A staged program ready to launch, holding a shared lease until the holder takes its own.
pub struct Staged {
    pub executable: PathBuf,
    pub version_dir: Option<PathBuf>,
    _lease: Option<Lease>,
}

/// Shared OS file lock on a staged version. Dropping releases the lock and removes the version
/// immediately if it is no longer current and nothing else uses it.
pub struct Lease {
    program_dir: PathBuf,
    version_name: String,
    file: Option<File>,
}

impl Lease {
    pub fn acquire_for_executable(executable: &Path) -> Option<Self> {
        let (program_dir, version_name) = staged_location(executable)?;
        if !program_dir.join(&version_name).join(READY_MARKER).is_file() {
            return None;
        }
        Self::acquire(&program_dir, &version_name).ok()
    }

    fn acquire(program_dir: &Path, version_name: &str) -> io::Result<Self> {
        let _leases = lock_exclusive(&program_dir.join(LEASES_LOCK))?;
        let file = locks::try_shared_file_lock(&lease_file(program_dir, version_name))?;
        Ok(Self {
            program_dir: program_dir.to_owned(),
            version_name: version_name.to_owned(),
            file: Some(file),
        })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        drop(self.file.take());
        retire_if_unused(&self.program_dir, &self.version_name);
    }
}

/// Stage `source` into its immutable version directory and return a leased launch path. Keep
/// `Staged` alive until the holder has started and taken its own lease.
pub fn prepare(
    home_root: &Path,
    program: &str,
    source: &Path,
    npm_package: Option<&str>,
) -> io::Result<Staged> {
    let Some(plan) = StagePlan::for_source(source, npm_package)? else {
        return Ok(Staged {
            executable: source.to_owned(),
            version_dir: None,
            _lease: None,
        });
    };
    let program_dir = program_root(home_root, program);
    files::create_private_dirs(&program_dir)?;
    let key = plan.version_key()?;
    let version_dir = program_dir.join(&key);
    let staged_exe = version_dir.join(&plan.relative_entry);
    let lease = Lease::acquire(&program_dir, &key)?;
    if !version_dir.join(READY_MARKER).is_file() || !staged_exe.is_file() {
        let _install = lock_exclusive(&program_dir.join(INSTALL_LOCK))?;
        if !version_dir.join(READY_MARKER).is_file() || !staged_exe.is_file() {
            let staging = tempfile::Builder::new()
                .prefix(STAGING_PREFIX)
                .tempdir_in(&program_dir)?;
            plan.copy_into(staging.path())?;
            fs::write(staging.path().join(READY_MARKER), b"")?;
            if version_dir.exists()
                && let Ok(aside) = tempfile::Builder::new()
                    .prefix(RETIRED_PREFIX)
                    .tempdir_in(&program_dir)
            {
                let aside = aside.keep();
                let _ = fs::rename(&version_dir, aside.join("incomplete"));
                remove_tree(&aside);
            }
            fs::rename(staging.keep(), &version_dir)?;
        }
    }
    let published = {
        let _leases = lock_exclusive(&program_dir.join(LEASES_LOCK))?;
        let still_same = StagePlan::for_source(source, npm_package)
            .ok()
            .flatten()
            .and_then(|now| now.version_key().ok())
            .is_some_and(|now| now == key);
        if still_same && read_current(&program_dir).as_deref() != Some(key.as_str()) {
            let _ = files::write_atomic(&program_dir.join(CURRENT_FILE), key.as_bytes());
            true
        } else {
            false
        }
    };
    if published {
        sweep_program(&program_dir);
    }
    Ok(Staged {
        executable: staged_exe,
        version_dir: Some(version_dir),
        _lease: Some(lease),
    })
}

/// Stage the installed executable beside any running older versions and sweep unused ones.
pub fn stage_current(
    home_root: &Path,
    program: &str,
    source: &Path,
    npm_package: Option<&str>,
) -> io::Result<Option<PathBuf>> {
    let staged = prepare(home_root, program, source, npm_package)?;
    let dir = staged.version_dir.clone();
    drop(staged);
    sweep_program(&program_root(home_root, program));
    Ok(dir)
}

/// Whether an adopted holder's executable belongs to a version superseded by `current`.
pub fn is_outdated_executable(home_root: &Path, program: &str, executable: &Path) -> bool {
    let Some((dir, version_name)) = staged_location(executable) else {
        return false;
    };
    let expected = program_root(home_root, program);
    let same = dir == expected || fs::canonicalize(&dir).ok() == fs::canonicalize(&expected).ok();
    same && read_current(&expected).is_some_and(|current| current != version_name)
}

/// Sweep unused versions, retired/aborted directories, orphaned leases, and legacy drain markers.
pub fn sweep_all(home_root: &Path) {
    let agents = home_root.join("agents");
    let _ = fs::remove_dir_all(agents.join("program-draining"));
    let _ = fs::remove_dir_all(agents.join("program-installing"));
    let Ok(entries) = fs::read_dir(home_root.join("run/agents/programs")) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            sweep_program(&entry.path());
        }
    }
}

pub fn sweep_program(program_dir: &Path) {
    let Ok(entries) = fs::read_dir(program_dir) else {
        return;
    };
    let current = read_current(program_dir);
    let (mut to_delete, mut versions, mut leases, mut stagings) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(text) = name.to_str() else { continue };
        let path = entry.path();
        if text.starts_with(RETIRED_PREFIX) {
            to_delete.push(path);
        } else if text.starts_with(STAGING_PREFIX) {
            stagings.push(path);
        } else if let Some(version) = text.strip_suffix(LEASE_SUFFIX) {
            if version.starts_with(VERSION_PREFIX) && !program_dir.join(version).exists() {
                leases.push(version.to_owned());
            }
        } else if text.starts_with(VERSION_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && current.as_deref() != Some(text)
        {
            versions.push(text.to_owned());
        }
    }
    if !stagings.is_empty()
        && let Ok(_install) = locks::try_file_lock(&program_dir.join(INSTALL_LOCK))
    {
        to_delete.extend(stagings);
    }
    if (!versions.is_empty() || !leases.is_empty())
        && let Ok(_leases) = lock_exclusive(&program_dir.join(LEASES_LOCK))
    {
        let current = read_current(program_dir);
        for version in versions {
            if current.as_deref() != Some(version.as_str())
                && let Some(retired) = retire_version_locked(program_dir, &version)
            {
                to_delete.push(retired);
            }
        }
        for version in leases {
            let path = lease_file(program_dir, &version);
            if !program_dir.join(&version).exists()
                && let Ok(guard) = locks::try_file_lock(&path)
            {
                drop(guard);
                let _ = fs::remove_file(&path);
            }
        }
    }
    for path in to_delete {
        remove_tree(&path);
    }
}

fn retire_if_unused(program_dir: &Path, version_name: &str) {
    if read_current(program_dir).as_deref() == Some(version_name) {
        return;
    }
    let retired = {
        let Ok(_leases) = lock_exclusive(&program_dir.join(LEASES_LOCK)) else {
            return;
        };
        if read_current(program_dir).as_deref() == Some(version_name) {
            return;
        }
        retire_version_locked(program_dir, version_name)
    };
    if let Some(retired) = retired {
        remove_tree(&retired);
    }
}

fn retire_version_locked(program_dir: &Path, version_name: &str) -> Option<PathBuf> {
    let version_dir = program_dir.join(version_name);
    if !version_dir.is_dir() {
        return None;
    }
    let lease_path = lease_file(program_dir, version_name);
    let guard = locks::try_file_lock(&lease_path).ok()?;
    let retired = tempfile::Builder::new()
        .prefix(RETIRED_PREFIX)
        .tempdir_in(program_dir)
        .ok()?
        .keep();
    if retry_windows(|| fs::rename(&version_dir, retired.join(version_name))).is_err() {
        let _ = fs::remove_dir(&retired);
        return None;
    }
    drop(guard);
    let _ = fs::remove_file(&lease_path);
    Some(retired)
}

fn lease_file(program_dir: &Path, version_name: &str) -> PathBuf {
    program_dir.join(format!("{version_name}{LEASE_SUFFIX}"))
}

fn read_current(program_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(program_dir.join(CURRENT_FILE)).ok()?;
    let name = text.trim();
    name.starts_with(VERSION_PREFIX).then(|| name.to_owned())
}

fn staged_location(executable: &Path) -> Option<(PathBuf, String)> {
    for ancestor in executable.ancestors().skip(1) {
        let name = ancestor.file_name()?.to_str()?;
        if name.starts_with(VERSION_PREFIX)
            && let Some(program_dir) = ancestor.parent()
            && program_dir.parent().and_then(Path::file_name) == Some(OsStr::new("programs"))
        {
            return Some((program_dir.to_owned(), name.to_owned()));
        }
    }
    None
}

fn lock_exclusive(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        files::create_private_dirs(parent)?;
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

fn retry_windows(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let attempts = if cfg!(windows) { 10 } else { 1 };
    for attempt in 0..attempts {
        match op() {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && attempt + 1 < attempts
                    && (error.kind() == io::ErrorKind::PermissionDenied
                        || matches!(error.raw_os_error(), Some(5 | 32 | 33))) =>
            {
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn remove_tree(path: &Path) {
    let _ = retry_windows(|| match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    });
}

struct StagePlan {
    relative_entry: PathBuf,
    files: Vec<(PathBuf, PathBuf)>,
    package_dir: Option<(PathBuf, PathBuf)>,
}

impl StagePlan {
    fn for_source(source: &Path, npm_package: Option<&str>) -> io::Result<Option<Self>> {
        let real = fs::canonicalize(source).unwrap_or_else(|_| source.to_owned());
        if inside_app_bundle(&real) {
            return Ok(None);
        }
        let file_name = source
            .file_name()
            .or_else(|| real.file_name())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Program has no name."))?
            .to_owned();
        if let Some(package) = npm_package
            && let Some(prefix) = npm_global_prefix(&real.to_string_lossy(), package)
        {
            let pkg_root = PathBuf::from(&prefix).join("lib/node_modules").join(package);
            if let Ok(rel_entry) = real.strip_prefix(&pkg_root)
                && pkg_root.is_dir()
            {
                let rel_pkg = PathBuf::from("node_modules").join(package);
                return Ok(Some(Self {
                    relative_entry: rel_pkg.join(rel_entry),
                    files: Vec::new(),
                    package_dir: Some((pkg_root, rel_pkg)),
                }));
            }
        }
        let is_shim = real
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        if is_shim && let Some(parent) = real.parent() {
            let mut copy_files = vec![(real.clone(), PathBuf::from(&file_name))];
            let package_dir = npm_package.and_then(|pkg| {
                let dir = parent.join("node_modules").join(pkg);
                dir.is_dir()
                    .then(|| (dir, PathBuf::from("node_modules").join(pkg)))
            });
            if package_dir.is_none()
                && let Some(stem) = real.file_stem().and_then(OsStr::to_str)
            {
                for ext in ["cjs", "js", "mjs", "exe", "ps1"] {
                    let sibling = parent.join(format!("{stem}.{ext}"));
                    if sibling.is_file() {
                        copy_files.push((sibling, PathBuf::from(format!("{stem}.{ext}"))));
                    }
                }
            }
            return Ok(Some(Self {
                relative_entry: PathBuf::from(&file_name),
                files: copy_files,
                package_dir,
            }));
        }
        Ok(Some(Self {
            relative_entry: PathBuf::from(&file_name),
            files: vec![(real, PathBuf::from(&file_name))],
            package_dir: None,
        }))
    }

    fn version_key(&self) -> io::Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(self.relative_entry.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        for (source, dest) in &self.files {
            hasher.update(dest.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            hash_file(&mut hasher, source)?;
        }
        if let Some((pkg_dir, rel_dest)) = &self.package_dir {
            hasher.update(rel_dest.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
            hash_file(&mut hasher, &pkg_dir.join("package.json"))?;
            if let Ok(rel) = self.relative_entry.strip_prefix(rel_dest)
                && pkg_dir.join(rel).is_file()
            {
                hash_file(&mut hasher, &pkg_dir.join(rel))?;
            }
        }
        Ok(format!("{VERSION_PREFIX}{}", &hex::encode(hasher.finalize())[..16]))
    }

    fn copy_into(&self, staging_dir: &Path) -> io::Result<()> {
        if let Some((source_dir, rel_dest)) = &self.package_dir {
            copy_dir_recursive(source_dir, &staging_dir.join(rel_dest))?;
        }
        for (source, rel_dest) in &self.files {
            let target = staging_dir.join(rel_dest);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, &target)?;
        }
        Ok(())
    }
}

fn inside_app_bundle(path: &Path) -> bool {
    let mut saw_app = false;
    for component in path.components() {
        let text = component.as_os_str().to_string_lossy();
        if saw_app && text == "Contents" {
            return true;
        }
        saw_app = text.ends_with(".app");
    }
    false
}

fn hash_file(hasher: &mut Sha256, path: &Path) -> io::Result<()> {
    let meta = fs::metadata(path)?;
    hasher.update(path.as_os_str().as_encoded_bytes());
    hasher.update(b"\0");
    hasher.update(meta.len().to_le_bytes());
    if let Ok(modified) = meta.modified()
        && let Ok(since) = modified.duration_since(SystemTime::UNIX_EPOCH)
    {
        hasher.update(since.as_secs().to_le_bytes());
        hasher.update(since.subsec_nanos().to_le_bytes());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hasher.update(meta.dev().to_le_bytes());
        hasher.update(meta.ino().to_le_bytes());
    }
    let mut file = File::open(path)?;
    if meta.len() <= FULL_HASH_LIMIT {
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.read_to_end(&mut bytes)?;
        hasher.update(&bytes);
    } else {
        let mut buf = [0u8; EDGE_HASH_BYTES];
        let n = file.read(&mut buf)?;
        hasher.update(&buf[..n]);
        if file.seek(SeekFrom::End(-(EDGE_HASH_BYTES as i64))).is_ok() {
            let n = file.read(&mut buf)?;
            hasher.update(&buf[..n]);
        }
    }
    Ok(())
}

fn copy_dir_recursive(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            {
                let link = fs::read_link(&source)?;
                std::os::unix::fs::symlink(&link, &target)?;
            }
            #[cfg(windows)]
            if source.is_dir() {
                copy_dir_recursive(&source, &target)?;
            } else {
                fs::copy(&source, &target)?;
            }
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}
