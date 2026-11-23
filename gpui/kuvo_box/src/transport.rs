//! Where a box's routes are reached: its local server, its CLI over SSH, or the relay. Clients
//! send the same requests through each transport.

use std::io::BufRead;
use std::sync::Arc;

use crate::error::{Error, Result};

pub(crate) use kuvo_protocol::version::ROUTE_VERSION_HEADER;
use kuvo_protocol::version::ROUTE_VERSION;

pub(crate) fn check_route_version(given: Option<&str>) -> Result<()> {
    match given {
        None | Some(ROUTE_VERSION) => Ok(()),
        Some(_) => Err(Error::BadAnswer("The box's route format version is unsupported (this client reads version 1).".into())),
    }
}

/// A request for one of a box's routes (PROTOCOL.md, "Messages in a session"): its method, its
/// path without a leading slash (`kuvo/info`, `api/session/ses_1/message?limit=50`), the headers
/// a box reads (`x-opencode-directory`, `idempotency-key`) and a JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<serde_json::Value>,
}

impl Request {
    pub fn get(path: impl Into<String>) -> Request {
        Request { method: "GET".into(), path: path.into(), headers: Vec::new(), body: None }
    }

    pub fn post(path: impl Into<String>, body: serde_json::Value) -> Request {
        Request { method: "POST".into(), path: path.into(), headers: Vec::new(), body: Some(body) }
    }

    pub fn delete(path: impl Into<String>) -> Request {
        Request { method: "DELETE".into(), path: path.into(), headers: Vec::new(), body: None }
    }

    pub fn put(path: impl Into<String>, body: serde_json::Value) -> Request {
        Request { method: "PUT".into(), path: path.into(), headers: Vec::new(), body: Some(body) }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Request {
        self.headers.push((name.to_ascii_lowercase(), value.into()));
        self
    }

    pub(crate) fn wire_headers(&self) -> impl Iterator<Item = (&str, &str)> {
        let json = (self.body.is_some() && !self.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")))
            .then_some(("content-type", "application/json"));
        self.headers.iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case(ROUTE_VERSION_HEADER))
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .chain(json)
            .chain(std::iter::once((ROUTE_VERSION_HEADER, ROUTE_VERSION)))
    }

    /// The route it asks, without its query.
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }
}

/// A box's whole answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub(crate) fn verified(&self) -> Result<()> {
        if self.is_success() { Ok(()) } else { Err(Error::Refused(crate::error::Refusal::from_answer(self.status, &self.body))) }
    }
}

/// An answer read as it comes: a followed route (`…/events`, ndjson) or the engine's event stream
/// (`api/event`, server-sent events). Closing it (or dropping its [`Closer`]) ends it from any
/// thread, which a reader blocked on it sees as the end.
pub struct Stream {
    pub status: u16,
    pub content_type: Option<String>,
    pub reader: Box<dyn BufRead + Send>,
    pub closer: Closer,
}

/// Ends a [`Stream`] from another thread.
#[derive(Clone)]
pub struct Closer(Arc<dyn Fn() + Send + Sync>);

impl Closer {
    pub fn new(close: impl Fn() + Send + Sync + 'static) -> Closer {
        Closer(Arc::new(close))
    }

    /// One that has nothing to close (an answer already read whole).
    pub fn nothing() -> Closer {
        Closer::new(|| {})
    }

    pub fn close(&self) {
        (self.0)()
    }
}

/// A WebSocket a box opened on its engine for this client (PROTOCOL.md, "Sockets"): today only a
/// terminal's (`api/pty/<id>/connect`). What comes on it arrives on `incoming` a message at a
/// time, its last [`SocketMessage::Ended`], and the channel closes after it; `outgoing` sends on it
/// and closes it.
pub struct Socket {
    pub incoming: async_channel::Receiver<SocketMessage>,
    pub outgoing: Arc<dyn SocketOut>,
}

/// One message from the other side of a [`Socket`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketMessage {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    /// It ended, and why where that was said (the close's reason, the connection dropping).
    Ended(Option<String>),
}

/// What sends on a [`Socket`] and closes it, from any thread.
pub trait SocketOut: Send + Sync {
    /// Sends `text` as one text message; fails once the socket has ended.
    fn send_text(&self, text: &str) -> Result<()>;
    /// Closes it: its end comes on `incoming`, and nothing after.
    fn close(&self);
}

/// A refusal's `reason` when this client can't open a socket on a box the way it reaches it (over
/// SSH, or through a server that doesn't pass one on yet): said plainly where it's shown, never a
/// silent failure.
pub const SOCKETS_NOT_HERE: &str = "socketsNotHere";

