//! A terminal on a box: a shell in its engine's PTY, through the engine's own routes as the box
//! passes them on (`api/pty`: made, read, resized and removed), and its output and your keys over a
//! socket (`api/pty/<id>/connect`, PROTOCOL.md "Sockets").
//!
//! The engine runs the shell and keeps what it printed; this only asks for one, carries bytes both
//! ways and says the size it's drawn at. Where the way a box is reached can't carry the socket yet
//! (over SSH; a box whose own server doesn't pass it on), [`PtyError::NotHereYet`] says so in words
//! to show, never a silent failure.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::BoxClient;
use crate::error::Error;
use crate::paths;
use crate::transport::{Request, SocketMessage, SocketOut, is_sockets_not_here};

/// The most of what's typed or pasted sent in one message: well inside a relay message.
const MOST_SENT: usize = 16 * 1024;

/// A PTY as the engine describes it (its `Pty`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyInfo {
    pub id: String,
    /// The engine's title for it (`Terminal 1a2b` unless one was given).
    #[serde(default)]
    pub title: String,
    /// What runs in it: the shell, as a path (`/bin/zsh`).
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    /// `running` or `exited`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
}

impl PtyInfo {
    /// Whether what runs in it is still running.
    pub fn running(&self) -> bool {
        self.status != "exited"
    }

    /// The shell's name, as the engine says what runs in it (`zsh` for `/bin/zsh`); none where it
    /// didn't say.
    pub fn shell(&self) -> Option<String> {
        let name = self.command.rsplit('/').next().unwrap_or("").trim();
        (!name.is_empty()).then(|| name.to_owned())
    }

    /// How it ended, in words: with its exit code where it has one.
    pub fn ended(&self) -> String {
        match self.exit_code {
            Some(0) | None => "The shell exited.".to_owned(),
            Some(code) => format!("The shell exited with code {code}."),
        }
    }
}

/// Why a terminal couldn't be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyError {
    /// The way this box is reached can't carry a terminal yet: why, in words to show.
    NotHereYet(String),
    /// Anything else, in words to show.
    Failed(String),
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PtyError::NotHereYet(why) | PtyError::Failed(why) => f.write_str(why),
        }
    }
}

impl From<Error> for PtyError {
    fn from(error: Error) -> PtyError {
        if is_sockets_not_here(&error) {
            return PtyError::NotHereYet(error.to_string());
        }
        PtyError::Failed(error.to_string())
    }
}

/// What a terminal's socket brings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyOutput {
    /// What the shell printed, as it came: escape sequences and all, for a terminal to read.
    Bytes(Vec<u8>),
    /// The socket ended (the shell exited, or the connection went), and why where it was said.
    Ended(Option<String>),
}

/// A terminal's open socket: what the shell prints, comes on [`PtyConnection::next`]; what you type
/// goes with [`PtyConnection::write`]. Cheap to clone: clones share it.
#[derive(Clone)]
pub struct PtyConnection {
    incoming: async_channel::Receiver<SocketMessage>,
    outgoing: Arc<dyn SocketOut>,
    /// Where the engine's output had got to when it last said (its socket's `{cursor}` note), for a
    /// socket opened again to take up from there.
    cursor: Arc<AtomicU64>,
}

impl PtyConnection {
    /// What the shell printed next, or the socket's end, waiting for it; none once it has ended.
    pub async fn next(&self) -> Option<PtyOutput> {
        loop {
            let message = self.incoming.recv().await.ok()?;
            if let Some(output) = self.read(message) {
                return Some(output);
            }
        }
    }

    /// As [`PtyConnection::next`], blocking the thread it's called on (a client without an
    /// executor).
    pub fn next_blocking(&self) -> Option<PtyOutput> {
        loop {
            let message = self.incoming.recv_blocking().ok()?;
            if let Some(output) = self.read(message) {
                return Some(output);
            }
        }
    }

    /// What's come already, without waiting: none when nothing has.
    pub fn try_next(&self) -> Option<PtyOutput> {
        loop {
            let message = self.incoming.try_recv().ok()?;
            if let Some(output) = self.read(message) {
                return Some(output);
            }
        }
    }

