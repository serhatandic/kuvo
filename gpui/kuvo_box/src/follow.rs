//! Following a box as it changes: its session lists, and one conversation. Each follow runs on a
//! thread of its own and hands what it sees to an [`async_channel`] receiver, which a GPUI view
//! awaits on its executor and a command-line tool reads with `next_blocking`. Only the latest is
//! kept: a view that reads late gets the state now, never a backlog. Dropping the [`Follow`] (or
//! calling [`Follow::stop`]) ends it, closing what it has open, a request still waiting for its
//! answer too.
//!
//! Over a box's own server, what's followed streams. Over SSH, a box answers each request once,
//! so it's read again on a timer ([`Timing`]).

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::BoxClient;
use crate::error::{Error, Refusal};
use crate::lines;
use crate::messages::{self, Applied, Conversation, Message, Page, ReadParts, Where};
use crate::parts::Parts;
use crate::sessions::{self, Line, Listing};
use crate::transport::{Closer, Request, Stream};

/// How long a follow waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// After a stream drops or a read fails, before trying again; doubled each time, up to `most`.
    pub retry_after: Duration,
    pub most: Duration,
    /// How often a box that doesn't stream (a machine over SSH) has its session list read again:
    /// every half minute, as PROTOCOL.md says.
    pub lists_again: Duration,
    /// How often a conversation on such a box is read again.
    pub conversation_again: Duration,
    /// How often a box held for its update is asked again whether it has it.
    pub held_again: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            retry_after: Duration::from_secs(1),
            most: Duration::from_secs(30),
            lists_again: Duration::from_secs(30),
            conversation_again: Duration::from_secs(5),
            held_again: Duration::from_secs(10),
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
    /// Reading it failed; it's tried again.
    Failed(Error),
}

/// What a conversation's follow says.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationUpdate {
    /// The conversation as it is now.
    Conversation(Conversation),
    /// Reading it failed; it's tried again.
    Failed(Error),
    /// It isn't on the box (any more): the follow ends.
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

/// Ending a follow from any thread: wakes its waits and closes what it has open.
#[derive(Default)]
struct Stop {
    state: Mutex<(bool, Vec<Closer>)>,
    woken: Condvar,
    /// What else wakes as it ends (a reader waiting to be asked).
    wakes: Mutex<Vec<Arc<Wants>>>,
}

impl Stop {
    fn stop(&self) {
        let closers = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.0 = true;
            std::mem::take(&mut state.1)
        };
        self.woken.notify_all();
        for closer in closers {
            closer.close();
        }
        for wants in self.wakes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter() {
            wants.close();
        }
    }

    fn is_stopped(&self) -> bool {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).0
    }

    /// Waits `time`, or until stopped: whether it's stopped.
    fn wait(&self, time: Duration) -> bool {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (state, _) = self.woken.wait_timeout_while(state, time, |state| !state.0).unwrap_or_else(|poisoned| poisoned.into_inner());
        state.0
    }

    /// Keeps `closer` to close on stop; false (and closed) when it's stopped already.
    fn hold(&self, closer: Closer) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.0 {
            drop(state);
            closer.close();
            return false;
        }
        state.1.push(closer);
        true
    }

    fn release(&self) {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).1.clear();
    }
}

