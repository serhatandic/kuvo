//! The one line, as a client follows a conversation on it (PROTOCOL.md, "The line"): its live turn
//! keeps arriving across a box restart and while the box answers slowly, a reconnect gets what it
//! missed, and only the line's silence says the box is gone. The box is a stand-in that answers
//! `kuvo/line` as `box/src/link/server/line.ts` does (numbered, resumable, fresh after a restart).

use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use kuvo_box::follow::{ConversationUpdate, ListUpdate, Timing};
use kuvo_box::line::LineTiming;
use kuvo_box::messages::{Content, Message};
use kuvo_box::transport::{Closer, Request, Response, Stream, Transport};
use kuvo_box::{BoxClient, Error};
use serde_json::{Value, json};

const SESSION: &str = "ses_1";
const EVENTS: &str = "api/event";

#[derive(Default)]
struct State {
    /// The line the box keeps, what it sent on it (numbered), and the topics' names by route.
    line: String,
    sent: Vec<(String, Value)>,
    names: HashMap<String, String>,
    live: Option<Sender<Vec<u8>>>,
    /// Each `kuvo/line` request's path, in order.
    opened: Vec<String>,
    /// The conversation as its engine keeps it, oldest first, and how often it was read.
    messages: Vec<Value>,
    reads: usize,
    /// How long each request takes to answer (a box under heavy load).
    slow: Duration,
    /// Off: nothing answers.
    down: bool,
    /// Still on the release before the line: `kuvo/line` isn't its route, so its engine answers it,
    /// `404` with nothing said.
    before_the_line: bool,
    restarts: usize,
    session_info: Option<(u16, Value)>,
}

#[derive(Clone, Default)]
struct StandIn(Arc<Mutex<State>>);

impl StandIn {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }

    /// What the box pushes for `route`: kept to send again, and sent now if the line is open.
    fn push(&self, route: &str, data: Value) {
        let mut state = self.state();
        state.sent.push((route.to_owned(), data.clone()));
        let n = state.sent.len();
        if let (Some(live), Some(name)) = (&state.live, state.names.get(route)) {
            let _ = live.send(format!("{}\n", json!({ "t": name, "n": n, "d": data })).into_bytes());
        }
    }

    /// The connection drops; the box goes on.
    fn drop_stream(&self) {
        self.state().live = None;
    }

    /// The box starts again: its line, and what it kept to send again, are gone.
    fn restart(&self) {
        let mut state = self.state();
        state.live = None;
        state.sent.clear();
        state.restarts += 1;
        state.line = format!("line-{}", state.restarts);
    }

    fn opened(&self) -> Vec<String> {
        self.state().opened.clone()
    }
}

struct Pipe(Receiver<Vec<u8>>, Vec<u8>);

impl Read for Pipe {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.1.is_empty() {
            match self.0.recv() {
                Ok(bytes) => self.1 = bytes,
                Err(_) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.1.len());
        buffer[..count].copy_from_slice(&self.1[..count]);
        self.1.drain(..count);
        Ok(count)
    }
}

fn query<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    path.split_once('?')?.1.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

impl Transport for StandIn {
    fn send(&self, request: &Request) -> kuvo_box::Result<Response> {
        let slow = self.state().slow;
        std::thread::sleep(slow);
        let mut state = self.state();
        if state.down {
            return Err(Error::Unreachable("It may be asleep or offline.".into()));
        }
        let json = |status: u16, value: Value| Ok(Response { status, content_type: Some("application/json".into()), body: value.to_string().into_bytes() });
        match (request.method.as_str(), request.route()) {
            ("GET", "kuvo/info") if state.session_info.is_some() => {
                let (status, value) = state.session_info.as_ref().unwrap();
                json(*status, value.clone())
            }
            ("GET", "kuvo/sessions/ses_1/location") => json(200, json!({ "place": "engine", "running": true })),
            ("GET", "api/session/active") => json(200, json!({ "data": { (SESSION): {} } })),
            ("GET", "api/session/ses_1/message") => {
                state.reads += 1;
                json(200, json!({ "data": state.messages.iter().rev().cloned().collect::<Vec<_>>() }))
            }
            ("POST", "kuvo/line/watch") => {
                let body = request.body.clone().unwrap_or_default();
                if body["line"].as_str() != Some(state.line.as_str()) {
                    return json(404, json!({ "fresh": true, "message": "Open the line again." }));
                }
                state.names = body["watch"].as_object().into_iter().flatten().filter_map(|(name, route)| Some((route.as_str()?.to_owned(), name.clone()))).collect();
                json(200, json!({ "line": state.line }))
            }
            _ => json(404, json!({})),
        }
    }

