use super::*;
use rustix::fs::{FlockOperation, flock};
use std::{
    fs::{self, DirBuilder, File, Metadata, Permissions},
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
};
use tokio::net::{UnixListener, UnixStream};

pub(super) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    inode: (u64, u64),
}

fn private(metadata: &Metadata) -> bool {
    metadata.uid() == rustix::process::getuid().as_raw() && metadata.mode() & 0o077 == 0
}

fn permission_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Kuvo local IPC requires a private socket and folder",
    )
}

fn check_socket(endpoint: &Endpoint) -> io::Result<Metadata> {
    let info = fs::symlink_metadata(&endpoint.path)?;
    let permitted = match endpoint.channel {
        Channel::Ssh | Channel::Keeper => private(&info),
        // Legacy holders had the default umask, inside a mode-0700 directory.
        Channel::Holder(_) => info.uid() == rustix::process::getuid().as_raw(),
    };
    if !info.file_type().is_socket() || !permitted {
        return Err(permission_error());
    }
    Ok(info)
}

fn check_folder(path: &Path) -> io::Result<File> {
    let folder = File::open(path)?;
    let info = folder.metadata()?;
    if !info.is_dir() || !private(&info) {
        return Err(permission_error());
    }
    Ok(folder)
}

pub(super) fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
    let path = &endpoint.path;
    let parent = path.parent().expect("validated socket filename");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let folder = File::open(parent)?;
    let info = folder.metadata()?;
    if !info.is_dir() || info.uid() != rustix::process::getuid().as_raw() {
        return Err(permission_error());
    }
    if info.mode() & 0o077 != 0 {
        folder.set_permissions(Permissions::from_mode(0o700))?;
    }
    // Serialize stale-socket recovery and unlinking among Rust listeners without a lock file.
    flock(&folder, FlockOperation::LockExclusive)?;
    match check_socket(endpoint) {
        Ok(_) => {
            let probe = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
            probe.set_nonblocking(true)?;
            match probe.connect(&socket2::SockAddr::unix(path)?) {
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    fs::remove_file(path)?
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "local socket already has an owner",
                    ));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    // The folder already prevents other users from reaching the socket. No process-wide umask.
    let listener = UnixListener::bind(path)?;
    let info = fs::set_permissions(path, Permissions::from_mode(0o600))
        .and_then(|()| fs::symlink_metadata(path));
    match info {
        Ok(info) => Ok(Listener {
            listener,
            path: path.clone(),
            inode: (info.dev(), info.ino()),
        }),
        Err(error) => {
            let _ = fs::remove_file(path);
            Err(error)
        }
    }
}

pub(super) async fn connect(endpoint: &Endpoint) -> io::Result<LocalStream> {
    check_folder(endpoint.path.parent().expect("validated socket filename"))?;
    check_socket(endpoint)?;
    Ok(Box::new(UnixStream::connect(&endpoint.path).await?))
}

impl Listener {
    pub(super) async fn accept(&mut self) -> io::Result<LocalStream> {
        Ok(Box::new(self.listener.accept().await?.0))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let Ok(folder) = check_folder(self.path.parent().expect("validated socket filename"))
        else {
            return;
        };
        if flock(&folder, FlockOperation::LockExclusive).is_ok()
            && fs::symlink_metadata(&self.path)
                .is_ok_and(|info| (info.dev(), info.ino()) == self.inode)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
