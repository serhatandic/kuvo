//! The stdio MCP forwarder an agent starts in its sandbox (`kuvo-box mcp`):
//! each message goes to the box's tools relay with this session's key, and
//! each answer comes back on stdout. Only the relay's address and the key
//! are given to it (`KUVO_TOOL_RELAY`, `KUVO_TOOL_RELAY_KEY`), with the
//! longest a tool may take (`KUVO_TOOL_TIMEOUT`, milliseconds).
//!
//! While the relay restarts (a box update), a call waits for it and is sent
//! again with the same `kuvo.operation`, so the tool does it once. A browser
//! call is not sent again: it may have acted already.

use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    time::{Instant, sleep},
};
use tokio_util::sync::CancellationToken;

/// How long a call keeps trying to reach a relay that's restarting.
const RETRY_FOR: Duration = Duration::from_secs(80);
const NOTIFY_WAIT: Duration = Duration::from_secs(5);

/// Where the relay is, and this session's key.
#[derive(Clone, Debug)]
pub struct Relay {
    pub url: String,
    pub key: String,
    /// The longest one tool may take.
    pub timeout: Duration,
}

impl Relay {
    /// The relay as the agent's environment gives it.
    pub fn from_env() -> io::Result<Self> {
        let read = |name: &str| {
            std::env::var(name).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("{name} isn't set."))
            })
        };
        Ok(Self {
            url: read("KUVO_TOOL_RELAY")?,
            key: read("KUVO_TOOL_RELAY_KEY")?,
            // As the bridge's forwarder reads it: a missing or bad value
            // leaves only the restart allowance.
            timeout: Duration::from_millis(
                std::env::var("KUVO_TOOL_TIMEOUT")
                    .ok()
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0),
            ),
        })
    }
}

struct Shared {
    relay: Relay,
    client: reqwest::Client,
    session: Mutex<Option<String>>,
    calls: Mutex<HashMap<String, CancellationToken>>,
    output: mpsc::Sender<Value>,
}

/// Forward MCP from `input` to the relay until `input` closes.
pub async fn forward(
    relay: Relay,
    input: impl AsyncBufRead + Unpin,
    mut output: impl AsyncWrite + Unpin + Send + 'static,
) -> io::Result<()> {
    // The relay is on this computer: never through a proxy.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(io::Error::other)?;
    let (answers, mut written) = mpsc::channel::<Value>(64);
    let writer = tokio::spawn(async move {
        while let Some(answer) = written.recv().await {
            let mut line = answer.to_string();
            line.push('\n');
            if output.write_all(line.as_bytes()).await.is_err() || output.flush().await.is_err() {
                return;
            }
        }
    });
    let shared = Arc::new(Shared {
        relay,
        client,
        session: Mutex::new(None),
        calls: Mutex::new(HashMap::new()),
        output: answers,
    });
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await? {
        let shared = shared.clone();
        tokio::spawn(async move { shared.handle(line).await });
    }
    // The agent closed its end: as the bridge's forwarder, it ends now.
    drop(shared);
    writer.abort();
    Ok(())
}

/// What the agent's request is called by (`id`), as a key.
fn key_of(id: &Value) -> String {
    id.to_string()
}

