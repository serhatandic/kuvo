//! A computer's browser, live (PROTOCOL.md, "The browser's live view"): its pages' facts and
//! pictures over the box's socket (`kuvo/browser/view`), and what's done in a page sent back.
//!
//! The box's keeper owns the browser, its pages and their sizes; this reads what it says, carries
//! its pictures (a binary message each: a four-byte big-endian header length, the JSON header, the
//! JPEG) and sends the view's commands. Where its box has no live view (a Mac's), its browser isn't
//! running, or the way the box is reached can't carry the socket, [`BrowserViewError`] says which.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::BoxClient;
use crate::error::Error;
use crate::transport::{Request, SocketMessage, SocketOut, is_sockets_not_here};

/// What a box lists where it shows its browser live (a machine's box; a Mac's doesn't).
pub const FEATURE: &str = "browser.view";
const ROUTE: &str = "kuvo/browser/view";

/// Why a computer's browser couldn't be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserViewError {
    /// Its box doesn't show a browser of its own (a Mac's box, or one from before the live view):
    /// what was said.
    NoBrowser(String),
    /// The way this client reaches the box can't carry the socket yet (over SSH): its words.
    NotThisWay(String),
    /// Its browser isn't running: the socket couldn't reach it. What was said.
    NotRunning(String),
    /// Anything else, in words to show.
    Failed(String),
}

impl std::fmt::Display for BrowserViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserViewError::NoBrowser(why) | BrowserViewError::NotThisWay(why) | BrowserViewError::NotRunning(why) | BrowserViewError::Failed(why) => f.write_str(why),
        }
    }
}

impl From<Error> for BrowserViewError {
    fn from(error: Error) -> BrowserViewError {
        if is_sockets_not_here(&error) {
            return BrowserViewError::NotThisWay(error.to_string());
        }
        match error.status() {
            // A Mac's box answers 404 with why; the keeper not answering ends the socket before it opens.
            Some(404) => BrowserViewError::NoBrowser(error.to_string()),
            Some(502) => BrowserViewError::NotRunning(error.to_string()),
            _ => BrowserViewError::Failed(error.to_string()),
        }
    }
}

// MARK: What the box says

/// One of the browser's pages, as its keeper says it (`Kuvo.pages`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Page {
    pub target_id: String,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// An agent is at work in it: a viewer can't change its size meanwhile.
    pub working: bool,
    pub held: bool,
    /// The conversation it's claimed for.
    pub session: Option<String>,
    pub opener: Option<String>,
    /// The dialog it shows (Chrome's facts), where it shows one.
    pub dialog: Option<Value>,
}

impl Page {
    /// Its tab's name, as the protocol says one is named.
    pub fn name(&self) -> String {
        page_name(&self.title, &self.url)
    }
}

/// A tab's name: its nonempty title, else its address's host without `www.`, else "New Tab".
pub fn page_name(title: &str, url: &str) -> String {
    let title = title.trim();
    if !title.is_empty() {
        return title.to_owned();
    }
    host(url).map(|host| host.strip_prefix("www.").unwrap_or(host)).filter(|host| !host.is_empty()).map_or_else(|| "New Tab".to_owned(), str::to_owned)
}

/// An address's host (with its port where it names one), where it has one.
fn host(url: &str) -> Option<&str> {
    let (_, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    Some(authority.rsplit_once('@').map_or(authority, |(_, host)| host))
}

/// The browser's pages, and the one in use.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Pages {
    pub active: Option<String>,
    pub pages: Vec<Page>,
}

impl Pages {
    pub fn page(&self, target_id: &str) -> Option<&Page> {
        self.pages.iter().find(|page| page.target_id == target_id)
    }
}

/// A page's size, as its keeper set it (`Kuvo.viewport`): its CSS viewport, which its pictures
/// show whole.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub target_id: String,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub scale: Option<f64>,
    /// Who set it: `agent` or `viewer`.
    #[serde(default)]
    pub by: Option<String>,
}

