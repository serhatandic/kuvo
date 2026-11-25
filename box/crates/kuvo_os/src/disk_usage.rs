//! Allocated directory sizes, measured away from the request loop.
use std::{collections::BTreeMap, io, path::PathBuf, time::Duration};

#[cfg(unix)]
pub async fn measure(paths: Vec<PathBuf>, depth: usize) -> io::Result<BTreeMap<PathBuf, u64>> {
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut command = tokio::process::Command::new("du");
    command
        .args(["-k", "-d", &depth.to_string(), "--"])
        .args(paths)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(600), command.output())
        .await
        .map_err(|_| {
            io::Error::other("measuring took too long (a folder on a slow or stalled disk?).")
        })??;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (kb, path) = line.split_once('\t')?;
            Some((
                PathBuf::from(path),
                kb.parse::<u64>().ok()?.saturating_mul(1024),
            ))
        })
        .collect())
}

#[cfg(windows)]
pub async fn measure(paths: Vec<PathBuf>, depth: usize) -> io::Result<BTreeMap<PathBuf, u64>> {
    tokio::task::spawn_blocking(move || {
        let mut sizes = BTreeMap::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(600);
        fn walk(
            path: &std::path::Path,
            depth: usize,
            show: bool,
            deadline: std::time::Instant,
            sizes: &mut BTreeMap<PathBuf, u64>,
        ) -> io::Result<u64> {
            if std::time::Instant::now() > deadline {
                return Err(io::Error::other(
                    "measuring took too long (a folder on a slow or stalled disk?).",
                ));
            }
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                return Ok(0);
            };
            if meta.file_type().is_symlink() {
                return Ok(0);
            }
            let mut bytes = meta.len();
            if meta.is_dir() {
                for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                    bytes = bytes.saturating_add(walk(
                        &entry.path(),
                        depth.saturating_sub(1),
                        show && depth > 0,
                        deadline,
                        sizes,
                    )?);
                }
                if show {
                    sizes.insert(path.to_owned(), bytes);
                }
            }
            Ok(bytes)
        }
        for path in paths {
            walk(&path, depth, true, deadline, &mut sizes)?;
        }
        Ok(sizes)
    })
    .await
    .map_err(io::Error::other)?
}