    fn open(&self, request: &Request, hold: &dyn Fn(Closer) -> bool) -> kuvo_box::Result<Stream> {
        let mut state = self.state();
        state.opened.push(request.path.clone());
        if state.down || request.route() != "kuvo/line" {
            return Err(Error::Unreachable("It may be asleep or offline.".into()));
        }
        if state.before_the_line {
            return Ok(Stream { status: 404, content_type: Some("application/json".into()), reader: Box::new(std::io::Cursor::new(b"{}".to_vec())), closer: Closer::nothing() });
        }
        let watch: Value = query(&request.path, "watch").and_then(|given| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(given).ok()).and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        state.names = watch.as_object().into_iter().flatten().filter_map(|(name, route)| Some((route.as_str()?.to_owned(), name.clone()))).collect();
        if state.line.is_empty() {
            state.line = "line-0".into();
        }
        let after: Option<usize> = query(&request.path, "after").and_then(|after| after.parse().ok());
        let resumed = query(&request.path, "line") == Some(state.line.as_str()) && after.is_some_and(|after| after <= state.sent.len());
        let (live, receiver) = channel();
        let _ = live.send(format!("{}\n", json!({ "hello": { "line": state.line, "fresh": !resumed, "beat": 5000 } })).into_bytes());
        // What it missed, as the box sends it again.
        if resumed {
            for (n, (route, data)) in state.sent.iter().enumerate().skip(after.unwrap_or(0)) {
                if let Some(name) = state.names.get(route) {
                    let _ = live.send(format!("{}\n", json!({ "t": name, "n": n + 1, "d": data })).into_bytes());
                }
            }
        }
        let local = "kuvo/sessions/list/events";
        let merged = "kuvo/setup/sessions/list/events";
        if let Some(name) = state.names.get(merged).or_else(|| state.names.get(local)) {
            let mut sessions = json!({ "ses_here": { "kind": "chat", "directory": "/chats", "created": 1, "updated": 1 } });
            if state.names.contains_key(merged) {
                sessions["ses_peer"] = json!({ "kind": "chat", "directory": "/chats", "created": 2, "updated": 2,
                    "on": { "id": "peer", "name": "Another Mac", "away": false, "actions": ["open"] } });
            }
            let _ = live.send(format!("{}\n", json!({ "t": name, "d": {
                "at": state.line, "full": true, "archivedGroups": [], "sessions": sessions,
            } })).into_bytes());
        }
        for (route, name) in &state.names {
            let local = route.starts_with("kuvo/library/events?");
            let merged = route.starts_with("kuvo/setup/library/events?");
            if local || merged {
                let computer = if merged { "Mac mini" } else { "MacBook" };
                let _ = live.send(format!("{}\n", json!({ "t": name, "d": {
                    "items": [], "computers": [{ "key": "mini", "name": computer, "state": "there" }],
                } })).into_bytes());
            }
        }
        for (route, name) in &state.names {
            let merged = route == "kuvo/setup/outputs/events?session=ses_root";
            if merged || route == "kuvo/artifacts/events?session=ses_root" {
                let artifacts = if merged { json!([{
                    "id": "art_peer", "name": "report.md", "path": "/reports/report.md", "kind": "document",
                    "type": "text/markdown", "latest": {"n": 1}, "computer": {"id": "mini", "name": "Mac mini"}
                }]) } else { json!([]) };
                let _ = live.send(format!("{}\n", json!({ "t": name, "d": { "artifacts": artifacts } })).into_bytes());
            }
        }
        if let Some(name) = state.names.get("kuvo/setup/notices/events") {
            let _ = live.send(format!("{}\n", json!({ "t": name, "d": {
                "origins": [{"computer": {"id": "mini", "name": "Mac mini"}, "telling": true, "away": false}],
                "notices": [{"id": "notice_1", "at": 15, "computer": {"id": "mini", "name": "Mac mini"},
                    "notice": {"category": "finished"}}]
            } })).into_bytes());
        }
        let local_waiting = "kuvo/waiting/events";
        let setup_waiting = "kuvo/setup/waiting/counts/events";
        if let Some(name) = state.names.get(setup_waiting).or_else(|| state.names.get(local_waiting)) {
            let sessions = usize::from(state.names.contains_key(setup_waiting));
            let _ = live.send(format!("{}\n", json!({ "t": name, "d": { "sessions": sessions, "bots": 0, "queue": 0 } })).into_bytes());
        }
        state.live = Some(live);
        hold(Closer::nothing());
        Ok(Stream { status: 200, content_type: Some("application/x-ndjson".into()), reader: Box::new(BufReader::new(Pipe(receiver, Vec::new()))), closer: Closer::nothing() })
    }

