//! One box as a client sees it: its routes, wherever it is, behind its update gate.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{Error, Refusal, Result};
use crate::gate::Gate;
use crate::info::BoxInfo;
use crate::messages::{self, Location, Message, Page, Runtime, Where};
use crate::sessions::{self, Line};
use crate::ssh::SshBox;
use crate::transport::{Closer, Request, Response, Socket, Stream, Transport};
use crate::parts::{self, Comment, Delivery, File, Notes, Parts, Sending};
use crate::typed::{self, Answer};
use crate::{ids, place};

/// Whether this app makes sure it's you (Touch ID, or the computer's password) before what lets
/// something use what's yours from now on: said to each box with its settings requests
/// (`x-kuvo-confirms-you: 1`, PROTOCOL.md "A box's settings"), so it offers those; one that doesn't
/// say so is refused them. Set once as the app starts, where it can ask.
static CONFIRMS_YOU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_confirms_you(on: bool) {
    CONFIRMS_YOU.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn confirms_you() -> bool {
    CONFIRMS_YOU.load(std::sync::atomic::Ordering::Relaxed)
}

/// A request as it goes: a settings request says whether this app makes sure it's you first.
fn said(request: &Request) -> std::borrow::Cow<'_, Request> {
    if confirms_you() && request.route().starts_with("kuvo/settings") && !request.headers.iter().any(|(name, _)| name == "x-kuvo-confirms-you") {
        return std::borrow::Cow::Owned(request.clone().header("x-kuvo-confirms-you", "1"));
    }
    std::borrow::Cow::Borrowed(request)
}

/// One box's routes. Cheap to clone: clones share the box's transport and its gate.
#[derive(Clone)]
pub struct BoxClient {
    transport: Arc<dyn Transport>,
    gate: Arc<Mutex<Capabilities>>,
}

/// The negotiated features and gate, from one of the box's server instances.
struct Capabilities {
    gate: Gate,
    server: u64,
    features: Vec<String>,
    /// The gate last read, kept while it's read again for a server that started since.
    known: Gate,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self { gate: Gate::Unknown, server: 0, features: Vec::new(), known: Gate::Unknown }
    }
}

/// What a composer sends: the text as typed, and what goes with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The send's own id: the same send again (after a dropped connection) is taken once.
    pub id: String,
    route: Arc<OnceLock<DraftRoute>>,
    legacy_attempted: Arc<OnceLock<()>>,
    /// The box it was first sent to (`kuvo/sessions/<id>/draft`): asked again, it's of that one.
    asked_box: Arc<OnceLock<String>>,
    /// What was typed (what it replies to apart: see [`BoxClient::send_draft`]'s `quote`).
    pub text: String,
    /// Local quote rendering, carried separately from the text the owner interprets.
    pub quote: Option<String>,
    /// Files and pictures, as the engine takes them, and the note on those sent otherwise.
    pub files: Vec<File>,
    pub notes: Notes,
    /// Review comments on the changes that go with it.
    pub comments: Vec<Comment>,
    /// The sessions picked from the "&" list as it was written.
    pub named: Vec<String>,
    /// The title of the widget it answers ("" for one without a title).
    pub widget: Option<String>,
    /// Why the conversation's model can't run, as its box says it (`kuvo/sessions/models`'
    /// `refusal`): the message is kept, saying so.
    pub refusal: Option<String>,
    /// Now, or after the run under way.
    pub delivery: Delivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DraftRoute {
    Decision,
    Draft,
    Submission(String),
}

impl Draft {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Draft {
        Draft {
            id: id.into(),
            route: Arc::new(OnceLock::new()),
            legacy_attempted: Arc::new(OnceLock::new()),
            asked_box: Arc::new(OnceLock::new()),
            text: text.into(),
            quote: None,
            files: Vec::new(),
            notes: Notes::default(),
            comments: Vec::new(),
            named: Vec::new(),
            widget: None,
            refusal: None,
            delivery: Delivery::Steer,
        }
    }
    /// Retain the quote rendering with the draft so a retry carries the same intent.
    pub fn capture_quote(&mut self, quote: &dyn Fn(&str) -> String) {
        self.quote = Some(quote(&self.text));
    }

    /// Use the original handle for an explicit same-submission retry.
    pub fn bind_submission(&self, submission: String) -> bool {
        if !submission.rsplit_once('.').is_some_and(|(_, id)| id == self.id) {
            return false;
        }
        let _ = self.route.set(DraftRoute::Submission(submission.clone()));
        self.submission() == Some(submission.as_str())
    }

    pub fn submission(&self) -> Option<&str> {
        match self.route.get() {
            Some(DraftRoute::Submission(id)) => Some(id),
            _ => None,
        }
    }
}

