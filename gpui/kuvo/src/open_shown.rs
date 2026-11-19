//! Materializing and opening files shared in a conversation. Run these blocking operations off
//! the UI thread, using the client for the conversation's computer.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use kuvo_box::shown_files::{FILE_MOST, ShownFile};
use kuvo_box::{BoxClient, Error};

#[derive(Debug)]
pub enum OpenShownError {
    Box(Error),
    Io(io::Error),
}

impl From<Error> for OpenShownError {
    fn from(error: Error) -> Self {
        Self::Box(error)
    }
}

impl From<io::Error> for OpenShownError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for OpenShownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Box(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for OpenShownError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Box(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

/// Opens a local file in the default app, or reveals it if opening could run it. For a remote
/// file, downloads a fresh copy into the session's cache first.
pub fn open_shown_file(client: &BoxClient, shown: &ShownFile) -> Result<(), OpenShownError> {
    if client.here() {
        return open_path(local_path(shown)?);
    }

    let session = shown
        .session
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing session ID"))?;
    let name = Path::new(&shown.path)
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing file name"))?;
    let (bytes, _) = client.file_bytes(&shown.uri, FILE_MOST)?;
    let folder = cache_root()?.join(format!("session-{}", kuvo_box::paths::segment(session)));
    fs::create_dir_all(&folder)?;
    // A unique private directory keeps simultaneous downloads of the same name independent.
    let downloaded = tempfile::tempdir_in(folder)?;
    let path = downloaded.path().join(name);
    write_received(&bytes, &path)?;
    open_path(&path)?;
    let _ = downloaded.keep();
    Ok(())
}

/// Saves a copy to `dest`, replacing an existing destination only after the copy is complete.
pub fn save_copy(client: &BoxClient, shown: &ShownFile, dest: &Path) -> Result<(), OpenShownError> {
    if !client.here() {
        let (bytes, _) = client.file_bytes(&shown.uri, FILE_MOST)?;
        return Ok(write_received(&bytes, dest)?);
    }
    let path = local_path(shown)?;
    if let (Ok(source), Ok(target)) = (path.canonicalize(), dest.canonicalize())
        && source == target
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "The destination is the source file",
        )
        .into());
    }
    if !path.metadata().map_err(source_error)?.is_file() {
        return Err(Error::NotFound.into());
    }
    let mut source = open_for_read(path).map_err(source_error)?;
    if !source.metadata()?.is_file() {
        return Err(Error::NotFound.into());
    }
    let mut copy = copy_at(dest)?;
    io::copy(&mut source, &mut copy)?;
    copy.persist(dest).map_err(|error| error.error)?;
    Ok(())
}

fn local_path(shown: &ShownFile) -> io::Result<&Path> {
    let path = Path::new(&shown.path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an absolute file path",
        ))
    }
}

fn open_for_read(path: &Path) -> io::Result<File> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

fn source_error(error: io::Error) -> OpenShownError {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Error::NotFound.into(),
        _ => error.into(),
    }
}

fn cache_root() -> io::Result<PathBuf> {
    let dir = |name| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let home =
        dir("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Home directory"))?;
    let demo_root = std::env::var("KUVO_DEMO_MACHINE_ROOT").ok();
    let names =
        kuvo_box::install::names_for(&std::env::current_exe()?, demo_root.as_deref(), &home)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "App cache directory"))?;
    let caches = if cfg!(target_os = "macos") {
        home.join("Library/Caches")
    } else if let Some(cache) = dir("XDG_CACHE_HOME") {
        cache
    } else {
        home.join(".cache")
    };
    Ok(caches.join(names.app_id).join("shown-files"))
}

fn copy_at(dest: &Path) -> io::Result<tempfile::NamedTempFile> {
    let parent = dest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    tempfile::NamedTempFile::new_in(parent)
}

