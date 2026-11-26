use super::Size;
use rustix::{
    fs::{Mode, OFlags},
    io::{FdFlags, fcntl_setfd},
    pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt},
    termios::{Winsize, tcsetwinsize},
};
use std::{fs::File, io, os::fd::OwnedFd, sync::Arc};
use tokio::io::{Interest, unix::AsyncFd};

/// The program's side of the terminal, while it starts.
pub(crate) struct Console(File);

impl Console {
    pub(crate) fn terminal(&self) -> &File {
        &self.0
    }
}

/// The box's side (the controller), read and written without blocking.
type Controller = Arc<AsyncFd<OwnedFd>>;

pub(super) struct Output(Controller);

#[derive(Clone)]
pub(super) struct Input(Controller);

pub(super) fn open(size: Size) -> io::Result<(Console, Output, Input)> {
    let controller = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    fcntl_setfd(&controller, FdFlags::CLOEXEC)?;
    grantpt(&controller)?;
    unlockpt(&controller)?;
    let name = ptsname(&controller, Vec::new())?;
    let terminal = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    tcsetwinsize(&controller, winsize(size))?;
    rustix::io::ioctl_fionbio(&controller, true)?;
    // SAFETY: the controller is owned here and only ever read and written through this AsyncFd.
    let controller = unsafe {
        AsyncFd::register_with_interest(controller, Interest::READABLE | Interest::WRITABLE)
    }
    .map_err(io::Error::from)?;
    let controller = Arc::new(controller);
    Ok((
        Console(File::from(terminal)),
        Output(controller.clone()),
        Input(controller),
    ))
}

fn winsize(size: Size) -> Winsize {
    Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

impl Output {
    pub(super) async fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let mut ready = self.0.readable().await?;
            match ready.try_io(|fd| Ok(rustix::io::read(fd.get_ref(), &mut buffer[..])?)) {
                Ok(Ok(0)) => return Ok(None),
                Ok(Ok(read)) => {
                    buffer.truncate(read);
                    return Ok(Some(buffer));
                }
                // Linux says EIO once nothing has the program's side open any more.
                Ok(Err(error)) if error.raw_os_error() == Some(libc::EIO) => return Ok(None),
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => {}
            }
        }
    }
}

impl Input {
    pub(super) async fn write(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut ready = self.0.writable().await?;
            match ready.try_io(|fd| Ok(rustix::io::write(fd.get_ref(), bytes)?)) {
                Ok(Ok(written)) => bytes = &bytes[written..],
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => {}
            }
        }
        Ok(())
    }

    pub(super) fn resize(&self, size: Size) -> io::Result<()> {
        Ok(tcsetwinsize(self.0.get_ref(), winsize(size))?)
    }

    pub(super) fn close(&self) {}
}
