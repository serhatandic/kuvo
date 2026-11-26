use super::Size;
use std::{
    io::{self, PipeReader, PipeWriter, Read, Write},
    os::windows::io::AsRawHandle,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};

/// The pseudoconsole while its program starts, with the pipe ends it was made on (the console
/// host keeps its own copies).
pub(crate) struct Console {
    handle: HPCON,
    _ends: (PipeReader, PipeWriter),
}

impl Console {
    pub(crate) fn handle(&self) -> HPCON {
        self.handle
    }
}

/// Anonymous pipes can't be waited on by the runtime: a thread reads, until the console host
/// closes its end.
pub(super) struct Output(mpsc::Receiver<io::Result<Vec<u8>>>);

#[derive(Clone)]
pub(super) struct Input(Arc<Shared>);

struct Shared {
    writer: Mutex<PipeWriter>,
    /// None once closed.
    console: Mutex<Option<Pseudoconsole>>,
}

struct Pseudoconsole(HPCON);

// SAFETY: a pseudoconsole handle may be used and closed from any thread.
unsafe impl Send for Pseudoconsole {}

impl Drop for Pseudoconsole {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreatePseudoConsole and is closed once, here. Output is
        // still being drained by its reader, which closing waits for on older Windows.
        unsafe { ClosePseudoConsole(self.0) };
    }
}

fn coord(size: Size) -> COORD {
    COORD {
        X: size.cols.min(i16::MAX as u16) as i16,
        Y: size.rows.min(i16::MAX as u16) as i16,
    }
}

fn hresult(result: i32) -> io::Result<()> {
    if result < 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    Ok(())
}

pub(super) fn open(size: Size) -> io::Result<(Console, Output, Input)> {
    let (console_input, typed) = io::pipe()?;
    let (mut printed, console_output) = io::pipe()?;
    let mut handle: HPCON = 0;
    // SAFETY: both pipe handles are live; the console host duplicates them.
    hresult(unsafe {
        CreatePseudoConsole(
            coord(size),
            console_input.as_raw_handle(),
            console_output.as_raw_handle(),
            0,
            &mut handle,
        )
    })?;
    let console = Pseudoconsole(handle);
    let (send, receive) = mpsc::channel(8);
    std::thread::Builder::new()
        .name("terminal-output".into())
        .spawn(move || {
            let mut open = true;
            loop {
                let mut buffer = vec![0; 64 * 1024];
                let piece = match printed.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        buffer.truncate(read);
                        buffer
                    }
                };
                // (Read on to the end even once nobody listens: closing waits for that.)
                open = open && send.blocking_send(Ok(piece)).is_ok();
            }
        })?;
    Ok((
        Console {
            handle,
            _ends: (console_input, console_output),
        },
        Output(receive),
        Input(Arc::new(Shared {
            writer: Mutex::new(typed),
            console: Mutex::new(Some(console)),
        })),
    ))
}

impl Output {
    pub(super) async fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.0.recv().await.transpose()
    }
}

impl Input {
    pub(super) async fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let shared = self.0.clone();
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut writer = shared
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            writer.write_all(&bytes)?;
            writer.flush()
        })
        .await
        .map_err(io::Error::other)?
    }

    pub(super) fn resize(&self, size: Size) -> io::Result<()> {
        let console = self
            .0
            .console
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(console) = console.as_ref() else {
            return Ok(());
        };
        // SAFETY: the pseudoconsole is live while it's held here.
        hresult(unsafe { ResizePseudoConsole(console.0, coord(size)) })
    }

    pub(super) fn close(&self) {
        let console = self
            .0
            .console
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        // (Closing can wait for the reader: never on the runtime's own threads.)
        if let Some(console) = console {
            let _ = std::thread::Builder::new()
                .name("terminal-close".into())
                .spawn(move || drop(console));
        }
    }
}