    fn streams(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        "Mac mini".into()
    }
}

#[test]
fn a_session_list_picks_up_peer_sessions_after_a_failed_first_box_info_read() {
    session_list_picks_up_peer_sessions_after_reconnect(503, json!({ "message": "The box is starting." }));
}

#[test]
fn a_session_list_picks_up_peer_sessions_after_the_box_updates() {
    session_list_picks_up_peer_sessions_after_reconnect(200, json!({ "kind": "mac", "name": "Mac mini", "features": [] }));
}

fn session_list_picks_up_peer_sessions_after_reconnect(status: u16, initial_info: Value) {
    let box_ = StandIn::default();
    box_.state().session_info = Some((status, initial_info));
    let client = BoxClient::new(box_.clone());
    let follow = client.follow_sessions(at_once());
    let ListUpdate::Listing(first) = follow.next_blocking().unwrap() else { panic!("the local list") };
    assert_eq!(first.sidebar().chats, ["ses_here"]);

    // The next connection can read the capabilities of the box that now answers.
    box_.state().session_info = Some((200, json!({ "kind": "mac", "name": "Mac mini", "features": ["line.merged-sessions"] })));
    box_.restart();
    let ListUpdate::Listing(next) = follow.next_blocking().unwrap() else { panic!("the list after reconnect") };
    assert_eq!(next.sidebar().chats, ["ses_peer", "ses_here"]);
    assert_eq!(next.computer_of("ses_peer").unwrap().name, "Another Mac");
    follow.stop();
    box_.drop_stream();
}

fn at_once() -> Timing {
    Timing { retry_after: Duration::ZERO, most: Duration::ZERO, ..Timing::default() }
}

fn event(kind: &str, data: Value) -> Value {
    let mut data = data;
    data["sessionID"] = json!(SESSION);
    json!({ "id": "evt", "type": kind, "created": 5, "data": data })
}

fn user(id: &str, text: &str, at: f64) -> Value {
    json!({ "type": "user", "id": id, "time": { "created": at }, "text": text })
}

fn assistant(id: &str, text: &str, at: f64) -> Value {
    json!({ "type": "assistant", "id": id, "time": { "created": at, "completed": at + 1.0 }, "agent": "build", "model": { "providerID": "p", "id": "m" }, "content": [{ "type": "text", "text": text }], "finish": "stop" })
}

fn words(update: &ConversationUpdate) -> Option<(String, String)> {
    let ConversationUpdate::Conversation(conversation) = update else { return None };
    match conversation.messages.last()? {
        Message::Assistant(step) => Some((step.id.clone(), step.content.iter().filter_map(|content| if let Content::Text(text) = content { Some(text.as_str()) } else { None }).collect())),
        _ => None,
    }
}

/// Waits for the conversation to show step `id` saying `said`.
fn shows(follow: &kuvo_box::Follow<ConversationUpdate>, id: &str, said: &str) -> ConversationUpdate {
    loop {
        let update = follow.next_blocking().expect("the conversation goes on");
        assert!(!matches!(update, ConversationUpdate::Failed(_) | ConversationUpdate::Gone(_)), "nothing fails while the box is only restarting or slow: {update:?}");
        if words(&update).is_some_and(|(step, text)| step == id && text == said) {
            return update;
        }
    }
}

