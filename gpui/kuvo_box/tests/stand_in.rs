//! Scenarios against a stand-in box on this computer: finding it by its record, the update gate,
//! the session lists and a conversation followed live, and sending what a composer sends.

mod support;

use std::time::Duration;

use kuvo_box::follow::{ConversationUpdate, ListUpdate, Timing};
use kuvo_box::messages::{Content, Message};
use kuvo_box::{BoxClient, Delivery, Draft, Error, Gate, Sent, Where};
use serde_json::{Value, json};
use support::{Answer, ServeProcess, StandInBox, app_box, current_features, home, mac_info, write_record};

/// No waiting between tries: each one follows from what the stand-in does.
fn at_once() -> Timing {
    Timing { retry_after: Duration::ZERO, most: Duration::ZERO, ..Timing::default() }
}

/// This Mac's box, as the app's keeper leaves it: its record in the app's folder, naming a process
/// that runs as its server.
fn this_mac(stand_in: &StandInBox) -> (tempfile::TempDir, ServeProcess, BoxClient) {
    let home = home();
    let serve = ServeProcess::start();
    write_record(&app_box(home.path()), serve.pid(), stand_in.port, &stand_in.password);
    let client = BoxClient::this_mac(home.path()).expect("this Mac's box is found");
    (home, serve, client)
}

fn session(kind: &str, directory: &str, updated: f64) -> Value {
    json!({ "kind": kind, "project": directory, "directory": directory, "created": updated - 1.0, "updated": updated })
}

#[test]
fn this_macs_box_is_found_by_its_record_and_lists_its_sessions() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        // (Held a moment first, as a box holds a request: the interim answer is skipped.)
        "kuvo/info" => Answer::Whole { status: 200, body: mac_info(current_features()), interim: true },
        "kuvo/sessions/list" => Answer::json(200, json!({
            "at": "7",
            "full": true,
            "sessions": { "ses_a": session("project", "/home/me/web", 10.0), "ses_b": session("project", "/home/me/api", 20.0), "ses_sub": { "kind": "subagent", "directory": "/home/me/web", "created": 1, "updated": 30, "parent": "ses_a" } },
            "projects": { "/home/me/web": { "name": "Website", "updated": 10 }, "/home/me/api": { "updated": 20 } }
        })),
        _ => Answer::json(404, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);

    let info = client.connect().expect("kuvo/info");
    assert_eq!(info.name, "Stand-in Mac");
    assert!(info.is_mac());
    assert_eq!(client.gate(), Gate::Open);

    let line = client.session_list(None).expect("the list");
    let mut listing = kuvo_box::Listing::new();
    listing.take(line);
    let sidebar = listing.sidebar();
    let projects: Vec<(&str, &str)> = sidebar.projects.iter().map(|group| (group.directory.as_str(), group.name.as_str())).collect();
    assert_eq!(projects, vec![("/home/me/api", "api"), ("/home/me/web", "Website")]);
    assert_eq!(sidebar.projects[1].sessions, vec!["ses_a"]);

    // As the app is to its own box: its password, and who's asking.
    let seen = stand_in.seen();
    assert_eq!(seen.iter().map(|seen| seen.route()).collect::<Vec<_>>(), vec!["kuvo/info", "kuvo/sessions/list"]);
    assert!(seen.iter().all(|seen| seen.headers.get("x-kuvo-from").map(String::as_str) == Some("app")));
}

#[test]
fn a_record_naming_another_process_is_never_sent_the_password() {
    let stand_in = StandInBox::start(|_| Answer::json(200, mac_info(current_features())));
    let home = home();
    // A record a crash left: its pid is now some other process (this test's own).
    write_record(&app_box(home.path()), std::process::id(), stand_in.port, &stand_in.password);
    match BoxClient::this_mac(home.path()) {
        Err(Error::NotRunning(_)) => {}
        Err(other) => panic!("not running, not {other:?}"),
        Ok(_) => panic!("a record naming another process was taken"),
    }
    // No record at all, the same.
    let empty = support::home();
    std::fs::create_dir_all(app_box(empty.path())).unwrap();
    assert!(matches!(BoxClient::this_mac(empty.path()), Err(Error::NotRunning(_))));
    assert!(stand_in.seen().is_empty(), "nothing reached the port");
}

#[test]
fn this_macs_box_in_a_home_of_its_own_is_the_one_used() {
    let moved = StandInBox::start(|_| Answer::json(200, mac_info(current_features())));
    let old = StandInBox::start(|_| Answer::json(200, mac_info(current_features())));
    let home = home();
    let serve = ServeProcess::start();
    // The app's folder still has a record; the box moved out and says it's this Mac's.
    write_record(&app_box(home.path()), serve.pid(), old.port, &old.password);
    let own = home.path().join(".kuvo-code");
    std::fs::create_dir_all(own.join("box")).unwrap();
    std::fs::write(own.join("box/kind"), "mac\n").unwrap();
    write_record(&own, serve.pid(), moved.port, &moved.password);

    BoxClient::this_mac(home.path()).unwrap().connect().unwrap();
    assert_eq!(moved.seen().len(), 1);
    assert!(old.seen().is_empty());

    // A machine's box in that home isn't this Mac's own.
    std::fs::write(own.join("box/kind"), "machine\n").unwrap();
    BoxClient::this_mac(home.path()).unwrap().connect().unwrap();
    assert_eq!(old.seen().len(), 1);
}

#[test]
fn a_box_older_than_this_client_is_held_until_it_updates() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(json!(["sessions.list", "git.status"]))),
        _ => Answer::json(200, json!({ "data": [] })),
    });
    let (_home, _serve, client) = this_mac(&stand_in);

    // Its first request reads what it has first.
    match client.session_list(None) {
        Err(Error::NeedsUpdating { missing }) => assert!(missing.contains(&"typed".to_owned()) && !missing.contains(&"sessions.list".to_owned())),
        other => panic!("held, not {other:?}"),
    }
    assert!(client.gate().is_held());
    // What keeps work under way going still goes; the rest never reaches the box.
    assert!(client.messages("ses_1", Where::Engine, 10).is_ok());
    assert!(matches!(client.typed_offers("ses_1"), Err(Error::NeedsUpdating { .. })));
    assert_eq!(stand_in.seen_routes(), vec!["GET kuvo/info", "GET api/session/ses_1/message?limit=10&order=desc"]);

    // Updated, it's let go once it's read again.
    stand_in.answer(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        _ => Answer::json(200, json!({ "keywords": [], "commands": [] })),
    });
    client.connect().unwrap();
    assert_eq!(client.gate(), Gate::Open);
    assert!(client.typed_offers("ses_1").is_ok());

    // One that answers no kuvo/info at all lacks everything.
    stand_in.answer(|_| Answer::json(404, json!({})));
    assert!(matches!(client.connect(), Err(Error::NeedsUpdating { missing }) if missing.len() == kuvo_box::gate::REQUIRED.len()));
}

