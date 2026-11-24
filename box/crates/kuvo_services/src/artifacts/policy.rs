//! Copy policy for artifact versions: secrets, git-ignored files, git-tracked code,
//! and the free-space floor, matching `artifacts/policy.ts`.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::kinds::basename;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoCopy {
    Secret,
    Ignored,
    Git,
    TooBig,
    LowDisk,
    Cleared,
}

impl NoCopy {
    pub fn is_private(self) -> bool {
        matches!(self, Self::Secret | Self::Ignored)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskSpace {
    pub free: u64,
    pub total: u64,
}

pub type DiskProbe = Arc<dyn Fn() -> DiskSpace + Send + Sync>;

/// Whether a file's name or folder says it likely holds a secret.
pub fn is_secret(path: &str) -> bool {
    let name = basename(path);
    if name == ".env" || name.starts_with(".env.") {
        return true;
    }
    if let Some(dot) = name.rfind('.') {
        let ext = name[dot + 1..].to_ascii_lowercase();
        if matches!(
            ext.as_str(),
            "pem" | "key" | "p12" | "pfx" | "keystore" | "jks"
        ) {
            return true;
        }
    }
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("id_") {
        let core = rest.strip_suffix(".pub").unwrap_or(rest);
        if !core.is_empty()
            && core
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return true;
        }
    }
    path.split(['/', '\\'])
        .any(|part| matches!(part, ".ssh" | "keys" | ".gnupg"))
}

/// The free-space floor: whichever is less of 10 GiB and a tenth of the disk.
pub fn floor_of(total: u64) -> u64 {
    let ten_gib = 10u64 * 1024 * 1024 * 1024;
    ten_gib.min(total / 10)
}

#[derive(Default)]
struct Cache {
    tops: HashMap<PathBuf, Option<PathBuf>>,
    ignored: HashMap<PathBuf, bool>,
}

#[derive(Clone)]
pub struct CopyPolicy {
    disk: DiskProbe,
    cache: Arc<Mutex<Cache>>,
}

impl CopyPolicy {
    pub fn new(disk: DiskProbe) -> Self {
        Self {
            disk,
            cache: Arc::new(Mutex::new(Cache::default())),
        }
    }

    pub fn for_folder(folder: PathBuf) -> Self {
        Self::new(Arc::new(move || free_space(&folder)))
    }

    pub fn privacy(&self, path: &Path) -> Option<NoCopy> {
        if is_secret(&path.to_string_lossy()) {
            return Some(NoCopy::Secret);
        }
        let parent = path.parent()?;
        let top = self.top(parent);
        if top.is_some() {
            let cache = self.cache.lock().unwrap();
            if cache.ignored.get(path).copied() != Some(false) {
                return Some(NoCopy::Ignored);
            }
        }
        None
    }

    pub async fn learn(&self, paths: &[PathBuf]) {
        let mut asks: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let unique: HashSet<&PathBuf> = paths.iter().collect();
        for path in unique {
            if is_secret(&path.to_string_lossy()) {
                continue;
            }
            {
                let cache = self.cache.lock().unwrap();
                if cache.ignored.contains_key(path) {
                    continue;
                }
            }
            if let Some(parent) = path.parent()
                && let Some(top) = self.top(parent)
            {
                asks.entry(top).or_default().push(path.clone());
            }
        }
        let mut set = tokio::task::JoinSet::new();
        for (top, asked) in asks {
            set.spawn(async move {
                let ignored = match check_ignore(&top, &asked).await {
                    Some(found) => found,
                    None => {
                        let mut one_by_one = HashSet::new();
                        for path in &asked {
                            if check_ignore(&top, std::slice::from_ref(path))
                                .await
                                .is_some_and(|found| !found.is_empty())
                            {
                                one_by_one.insert(path.clone());
                            }
                        }
                        one_by_one
                    }
                };
                (asked, ignored)
            });
        }
        while let Some(Ok((asked, ignored))) = set.join_next().await {
            let mut cache = self.cache.lock().unwrap();
            for path in asked {
                let is_ignored = ignored.contains(&path);
                cache.ignored.insert(path, is_ignored);
            }
        }
    }