/// One picture of a page: its header's facts and its JPEG. `width` and `height` are the picture's
/// own pixels; it shows the page's whole viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub target_id: String,
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub jpeg: Vec<u8>,
}

/// What came on the socket.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewMessage {
    Pages(Pages),
    Viewport(Viewport),
    Frame(Frame),
    /// Its answer to a command sent with an id: its words where it refused.
    Answer { id: u64, refused: Option<String> },
    /// A typed address names another app's scheme (`mailto:`): a client asks before opening it.
    External { target_id: String, url: String },
    /// Anything else it says (its hello, agents' status, a dialog, the pointer), as it said it.
    Other { method: String, params: Value },
    /// The socket ended, and why where that was said.
    Ended(Option<String>),
}

/// A binary message's picture: four bytes of big-endian header length, that many bytes of
/// `{method: "Kuvo.frame", params}`, then the JPEG. None for anything else.
pub fn read_frame(bytes: &[u8]) -> Option<Frame> {
    let length = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    if length == 0 || length >= bytes.len() - 4 {
        return None;
    }
    let header: Value = serde_json::from_slice(&bytes[4..4 + length]).ok()?;
    if header.get("method")?.as_str()? != "Kuvo.frame" {
        return None;
    }
    let params = header.get("params")?;
    let number = |name: &str| params.get(name).and_then(Value::as_f64);
    Some(Frame {
        target_id: params.get("targetId")?.as_str()?.to_owned(),
        frame_id: params.get("frameId")?.as_u64()?,
        width: number("width")?.max(0.).round() as u32,
        height: number("height")?.max(0.).round() as u32,
        scroll_x: number("scrollX").unwrap_or(0.),
        scroll_y: number("scrollY").unwrap_or(0.),
        jpeg: bytes[4 + length..].to_vec(),
    })
}

/// A text message's facts, a line each (a line that can't be read is passed over).
pub fn read_text(bytes: &[u8]) -> Vec<ViewMessage> {
    let mut read = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(message) = serde_json::from_slice::<Value>(line) else { continue };
        read.extend(read_message(message));
    }
    read
}

fn read_message(message: Value) -> Vec<ViewMessage> {
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let refused = message.get("error").map(|error| error.get("message").and_then(Value::as_str).unwrap_or("The browser refused that.").to_owned());
        let mut read = vec![ViewMessage::Answer { id, refused }];
        // (The answer to `Kuvo.pages` is the pages.)
        if let Some(pages) = message.get("result").filter(|result| result.get("pages").is_some_and(Value::is_array)).and_then(|result| serde_json::from_value(result.clone()).ok()) {
            read.push(ViewMessage::Pages(pages));
        }
        return read;
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else { return Vec::new() };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let read = match method {
        "Kuvo.pages" => serde_json::from_value(params.clone()).ok().map(ViewMessage::Pages),
        "Kuvo.viewport" => serde_json::from_value(params.clone()).ok().map(ViewMessage::Viewport),
        "Kuvo.external" => match (params.get("targetId").and_then(Value::as_str), params.get("url").and_then(Value::as_str)) {
            (Some(target_id), Some(url)) => Some(ViewMessage::External { target_id: target_id.to_owned(), url: url.to_owned() }),
            _ => None,
        },
        _ => None,
    };
    vec![read.unwrap_or_else(|| ViewMessage::Other { method: method.to_owned(), params })]
}

// MARK: What's sent

/// What a viewer sends (PROTOCOL.md's table): each to the page it names.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The pages' facts, again.
    Pages,
    /// Pictures of a page, for a view `width` by `height` CSS pixels at `scale` (the display's).
    View { target_id: String, width: u32, height: u32, scale: f32 },
    /// No more pictures.
    StopView,
    /// A picture was drawn.
    FrameAck(u64),
    /// A picture was never drawn.
    FrameDropped { frame_id: u64, reason: Dropped },
    Go { target_id: String, go: Go },
    Input { target_id: String, input: Input },
}

/// Why a picture wasn't drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// A newer one came before it could be.
    Backpressure,
    /// It was never shown (the view was hidden, or showed another page).
    Unsent,
}