#[test]
fn the_session_list_follows_the_box_and_resumes_where_it_was() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        _ => Answer::json(500, json!({})),
    });
    let first = stand_in.stream_for("kuvo/sessions/list/events");
    let second = stand_in.stream_for("kuvo/sessions/list/events");
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_sessions(at_once());

    let projects = json!({ "/home/me/web": { "updated": 20 } });
    first.send(json!({ "at": "1", "full": true, "sessions": { "ses_a": session("project", "/home/me/web", 10.0), "ses_b": session("project", "/home/me/web", 20.0) }, "projects": projects }).to_string()).unwrap();
    let ListUpdate::Listing(listing) = follow.next_blocking().unwrap() else { panic!("the list") };
    assert_eq!(listing.sidebar().projects[0].sessions, vec!["ses_b", "ses_a"]);

    // A change: one renamed and moved to the top, one deleted.
    let mut renamed = session("project", "/home/me/web", 30.0);
    renamed["title"] = json!("Renamed");
    first.send(json!({ "at": "2", "sessions": { "ses_a": renamed }, "gone": ["ses_b"], "projects": projects }).to_string()).unwrap();
    let ListUpdate::Listing(listing) = follow.next_blocking().unwrap() else { panic!("the change") };
    assert_eq!(listing.sidebar().projects[0].sessions, vec!["ses_a"]);
    assert_eq!(listing.facts("ses_a").unwrap().title.as_deref(), Some("Renamed"));

    // The stream ends: it's opened again from where it was, and goes on.
    drop(first);
    second.send(json!({ "at": "3", "sessions": { "ses_c": { "kind": "chat", "directory": "/home/me/.kuvo-code/chats", "created": 1, "updated": 40 } }, "projects": projects }).to_string()).unwrap();
    let ListUpdate::Listing(listing) = follow.next_blocking().unwrap() else { panic!("the list again") };
    assert_eq!(listing.sidebar().chats, vec!["ses_c"]);
    assert_eq!(listing.sidebar().projects[0].sessions, vec!["ses_a"]);
    let opened: Vec<String> = stand_in.seen().iter().filter(|seen| seen.route() == "kuvo/sessions/list/events").map(|seen| seen.path.clone()).collect();
    assert_eq!(opened, vec!["kuvo/sessions/list/events", "kuvo/sessions/list/events?since=2"]);

    // Stopped, it says nothing more and closes what it had open.
    follow.stop();
    drop(second);
    assert!(follow.next_blocking().is_none());
}

#[test]
fn a_list_the_box_cant_read_yet_waits() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        _ => Answer::json(503, json!({ "message": "The engine isn't answering yet." })),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_sessions(at_once());
    assert_eq!(follow.next_blocking(), Some(ListUpdate::Waiting("The engine isn't answering yet.".into())));
}

fn user(id: &str, text: &str, at: f64) -> Value {
    json!({ "type": "user", "id": id, "time": { "created": at }, "text": text })
}

fn assistant(id: &str, text: &str, at: f64) -> Value {
    json!({ "type": "assistant", "id": id, "time": { "created": at, "completed": at + 1.0 }, "agent": "build", "model": { "providerID": "p", "id": "m" }, "content": [{ "type": "text", "text": text }], "finish": "stop" })
}

fn event(kind: &str, session: &str, data: Value) -> String {
    let mut data = data;
    data["sessionID"] = json!(session);
    json!({ "id": "evt", "type": kind, "created": 5, "data": data }).to_string()
}

/// The words of the conversation's last step, if it's a step.
fn last_words(update: &ConversationUpdate) -> Option<String> {
    let ConversationUpdate::Conversation(conversation) = update else { return None };
    match conversation.messages.last()? {
        Message::Assistant(step) => Some(step.content.iter().filter_map(|content| if let Content::Text(text) = content { Some(text.as_str()) } else { None }).collect()),
        _ => None,
    }
}

#[test]
fn a_conversation_streams_its_words_and_is_read_again_as_steps_end() {
    let stored = std::sync::Arc::new(std::sync::Mutex::new(json!([user("msg_1", "Hi", 1.0)])));
    let messages = stored.clone();
    let stand_in = StandInBox::start(move |seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        "api/session/ses_1" => Answer::json(200, json!({ "data": { "id": "ses_1" } })),
        // Newest first, as asked.
        "api/session/ses_1/message" => {
            let mut newest: Vec<Value> = messages.lock().unwrap().as_array().unwrap().clone();
            newest.reverse();
            Answer::json(200, json!({ "data": newest, "cursor": null }))
        }
        _ => Answer::json(404, json!({})),
    });
    let events = stand_in.stream_for("api/event");
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_conversation("ses_1", at_once());

    // Read once it listens.
    let ConversationUpdate::Conversation(read) = follow.next_blocking().unwrap() else { panic!("the messages") };
    assert_eq!(read.place, Where::Engine);
    assert!(matches!(&read.messages[..], [Message::User(user)] if user.text == "Hi"));

    // A step streams its words as they come; another conversation's events change nothing.
    events.send(event("session.step.started", "ses_1", json!({ "assistantMessageID": "msg_2", "agent": "build" }))).unwrap();
    events.send(event("session.text.delta", "ses_other", json!({ "assistantMessageID": "msg_9", "delta": "no" }))).unwrap();
    events.send(event("session.text.started", "ses_1", json!({ "assistantMessageID": "msg_2", "ordinal": 0 }))).unwrap();
    events.send(event("session.text.delta", "ses_1", json!({ "assistantMessageID": "msg_2", "ordinal": 0, "delta": "Hel" }))).unwrap();
    events.send(event("session.text.delta", "ses_1", json!({ "assistantMessageID": "msg_2", "ordinal": 0, "delta": "lo" }))).unwrap();
    let mut words = Vec::new();
    while words.last().map(String::as_str) != Some("Hello") {
        let update = follow.next_blocking().unwrap();
        if let ConversationUpdate::Conversation(conversation) = &update {
            assert!(conversation.running);
        }
        words.extend(last_words(&update));
    }
    // (Only the latest is handed on: some of these may have gone by unseen, never out of order.)
    assert!(words.windows(2).all(|pair| pair[1].starts_with(&pair[0])), "{words:?}");

    // The step ends: what the engine kept is read again, whole.
    *stored.lock().unwrap() = json!([user("msg_1", "Hi", 1.0), assistant("msg_2", "Hello there.", 2.0)]);
    events.send(event("session.step.ended", "ses_1", json!({ "assistantMessageID": "msg_2", "finish": "stop" }))).unwrap();
    loop {
        let update = follow.next_blocking().unwrap();
        if last_words(&update).as_deref() == Some("Hello there.") {
            break;
        }
    }
    // A turn's end is the end of the run.
    events.send(event("session.execution.succeeded", "ses_1", json!({}))).unwrap();
    loop {
        if let ConversationUpdate::Conversation(conversation) = follow.next_blocking().unwrap()
            && !conversation.running
        {
            break;
        }
    }
    drop(follow);
}