    pub fn in_git(&self, path: &Path) -> bool {
        path.parent().and_then(|parent| self.top(parent)).is_some()
    }

    pub fn low_on(&self, size: u64) -> bool {
        let space = (self.disk)();
        space.free.saturating_sub(size) < floor_of(space.total)
    }

    pub fn space(&self) -> (u64, u64) {
        let space = (self.disk)();
        (space.free, floor_of(space.total))
    }

    fn top(&self, folder: &Path) -> Option<PathBuf> {
        let mut cache = self.cache.lock().unwrap();
        top_locked(&mut cache.tops, folder)
    }
}

fn top_locked(tops: &mut HashMap<PathBuf, Option<PathBuf>>, folder: &Path) -> Option<PathBuf> {
    if let Some(known) = tops.get(folder) {
        return known.clone();
    }
    let found = if folder.join(".git").exists() {
        Some(folder.to_path_buf())
    } else if let Some(parent) = folder.parent()
        && parent != folder
    {
        top_locked(tops, parent)
    } else {
        None
    };
    tops.insert(folder.to_path_buf(), found.clone());
    found
}

fn git_rel(top: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(top).unwrap_or(path);
    let text = rel.to_string_lossy();
    #[cfg(windows)]
    let text = text.replace('\\', "/");
    format!("./{text}")
}

/// Returns `None` when git refused the batch because one path is beyond a symlink.
async fn check_ignore(top: &Path, paths: &[PathBuf]) -> Option<HashSet<PathBuf>> {
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(git_rel(top, path).as_bytes());
        input.push(0);
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["-C"])
        .arg(top)
        .args(["check-ignore", "-z", "--stdin"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let Ok(mut child) = cmd.spawn() else {
        return Some(paths.iter().cloned().collect());
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&input).await;
    }
    let Ok(Ok(output)) =
        tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await
    else {
        return Some(paths.iter().cloned().collect());
    };
    let code = output.status.code().unwrap_or(-1);
    if code == 128 && paths.len() > 1 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("beyond a symbolic link") {
            return None;
        }
    }
    if code != 0 && code != 1 {
        return Some(paths.iter().cloned().collect());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let said: HashSet<&str> = stdout.split('\0').filter(|s| !s.is_empty()).collect();
    Some(
        paths
            .iter()
            .filter(|path| said.contains(git_rel(top, path).as_str()))
            .cloned()
            .collect(),
    )
}

pub fn free_space(folder: &Path) -> DiskSpace {
    platform_free_space(folder).unwrap_or(DiskSpace {
        free: (1u64 << 53) - 1,
        total: (1u64 << 53) - 1,
    })
}

#[cfg(unix)]
fn platform_free_space(folder: &Path) -> Option<DiskSpace> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(folder.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated C string and `stats` is valid writable storage.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: `statvfs` returned 0, so `stats` is initialized.
    let stats = unsafe { stats.assume_init() };
    #[allow(clippy::unnecessary_cast)]
    let unit = if stats.f_frsize > 0 {
        stats.f_frsize as u64
    } else {
        stats.f_bsize as u64
    };
    #[allow(clippy::unnecessary_cast)]
    Some(DiskSpace {
        free: (stats.f_bavail as u64).saturating_mul(unit),
        total: (stats.f_blocks as u64).saturating_mul(unit),
    })
}

#[cfg(windows)]
fn platform_free_space(folder: &Path) -> Option<DiskSpace> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = folder
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut avail = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated UTF-16 and the three output pointers are valid writable u64s.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
    if ok == 0 {
        return None;
    }
    Some(DiskSpace { free: avail, total })
}

#[cfg(not(any(unix, windows)))]
fn platform_free_space(_folder: &Path) -> Option<DiskSpace> {
    None
}
