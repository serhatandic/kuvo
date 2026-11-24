//! The tools relay with stand-in tools: a program (this test binary, as a
//! probe) and an app server. Each case names the bridge's
//! `toolrelay.test.ts` case it ports.

use futures_util::{FutureExt, future::BoxFuture};
use kuvo_agents::tool_relay::{
    ComputerServer, ComputerServers, RelayedServer, RelayedTools, ServerHost, ServerKind,
    ToolRelay, ToolSource, Tools,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// The stand-in program: `said` answers each call with the conversation it
/// was for; `holds` holds each call until it's cancelled.
#[test]
#[ignore = "a program the relay starts"]
fn relay_program_probe() {
    let mode = std::env::var("RELAY_PROBE").unwrap_or_default();
    let holding = std::env::var_os("RELAY_HOLDING").map(PathBuf::from);
    let mut held = std::collections::HashSet::new();
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let answer = |text: Value| json!({"jsonrpc":"2.0","id":message["id"],"result":{"content":[{"type":"text","text":text}]}});
        let said = match (mode.as_str(), message["method"].as_str()) {
            ("said", _) if !message["id"].is_null() => {
                let session = message["params"]["_meta"]["ai.opencode/sessionID"].clone();
                let session = if session.is_null() { json!(std::env::var("KUVO_SESSION").unwrap_or_default()) } else { session };
                Some(answer(session))
            }
            ("holds", Some("initialize")) => Some(json!({"jsonrpc":"2.0","id":message["id"],"result":{}})),
            ("server", Some("initialize")) => Some(json!({"jsonrpc":"2.0","id":message["id"],"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}}}})),
            ("server", Some("tools/call")) => Some(answer(json!(format!(
                "{} {} {}",
                std::env::var("OPENCODE_SESSION_ID").unwrap_or_default(),
                std::env::var("SERVER_KEY").unwrap_or_default(),
                std::process::id()
            )))),
            ("refuses", Some("initialize")) => Some(json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32000,"message":"no token"}})),
            ("holds", Some("tools/call")) => {
                held.insert(message["id"].clone());
                if let Some(path) = &holding {
                    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
                    writeln!(file, "{}", message["id"]).unwrap();
                }
                None
            }
            ("holds", Some("notifications/cancelled")) if held.remove(&message["params"]["requestId"]) => Some(
                json!({"jsonrpc":"2.0","id":message["params"]["requestId"],"result":{"content":[{"type":"text","text":"cancelled"}]}}),
            ),
            _ => None,
        };
        if let Some(said) = said {
            // (On a line of its own: the test harness's words come before.)
            writeln!(out, "\n{said}").unwrap();
            out.flush().unwrap();
        }
    }
}

struct Source(Mutex<Option<Tools>>);

impl ToolSource for Source {
    fn tools(&self, session: &str) -> Option<Tools> {
        let mut tools = self.0.lock().unwrap().clone()?;
        if let Tools::Program { env, .. } = &mut tools {
            env.insert("KUVO_SESSION".into(), session.into());
        }
        Some(tools)
    }
}

fn program(mode: &str, holding: Option<&Path>) -> Arc<Source> {
    let mut env: kuvo_os::platform::Environment = std::env::vars_os().collect();
    env.insert("RELAY_PROBE".into(), mode.into());
    if let Some(holding) = holding {
        env.insert("RELAY_HOLDING".into(), holding.into());
    }
    Arc::new(Source(Mutex::new(Some(Tools::Program {
        key: mode.into(),
        command: std::env::current_exe().unwrap(),
        args: ["--exact", "relay_program_probe", "--ignored", "--nocapture", "--test-threads=1"]
            .map(Into::into)
            .to_vec(),
        env,
        timeout: Duration::from_secs(10),
    }))))
}

async fn call(tools: &RelayedTools, id: u64, claims: Option<&str>) -> (u16, Value) {
    call_with(tools, &tools.authorization, id, claims, &[]).await
}