#[test]
fn a_coding_agents_conversation_is_read_from_its_bridge() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        "api/session/ses_cc" => Answer::json(404, json!({})),
        "kuvo/agents/api/session/ses_cc" => Answer::json(200, json!({ "data": { "id": "ses_cc" } })),
        "kuvo/agents/api/session/ses_cc/message" => Answer::json(200, json!({ "data": [assistant("msg_2", "From Claude Code.", 2.0), user("msg_1", "Hi", 1.0)] })),
        "api/session/ses_none" | "kuvo/agents/api/session/ses_none" => Answer::json(404, json!({})),
        _ => Answer::json(500, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    let place = client.where_is("ses_cc").unwrap();
    assert_eq!(place, Where::Agents);
    let messages = client.messages("ses_cc", place, 50).unwrap();
    assert_eq!(messages.iter().map(Message::id).collect::<Vec<_>>(), vec!["msg_1", "msg_2"]);
    assert_eq!(client.where_is("ses_none").unwrap_err().status(), Some(404));
}

#[test]
fn a_composer_sends_its_fields_and_asks_the_box_what_a_typed_text_means() {
    use kuvo_box::parts::{Comment, File, Notes, Side};
    let stand_in = StandInBox::start(|seen| match (seen.method.as_str(), seen.route()) {
        (_, "kuvo/info") => Answer::json(200, mac_info(current_features())),
        ("POST", "kuvo/sessions/ses_1/send") => Answer::json(200, json!({})),
        ("POST", "kuvo/sessions/ses_1/typed") => match seen.body.as_ref().unwrap()["text"].as_str().unwrap() {
            "/plan sketch it" => Answer::json(200, json!({ "keyword": "plan", "agent": "plan", "send": { "text": "sketch it", "typed": "/plan", "quoted": true } })),
            "/loop every hour check" => Answer::json(200, json!({ "wakeUp": { "due": 1, "every": "1h", "note": "check" }, "send": { "text": "check", "typed": "/loop every hour", "steer": true } })),
            "/init now" => Answer::json(200, json!({ "command": { "name": "init", "arguments": "now" } })),
            "!ls" => Answer::json(200, json!({ "shell": "ls" })),
            _ => Answer::json(503, json!({ "message": "The engine isn't answering.", "restore": { "text": "/review now" } })),
        },
        _ => Answer::json(404, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    let quote = |words: &str| format!("> the line\n\n{words}");

    // A plain message goes to its box as fields, under its id, which the box takes once: the
    // words with what they reply to, the files, the comments, the agents named and the widget.
    let id = BoxClient::new_id();
    assert!(id.starts_with("msg_") && id.len() == 4 + 26);
    let mut draft = Draft::new(id.clone(), "fix the tests");
    draft.files = vec![File { uri: "file:///Users/me/shot.png".into(), name: Some("shot.png".into()) }];
    draft.notes = Notes { paths: vec!["/Users/me/big.zip".into()], ..Notes::default() };
    draft.comments = vec![Comment { file: "a.js".into(), line: 4, side: Side::New, snippet: "x()".into(), text: "Why?".into() }];
    draft.named = vec!["ses_mira".into()];
    draft.widget = Some("Pick one".into());
    assert_eq!(client.send_draft("ses_1", &draft, Some(&quote)).unwrap(), Sent::Message { id: id.clone() });
    let sent = stand_in.seen().pop().unwrap();
    assert_eq!(sent.route(), "kuvo/sessions/ses_1/send");
    assert_eq!(sent.body.unwrap(), json!({
        "id": id, "words": "> the line\n\nfix the tests", "delivery": "steer",
        "files": [{ "uri": "file:///Users/me/shot.png", "name": "shot.png" }],
        "notes": { "paths": ["/Users/me/big.zip"] },
        "comments": [{ "file": "a.js", "line": 4, "side": "new", "snippet": "x()", "text": "Why?" }],
        "named": ["ses_mira"], "widget": "Pick one"
    }));
    assert_eq!(sent.headers.get("idempotency-key"), Some(&id));

    // One for after the run says so; the box puts it in the queue.
    let mut later = Draft::new("msg_q", "then this");
    later.delivery = Delivery::Queue;
    client.send_draft("ses_1", &later, None).unwrap();
    assert_eq!(stand_in.seen().pop().unwrap().body.unwrap(), json!({ "id": "msg_q", "words": "then this", "delivery": "queue" }));

    // With a model that can't run, it's kept, saying why: nothing goes.
    let before = stand_in.seen().len();
    let mut refused = Draft::new("msg_k", "fix it");
    refused.refusal = Some("Sign in to Anthropic again.".into());
    let Sent::Typed { answer, message: None } = client.send_draft("ses_1", &refused, None).unwrap() else { panic!("kept") };
    assert_eq!((answer.error.as_deref(), answer.restore.unwrap().text.as_str()), (Some("Sign in to Anthropic again."), "fix it"));
    assert_eq!(stand_in.seen().len(), before);

    // "/plan sketch it": the box says what it means; the words it says to send go with the keyword
    // as typed and what they reply to, and the box writes what the agent reads.
    let mut plan = Draft::new("msg_p", "/plan sketch it");
    plan.files = vec![File { uri: "data:image/png;base64,AA==".into(), name: None }];
    plan.widget = Some("Pick one".into());
    let Sent::Typed { answer, message } = client.send_draft("ses_1", &plan, Some(&quote)).unwrap() else { panic!("typed") };
    assert_eq!((answer.agent.as_deref(), message.as_deref()), (Some("plan"), Some("msg_p")));
    let seen = stand_in.seen();
    let [.., typed, sent] = &seen[..] else { panic!() };
    assert_eq!(typed.body.as_ref().unwrap(), &json!({ "id": "msg_p", "text": "/plan sketch it", "files": true, "side": false }));
    assert_eq!(sent.body.as_ref().unwrap(), &json!({ "id": "msg_p", "words": "> the line\n\nsketch it", "keyword": "/plan", "delivery": "steer", "files": [{ "uri": "data:image/png;base64,AA==" }], "widget": "Pick one" }));

    // A /loop's first goes now, whatever was asked.
    let mut looped = Draft::new("msg_l", "/loop every hour check");
    looped.delivery = Delivery::Queue;
    client.send_draft("ses_1", &looped, None).unwrap();
    assert_eq!(stand_in.seen().pop().unwrap().body.unwrap(), json!({ "id": "msg_l", "words": "check", "keyword": "/loop every hour", "delivery": "steer" }));

    // One of the engine's commands runs with its arguments as written: not a message, so no id.
    let Sent::Typed { message: None, .. } = client.send_draft("ses_1", &Draft::new("msg_c", "/init now"), None).unwrap() else { panic!("a command") };
    let ran = stand_in.seen().pop().unwrap();
    assert_eq!(ran.body.unwrap(), json!({ "words": "now", "command": "init", "delivery": "steer" }));
    assert_eq!(ran.headers.get("idempotency-key"), None);

    // With the side thread's tag on, a message is a follow-up there, the client's to ask.
    let before = stand_in.seen().len();
    let mut side = Draft::new("msg_t", "what about tests?");
    side.side = true;
    let Sent::Typed { answer, message: None } = client.send_draft("ses_1", &side, None).unwrap() else { panic!("side") };
    assert_eq!(answer.side_thread, Some(true));
    assert_eq!(stand_in.seen().len(), before);

    // Both at once: the model that can't run is said first, as the box checks it first
    // (typed/decide.ts), and the message is kept, not taken to the side thread.
    side.refusal = Some("Sign in to Anthropic again.".into());
    let Sent::Typed { answer, message: None } = client.send_draft("ses_1", &side, None).unwrap() else { panic!("kept") };
    assert_eq!((answer.side_thread, answer.error.as_deref()), (None, Some("Sign in to Anthropic again.")));
    assert_eq!(answer.restore.map(|restore| restore.text), Some("what about tests?".to_owned()));
    assert_eq!(stand_in.seen().len(), before);

    // "!ls": the client's to run; nothing's sent.
    let before = stand_in.seen().len();
    let Sent::Typed { answer, message } = client.send_draft("ses_1", &Draft::new("msg_s", "!ls"), None).unwrap() else { panic!() };
    assert_eq!((answer.shell.as_deref(), message), (Some("ls"), None));
    assert_eq!(stand_in.seen().len(), before + 1);

    // Refused: it all goes back to the composer, saying why.
    let Sent::Typed { answer, message } = client.send_draft("ses_1", &Draft::new("msg_r", "/review now"), None).unwrap() else { panic!() };
    assert_eq!(message, None);
    assert_eq!(answer.error.as_deref(), Some("The engine isn't answering."));
    assert_eq!(answer.restore.unwrap().text_beside(""), "/review now");
    // Nothing went the way before.
    assert!(stand_in.seen().iter().all(|seen| !seen.route().ends_with("/prompt") && !seen.route().ends_with("/queue/add")));
}

#[test]
fn the_work_and_ask_counts() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        // (The count of what's open is the box's, whatever its list says.)
        "kuvo/work-queue" => Answer::json(200, json!({ "owner": "all", "statuses": [], "priorities": [], "labels": [], "assignees": [], "owners": [], "removed": [], "computers": [], "items": [
            { "id": "a", "status": "todo" }, { "id": "b", "status": "done" }, { "id": "c", "status": "backlog" }
        ], "counts": { "open": 2, "statuses": {}, "priorities": {}, "labels": {}, "sources": {}, "computers": {}, "removed": 0 } })),
        "kuvo/asks" => Answer::json(200, json!({ "asks": [{ "id": "1", "kind": "signIn", "host": "x.com" }] })),
        _ => Answer::json(404, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    assert_eq!(client.work_counts().unwrap(), kuvo_box::WorkCounts { open: 2, all: 3 });
    assert_eq!(client.asks_count().unwrap(), 1);
    assert!(stand_in.seen().iter().any(|seen| seen.path == "kuvo/work-queue?owner=all&glance=1&counts=1"));
}

#[test]
fn a_box_whose_server_started_again_is_found_at_its_new_place_and_read_again() {
    let first = StandInBox::start(|_| Answer::json(200, mac_info(current_features())));
    let (home, first_serve, client) = this_mac(&first);
    client.connect().unwrap();
    assert_eq!(client.gate(), Gate::Open);

    // It starts again (an update, say): a new process, port and password, and it's an older one.
    let again = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(json!(["sessions.list"]))),
        _ => Answer::json(200, json!({ "at": "1", "sessions": {}, "projects": {} })),
    });
    let second_serve = ServeProcess::start();
    write_record(&app_box(home.path()), second_serve.pid(), again.port, &again.password);
    drop(first_serve);

    // What it is is read again there before anything else is asked.
    assert!(matches!(client.session_list(None), Err(Error::NeedsUpdating { .. })));
    assert_eq!(again.seen_routes(), vec!["GET kuvo/info"]);
    assert_eq!(first.seen().len(), 1);

    // Its record left behind by a server that's gone: the port it names is never sent anything.
    drop(second_serve);
    assert!(matches!(client.connect(), Err(Error::NotRunning(_))));
    assert_eq!(again.seen().len(), 1);
}