    fn read(&self, message: SocketMessage) -> Option<PtyOutput> {
        match message {
            SocketMessage::Text(bytes) => Some(PtyOutput::Bytes(bytes)),
            SocketMessage::Binary(bytes) => {
                if let Some(cursor) = cursor_note(&bytes) {
                    self.cursor.store(cursor, Ordering::Relaxed);
                }
                None
            }
            SocketMessage::Ended(why) => Some(PtyOutput::Ended(why)),
        }
    }

    /// Sends what's typed or pasted to the shell, in pieces small enough for any link.
    pub fn write(&self, text: &str) -> Result<(), PtyError> {
        for piece in pieces(text, MOST_SENT) {
            self.outgoing.send_text(piece)?;
        }
        Ok(())
    }

    /// Where the engine's output had got to when it last said.
    pub fn cursor(&self) -> u64 {
        self.cursor.load(Ordering::Relaxed)
    }

    /// Closes the socket (the shell runs on: [`BoxClient::remove_pty`] ends it).
    pub fn close(&self) {
        self.outgoing.close();
    }
}

/// The engine's note on its socket: a zero byte, then JSON (`{"cursor": n}`).
fn cursor_note(bytes: &[u8]) -> Option<u64> {
    let (&0, json) = bytes.split_first()? else { return None };
    serde_json::from_slice::<Value>(json).ok()?.get("cursor")?.as_u64()
}

/// `text` in pieces of at most `most` bytes, each ending on a character's boundary.
fn pieces(text: &str, most: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.len() > most {
        let mut at = most;
        while !rest.is_char_boundary(at) {
            at -= 1;
        }
        let (piece, after) = rest.split_at(at);
        pieces.push(piece);
        rest = after;
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

/// A request in `directory` (the folder the engine takes it for).
fn located(request: Request, directory: Option<&str>) -> Request {
    match directory {
        Some(folder) => request.header("x-opencode-directory", paths::query_value(folder)),
        None => request,
    }
}

/// The engine's answer: its `{data}`, or the value itself.
fn read_info(body: &[u8]) -> Result<PtyInfo, PtyError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| PtyError::Failed("The terminal’s details couldn’t be read.".into()))?;
    let info = value.get("data").filter(|data| data.is_object()).cloned().unwrap_or(value);
    serde_json::from_value(info).map_err(|_| PtyError::Failed("The terminal’s details couldn’t be read.".into()))
}

fn route(id: &str) -> String {
    format!("api/pty/{}", paths::segment(id))
}

/// What asks the engine for a new PTY: its shell (the engine's own), in `directory`.
fn create_request(directory: Option<&str>) -> Request {
    let mut body = json!({});
    if let Some(folder) = directory {
        body["cwd"] = folder.into();
    }
    located(Request::post("api/pty", body), directory)
}

/// What says the size a PTY is drawn at.
fn resize_request(id: &str, directory: Option<&str>, rows: u16, cols: u16) -> Request {
    located(Request::put(route(id), json!({ "size": { "rows": rows, "cols": cols } })), directory)
}

impl BoxClient {
    /// A new terminal on the box: the engine's shell, in `directory` (the engine's own folder when
    /// none).
    pub fn create_pty(&self, directory: Option<&str>) -> Result<PtyInfo, PtyError> {
        let response = self.send(&create_request(directory))?;
        response.verified()?;
        read_info(&response.body)
    }

    /// A terminal as the engine has it now: running, or how it ended.
    pub fn pty(&self, id: &str, directory: Option<&str>) -> Result<PtyInfo, PtyError> {
        let response = self.send(&located(Request::get(route(id)), directory))?;
        response.verified()?;
        read_info(&response.body)
    }

    /// Says the size a terminal is drawn at: its shell's programs lay out to it.
    pub fn resize_pty(&self, id: &str, directory: Option<&str>, rows: u16, cols: u16) -> Result<(), PtyError> {
        Ok(self.send(&resize_request(id, directory, rows, cols))?.verified()?)
    }

    /// Ends a terminal: its shell, and what runs in it, are stopped and it's gone.
    pub fn remove_pty(&self, id: &str, directory: Option<&str>) -> Result<(), PtyError> {
        let response = self.send(&located(Request::delete(route(id)), directory))?;
        // (One that's gone already is gone.)
        if response.status == 404 {
            return Ok(());
        }
        Ok(response.verified()?)
    }