async fn call_with(
    tools: &RelayedTools,
    authorization: &str,
    id: u64,
    claims: Option<&str>,
    extra: &[(&str, &str)],
) -> (u16, Value) {
    let mut params = json!({"name":"show_in_panel","arguments":{}});
    if let Some(claims) = claims {
        params["_meta"] = json!({"ai.opencode/sessionID": claims});
    }
    let mut request = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(&tools.url)
        .header("Authorization", authorization)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let answer = request
        .body(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":params}).to_string())
        .send()
        .await
        .unwrap();
    let status = answer.status().as_u16();
    (status, answer.json().await.unwrap_or(Value::Null))
}

fn said(answer: &Value) -> &str {
    answer["result"]["content"][0]["text"].as_str().unwrap_or_default()
}

// toolrelay.test.ts: "an agent that outlived its bridge (held through an
// update) reaches its tools at the same address with the same key; a key the
// new bridge took up no agent for is turned down"
#[tokio::test]
async fn an_agent_that_outlived_its_resident_reaches_its_tools_at_the_same_address_and_key() {
    let scratch = tempfile::tempdir().unwrap();
    let record = scratch.path().join("run/agents/relay.json");
    let first = ToolRelay::new(Some(record.clone()), program("said", None)).unwrap();
    let one = first.relayed_tools("ses_one", 1).await.unwrap().unwrap();
    let two = first.relayed_tools("ses_two", 1).await.unwrap().unwrap();
    first.shutdown().await;
    drop(first);
    // The next resident took up ses_one's agent; ses_two's was gone by then.
    let next = ToolRelay::new(Some(record), program("said", None)).unwrap();
    assert_eq!(next.relayed_tools("ses_one", 9).await.unwrap().unwrap(), one);
    next.keep_keys(&BTreeSet::from(["ses_one".to_owned()]));
    let (status, answer) = call(&one, 1, None).await;
    assert_eq!(status, 200);
    assert_eq!(said(&answer), "ses_one");
    assert_eq!(call(&two, 2, None).await.0, 401);
    next.shutdown().await;
}

// "each call reaches the program as its own session's, even one that names
// another, and answers its own request"
#[tokio::test]
async fn each_call_reaches_the_program_as_its_own_conversations_even_one_naming_another() {
    let relay = ToolRelay::new(None, program("said", None)).unwrap();
    let one = relay.relayed_tools("ses_one", 1).await.unwrap().unwrap();
    let two = relay.relayed_tools("ses_two", 1).await.unwrap().unwrap();
    assert!(one.url.starts_with("http://127.0.0.1:"));
    let (mine, claimed, theirs) = tokio::join!(
        call(&one, 7, None),
        call(&one, 7, Some("ses_two")),
        call(&two, 7, None)
    );
    assert_eq!(mine.1, json!({"jsonrpc":"2.0","id":7,"result":{"content":[{"type":"text","text":"ses_one"}]}}));
    assert_eq!(said(&claimed.1), "ses_one");
    assert_eq!(said(&theirs.1), "ses_two");
    relay.shutdown().await;
}

// "two of its connections (Codex has one a thread) using the same request's
// number: a cancel stops only its own connection's call"
#[tokio::test]
async fn a_cancel_stops_only_its_own_connections_call() {
    let scratch = tempfile::tempdir().unwrap();
    let holding = scratch.path().join("holding.txt");
    let relay = ToolRelay::new(None, program("holds", Some(&holding))).unwrap();
    let tools = relay.relayed_tools("ses_one", 1).await.unwrap().unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let post = |body: Value, connection: Option<String>| {
        let mut request = client
            .post(&tools.url)
            .header("Authorization", &tools.authorization)
            .header("Content-Type", "application/json");
        if let Some(connection) = connection {
            request = request.header("mcp-session-id", connection);
        }
        request.body(body.to_string()).send()
    };
    let connect = || async {
        let answer = post(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}), None)
            .await
            .unwrap();
        answer.headers()["mcp-session-id"].to_str().unwrap().to_owned()
    };
    let first = connect().await;
    let second = connect().await;
    assert_ne!(first, second);
    let call = |connection: String| {
        let request = post(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"show_in_panel","arguments":{}}}),
            Some(connection),
        );
        async move { request.await.unwrap().json::<Value>().await.unwrap() }
    };
    let first_call = tokio::spawn(call(first.clone()));
    let second_call = tokio::spawn(call(second.clone()));
    for _ in 0..500 {
        let held = std::fs::read_to_string(&holding).unwrap_or_default();
        if held.lines().count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let cancel = json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}});
    assert_eq!(post(cancel.clone(), Some(first)).await.unwrap().status(), 202);
    assert_eq!(said(&first_call.await.unwrap()), "cancelled");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!second_call.is_finished());
    post(cancel, Some(second)).await.unwrap();
    assert_eq!(said(&second_call.await.unwrap()), "cancelled");
    relay.shutdown().await;
}

