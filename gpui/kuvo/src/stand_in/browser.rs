//! The stand-in's browser, as a machine's box shows it live (`kuvo/browser/view`): its socket says
//! hello, then its pages and their sizes, as the keeper does; a test changes its pages, sends a
//! picture, reads what the app sent on it (each command, as JSON), and ends it. Each command sent
//! with an id is answered at once (the pages, for `Kuvo.pages`). Nothing happens by itself.

use kuvo_box::transport::SocketMessage;
use serde_json::{Value, json};

/// Its words, as a Mac's box says why it has no live view.
pub const NO_VIEW_ON_MAC: &str = "Kuvo on a Mac has no browser of its own yet: its agents browse in Kuvo Code's panel on that Mac.";

pub struct Browser {
    /// Its keeper runs: the socket opens.
    pub running: bool,
    /// `Kuvo.pages`'s params: `{active, pages}`.
    pub pages: Value,
    /// Each page's size, as `Kuvo.viewport` says it.
    pub viewports: Vec<Value>,
    /// Its open socket's side here.
    pub socket: Option<async_channel::Sender<SocketMessage>>,
    /// What the app sent on it, a command each, in order.
    pub sent: Vec<Value>,
    /// How many times it was opened.
    pub opened: usize,
    /// Its socket was closed from the app's side.
    pub closed: bool,
}

impl Default for Browser {
    fn default() -> Browser {
        Browser { running: true, pages: json!({ "active": null, "pages": [] }), viewports: Vec::new(), socket: None, sent: Vec::new(), opened: 0, closed: false }
    }
}

/// A fact, as a text message.
pub(crate) fn fact(method: &str, params: &Value) -> SocketMessage {
    SocketMessage::Text(format!("{}\n", json!({ "method": method, "params": params })).into_bytes())
}

/// A picture, as a binary message: its header's length, its header, its JPEG.
pub(crate) fn picture(params: &Value, jpeg: &[u8]) -> SocketMessage {
    let header = json!({ "method": "Kuvo.frame", "params": params }).to_string().into_bytes();
    let mut bytes = (header.len() as u32).to_be_bytes().to_vec();
    bytes.extend(header);
    bytes.extend_from_slice(jpeg);
    SocketMessage::Binary(bytes)
}

impl Browser {
    /// Opens its socket: hello, its pages, their sizes, as the keeper greets a viewer.
    pub(crate) fn open(&mut self) -> async_channel::Receiver<SocketMessage> {
        let (sender, incoming) = async_channel::unbounded();
        let active = self.pages.get("active").cloned().unwrap_or(Value::Null);
        let _ = sender.send_blocking(fact("Kuvo.hello", &json!({ "targetId": active, "keeper": 60, "idleStop": true })));
        let _ = sender.send_blocking(fact("Kuvo.pages", &self.pages));
        for viewport in &self.viewports {
            let _ = sender.send_blocking(fact("Kuvo.viewport", viewport));
        }
        self.socket = Some(sender);
        self.opened += 1;
        self.closed = false;
        incoming
    }

    /// What the app sent, a line a command: kept, and those with an id answered.
    pub(crate) fn heard(&mut self, text: &str) {
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(command) = serde_json::from_str::<Value>(line) else { continue };
            if let (Some(id), Some(socket)) = (command.get("id").cloned(), &self.socket) {
                let result = if command["method"] == "Kuvo.pages" { self.pages.clone() } else { json!({}) };
                let _ = socket.send_blocking(SocketMessage::Text(format!("{}\n", json!({ "id": id, "result": result })).into_bytes()));
            }
            self.sent.push(command);
        }
    }

    /// Sends `message` on its socket, where it's open.
    pub(crate) fn say(&self, message: SocketMessage) {
        if let Some(socket) = &self.socket {
            let _ = socket.send_blocking(message);
        }
    }
}