/// This client can't open a socket on that box the way it reaches it: `message` says so.
pub fn sockets_not_here(message: impl Into<String>) -> Error {
    Error::Refused(crate::error::Refusal { status: 501, message: message.into(), title: None, reason: Some(SOCKETS_NOT_HERE.into()), older_box: false, action: None })
}

/// Whether `error` is [`sockets_not_here`].
pub fn is_sockets_not_here(error: &Error) -> bool {
    error.refusal().is_some_and(|refusal| refusal.reason.as_deref() == Some(SOCKETS_NOT_HERE))
}

/// How a client reaches one box.
pub trait Transport: Send + Sync {
    /// Sends `request` and reads its whole answer.
    fn send(&self, request: &Request) -> Result<Response>;

    /// Checks whether a route can authenticate and reach this box before sending user work.
    fn probe(&self, hold: &dyn Fn(Closer) -> bool) -> Result<()> {
        if !hold(Closer::nothing()) {
            return Err(Error::Unreachable("The connection closed.".into()));
        }
        self.send(&Request::get("kuvo/info"))?.verified()
    }

    /// Opens a route that follows something. A transport that doesn't stream answers it once (a
    /// followed route's first line) and ends. `hold` is handed what closes it as soon as there's
    /// something to close, before the answer's head comes (a box holds some requests a while), so
    /// what opened it can end it meanwhile; `hold` saying no (it's ended already) ends it there.
    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream>;

    /// Opens a WebSocket on the box's engine (`request`: a GET, with no body; PROTOCOL.md,
    /// "Sockets"). One that can't carry a socket says so ([`sockets_not_here`]): the only socket
    /// today is a terminal's, so it says it of terminals.
    fn socket(&self, _request: &Request) -> Result<Socket> {
        Err(sockets_not_here(format!("Terminals on {} aren’t available this way yet.", self.describe())))
    }

    /// Looks again where the box is, and says which time it's found it somewhere new: a client
    /// reads a box's `kuvo/info` again once its server started again (another version's, maybe).
    fn refresh(&self) -> u64 {
        0
    }

    /// Whether what [`Transport::open`] gives keeps coming as things change. One that doesn't is
    /// read again on a timer by what follows it.
    fn streams(&self) -> bool;

    /// Whether the box is on the computer this client runs on (its own server, not a machine's over
    /// SSH): what a box says opens only on its own computer is opened only then.
    fn here(&self) -> bool {
        false
    }

    /// The local setup reader if this transport belongs to a collection of boxes.
    fn setup_client(&self) -> Option<crate::BoxClient> {
        None
    }

    /// How another of your computers' boxes is reached from where this one is, by its key
    /// (`kuvo/computers`): none where this client can't reach it yet.
    fn peer(&self, _key: &str) -> Option<std::sync::Arc<dyn Transport>> {
        None
    }

    /// Where it goes, in words (for a refusal of the client's own): "this Mac", a machine's name.
    fn describe(&self) -> String;

    /// Stop: the messages under these ids (`idempotency-key`) waiting to be sent again that never
    /// reached the box don't go (PROTOCOL.md, "Sending again"). One that may have reached it still
    /// goes, and its box takes it once.
    fn take_back(&self, _keys: &[String]) {}
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn route_writers_send_one_current_version_and_read_the_legacy_and_current_versions() {
        let request = Request::get("kuvo/info").header("X-Kuvo-Route-Version", "0").header("other", "value");
        assert_eq!(request.wire_headers().collect::<Vec<_>>(), [("other", "value"), (ROUTE_VERSION_HEADER, "1")]);
        assert!(check_route_version(None).is_ok());
        assert!(check_route_version(Some("1")).is_ok());
        assert!(matches!(check_route_version(Some("2")), Err(Error::BadAnswer(_))));
        assert!(matches!(check_route_version(Some("")), Err(Error::BadAnswer(_))));
    }

    #[test]
    fn every_transport_sends_json_for_a_structured_body_and_preserves_an_explicit_type() {
        let request = Request::post("route", serde_json::json!({"text": "same request"})).header("x-kuvo-operation", "saved-once");
        assert!(request.wire_headers().any(|header| header == ("content-type", "application/json")));
        let request = request.header("Content-Type", "application/vnd.kuvo+json");
        let kinds = request.wire_headers().filter(|(name, _)| name.eq_ignore_ascii_case("content-type")).collect::<Vec<_>>();
        assert_eq!(kinds, [("content-type", "application/vnd.kuvo+json")]);
        assert!(request.wire_headers().any(|header| header == ("x-kuvo-operation", "saved-once")));
    }
}