// "a key it didn't give, or one whose session is done, reaches nothing"
#[tokio::test]
async fn a_key_it_didnt_give_or_one_whose_conversation_is_done_reaches_nothing() {
    let relay = ToolRelay::new(None, program("said", None)).unwrap();
    let one = relay.relayed_tools("ses_one", 1).await.unwrap().unwrap();
    assert_eq!(call_with(&one, "Bearer nope", 1, None, &[]).await.0, 401);
    relay.drop_relayed("ses_one", 1);
    assert_eq!(call(&one, 1, None).await.0, 401);
    assert!(!relay.uses("ses_one"));
    relay.shutdown().await;
}

// "a session's other agent keeps reaching its tools when one is done with
// them (an account switched, say)"
#[tokio::test]
async fn a_conversations_other_agent_keeps_reaching_its_tools_when_one_is_done() {
    let relay = ToolRelay::new(None, program("said", None)).unwrap();
    relay.relayed_tools("ses_one", 1).await.unwrap().unwrap();
    let one = relay.relayed_tools("ses_one", 2).await.unwrap().unwrap();
    relay.drop_relayed("ses_one", 1);
    assert_eq!(call_with(&one, "Bearer nope", 1, None, &[]).await.0, 401);
    assert_eq!(call(&one, 1, None).await.0, 200);
    relay.drop_relayed("ses_one", 2);
    assert_eq!(call(&one, 1, None).await.0, 401);
    relay.shutdown().await;
}

// "each call goes to the app's server with the app's password and its own
// session, whatever it says"
#[tokio::test]
async fn each_call_goes_to_the_apps_server_with_its_password_and_own_conversation() {
    let app = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = app.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let noted = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = app.accept().await else { return };
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            let (head, body) = loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned))
                        .and_then(|length| length.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() >= length {
                        break (head.to_owned(), body.to_owned());
                    }
                }
            };
            let header = |name: &str| {
                head.lines().find_map(|line| {
                    let (key, value) = line.split_once(": ")?;
                    key.eq_ignore_ascii_case(name).then(|| value.to_owned())
                })
            };
            let body: Value = serde_json::from_str(&body).unwrap();
            noted.lock().unwrap().push(json!({
                "authorization": header("authorization"), "session": header("x-kuvo-session"),
                "claims": body["params"]["_meta"]["ai.opencode/sessionID"],
            }));
            let answer = json!({"jsonrpc":"2.0","id":body["id"],"result":{"content":[]}}).to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    struct App(u16);
    impl ToolSource for App {
        fn tools(&self, session: &str) -> Option<Tools> {
            Some(Tools::Http {
                url: format!("http://127.0.0.1:{}/mcp", self.0),
                headers: vec![
                    ("Authorization".into(), "Bearer app-password".into()),
                    ("X-Kuvo-Session".into(), session.into()),
                ],
                timeout: Duration::from_secs(10),
            })
        }
    }
    let relay = ToolRelay::new(None, Arc::new(App(port))).unwrap();
    let one = relay.relayed_tools("ses_one", 1).await.unwrap().unwrap();
    assert!(!one.authorization.contains("app-password"));
    let (status, answer) = call_with(&one, &one.authorization, 3, Some("ses_two"), &[("X-Kuvo-Session", "ses_two")]).await;
    assert_eq!(status, 200);
    assert_eq!(answer, json!({"jsonrpc":"2.0","id":3,"result":{"content":[]}}));
    assert_eq!(
        *seen.lock().unwrap(),
        [json!({"authorization":"Bearer app-password","session":"ses_one","claims":"ses_one"})]
    );
    relay.shutdown().await;
}

/// A stand-in app or URL server: each request's headers and body, and what
/// `answer` says to it.
async fn http_server(
    answer: impl Fn(&str, &Value) -> (u16, Value) + Send + Sync + 'static,
) -> u16 {
    let app = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = app.local_addr().unwrap().port();
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = app.accept().await else { return };
            let answer = answer.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                let (head, body) = loop {
                    let Ok(read) = socket.read(&mut buffer).await else { return };
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned))
                            .and_then(|length| length.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if body.len() >= length {
                            break (head.to_owned(), body.to_owned());
                        }
                    }
                };
                let (status, said) = answer(&head, &serde_json::from_str(&body).unwrap_or(Value::Null));
                let said = said.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{said}",
                    said.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    port
}

