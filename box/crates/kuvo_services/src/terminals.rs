//! The app's terminals, the box's own: `api/pty` at the paths, answers and socket frames the
//! OpenCode server used for them (PROTOCOL.md "Sockets"), so clients are unchanged. Each is the
//! computer's shell in a pseudo-terminal (`kuvo_os::pty`), with what it printed kept (the last
//! 2 MiB, counted in UTF-16 units as OpenCode counts) for a socket opened again to take up where
//! it left off. Nothing runs for terminals until one is made, and nothing stays once it's removed.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use http::{Method, StatusCode};
use kuvo_os::{
    platform::{Account, Environment, Executable, Platform, which},
    process::{ChildGroup, GroupSignal, Spawn},
    pty::{Input, Output, Size},
};
use kuvo_server::{
    DispatchRequest, Refusal, Registry, Reply,
    socket::{CloseCode, CloseFrame, Message, Socket},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// What's kept of a terminal's output, in UTF-16 units.
const KEPT: u64 = 2 * 1024 * 1024;
/// Kept output is sent again in pieces of at most this many UTF-16 units.
const PIECE: u64 = 64 * 1024;
/// How long what a program printed last has to come through once it exited.
const LAST_OUTPUT: Duration = Duration::from_millis(250);
/// What a socket may fall behind by before it's let go.
const BEHIND: usize = 1024;

/// The computer's terminals.
#[derive(Clone)]
pub struct Terminals(Arc<Inner>);

struct Inner {
    /// Where a terminal opens when the request names no folder.
    workspace: PathBuf,
    /// What a terminal's shell starts with.
    env: Environment,
    list: Mutex<Vec<Arc<Terminal>>>,
}

struct Terminal {
    /// The folder it was made in (OpenCode lists a folder's own).
    directory: String,
    input: Input,
    state: Mutex<State>,
    removed: Mutex<Option<oneshot::Sender<()>>>,
}

struct State {
    info: Info,
    kept: Kept,
    sockets: Vec<mpsc::Sender<Message>>,
}

/// A terminal as clients read it (OpenCode's `Pty`).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    id: String,
    title: String,
    command: String,
    args: Vec<String>,
    cwd: String,
    status: &'static str,
    pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<String>,
    title: Option<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    title: Option<String>,
    size: Option<Resize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resize {
    rows: u16,
    cols: u16,
}

pub fn register(routes: &mut Registry, home: &Path) -> Terminals {
    let terminals = Terminals::new(home.join("workspace"));
    let http = terminals.clone();
    routes.own_api("api/pty", move |request, _| {
        let terminals = http.clone();
        async move { terminals.answer(request) }
    });
    let sockets = terminals.clone();
    routes.sockets(move |request, cancel| {
        let terminals = sockets.clone();
        async move { terminals.connect(&request, cancel) }
    });
    terminals
}

impl Terminals {
    fn new(workspace: PathBuf) -> Self {
        Self(Arc::new(Inner {
            workspace,
            env: shell_environment(std::env::vars_os()),
            list: Mutex::new(Vec::new()),
        }))
    }

    fn list(&self) -> MutexGuard<'_, Vec<Arc<Terminal>>> {
        self.0
            .list
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn find(&self, id: &str) -> Option<Arc<Terminal>> {
        self.list()
            .iter()
            .find(|terminal| terminal.info().id == id)
            .cloned()
    }

    fn answer(&self, request: DispatchRequest) -> Reply {
        let (path, query) = request.path.split_once('?').unwrap_or((&request.path, ""));
        let (named, directory) = self.directory(&request, query);
        let located = |data: Value| {
            Reply::json(
                StatusCode::OK,
                json!({ "location": { "directory": directory }, "data": data }),
            )
        };
        let segments: Vec<&str> = path.split('/').skip(2).collect();
        let answer = match (&request.method, segments.as_slice()) {
            (&Method::GET, []) => Ok(located(json!(
                self.list()
                    .iter()
                    .filter(|terminal| terminal.directory == directory)
                    .map(|terminal| terminal.info())
                    .collect::<Vec<_>>()
            ))),
            (&Method::POST, []) => payload::<Create>(&request.body)
                .and_then(|create| self.create(create, &named, &directory))
                .map(|info| located(json!(info))),
            (method, [id]) => self.one(method, id, &request.body).map(|info| match info {
                Some(info) => located(json!(info)),
                None => Reply::bytes(StatusCode::NO_CONTENT, "application/json", ""),
            }),
            _ => Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))),
        };
        answer.unwrap_or_else(Refusal::reply)
    }

    /// The folder a request is for, as OpenCode takes it (its header, else its `directory`): as
    /// named, and as its real path on Unix (Windows keeps the path given).
    fn directory(&self, request: &DispatchRequest, query: &str) -> (String, String) {
        let named = request
            .headers
            .get("x-opencode-directory")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                percent_encoding::percent_decode_str(value)
                    .decode_utf8_lossy()
                    .into_owned()
            })
            .or_else(|| {
                form_urlencoded::parse(query.as_bytes())
                    .find(|(key, _)| key == "directory")
                    .map(|(_, value)| value.into_owned())
            })
            .filter(|folder| !folder.is_empty());
        let folder = named.unwrap_or_else(|| self.0.workspace.to_string_lossy().into_owned());
        let real = std::fs::canonicalize(&folder)
            .ok()
            .filter(|_| cfg!(unix))
            .map_or_else(
                || folder.clone(),
                |real| real.to_string_lossy().into_owned(),
            );
        (folder, real)
    }

    /// One terminal read, changed or removed; none once removed.
    fn one(&self, method: &Method, id: &str, body: &[u8]) -> Result<Option<Info>, Refusal> {
        if !id.starts_with("pty") {
            return Err(Refusal::json(
                StatusCode::BAD_REQUEST,
                json!({ "_tag": "InvalidRequestError", "message": "Expected a string starting with \"pty\"\n  at [\"ptyID\"]", "kind": "Params" }),
            ));
        }
        let not_found = || {
            Refusal::json(
                StatusCode::NOT_FOUND,
                json!({ "_tag": "PtyNotFoundError", "ptyID": id, "message": format!("PTY session not found: {id}") }),
            )
        };
        let terminal = self.find(id).ok_or_else(not_found)?;
        match *method {
            Method::GET => Ok(Some(terminal.info())),
            Method::PUT => {
                let update = payload::<Update>(body)?;
                if let Some(size) = update.size {
                    if size.rows == 0 || size.cols == 0 {
                        return Err(invalid("Expected a value greater than 0\n  at [\"size\"]"));
                    }
                    // (One that has exited has no size to change.)
                    let _ = terminal.input.resize(Size {
                        rows: size.rows,
                        cols: size.cols,
                    });
                }
                let mut state = terminal.state();
                if let Some(title) = update.title {
                    state.info.title = title;
                }
                Ok(Some(state.info.clone()))
            }
            Method::DELETE => {
                self.list().retain(|kept| !Arc::ptr_eq(kept, &terminal));
                let removed = terminal
                    .removed
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(removed) = removed {
                    let _ = removed.send(());
                }
                Ok(None)
            }
            _ => Err(Refusal::json(StatusCode::NOT_FOUND, json!({}))),
        }
    }

    fn create(&self, create: Create, named: &str, directory: &str) -> Result<Info, Refusal> {
        let failed = |error: std::io::Error| {
            Refusal::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("The terminal couldn’t start: {error}"),
            )
        };
        let mut env = self.0.env.clone();
        env.extend(
            create
                .env
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        let executable = match &create.command {
            Some(command) => which(command.as_ref(), &env, &[]),
            None => login_shell(&env),
        }
        .ok_or_else(|| failed(std::io::ErrorKind::NotFound.into()))?;
        let command = executable.path().to_string_lossy().into_owned();
        let mut args = create.args;
        if is_posix_shell(executable.path()) {
            args.push("-l".into());
        }
        let cwd = create.cwd.unwrap_or_else(|| named.to_owned());
        let id = new_id();
        let mut spawn = Spawn::new(PathBuf::new(), env);
        spawn.executable = executable;
        spawn.args = args.iter().map(OsString::from).collect();
        spawn.cwd = Some(start_folder(&cwd));
        let terminal = spawn
            .spawn_terminal(Size { rows: 24, cols: 80 })
            .map_err(failed)?;
        let info = Info {
            title: create
                .title
                .unwrap_or_else(|| format!("Terminal {}", &id[id.len() - 4..])),
            pid: terminal.child.identity().pid,
            id,
            command,
            args,
            cwd,
            status: "running",
            exit_code: None,
        };
        let (removed, on_removed) = oneshot::channel();
        let entry = Arc::new(Terminal {
            directory: directory.to_owned(),
            input: terminal.input,
            state: Mutex::new(State {
                info: info.clone(),
                kept: Kept::default(),
                sockets: Vec::new(),
            }),
            removed: Mutex::new(Some(removed)),
        });
        self.list().push(entry.clone());
        tokio::spawn(entry.run(terminal.output, terminal.child, on_removed));
        Ok(info)
    }

    /// A terminal's socket (`api/pty/:id/connect`): what it printed since `cursor` first, then its
    /// note of where its output had got to, then what it prints; what's typed goes to it.
    fn connect(
        &self,
        request: &DispatchRequest,
        cancel: CancellationToken,
    ) -> Result<Socket, Refusal> {
        let (path, query) = request.path.split_once('?').unwrap_or((&request.path, ""));
        let id = match path.split('/').collect::<Vec<_>>().as_slice() {
            ["api", "pty", id, "connect"] if !id.is_empty() => (*id).to_owned(),
            _ => {
                return Err(Refusal::new(
                    StatusCode::FORBIDDEN,
                    "That isn't available over the link.",
                ));
            }
        };
        let terminal = self.find(&id).ok_or_else(|| {
            Refusal::new(
                StatusCode::BAD_GATEWAY,
                "That terminal isn’t on this computer.",
            )
        })?;
        let cursor = form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "cursor")
            .and_then(|(_, value)| value.parse::<i64>().ok())
            .unwrap_or(0);
        let (client, server) = Socket::pair();
        tokio::spawn(terminal.serve(server, cursor, cancel));
        Ok(client)
    }
}