/// A step's words, streamed as the engine streams them; its engine keeps the step mid-reply, its
/// block with none of its words yet, as a read finds it.
fn stream_step(box_: &StandIn, step: &str, words: &[&str]) {
    {
        let mut state = box_.state();
        let at = state.messages.len() as f64 + 1.0;
        state.messages.push(json!({ "type": "assistant", "id": step, "time": { "created": at }, "agent": "build", "model": {}, "content": [{ "type": "text", "text": "" }] }));
    }
    box_.push(EVENTS, event("session.step.started", json!({ "assistantMessageID": step })));
    box_.push(EVENTS, event("session.text.started", json!({ "assistantMessageID": step, "ordinal": 0 })));
    // (`at`: where in the block's words each delta starts.)
    let mut at = 0;
    for word in words {
        box_.push(EVENTS, event("session.text.delta", json!({ "assistantMessageID": step, "ordinal": 0, "delta": word, "at": at })));
        at += word.chars().count();
    }
}

fn started(box_: &StandIn) -> (BoxClient, kuvo_box::Follow<ConversationUpdate>) {
    let client = BoxClient::new(box_.clone());
    let follow = client.follow_conversation(SESSION, at_once());
    // Its first read, then its events on the line.
    loop {
        if let ConversationUpdate::Conversation(shown) = follow.next_blocking().unwrap()
            && !shown.messages.is_empty()
        {
            break;
        }
    }
    while !box_.state().names.contains_key(EVENTS) {
        std::thread::yield_now();
    }
    (client, follow)
}

#[test]
fn a_conversations_live_turn_keeps_arriving_across_a_box_restart_and_a_slow_box() {
    let box_ = StandIn::default();
    box_.state().messages = vec![user("msg_1", "Run the tests", 1.0)];
    let (_client, follow) = started(&box_);
    // The box is under heavy load: every request takes a while. What it pushes still comes.
    box_.state().slow = Duration::from_millis(150);
    stream_step(&box_, "msg_2", &["Run", "ning"]);
    shows(&follow, "msg_2", "Running");

    // It starts again (an update) while the turn goes on: what it did meanwhile is read once its
    // line is back, and the turn keeps streaming after.
    box_.restart();
    box_.state().messages = vec![user("msg_1", "Run the tests", 1.0), assistant("msg_2", "Running them now.", 2.0), user("msg_3", "Are you stuck?", 4.0)];
    loop {
        let update = follow.next_blocking().unwrap();
        assert!(!matches!(update, ConversationUpdate::Failed(_)), "a restart isn't said as a failure: {update:?}");
        if let ConversationUpdate::Conversation(shown) = &update
            && shown.messages.last().is_some_and(|message| message.id() == "msg_3")
        {
            break;
        }
    }
    while !box_.state().names.contains_key(EVENTS) || box_.state().live.is_none() {
        std::thread::yield_now();
    }
    stream_step(&box_, "msg_4", &["No, ", "done."]);
    shows(&follow, "msg_4", "No, done.");
}

#[test]
fn a_reconnect_gets_what_was_missed_from_where_it_was() {
    let box_ = StandIn::default();
    box_.state().messages = vec![user("msg_1", "Hi", 1.0)];
    let (_client, follow) = started(&box_);
    stream_step(&box_, "msg_2", &["A"]);
    shows(&follow, "msg_2", "A");
    let heard = box_.state().sent.len();
    let reads = box_.state().reads;

    // The connection drops; the box goes on, and its line keeps what it sends.
    box_.drop_stream();
    box_.push(EVENTS, event("session.text.delta", json!({ "assistantMessageID": "msg_2", "ordinal": 0, "delta": "B", "at": 1 })));
    box_.push(EVENTS, event("session.text.delta", json!({ "assistantMessageID": "msg_2", "ordinal": 0, "delta": "C", "at": 2 })));
    shows(&follow, "msg_2", "ABC");
    // Opened again from where it was, and given what it missed: nothing read again for it.
    let opened = box_.opened();
    assert!(opened.last().unwrap().contains(&format!("&line=line-0&after={heard}")), "{opened:?}");
    assert_eq!(box_.state().reads, reads);
}

