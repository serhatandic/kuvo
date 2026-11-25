//! Supplement folder discovery on Darwin: FSEvents may defer SQLite writes
//! until a writer closes. Vnode events observe writes to open maker files.
use std::{
    io,
    path::{Path, PathBuf},
};

#[cfg(target_os = "macos")]
mod darwin {
    use super::*;
    use std::{
        collections::BTreeMap,
        fs::File,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        os::unix::fs::OpenOptionsExt,
    };
    use tokio::io::unix::AsyncFd;
    const MAX_HANDLES: usize = 128;

    pub struct LiveFiles {
        queue: AsyncFd<OwnedFd>,
        files: BTreeMap<PathBuf, (File, u64)>,
        serial: u64,
    }
    impl LiveFiles {
        pub fn new() -> io::Result<Option<Self>> {
            // SAFETY: kqueue returns a new owned descriptor or -1.
            let fd = unsafe { libc::kqueue() };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fd was created above and has no other owner.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            // SAFETY: fcntl sets the descriptor's close-on-exec flag.
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the OwnedFd remains open and uniquely owned until AsyncFd deregisters it.
            let queue =
                unsafe { AsyncFd::register_with_interest(fd, tokio::io::Interest::READABLE) }
                    .map_err(|error| error.into_parts().1)?;
            Ok(Some(Self {
                queue,
                files: BTreeMap::new(),
                serial: 0,
            }))
        }
        pub fn track(&mut self, path: &Path) -> io::Result<()> {
            self.serial = self.serial.wrapping_add(1);
            if let Some((_, sequence)) = self.files.get_mut(path) {
                *sequence = self.serial;
                return Ok(());
            }
            let file = match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_EVTONLY)
                .open(path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            let event = libc::kevent {
                ident: file.as_raw_fd() as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE
                    | libc::NOTE_EXTEND
                    | libc::NOTE_DELETE
                    | libc::NOTE_RENAME
                    | libc::NOTE_REVOKE,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: event is a valid one-element change list, with no result buffer.
            if unsafe {
                libc::kevent(
                    self.queue.get_ref().as_raw_fd(),
                    &event,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
            if self.files.len() >= MAX_HANDLES {
                let oldest = self
                    .files
                    .iter()
                    .min_by_key(|(_, (_, seq))| *seq)
                    .map(|(p, _)| p.clone());
                if let Some(oldest) = oldest {
                    self.files.remove(&oldest);
                }
            }
            self.files.insert(path.to_owned(), (file, self.serial));
            Ok(())
        }
        pub async fn recv(&mut self) -> io::Result<Vec<PathBuf>> {
            loop {
                let mut ready = self.queue.readable().await?;
                // SAFETY: kevent can initialize this event buffer with its results.
                let mut events: [libc::kevent; 32] = unsafe { std::mem::zeroed() };
                let timeout = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                // SAFETY: events is an initialized result buffer with room for 32 events.
                let count = unsafe {
                    libc::kevent(
                        self.queue.get_ref().as_raw_fd(),
                        std::ptr::null(),
                        0,
                        events.as_mut_ptr(),
                        events.len() as i32,
                        &timeout,
                    )
                };
                if count < 0 {
                    return Err(io::Error::last_os_error());
                }
                if count < events.len() as i32 {
                    ready.clear_ready();
                }
                drop(ready);
                if count == 0 {
                    continue;
                }
                let mut paths = Vec::new();
                for event in &events[..count as usize] {
                    let id = event.ident;
                    let found = self
                        .files
                        .iter()
                        .find(|(_, (file, _))| file.as_raw_fd() as libc::uintptr_t == id)
                        .map(|(path, _)| path.clone());
                    let Some(path) = found else {
                        continue;
                    };
                    if event.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE)
                        != 0
                    {
                        self.files.remove(&path);
                    }
                    paths.push(path);
                }
                if !paths.is_empty() {
                    return Ok(paths);
                }
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub use darwin::LiveFiles;
#[cfg(not(target_os = "macos"))]
pub struct LiveFiles;
#[cfg(not(target_os = "macos"))]
impl LiveFiles {
    pub fn new() -> io::Result<Option<Self>> {
        Ok(None)
    }
    pub fn track(&mut self, _: &Path) -> io::Result<()> {
        Ok(())
    }
    pub async fn recv(&mut self) -> io::Result<Vec<PathBuf>> {
        std::future::pending().await
    }
}