impl Terminal {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn info(&self) -> Info {
        self.state().info.clone()
    }

    /// The terminal's life: what it prints kept and passed on, until its program exits or it's
    /// removed.
    async fn run(
        self: Arc<Self>,
        mut output: Output,
        mut child: ChildGroup,
        mut removed: oneshot::Receiver<()>,
    ) {
        let mut decoder = Decoder::default();
        let mut printing = true;
        let gone = {
            let watch = child.identity().watch_exit();
            let exited = async {
                match watch {
                    Ok(exited) => drop(exited.await),
                    // (Without a watch, it's looked at now and then.)
                    Err(_) => {
                        while !child.has_exited().unwrap_or(true) {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            };
            tokio::pin!(exited);
            loop {
                tokio::select! {
                    piece = output.read(), if printing => match piece {
                        Ok(Some(bytes)) => self.printed(&decoder.decode(&bytes)),
                        _ => printing = false,
                    },
                    () = &mut exited => break false,
                    _ = &mut removed => break true,
                }
            }
        };
        if gone {
            // As a terminal closed: its programs are hung up on, then stopped.
            let _ = child.signal(GroupSignal::Hangup);
            tokio::task::spawn_blocking(move || child.stop(Duration::from_secs(1), |_| {}));
            self.ended(None);
            return;
        }
        self.input.close();
        let _ = tokio::time::timeout(LAST_OUTPUT, async {
            while let Ok(Some(bytes)) = output.read().await {
                self.printed(&decoder.decode(&bytes));
            }
        })
        .await;
        let code = tokio::task::spawn_blocking(move || child.wait().map(exit_code))
            .await
            .ok()
            .and_then(Result::ok);
        self.ended(Some(code.unwrap_or(-1)));
    }

    fn printed(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut state = self.state();
        state.kept.push(text);
        let message = Message::Text(text.to_owned().into());
        state
            .sockets
            .retain(|socket| socket.try_send(message.clone()).is_ok());
    }

    /// Exited with `code`, or removed: its sockets close as OpenCode closes them.
    fn ended(&self, code: Option<i32>) {
        let mut state = self.state();
        if let Some(code) = code {
            state.info.status = "exited";
            state.info.exit_code = Some(code);
        }
        for socket in state.sockets.drain(..) {
            let _ = socket.try_send(close(CloseCode::Normal, ""));
        }
    }

    async fn serve(self: Arc<Self>, mut socket: Socket, cursor: i64, cancel: CancellationToken) {
        let (send, mut printed) = mpsc::channel(BEHIND);
        {
            let mut state = self.state();
            if state.info.status == "exited" {
                let _ = send.try_send(close(CloseCode::Library(4404), "session exited"));
            } else {
                let end = state.kept.end();
                let from = u64::try_from(cursor).unwrap_or(end);
                for piece in state.kept.since(from) {
                    let _ = send.try_send(Message::Text(piece.to_owned().into()));
                }
                let note = [&[0][..], format!("{{\"cursor\":{end}}}").as_bytes()].concat();
                let _ = send.try_send(Message::Binary(note.into()));
                state.sockets.push(send);
            }
        }
        loop {
            tokio::select! {
                message = printed.recv() => {
                    let message = message.unwrap_or_else(|| close(CloseCode::Normal, ""));
                    let closing = message.is_close();
                    if socket.send.send(message).await.is_err() || closing {
                        break;
                    }
                }
                message = socket.receive.recv() => match message {
                    Some(Message::Text(text)) => drop(self.input.write(text.as_str().as_bytes()).await),
                    Some(Message::Binary(bytes)) => drop(self.input.write(&bytes).await),
                    // The client's close, answered with its own code.
                    Some(Message::Close(frame)) => {
                        let _ = socket.send.send(Message::Close(frame)).await;
                        break;
                    }
                    Some(_) => {}
                    None => break,
                },
                () = cancel.cancelled() => break,
            }
        }
    }
}

fn close(code: CloseCode, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.to_owned().into(),
    }))
}