    /// Opens a terminal's socket: what it printed so far comes first, then what it prints.
    pub fn connect_pty(&self, id: &str, directory: Option<&str>) -> Result<PtyConnection, PtyError> {
        let socket = self.socket(&located(Request::get(format!("{}/connect", route(id))), directory))?;
        Ok(PtyConnection { incoming: socket.incoming, outgoing: socket.outgoing, cursor: Arc::new(AtomicU64::new(0)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_terminal_is_asked_for_in_its_folder_as_the_engine_takes_it() {
        let asked = create_request(Some("/Users/sam/My Projects/app"));
        assert_eq!((asked.method.as_str(), asked.path.as_str()), ("POST", "api/pty"));
        assert_eq!(asked.body, Some(json!({ "cwd": "/Users/sam/My Projects/app" })));
        assert!(asked.headers.contains(&("x-opencode-directory".into(), "%2FUsers%2Fsam%2FMy%20Projects%2Fapp".into())));
        let anywhere = create_request(None);
        assert_eq!(anywhere.body, Some(json!({})));
        assert!(anywhere.headers.is_empty());
    }

    #[test]
    fn its_size_is_said_in_rows_and_columns() {
        let asked = resize_request("pty_01JX", Some("/srv/app"), 24, 80);
        assert_eq!((asked.method.as_str(), asked.path.as_str()), ("PUT", "api/pty/pty_01JX"));
        assert_eq!(asked.body, Some(json!({ "size": { "rows": 24, "cols": 80 } })));
    }

    #[test]
    fn the_engines_details_read_with_or_without_its_data_envelope() {
        let wrapped = br#"{"data":{"id":"pty_1","title":"Terminal 1a2b","command":"/bin/zsh","args":["-l"],"cwd":"/Users/sam/app","status":"running","pid":4242}}"#;
        let info = read_info(wrapped).unwrap();
        assert_eq!((info.id.as_str(), info.shell().as_deref(), info.running()), ("pty_1", Some("zsh"), true));
        let plain = br#"{"id":"pty_2","title":"T","command":"/usr/bin/fish","args":[],"cwd":"/","status":"exited","pid":1,"exitCode":127}"#;
        let ended = read_info(plain).unwrap();
        assert!(!ended.running());
        assert_eq!(ended.ended(), "The shell exited with code 127.");
        assert!(matches!(read_info(b"<html>"), Err(PtyError::Failed(_))));
    }

    #[test]
    fn the_engines_cursor_note_is_read_and_its_output_passed_on() {
        assert_eq!(cursor_note(b"\0{\"cursor\":4096}"), Some(4096));
        assert_eq!(cursor_note(b"{\"cursor\":1}"), None, "a note starts with a zero byte");
        let (sender, incoming) = async_channel::unbounded();
        struct Nowhere;
        impl SocketOut for Nowhere {
            fn send_text(&self, _text: &str) -> crate::Result<()> { Ok(()) }
            fn close(&self) {}
        }
        let connection = PtyConnection { incoming, outgoing: Arc::new(Nowhere), cursor: Arc::new(AtomicU64::new(0)) };
        sender.send_blocking(SocketMessage::Text(b"$ ".to_vec())).unwrap();
        sender.send_blocking(SocketMessage::Binary(b"\0{\"cursor\":2}".to_vec())).unwrap();
        sender.send_blocking(SocketMessage::Ended(None)).unwrap();
        assert_eq!(connection.try_next(), Some(PtyOutput::Bytes(b"$ ".to_vec())));
        assert_eq!(connection.try_next(), Some(PtyOutput::Ended(None)));
        assert_eq!(connection.cursor(), 2);
    }

    #[test]
    fn a_long_paste_goes_in_pieces_that_never_split_a_character() {
        let text = "é".repeat(10);
        let split = pieces(&text, 5);
        assert!(split.iter().all(|piece| piece.len() <= 5));
        assert_eq!(split.concat(), text);
        assert_eq!(pieces("", 5), Vec::<&str>::new());
    }

    #[test]
    fn a_way_that_cant_carry_a_terminal_says_so_in_words() {
        let error = crate::transport::sockets_not_here("Terminals on devbox aren’t available over SSH yet.");
        assert_eq!(PtyError::from(error), PtyError::NotHereYet("Terminals on devbox aren’t available over SSH yet.".into()));
        assert!(matches!(PtyError::from(Error::Unreachable("It may be asleep or offline.".into())), PtyError::Failed(_)));
    }
}