impl Shared {
    async fn handle(self: Arc<Self>, line: String) {
        let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(&line) else {
            if serde_json::from_str::<Value>(&line).is_err() {
                let _ = self
                    .output
                    .send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}))
                    .await;
            }
            return;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        let Some(id) = id else {
            match method.as_deref() {
                Some("notifications/initialized") => {
                    let _ = self.send(line, Some(NOTIFY_WAIT), None).await;
                }
                Some("notifications/cancelled") => {
                    let Some(call) = message
                        .get("params")
                        .and_then(|params| params.get("requestId"))
                        .filter(|id| !id.is_null())
                    else {
                        return;
                    };
                    let token = self
                        .calls
                        .lock()
                        .expect("the call table is never poisoned")
                        .get(&key_of(call))
                        .cloned();
                    if let Some(token) = token {
                        token.cancel();
                        let _ = self.send(line, Some(NOTIFY_WAIT), None).await;
                    }
                }
                _ => {}
            }
            return;
        };
        let cancel = CancellationToken::new();
        self.calls
            .lock()
            .expect("the call table is never poisoned")
            .insert(key_of(&id), cancel.clone());
        let tool_call = method.as_deref() == Some("tools/call");
        let name = message
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let replay = !tool_call || name.is_some_and(|name| !name.starts_with("browser_"));
        if tool_call {
            add_operation(&mut message);
        }
        let initialize = method.as_deref() == Some("initialize");
        let body = Value::Object(message).to_string();
        let deadline = Instant::now() + self.relay.timeout + RETRY_FOR;
        let outcome = tokio::select! {
            outcome = self.call(&body, replay, initialize) => Some(outcome),
            () = cancel.cancelled() => None,
            () = tokio::time::sleep_until(deadline) => Some(Err(didnt_answer())),
        };
        self.calls
            .lock()
            .expect("the call table is never poisoned")
            .remove(&key_of(&id));
        let answer = match outcome {
            // Cancelled: the agent no longer waits for it.
            None => return,
            Some(Ok(answer)) => answer,
            Some(Err(message)) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":message}})
            }
        };
        let _ = self.output.send(answer).await;
    }

    async fn call(&self, body: &str, replay: bool, initialize: bool) -> Result<Value, String> {
        let mut retry_by: Option<Instant> = None;
        loop {
            match self.send(body.to_owned(), None, Some(initialize)).await {
                Ok((status, answer)) if status != 502 && status != 503 => {
                    let Some(answer) = answer else {
                        return Err(format!("Kuvo Code's tools said HTTP {status}."));
                    };
                    let restarting = truthy(&answer["error"]["data"]["kuvoRelayRestart"]);
                    if !restarting || !replay {
                        return Ok(answer);
                    }
                }
                Ok((status, _)) => {
                    if status == 502 && !replay {
                        break;
                    }
                }
                Err(error) => {
                    if !replay && !error.is_connect() {
                        break;
                    }
                }
            }
            let by = *retry_by.get_or_insert_with(|| Instant::now() + RETRY_FOR);
            if Instant::now() >= by {
                break;
            }
            sleep(Duration::from_millis(200)).await;
        }
        Err(didnt_answer())
    }

    /// POST one message. Answers its status and, for a success, its JSON.
    async fn send(
        &self,
        body: String,
        wait: Option<Duration>,
        initialize: Option<bool>,
    ) -> Result<(u16, Option<Value>), reqwest::Error> {
        let mut request = self
            .client
            .post(&self.relay.url)
            .header("Content-Type", "application/json")
            .header("Authorization", &self.relay.key)
            .header("X-Kuvo-Tool-Forwarder", "1")
            .body(body);
        let session = self
            .session
            .lock()
            .expect("the session is never poisoned")
            .clone();
        if let Some(session) = session {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(wait) = wait {
            request = request.timeout(wait);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() || initialize.is_none() {
            return Ok((status.as_u16(), None));
        }
        if initialize == Some(true)
            && let Some(session) = response.headers().get("mcp-session-id")
        {
            *self.session.lock().expect("the session is never poisoned") =
                session.to_str().ok().map(str::to_owned);
        }
        let answer = response.json::<Value>().await?;
        Ok((status.as_u16(), Some(answer)))
    }
}

/// A tool call gets an identity of its own, the same each time it's sent.
fn add_operation(message: &mut Map<String, Value>) {
    let params = message
        .entry("params")
        .or_insert_with(|| Value::Object(Map::new()));
    if !params.is_object() {
        *params = Value::Object(Map::new());
    }
    let params = params.as_object_mut().expect("params are an object");
    let meta = params
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    if !meta.is_object() {
        *meta = Value::Object(Map::new());
    }
    meta.as_object_mut()
        .expect("_meta is an object")
        .insert("kuvo.operation".into(), operation_id().into());
}

/// A random (version 4) UUID.
fn operation_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system supplies random bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Whether a JSON value counts as set, as the relay's JavaScript reads it.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

fn didnt_answer() -> String {
    "Kuvo Code's tools didn't answer.".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_cancellation_without_a_request_id_is_ignored() {
        let (output, mut answers) = mpsc::channel(1);
        let waiting = CancellationToken::new();
        let shared = Arc::new(Shared {
            relay: Relay {
                url: "http://127.0.0.1:0".into(),
                key: "fixture".into(),
                timeout: Duration::from_secs(1),
            },
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            session: Mutex::new(None),
            calls: Mutex::new(HashMap::from([(key_of(&json!(1)), waiting.clone())])),
            output,
        });
        // Execute the actual notification handler so a detached task panic
        // cannot be mistaken for a successful ignored notification.
        for notice in [
            json!({"jsonrpc":"2.0","method":"notifications/cancelled"}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":null}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":null}}),
        ] {
            shared.clone().handle(notice.to_string()).await;
            assert!(!waiting.is_cancelled());
            assert!(answers.try_recv().is_err());
        }
    }
}