/// What a conversation's reader is asked for: its newest messages again, earlier ones, or to end;
/// and, while nothing streams (a machine over SSH, or a stream that can't be opened again yet), to
/// read the newest again on a timer. Asked again while it reads, it reads once more after.
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
    /// The socket whose first successful catch-up can disable fallback reads.
    feed: Option<u64>,
    next_feed: u64,
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

    /// Reads on a timer from now on (`every`), or only as asked (none).
    fn read_every(&self, every: Option<Duration>) {
        self.change(|wanted| {
            wanted.feed = None;
            if wanted.timer != every {
                wanted.timer = every;
                wanted.due = every.map(|every| Instant::now() + every);
            }
        });
    }

    fn opened(&self) {
        self.change(|wanted| {
            wanted.next_feed += 1;
            wanted.feed = Some(wanted.next_feed);
            wanted.again = true;
        });
    }

    fn caught_up(&self, feed: Option<u64>) {
        self.change(|wanted| {
            if feed.is_some() && feed == wanted.feed {
                wanted.timer = None;
                wanted.due = None;
            }
        });
    }

    /// What's wanted (the newest again, earlier ones), waiting for it, or for the timer to come
    /// due (asking for earlier ones doesn't put it off); none once it's ended.
    fn next(&self) -> Option<(bool, bool, Option<u64>)> {
        let mut wanted = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if wanted.closed {
                return None;
            }
            let due = wanted.due.is_some_and(|due| Instant::now() >= due);
            if wanted.again || wanted.earlier || due {
                let asked = (wanted.again || due, wanted.earlier, wanted.feed);
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

/// A passing drop, taken quietly: once a follow has come through, the first failure to reach its
/// box after it (the link dropped, a session ended under it) is tried again at once, unsaid; only
/// one that fails again is said (PROTOCOL.md, "When a client says a computer isn't connected").
/// Only after it held a while: one that drops again right after it's back is said, not retried
/// over and over.
#[derive(Default)]
struct Quiet {
    /// Since when it's come through, this time.
    since: Option<std::time::Instant>,
}

/// How long it must have held for a drop to be taken quietly.
const HELD_AWHILE: Duration = Duration::from_secs(10);

impl Quiet {
    /// It came through.
    fn came(&mut self) {
        self.since.get_or_insert_with(std::time::Instant::now);
    }
    /// Whether this failure is the one taken quietly (then tried again at once).
    fn takes(&mut self, error: &Error) -> bool {
        let quiet = matches!(error, Error::Unreachable(_)) && self.since.is_some_and(|since| since.elapsed() >= HELD_AWHILE);
        self.since = None;
        quiet
    }
}

/// Hands on the latest update, in place of one not taken yet.
struct Latest<T> {
    sender: async_channel::Sender<T>,
    taken: async_channel::Receiver<T>,
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
        run(&latest, theirs);
        latest.close();
    });
    if started.is_err() {
        updates.close();
    }
    Follow { updates, stop, wants: None }
}

/// The rest of an answer, for what it says when it's a refusal.
fn refusal(stream: &mut Stream) -> Refusal {
    let mut body = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stream.reader, &mut body);
    Refusal::from_answer(stream.status, &body)
}

impl BoxClient {
    /// Follows the box's session lists: the list now, and again as it changes (`kuvo/sessions/list/
    /// events`, resumed from where it was after a drop; over SSH, read again every half minute).
    pub fn follow_sessions(&self, timing: Timing) -> Follow<ListUpdate> {
        let client = self.clone();
        spawn("kuvo-box sessions", move |latest, stop| follow_sessions(client, timing, latest, stop))
    }