#[test]
fn a_read_landing_mid_reply_keeps_the_words_streamed_so_far() {
    // The second read waits until the test lets it go, so it lands while words stream.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = std::sync::Mutex::new(held);
    let reads = std::sync::atomic::AtomicUsize::new(0);
    let stand_in = StandInBox::start(move |seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        "api/session/ses_1" => Answer::json(200, json!({ "data": { "id": "ses_1" } })),
        "api/session/active" => Answer::json(200, json!({ "data": { "ses_1": {} } })),
        "api/session/ses_1/message" => {
            if reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Answer::json(200, json!({ "data": [user("msg_1", "Hi", 1.0)] }));
            }
            held.lock().unwrap().recv().unwrap();
            // As the engine keeps a step mid-reply: its block, with none of its words yet.
            let step = json!({ "type": "assistant", "id": "msg_2", "time": { "created": 2 }, "agent": "build", "model": {}, "content": [{ "type": "text", "text": "" }] });
            Answer::json(200, json!({ "data": [step, user("msg_1", "Hi", 1.0), user("msg_0", "Before", 0.5)] }))
        }
        _ => Answer::json(404, json!({})),
    });
    let events = stand_in.stream_for("api/event");
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_conversation("ses_1", at_once());
    let ConversationUpdate::Conversation(read) = follow.next_blocking().unwrap() else { panic!() };
    assert!(read.running, "the box says a run is under way");

    events.send(event("session.step.started", "ses_1", json!({ "assistantMessageID": "msg_2" }))).unwrap();
    events.send(event("session.text.started", "ses_1", json!({ "assistantMessageID": "msg_2", "ordinal": 0 }))).unwrap();
    events.send(event("session.text.delta", "ses_1", json!({ "assistantMessageID": "msg_2", "ordinal": 0, "delta": "Hel" }))).unwrap();
    // A message delivered asks for a read, which the stand-in holds.
    events.send(event("session.inbox.delivered", "ses_1", json!({ "inboxID": "msg_x" }))).unwrap();
    events.send(event("session.text.delta", "ses_1", json!({ "assistantMessageID": "msg_2", "ordinal": 0, "delta": "lo" }))).unwrap();
    while last_words(&follow.next_blocking().expect("the follow goes on")).as_deref() != Some("Hello") {}

    // The read comes back with none of the words: those shown stay, and nothing is doubled.
    release.send(()).unwrap();
    let after = loop {
        if let ConversationUpdate::Conversation(conversation) = follow.next_blocking().expect("the follow goes on")
            && conversation.messages.len() == 3
        {
            break conversation;
        }
    };
    assert_eq!(last_words(&ConversationUpdate::Conversation(after)).as_deref(), Some("Hello"));
    events.send(event("session.text.delta", "ses_1", json!({ "assistantMessageID": "msg_2", "ordinal": 0, "delta": "!" }))).unwrap();
    while last_words(&follow.next_blocking().expect("the follow goes on")).as_deref() != Some("Hello!") {}
    // Only the two reads were made: streamed words ask for none.
    let reads: Vec<_> = stand_in.seen().into_iter().filter(|seen| seen.route() == "api/session/ses_1/message").collect();
    assert_eq!(reads.len(), 2);
}

