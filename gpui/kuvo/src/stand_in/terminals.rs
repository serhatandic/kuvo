//! The stand-in's terminals: its engine's PTYs as their routes answer (`api/pty`, made, read,
//! resized and removed) and each one's socket (`api/pty/<id>/connect`). A test prints the shell's
//! output into one ([`crate::stand_in::StandIn::pty_print`]), reads what was typed in it, and ends
//! it; nothing runs, and nothing happens by itself.

use std::collections::BTreeMap;

use kuvo_box::transport::SocketMessage;
use serde_json::{Value, json};

/// The stand-in's PTYs.
#[derive(Default)]
pub struct Terminals {
    next: u64,
    ptys: BTreeMap<String, Pty>,
    /// Why a terminal's socket can't be opened here (a computer reached over SSH, a box whose server
    /// doesn't pass one on yet), where a test says so.
    pub not_here: Option<String>,
    /// Its engine can't make one now: why.
    pub refuses: Option<String>,
}

pub(crate) struct Pty {
    pub info: Value,
    /// What its shell printed, as a socket opened on it replays first.
    pub printed: Vec<u8>,
    /// Each size it was told, in order (rows, columns).
    pub sizes: Vec<(u16, u16)>,
    /// What was typed in it, in order.
    pub typed: Vec<String>,
    /// Its open socket's side here.
    pub socket: Option<async_channel::Sender<SocketMessage>>,
    /// Its socket was closed from the app's side.
    pub closed: bool,
}

impl Terminals {
    pub(crate) fn get(&mut self, id: &str) -> Option<&mut Pty> {
        self.ptys.get_mut(id)
    }

    /// Those it has now (not removed), by id.
    pub fn ids(&self) -> Vec<String> {
        self.ptys.keys().cloned().collect()
    }

    /// Its answer to one of the engine's PTY routes, where it is one.
    pub(crate) fn answer(&mut self, method: &str, route: &str, body: Option<&Value>, directory: Option<String>) -> Option<(u16, Value)> {
        let parts: Vec<&str> = route.split('/').collect();
        match (method, parts.as_slice()) {
            ("GET", ["api", "pty"]) => Some((200, json!({ "data": self.ptys.values().map(|pty| pty.info.clone()).collect::<Vec<_>>() }))),
            ("POST", ["api", "pty"]) => {
                if let Some(why) = &self.refuses {
                    return Some((503, json!({ "message": why })));
                }
                self.next += 1;
                let id = format!("pty_{}", self.next);
                let cwd = body.and_then(|body| body.get("cwd")).and_then(Value::as_str).map(str::to_owned).or(directory).unwrap_or_else(|| super::HOME.to_owned());
                let info = json!({ "id": id, "title": format!("Terminal {:04}", self.next), "command": "/bin/zsh", "args": ["-l"], "cwd": cwd, "status": "running", "pid": 4000 + self.next });
                self.ptys.insert(id, Pty { info: info.clone(), printed: Vec::new(), sizes: Vec::new(), typed: Vec::new(), socket: None, closed: false });
                Some((200, json!({ "data": info })))
            }
            (_, ["api", "pty", id]) => {
                let Some(pty) = self.ptys.get_mut(*id) else { return Some((404, json!({ "message": format!("PTY session not found: {id}") }))) };
                match method {
                    "GET" => Some((200, json!({ "data": pty.info }))),
                    "PUT" => {
                        let size = body.and_then(|body| body.get("size"));
                        let read = |name: &str| size.and_then(|size| size.get(name)).and_then(Value::as_u64).map(|value| value as u16);
                        if let (Some(rows), Some(cols)) = (read("rows"), read("cols")) {
                            pty.sizes.push((rows, cols));
                        }
                        Some((200, json!({ "data": pty.info })))
                    }
                    "DELETE" => {
                        if let Some(pty) = self.ptys.remove(*id)
                            && let Some(socket) = pty.socket
                        {
                            let _ = socket.send_blocking(SocketMessage::Ended(None));
                        }
                        Some((204, json!({})))
                    }
                    _ => Some((405, json!({}))),
                }
            }
            _ => None,
        }
    }
}
