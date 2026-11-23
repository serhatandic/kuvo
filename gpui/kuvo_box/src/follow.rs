//! Following a box as it changes: its session lists, one conversation, and the rest of what a
//! view draws. Each follow is a topic on the box's one line ([`crate::line`]), which hands what
//! it says to an [`async_channel`] receiver, which a GPUI view awaits on its executor and a
//! command-line tool reads with `next_blocking`. Only the latest is kept: a view that reads late
//! gets the state now, never a backlog. Dropping the [`Follow`] (or calling [`Follow::stop`]) ends
//! it.
//!
//! A dropped connection is never handed on: the line opens again and the box sends what was
//! missed, so a view keeps the last state it received. Only what the box itself turns down is.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::BoxClient;
use crate::error::{Error, Refusal};
use crate::gate::Gate;
use crate::line::{Said, Watch};
use crate::messages::{self, Applied, Conversation, Message, Page, ReadParts, Where};
use crate::parts::Parts;
use crate::sessions::{self, Line, Listing};

/// How long a follow waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// After a read fails, before trying again; doubled each time, up to `most`.
    pub retry_after: Duration,
    pub most: Duration,
    /// How often a conversation on a box that can't keep a stream (a one-shot `box route` over SSH)
    /// is read again.
    pub conversation_again: Duration,
    /// How often a box held for its update is asked again whether it has it.
    pub held_again: Duration,
    /// A conversation's reads that fail only say so after failing this long: a slow or restarting
    /// box isn't shown as gone (PROTOCOL.md, "The line").
    pub silent_after: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            retry_after: Duration::from_secs(1),
            most: Duration::from_secs(30),
            conversation_again: Duration::from_secs(5),
            held_again: Duration::from_secs(10),
            silent_after: Duration::from_secs(30),
        }
    }
}

/// What a session lists' follow says.
#[derive(Debug, Clone, PartialEq)]
pub enum ListUpdate {
    /// The list as it is now.
    Listing(Listing),
    /// The box can't read its engine yet (`503`): its lists wait ("Loading sessions"), saying why.
    Waiting(String),
    /// The box is older than this client needs, and waits for its update. It's asked again now
    /// and then, and the list follows once it has updated.
    Held { missing: Vec<String> },
    /// The box turned it down, or what it said couldn't be read.
    Failed(Error),
}

/// What a conversation's follow says.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationUpdate {
    /// The conversation as it is now.
    Conversation(Conversation),
    /// Reading it has failed for a while (or the box turned it down); it's tried again.
    Failed(Error),
    /// The setup no longer knows it: the follow ends.
    Gone(Refusal),
    /// The messages before those shown couldn't be read; the conversation goes on, and they can be
    /// asked for again.
    EarlierFailed(Error),
}

/// Something followed. Ends when dropped.
pub struct Follow<T> {
    updates: async_channel::Receiver<T>,
    stop: Arc<Stop>,
    /// What a conversation's reader is asked for.
    wants: Option<Arc<Wants>>,
}

impl Follow<ConversationUpdate> {
    /// Reads the messages before those shown (as you scroll up), and puts them ahead of them: the
    /// conversation that follows has them, and `earlier` says whether there are more.
    pub fn load_earlier(&self) {
        if let Some(wants) = &self.wants {
            wants.earlier();
        }
    }
}

impl<T> Follow<T> {
    /// What it sees, the latest of it.
    pub fn updates(&self) -> &async_channel::Receiver<T> {
        &self.updates
    }

    /// The next update, waiting for it (for a thread that isn't a UI's); none once it's ended.
    pub fn next_blocking(&self) -> Option<T> {
        self.updates.recv_blocking().ok()
    }

    /// Ends it.
    pub fn stop(&self) {
        self.stop.stop();
    }
}

impl<T> Drop for Follow<T> {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// Ending a follow from any thread: wakes its waits and ends its topics on the line.
#[derive(Default)]
struct Stop {
    stopped: Mutex<bool>,
    woken: Condvar,
    /// What else wakes as it ends (a reader waiting to be asked).
    wakes: Mutex<Vec<Arc<Wants>>>,
    /// Its topics on the line.
    watches: Mutex<Vec<Watch>>,
}

impl Stop {
    fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.woken.notify_all();
        let watches = std::mem::take(&mut *self.watches.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        drop(watches);
        for wants in self.wakes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter() {
            wants.close();
        }
    }

    fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Waits `time`, or until stopped: whether it's stopped.
    fn wait(&self, time: Duration) -> bool {
        let stopped = self.stopped.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (stopped, _) = self.woken.wait_timeout_while(stopped, time, |stopped| !*stopped).unwrap_or_else(|poisoned| poisoned.into_inner());
        *stopped
    }

    /// Keeps `watch` until it's stopped (at once, when it is already).
    fn keep(&self, watch: Watch) {
        let mut watches = self.watches.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.is_stopped() {
            drop(watches);
            drop(watch);
            return;
        }
        watches.push(watch);
    }
}

/// What a conversation's reader is asked for: its newest messages again, earlier ones, or to end;
/// and, on a box that can't keep a stream, to read the newest again on a timer. Asked again while
/// it reads, it reads once more after.
#[derive(Default)]
struct Wants {
    state: Mutex<Wanted>,
    woken: Condvar,
}

#[derive(Default)]
struct Wanted {
    again: bool,
    earlier: bool,
    closed: bool,
    /// How often the newest is read again with nothing asking, and when it's next due.
    timer: Option<Duration>,
    due: Option<Instant>,
}

impl Wants {
    fn change(&self, change: impl FnOnce(&mut Wanted)) {
        change(&mut self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        self.woken.notify_all();
    }

    fn again(&self) {
        self.change(|wanted| wanted.again = true);
    }

    fn earlier(&self) {
        self.change(|wanted| wanted.earlier = true);
    }

    fn close(&self) {
        self.change(|wanted| wanted.closed = true);
    }

    /// Reads on a timer from now on (`every`): only on a box that can't keep a stream.
    fn read_every(&self, every: Duration) {
        self.change(|wanted| {
            wanted.timer = Some(every);
            wanted.due = Some(Instant::now() + every);
        });
    }

    /// What's wanted (the newest again, earlier ones), waiting for it, or for the timer to come
    /// due; none once it's ended.
    fn next(&self) -> Option<(bool, bool)> {
        let mut wanted = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if wanted.closed {
                return None;
            }
            let due = wanted.due.is_some_and(|due| Instant::now() >= due);
            if wanted.again || wanted.earlier || due {
                let asked = (wanted.again || due, wanted.earlier);
                (wanted.again, wanted.earlier) = (false, false);
                if asked.0 {
                    wanted.due = wanted.timer.map(|every| Instant::now() + every);
                }
                return Some(asked);
            }
            wanted = match wanted.due {
                Some(due) => self.woken.wait_timeout(wanted, due.saturating_duration_since(Instant::now())).unwrap_or_else(|poisoned| poisoned.into_inner()).0,
                None => self.woken.wait(wanted).unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
        }
    }
}

/// Waits after failures, longer each time.
struct Backoff {
    timing: Timing,
    next: Duration,
}

impl Backoff {
    fn new(timing: Timing) -> Backoff {
        Backoff { timing, next: timing.retry_after }
    }
    fn reset(&mut self) {
        self.next = self.timing.retry_after;
    }
    /// Waits, then waits longer next time: whether it's stopped meanwhile.
    fn wait(&mut self, stop: &Stop) -> bool {
        let now = self.next;
        self.next = (self.next * 2).min(self.timing.most).max(self.timing.retry_after);
        stop.wait(now)
    }
}

/// Failures said only once they've gone on a while (`silent_after`): a box that's slow, busy or
/// starting again isn't shown as gone; one the box turns down is said at once.
struct Patience {
    after: Duration,
    since: Option<Instant>,
}

impl Patience {
    fn new(timing: Timing) -> Patience {
        Patience { after: timing.silent_after, since: None }
    }
    fn came(&mut self) {
        self.since = None;
    }
    /// Whether this failure is said now.
    fn says(&mut self, error: &Error) -> bool {
        let since = *self.since.get_or_insert_with(Instant::now);
        !matches!(error, Error::Unreachable(_) | Error::NotRunning(_)) || since.elapsed() >= self.after
    }
}

/// Hands on the latest update, in place of one not taken yet.
struct Latest<T> {
    sender: async_channel::Sender<T>,
    taken: async_channel::Receiver<T>,
}

impl<T> Clone for Latest<T> {
    fn clone(&self) -> Self {
        Latest { sender: self.sender.clone(), taken: self.taken.clone() }
    }
}

impl<T> Latest<T> {
    fn put(&self, update: T) {
        let mut update = update;
        loop {
            match self.sender.try_send(update) {
                Ok(()) | Err(async_channel::TrySendError::Closed(_)) => return,
                Err(async_channel::TrySendError::Full(back)) => {
                    let _ = self.taken.try_recv();
                    update = back;
                }
            }
        }
    }