/// What became of a text sent from a composer.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// A message, sent to the agent under this id.
    Message { id: String },
    /// Saved at the listing Box for the conversation owner.
    Pending { id: String, receipt: DraftReceipt },
    /// The box read it (`typed`), or it was kept for a model that can't run: what it did and what
    /// the client does with the rest. Where it said to send a message or run a command, that's been
    /// sent (`message`, a message's id; none for a command, which has none); anything else in it is
    /// the client's to do (a `!` command, compact, switch the mode shown, a follow-up in the side
    /// thread), and `restore` and `error` go back to the composer.
    Typed { answer: Box<Answer>, message: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftReceiptStatus {
    Pending,
    Committed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftReceipt {
    pub status: DraftReceiptStatus,
    pub operation: String,
    pub owner: String,
    pub message: Option<String>,
    pub accepted_at: Option<u64>,
    pub session: Option<String>,
    pub submission: Option<String>,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSubmissions {
    pub current_owner: Option<String>,
    pub setup: Option<String>,
    pub outcomes: Vec<DraftOutcome>,
    pub complete: bool,
    pub can_submit: Option<bool>,
    pub next: Option<String>,
    pub unknown: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftOutcome {
    pub submission: String,
    pub original_owner: String,
    pub digest: String,
    pub submitter: Value,
    pub status: String,
    pub evidence: String,
    pub conflict: Option<bool>,
    pub accepted_at: Option<u64>,
    pub effect: Option<Value>,
    pub decision: Option<Value>,
    pub draft: Option<Value>,
    pub draft_available: Option<bool>,
    pub recovery_computer: Option<String>,
}

/// The Work Queue's counts, as its panel shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkCounts {
    /// Items still open ("24 open"), as the box counts them.
    pub open: usize,
    /// Every item, done too.
    pub all: usize,
}

impl BoxClient {
    /// A box reached through `transport`.
    pub fn new(transport: impl Transport + 'static) -> BoxClient {
        BoxClient { transport: Arc::new(transport), gate: Arc::default() }
    }

    /// A box reached through a transport that's shared already.
    pub fn over(transport: Arc<dyn Transport>) -> BoxClient {
        BoxClient { transport, gate: Arc::default() }
    }

    /// The one line to this box, which everything followed on it shares (PROTOCOL.md, "The line").
    pub fn line(&self) -> Arc<crate::line::Line> {
        crate::line::Line::of(&self.transport)
    }

    /// How it's reached.
    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    /// This Mac's own box, as found under `home` (the user's home folder): its server, by its record.
    /// [`Error::NotRunning`] when Kuvo Code isn't running here.
    pub fn this_mac(home: &Path) -> Result<BoxClient> {
        let found = place::this_mac(home).ok_or_else(|| Error::NotRunning("Kuvo isn't set up on this Mac.".into()))?;
        Ok(BoxClient::new(place::server(&found, "this Mac")?))
    }

    /// A machine's box over SSH.
    pub fn machine(ssh: SshBox) -> BoxClient {
        BoxClient::new(ssh)
    }

    /// The box of `computer`, one of this box's `kuvo/computers`: this one for itself, another's
    /// where this client can reach it from here; none where it can't yet.
    pub fn computer(&self, computer: &crate::areas::Computer) -> Option<BoxClient> {
        if computer.here {
            return Some(self.clone());
        }
        let transport = self.transport.peer(&computer.key)?;
        Some(BoxClient { transport, gate: Arc::default() })
    }

    /// Messages under these ids waiting to be sent again that never reached the box don't go
    /// (PROTOCOL.md, "Sending again"); one that may have reached it, its box takes once.
    pub fn take_back(&self, ids: &[String]) {
        self.transport.take_back(ids);
    }

    /// The nearest box that serves setup-wide reads. A standalone client reads them on its box.
    pub(crate) fn setup_reader(&self) -> BoxClient {
        self.transport.setup_client().unwrap_or_else(|| self.clone())
    }

    /// The box of another of your computers, by its key, as this client reaches it (none where it
    /// doesn't yet).
    pub fn peer(&self, key: &str) -> Option<BoxClient> {
        let transport = self.transport.peer(key)?;
        Some(BoxClient { transport, gate: Arc::default() })
    }

    /// The setup's sessions, read once (a stand-in's: its events route answers its state and ends).
    #[cfg(feature = "stand-in")]
    pub fn merged_sessions_at_once(&self) -> Result<Line> {
        let response = self.transport.send(&Request::get(sessions::MERGED_SESSIONS_ROUTE))?;
        let line: Line = decode(&response)?;
        line.check_start(self.advertised(sessions::LIST_START))
    }

    /// Whether a followed route would use the same connection as before.
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.transport, &other.transport)
    }

    /// Whether what this box follows keeps coming as things change (its own server), or is read
    /// again on a timer (a machine over SSH).
    pub fn streams(&self) -> bool {
        self.transport.streams()
    }

    /// Whether the box is on the computer this client runs on.
    pub fn here(&self) -> bool {
        self.transport.here()
    }

    /// Where the box stands with this client, as last read.
    pub fn gate(&self) -> Gate {
        self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).gate.clone()
    }

    pub(crate) fn has_fleet_updates(&self) -> bool {
        self.advertised("update.fleet")
    }

    fn set_gate(&self, gate: Gate, server: u64, features: Vec<String>) {
        *self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Capabilities { known: gate.clone(), gate, server, features };
    }

    /// The gate to draw by: the one read now, or, while it's read again for a box whose server
    /// started again (a restart, a connection made anew), the one last read, so what's drawn by it
    /// (the Library in the rail) doesn't blink out and back. Never for what's sent: that waits for
    /// the gate read now.
    pub fn drawn_gate(&self) -> Gate {
        let capabilities = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if capabilities.gate == Gate::Unknown { capabilities.known.clone() } else { capabilities.gate.clone() }
    }

    /// The gate for the box's server now: not known yet for one that started again since it was
    /// read (another version's, maybe), which is read again before it's asked anything else.
    pub(crate) fn current_gate(&self) -> Gate {
        let server = self.transport.refresh();
        let mut gate = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if gate.server != server {
            let known = if gate.gate == Gate::Unknown { gate.known.clone() } else { gate.gate.clone() };
            *gate = Capabilities { server, known, ..Capabilities::default() };
        }
        gate.gate.clone()
    }

    /// Reads `kuvo/info` and takes the update gate as the box says it (`compatibility`); one
    /// answering no `kuvo/info` (`404` or `405`) is held. Read again to let a held box go once it
    /// has updated.
    pub fn connect(&self) -> Result<BoxInfo> {
        self.connect_for(None)
    }

    /// As [`BoxClient::connect`], for a message under its id (`key`) waiting on it: read with the
    /// same patience as the message, which the link sends again while it comes back (a message
    /// sent just after the connection dropped isn't failed by this read alone).
    fn connect_for(&self, key: Option<&str>) -> Result<BoxInfo> {
        let server = self.transport.refresh();
        let mut asked = Request::get("kuvo/info");
        if let Some(key) = key {
            asked = asked.header("idempotency-key", key);
        }
        let response = self.transport.send(&asked)?;
        if response.status == 404 || response.status == 405 {
            // (One that answers no `kuvo/info` at all can't say how it stands: held.)
            self.set_gate(Gate::Held { missing: Vec::new() }, server, Vec::new());
            return Err(Error::NeedsUpdating { missing: Vec::new() });
        }
        let info: BoxInfo = decode(&response)?;
        self.set_gate(Gate::said(&info.compatibility), server, info.features.clone());
        Ok(info)
    }

    /// Before a request to a box whose server hasn't been read yet: what it is, read first, so
    /// what's drawn of it is what it says. Never a refusal: every request goes to the box, which
    /// answers it (a read that fails is the request's to say, as it goes).
    pub(crate) fn check_gate(&self, request: &Request) {
        if request.route() == "kuvo/info" {
            return;
        }
        let key = request.headers.iter().find(|(name, value)| name.eq_ignore_ascii_case("idempotency-key") && !value.is_empty()).map(|(_, key)| key.as_str());
        let _ = self.read_first(key);
    }

    fn read_first(&self, key: Option<&str>) -> Result<()> {
        if self.current_gate() == Gate::Unknown {
            match self.connect_for(key) {
                Ok(_) | Err(Error::NeedsUpdating { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Whether this box advertises an optional feature (which of its routes to use), read first.
    pub(crate) fn has_feature(&self, name: &str) -> Result<bool> {
        loop {
            self.read_first(None)?;
            let capabilities = self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match &capabilities.gate {
                Gate::Unknown => continue, // A concurrent request observed a new server.
                _ => return Ok(capabilities.features.iter().any(|feature| feature == name)),
            }
        }
    }

    /// Whether the box said it has `name`, one this client draws without where it hasn't (not
    /// one of [`crate::gate::REQUIRED`], which [`Self::gate`] answers for).
    pub fn advertises(&self, name: &str) -> bool {
        self.advertised(name)
    }

    /// A feature from the negotiation that preceded an already open request.
    pub(crate) fn advertised(&self, name: &str) -> bool {
        self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).features.iter().any(|feature| feature == name)
    }

    /// Sends `request` (what it is read first) and reads its whole answer, whatever its status.
    pub fn send(&self, request: &Request) -> Result<Response> {
        self.check_gate(request);
        self.transport.send(&said(request))
    }

    /// Opens a followed route (what it is read first). `hold` gets what closes it, before its answer
    /// comes (see [`Transport::open`]).
    pub fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> Result<Stream> {
        self.check_gate(request);
        self.transport.open(&said(request), hold)
    }

    /// Opens a socket on the box's engine (what it is read first; PROTOCOL.md, "Sockets"), where the way
    /// it's reached can carry one; where it can't, says so ([`crate::transport::sockets_not_here`]).
    pub fn socket(&self, request: &Request) -> Result<Socket> {
        self.check_gate(request);
        self.transport.socket(request)
    }

    /// Its answer, read as `T`; what the box refuses, as it said it.
    pub fn value<T: DeserializeOwned>(&self, request: &Request) -> Result<T> {
        decode(&self.send(request)?)
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.value(&Request::get(path))
    }

    /// Does what the box offered beside a refusal (its `action`, posted to the route it named):
    /// Try Again for its coding agents (`kuvo/agents/start`), answered once they run, in 45 s at
    /// most. Still not, its refusal, with the state they're in.
    pub fn take_action(&self, action: &crate::error::Offered) -> Result<()> {
        self.done(&Request::post(action.route.clone(), serde_json::json!({})))
    }

    // MARK: Sessions

    /// The session list now (`since`: only what changed after where a client was).
    pub fn session_list(&self, since: Option<&str>) -> Result<Line> {
        let line: Line = self.get(&sessions::route(since))?;
        line.check_start(self.advertised(sessions::LIST_START))
    }

    /// Project `project`'s list as far as `cursor` shows it (`kuvo/sessions/page`): asked as "Show
    /// more" is, and again with the same cursor as the list changes.
    pub fn session_page(&self, project: &str, cursor: &str) -> Result<sessions::Page> {
        self.get(&sessions::page_route(project, cursor))
    }

    // MARK: A conversation

    /// The setup's location for this conversation, or the box's refusal.
    pub fn where_is(&self, session: &str) -> Result<Location> {
        self.get(&format!("kuvo/sessions/{}/location", crate::paths::segment(session)))
    }

    /// The conversation's runtime, as the owner reports it.
    pub fn conversation_runtime(&self, session: &str, location: &Location) -> Result<Runtime> {
        match location {
            Location::Here(runtime) => Ok(*runtime),
            Location::Peer { on } => self.get(&messages::peer_route(&on.id, session, "location")),
        }
    }

    /// The owner used for conversation actions.
    pub fn conversation_owner(&self, location: &Location) -> Option<Self> {
        match location {
            Location::Here(_) => Some(self.clone()),
            Location::Peer { on } if on.opens() => self.peer(&on.id),
            Location::Peer { .. } => None,
        }
    }

    /// The conversation's latest page, through this box.
    pub fn conversation_page(&self, session: &str, location: &Location, place: Where, limit: usize) -> Result<Page> {
        self.get(&location.message_route(session, messages::newest_route(place, session, limit)))
    }

    /// The conversation's newest `limit` messages, oldest first.
    pub fn messages(&self, session: &str, place: Where, limit: usize) -> Result<Vec<Message>> {
        let page: Page = self.get(&messages::newest_route(place, session, limit))?;
        let mut messages = page.data;
        messages.reverse();
        Ok(messages)
    }

    /// Sends a message as its fields (`kuvo/sessions/<id>/send`): the box writes what the agent
    /// reads, and keeps its parts with it. A message is taken once per `id` however often it comes.
    pub fn send_message(&self, session: &str, sending: &Sending) -> Result<()> {
        let body = serde_json::to_value(sending).map_err(|error| Error::BadAnswer(error.to_string()))?;
        let mut request = Request::post(parts::send_route(session), body);
        if let Some(id) = &sending.id {
            request = request.header("idempotency-key", id.clone());
        }
        let response = self.send(&request)?;
        if response.is_success() { Ok(()) } else { Err(Error::Refused(Refusal::from_answer(response.status, &response.body))) }
    }

    /// A composer's draft sent, as every client sends it: through its box's draft route where it
    /// has one, else straight to the box as fields; whatever it starts with, it's a message for the
    /// agent. `quote', given what the composer replies to:
    /// the words with it, as the client writes a reply (KuvoClient's `Quoting.message`), for a
    /// message that takes it.
    pub fn send_draft(&self, session: &str, draft: &Draft, quote: Option<&dyn Fn(&str) -> String>) -> Result<Sent> {
        // Sent before: asked again only under its id, of the box it was sent to as a draft, which
        // takes one id once and settles one a restart cut off (its answer lost, its box
        // restarting); never of another box or another way, which could send it twice.
        if draft.legacy_attempted.get().is_some()
            && !(matches!(draft.route.get(), Some(DraftRoute::Draft | DraftRoute::Decision)) && draft.asked_box.get().is_some_and(|asked| *asked == self.describe())) {
            return Ok(uncertain_draft());
        }
        let recovery = self.has_feature("typed.submission-recovery")?;
        let full = self.has_feature("typed.draft")?;
        if draft.route.get().is_none() {
            if recovery {
                let scope: DraftSubmissions = self.get(&format!("kuvo/sessions/{}/submissions?scope=1", crate::paths::segment(session)))?;
                if scope.can_submit == Some(true) && let (Some(setup), Some(owner)) = (&scope.setup, &scope.current_owner) {
                    draft.bind_submission(format!("{setup}.{owner}.{}", draft.id));
                }
            }
            let _ = draft.route.set(if full { DraftRoute::Draft } else { DraftRoute::Decision });
        }
        match draft.route.get() {
            Some(DraftRoute::Submission(_)) if recovery => return self.submit_draft(session, draft, quote),
            Some(DraftRoute::Draft) if full => {
                if draft.legacy_attempted.set(()).is_ok() {
                    let _ = draft.asked_box.set(self.describe());
                }
                return self.submit_draft(session, draft, quote);
            }
            Some(DraftRoute::Submission(_) | DraftRoute::Draft) => return Ok(uncertain_draft()),
            _ => {}
        }
        // (Its fields under its id, `idempotency-key`: its box takes one id once, so it's asked
        // again of the same box as often as it takes, never of another.)
        if draft.legacy_attempted.set(()).is_ok() {
            let _ = draft.asked_box.set(self.describe());
        }
        let quoted = |words: &str| {
            if words == draft.text && let Some(rendered) = &draft.quote {
                rendered.clone()
            } else {
                quote.map_or_else(|| words.to_owned(), |quote| quote(words))
            }
        };
        // As the box would say: with a model that can't run, it's kept, saying why.
        if let Some(answer) = typed::plain_answer(&draft.text, draft.refusal.as_deref()) {
            return Ok(Sent::Typed { answer: Box::new(answer), message: None });
        }
        let sending = Sending {
            id: Some(draft.id.clone()),
            words: quoted(&draft.text),
            files: draft.files.clone(),
            delivery: Some(draft.delivery),
            widget: draft.widget.clone(),
            comments: draft.comments.clone(),
            named: draft.named.clone(),
            notes: draft.notes.clone(),
            ..Sending::default()
        };
        self.send_message(session, &sending)?;
        Ok(Sent::Message { id: draft.id.clone() })
    }

    fn submit_draft(&self, session: &str, draft: &Draft, quote: Option<&dyn Fn(&str) -> String>) -> Result<Sent> {
        #[derive(serde::Deserialize)]
        struct Outcome {
            answer: Answer,
            message: Option<String>,
        }
        let mut body = draft_body(draft, quote);
        let action = if let Some(submission) = draft.submission() {
            body["submission"] = submission.into();
            "submit"
        } else {
            "draft"
        };
        let route = format!("kuvo/sessions/{}/{action}", crate::paths::segment(session));
        // Do not ask the transport to auto-repeat live work after disconnect. An explicit
        // same-ID retry can still recover the owner's recorded decision.
        let request = Request::post(route, body);
        let response = match self.send(&request) {
            Ok(response) => response,
            Err(_) => return Ok(uncertain_draft()),
        };
        if response.status == 202 {
            return Ok(match decode(&response) {
                Ok(receipt) => Sent::Pending { id: draft.id.clone(), receipt },
                Err(_) => uncertain_draft(),
            });
        }
        if response.status != 200 {
            let body = serde_json::from_slice::<Value>(&response.body).unwrap_or_default();
            let mut answer = Answer::refused(response.status, &response.body, &draft.text);
            if body["ambiguous"].as_bool() == Some(true) {
                answer.restore = None;
                answer.ambiguous = Some(true);
            } else if response.status >= 500 {
                let Ok(restore) = serde_json::from_value::<typed::Restore>(body["restore"].clone()) else {
                    return Ok(uncertain_draft());
                };
                answer.restore = Some(restore);
            }
            return Ok(Sent::Typed { answer: Box::new(answer), message: None });
        }
        let Outcome { answer, message } = match decode(&response) {
            Ok(outcome) => outcome,
            Err(_) => return Ok(uncertain_draft()),
        };
        if answer.send.is_some() && let Some(id) = &message {
            return Ok(Sent::Message { id: id.clone() });
        }
        Ok(Sent::Typed { answer: Box::new(answer), message })
    }

    /// Reads a pending draft's owner outcome from the Box that saved the submission.
    pub fn draft_receipt(&self, operation: &str) -> Result<DraftReceipt> {
        let mut receipt: DraftReceipt = self.get(&format!("kuvo/data/messages/{}", crate::paths::segment(operation)))?;
        if receipt.status == DraftReceiptStatus::Interrupted
            && let (Some(session), Some(id), Some(digest)) = (&receipt.session, &receipt.submission, &receipt.digest)
            && let Ok(report) = self.draft_submission_with_digest(session, id, Some(digest))
            && let Some(outcome) = report.outcomes.iter().find(|outcome| outcome.submission == *id && outcome.status == "committed"
                && outcome.evidence == "owner" && outcome.digest == *digest && outcome.accepted_at.is_some() && outcome.conflict != Some(true))
        {
            receipt.status = DraftReceiptStatus::Committed;
            receipt.accepted_at = outcome.accepted_at;
            receipt.message = None;
        }
        Ok(receipt)
    }

    /// Discover earlier submissions after reopening a conversation.
    pub fn draft_submissions(&self, session: &str, after: Option<&str>) -> Result<DraftSubmissions> {
        let mut route = format!("kuvo/sessions/{}/submissions", crate::paths::segment(session));
        if let Some(after) = after {
            route.push_str(&format!("?after={}", crate::paths::segment(after)));
        }
        self.get(&route)
    }

    /// Look up the original authority's outcome without resubmitting the draft.
    pub fn draft_submission(&self, session: &str, id: &str) -> Result<DraftSubmissions> {
        self.draft_submission_with_digest(session, id, None)
    }

    fn draft_submission_with_digest(&self, session: &str, id: &str, digest: Option<&str>) -> Result<DraftSubmissions> {
        let mut route = format!("kuvo/sessions/{}/submissions/{}", crate::paths::segment(session), crate::paths::segment(id));
        if let Some(digest) = digest {
            route.push_str(&format!("?digest={}", crate::paths::segment(digest)));
        }
        self.get(&route)
    }

    /// The parts of messages from before (none kept with them) and of those that ran one of the
    /// engine's commands, by their ids and texts (`kuvo/messages/parts`): one for each, in order.
    /// `session`, their conversation: a command still on its way there is waited for a moment.
    pub fn read_parts(&self, session: &str, messages: &[(String, String)]) -> Result<Vec<Parts>> {
        #[derive(serde::Deserialize)]
        struct Told {
            parts: Vec<Parts>,
        }
        let mut all = Vec::with_capacity(messages.len());
        for batch in messages.chunks(parts::READ_MOST) {
            let (ids, texts): (Vec<&String>, Vec<&String>) = batch.iter().map(|(id, text)| (id, text)).unzip();
            let told: Told = self.value(&Request::post(parts::READ_ROUTE, json!({ "texts": texts, "ids": ids, "session": session })))?;
            if told.parts.len() != batch.len() {
                return Err(Error::BadAnswer("The box read a different number of messages than it was asked.".into()));
            }
            all.extend(told.parts);
        }
        Ok(all)
    }

    /// The messages before `cursor` (a page's [`Page::earlier`]), newest first as the engine
    /// answers, and the way back from them.
    pub fn earlier_messages(&self, session: &str, place: Where, cursor: &str, limit: usize) -> Result<(Vec<Message>, Option<String>)> {
        let page: Page = self.get(&messages::earlier_route(place, session, cursor, limit))?;
        let earlier = page.earlier(limit);
        Ok((page.data, earlier))
    }

    /// A new send's own id.
    pub fn new_id() -> String {
        ids::message()
    }

    // MARK: Counts

    /// The Work Queue's counts, every queue on the box (`owner=all`), read without asking GitHub:
    /// how many are open is the box's count (`counts.open`).
    pub fn work_counts(&self) -> Result<WorkCounts> {
        let view: Value = self.get("kuvo/work-queue?owner=all&glance=1&counts=1")?;
        let all = view.get("items").and_then(Value::as_array).map_or(0, Vec::len);
        let open = view.get("counts").and_then(|counts| counts.get("open")).and_then(Value::as_u64).unwrap_or(0) as usize;
        Ok(WorkCounts { open, all })
    }

    /// How many asks wait on you in the box's browser (`kuvo/asks`: a sign-in, a code, an action).
    pub fn asks_count(&self) -> Result<usize> {
        let asks: Value = self.get("kuvo/asks")?;
        Ok(asks.get("asks").and_then(Value::as_array).map_or(0, Vec::len))
    }

    /// Where it is, in words.
    pub fn describe(&self) -> String {
        self.transport.describe()
    }
}

pub(crate) fn draft_body(draft: &Draft, quote: Option<&dyn Fn(&str) -> String>) -> Value {
    let mut prepared = draft.clone();
    if prepared.quote.is_none() && let Some(quote) = quote {
        prepared.capture_quote(quote);
    }
    let message = Sending {
        files: draft.files.clone(), notes: draft.notes.clone(), comments: draft.comments.clone(),
        named: draft.named.clone(), widget: draft.widget.clone(), delivery: Some(draft.delivery),
        ..Sending::default()
    };
    let mut body = json!({
        "id": draft.id, "text": draft.text, "message": message
    });
    if let Some(rendered) = prepared.quote {
        body["quotedText"] = rendered.into();
    }
    body
}

fn uncertain_draft() -> Sent {
    Sent::Typed { answer: Box::new(Answer {
        error: Some("The draft's outcome is unknown. Check the conversation before sending again.".into()),
        ambiguous: Some(true), ..Answer::default()
    }), message: None }
}

/// A successful answer read as `T`, else the box's refusal.
pub(crate) fn decode<T: DeserializeOwned>(response: &Response) -> Result<T> {
    if !response.is_success() {
        return Err(Error::Refused(Refusal::from_answer(response.status, &response.body)));
    }
    serde_json::from_slice(&response.body).map_err(|error| Error::BadAnswer(format!("The answer couldn't be read: {error}")))
}

#[cfg(test)]
mod confirms_you_tests {
    use super::*;

    /// (Only ever turned on here, as the app does where it can ask: a test that turned it off
    /// could race another's.)
    #[test]
    fn a_settings_request_says_this_app_makes_sure_its_you_and_no_other_does() {
        set_confirms_you(true);
        let page = Request::get("kuvo/settings/vault");
        let page = said(&page);
        assert!(page.headers.contains(&("x-kuvo-confirms-you".to_owned(), "1".to_owned())));
        let act = Request::post("kuvo/settings/vault/ask-q_1/always", json!({ "was": "r" }));
        let act = said(&act);
        assert_eq!(act.headers.iter().filter(|(name, _)| name == "x-kuvo-confirms-you").count(), 1);
        assert!(said(&Request::get("kuvo/vault/items/v_abcdefghij/values")).headers.is_empty());
        assert!(said(&Request::get("api/session")).headers.is_empty());
    }
}

#[cfg(test)]
mod draft_submission_tests {
    use super::*;
    use crate::transport::{Closer, Response, Stream, Transport};

    struct Owner;

    struct Recovery {
        observed: Arc<Mutex<Vec<String>>>,
        current: String,
        digest: String,
    }

    impl Transport for Recovery {
        fn send(&self, request: &Request) -> Result<Response> {
            self.observed.lock().unwrap().push(request.path.clone());
            let value = match request.route() {
                "kuvo/info" => {
                    let features: Vec<_> = crate::gate::REQUIRED.iter().copied().chain(["typed.draft", "typed.submission-recovery"]).collect();
                    json!({ "kind": "machine", "name": "Owner", "features": features })
                }
                "kuvo/sessions/ses_owned/submissions" if request.path.ends_with("?scope=1") => {
                    json!({ "currentOwner": self.current, "setup": "setup", "outcomes": [], "complete": true, "canSubmit": true })
                }
                "kuvo/sessions/ses_owned/submit" => {
                    assert_eq!(request.body.as_ref().unwrap()["submission"], "setup.original.msg_draft");
                    return Err(Error::Unreachable("Reply lost".into()));
                }
                "kuvo/data/messages/op" => json!({ "status": "interrupted", "operation": "op", "owner": "original",
                    "session": "ses_owned", "submission": "setup.original.msg_draft", "digest": "accepted" }),
                route if route.ends_with("submissions/setup.original.msg_draft") => json!({
                    "currentOwner": self.current, "complete": true, "outcomes": [{
                        "submission": "setup.original.msg_draft", "originalOwner": "original", "digest": self.digest,
                        "evidence": "owner", "submitter": {"kind": "computer"}, "status": "committed", "acceptedAt": 42,
                        "effect": {"message": "msg_draft"}
                    }]
                }),
                route => panic!("Unexpected draft recovery route {route}"),
            };
            Ok(Response { status: 200, content_type: Some("application/json".into()), body: serde_json::to_vec(&value).unwrap() })
        }
        fn open(&self, _: &Request, _: &dyn Fn(Closer) -> bool) -> Result<Stream> { unreachable!() }
        fn streams(&self) -> bool { false }
        fn describe(&self) -> String { self.current.clone() }
    }

    impl Transport for Owner {
        fn send(&self, request: &Request) -> Result<Response> {
            let (status, value) = match request.route() {
                "kuvo/info" => {
                    let features: Vec<_> = crate::gate::REQUIRED.iter().copied().chain(["typed.draft"]).collect();
                    (200, json!({ "kind": "machine", "name": "Owner", "features": features }))
                }
                "kuvo/sessions/ses_owned/draft" => {
                    assert!(!request.headers.iter().any(|(key, _)| key == "idempotency-key"));
                    let draft = request.body.as_ref().unwrap();
                    assert!(draft.get("refusal").is_none());
                    if draft["text"] == "!test" {
                        (409, json!({ "ambiguous": true, "message": "Check the owner's result." }))
                    } else if draft["text"] == "Wait" {
                        (202, json!({ "status": "pending", "operation": "op", "owner": "owner" }))
                    } else if draft["text"] == "Lost reply" {
                        return Err(crate::Error::Unreachable("Connection closed".into()));
                    } else if draft["text"] == "Gateway reply" {
                        (502, json!({ "message": "Gateway lost the reply." }))
                    } else if draft["text"] == "Malformed restore" {
                        (503, json!({ "restore": {} }))
                    } else if draft["text"] == "Proven refusal" {
                        (503, json!({ "message": 7, "restore": { "text": "Keep this draft" } }))
                    } else {
                        assert_eq!(draft["quotedText"], "[Hello]");
                        (200, json!({ "answer": { "send": { "text": "Hello", "quoted": true } }, "message": "msg_draft" }))
                    }
                }
                "kuvo/data/messages/op" => (200, json!({ "status": "interrupted", "operation": "op", "owner": "owner", "message": "Check the conversation." })),
                route => panic!("Unexpected client-side execution route {route}"),
            };
            Ok(Response { status, content_type: Some("application/json".into()), body: serde_json::to_vec(&value).unwrap() })
        }
        fn open(&self, _: &Request, _: &dyn Fn(Closer) -> bool) -> Result<Stream> { unreachable!() }
        fn streams(&self) -> bool { false }
        fn describe(&self) -> String { "Owner".into() }
    }

    #[test]
    fn owner_decides_using_live_facts_and_uncertain_commands_are_not_restored_for_a_new_send() {
        let client = BoxClient::new(Owner);
        let mut draft = Draft::new("msg_draft", "Hello");
        draft.refusal = Some("This is stale.".into());
        let sent = client.send_draft("ses_owned", &draft, Some(&|text| format!("[{text}]"))).unwrap();
        assert!(matches!(sent, Sent::Message { id } if id == "msg_draft"));
        let shell = client.send_draft("ses_owned", &Draft::new("msg_shell", "!test"), None).unwrap();
        assert!(matches!(shell, Sent::Typed { answer, .. } if answer.restore.is_none() && answer.ambiguous == Some(true)));
    }

    #[test]
    fn pending_draft_keeps_its_durable_receipt() {
        let client = BoxClient::new(Owner);
        let sent = client.send_draft("ses_owned", &Draft::new("msg_wait", "Wait"), None).unwrap();
        let Sent::Pending { id, receipt } = sent else { panic!("Expected a pending draft"); };
        assert_eq!(id, "msg_wait");
        assert_eq!(receipt.status, DraftReceiptStatus::Pending);
        let receipt = client.draft_receipt(&receipt.operation).unwrap();
        assert_eq!(receipt.status, DraftReceiptStatus::Interrupted);
        assert_eq!(receipt.message.as_deref(), Some("Check the conversation."));
    }

    #[test]
    fn a_relaunched_client_recovers_only_an_exact_committed_intent() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let client = |current: &str, digest: &str| BoxClient::new(Recovery { observed: observed.clone(), current: current.into(), digest: digest.into() });
        let draft = Draft::new("msg_draft", "Hello");
        assert!(matches!(client("original", "accepted").send_draft("ses_owned", &draft, None).unwrap(),
            Sent::Typed { answer, .. } if answer.ambiguous == Some(true)));
        assert_eq!(draft.submission(), Some("setup.original.msg_draft"));
        let relaunched = client("successor", "accepted");
        let report = relaunched.draft_submission("ses_owned", "setup.original.msg_draft").unwrap();
        assert_eq!(report.current_owner.as_deref(), Some("successor"));
        assert_eq!(report.outcomes[0].original_owner, "original");
        assert_eq!(relaunched.draft_receipt("op").unwrap().accepted_at, Some(42));
        assert_eq!(client("successor", "changed").draft_receipt("op").unwrap().status, DraftReceiptStatus::Interrupted);
        assert_eq!(observed.lock().unwrap().iter().filter(|route| route.ends_with("/submit")).count(), 1);
    }

    #[test]
    fn losing_a_legacy_reply_never_moves_the_same_draft_to_a_different_protocol_lane() {
        let draft = Draft::new("msg_draft", "Lost reply");
        let _ = BoxClient::new(Owner).send_draft("ses_owned", &draft, None).unwrap();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let other = BoxClient::new(Recovery { observed: observed.clone(), current: "successor".into(), digest: "accepted".into() });
        assert!(matches!(other.send_draft("ses_owned", &draft.clone(), None).unwrap(),
            Sent::Typed { answer, .. } if answer.ambiguous == Some(true)));
        assert!(observed.lock().unwrap().is_empty());
    }

    /// Its reply lost once (its box restarting), then answered.
    struct LosesOnce {
        drafts: Arc<Mutex<u32>>,
    }

    impl Transport for LosesOnce {
        fn send(&self, request: &Request) -> Result<Response> {
            let value = match request.route() {
                "kuvo/info" => {
                    let features: Vec<_> = crate::gate::REQUIRED.iter().copied().chain(["typed.draft"]).collect();
                    json!({ "kind": "machine", "name": "Owner", "features": features })
                }
                "kuvo/sessions/ses_owned/draft" => {
                    let mut drafts = self.drafts.lock().unwrap();
                    *drafts += 1;
                    assert_eq!(request.body.as_ref().unwrap()["id"], "msg_again");
                    if *drafts == 1 {
                        return Err(Error::Unreachable("Connection closed".into()));
                    }
                    json!({ "answer": { "send": { "text": "Hello" } }, "message": "msg_again" })
                }
                route => panic!("Unexpected route {route}"),
            };
            Ok(Response { status: 200, content_type: Some("application/json".into()), body: serde_json::to_vec(&value).unwrap() })
        }
        fn open(&self, _: &Request, _: &dyn Fn(Closer) -> bool) -> Result<Stream> { unreachable!() }
        fn streams(&self) -> bool { false }
        fn describe(&self) -> String { "Owner".into() }
    }

    /// #809: a send whose reply was lost is asked again of its box under its id (which takes one id
    /// once), and answered, not left "unknown" for good.
    #[test]
    fn a_draft_whose_reply_was_lost_is_asked_again_of_its_box_under_its_id() {
        let drafts = Arc::new(Mutex::new(0));
        let client = BoxClient::new(LosesOnce { drafts: drafts.clone() });
        let draft = Draft::new("msg_again", "Hello");
        assert!(matches!(client.send_draft("ses_owned", &draft, None).unwrap(), Sent::Typed { answer, .. } if answer.ambiguous == Some(true)));
        // (As the composer keeps it: a copy.)
        assert_eq!(client.send_draft("ses_owned", &draft.clone(), None).unwrap(), Sent::Message { id: "msg_again".into() });
        assert_eq!(*drafts.lock().unwrap(), 2);
    }

    #[test]
    fn loss_of_a_plain_draft_reply_preserves_uncertainty() {
        let client = BoxClient::new(Owner);
        for text in ["Lost reply", "Gateway reply", "Malformed restore"] {
            let sent = client.send_draft("ses_owned", &Draft::new("msg_lost", text), None).unwrap();
            assert!(matches!(sent, Sent::Typed { answer, .. } if answer.restore.is_none() && answer.ambiguous == Some(true)));
        }
        let sent = client.send_draft("ses_owned", &Draft::new("msg_refused", "Proven refusal"), None).unwrap();
        assert!(matches!(sent, Sent::Typed { answer, .. } if answer.ambiguous.is_none() && answer.restore.as_ref().is_some_and(|restore| restore.text == "Keep this draft")));
    }
}

#[cfg(test)]
mod drawn_gate_tests {
    use super::*;
    use crate::transport::{Closer, Response, Stream, Transport};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// A box that starts again (`server`), answering its info or not yet (`answers`), and how it
    /// says it stands with this client (`compatibility`).
    struct Restarting {
        server: Arc<AtomicU64>,
        answers: Arc<AtomicBool>,
        compatibility: Arc<Mutex<serde_json::Value>>,
    }

    impl Transport for Restarting {
        fn send(&self, request: &Request) -> Result<Response> {
            if !self.answers.load(Ordering::SeqCst) {
                return Err(crate::Error::Unreachable("Starting".into()));
            }
            let value = match request.route() {
                "kuvo/info" => json!({ "kind": "machine", "name": "lokum", "features": [], "compatibility": self.compatibility.lock().unwrap().clone() }),
                _ => json!({}),
            };
            Ok(Response { status: 200, content_type: Some("application/json".into()), body: serde_json::to_vec(&value).unwrap() })
        }
        fn open(&self, _: &Request, _: &dyn Fn(Closer) -> bool) -> Result<Stream> { unreachable!() }
        fn streams(&self) -> bool { false }
        fn refresh(&self) -> u64 { self.server.load(Ordering::SeqCst) }
        fn describe(&self) -> String { "lokum".into() }
    }

    #[test]
    fn what_a_box_has_stays_drawn_while_its_gate_is_read_again_after_it_starts_again() {
        let (server, answers) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(true)));
        let compatibility = Arc::new(Mutex::new(json!({ "state": "open" })));
        let client = BoxClient::new(Restarting { server: server.clone(), answers: answers.clone(), compatibility: compatibility.clone() });
        client.connect().unwrap();
        assert!(client.drawn_gate().has("library"));

        // It starts again and doesn't answer yet: nothing's sent past the gate, but the Library
        // stays drawn as it was.
        server.store(1, Ordering::SeqCst);
        answers.store(false, Ordering::SeqCst);
        assert!(client.send(&Request::get("kuvo/library")).is_err());
        assert_eq!(client.gate(), Gate::Unknown);
        assert!(client.drawn_gate().has("library"));

        // Back, an older version's without it: drawn as it says now.
        answers.store(true, Ordering::SeqCst);
        *compatibility.lock().unwrap() = json!({ "state": "behind", "missing": ["library"] });
        let _ = client.send(&Request::get("kuvo/library"));
        assert!(!client.drawn_gate().has("library"));
    }
}
