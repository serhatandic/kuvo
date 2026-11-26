//! A terminal for a program: a pseudo-terminal (a `posix_openpt` pair on Unix, a ConPTY on
//! Windows) that the program has as its console, while the box reads what it prints and writes
//! what's typed. The program is owned as any other child (`process`): its group or job ends with it.

use std::io;

use crate::process::{ChildGroup, Spawn};

#[cfg(unix)]
#[path = "pty/unix.rs"]
mod sys;
#[cfg(windows)]
#[path = "pty/windows.rs"]
mod sys;

pub(crate) use sys::Console;

/// A terminal's size in character cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

/// A program running in a terminal.
pub struct Terminal {
    pub child: ChildGroup,
    /// What it prints.
    pub output: Output,
    /// What's typed to it, and its size.
    pub input: Input,
}

/// What a terminal's program prints, as it comes.
pub struct Output(sys::Output);

impl Output {
    /// The next piece printed; none once the terminal's output has ended.
    pub async fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.0.read().await
    }
}

/// Where what's typed to a terminal goes. Cheap to clone: clones share the terminal.
#[derive(Clone)]
pub struct Input(sys::Input);

impl Input {
    /// Writes `bytes` to the program, waiting while it's slow to read them.
    pub async fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.0.write(bytes).await
    }

    /// Says the size the terminal is drawn at: the program lays out to it.
    pub fn resize(&self, size: Size) -> io::Result<()> {
        self.0.resize(size)
    }

    /// Ends the terminal itself, once its program has exited: what it printed last comes through
    /// on [`Output`], then its end. (On Unix the end comes by itself once nothing holds it.)
    pub fn close(&self) {
        self.0.close();
    }
}

impl Spawn {
    /// Starts this program in a new terminal of `size`, as its stdin, stdout and stderr (and its
    /// controlling terminal on Unix). The terminal's environment is the spawn's.
    pub fn spawn_terminal(&self, size: Size) -> io::Result<Terminal> {
        let (console, output, input) = sys::open(size)?;
        let child = self.launch(None, false, Some(&console))?;
        // (Our copy of the program's side goes, so its end reads as the end.)
        drop(console);
        Ok(Terminal {
            child,
            output: Output(output),
            input: Input(input),
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::platform::Environment;

    fn shell(script: &str) -> Spawn {
        let mut env = Environment::new();
        env.insert("PATH".into(), "/usr/bin:/bin".into());
        env.insert("TERM".into(), "xterm-256color".into());
        let mut spawn = Spawn::new("/bin/sh", env);
        spawn.args = vec!["-c".into(), script.into()];
        spawn
    }

    async fn everything(output: &mut Output) -> String {
        let mut all = Vec::new();
        while let Some(piece) = output.read().await.unwrap() {
            all.extend(piece);
        }
        String::from_utf8_lossy(&all).into_owned()
    }

    #[tokio::test]
    async fn a_program_has_the_terminal_as_its_console_at_its_size() {
        let mut terminal = shell("stty size; test -t 0 && test -t 1 && echo both-a-terminal")
            .spawn_terminal(Size {
                rows: 30,
                cols: 100,
            })
            .unwrap();
        let printed = everything(&mut terminal.output).await;
        assert!(printed.contains("30 100"), "{printed:?}");
        assert!(printed.contains("both-a-terminal"), "{printed:?}");
        assert!(terminal.child.wait().unwrap().success());
    }

    #[tokio::test]
    async fn what_is_typed_reaches_it_and_a_new_size_is_seen() {
        let mut terminal = shell("read line; echo \"got:$line\"; read again; stty size")
            .spawn_terminal(Size { rows: 24, cols: 80 })
            .unwrap();
        terminal.input.write(b"hello\r").await.unwrap();
        terminal
            .input
            .resize(Size {
                rows: 40,
                cols: 120,
            })
            .unwrap();
        terminal.input.write(b"\r").await.unwrap();
        let printed = everything(&mut terminal.output).await;
        assert!(printed.contains("got:hello"), "{printed:?}");
        assert!(printed.contains("40 120"), "{printed:?}");
        let _ = terminal.child.wait();
    }

    #[tokio::test]
    async fn its_exit_code_is_kept() {
        let mut terminal = shell("exit 3")
            .spawn_terminal(Size { rows: 24, cols: 80 })
            .unwrap();
        everything(&mut terminal.output).await;
        assert_eq!(terminal.child.wait().unwrap().code(), Some(3));
    }
}
