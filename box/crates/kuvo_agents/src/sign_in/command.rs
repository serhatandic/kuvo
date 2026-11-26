//! A vendor sign-in command owns its process tree, pipes and browser catcher.

use std::{
    fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, mpsc as sync},
    thread,
    time::{Duration, Instant},
};

use kuvo_os::process::{Spawn, Stdio};
use kuvo_store::files;
use tokio::sync::{mpsc, oneshot, watch};

use crate::runtime::AdapterCall;

use super::{Attempt, Mode, Page, Status};

type Code = (String, oneshot::Sender<io::Result<()>>);

pub struct Options {
    pub spawn: Spawn,
    pub temporary: PathBuf,
    pub printed_page: bool,
    pub code_prompt: Option<&'static str>,
    pub instructions: &'static str,
    pub code_instructions: &'static str,
    /// The code its page asks for on another device, as the command prints it.
    pub user_code: Option<fn(&str) -> Option<String>>,
    pub confirm: Box<dyn FnOnce() -> bool + Send>,
    pub completed: Box<dyn FnOnce() + Send>,
}

pub async fn start(options: Options) -> io::Result<Arc<dyn Attempt>> {
    kuvo_os::process::wait_for_background_memory().await;
    let initial = page(&options, "", options.printed_page, "");
    let (status_tx, mut status_rx) = watch::channel(Status::Pending { page: initial });
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (code_tx, code_rx) = mpsc::channel(1);
    let (ready_tx, ready_rx) = oneshot::channel();
    thread::Builder::new()
        .name("agent-sign-in".into())
        .spawn(move || {
            let failure = run(options, &status_tx, cancel_rx, code_rx, ready_tx);
            if failure.is_err() {
                status_tx.send_replace(failed("The sign-in command stopped. Start it again."));
            }
        })?;
    ready_rx
        .await
        .map_err(|_| io::Error::other("The sign-in program couldn't start. Try again."))??;
    let _ = tokio::time::timeout(
        Duration::from_secs(15),
        status_rx
            .wait_for(|status| !matches!(status, Status::Pending { page } if page.url.is_empty())),
    )
    .await;
    Ok(Arc::new(Command {
        status: status_rx,
        cancel: cancel_tx,
        codes: code_tx,
    }))
}

struct Command {
    status: watch::Receiver<Status>,
    cancel: watch::Sender<bool>,
    codes: mpsc::Sender<Code>,
}

impl Attempt for Command {
    fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    fn submit_code(&self, code: String) -> AdapterCall<'_, ()> {
        Box::pin(async move {
            let code = code.trim();
            if code.is_empty() || code.len() > 1024 || code.contains(['\n', '\r', '\0']) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Paste the code from the sign-in page.",
                ));
            }
            let (reply, answer) = oneshot::channel();
            self.codes
                .try_send((code.to_owned(), reply))
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => {
                        io::Error::other("A code is already being submitted.")
                    }
                    mpsc::error::TrySendError::Closed(_) => over(),
                })?;
            answer.await.map_err(|_| over())?
        })
    }

    fn cancel(&self) {
        self.cancel.send_replace(true);
    }
}

struct Catcher(PathBuf);

impl Catcher {
    fn install(temporary: &Path, spawn: &mut Spawn) -> io::Result<Self> {
        let mut random = [0; 12];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let folder = temporary.join(format!("sign-in-{}", hex::encode(random)));
        files::private_folder(&folder)?;
        let catcher = Self(folder);
        let script = catcher.script()?;
        spawn.env.retain(|key, _| {
            !key.eq_ignore_ascii_case("BROWSER") && !key.eq_ignore_ascii_case("KUVO_URL_FILE")
        });
        spawn.env.insert("BROWSER".into(), script.into());
        spawn
            .env
            .insert("KUVO_URL_FILE".into(), catcher.0.join("url").into());
        Ok(catcher)
    }

    fn url(&self) -> Option<String> {
        let file = fs::File::open(self.0.join("url")).ok()?;
        let mut text = String::new();
        file.take(8192).read_to_string(&mut text).ok()?;
        first_url(&text).map(str::to_owned)
    }

    fn script(&self) -> io::Result<PathBuf> {
        catcher_script(&self.0)
    }
}

/// A `BROWSER` for an agent, in `folder`: the pages it opens are written down in
/// `KUVO_URL_FILE` instead.
#[cfg(unix)]
pub fn catcher_script(folder: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let script = folder.join("browser");
    files::write_new(
        &script,
        b"#!/bin/sh\numask 077\nprintf '%s\\n' \"$1\" >> \"$KUVO_URL_FILE\"\n",
    )?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
    Ok(script)
}

/// A `BROWSER` for an agent, in `folder`: the pages it opens are written down in
/// `KUVO_URL_FILE` instead.
#[cfg(windows)]
pub fn catcher_script(folder: &Path) -> io::Result<PathBuf> {
    let script = folder.join("browser.cmd");
    files::write_new(
        &script,
        b"@echo off\r\nsetlocal DisableDelayedExpansion\r\n>>\"%KUVO_URL_FILE%\" echo(%1\r\n",
    )?;
    Ok(script)
}