    /// Ends the channel, so a reader waiting on it hears so (its own copy of the receiver would
    /// keep it open).
    fn close(&self) {
        self.sender.close();
    }
}

fn spawn<T: Send + 'static>(name: &str, run: impl FnOnce(&Latest<T>, Arc<Stop>) + Send + 'static) -> Follow<T> {
    let (sender, updates) = async_channel::bounded(1);
    let latest = Latest { sender, taken: updates.clone() };
    let stop = Arc::new(Stop::default());
    let theirs = stop.clone();
    let started = thread::Builder::new().name(name.into()).spawn(move || {
        run(&latest, theirs.clone());
        // (Its topics go with it: nothing more is said.)
        theirs.stop();
        latest.close();
    });
    if started.is_err() {
        updates.close();
    }
    Follow { updates, stop, wants: None }
}

/// A follow with no thread of its own: what its topics say is handed on as it comes.
fn on_line<T: Send + 'static>(watch: impl FnOnce(Latest<T>) -> Watch) -> Follow<T> {
    let (sender, updates) = async_channel::bounded(1);
    let latest = Latest { sender, taken: updates.clone() };
    let stop = Arc::new(Stop::default());
    stop.keep(watch(latest));
    Follow { updates, stop, wants: None }
}

impl BoxClient {
    /// Follows the box's session lists on the line, across the setup where the box supports it.
    pub fn follow_sessions(&self, timing: Timing) -> Follow<ListUpdate> {
        let client = self.clone();
        spawn("kuvo-box sessions", move |latest, stop| follow_sessions(client, timing, latest, &stop))
    }

    /// Follows one conversation: its messages now, the words it streams as they come, and its
    /// messages read again as each step, tool call or turn ends.
    pub fn follow_conversation(&self, session: &str, timing: Timing) -> Follow<ConversationUpdate> {
        let client = self.clone();
        let session = session.to_owned();
        let wants = Arc::new(Wants::default());
        let theirs = wants.clone();
        let mut follow = spawn("kuvo-box conversation", move |latest, stop| {
            stop.wakes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(theirs.clone());
            // (Stopped before it started: it ends at once.)
            if stop.is_stopped() {
                return;
            }
            follow_conversation(client, session, timing, latest, &stop, &theirs);
        });
        follow.wants = Some(wants);
        follow
    }
}

fn follow_sessions(client: BoxClient, timing: Timing, latest: &Latest<ListUpdate>, stop: &Stop) {
    // (A box that says it's held for this client shows its update, not its sessions: its list
    // waits, and what it is is read again in a while, to let it go once it's updated.)
    loop {
        if stop.is_stopped() {
            return;
        }
        if client.current_gate() == Gate::Unknown {
            let _ = client.connect();
        }
        let Gate::Held { missing } = client.gate() else { break };
        latest.put(ListUpdate::Held { missing });
        if stop.wait(timing.held_again) {
            return;
        }
        let _ = client.connect();
    }
    // The list as last taken, and whether the next good line is handed on even if unchanged.
    let listing = Arc::new(Mutex::new((Listing::new(), true)));
    let route_for = |merged| if merged {
        sessions::MERGED_SESSIONS_ROUTE.into()
    } else {
        sessions::events_route(None)
    };
    let route = route_for(client.advertised(sessions::MERGED_SESSIONS));
    let unconfirmed = client.gate() == Gate::Unknown;
    let box_ = client.clone();
    let theirs = latest.clone();
    let watch = client.line().watch_negotiated(route, unconfirmed, move || {
        box_.connect().ok().map(|info| route_for(info.has(sessions::MERGED_SESSIONS)))
    }, move |said| {
        let mut now = listing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match said {
            Said::Data(value) => match serde_json::from_value::<Line>(value)
                .map_err(|error| Error::BadAnswer(format!("A line of the session list couldn't be read: {error}")))
                .and_then(|line| line.check_start(client.advertised(sessions::LIST_START)))
            {
                Ok(line) => {
                    if now.0.take(line) || now.1 {
                        now.1 = false;
                        theirs.put(ListUpdate::Listing(now.0.clone()));
                    }
                }
                Err(error) => {
                    now.1 = true;
                    theirs.put(ListUpdate::Failed(error));
                }
            },
            Said::Refused(refusal) => {
                now.1 = true;
                theirs.put(if refusal.status == 503 { ListUpdate::Waiting(refusal.message) } else { ListUpdate::Failed(Error::Refused(refusal)) });
            }
            // (The next line is the whole list.)
            Said::Fresh | Said::Gap => now.1 = true,
        }
    });
    stop.keep(watch);
    while !stop.wait(Duration::from_secs(3600)) {}
}