/// Where a page goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Go {
    /// What was typed in the address field: an address, a path on that computer, or words to search.
    Address(String),
    Back,
    Forward,
    Reload,
    Stop,
}

/// What's done in a page, in its CSS pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Mouse(Mouse),
    Wheel(Wheel),
    Key(Key),
    /// Text put in where the page is typed into (a paste).
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mouse {
    pub action: MouseAction,
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
    pub click_count: u32,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Pressed,
    Released,
    Moved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    None,
    Left,
    Middle,
    Right,
}

impl MouseButton {
    fn name(self) -> &'static str {
        match self {
            MouseButton::None => "none",
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
        }
    }

    /// The button down, as CDP's `buttons` mask says it.
    fn mask(self) -> u32 {
        match self {
            MouseButton::None => 0,
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    pub x: f64,
    pub y: f64,
    /// Down and right are positive, as a page scrolls.
    pub delta_x: f64,
    pub delta_y: f64,
    pub modifiers: Modifiers,
}

/// A key, as CDP's `Input.dispatchKeyEvent` takes it: its `key` and `code`, the text it types, and
/// the key code Chrome edits by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub action: KeyAction,
    pub key: String,
    pub code: String,
    pub text: Option<String>,
    pub key_code: Option<u32>,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Down, typing its text.
    Down,
    /// Down, typing nothing (Return's typed as its text; arrows, Backspace and the like aren't).
    RawDown,
    Up,
}

/// The modifiers held, sent as the protocol's names. Control stands for ⌘ (the keeper makes it ⌘
/// where Chrome runs on a Mac).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub alt: bool,
    pub control: bool,
    pub meta: bool,
    pub shift: bool,
}

impl Modifiers {
    fn names(self) -> Vec<&'static str> {
        [(self.alt, "alt"), (self.control, "control"), (self.meta, "meta"), (self.shift, "shift")].into_iter().filter(|(held, _)| *held).map(|(_, name)| name).collect()
    }

    fn write_into(self, params: &mut Map<String, Value>) {
        let names = self.names();
        if !names.is_empty() {
            params.insert("modifiers".into(), json!(names));
        }
    }
}

impl Command {
    /// It as sent: `{id?, method, params}`.
    pub fn message(&self, id: Option<u64>) -> Value {
        let (method, params) = match self {
            Command::Pages => ("Kuvo.pages", json!({})),
            Command::View { target_id, width, height, scale } => ("Kuvo.view", json!({ "targetId": target_id, "width": width, "height": height, "scale": scale })),
            Command::StopView => ("Kuvo.view", json!({ "targetId": null })),
            Command::FrameAck(frame_id) => ("Kuvo.frameAck", json!({ "frameId": frame_id })),
            Command::FrameDropped { frame_id, reason } => ("Kuvo.frameDropped", json!({ "frameId": frame_id, "reason": match reason { Dropped::Backpressure => "backpressure", Dropped::Unsent => "unsent" } })),
            Command::Go { target_id, go } => {
                let mut params = json!({ "targetId": target_id });
                let (name, value) = match go {
                    Go::Address(input) => ("input", json!(input)),
                    Go::Back => ("back", json!(true)),
                    Go::Forward => ("forward", json!(true)),
                    Go::Reload => ("reload", json!(true)),
                    Go::Stop => ("stop", json!(true)),
                };
                params[name] = value;
                ("Kuvo.go", params)
            }
            Command::Input { target_id, input } => ("Kuvo.input", input_params(target_id, input)),
        };
        let mut message = json!({ "method": method, "params": params });
        if let Some(id) = id {
            message["id"] = json!(id);
        }
        message
    }
}