#[test]
fn a_conversation_that_isnt_there_ends_its_follow_saying_so() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        _ => Answer::json(404, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_conversation("ses_gone", at_once());
    match follow.next_blocking() {
        Some(ConversationUpdate::Gone(refusal)) => assert_eq!(refusal.message, "This conversation isn’t on this Mac any more."),
        other => panic!("gone, not {other:?}"),
    }
    assert!(follow.next_blocking().is_none(), "the follow ended");
}

#[test]
fn a_password_the_box_doesnt_take_is_said_plainly() {
    let stand_in = StandInBox::start(|_| Answer::json(200, mac_info(current_features())));
    let home = home();
    let serve = ServeProcess::start();
    write_record(&app_box(home.path()), serve.pid(), stand_in.port, "not-its-password");
    match BoxClient::this_mac(home.path()).unwrap().connect() {
        Err(Error::NotRunning(why)) => assert!(why.contains("password"), "{why}"),
        other => panic!("not taken, not {other:?}"),
    }
    assert!(stand_in.seen().is_empty());
}

#[test]
fn a_conversation_shows_each_message_by_its_parts_and_pages_back() {
    // The newest 100, newest first: one keeps its parts, one is the box's note, the rest are from
    // before; then 3 older ones before them.
    let user_with = |n: usize, metadata: Value| json!({ "type": "user", "id": format!("msg_{n:03}"), "time": { "created": n }, "text": format!("text {n}"), "metadata": metadata });
    let newest: Vec<Value> = (100..200)
        .rev()
        .map(|n| match n {
            199 => user_with(n, json!({ "kuvo": { "parts": { "words": "kept words", "keyword": "/plan" } } })),
            198 => user_with(n, json!({ "kuvo": { "note": { "from": "Wake-up", "line": "x" } } })),
            _ => user_with(n, json!(null)),
        })
        .collect();
    let older: Vec<Value> = (1..4).rev().map(|n| user_with(n, json!(null))).collect();
    let stand_in = StandInBox::start(move |seen| match (seen.method.as_str(), seen.path.as_str()) {
        (_, "kuvo/info") => Answer::json(200, mac_info(current_features())),
        (_, "api/session/ses_1") => Answer::json(200, json!({ "data": { "id": "ses_1" } })),
        (_, "api/session/ses_1/message?limit=100&order=desc") => Answer::json(200, json!({ "data": newest, "cursor": { "next": "c1" } })),
        (_, "api/session/ses_1/message?limit=100&cursor=c1") => Answer::json(200, json!({ "data": older, "cursor": { "next": "c2" } })),
        // The box reads each text from before: "text 5" was sent with /goal.
        ("POST", "kuvo/messages/parts") => {
            let texts: Vec<String> = serde_json::from_value(seen.body.as_ref().unwrap()["texts"].clone()).unwrap();
            Answer::json(200, json!({ "parts": texts.iter().map(|text| json!({ "words": format!("read {text}"), "keyword": "/goal" })).collect::<Vec<_>>() }))
        }
        _ => Answer::json(404, json!({})),
    });
    let events = stand_in.stream_for("api/event");
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_conversation("ses_1", at_once());
    let ConversationUpdate::Conversation(shown) = follow.next_blocking().unwrap() else { panic!() };
    assert_eq!(shown.messages.len(), 100);
    assert_eq!(shown.earlier.as_deref(), Some("c1"));
    let parts = |conversation: &kuvo_box::Conversation, id: &str| {
        let Some(Message::User(user)) = conversation.messages.iter().find(|message| message.id() == id) else { panic!("{id}") };
        user.parts()
    };
    // Kept with it; read by its box; the box's note as it is.
    assert_eq!((parts(&shown, "msg_199").words.as_str(), parts(&shown, "msg_199").keyword.as_deref()), ("kept words", Some("/plan")));
    assert_eq!((parts(&shown, "msg_150").words.as_str(), parts(&shown, "msg_150").keyword.as_deref()), ("read text 150", Some("/goal")));
    assert_eq!(parts(&shown, "msg_198").words, "text 198");
    let asked = |stand_in: &StandInBox| -> Vec<Vec<String>> {
        stand_in.seen().iter().filter(|seen| seen.route() == "kuvo/messages/parts").map(|seen| serde_json::from_value(seen.body.as_ref().unwrap()["texts"].clone()).unwrap()).collect()
    };
    assert_eq!(asked(&stand_in), vec![(100..198).rev().map(|n| format!("text {n}")).collect::<Vec<_>>()]);

    // Back past the first page: the 3 before, with theirs read, and nothing more before them.
    follow.load_earlier();
    let ConversationUpdate::Conversation(shown) = follow.next_blocking().unwrap() else { panic!() };
    assert_eq!(shown.messages.len(), 103);
    assert_eq!(shown.messages[..4].iter().map(Message::id).collect::<Vec<_>>(), vec!["msg_001", "msg_002", "msg_003", "msg_100"]);
    assert_eq!(shown.earlier, None);
    assert_eq!(parts(&shown, "msg_002").words, "read text 2");

    // Read again (a message delivered): the earlier ones stay ahead, their parts with them, and
    // nothing is asked twice.
    events.send(event("session.inbox.delivered", "ses_1", json!({ "inboxID": "msg_x" }))).unwrap();
    let ConversationUpdate::Conversation(shown) = follow.next_blocking().unwrap() else { panic!() };
    assert_eq!(shown.messages.len(), 103);
    assert_eq!(shown.messages[0].id(), "msg_001");
    assert_eq!(parts(&shown, "msg_150").words, "read text 150");
    assert_eq!(asked(&stand_in).len(), 2);
}