/// A followed conversation: what's shown, and while a read is on its way, the events that came
/// meanwhile (to apply again on top of it).
struct Shown {
    conversation: Conversation,
    during: Option<Vec<Value>>,
    reconnecting: Option<ReconnectAnchor>,
    /// The parts read of its messages from before, kept here (not in each copy handed on).
    parts: ReadParts,
    /// Whether its computer has answered a read since it was shown: a last copy then only says
    /// it's away, leaving what was shown as it is.
    live: bool,
}

/// The transcript boundary at disconnect, before a new feed can add messages.
struct ReconnectAnchor {
    ids: HashSet<String>,
    oldest: f64,
}

impl Shown {
    fn disconnected(&mut self) {
        if !self.conversation.running || self.reconnecting.is_some() {
            return;
        }
        let messages = &self.conversation.messages;
        if let Some(oldest) = messages.iter().map(Message::created).filter(|created| *created > 0.0).reduce(f64::min) {
            self.reconnecting = Some(ReconnectAnchor {
                ids: messages.iter().map(|message| message.id().to_owned()).collect(),
                oldest,
            });
        }
    }
}

/// How many of a conversation's newest messages a follow reads.
const READ: usize = 100;

fn earlier_page(client: &BoxClient, location: &messages::Location, place: Where, session: &str, cursor: &str) -> crate::Result<(Vec<Message>, Option<String>)> {
    let page: Page = client.get(&location.message_route(session, messages::earlier_route(place, session, cursor, READ)))?;
    let earlier = page.earlier(READ);
    Ok((page.data, earlier))
}

/// On a reconnect, bridge a gap wider than one page of an already visible running turn. Stop
/// paging at the old transcript's time range if there is no shared message (such as after Undo).
fn reconnect_page(client: &BoxClient, location: &messages::Location, place: Where, session: &str, page: Page, known: Option<&ReconnectAnchor>) -> crate::Result<(Vec<Message>, Option<String>)> {
    let earlier = page.earlier(READ);
    let mut newest = page.data;
    let Some(ReconnectAnchor { ids, oldest }) = known else { return Ok((newest, earlier)) };
    if newest.iter().any(|message| ids.contains(message.id())) {
        return Ok((newest, earlier));
    }

    let mut previous = earlier.clone();
    let mut gap = Vec::new();
    let mut seen = HashSet::new();
    let mut created = newest.last().map_or(0.0, Message::created);
    while created >= *oldest && let Some(cursor) = previous.take().filter(|cursor| seen.insert(cursor.clone())) {
        let (older, next) = earlier_page(client, location, place, session, &cursor).map_err(|error| {
            // An old cursor failing is not the conversation itself disappearing.
            Error::Unreachable(format!("The earlier messages couldn’t be recovered: {error}"))
        })?;
        let reaches = older.iter().any(|message| ids.contains(message.id()));
        created = older.last().map_or(f64::NEG_INFINITY, Message::created);
        gap.extend(older);
        if reaches {
            newest.extend(gap);
            return Ok((newest, next));
        }
        previous = next;
    }
    Ok((newest, earlier))
}

/// The last copy the nearest box kept of another computer's conversation, while that computer
/// can't say where it runs: none where it's this box's own, or the box keeps none.
fn last_copy(client: &BoxClient, session: &str) -> Option<Conversation> {
    let location = client.where_is(session).ok()?;
    if !matches!(location, messages::Location::Peer { .. }) {
        return None;
    }
    // (The owner picks the runtime of a page read through another box: the place is a stand-in.)
    let page = client.conversation_page(session, &location, Where::Engine, READ).ok().filter(|page| page.away.is_some())?;
    let mut conversation = Conversation::new(session, Where::Engine);
    conversation.away = page.away.clone();
    conversation.catch_up(page.data, None, &[], Some(false));
    Some(conversation)
}