fn input_params(target_id: &str, input: &Input) -> Value {
    let mut params = Map::new();
    params.insert("targetId".into(), json!(target_id));
    match input {
        Input::Mouse(mouse) => {
            let action = match mouse.action {
                MouseAction::Pressed => "mousePressed",
                MouseAction::Released => "mouseReleased",
                MouseAction::Moved => "mouseMoved",
            };
            let buttons = if mouse.action == MouseAction::Released { 0 } else { mouse.button.mask() };
            params.extend([
                ("kind".into(), json!("mouse")),
                ("type".into(), json!(action)),
                ("x".into(), json!(mouse.x)),
                ("y".into(), json!(mouse.y)),
                ("button".into(), json!(mouse.button.name())),
                ("buttons".into(), json!(buttons)),
                ("clickCount".into(), json!(mouse.click_count)),
            ]);
            mouse.modifiers.write_into(&mut params);
        }
        Input::Wheel(wheel) => {
            params.extend([("kind".into(), json!("wheel")), ("x".into(), json!(wheel.x)), ("y".into(), json!(wheel.y)), ("deltaX".into(), json!(wheel.delta_x)), ("deltaY".into(), json!(wheel.delta_y))]);
            wheel.modifiers.write_into(&mut params);
        }
        Input::Key(key) => {
            let action = match key.action {
                KeyAction::Down => "keyDown",
                KeyAction::RawDown => "rawKeyDown",
                KeyAction::Up => "keyUp",
            };
            params.extend([("kind".into(), json!("key")), ("type".into(), json!(action)), ("key".into(), json!(key.key)), ("code".into(), json!(key.code))]);
            if let Some(text) = &key.text {
                params.insert("text".into(), json!(text));
            }
            if let Some(code) = key.key_code {
                params.insert("windowsVirtualKeyCode".into(), json!(code));
            }
            key.modifiers.write_into(&mut params);
        }
        Input::Text(text) => {
            params.extend([("kind".into(), json!("text")), ("text".into(), json!(text))]);
        }
    }
    Value::Object(params)
}

// MARK: The socket

/// A computer's browser's open socket: what it says comes on [`BrowserViewConnection::next`]; what
/// the view does goes with [`BrowserViewConnection::send`] (or [`BrowserViewConnection::ask`], to
/// hear when it's done). Cheap to clone: clones share it.
#[derive(Clone)]
pub struct BrowserViewConnection {
    incoming: async_channel::Receiver<SocketMessage>,
    outgoing: Arc<dyn SocketOut>,
    next_id: Arc<AtomicU64>,
}

impl BrowserViewConnection {
    /// What the next message on the socket brought (its end last), waiting for it; none once it
    /// has ended.
    pub async fn next(&self) -> Option<Vec<ViewMessage>> {
        loop {
            let read = read_socket(self.incoming.recv().await.ok()?);
            if !read.is_empty() {
                return Some(read);
            }
        }
    }

    /// What's come already, without waiting: none when nothing has.
    pub fn try_next(&self) -> Option<Vec<ViewMessage>> {
        loop {
            let read = read_socket(self.incoming.try_recv().ok()?);
            if !read.is_empty() {
                return Some(read);
            }
        }
    }

    /// Sends `command`, with no answer.
    pub fn send(&self, command: &Command) -> Result<(), BrowserViewError> {
        self.write(&command.message(None))
    }

    /// Sends `command` with an id of its own, which its answer ([`ViewMessage::Answer`]) carries.
    pub fn ask(&self, command: &Command) -> Result<u64, BrowserViewError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.write(&command.message(Some(id)))?;
        Ok(id)
    }

    fn write(&self, message: &Value) -> Result<(), BrowserViewError> {
        Ok(self.outgoing.send_text(&format!("{message}\n"))?)
    }

    /// Closes the socket (the browser runs on).
    pub fn close(&self) {
        self.outgoing.close();
    }
}

fn read_socket(message: SocketMessage) -> Vec<ViewMessage> {
    match message {
        SocketMessage::Text(bytes) => read_text(&bytes),
        SocketMessage::Binary(bytes) => read_frame(&bytes).map(ViewMessage::Frame).into_iter().collect(),
        SocketMessage::Ended(why) => vec![ViewMessage::Ended(why)],
    }
}