async fn post(server: &RelayedServer, body: Value, connection: Option<&str>) -> (u16, Option<String>, Value) {
    let mut request = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(&server.url)
        .header("Authorization", &server.authorization)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(connection) = connection {
        request = request.header("mcp-session-id", connection);
    }
    let answer = request.body(body.to_string()).send().await.unwrap();
    let connection = answer
        .headers()
        .get("mcp-session-id")
        .map(|value| value.to_str().unwrap().to_owned());
    (answer.status().as_u16(), connection, answer.json().await.unwrap_or(Value::Null))
}

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})
}

fn tool_call(id: u64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"look","arguments":{}}})
}

fn remote(name: &str, port: u16, token: &str) -> ComputerServer {
    ComputerServer {
        name: name.into(),
        timeout: Duration::from_secs(10),
        kind: ServerKind::Remote {
            url: format!("http://127.0.0.1:{port}/mcp"),
            headers: vec![("Authorization".into(), format!("Bearer {token}"))],
            expires: None,
        },
    }
}

#[derive(Default)]
struct Host {
    /// The server as the host has it now, after a reconnect.
    now: Mutex<Option<ComputerServer>>,
    reconnects: Mutex<u32>,
}

impl ServerHost for Host {
    fn renewed<'a>(&'a self, _: &'a Path, _: &'a str) -> BoxFuture<'a, Option<ComputerServer>> {
        async { self.now.lock().unwrap().clone() }.boxed()
    }

    fn reconnect<'a>(&'a self, _: &'a Path, _: &'a str) -> BoxFuture<'a, ()> {
        async { *self.reconnects.lock().unwrap() += 1 }.boxed()
    }
}

#[tokio::test]
async fn a_server_that_cant_be_reached_is_a_stand_in_whose_words_say_why() {
    let closed = TcpListener::bind(("127.0.0.1", 0)).await.unwrap().local_addr().unwrap().port();
    let relay = ToolRelay::new(None, program("said", None)).unwrap();
    let found = ComputerServers { servers: vec![remote("docs", closed, "t")], key: "docs".into() };
    let scratch = tempfile::tempdir().unwrap();
    let servers = relay.relayed_servers("ses_one", 1, scratch.path(), &found, true).await.unwrap();
    assert_eq!(servers.len(), 1);
    assert!(servers[0].url.ends_with("/mcp/docs"), "{}", servers[0].url);
    let (status, connection, started) = post(&servers[0], initialize(), None).await;
    assert_eq!(status, 200);
    let connection = connection.unwrap();
    assert!(connection.starts_with("unavailable-"));
    assert_eq!(started["result"]["serverInfo"]["name"], "unavailable");
    assert_eq!(
        started["result"]["instructions"],
        "The docs server couldn't be reached (no answer), so its tools aren't available for now."
    );
    let (_, _, listed) = post(&servers[0], json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}), Some(&connection)).await;
    assert_eq!(listed["result"]["tools"], json!([]));
    let (_, _, called) = post(&servers[0], tool_call(2), Some(&connection)).await;
    assert_eq!(called["result"]["isError"], true);
    // Another conversation's key can't reach this one's servers.
    let other = relay.relayed_tools("ses_two", 1).await.unwrap().unwrap();
    let theirs = RelayedServer { authorization: other.authorization, ..servers[0].clone() };
    assert_eq!(post(&theirs, initialize(), None).await.0, 404);
    relay.shutdown().await;
}