/// `session` read at once, as a follow first reads it (for shots and tests): another computer's
/// through its nearest box, which answers its last copy while that computer is away.
pub fn read_at_once(client: &BoxClient, session: &str) -> crate::Result<Conversation> {
    let location = client.where_is(session)?;
    let runtime = match client.conversation_runtime(session, &location) {
        Ok(runtime) => runtime,
        Err(error) => return last_copy(client, session).ok_or(error),
    };
    let place = runtime.place;
    let page = client.conversation_page(session, &location, place, READ)?;
    let mut conversation = Conversation::new(session, place);
    if page.away.is_some() {
        conversation.away = page.away.clone();
        conversation.catch_up(page.data, None, &[], Some(false));
    } else {
        let earlier = page.earlier(READ);
        conversation.catch_up(page.data, earlier, &[], Some(runtime.running));
    }
    Ok(conversation)
}

/// What a read made of the conversation.
enum Read {
    Shown,
    /// Its computer isn't answering: the last copy its nearest box kept is shown, and it's read
    /// again in a while.
    Away,
    /// The setup no longer knows it: the follow ends.
    Gone,
    Failed(Error),
}

/// Reads the conversation, and shows it with what came meanwhile: under the lock, so no older
/// copy is ever handed on after a newer one. The parts of messages from before are read first, so
/// the page shows as written.
fn read(client: &BoxClient, location: &messages::Location, shown: &Mutex<Shown>, latest: &Latest<ConversationUpdate>) -> Read {
    let (session, place, known) = {
        let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        now.during = Some(Vec::new());
        (now.conversation.session.clone(), now.conversation.place, now.reconnecting.take())
    };
    let page = match client.conversation_page(&session, location, place, READ) {
        Ok(page) if page.away.is_some() => {
            let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            now.during = None;
            if known.is_some() {
                now.reconnecting = known;
            }
            // (What its computer last said stays as it is: only that it's away now is added.)
            if now.live {
                now.conversation.away = page.away;
            } else {
                let away = page.away.clone();
                now.conversation.catch_up(page.data, None, &[], Some(false));
                now.conversation.away = away;
            }
            latest.put(ConversationUpdate::Conversation(now.conversation.clone()));
            return Read::Away;
        }
        page => page,
    };
    let read = page.and_then(|page| reconnect_page(client, location, place, &session, page, known.as_ref()));
    let (parts, running) = match &read {
        Ok((messages, _)) => {
            let owner = client.conversation_owner(location);
            let parts = owner.as_ref().map_or_else(Vec::new, |owner| read_parts(owner, shown, messages));
            let running = match location {
                messages::Location::Here(_) => is_running(client, place, &session),
                messages::Location::Peer { .. } => client.conversation_runtime(&session, location).ok().map(|runtime| runtime.running),
            };
            (parts, running)
        }
        Err(_) => (Vec::new(), None),
    };
    // A missing page from one runtime or computer cannot say the setup lost the conversation.
    let absent = match &read {
        Err(error) if error.status() == Some(404) => client.where_is(&session).err().and_then(|error| match error {
            Error::Refused(refusal) if refusal.status == 404 => Some(refusal),
            _ => None,
        }),
        _ => None,
    };
    let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let during = now.during.take().unwrap_or_default();
    match read {
        Ok((messages, earlier)) => {
            now.parts.take(parts);
            now.conversation.catch_up(messages, earlier, &during, running);
            now.conversation.away = None;
            now.live = true;
            let Shown { conversation, parts, .. } = &mut *now;
            conversation.take_parts(parts);
            latest.put(ConversationUpdate::Conversation(now.conversation.clone()));
            Read::Shown
        }
        Err(error) => {
            if let Some(refusal) = absent {
                latest.put(ConversationUpdate::Gone(refusal));
                return Read::Gone;
            }
            if known.is_some() {
                now.reconnecting = known;
            }
            Read::Failed(error)
        }
    }
}