#[test]
fn a_message_that_ran_a_command_reads_as_typed_by_its_id() {
    // The engine kept the prompt the command made; its box keeps what was typed, by the message's id.
    let made = json!({ "type": "user", "id": "msg_cmd", "time": { "created": 2 }, "text": "Review the code on this branch: look for bugs" });
    let plain = json!({ "type": "user", "id": "msg_plain", "time": { "created": 1 }, "text": "hello" });
    let stand_in = StandInBox::start(move |seen| match (seen.method.as_str(), seen.route()) {
        (_, "kuvo/info") => Answer::json(200, mac_info(current_features())),
        (_, "api/session/ses_1") => Answer::json(200, json!({ "data": { "id": "ses_1" } })),
        (_, "api/session/ses_1/message") => Answer::json(200, json!({ "data": [made.clone(), plain.clone()] })),
        ("POST", "kuvo/messages/parts") => {
            let body = seen.body.clone().unwrap();
            let ids: Vec<String> = serde_json::from_value(body["ids"].clone()).unwrap_or_default();
            let texts: Vec<String> = serde_json::from_value(body["texts"].clone()).unwrap();
            // (Each with its id, and the conversation they're in.)
            if ids.len() != texts.len() || body["session"] != json!("ses_1") {
                return Answer::json(400, json!({ "message": "Send an id for each text (ids)." }));
            }
            Answer::json(200, json!({ "parts": ids.iter().zip(&texts).map(|(id, text)| if id == "msg_cmd" { json!({ "words": "this branch", "keyword": "/review", "command": true }) } else { json!({ "words": text }) }).collect::<Vec<_>>() }))
        }
        _ => Answer::json(404, json!({})),
    });
    let _events = stand_in.stream_for("api/event");
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_conversation("ses_1", at_once());
    let ConversationUpdate::Conversation(shown) = follow.next_blocking().unwrap() else { panic!() };
    let Some(Message::User(user)) = shown.messages.iter().find(|message| message.id() == "msg_cmd") else { panic!() };
    assert_eq!((user.parts().words.as_str(), user.parts().keyword.as_deref()), ("this branch", Some("/review")));
}

#[test]
fn after_a_gap_a_conversation_starts_again_from_its_newest() {
    // First the newest are 1–3; then (the stream down while more came than a read holds) 7–9.
    let user = |n: usize| json!({ "type": "user", "id": format!("msg_{n}"), "time": { "created": n }, "text": format!("text {n}") });
    let newest = std::sync::Mutex::new(vec![user(3), user(2), user(1)]);
    let gap = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let later = gap.clone();
    let stand_in = StandInBox::start(move |seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        "api/session/ses_1" => Answer::json(200, json!({ "data": { "id": "ses_1" } })),
        "api/session/ses_1/message" if seen.path.contains("cursor=") => Answer::json(200, json!({ "data": [user(6), user(5), user(4)] })),
        "api/session/ses_1/message" => {
            if later.load(std::sync::atomic::Ordering::SeqCst) {
                *newest.lock().unwrap() = vec![user(9), user(8), user(7)];
            }
            Answer::json(200, json!({ "data": newest.lock().unwrap().clone(), "cursor": { "next": "c" } }))
        }
        "kuvo/messages/parts" => Answer::json(503, json!({ "message": "Not now." })),
        _ => Answer::json(404, json!({})),
    });
    let events = stand_in.stream_for("api/event");
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_conversation("ses_1", at_once());
    let ConversationUpdate::Conversation(shown) = follow.next_blocking().unwrap() else { panic!() };
    assert_eq!(shown.messages.iter().map(Message::id).collect::<Vec<_>>(), vec!["msg_1", "msg_2", "msg_3"]);
    // (Its box couldn't read the parts: each shows as written.)
    assert!(matches!(&shown.messages[0], Message::User(user) if user.parts().words == "text 1"));

    gap.store(true, std::sync::atomic::Ordering::SeqCst);
    events.send(event("session.inbox.delivered", "ses_1", json!({ "inboxID": "msg_x" }))).unwrap();
    let ConversationUpdate::Conversation(shown) = follow.next_blocking().unwrap() else { panic!() };
    // Not 1–3 then 7–9, with 4–6 missing: the newest, and the way back from them.
    assert_eq!(shown.messages.iter().map(Message::id).collect::<Vec<_>>(), vec!["msg_7", "msg_8", "msg_9"]);
    assert_eq!(shown.earlier, None, "a page that wasn't full has nothing before it");
}

#[test]
fn a_conversation_whose_stream_cant_be_opened_is_read_on_a_timer_meanwhile() {
    let stored = std::sync::Arc::new(std::sync::Mutex::new(json!([user("msg_1", "Hi", 1.0)])));
    let messages = stored.clone();
    let stand_in = StandInBox::start(move |seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        "api/session/ses_1" => Answer::json(200, json!({ "data": { "id": "ses_1" } })),
        // Its events can't be followed now.
        "api/event" => Answer::json(503, json!({ "message": "The engine is starting." })),
        "api/session/ses_1/message" => {
            let mut newest: Vec<Value> = messages.lock().unwrap().as_array().unwrap().clone();
            newest.reverse();
            Answer::json(200, json!({ "data": newest }))
        }
        _ => Answer::json(404, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_conversation("ses_1", Timing { conversation_again: Duration::ZERO, ..at_once() });
    // A reply comes while nothing streams: it's read all the same.
    *stored.lock().unwrap() = json!([user("msg_1", "Hi", 1.0), assistant("msg_2", "Hello.", 2.0)]);
    loop {
        if let Some(words) = last_words(&follow.next_blocking().expect("the follow goes on"))
            && words == "Hello."
        {
            break;
        }
    }
}

/// What waits on you is followed as the box says it: its counts now, and again as they change
/// (`kuvo/waiting/events`), with nothing counted here.
#[test]
fn what_waits_on_you_follows_the_boxs_counts() {
    use kuvo_box::areas::Waiting;
    use kuvo_box::follow::Followed;
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        _ => Answer::json(404, json!({})),
    });
    let lines = stand_in.stream_for("kuvo/waiting/events");
    let (_home, _serve, client) = this_mac(&stand_in);
    let follow = client.follow_waiting(at_once());
    lines.send(json!({ "sessions": 2, "bots": 1, "queue": 0 }).to_string()).unwrap();
    assert_eq!(follow.next_blocking(), Some(Followed::Now(Waiting { sessions: 2, bots: 1, queue: 0 })));
    // An ask is answered: the box says its counts again.
    lines.send(json!({ "sessions": 1, "bots": 1, "queue": 0 }).to_string()).unwrap();
    assert_eq!(follow.next_blocking(), Some(Followed::Now(Waiting { sessions: 1, bots: 1, queue: 0 })));
}