// relay-live.test.ts: "the agent starts with a stand-in saying to sign in
// again, the engine finds it needs signing in, and signed in again the
// agent reaches it"
#[tokio::test]
async fn a_server_that_turns_down_its_sign_in_is_asked_again_once_and_reached_once_signed_in() {
    let port = http_server(|head, body| {
        if head.contains("Bearer good") {
            (200, json!({"jsonrpc":"2.0","id":body["id"],"result":{"protocolVersion":"2025-06-18","capabilities":{}}}))
        } else {
            (401, json!({"error":"sign in"}))
        }
    })
    .await;
    let host = Arc::new(Host::default());
    let relay = ToolRelay::with_host(None, program("said", None), host.clone()).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let before = ComputerServers { servers: vec![remote("notes", port, "old")], key: "notes".into() };
    let servers = relay.relayed_servers("ses_one", 1, scratch.path(), &before, false).await.unwrap();
    let (status, connection, started) = post(&servers[0], initialize(), None).await;
    assert_eq!(status, 200);
    assert!(connection.unwrap().starts_with("unavailable-"));
    assert!(started["result"]["instructions"].as_str().unwrap().contains("turned down its sign-in"));
    // The host is asked to connect it again, once for how it's set up.
    for _ in 0..100 {
        if *host.reconnects.lock().unwrap() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    post(&servers[0], initialize(), None).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(*host.reconnects.lock().unwrap(), 1);
    assert_eq!(relay.servers_key("ses_one", &before), "notes");
    // Signed in again: its agent is given it afresh, and reaches it.
    let after = ComputerServers { servers: vec![remote("notes", port, "good")], key: "notes".into() };
    assert_ne!(relay.servers_key("ses_one", &after), "notes");
    let again = relay.relayed_servers("ses_one", 1, scratch.path(), &after, false).await.unwrap();
    assert!(again[0].url.ends_with("/mcp/notes?again=1"), "{}", again[0].url);
    let (status, connection, started) = post(&again[0], initialize(), None).await;
    assert_eq!(status, 200);
    assert!(connection.is_none());
    assert_eq!(started["result"]["protocolVersion"], "2025-06-18");
    relay.shutdown().await;
}

#[tokio::test]
async fn a_command_server_starts_for_each_connection_with_its_keys_and_stops_when_left() {
    let relay = ToolRelay::new(None, program("said", None)).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let mut env = kuvo_os::platform::Environment::new();
    env.insert("RELAY_PROBE".into(), "server".into());
    env.insert("SERVER_KEY".into(), "k1".into());
    let mut command: Vec<std::ffi::OsString> = vec![std::env::current_exe().unwrap().into()];
    command.extend(["--exact", "relay_program_probe", "--ignored", "--nocapture", "--test-threads=1"].map(Into::into));
    let local = |name: &str, env: kuvo_os::platform::Environment| ComputerServer {
        name: name.into(),
        timeout: Duration::from_secs(10),
        kind: ServerKind::Local { command: command.clone(), env, cwd: None },
    };
    let mut refusing = env.clone();
    refusing.insert("RELAY_PROBE".into(), "refuses".into());
    let found = ComputerServers {
        servers: vec![local("files", env), local("broken", refusing)],
        key: "files broken".into(),
    };
    let servers = relay.relayed_servers("ses_one", 1, scratch.path(), &found, false).await.unwrap();
    let (status, first, _) = post(&servers[0], initialize(), None).await;
    assert_eq!(status, 200);
    let first = first.unwrap();
    let (_, second, _) = post(&servers[0], initialize(), None).await;
    let second = second.unwrap();
    assert_ne!(first, second);
    let said = |answer: Value| answer["result"]["content"][0]["text"].as_str().unwrap().to_owned();
    let one = said(post(&servers[0], tool_call(1), Some(&first)).await.2);
    let two = said(post(&servers[0], tool_call(1), Some(&second)).await.2);
    let (one, two) = (one.split(' ').collect::<Vec<_>>(), two.split(' ').collect::<Vec<_>>());
    assert_eq!(one[..2], ["ses_one", "k1"]);
    assert_eq!(two[..2], ["ses_one", "k1"]);
    assert_ne!(one[2], two[2], "each connection has a server of its own");
    // Left: that connection's server stops; the agent connects again.
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let left = client
        .delete(&servers[0].url)
        .header("Authorization", &servers[0].authorization)
        .header("mcp-session-id", &first)
        .send()
        .await
        .unwrap();
    assert_eq!(left.status(), 200);
    assert_eq!(post(&servers[0], tool_call(2), Some(&first)).await.0, 404);
    // One that says no as it starts is a stand-in saying so.
    let (_, connection, refused) = post(&servers[1], initialize(), None).await;
    assert!(connection.unwrap().starts_with("unavailable-"));
    assert_eq!(refused["result"]["instructions"], "The broken server couldn't start: no token.");
    relay.shutdown().await;
}