#[test]
fn only_the_lines_silence_says_the_box_is_gone() {
    let box_ = StandIn::default();
    box_.state().messages = vec![user("msg_1", "Hi", 1.0)];
    let client = BoxClient::new(box_.clone());
    let line = client.line();
    line.set_timing(LineTiming { quiet: Duration::from_millis(40), retry_after: Duration::from_millis(5), most: Duration::from_millis(5), ..LineTiming::default() });
    let (_client, _follow) = (client.clone(), client.follow_conversation(SESSION, at_once()));
    while !line.heard_within(Duration::from_secs(3600)) {
        std::thread::yield_now();
    }
    assert!(!line.silent_for(Duration::from_secs(3600)), "heard: not gone");
    // Gone for real: nothing comes on its line, and it can't be opened again.
    box_.state().down = true;
    box_.drop_stream();
    while !line.silent_for(Duration::from_millis(100)) {
        std::thread::yield_now();
    }
    assert!(!line.heard_within(Duration::from_millis(100)));
}

#[test]
fn a_computer_still_on_the_release_before_the_line_is_waited_for_never_said_to_need_a_newer_kuvo() {
    // (The update to beta.44: the app was new, its computer's Kuvo still the one before, mid-switch.)
    let box_ = StandIn::default();
    box_.state().before_the_line = true;
    let client = BoxClient::new(box_.clone());
    client.line().set_timing(LineTiming { retry_after: Duration::from_millis(5), most: Duration::from_millis(5), ..LineTiming::default() });
    let follow = client.follow_computers();
    // It's asked, and turned down, more than once while it switches.
    while box_.opened().iter().filter(|path| path.starts_with("kuvo/line")).count() < 3 {
        std::thread::yield_now();
    }
    // It's on the new release now: the list comes, and it's the first thing the view hears.
    box_.state().before_the_line = false;
    while !box_.state().names.contains_key("kuvo/computers/events") || box_.state().live.is_none() {
        std::thread::yield_now();
    }
    box_.push("kuvo/computers/events", json!({ "computers": [{ "key": "mini", "name": "Mac mini", "reach": "connected" }] }));
    match follow.next_blocking().expect("the list comes") {
        kuvo_box::follow::Followed::Now(list) => assert_eq!(list.computers.iter().map(|computer| computer.name.as_str()).collect::<Vec<_>>(), ["Mac mini"]),
        kuvo_box::follow::Followed::Failed(error) => panic!("a computer that's updating is never an error: {error}"),
    }
}

#[test]
fn library_follows_a_filtered_setup_topic_on_every_connection() {
    let box_ = StandIn::default();
    let client = BoxClient::new(box_.clone());
    client.line().set_timing(LineTiming { retry_after: Duration::ZERO, ..LineTiming::default() });
    let follow = client.follow_library(&kuvo_box::library::LibraryQuery {
        source: Some("bot_ada".into()),
        kind: Some(kuvo_box::library::LibraryKind::Document),
        words: "board brief".into(),
    });
    for _ in 0..2 {
        match follow.next_blocking().expect("the Library comes") {
            kuvo_box::follow::Followed::Now(list) => assert_eq!(list.computers[0].name, "Mac mini"),
            kuvo_box::follow::Followed::Failed(error) => panic!("{error}"),
        }
        box_.restart();
    }
    let expected = "kuvo/setup/library/events?source=bot_ada&kind=document&q=board%20brief";
    for path in box_.opened().iter().take(2) {
        let watch = query(path, "watch").expect("the line takes its topics");
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(watch).unwrap();
        let watch: HashMap<String, String> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(watch.values().map(String::as_str).collect::<Vec<_>>(), [expected]);
    }
}

#[test]
fn setup_waiting_counts_include_a_peers_ask_on_first_draw() {
    let client = BoxClient::new(StandIn::default());
    let follow = client.follow_waiting();
    match follow.next_blocking().expect("the waiting count comes") {
        kuvo_box::follow::Followed::Now(waiting) => assert_eq!(waiting.sessions, 1),
        kuvo_box::follow::Followed::Failed(error) => panic!("{error}"),
    }
}

#[test]
fn activity_outputs_follow_the_setup_topic_and_keep_the_owner() {
    let client = BoxClient::new(StandIn::default());
    let follow = client.follow_session_outputs("ses_root");
    match follow.next_blocking().expect("the outputs come") {
        kuvo_box::follow::Followed::Now(outputs) => {
            assert_eq!(outputs.artifacts.len(), 1);
            assert_eq!(outputs.artifacts[0].computer.as_ref().unwrap().id, "mini");
        }
        kuvo_box::follow::Followed::Failed(error) => panic!("{error}"),
    }
}