/// A session's menu, done by its box (PROTOCOL.md, "A session's actions"): what it offers is read as
/// the box says it, each change goes to the box's route (never the engine's metadata), and one the
/// box refuses (moving to another computer) says why in the box's words.
#[test]
fn a_sessions_menu_is_what_its_box_says_and_does() {
    let moving = "It’s moving to another computer. Try again once it’s there.";
    let stand_in = StandInBox::start(move |seen| match (seen.method.as_str(), seen.route()) {
        (_, "kuvo/info") => Answer::json(200, mac_info(current_features())),
        ("GET", "kuvo/sessions/ses_a/actions") => Answer::json(200, json!({ "session": "ses_a", "actions": [
            { "id": "pin", "label": "Pin" }, { "id": "rename", "label": "Rename" }, { "id": "archive", "label": "Archive" },
            { "id": "delete", "label": "Delete", "destructive": true, "confirm": { "title": "Delete “Trip”?", "message": "This can’t be undone.", "label": "Delete" } }
        ] })),
        ("POST", "kuvo/sessions/ses_a/pin" | "kuvo/sessions/ses_a/archive" | "kuvo/sessions/ses_a/rename") => {
            Answer::json(200, json!({ "session": "ses_a", "info": { "id": "ses_a", "title": "Trip", "metadata": seen.body.clone().unwrap_or_default() } }))
        }
        ("DELETE", "kuvo/sessions/ses_a") => Answer::json(200, json!({ "session": "ses_a" })),
        ("POST", "kuvo/sessions/deletes") => Answer::json(200, json!({ "title": "Delete 2 sessions?", "message": "Their messages and undo snapshots go too. This can’t be undone.", "label": "Delete" })),
        (_, route) if route.starts_with("kuvo/sessions/ses_moving") => Answer::json(409, json!({ "message": moving })),
        _ => Answer::json(404, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    client.connect().expect("kuvo/info");

    let menu = client.session_actions("ses_a").expect("its menu");
    assert_eq!(menu.actions.iter().map(|action| action.label.as_str()).collect::<Vec<_>>(), vec!["Pin", "Rename", "Archive", "Delete"]);
    assert_eq!(menu.actions[3].confirm.as_ref().map(|confirm| confirm.title.as_str()), Some("Delete “Trip”?"));
    assert!(menu.held.is_none());

    assert_eq!(client.pin_session("ses_a", true).expect("pinned")["id"], "ses_a");
    client.archive_session("ses_a", true).expect("archived");
    client.rename_session("ses_a", "Ship it").expect("renamed");
    client.delete_session("ses_a").expect("deleted");
    let sent: Vec<(String, String, Option<Value>)> = stand_in.seen().into_iter().filter(|seen| seen.route() != "kuvo/info").map(|seen| (seen.method.clone(), seen.route().to_owned(), seen.body.clone())).collect();
    assert_eq!(sent[1..], [
        ("POST".into(), "kuvo/sessions/ses_a/pin".into(), Some(json!({ "pinned": true }))),
        ("POST".into(), "kuvo/sessions/ses_a/archive".into(), Some(json!({ "archived": true }))),
        ("POST".into(), "kuvo/sessions/ses_a/rename".into(), Some(json!({ "title": "Ship it" }))),
        ("DELETE".into(), "kuvo/sessions/ses_a".into(), None),
    ]);

    let all = client.deletes_confirm(&["ses_a", "ses_b"]).expect("Delete All's question");
    assert_eq!(all.title, "Delete 2 sessions?");
    assert_eq!(stand_in.seen().last().and_then(|seen| seen.body.clone()), Some(json!({ "sessions": ["ses_a", "ses_b"] })));

    match client.archive_session("ses_moving", true) {
        Err(Error::Refused(refusal)) => assert_eq!((refusal.status, refusal.message.as_str()), (409, moving)),
        other => panic!("refused in the box's words, not {other:?}"),
    }
}

/// What a first run reads from the box (#733), as the box says it: the folders worth starting in
/// and whether you've been welcomed, and saying the first run is done.
#[test]
fn a_first_run_reads_the_boxs_folders_and_says_when_its_done() {
    let stand_in = StandInBox::start(|seen| match seen.route() {
        "kuvo/info" => Answer::json(200, mac_info(current_features())),
        "kuvo/folders/suggested" => Answer::json(200, json!({ "home": "/Users/sam", "folders": [
            { "path": "/Users/sam/Projects/website", "name": "website", "repository": true, "seen": 1_794_000_000_000_u64 },
            { "path": "/Users/sam/Documents/notes", "name": "notes", "seen": 1_793_000_000_000_u64 },
            { "path": "/Users/sam/Developer/dotfiles", "name": "dotfiles", "repository": true, "seen": null },
        ] })),
        "kuvo/setup" => Answer::json(200, json!({ "setup": null, "epoch": null, "primary": null, "you": { "key": null, "role": null }, "computers": [], "startsFromOthers": { "value": null, "default": "off" }, "heldBy": [], "welcomed": false })),
        "kuvo/setup/welcomed" => Answer::json(200, json!({ "welcomed": true })),
        _ => Answer::json(404, json!({})),
    });
    let (_home, _serve, client) = this_mac(&stand_in);
    client.connect().unwrap();
    let suggested = client.suggested_folders().unwrap();
    assert_eq!(suggested.home, "/Users/sam");
    assert_eq!(suggested.folders.iter().map(|folder| (folder.name.as_str(), folder.repository, folder.seen)).collect::<Vec<_>>(), vec![("website", true, Some(1_794_000_000_000)), ("notes", false, Some(1_793_000_000_000)), ("dotfiles", true, None)]);
    assert!(!client.welcomed().unwrap());
    assert!(client.set_welcomed(true).unwrap());
    let said: Vec<_> = stand_in.seen().into_iter().filter(|seen| seen.route() == "kuvo/setup/welcomed").collect();
    assert_eq!((said[0].method.as_str(), said[0].body.clone()), ("POST", Some(json!({ "welcomed": true }))));
}

/// A box's settings as it describes them (PROTOCOL.md, "A box's settings", #727): its sections
/// and a page read as the box says them, a change sent with the row's own revision, one the box
/// finds stale answered with its page now, one it refuses said in its words, what a row does, and
/// the page followed as it changes. Nothing about any setting is known here.
#[test]
fn a_boxs_settings_are_read_changed_and_followed_as_the_box_says_them() {
    use kuvo_box::follow::Followed;
    use kuvo_box::settings_pages::{Can, Changed, Kind};
    let page = |offers: bool, revision: &str| json!({ "id": "browser", "title": "Browser", "groups": [
        { "id": "browser", "rows": [
            { "id": "search-engine", "title": "Search engine", "kind": "choice", "value": "google", "choices": [{ "id": "google", "title": "Google" }, { "id": "kagi", "title": "Kagi" }], "can": true, "revision": "se1" },
            { "id": "offers", "title": "Offer to save sign-ins", "kind": "switch", "value": offers, "can": true, "revision": revision }
        ] },
        { "id": "never", "rows": [{ "id": "never", "title": "Never Offered", "kind": "list", "revision": "nv1", "empty": "None yet.", "items": [
            { "id": "never-localhost:3000", "title": "localhost:3000", "kind": "actions", "revision": "lh1", "actions": [{ "id": "offer-again", "title": "Offer Again" }] }
        ] }] }
    ] });
    let held = "Turn it on in a conversation: pick Full Access in its mode menu, and confirm it there.";
    let stand_in = StandInBox::start(move |seen| match (seen.method.as_str(), seen.route()) {
        (_, "kuvo/info") => Answer::json(200, mac_info(current_features())),
        ("GET", "kuvo/settings") => Answer::json(200, json!({ "sections": [
            { "id": "permissions", "title": "Permissions", "group": "Agents & Tools", "icon": "shield", "line": "What agents may do without asking.", "keywords": ["sandbox"] },
            { "id": "browser", "title": "Browser", "group": "System", "icon": "globe", "line": "The browser's search engine.", "keywords": [] }
        ] })),
        ("GET", "kuvo/settings/browser") => Answer::json(200, page(true, "of1")),
        ("PATCH", "kuvo/settings/browser") => match seen.body.as_ref().and_then(|body| body["was"].as_str()) {
            Some("of1") => Answer::json(200, page(false, "of2")),
            _ => Answer::json(409, json!({ "message": "It changed since you read it.", "page": page(false, "of2") })),
        },
        ("PATCH", "kuvo/settings/permissions") => Answer::json(403, json!({ "message": held })),
        ("PATCH", "kuvo/settings/engine") => Answer::json(409, json!({ "message": "The engine's settings file can't be changed from here: line 2." })),
        ("POST", "kuvo/settings/browser/never-localhost%3A3000/offer-again") => Answer::json(200, page(true, "of1")),
        _ => Answer::json(404, json!({})),
    });
    let lines = stand_in.stream_for("kuvo/settings/browser/events");
    let (_home, _serve, client) = this_mac(&stand_in);
    client.connect().expect("kuvo/info");

    let sections = client.settings_sections().expect("the sections");
    assert_eq!(sections.iter().map(|section| (section.id.as_str(), section.icon.as_str())).collect::<Vec<_>>(), vec![("permissions", "shield"), ("browser", "globe")]);

    let read = client.settings_page("browser").expect("the page");
    let offers = read.row("offers").expect("its row").clone();
    assert_eq!((offers.kind, offers.is_on(), offers.can.clone()), (Kind::Switch, true, Some(Can::Yes)));
    assert_eq!(read.row("search-engine").and_then(|row| row.chosen()).map(|choice| choice.title.as_str()), Some("Google"));

    // Changed from the revision it was read at: the box's page after it.
    match client.change_setting("browser", &offers, json!(false)).expect("changed") {
        Changed::Done(now) => assert!(!now.row("offers").unwrap().is_on()),
        other => panic!("done, not {other:?}"),
    }
    assert_eq!(stand_in.seen().last().and_then(|seen| seen.body.clone()), Some(json!({ "row": "offers", "value": false, "was": "of1" })));
    // From a reading made before: the box says so, with the page as it is now.
    let stale = kuvo_box::settings_pages::Row { revision: "of0".into(), ..offers.clone() };
    match client.change_setting("browser", &stale, json!(true)).expect("answered") {
        Changed::Stale { message, page } => {
            assert_eq!(message, "It changed since you read it.");
            assert_eq!(page.row("offers").map(|row| row.revision.as_str()), Some("of2"));
        }
        other => panic!("stale, not {other:?}"),
    }
    // One it can't change from here: refused in the box's words.
    match client.change_setting("permissions", &offers, json!(true)) {
        Err(Error::Refused(refusal)) => assert_eq!((refusal.status, refusal.message.as_str()), (403, held)),
        other => panic!("refused, not {other:?}"),
    }
    // A 409 of the route underneath (no page): a refusal in its words, not a stale reading.
    match client.change_setting("engine", &offers, json!(true)) {
        Err(Error::Refused(refusal)) => assert_eq!(refusal.status, 409),
        other => panic!("refused, not {other:?}"),
    }
    // What a list's row offers, sent to its own route, its id kept one part of the path.
    let site = read.row("never-localhost:3000").expect("the site").clone();
    assert!(matches!(client.setting_action("browser", &site, &site.actions[0]).expect("done"), Changed::Done(_)));
    assert_eq!(stand_in.seen().last().and_then(|seen| seen.body.clone()), Some(json!({ "was": "lh1" })));

    let follow = client.follow_settings_page("browser", at_once());
    lines.send(page(true, "of1").to_string()).unwrap();
    match follow.next_blocking() {
        Some(Followed::Now(now)) => assert!(now.row("offers").unwrap().is_on()),
        other => panic!("the page, not {other:?}"),
    }
    lines.send(page(false, "of2").to_string()).unwrap();
    match follow.next_blocking() {
        Some(Followed::Now(now)) => assert!(!now.row("offers").unwrap().is_on()),
        other => panic!("the page again, not {other:?}"),
    }
}

#[test]
fn compact_drafts_use_the_sessions_own_engine_and_preserve_refusal_words() {
    for agents in [false, true] {
        let stand_in = StandInBox::start(move |seen| match seen.route() {
            "kuvo/info" => Answer::json(200, mac_info(current_features())),
            "kuvo/sessions/ses_compact/typed" => Answer::json(200, json!({ "compact": true })),
            "api/session/ses_compact" if !agents => Answer::json(200, json!({ "id": "ses_compact" })),
            "kuvo/agents/api/session/ses_compact" if agents => Answer::json(200, json!({ "id": "ses_compact" })),
            "api/session/ses_compact/compact" if !agents => Answer::json(200, json!({ "data": { "state": "waiting" } })),
            "kuvo/agents/api/session/ses_compact/compact" if agents => Answer::json(400, json!({ "message": "There’s nothing to summarize yet." })),
            _ => Answer::json(404, json!({})),
        });
        let (_home, _serve, client) = this_mac(&stand_in);
        client.connect().unwrap();
        let result = client.send_draft("ses_compact", &Draft::new("draft_compact", "/compact"), None);
        if agents {
            assert!(result.unwrap_err().to_string().contains("There’s nothing to summarize yet."));
        } else {
            assert!(matches!(result.unwrap(), Sent::Typed { answer, .. } if answer.compact == Some(true)));
        }
        let seen = stand_in.seen();
        let expected = if agents { "kuvo/agents/api/session/ses_compact/compact" } else { "api/session/ses_compact/compact" };
        assert_eq!(seen.last().unwrap().route(), expected);
    }
}

#[test]
fn shell_drafts_send_the_box_command_to_the_sessions_engine() {
    for agents in [false, true] {
        let stand_in = StandInBox::start(move |seen| match seen.route() {
            "kuvo/info" => Answer::json(200, mac_info(current_features())),
            "kuvo/sessions/ses_shell/typed" => Answer::json(200, json!({ "shell": "printf hello" })),
            "api/session/ses_shell" if !agents => Answer::json(200, json!({})),
            "kuvo/agents/api/session/ses_shell" if agents => Answer::json(200, json!({})),
            "api/session/ses_shell/shell" | "kuvo/agents/api/session/ses_shell/shell" => {
                assert_eq!(seen.method, "POST");
                Answer::json(200, json!({}))
            }
            _ => Answer::json(404, json!({})),
        });
        let (_home, _serve, client) = this_mac(&stand_in);
        client.connect().unwrap();
        client.send_draft("ses_shell", &Draft::new("draft_shell", "!printf hello"), None).unwrap();
        let seen = stand_in.seen();
        let last = seen.last().unwrap();
        assert_eq!(last.route(), if agents { "kuvo/agents/api/session/ses_shell/shell" } else { "api/session/ses_shell/shell" });
        assert_eq!(last.body, Some(json!({ "command": "printf hello" })));
    }
}