fn payload<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, Refusal> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))
}

fn invalid(message: &str) -> Refusal {
    Refusal::json(
        StatusCode::BAD_REQUEST,
        json!({ "_tag": "InvalidRequestError", "message": message, "kind": "Payload" }),
    )
}

/// The box's own environment for a shell: without Kuvo's and OpenCode's own settings and
/// secrets, with a terminal's `TERM`.
fn shell_environment(vars: impl Iterator<Item = (OsString, OsString)>) -> Environment {
    let mut env: Environment = vars
        .filter(|(name, _)| {
            let name = name.to_string_lossy().to_ascii_uppercase();
            !["KUVO_", "OPENCODE_", "BUN_"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
                && !name.starts_with('=')
                && name != "TERM"
        })
        .collect();
    env.insert("TERM".into(), "xterm-256color".into());
    env
}

/// The computer's shell: the account's login shell, else (on Windows) its command prompt.
fn login_shell(env: &Environment) -> Option<Executable> {
    let platform = Platform::current();
    Account::current()
        .ok()
        .and_then(|account| account.login_shell(platform, env))
        .map(Executable::Native)
        .or_else(|| which("cmd.exe".as_ref(), env, &[]))
}

/// A shell that reads its login profile with `-l`, as OpenCode starts one.
fn is_posix_shell(program: &Path) -> bool {
    let name = program
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish")
}

/// Where its shell starts: the folder asked for, as a path here, else the home folder.
fn start_folder(cwd: &str) -> PathBuf {
    let native = if cfg!(windows) {
        kuvo_os::platform::windows_native_path(cwd)
            .map_or_else(|| PathBuf::from(cwd), PathBuf::from)
    } else {
        PathBuf::from(cwd)
    };
    if native.is_absolute() && native.is_dir() {
        return native;
    }
    let platform = Platform::current();
    let env: Environment = std::env::vars_os().collect();
    Account::current()
        .and_then(|account| account.home_folder(platform, &env))
        .unwrap_or_else(|_| std::env::temp_dir())
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or(status.signal().map(|signal| 128 + signal))
        .unwrap_or(-1)
}

#[cfg(windows)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// An ID in OpenCode's ascending shape (`pty_` + time + random).
fn new_id() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = (jiff::Timestamp::now().as_millisecond().max(0) as u64) << 12;
    let tick = CLOCK
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |earlier| Some(now.max(earlier + 1)),
        )
        .map_or(now, |earlier| now.max(earlier + 1));
    let mut random = [0u8; 14];
    let _ = getrandom::fill(&mut random);
    let random: String = random
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect();
    format!("pty_{:012x}{random}", tick & 0xffff_ffff_ffff)
}