fn write_received(bytes: &[u8], dest: &Path) -> io::Result<()> {
    let mut copy = copy_at(dest)?;
    copy.write_all(bytes)?;
    #[cfg(target_os = "macos")]
    quarantine(copy.as_file())?;
    copy.persist(dest).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn quarantine(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let value = format!("0081;{seconds:x};Kuvo;");
    let result = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            c"com.apple.quarantine".as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn open_path(path: &Path) -> Result<(), OpenShownError> {
    let resolved = path.canonicalize().map_err(source_error)?;
    let metadata = resolved.metadata().map_err(source_error)?;
    let reveal = should_reveal(path, &resolved, &metadata).map_err(source_error)?;
    if !reveal && !metadata.is_file() {
        return Err(Error::NotFound.into());
    }
    open_or_reveal(&resolved, reveal, &metadata)
}

fn should_reveal(path: &Path, resolved: &Path, metadata: &fs::Metadata) -> io::Result<bool> {
    if runnable_extension(path) || runnable_extension(resolved) {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Ok(true);
        }
    }
    #[cfg(target_os = "macos")]
    if mac_runnable_type(resolved) {
        return Ok(true);
    }
    if !metadata.is_file() {
        return Ok(false);
    }
    // Detect native binaries and shebang scripts even when their name and permissions don't
    // indicate it (for example, a downloaded file has no executable bit).
    let mut head = [0; 4];
    open_for_read(resolved)?
        .take(4)
        .read_exact(&mut head)
        .or_else(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                Ok(())
            } else {
                Err(error)
            }
        })?;
    Ok(matches!(&head[..2], b"#!" | b"MZ")
        || matches!(
            &head,
            b"\x7fELF"
                | b"\xfe\xed\xfa\xce"
                | b"\xce\xfa\xed\xfe"
                | b"\xfe\xed\xfa\xcf"
                | b"\xcf\xfa\xed\xfe"
                | b"\xca\xfe\xba\xbe"
                | b"\xbe\xba\xfe\xca"
                | b"\xca\xfe\xba\xbf"
                | b"\xbf\xba\xfe\xca"
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stand_in::{Replies, StandIn};
    use kuvo_box::shown_files::FileKind;

    fn reveal(path: &Path) -> bool {
        let resolved = path.canonicalize().unwrap();
        should_reveal(path, &resolved, &resolved.metadata().unwrap()).unwrap()
    }

    #[test]
    fn runnable_names_and_file_headers_are_revealed() {
        let folder = tempfile::tempdir().unwrap();
        let bundle = folder.path().join("Example.APP");
        fs::create_dir(&bundle).unwrap();
        assert!(reveal(&bundle));
        for (name, bytes) in [
            ("signed.DMG", b"disk".as_slice()),
            ("installer.pkg", b"package".as_slice()),
            ("run.sh", b"exit".as_slice()),
            ("build.PY", b"pass".as_slice()),
            ("link.webloc", b"link".as_slice()),
            ("setup", b"#!/usr/bin/env bash".as_slice()),
            ("short", b"#!".as_slice()),
            ("binary", b"\xcf\xfa\xed\xfe".as_slice()),
            ("data.bin", b"\x7fELF".as_slice()),
        ] {
            let file = folder.path().join(name);
            fs::write(&file, bytes).unwrap();
            assert!(reveal(&file), "{name}");
        }
        let document = folder.path().join("report.pdf");
        fs::write(&document, b"%PDF").unwrap();
        assert!(!reveal(&document));
    }

    #[cfg(unix)]
    #[test]
    fn execute_permissions_and_symlink_targets_are_checked() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let folder = tempfile::tempdir().unwrap();
        let executable = folder.path().join("test");
        fs::write(&executable, "").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(reveal(&executable));

        let script = folder.path().join("tool.command");
        fs::write(&script, "exit").unwrap();
        let link = folder.path().join("report.pdf");
        symlink(script, &link).unwrap();
        assert!(reveal(&link));
    }

    #[test]
    fn save_copy_uses_the_conversations_box_and_leaves_existing_files_on_failure() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("source.pdf");
        let dest = folder.path().join("copy.pdf");
        let path = source.to_str().unwrap();
        let shown = ShownFile {
            id: "file".into(),
            uri: kuvo_box::pictures::file_uri(path),
            path: path.into(),
            name: "source.pdf".into(),
            caption: None,
            kind: FileKind::Pdf,
            computer: None,
            session: None,
        };
        let stand_in = StandIn::new(Replies::Never);
        let client = BoxClient::new(stand_in.clone());
        fs::write(&source, "local").unwrap();
        assert!(
            matches!(save_copy(&client, &shown, &source), Err(OpenShownError::Io(error))
            if error.kind() == io::ErrorKind::InvalidInput)
        );
        save_copy(&client, &shown, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"local");

        stand_in.set_elsewhere(true);
        assert!(matches!(
            save_copy(&client, &shown, &dest),
            Err(OpenShownError::Box(Error::NotFound))
        ));
        assert_eq!(fs::read(&dest).unwrap(), b"local");
        stand_in.put_file(path, b"remote".to_vec());
        assert_eq!(client.file_info(&shown.uri).unwrap().kind, FileKind::Pdf);
        save_copy(&client, &shown, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"remote");
        assert_eq!(fs::read(&source).unwrap(), b"local");
    }
}

