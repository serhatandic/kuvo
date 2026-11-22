//! Pictures pasted into a composer, kept as files to attach like any other (0.7's
//! `Attachments.save`): a picture the engine takes (PNG, JPEG, GIF, WebP) as it came, any other
//! (TIFF from Safari or Preview, BMP) as a PNG, or a JPEG for a large opaque photo, which would
//! otherwise be huge. Named like the system's screenshots: "Pasted Image 2026-11-16 at 15.32.10".
//!
//! They're kept in the app's own folder (`~/.kuvo-code/desktop/Pasted`), so they go when it does,
//! and each is let go a week after it was pasted: a message carries its pictures' bytes (as to
//! another computer: `parts::carried`), never a path to one here, so its box keeps its own copy.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{App, Global, Image, ImageFormat};

/// Where pasted pictures are kept (`main` sets the app's; tests their own folder).
pub struct PastedFolder(pub PathBuf);

impl Global for PastedFolder {}

/// How long a pasted picture is kept.
const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Above this many pixels an opaque picture is kept as a JPEG (as 0.7 did).
const LARGE_PIXELS: u64 = 4_000_000;

/// The folder pasted pictures go to: the app's, or the system's temporary folder where it has none
/// set (a build run without a folder of its own).
pub fn folder(cx: &App) -> PathBuf {
    cx.try_global::<PastedFolder>().map(|folder| folder.0.clone()).unwrap_or_else(|| std::env::temp_dir().join("kuvo-pasted"))
}

/// `image` written to a new file in `folder`, named for `now`; what went wrong, in plain words,
/// where it couldn't be. Pictures pasted more than a week ago are let go first.
pub fn save(image: &Image, folder: &Path, now: SystemTime) -> Result<PathBuf, String> {
    let (bytes, extension) = engine_bytes(image).ok_or_else(|| "That picture couldn’t be read.".to_owned())?;
    std::fs::create_dir_all(folder).map_err(|_| "The picture couldn’t be kept to attach.".to_owned())?;
    let_old_go(folder, now);
    let name = format!("Pasted Image {}", stamp(now));
    for counter in 1.. {
        let file = if counter == 1 { format!("{name}.{extension}") } else { format!("{name} {counter}.{extension}") };
        let path = folder.join(file);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut out) => {
                use std::io::Write as _;
                return match out.write_all(&bytes) {
                    Ok(()) => Ok(path),
                    Err(_) => {
                        let _ = std::fs::remove_file(&path);
                        Err("The picture couldn’t be kept to attach.".to_owned())
                    }
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("The picture couldn’t be kept to attach.".to_owned()),
        }
    }
    unreachable!("a free name is always found")
}

/// The picture's bytes as the engine takes them, with their extension.
fn engine_bytes(image: &Image) -> Option<(Vec<u8>, &'static str)> {
    let kept = match image.format() {
        ImageFormat::Png => Some("png"),
        ImageFormat::Jpeg => Some("jpg"),
        ImageFormat::Gif => Some("gif"),
        ImageFormat::Webp => Some("webp"),
        // (Text the engine reads as it is.)
        ImageFormat::Svg => Some("svg"),
        _ => None,
    };
    if let Some(extension) = kept {
        return (!image.bytes().is_empty()).then(|| (image.bytes().to_vec(), extension));
    }
    let decoded = image::load_from_memory(image.bytes()).ok()?;
    let large = u64::from(decoded.width()) * u64::from(decoded.height()) > LARGE_PIXELS;
    let mut out = std::io::Cursor::new(Vec::new());
    if large && !decoded.color().has_alpha() {
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
        decoded.to_rgb8().write_with_encoder(encoder).ok()?;
        Some((out.into_inner(), "jpg"))
    } else {
        decoded.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some((out.into_inner(), "png"))
    }
}

/// What was kept here more than `KEPT_FOR` ago, let go: pictures pasted, and copies of files
/// attached from another app's data (each in a folder of its own).
fn let_old_go(folder: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().ok().and_then(|meta| meta.modified().ok()).and_then(|at| now.duration_since(at).ok()).is_some_and(|age| age > KEPT_FOR);
        let Ok(kind) = entry.file_type() else { continue };
        if !old {
            continue;
        }
        if kind.is_file() && entry.file_name().to_string_lossy().starts_with("Pasted Image ") {
            let _ = std::fs::remove_file(entry.path());
        } else if kind.is_dir() && entry.file_name().to_string_lossy().starts_with("copy-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether `path` is in another app's own data (its container: Notes' pictures, Mail's), which
/// macOS asks you about when Kuvo reads it later. (Not iCloud Drive's or a cloud drive's files in
/// `~/Library`, which are yours.)
pub fn is_another_apps(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return false };
    path.starts_with(home.join("Library/Containers")) || path.starts_with(home.join("Library/Group Containers"))
}

/// A copy of `path` kept in `folder` (a folder of its own there, so it keeps its name), read now,
/// while you hand it over. What went wrong, in plain words, where it couldn't be.
pub fn keep_copy(path: &Path, folder: &Path, now: SystemTime) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or_else(|| "That file couldn’t be read.".to_owned())?;
    kept(name.as_ref(), folder, now, |copy| std::fs::copy(path, copy).map(|_| ()))
}

/// `bytes` kept in `folder` as a file named `name` (a folder of its own there), to attach again: a
/// file a message taken back to edit went with, that isn't on this computer's disk (sent with its
/// bytes, or kept on another computer's box). It goes again with its bytes, as a pasted picture does.
pub fn keep_bytes(bytes: &[u8], name: &str, folder: &Path, now: SystemTime) -> Result<PathBuf, String> {
    // (Its name alone: never a path out of its folder.)
    let name = Path::new(name).file_name().filter(|name| !name.is_empty()).unwrap_or("attachment".as_ref());
    kept(name, folder, now, |copy| std::fs::write(copy, bytes))
}