#[test]
fn notifications_follow_one_setup_history_with_stable_origins() {
    let box_ = StandIn::default();
    let client = BoxClient::new(box_.clone());
    client.line().set_timing(LineTiming { retry_after: Duration::ZERO, ..LineTiming::default() });
    let follow = client.follow_setup_notices();
    for _ in 0..2 {
        match follow.next_blocking().expect("the notice history comes") {
            kuvo_box::follow::Followed::Now(history) => {
                assert_eq!(history.origins[0].computer.id, "mini");
                assert_eq!(history.notices[0].computer.id, "mini");
                assert_eq!(history.notices[0].told.id, "notice_1");
            }
            kuvo_box::follow::Followed::Failed(error) => panic!("{error}"),
        }
        box_.restart();
    }
}

/// The real resident opens, resumes, and changes a line through this client's recorded HTTP
/// transport. Area fixtures above continue to exercise conversation and merged-list behavior.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
#[ignore = "set KUVO_BOX_BIN to a built Rust kuvo-box"]
fn the_desktop_reads_and_resumes_a_real_residents_line() {
    use std::io::BufRead;
    use std::process::{Child, Command, Stdio};
    use kuvo_box::local::{LocalServer, Speaker};

    struct Resident(Child);
    impl Drop for Resident {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let binary = std::env::var_os("KUVO_BOX_BIN").expect("path to kuvo-box");
    let home = tempfile::tempdir().unwrap();
    let mut server = Resident(Command::new(binary).arg("serve").env_clear()
        .env("KUVO_HOME", home.path()).stdin(Stdio::null()).stdout(Stdio::piped()).spawn().unwrap());
    let (sent, ready) = channel();
    let mut stdout = BufReader::new(server.0.stdout.take().unwrap());
    std::thread::spawn(move || {
        let mut announcement = String::new();
        stdout.read_line(&mut announcement).unwrap();
        sent.send(announcement).unwrap();
    });
    let ready: Value = serde_json::from_str(&ready.recv_timeout(Duration::from_secs(10)).unwrap()).unwrap();
    assert!(ready["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));

    let local = LocalServer::recorded(home.path(), Speaker::Mac, "fixture");
    let topics = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"info":"kuvo/info"}"#);
    let mut stream = local.open(&Request::get(format!("kuvo/line?watch={topics}")), &|_| true).unwrap();
    let read = |stream: &mut Stream| -> Value {
        let mut line = String::new();
        stream.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let hello = read(&mut stream);
    assert_eq!(hello["hello"]["fresh"], true);
    let id = hello["hello"]["line"].as_str().unwrap();
    let info = read(&mut stream);
    assert_eq!((info["t"].as_str(), info["n"].as_u64()), (Some("info"), Some(1)));
    assert_eq!(info["d"]["features"], json!(["info", "line"]));
    drop(stream);
    let mut resumed = local.open(&Request::get(format!("kuvo/line?watch={topics}&line={id}&after=0")), &|_| true).unwrap();
    assert_eq!(read(&mut resumed)["hello"], json!({"fresh":false,"line":id,"beat":5000}));
    assert_eq!(read(&mut resumed), info);
    let reply = local.send(&Request::post("kuvo/line/watch", json!({"line":id,"watch":{"missing":"kuvo/not-installed"}}))).unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(read(&mut resumed), json!({"n":2,"t":"missing","refused":{"status":404,"message":"{}","said":{}}}));
    drop(resumed);

    let client = BoxClient::new(local);
    let waiting = client.follow_waiting();
    let updates = waiting.updates().clone();
    let (send, update) = channel();
    std::thread::spawn(move || { let _ = send.send(updates.recv_blocking()); });
    assert!(matches!(
        update.recv_timeout(Duration::from_secs(10)).unwrap().unwrap(),
        kuvo_box::follow::Followed::Failed(Error::Refused(refusal)) if refusal.status == 404
    ));
    assert!(client.line().heard_within(Duration::from_secs(10)));
}