impl Drop for Catcher {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(
    mut options: Options,
    status: &watch::Sender<Status>,
    cancel: watch::Receiver<bool>,
    mut codes: mpsc::Receiver<Code>,
    ready: oneshot::Sender<io::Result<()>>,
) -> io::Result<()> {
    let catcher = Catcher::install(&options.temporary, &mut options.spawn)?;
    options.spawn.priority = kuvo_os::process::Priority::Background;
    options.spawn.stdin = Stdio::Pipe;
    options.spawn.stdout = Stdio::Pipe;
    options.spawn.stderr = Stdio::Pipe;
    let mut child = match options.spawn.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    let (tx, rx) = sync::sync_channel(16);
    output(
        child.stdout.take().expect("requested stdout pipe"),
        tx.clone(),
    )?;
    output(child.stderr.take().expect("requested stderr pipe"), tx)?;
    if ready.send(Ok(())).is_err() {
        return Ok(());
    }

    let started = Instant::now();
    let mut transcript = Vec::new();
    let mut printed_at = None;
    let mut pasted = false;
    let mut submitting: Option<(Instant, oneshot::Sender<io::Result<()>>)> = None;
    let mut shown = false;
    loop {
        if cancel.has_changed().is_err()
            || *cancel.borrow()
            || started.elapsed() >= Duration::from_secs(30 * 60)
        {
            child.stop(Duration::from_secs(1), |_| {})?;
            status.send_replace(failed("Cancelled."));
            return Ok(());
        }
        if child.has_exited()? {
            let success = child.wait()?.success();
            let signed_in = success && (options.confirm)();
            if signed_in {
                (options.completed)();
                status.send_replace(Status::Complete);
            } else {
                status.send_replace(failed(if pasted {
                    "That code didn't work. Start again, and paste the code from the page this sign-in opens."
                } else {
                    "The sign-in ended before it finished. Try again."
                }));
            }
            if let Some((_, reply)) = submitting.take() {
                let _ = reply.send(if signed_in { Ok(()) } else { Err(over()) });
            }
            return Ok(());
        }
        for bytes in rx.try_iter().take(16) {
            transcript.extend_from_slice(&bytes);
        }
        if transcript.len() > 65_536 {
            transcript.drain(..transcript.len() - 65_536);
        }
        let text = plain(&transcript);
        if !shown {
            let printed = first_url(&text);
            if printed.is_some() {
                printed_at.get_or_insert_with(Instant::now);
            }
            let auto = (!options.printed_page).then(|| catcher.url()).flatten();
            let manual = printed.filter(|_| {
                options.printed_page
                    || printed_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(3))
            });
            if let Some((url, manual)) = auto
                .as_deref()
                .map(|url| (url, false))
                .or_else(|| manual.map(|url| (url, true)))
            {
                status.send_replace(Status::Pending {
                    page: page(&options, url, manual, &text),
                });
                shown = true;
            }
        }
        if let Some((at, _)) = &submitting {
            let rejected = text.to_ascii_lowercase().contains("invalid code");
            if rejected || at.elapsed() >= Duration::from_millis(1500) {
                let (_, reply) = submitting.take().expect("submitted code");
                pasted = !rejected;
                let _ = reply.send(if rejected {
                    Err(io::Error::new(io::ErrorKind::InvalidInput, "That code couldn't be read. Copy all of it from the page, and paste it again."))
                } else {
                    Ok(())
                });
            }
        }
        if let Ok((code, reply)) = codes.try_recv() {
            if submitting.is_some() {
                let _ = reply.send(Err(io::Error::other("A code is already being submitted.")));
            } else if options
                .code_prompt
                .is_some_and(|prompt| text.contains(prompt))
            {
                let stdin = child.stdin.as_mut().expect("requested stdin pipe");
                stdin.write_all(format!("{code}\n").as_bytes())?;
                transcript.clear();
                pasted = true;
                submitting = Some((Instant::now(), reply));
            } else {
                let _ = reply.send(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "The sign-in command isn't ready for a code yet.",
                )));
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn output(mut reader: impl Read + Send + 'static, tx: sync::SyncSender<Vec<u8>>) -> io::Result<()> {
    thread::Builder::new()
        .name("agent-sign-in-output".into())
        .spawn(move || {
            let mut bytes = [0; 8192];
            while let Ok(size @ 1..) = reader.read(&mut bytes) {
                if tx.send(bytes[..size].to_vec()).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

fn plain(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(c) = characters.next() {
        if c == '\x1b' && characters.next_if_eq(&'[').is_some() {
            for c in characters.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else if !c.is_control() || c.is_whitespace() {
            plain.push(c);
        }
    }
    plain
}

fn first_url(text: &str) -> Option<&str> {
    let start = text.find("https://").or_else(|| text.find("http://"))?;
    let url = text[start..]
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>'))
        .next()?;
    let host = url.split_once("://")?.1.split(['/', '?', '#']).next()?;
    (!host.is_empty() && !host.contains('@')).then_some(url)
}

fn page(options: &Options, url: &str, manual: bool, said: &str) -> Page {
    let code = manual && options.code_prompt.is_some();
    Page {
        url: url.into(),
        instructions: if code {
            options.code_instructions
        } else {
            options.instructions
        }
        .into(),
        mode: if code { Mode::Code } else { Mode::Auto },
        accepts_code: options.code_prompt.is_some(),
        user_code: options.user_code.and_then(|read| read(said)),
    }
}

fn failed(message: &str) -> Status {
    Status::Failed {
        message: message.into(),
    }
}

fn over() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "That sign-in is over. Start it again.",
    )
}