/// Reads the messages before those shown, and puts them ahead.
fn read_earlier(client: &BoxClient, location: &messages::Location, shown: &Mutex<Shown>, latest: &Latest<ConversationUpdate>) {
    let (session, place, cursor) = {
        let now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(cursor) = now.conversation.earlier.clone() else { return };
        (now.conversation.session.clone(), now.conversation.place, cursor)
    };
    match earlier_page(client, location, place, &session, &cursor) {
        Ok((older, earlier)) => {
            let parts = client.conversation_owner(location).map_or_else(Vec::new, |owner| read_parts(&owner, shown, &older));
            let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            now.parts.take(parts);
            now.conversation.take_earlier(older, earlier);
            let Shown { conversation, parts, .. } = &mut *now;
            conversation.take_parts(parts);
            latest.put(ConversationUpdate::Conversation(now.conversation.clone()));
        }
        Err(error) => latest.put(ConversationUpdate::EarlierFailed(error)),
    }
}

/// The parts of those of `messages` from before, as their box reads them; none where it couldn't
/// say (they show as written, and are asked again with the next read).
fn read_parts(client: &BoxClient, shown: &Mutex<Shown>, messages: &[Message]) -> Vec<((String, String), Parts)> {
    let unread = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).parts.unread(messages);
    if unread.is_empty() {
        return Vec::new();
    }
    match client.read_parts(&unread) {
        Ok(parts) => unread.into_iter().zip(parts).collect(),
        Err(_) => Vec::new(),
    }
}

/// Whether a run is under way in it, as its engine (or bridge) says (`api/session/active`); none
/// when it couldn't say.
fn is_running(client: &BoxClient, place: Where, session: &str) -> Option<bool> {
    let active: Value = client.get(&format!("{}api/session/active", place.prefix())).ok()?;
    Some(active.get("data")?.as_object()?.contains_key(session))
}

/// Reads as it's asked to (and on its timer while one's set), until it's ended or the conversation's
/// gone. A read that fails is tried again, and said only once it's failed a while.
fn reader(client: &BoxClient, location: &messages::Location, shown: &Mutex<Shown>, latest: &Latest<ConversationUpdate>, stop: &Stop, wants: &Wants, timing: Timing) {
    let mut backoff = Backoff::new(timing);
    let mut patience = Patience::new(timing);
    while let Some((again, earlier)) = wants.next() {
        if stop.is_stopped() {
            return;
        }
        if earlier {
            read_earlier(client, location, shown, latest);
        }
        if !again {
            continue;
        }
        match read(client, location, shown, latest) {
            Read::Shown => {
                backoff.reset();
                patience.came();
            }
            // (Read again in a while, to catch up as soon as it answers.)
            Read::Away => {
                if backoff.wait(stop) {
                    return;
                }
                wants.again();
            }
            Read::Gone => {
                stop.stop();
                return;
            }
            Read::Failed(error) => {
                if patience.says(&error) {
                    latest.put(ConversationUpdate::Failed(error));
                }
                if backoff.wait(stop) {
                    return;
                }
                wants.again();
            }
        }
    }
}

fn follow_conversation(client: BoxClient, session: String, timing: Timing, latest: &Latest<ConversationUpdate>, stop: &Stop, wants: &Arc<Wants>) {
    let mut backoff = Backoff::new(timing);
    let mut patience = Patience::new(timing);
    // Where it lives, once.
    let (location, place) = loop {
        // Only the setup's lookup can say a conversation is gone.
        let found = match client.where_is(&session) {
            Ok(location) => client.conversation_runtime(&session, &location).map(|runtime| (location, runtime.place)).map_err(|error| {
                if error.status() == Some(404) { Error::Unreachable(error.to_string()) } else { error }
            }),
            Err(Error::Refused(refusal)) if refusal.status == 404 => {
                latest.put(ConversationUpdate::Gone(refusal));
                return;
            }
            Err(error) => Err(error),
        };
        match found {
            Ok(found) => break found,
            Err(error) => {
                // Its computer not answering: the last copy its nearest box kept, meanwhile.
                if let Some(conversation) = last_copy(&client, &session) {
                    latest.put(ConversationUpdate::Conversation(conversation));
                } else if patience.says(&error) {
                    latest.put(ConversationUpdate::Failed(error));
                }
                if backoff.wait(stop) {
                    return;
                }
            }
        }
    };
    let shown = Arc::new(Mutex::new(Shown { conversation: Conversation::new(&session, place), during: None, reconnecting: None, parts: ReadParts::default(), live: false }));

    if !client.streams() {
        // Read now, then again on the timer, and earlier ones as asked.
        wants.read_every(timing.conversation_again);
        wants.again();
        reader(&client, &location, &shown, latest, stop, wants, timing);
        return;
    }

    // Its events, on the line: applied as they come, and read again where they say to. Watched
    // before the first read, so what comes during it is applied on top.
    let wants_events = wants.clone();
    let (theirs, told) = (shown.clone(), latest.clone());
    let watch = client.line().watch(location.read_route(&session, place.events_route(), "events"), move |said| {
        let again = match said {
            Said::Data(event) => {
                let mut now = theirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let applied = now.conversation.apply(&event);
                if applied != Applied::Nothing
                    && let Some(during) = now.during.as_mut()
                {
                    during.push(event);
                }
                if matches!(applied, Applied::Changed | Applied::ChangedAndReadAgain) {
                    told.put(ConversationUpdate::Conversation(now.conversation.clone()));
                }
                matches!(applied, Applied::ReadAgain | Applied::ChangedAndReadAgain)
            }
            // What came meanwhile may be missing: read again, bridging the gap. (Its events not
            // ready yet: the box tries again by itself, and says the gap once they come.)
            Said::Refused(refusal) if refusal.status >= 500 => false,
            Said::Gap | Said::Fresh => {
                theirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).disconnected();
                true
            }
            Said::Refused(refusal) => {
                told.put(ConversationUpdate::Failed(Error::Refused(refusal)));
                false
            }
        };
        if again {
            wants_events.again();
        }
    });
    stop.keep(watch);
    wants.again();
    reader(&client, &location, &shown, latest, stop, wants, timing);
}