fn kept(name: &std::ffi::OsStr, folder: &Path, now: SystemTime, write: impl Fn(&Path) -> std::io::Result<()>) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|_| "That file couldn’t be kept to attach.".to_owned())?;
    let_old_go(folder, now);
    let stamp = now.duration_since(SystemTime::UNIX_EPOCH).map(|since| since.as_nanos()).unwrap_or(0);
    for n in 0.. {
        let own = folder.join(format!("copy-{stamp}-{n}"));
        match std::fs::create_dir(&own) {
            Ok(()) => {
                let copy = own.join(name);
                return match write(&copy) {
                    Ok(()) => Ok(copy),
                    Err(_) => {
                        let _ = std::fs::remove_dir_all(&own);
                        Err("That file couldn’t be read.".to_owned())
                    }
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("That file couldn’t be kept to attach.".to_owned()),
        }
    }
    unreachable!("a free folder is always found")
}

/// `now` on this computer's clock: "2026-11-16 at 15.32.10".
fn stamp(now: SystemTime) -> String {
    let seconds = now.duration_since(SystemTime::UNIX_EPOCH).map(|since| since.as_secs() as i64).unwrap_or(0);
    let local = seconds + utc_offset(seconds);
    let (year, month, day) = crate::time::civil(local.div_euclid(86_400));
    let of_day = local.rem_euclid(86_400);
    format!("{year:04}-{month:02}-{day:02} at {:02}.{:02}.{:02}", of_day / 3600, of_day % 3600 / 60, of_day % 60)
}

/// How far this computer's clock is ahead of UTC at `seconds`, in seconds.
#[cfg(target_os = "macos")]
pub(crate) fn utc_offset(seconds: i64) -> i64 {
    let time: libc::time_t = seconds as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` writes only into `parts`, which lives through the call.
    let found = unsafe { libc::localtime_r(&time, &mut parts) };
    if found.is_null() { 0 } else { parts.tm_gmtoff as i64 }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn utc_offset(_seconds: i64) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        if alpha {
            image::RgbaImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        } else {
            image::RgbImage::new(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn a_pasted_screenshot_is_kept_as_it_came_and_a_second_gets_its_own_name() {
        let folder = tempfile::tempdir().unwrap();
        let bytes = png(4, 3, true);
        let image = Image::from_bytes(ImageFormat::Png, bytes.clone());
        let now = SystemTime::now();
        let first = save(&image, folder.path(), now).unwrap();
        let second = save(&image, folder.path(), now).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), bytes);
        assert_ne!(first, second);
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("Pasted Image ") && name.ends_with(".png"), "{name}");
        assert!(second.file_name().unwrap().to_string_lossy().ends_with(" 2.png"));
    }

    #[test]
    fn a_tiff_from_safari_or_preview_is_kept_as_a_png_the_engine_takes() {
        let folder = tempfile::tempdir().unwrap();
        let mut tiff = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(5, 5).write_to(&mut tiff, image::ImageFormat::Tiff).unwrap();
        let path = save(&Image::from_bytes(ImageFormat::Tiff, tiff.into_inner()), folder.path(), SystemTime::now()).unwrap();
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(image::open(&path).unwrap().width(), 5);
    }

    #[test]
    fn what_isnt_a_picture_says_so_and_leaves_nothing() {
        let folder = tempfile::tempdir().unwrap();
        let error = save(&Image::from_bytes(ImageFormat::Tiff, b"not a picture".to_vec()), folder.path(), SystemTime::now()).unwrap_err();
        assert_eq!(error, "That picture couldn’t be read.");
        assert!(std::fs::read_dir(folder.path()).map(|entries| entries.count() == 0).unwrap_or(true));
    }

    #[test]
    fn pictures_pasted_over_a_week_ago_are_let_go_and_nothing_else() {
        let folder = tempfile::tempdir().unwrap();
        let old = folder.path().join("Pasted Image 2026-01-01 at 10.00.00.png");
        let yours = folder.path().join("notes.txt");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&yours, b"x").unwrap();
        let later = SystemTime::now() + Duration::from_secs(8 * 24 * 60 * 60);
        save(&Image::from_bytes(ImageFormat::Png, png(1, 1, false)), folder.path(), later).unwrap();
        assert!(!old.exists());
        assert!(yours.exists());
    }

    #[test]
    fn a_file_from_another_apps_data_is_copied_once_under_its_own_name() {
        let folder = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        let picture = notes.path().join("Pasted Graphic 3.png");
        std::fs::write(&picture, b"png").unwrap();
        let copy = keep_copy(&picture, folder.path(), SystemTime::now()).unwrap();
        assert_eq!(copy.file_name().unwrap(), "Pasted Graphic 3.png");
        assert_eq!(std::fs::read(&copy).unwrap(), b"png");
        let again = keep_copy(&picture, folder.path(), SystemTime::now()).unwrap();
        assert_ne!(copy, again, "each in a folder of its own");
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap();
        assert!(is_another_apps(&home.join("Library/Group Containers/group.com.apple.notes/x.png")));
        assert!(!is_another_apps(&home.join("Desktop/x.png")));
        assert!(!is_another_apps(&home.join("Library/Mobile Documents/com~apple~CloudDocs/x.png")), "iCloud Drive is yours");
    }

    #[test]
    fn the_name_reads_like_a_screenshot() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_763_303_530);
        let stamp = stamp(at);
        assert_eq!(stamp.len(), "2025-11-16 at 14.32.10".len());
        assert!(stamp.contains(" at ") && stamp.matches('.').count() == 2, "{stamp}");
    }
}