/// What's kept of a terminal's output: its last [`KEPT`] UTF-16 units.
#[derive(Default)]
struct Kept {
    text: String,
    /// Units no longer kept.
    dropped: u64,
    /// Units in `text`.
    units: u64,
}

fn units(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

impl Kept {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        self.units += units(text);
        if self.units > KEPT {
            let (at, cut) = self.offset(self.units - KEPT);
            self.text.drain(..at);
            self.dropped += cut;
            self.units -= cut;
        }
    }

    fn end(&self) -> u64 {
        self.dropped + self.units
    }

    /// The byte offset in `text` after at least `skip` units, and the units before it.
    fn offset(&self, skip: u64) -> (usize, u64) {
        let mut counted = 0;
        for (at, character) in self.text.char_indices() {
            if counted >= skip {
                return (at, counted);
            }
            counted += character.len_utf16() as u64;
        }
        (self.text.len(), counted)
    }

    /// What it printed from `cursor` on, in pieces of at most [`PIECE`] units.
    fn since(&self, cursor: u64) -> Vec<&str> {
        let (mut at, _) = self.offset(cursor.saturating_sub(self.dropped));
        let mut pieces = Vec::new();
        while at < self.text.len() {
            let rest = &self.text[at..];
            let mut counted = 0;
            let end = rest
                .char_indices()
                .find(|(_, character)| {
                    counted += character.len_utf16() as u64;
                    counted > PIECE
                })
                .map_or(rest.len(), |(end, _)| end);
            pieces.push(&rest[..end]);
            at += end;
        }
        pieces
    }
}

/// Output as text: a character split across reads waits for its end, and bytes that aren't text
/// become U+FFFD.
#[derive(Default)]
struct Decoder(Vec<u8>);

impl Decoder {
    fn decode(&mut self, bytes: &[u8]) -> String {
        self.0.extend_from_slice(bytes);
        let whole = self.0.len() - incomplete_tail(&self.0);
        let text = String::from_utf8_lossy(&self.0[..whole]).into_owned();
        self.0.drain(..whole);
        text
    }
}

/// How many bytes at the end begin a character that hasn't all come yet.
fn incomplete_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let byte = bytes[bytes.len() - back];
        if byte & 0xC0 == 0x80 {
            continue;
        }
        let needs = match byte {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        };
        return if needs > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
#[path = "terminals_tests.rs"]
mod tests;