impl BoxClient {
    /// Signal when accounts may have changed: as the engine (or bridge) says so, and once as it's
    /// followed and after anything may have been missed.
    pub fn follow_account_changes(&self, place: Where, _timing: Timing) -> Follow<()> {
        let line = self.line();
        let follow = on_line(|latest: Latest<()>| {
            latest.put(());
            line.watch(place.events_route(), move |said| match said {
                Said::Data(event) => {
                    if matches!(event["type"].as_str(), Some("integration.updated" | "credential.updated" | "credential.switched")) {
                        latest.put(());
                    }
                }
                Said::Gap | Said::Fresh => latest.put(()),
                Said::Refused(_) => {}
            })
        });
        follow
    }
}

/// What a follow of one of the box's own lists says (a conversation's asks, its queue).
#[derive(Debug, Clone, PartialEq)]
pub enum Followed<T> {
    /// As it is now.
    Now(T),
    /// The box turned it down, or what it said couldn't be read.
    Failed(Error),
}

/// Follows `events` (ndjson: the state now, then a line as it changes) on the line.
pub(crate) fn follow_lines<T: serde::de::DeserializeOwned + Send + 'static>(client: &BoxClient, events: String) -> Follow<Followed<T>> {
    follow_lines_filter_map(client, events, Some)
}

/// The same, each line passed through `fill` before it's handed on: a line that leaves out what's
/// as it was gets it there, so one dropped for a later line (only the latest waits to be taken)
/// loses nothing.
pub(crate) fn follow_lines_filled<T: serde::de::DeserializeOwned + Send + 'static>(client: &BoxClient, events: String, mut fill: impl FnMut(T) -> T + Send + 'static) -> Follow<Followed<T>> {
    follow_lines_filter_map(client, events, move |now| Some(fill(now)))
}

/// Filter a line before it can replace the latest update waiting for the view.
pub(crate) fn follow_lines_filter_map<T: serde::de::DeserializeOwned + Send + 'static>(client: &BoxClient, events: String, take: impl FnMut(T) -> Option<T> + Send + 'static) -> Follow<Followed<T>> {
    let line = client.line();
    let take = Mutex::new(take);
    on_line(move |latest: Latest<Followed<T>>| {
        line.watch(events, move |said| match said {
            Said::Data(value) => match serde_json::from_value::<T>(value) {
                Ok(now) => {
                    let taken = (take.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))(now);
                    if let Some(now) = taken {
                        latest.put(Followed::Now(now));
                    }
                }
                Err(error) => latest.put(Followed::Failed(Error::BadAnswer(format!("What the box said couldn't be read: {error}")))),
            },
            // (Not ready yet, a box starting again: it tries again by itself, and what was said
            // last stays. Only what it turns down is said.)
            Said::Refused(refusal) if refusal.status >= 500 => {}
            Said::Refused(refusal) => latest.put(Followed::Failed(Error::Refused(refusal))),
            // (The next line is the state now.)
            Said::Gap | Said::Fresh => {}
        })
    })
}