fn runnable_extension(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        extension.as_deref(),
        Some(
            "app"
                | "bundle"
                | "dmg"
                | "iso"
                | "pkg"
                | "mpkg"
                | "exe"
                | "msi"
                | "com"
                | "scr"
                | "appimage"
                | "command"
                | "tool"
                | "sh"
                | "bash"
                | "zsh"
                | "fish"
                | "csh"
                | "ksh"
                | "py"
                | "pyw"
                | "rb"
                | "pl"
                | "php"
                | "lua"
                | "js"
                | "mjs"
                | "cjs"
                | "scpt"
                | "applescript"
                | "bat"
                | "cmd"
                | "ps1"
                | "vbs"
                | "vbe"
                | "wsf"
                | "jar"
                | "jnlp"
                | "terminal"
                | "mobileconfig"
                | "workflow"
                | "shortcut"
                | "webloc"
                | "inetloc"
                | "url"
                | "lnk"
                | "desktop"
        )
    )
}

#[cfg(target_os = "macos")]
fn mac_runnable_type(path: &Path) -> bool {
    use objc2_foundation::{NSArray, NSString, NSURL, NSURLContentTypeKey};
    use objc2_uniform_type_identifiers::*;

    let Some(path) = path.to_str() else {
        return false;
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let key = unsafe { NSURLContentTypeKey };
    let Some(kind) = url
        .resourceValuesForKeys_error(&NSArray::from_slice(&[key]))
        .ok()
        .and_then(|values| values.objectForKey(key))
        .and_then(|value| value.downcast::<UTType>().ok())
    else {
        return false;
    };
    // Finder aliases and file types installed by other apps can be recognized by the system.
    unsafe {
        [
            UTTypeExecutable,
            UTTypeScript,
            UTTypeApplication,
            UTTypeBundle,
            UTTypeDiskImage,
            UTTypeInternetLocation,
            UTTypeInternetShortcut,
            UTTypeAliasFile,
        ]
        .iter()
        .any(|runnable| kind.conformsToType(runnable))
    }
}

fn open_or_reveal(
    path: &Path,
    reveal: bool,
    metadata: &fs::Metadata,
) -> Result<(), OpenShownError> {
    #[cfg(target_os = "macos")]
    let status = {
        use std::os::unix::fs::MetadataExt;

        let mut command = std::process::Command::new("/usr/bin/open");
        if reveal {
            command.arg("-R");
        } else {
            // Bind the launch to the default application. Asking open to choose the application
            // itself could launch the shared path if it were replaced with an app in between.
            let app = default_application(path)?;
            let now = path.symlink_metadata().map_err(source_error)?;
            if !now.is_file() || (now.dev(), now.ino()) != (metadata.dev(), metadata.ino()) {
                return Err(io::Error::other("The file changed before it could be opened").into());
            }
            command.arg("-a").arg(app);
        }
        command.arg(path).status()?
    };
    #[cfg(target_os = "linux")]
    let status = {
        let _ = metadata;
        let target = if reveal {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        std::process::Command::new("xdg-open")
            .arg(target)
            .status()?
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("File opener exited with {status}")).into())
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (path, reveal, metadata);
        Err(io::Error::from(io::ErrorKind::Unsupported).into())
    }
}

#[cfg(target_os = "macos")]
fn default_application(path: &Path) -> io::Result<PathBuf> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let path = path.to_str().ok_or(io::ErrorKind::InvalidData)?;
    let app = objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
        app.path().map(|name| PathBuf::from(name.to_string()))
    });
    app.filter(|app| app.is_absolute() && app != Path::new(path))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Default application"))
}
