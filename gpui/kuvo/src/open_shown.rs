//! Materializing and opening files shared in a conversation. Run these blocking operations off
//! the UI thread, using the client for the conversation's computer.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use kuvo_box::shown_files::{FILE_MOST, ShownFile};
use kuvo_box::{BoxClient, Error};

mod cache;

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
/// file, reads the current bytes and source mode and materializes that version in the session cache.
pub fn open_shown_file(client: &BoxClient, shown: &ShownFile) -> Result<(), OpenShownError> {
    if client.here() {
        return open_path(local_path(shown)?, false);
    }

    let root = cache_root()?;
    let (bytes, executable) = client.file_for_open(&shown.uri)?;
    let (path, reveal) = {
        let _cache = cache::LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let prepared = materialize_download(&root, shown, &bytes, executable)?;
        cache::prune(&root, prepared.0.parent());
        prepared
    };
    open_path(&path, reveal)
}

/// A Library item opened in its default app: this Mac's own file where it's on this Mac, else its
/// bytes from the computer it's on (or the record's copy once it's gone), kept in the same cache as a
/// shown file's. What could run something is revealed, as for a shown file.
pub fn open_library_item(client: &BoxClient, item: &kuvo_box::library::LibraryItem) -> Result<(), OpenShownError> {
    let path = Path::new(&item.path);
    if client.here() && item.is_here() && path.is_file() {
        return open_path(path, false);
    }
    let root = cache_root()?;
    let bytes = client.library_file(item)?;
    let shown = ShownFile {
        id: item.id.clone(),
        uri: format!("library:{}", item.id),
        path: item.path.clone(),
        name: item.name.clone(),
        caption: None,
        kind: kuvo_box::shown_files::FileKind::Other,
        computer: item.computer.as_ref().map(|computer| computer.key.clone()),
        session: Some(item.session.clone().or_else(|| item.group.clone()).unwrap_or_else(|| "library".into())),
    };
    let (path, reveal) = {
        let _cache = cache::LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let prepared = materialize_download(&root, &shown, &bytes, Some(false))?;
        cache::prune(&root, prepared.0.parent());
        prepared
    };
    open_path(&path, reveal)
}

fn materialize_download(
    root: &Path,
    shown: &ShownFile,
    bytes: &[u8],
    executable: Option<bool>,
) -> io::Result<(PathBuf, bool)> {
    // Older boxes do not report source mode. Treat their downloads conservatively.
    Ok((cache::put(root, shown, bytes)?, executable != Some(false)))
}

/// Evict old and excess remote downloads during normal app startup.
pub fn prune_download_cache() {
    if let Ok(root) = cache_root() {
        let _cache = cache::LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache::prune(&root, None);
    }
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
    #[cfg(target_os = "macos")]
    let quarantine = read_quarantine(&source)?;
    let mut copy = copy_at(dest)?;
    io::copy(&mut source, &mut copy)?;
    #[cfg(target_os = "macos")]
    if let Some(value) = quarantine {
        set_quarantine(copy.as_file(), &value)?;
    }
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
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let value = format!("0081;{seconds:x};Kuvo;");
    set_quarantine(file, value.as_bytes())
}

#[cfg(target_os = "macos")]
fn read_quarantine(file: &File) -> io::Result<Option<Vec<u8>>> {
    use std::os::fd::AsRawFd;

    let get = |buffer: *mut libc::c_void, size| unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            c"com.apple.quarantine".as_ptr(),
            buffer,
            size,
            0,
            0,
        )
    };
    let size = get(std::ptr::null_mut(), 0);
    if size < 0 {
        let error = io::Error::last_os_error();
        return if matches!(
            error.raw_os_error(),
            Some(libc::ENOATTR | libc::ENOTSUP | libc::EOPNOTSUPP)
        ) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let mut value = vec![0; size as usize];
    let read = get(value.as_mut_ptr().cast(), value.len());
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    if read != size {
        return Err(io::Error::other("Quarantine changed during copy"));
    }
    Ok(Some(value))
}

