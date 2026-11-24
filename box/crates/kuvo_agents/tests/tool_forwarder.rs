//! The stdio forwarder against a stand-in relay: an in-test HTTP listener.

use kuvo_agents::tool_forwarder::{Relay, forward};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream},
    net::TcpListener,
};

#[derive(Clone, Debug)]
struct Seen {
    headers: HashMap<String, String>,
    body: Value,
}

type Answer = (u16, Vec<(&'static str, String)>, String);

/// Serve `answer` for each request on `listener`, noting what came.
fn serve(
    listener: TcpListener,
    seen: Arc<Mutex<Vec<Seen>>>,
    answer: impl Fn(usize, &Seen) -> Option<Answer> + Send + Sync + 'static,
) {
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let (seen, answer) = (seen.clone(), answer.clone());
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                let mut headers = HashMap::new();
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                loop {
                    line.clear();
                    stream.read_line(&mut line).await.unwrap();
                    let Some((name, value)) = line.trim_end().split_once(": ") else { break };
                    headers.insert(name.to_ascii_lowercase(), value.to_owned());
                }
                let length = headers["content-length"].parse().unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                let request = Seen { headers, body: serde_json::from_slice(&body).unwrap() };
                let count = {
                    let mut seen = seen.lock().unwrap();
                    seen.push(request.clone());
                    seen.len()
                };
                // None: never answer (a tool that takes its time).
                let Some((status, extra, body)) = answer(count, &request) else {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    return;
                };
                let mut reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                    body.len()
                );
                for (name, value) in extra {
                    reply.push_str(&format!("{name}: {value}\r\n"));
                }
                reply.push_str("\r\n");
                reply.push_str(&body);
                let _ = stream.get_mut().write_all(reply.as_bytes()).await;
            });
        }
    });
}

struct Agent {
    input: DuplexStream,
    output: tokio::io::Lines<BufReader<DuplexStream>>,
}

impl Agent {
    async fn send(&mut self, message: Value) {
        self.input.write_all(format!("{message}\n").as_bytes()).await.unwrap();
    }

    async fn answer(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.output.next_line())
            .await
            .expect("no answer came")
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

fn start(port: u16) -> Agent {
    let (input, agent_input) = tokio::io::duplex(1 << 16);
    let (agent_output, output) = tokio::io::duplex(1 << 16);
    let relay = Relay {
        url: format!("http://127.0.0.1:{port}/mcp"),
        key: "Bearer fixture".into(),
        timeout: Duration::from_secs(5),
    };
    tokio::spawn(forward(relay, BufReader::new(agent_input), agent_output));
    Agent { input, output: BufReader::new(output).lines() }
}

fn ok(body: Value) -> Option<Answer> {
    Some((200, Vec::new(), body.to_string()))
}

#[tokio::test]
async fn a_call_waits_for_the_relay_and_is_sent_again_as_the_same_operation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut agent = start(port);
    agent
        .send(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"send_message"}}))
        .await;
    tokio::time::sleep(Duration::from_millis(350)).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    serve(TcpListener::bind(("127.0.0.1", port)).await.unwrap(), seen.clone(), |count, request| {
        assert_eq!(request.headers["authorization"], "Bearer fixture");
        assert_eq!(request.headers["x-kuvo-tool-forwarder"], "1");
        if count == 1 {
            ok(json!({"jsonrpc":"2.0","id":1,"error":{"data":{"kuvoRelayRestart":true}}}))
        } else {
            ok(json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Sent."}]}}))
        }
    });
    assert_eq!(agent.answer().await["result"]["content"][0]["text"], "Sent.");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let first = seen[0].body["params"]["_meta"]["kuvo.operation"].as_str().unwrap();
    assert_eq!(first.len(), 36);
    assert_eq!(first.as_bytes()[14], b'4');
    assert_eq!(seen[1].body["params"]["_meta"]["kuvo.operation"], first);
    assert_eq!(seen[0].body["params"]["name"], "send_message");
}

#[tokio::test]
async fn a_browser_call_is_never_sent_twice_and_refusals_are_told() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    serve(listener, seen.clone(), |_, request| match request.body["id"].as_i64() {
        Some(1) => Some((502, Vec::new(), String::new())),
        Some(2) => Some((500, Vec::new(), "{}".into())),
        _ => ok(json!({"jsonrpc":"2.0","id":request.body["id"],"error":{"data":{"kuvoRelayRestart":true}}})),
    });
    let mut agent = start(port);
    agent.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"browser_click"}})).await;
    assert_eq!(
        agent.answer().await,
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"Kuvo Code's tools didn't answer."}})
    );
    agent.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
    assert_eq!(
        agent.answer().await,
        json!({"jsonrpc":"2.0","id":2,"error":{"code":-32603,"message":"Kuvo Code's tools said HTTP 500."}})
    );
    // A browser call the relay says to send again is answered as it is.
    agent.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"browser_type"}})).await;
    assert_eq!(agent.answer().await["error"]["data"]["kuvoRelayRestart"], true);
    agent.input.write_all(b"not json\n").await.unwrap();
    assert_eq!(
        agent.answer().await,
        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})
    );
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn the_mcp_session_carries_on_and_a_cancelled_call_goes_unanswered() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    serve(listener, seen.clone(), |_, request| match request.body["method"].as_str() {
        Some("initialize") => Some((
            200,
            vec![("mcp-session-id", "mcp-1".into())],
            json!({"jsonrpc":"2.0","id":1,"result":{}}).to_string(),
        )),
        Some("tools/call") => None,
        Some("tools/list") => ok(json!({"jsonrpc":"2.0","id":4,"result":{"tools":[]}})),
        _ => Some((202, Vec::new(), String::new())),
    });
    let mut agent = start(port);
    agent.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})).await;
    assert_eq!(agent.answer().await["id"], 1);
    agent.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    agent.send(json!({"jsonrpc":"2.0","id":"slow","method":"tools/call","params":{"name":"wait"}})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    agent
        .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"slow"}}))
        .await;
    agent.send(json!({"jsonrpc":"2.0","id":4,"method":"tools/list"})).await;
    // The next answer is the list's, not the cancelled call's.
    let next = agent.answer().await;
    assert_eq!(next["id"], 4);
    let seen = seen.lock().unwrap();
    let methods = seen
        .iter()
        .map(|seen| seen.body["method"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(methods.contains(&"notifications/cancelled".to_owned()));
    assert!(seen[1..].iter().all(|seen| seen.headers["mcp-session-id"] == "mcp-1"));
    assert!(!seen[0].headers.contains_key("mcp-session-id"));
}
