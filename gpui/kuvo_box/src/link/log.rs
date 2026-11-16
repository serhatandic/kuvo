//! The link's own log: each relay connection made and lost, each session with a box ended, and
//! each message sent again, with why, so a "Couldn't connect" can be told apart afterwards. Never
//! shown on screen; only what the app names ([`to_file`]) gets it, and nothing of what's sent (no
//! words, no files, no keys): times, sizes, which computer and why.
//!
//! Kept small: past `MOST` bytes it starts over, the one before kept beside it (`.1`).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How big the log grows before it starts over.
const MOST: u64 = 512 * 1024;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where the link's log goes (the app's own folder); nothing is logged before this.
pub fn to_file(path: PathBuf) {
    if let Some(folder) = path.parent() {
        let mut folders = std::fs::DirBuilder::new();
        folders.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            folders.mode(0o700);
        }
        let _ = folders.create(folder);
    }
    *FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(path);
}

/// Notes `what`, with the time (UTC).
pub fn note(what: impl AsRef<str>) {
    let file = FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(path) = file.as_ref() else { return };
    if std::fs::metadata(path).is_ok_and(|kept| kept.len() > MOST) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    let line = format!("{} {}\n", stamp(SystemTime::now()), what.as_ref());
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut opened) = options.open(path) {
        let _ = opened.write_all(line.as_bytes());
    }
}

/// `at` as `2026-11-16 19:38:02.120Z`.
fn stamp(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (days, seconds) = ((since.as_secs() / 86_400) as i64, since.as_secs() % 86_400);
    // Days since 1970 to a date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}Z", seconds / 3_600, seconds % 3_600 / 60, seconds % 60, since.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn times_read_as_dates() {
        assert_eq!(stamp(UNIX_EPOCH), "1970-01-01 00:00:00.000Z");
        // 2026-11-16 16:38:02.120 UTC (19:38 in Istanbul).
        assert_eq!(stamp(UNIX_EPOCH + Duration::from_millis(1_794_847_082_120)), "2026-11-16 16:38:02.120Z");
        assert_eq!(stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29 00:00:00.000Z");
    }
}