impl BoxClient {
    /// Opens the box's browser's live view: its pages' facts first, then pictures of the page it's
    /// asked to show ([`Command::View`]). A box that doesn't list it isn't asked.
    pub fn open_browser_view(&self) -> Result<BrowserViewConnection, BrowserViewError> {
        if !self.has_feature(FEATURE)? {
            return Err(BrowserViewError::NoBrowser("Its box doesn’t show a browser of its own.".into()));
        }
        let socket = self.socket(&Request::get(ROUTE))?;
        Ok(BrowserViewConnection { incoming: socket.incoming, outgoing: socket.outgoing, next_id: Arc::new(AtomicU64::new(1)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../protocol/vectors/browser-view.json")).unwrap()
    }

    fn framed(header: &Value, jpeg: &[u8]) -> Vec<u8> {
        let header = header.to_string().into_bytes();
        let mut bytes = (header.len() as u32).to_be_bytes().to_vec();
        bytes.extend(header);
        bytes.extend(jpeg);
        bytes
    }

    #[test]
    fn a_picture_is_read_from_its_binary_header_and_jpeg() {
        let header = vectors()["binaryHeader"].clone();
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3, 0xff, 0xd9];
        let frame = read_frame(&framed(&header, &jpeg)).unwrap();
        assert_eq!(frame, Frame { target_id: "P".into(), frame_id: 12, width: 393, height: 246, scroll_x: 0., scroll_y: 0., jpeg: jpeg.to_vec() });

        // The box's own header carries its format's version: read as any other field is.
        let mut versioned = header.clone();
        versioned["formatVersion"] = json!(1);
        assert_eq!(read_frame(&framed(&versioned, &jpeg)).unwrap().frame_id, 12);
    }

    #[test]
    fn what_isnt_a_whole_picture_is_passed_over() {
        let header = vectors()["binaryHeader"].clone();
        assert_eq!(read_frame(&framed(&header, &[])), None, "no JPEG after its header");
        assert_eq!(read_frame(&[0, 0, 0]), None);
        assert_eq!(read_frame(&[0, 0, 0, 0, 1]), None, "an empty header");
        assert_eq!(read_frame(&framed(&json!({ "method": "Kuvo.pages", "params": {} }), &[1])), None);
        let mut long = framed(&header, &[1]);
        long[3] = 0xff;
        assert_eq!(read_frame(&long), None, "a header longer than the message");
    }

    #[test]
    fn its_facts_are_read_a_line_each() {
        let facts = vectors()["facts"].as_array().unwrap().iter().map(|fact| format!("{fact}\n")).collect::<String>();
        let read = read_text(facts.as_bytes());
        let ViewMessage::Pages(pages) = &read[0] else { panic!("{read:?}") };
        assert_eq!(pages.active.as_deref(), Some("P"));
        let page = pages.page("P").unwrap();
        assert_eq!((page.url.as_str(), page.title.as_str(), page.working, page.can_go_back), ("https://example.com/agent", "Example", true, false));
        assert_eq!(page.session.as_deref(), Some("ses_agent"));
        assert_eq!(read[1], ViewMessage::Viewport(Viewport { target_id: "P".into(), width: 1280., height: 800., scale: Some(1.), by: Some("agent".into()) }));
        assert_eq!(read[2], ViewMessage::External { target_id: "P".into(), url: "mailto:hello@example.com".into() });

        let said = read_text(b"{\"method\":\"Kuvo.hello\",\"params\":{\"targetId\":\"P\"}}\nnot json\n{\"id\":4,\"error\":{\"message\":\"That page is closed.\"}}\n{\"id\":5,\"result\":{}}");
        assert_eq!(said, [
            ViewMessage::Other { method: "Kuvo.hello".into(), params: json!({ "targetId": "P" }) },
            ViewMessage::Answer { id: 4, refused: Some("That page is closed.".into()) },
            ViewMessage::Answer { id: 5, refused: None },
        ]);
        // Asked for, the pages come as the answer.
        let answer = read_text(br#"{"id":6,"result":{"active":null,"pages":[]}}"#);
        assert_eq!(answer, [ViewMessage::Answer { id: 6, refused: None }, ViewMessage::Pages(Pages::default())]);
    }

    #[test]
    fn commands_are_sent_as_the_protocols_examples_have_them() {
        let examples = vectors()["commands"].as_array().unwrap().clone();
        let go = Command::Go { target_id: "P".into(), go: Go::Address("https://example.com/agent".into()) };
        assert_eq!(go.message(Some(2)), examples[1]);
        assert_eq!(Command::FrameAck(12).message(None), examples[2]);
        assert_eq!(Command::Input { target_id: "P".into(), input: Input::Text("hello".into()) }.message(Some(3)), examples[3]);
        assert_eq!(Command::StopView.message(None), examples[5]);
        let view = Command::View { target_id: "P".into(), width: 393, height: 660, scale: 3. }.message(Some(1));
        assert_eq!((view["method"].clone(), view["params"]["width"].clone(), view["params"]["scale"].clone()), (json!("Kuvo.view"), json!(393), json!(3.0)));

        let click = Command::Input { target_id: "P".into(), input: Input::Mouse(Mouse { action: MouseAction::Pressed, x: 640., y: 400., button: MouseButton::Left, click_count: 2, modifiers: Modifiers { shift: true, ..Modifiers::default() } }) };
        assert_eq!(click.message(None)["params"], json!({ "targetId": "P", "kind": "mouse", "type": "mousePressed", "x": 640.0, "y": 400.0, "button": "left", "buttons": 1, "clickCount": 2, "modifiers": ["shift"] }));
        let key = Command::Input { target_id: "P".into(), input: Input::Key(Key { action: KeyAction::RawDown, key: "Enter".into(), code: "Enter".into(), text: Some("\r".into()), key_code: Some(13), modifiers: Modifiers::default() }) };
        assert_eq!(key.message(None)["params"], json!({ "targetId": "P", "kind": "key", "type": "rawKeyDown", "key": "Enter", "code": "Enter", "text": "\r", "windowsVirtualKeyCode": 13 }));
        let wheel = Command::Input { target_id: "P".into(), input: Input::Wheel(Wheel { x: 1., y: 2., delta_x: 0., delta_y: 120., modifiers: Modifiers::default() }) };
        assert_eq!(wheel.message(None)["params"], json!({ "targetId": "P", "kind": "wheel", "x": 1.0, "y": 2.0, "deltaX": 0.0, "deltaY": 120.0 }));
        assert_eq!(Command::Go { target_id: "P".into(), go: Go::Back }.message(None)["params"], json!({ "targetId": "P", "back": true }));
    }

    #[test]
    fn a_tab_is_named_as_the_protocol_says() {
        assert_eq!(page_name("Example Domain", "https://example.com/"), "Example Domain");
        assert_eq!(page_name("  ", "https://www.example.com/a?b#c"), "example.com");
        assert_eq!(page_name("", "http://localhost:3000/signup"), "localhost:3000");
        assert_eq!(page_name("", "https://sam@docs.example.org"), "docs.example.org");
        assert_eq!(page_name("", "about:blank"), "New Tab");
        assert_eq!(page_name("", ""), "New Tab");
    }

    #[test]
    fn why_it_couldnt_be_had_is_told_apart() {
        let over_ssh = crate::transport::sockets_not_here("Terminals on devbox aren’t available over SSH yet.");
        assert!(matches!(BrowserViewError::from(over_ssh), BrowserViewError::NotThisWay(_)));
        let mac = Error::Refused(crate::error::Refusal::from_answer(404, br#"{"message":"Kuvo on a Mac has no browser of its own yet."}"#));
        assert_eq!(BrowserViewError::from(mac), BrowserViewError::NoBrowser("Kuvo on a Mac has no browser of its own yet.".into()));
        let stopped = Error::Refused(crate::error::Refusal::from_answer(502, br#"{"message":"connect ENOENT"}"#));
        assert!(matches!(BrowserViewError::from(stopped), BrowserViewError::NotRunning(_)));
        assert!(matches!(BrowserViewError::from(Error::Unreachable("It may be asleep.".into())), BrowserViewError::Failed(_)));
    }
}