    /// Follows one conversation: its messages now, the words it streams as they come, and its
    /// messages read again as each step, tool call or turn ends (over SSH, read again every few
    /// seconds).
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

fn follow_sessions(client: BoxClient, timing: Timing, latest: &Latest<ListUpdate>, stop: Arc<Stop>) {
    let mut listing = Listing::new();
    let mut backoff = Backoff::new(timing);
    let mut quiet = Quiet::default();
    while !stop.is_stopped() {
        let streams = client.streams();
        let path = if streams { sessions::events_route(listing.at()) } else { sessions::route(listing.at()) };
        let mut stream = match client.open(&Request::get(path), &|closer| stop.hold(closer)) {
            Ok(stream) => stream,
            Err(Error::NeedsUpdating { missing }) => {
                latest.put(ListUpdate::Held { missing });
                if stop.wait(timing.held_again) {
                    return;
                }
                // Asked again whether it has updated.
                let _ = client.connect();
                continue;
            }
            Err(error) => {
                stop.release();
                if stop.is_stopped() {
                    return;
                }
                if quiet.takes(&error) {
                    continue;
                }
                latest.put(ListUpdate::Failed(error));
                if backoff.wait(&stop) {
                    return;
                }
                continue;
            }
        };
        if !(200..300).contains(&stream.status) {
            let refusal = refusal(&mut stream);
            stop.release();
            latest.put(if stream.status == 503 { ListUpdate::Waiting(refusal.message) } else { ListUpdate::Failed(Error::Refused(refusal)) });
            if backoff.wait(&stop) {
                return;
            }
            continue;
        }
        let mut again_now = false;
        loop {
            match lines::next_line(&mut *stream.reader) {
                Ok(Some(text)) => match serde_json::from_str::<Line>(&text) {
                    Ok(line) => {
                        backoff.reset();
                        quiet.came();
                        if listing.take(line) {
                            latest.put(ListUpdate::Listing(listing.clone()));
                        }
                    }
                    Err(error) => latest.put(ListUpdate::Failed(Error::BadAnswer(format!("A line of the session list couldn't be read: {error}")))),
                },
                Ok(None) => break,
                Err(error) => {
                    let error = Error::Unreachable(format!("The session list stopped coming ({}).", error.kind()));
                    if !stop.is_stopped() {
                        again_now = quiet.takes(&error);
                        if !again_now {
                            latest.put(ListUpdate::Failed(error));
                        }
                    }
                    break;
                }
            }
        }
        stop.release();
        // A stream that ended is opened again from where it was; a box that answers once is read
        // again in a while. One that dropped quietly, at once.
        if again_now {
            continue;
        }
        let stopped = if streams { backoff.wait(&stop) } else { stop.wait(timing.lists_again) };
        if stopped {
            return;
        }
    }
}

/// A followed conversation: what's shown, and while a read is on its way, the events that came
/// meanwhile (to apply again on top of it).
struct Shown {
    conversation: Conversation,
    during: Option<Vec<Value>>,
    reconnecting: bool,
    /// The parts read of its messages from before, kept here (not in each copy handed on).
    parts: ReadParts,
}

/// How many of a conversation's newest messages a follow reads.
const READ: usize = 100;

/// On a reconnect, bridge a gap wider than one page of an already visible running turn. Stop
/// paging at the old transcript's time range if there is no shared message (such as after Undo).
fn reconnect_page(client: &BoxClient, place: Where, session: &str, page: Page, known: Option<(HashSet<String>, f64)>) -> crate::Result<(Vec<Message>, Option<String>)> {
    let earlier = page.earlier(READ);
    let mut newest = page.data;
    let Some((ids, oldest)) = known else { return Ok((newest, earlier)) };
    if newest.iter().any(|message| ids.contains(message.id())) {
        return Ok((newest, earlier));
    }

    let mut previous = earlier.clone();
    let mut gap = Vec::new();
    let mut seen = HashSet::new();
    let mut created = newest.last().map_or(0.0, Message::created);
    while created >= oldest && let Some(cursor) = previous.take().filter(|cursor| seen.insert(cursor.clone())) {
        let (older, next) = client.earlier_messages(session, place, &cursor, READ).map_err(|error| {
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

/// Reads the conversation, and shows it with what came meanwhile: under the lock, so no older
/// copy is ever handed on after a newer one. The parts of messages from before are read first, so
/// the page shows as written. False once it's gone, which ends the follow.
fn read(client: &BoxClient, shown: &Mutex<Shown>, latest: &Latest<ConversationUpdate>, caught_up: impl FnOnce()) -> bool {
    let (session, place, reconnecting, known) = {
        let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        now.during = Some(Vec::new());
        let reconnecting = std::mem::take(&mut now.reconnecting);
        let known = reconnecting.then(|| {
            let messages = &now.conversation.messages;
            let oldest = messages.iter().map(Message::created).filter(|created| *created > 0.0).reduce(f64::min)?;
            let ids = messages.iter().map(|message| message.id().to_owned()).collect();
            Some((ids, oldest))
        }).flatten();
        (now.conversation.session.clone(), now.conversation.place, reconnecting, known)
    };
    let read = client.get(&messages::newest_route(place, &session, READ)).and_then(|page| reconnect_page(client, place, &session, page, known));
    let (parts, running) = match &read {
        Ok((messages, _)) => (read_parts(client, shown, messages), is_running(client, place, &session)),
        Err(_) => (Vec::new(), None),
    };
    let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let during = now.during.take().unwrap_or_default();
    match read {
        Ok((messages, earlier)) => {
            now.parts.take(parts);
            now.conversation.catch_up(messages, earlier, &during, running);
            let Shown { conversation, parts, .. } = &mut *now;
            conversation.take_parts(parts);
            latest.put(ConversationUpdate::Conversation(now.conversation.clone()));
            caught_up();
            true
        }
        Err(Error::Refused(refusal)) if refusal.status == 404 => {
            latest.put(ConversationUpdate::Gone(client.not_here()));
            false
        }
        Err(error) => {
            now.reconnecting |= reconnecting;
            latest.put(ConversationUpdate::Failed(error));
            true
        }
    }
}

/// Reads the messages before those shown, and puts them ahead.
fn read_earlier(client: &BoxClient, shown: &Mutex<Shown>, latest: &Latest<ConversationUpdate>) {
    let (session, place, cursor) = {
        let now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(cursor) = now.conversation.earlier.clone() else { return };
        (now.conversation.session.clone(), now.conversation.place, cursor)
    };
    match client.earlier_messages(&session, place, &cursor, READ) {
        Ok((older, earlier)) => {
            let parts = read_parts(client, shown, &older);
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
    let (unread, session) = {
        let now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (now.parts.unread(messages), now.conversation.session.clone())
    };
    if unread.is_empty() {
        return Vec::new();
    }
    match client.read_parts(&session, &unread) {
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
/// gone.
fn reader(client: &BoxClient, shown: &Mutex<Shown>, latest: &Latest<ConversationUpdate>, stop: &Stop, wants: &Wants) {
    while let Some((again, earlier, feed)) = wants.next() {
        if stop.is_stopped() {
            return;
        }
        if earlier {
            read_earlier(client, shown, latest);
        }
        if again && !read(client, shown, latest, || wants.caught_up(feed)) {
            stop.stop();
            return;
        }
    }
}

fn follow_conversation(client: BoxClient, session: String, timing: Timing, latest: &Latest<ConversationUpdate>, stop: &Stop, wants: &Wants) {
    let mut backoff = Backoff::new(timing);
    // Where it lives, once.
    let place = loop {
        match client.where_is(&session) {
            Ok(place) => break place,
            Err(Error::Refused(refusal)) if refusal.status == 404 => {
                latest.put(ConversationUpdate::Gone(refusal));
                return;
            }
            Err(error) => {
                latest.put(ConversationUpdate::Failed(error));
                if backoff.wait(stop) {
                    return;
                }
            }
        }
    };
    let shown = Mutex::new(Shown { conversation: Conversation::new(session, place), during: None, reconnecting: false, parts: ReadParts::default() });

    if !client.streams() {
        // Read now, then again on the timer, and earlier ones as asked.
        wants.read_every(Some(timing.conversation_again));
        wants.again();
        reader(&client, &shown, latest, stop, wants);
        return;
    }

    // Reads come on a thread of their own, so the events keep coming meanwhile. Once the box no
    // longer streams, the reader goes on alone, on its timer, until the follow ends.
    // If the first stream takes time to open, the reader starts on the same fallback timer.
    wants.read_every(Some(timing.conversation_again));
    thread::scope(|scope| {
        let (client, shown) = (&client, &shown);
        let reads = thread::Builder::new().name("kuvo-box conversation reads".into()).spawn_scoped(scope, move || reader(client, shown, latest, stop, wants));
        if reads.is_err() || !follow_events(client, place, timing, latest, stop, shown, wants) {
            wants.close();
        }
    });
}

/// Follows the conversation's events, applying them as they come. While its stream can't be opened
/// again, the reader reads on its timer meanwhile; once the box doesn't stream at all, it's handed
/// over to the reader for good: true then, false once it's ended.
fn follow_events(client: &BoxClient, place: Where, timing: Timing, latest: &Latest<ConversationUpdate>, stop: &Stop, shown: &Mutex<Shown>, wants: &Wants) -> bool {
    let mut backoff = Backoff::new(timing);
    let mut quiet = Quiet::default();
    while !stop.is_stopped() {
        if !client.streams() {
            wants.read_every(Some(timing.conversation_again));
            wants.again();
            return true;
        }
        let mut stream = match client.open(&Request::get(place.events_route()), &|closer| stop.hold(closer)) {
            Ok(stream) if (200..300).contains(&stream.status) => stream,
            Ok(mut stream) => {
                let refusal = refusal(&mut stream);
                stop.release();
                // Read on the timer meanwhile.
                wants.read_every(Some(timing.conversation_again));
                latest.put(ConversationUpdate::Failed(Error::Refused(refusal)));
                if backoff.wait(stop) {
                    return false;
                }
                continue;
            }
            Err(error) => {
                stop.release();
                if stop.is_stopped() {
                    return false;
                }
                if quiet.takes(&error) {
                    continue;
                }
                // Read on the timer meanwhile.
                wants.read_every(Some(timing.conversation_again));
                latest.put(ConversationUpdate::Failed(error));
                if backoff.wait(stop) {
                    return false;
                }
                continue;
            }
        };
        // Read once it's listening so nothing from before is missed. Keep the fallback timer
        // until a read on this feed succeeds.
        wants.opened();
        let mut again_now = false;
        loop {
            match lines::next_event(&mut *stream.reader) {
                Ok(Some(data)) => {
                    let Ok(event) = serde_json::from_str::<Value>(&data) else { continue };
                    backoff.reset();
                    quiet.came();
                    let applied = {
                        let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        let applied = now.conversation.apply(&event);
                        if applied != Applied::Nothing
                            && let Some(during) = now.during.as_mut()
                        {
                            during.push(event);
                        }
                        if matches!(applied, Applied::Changed | Applied::ChangedAndReadAgain) {
                            latest.put(ConversationUpdate::Conversation(now.conversation.clone()));
                        }
                        applied
                    };
                    if matches!(applied, Applied::ReadAgain | Applied::ChangedAndReadAgain) {
                        wants.again();
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let error = Error::Unreachable(format!("The conversation stopped coming ({}).", error.kind()));
                    if !stop.is_stopped() {
                        again_now = quiet.takes(&error);
                        if !again_now {
                            latest.put(ConversationUpdate::Failed(error));
                        }
                    }
                    break;
                }
            }
        }
        stop.release();
        {
            let mut now = shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            now.reconnecting |= now.conversation.running;
        }
        // Read on the timer even if the immediate reopen takes a while to answer.
        wants.read_every(Some(timing.conversation_again));
        // Dropped once: opened again at once, unsaid (read again as it opens).
        if again_now {
            continue;
        }
        if stop.is_stopped() || backoff.wait(stop) {
            return false;
        }
    }
    false
}

impl BoxClient {
    /// Signal when accounts may have changed, including when the event stream reconnects.
    pub fn follow_account_changes(&self, place: Where, timing: Timing) -> Follow<()> {
        let client = self.clone();
        spawn("kuvo accounts events", move |latest, stop| {
            let mut backoff = Backoff::new(timing);
            while !stop.is_stopped() {
                if let Ok(mut stream) = client.open(&Request::get(place.events_route()), &|closer| stop.hold(closer))
                    && (200..300).contains(&stream.status)
                {
                    latest.put(());
                    while let Ok(Some(data)) = lines::next_event(&mut *stream.reader) {
                        backoff.reset();
                        let Ok(event) = serde_json::from_str::<Value>(&data) else { continue };
                        if matches!(event["type"].as_str(), Some("integration.updated" | "credential.updated" | "credential.switched")) {
                            latest.put(());
                        }
                    }
                }
                stop.release();
                if backoff.wait(&stop) {
                    break;
                }
            }
        })
    }
}

/// What a follow of one of the box's own lists says (a conversation's asks, its queue).
#[derive(Debug, Clone, PartialEq)]
pub enum Followed<T> {
    /// As it is now.
    Now(T),
    /// Reading it failed (or it's not there): it's tried again.
    Failed(Error),
}

/// Follows `events` (ndjson: the state now, then a line as it changes); over SSH, `once` (the same
/// answered once) is read again every few seconds.
pub(crate) fn follow_lines<T: serde::de::DeserializeOwned + Send + 'static>(client: &BoxClient, name: &str, events: String, once: String, timing: Timing) -> Follow<Followed<T>> {
    follow_lines_filled(client, name, events, once, timing, |now| now)
}

/// The same, each line passed through `fill` on the follow's own thread before it's handed on: a
/// line that leaves out what's as it was gets it there, so one dropped for a later line (only the
/// latest waits to be taken) loses nothing.
pub(crate) fn follow_lines_filled<T: serde::de::DeserializeOwned + Send + 'static>(client: &BoxClient, name: &str, events: String, once: String, timing: Timing, mut fill: impl FnMut(T) -> T + Send + 'static) -> Follow<Followed<T>> {
    let client = client.clone();
    spawn(name, move |latest, stop| {
        let mut backoff = Backoff::new(timing);
        let mut quiet = Quiet::default();
        while !stop.is_stopped() {
            let streams = client.streams();
            let path = if streams { events.clone() } else { once.clone() };
            let mut stream = match client.open(&Request::get(path), &|closer| stop.hold(closer)) {
                Ok(stream) => stream,
                Err(error) => {
                    stop.release();
                    if stop.is_stopped() {
                        return;
                    }
                    if quiet.takes(&error) {
                        continue;
                    }
                    latest.put(Followed::Failed(error));
                    if backoff.wait(&stop) {
                        return;
                    }
                    continue;
                }
            };
            if !(200..300).contains(&stream.status) {
                let refusal = refusal(&mut stream);
                stop.release();
                latest.put(Followed::Failed(Error::Refused(refusal)));
                if backoff.wait(&stop) {
                    return;
                }
                continue;
            }
            let mut again_now = false;
            loop {
                match lines::next_line(&mut *stream.reader) {
                    Ok(Some(text)) => match serde_json::from_str::<T>(&text) {
                        Ok(now) => {
                            backoff.reset();
                            quiet.came();
                            latest.put(Followed::Now(fill(now)));
                        }
                        Err(error) => latest.put(Followed::Failed(Error::BadAnswer(format!("What the box said couldn't be read: {error}")))),
                    },
                    Ok(None) => break,
                    Err(error) => {
                        let error = Error::Unreachable(format!("It stopped coming ({}).", error.kind()));
                        if !stop.is_stopped() {
                            again_now = quiet.takes(&error);
                            if !again_now {
                                latest.put(Followed::Failed(error));
                            }
                        }
                        break;
                    }
                }
            }
            stop.release();
            if again_now {
                continue;
            }
            let stopped = if streams { backoff.wait(&stop) } else { stop.wait(timing.conversation_again) };
            if stopped {
                return;
            }
        }
    })
}