#[cfg(target_os = "macos")]
fn set_quarantine(file: &File, value: &[u8]) -> io::Result<()> {
    use std::os::fd::AsRawFd;

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

fn open_path(path: &Path, source_requires_reveal: bool) -> Result<(), OpenShownError> {
    let resolved = path.canonicalize().map_err(source_error)?;
    let metadata = resolved.metadata().map_err(source_error)?;
    let reveal =
        should_reveal(path, &resolved, &metadata, source_requires_reveal).map_err(source_error)?;
    if !reveal && !metadata.is_file() {
        return Err(Error::NotFound.into());
    }
    open_or_reveal(&resolved, reveal, &metadata)
}

fn should_reveal(
    path: &Path,
    resolved: &Path,
    metadata: &fs::Metadata,
    source_requires_reveal: bool,
) -> io::Result<bool> {
    if source_requires_reveal || runnable_extension(path) || runnable_extension(resolved) {
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
        should_reveal(path, &resolved, &resolved.metadata().unwrap(), false).unwrap()
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
    fn macos_launchable_data_and_xml_documents_are_revealed() {
        let folder = tempfile::tempdir().unwrap();
        for extension in [
            "term",
            "terminal",
            "command",
            "tool",
            "scpt",
            "scptd",
            "workflow",
            "wflow",
            "shortcut",
            "webloc",
            "inetloc",
            "fileloc",
            "url",
            "jar",
            "action",
            "caction",
            "definition",
            "jnlp",
            "configprofile",
            "mobileconfig",
            "dist",
            "distz",
            "vncloc",
            "mailloc",
            "afploc",
            "ftploc",
            "newsloc",
        ] {
            let path = folder.path().join(format!("file.{extension}"));
            fs::write(&path, "<plist/>").unwrap();
            assert!(reveal(&path), "{extension}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_remote_extensionless_script_uses_the_source_mode() {
        use std::os::unix::fs::PermissionsExt;

        let folder = tempfile::tempdir().unwrap();
        let shown = ShownFile {
            id: "file".into(),
            uri: "file:///launch".into(),
            path: "/launch".into(),
            name: "launch".into(),
            caption: None,
            kind: FileKind::Other,
            computer: Some("remote".into()),
            session: Some("ses_1".into()),
        };
        for (mode, expected) in [(Some(false), false), (Some(true), true), (None, true)] {
            let (path, source_requires_reveal) =
                materialize_download(folder.path(), &shown, b"echo launch\n", mode).unwrap();
            let metadata = path.metadata().unwrap();
            assert_eq!(metadata.permissions().mode() & 0o111, 0);
            assert_eq!(
                should_reveal(&path, &path, &metadata, source_requires_reveal).unwrap(),
                expected
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn saving_a_local_copy_preserves_its_quarantine_attribute() {
        use std::os::fd::AsRawFd;

        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("source.pdf");
        let dest = folder.path().join("copy.pdf");
        fs::write(&source, b"%PDF").unwrap();
        let source_file = File::open(&source).unwrap();
        set_quarantine(&source_file, b"0083;65abc123;Safari;").unwrap();
        let attribute = |file: &File| {
            let mut value = [0u8; 256];
            let read = unsafe {
                libc::fgetxattr(
                    file.as_raw_fd(),
                    c"com.apple.quarantine".as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    0,
                    0,
                )
            };
            assert!(read > 0, "{}", io::Error::last_os_error());
            value[..read as usize].to_vec()
        };
        let shown = ShownFile {
            id: "file".into(),
            uri: kuvo_box::pictures::file_uri(source.to_str().unwrap()),
            path: source.to_str().unwrap().into(),
            name: "source.pdf".into(),
            caption: None,
            kind: FileKind::Pdf,
            computer: None,
            session: None,
        };
        let client = BoxClient::new(StandIn::new(Replies::Never));
        save_copy(&client, &shown, &dest).unwrap();
        assert_eq!(
            attribute(&File::open(dest).unwrap()),
            attribute(&source_file)
        );
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
                | "scptd"
                | "applescript"
                | "bat"
                | "cmd"
                | "ps1"
                | "vbs"
                | "vbe"
                | "wsf"
                | "jar"
                | "jnlp"
                // Terminal, Automator, Shortcuts and CoreTypes declarations include runnable
                // types that don't inherit from UTTypeExecutable or UTTypeScript.
                | "term"
                | "terminal"
                | "mobileconfig"
                | "configprofile"
                | "workflow"
                | "wflow"
                | "shortcut"
                | "action"
                | "caction"
                | "definition"
                | "dist"
                | "distz"
                | "webloc"
                | "inetloc"
                | "fileloc"
                | "vncloc"
                | "mailloc"
                | "afploc"
                | "ftploc"
                | "newsloc"
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
    // A few launchable types are declared as data, property lists, or plug-ins. Finder aliases
    // and other script or app types can be recognized by their conformance.
    if matches!(
        kind.identifier().to_string().as_str(),
        "com.apple.terminal.session"
            | "com.apple.terminal.settings"
            | "com.apple.automator-workflow"
            | "com.apple.automator-action"
            | "com.apple.automator-conversion-action"
            | "com.apple.automator-type-definition"
            | "com.apple.shortcut"
            | "com.apple.shortcuts.workflow-file"
            | "com.apple.configprofile"
            | "com.apple.mobileconfig"
            | "com.apple.installer-distribution-package"
            | "com.apple.installer-package-archive"
            | "com.sun.java-web-start"
    ) {
        return true;
    }
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
