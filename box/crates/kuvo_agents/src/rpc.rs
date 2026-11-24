//! JSON-RPC over a held agent's stdio, one message per line. Codex's
//! app-server speaks it without the `jsonrpc` field and ACP agents (Grok,
//! Antigravity) with it. The agent's own requests (an approval, a question)
//! reach the adapter as [`Incoming::Request`] and are answered with
//! [`Rpc::respond`].
//!
//! The connection is one task that owns the holder's input and output, so
//! request IDs, pending answers, acknowledgements and the stderr tail have a
//! single owner. Adapters hold cheap [`Rpc`] handles.

use crate::hold::{Controller, Output};
use serde_json::{Map, Value, json};
use std::{collections::HashMap, fmt, io, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, sleep_until},
};

/// Whether each message carries `"jsonrpc": "2.0"`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Framing {
    Bare,
    JsonRpc2,
}

/// An error answer, the agent's or the adapter's.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// The answer to a method the adapter doesn't handle.
    pub fn not_found(method: &str) -> Self {
        Self::new(-32601, format!("Method not found: {method}"))
    }

    fn read(value: &Value) -> Self {
        Self {
            code: value["code"].as_i64().unwrap_or(-32000),
            message: value["message"]
                .as_str()
                .unwrap_or("The agent refused.")
                .to_owned(),
            data: value.get("data").cloned(),
        }
    }
}

/// Why a request has no result.
#[derive(Debug)]
pub enum Failure {
    /// The agent isn't running (any more). Says what it last wrote to stderr.
    Closed(String),
    Refused(RpcError),
    TookTooLong(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed(why) => out.write_str(why),
            Self::Refused(error) => out.write_str(&error.message),
            Self::TookTooLong(method) => write!(out, "{method} took too long."),
        }
    }
}

impl std::error::Error for Failure {}

impl From<Failure> for io::Error {
    fn from(failure: Failure) -> Self {
        let kind = match failure {
            Failure::Closed(_) => io::ErrorKind::BrokenPipe,
            Failure::Refused(_) => io::ErrorKind::Other,
            Failure::TookTooLong(_) => io::ErrorKind::TimedOut,
        };
        io::Error::new(kind, failure)
    }
}

/// What the agent sent that isn't an answer to this connection's request.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Notification {
        method: String,
        params: Value,
    },
    /// Answer it with [`Rpc::respond`] and the same `id`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// An answer to a request an earlier resident sent the held agent.
    Stray {
        id: Value,
        answer: Result<Value, RpcError>,
    },
    /// The agent ended. `code` is -1 when another resident took it over.
    Exited {
        code: i32,
        signal: Option<String>,
    },
}

/// How the connection starts.
pub struct Options {
    pub framing: Framing,
    /// The first ID this connection's requests use: past an earlier
    /// resident's, so their answers are told apart.
    pub first_id: u64,
    /// Only lines that pass reach the protocol (an agent's other output).
    pub filter: Option<LineFilter>,
}

/// Which of an agent's lines are protocol messages.
pub type LineFilter = Box<dyn Fn(&str) -> bool + Send>;

impl Default for Options {
    fn default() -> Self {
        Self {
            framing: Framing::Bare,
            first_id: 1,
            filter: None,
        }
    }
}

/// What the connection asks the holder to do.
#[derive(Debug, PartialEq)]
pub(crate) enum Write {
    Line(String),
    Ack(u64),
    Signal(String),
    Bye,
}

enum Command {
    Request {
        method: String,
        params: Option<Value>,
        answer: oneshot::Sender<Result<Value, Failure>>,
        id: oneshot::Sender<u64>,
    },
    Forget(u64),
    Notify {
        method: String,
        params: Option<Value>,
    },
    Respond {
        id: Value,
        answer: Result<Value, RpcError>,
    },
    Stop,
}

/// A handle on the connection. Dropping every handle leaves the agent
/// running under its holder for the next resident.
#[derive(Clone)]
pub struct Rpc {
    commands: mpsc::Sender<Command>,
    stderr: watch::Receiver<String>,
}

/// How often taken-in lines are acknowledged, so the holder can prune them.
const ACK_EVERY: Duration = Duration::from_millis(200);
const STDERR_TAIL: usize = 8_000;

impl Rpc {
    /// Speak JSON-RPC with the agent behind `controller`.
    pub fn over(controller: Controller, options: Options) -> (Self, mpsc::Receiver<Incoming>) {
        let Controller {
            welcome,
            mut input,
            mut output,
        } = controller;
        let (frames, read) = mpsc::channel(64);
        tokio::spawn(async move {
            while let Ok(frame) = output.next().await {
                if frames.send(frame).await.is_err() {
                    return;
                }
            }
        });
        let (writes, mut written) = mpsc::channel::<Write>(64);
        tokio::spawn(async move {
            while let Some(write) = written.recv().await {
                let sent = match write {
                    Write::Line(line) => input.write(line).await,
                    Write::Ack(sequence) => input.acknowledge(sequence).await,
                    Write::Signal(signal) => input.signal(&signal).await,
                    Write::Bye => input.bye().await,
                };
                if sent.is_err() {
                    return;
                }
            }
        });
        let early = welcome.stderr.clone();
        Self::over_frames(read, writes, options, early)
    }

    pub(crate) fn over_frames(
        frames: mpsc::Receiver<Output>,
        writes: mpsc::Sender<Write>,
        options: Options,
        early_stderr: String,
    ) -> (Self, mpsc::Receiver<Incoming>) {
        let (commands, requests) = mpsc::channel(32);
        let (incoming, received) = mpsc::channel(64);
        let (stderr, tail) = watch::channel(String::new());
        let mut connection = Connection {
            options,
            writes,
            incoming,
            stderr,
            next: 0,
            pending: HashMap::new(),
            seen: 0,
            acked: 0,
            closed: false,
            fenced: false,
        };
        connection.next = connection.options.first_id.max(1);
        connection.stderr_text(early_stderr);
        tokio::spawn(connection.run(frames, requests));
        (
            Self {
                commands,
                stderr: tail,
            },
            received,
        )
    }

    /// Ask the agent; `timeout` bounds the wait for its answer.
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, Failure> {
        let (answer, answered) = oneshot::channel();
        let (id, numbered) = oneshot::channel();
        self.commands
            .send(Command::Request {
                method: method.into(),
                params,
                answer,
                id,
            })
            .await
            .map_err(|_| self.closed())?;
        let Some(limit) = timeout else {
            return answered.await.unwrap_or_else(|_| Err(self.closed()));
        };
        match tokio::time::timeout(limit, answered).await {
            Ok(answer) => answer.unwrap_or_else(|_| Err(self.closed())),
            Err(_) => {
                if let Ok(id) = numbered.await {
                    let _ = self.commands.send(Command::Forget(id)).await;
                }
                Err(Failure::TookTooLong(method.into()))
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) {
        let _ = self
            .commands
            .send(Command::Notify {
                method: method.into(),
                params,
            })
            .await;
    }

    /// Answer the agent's request `id`.
    pub async fn respond(&self, id: Value, answer: Result<Value, RpcError>) {
        let _ = self.commands.send(Command::Respond { id, answer }).await;
    }

    /// Stop the agent: its holder ends it and its process group.
    pub async fn stop(&self) {
        let _ = self.commands.send(Command::Stop).await;
    }

    /// The last of what the agent wrote to stderr.
    pub fn stderr(&self) -> String {
        self.stderr.borrow().clone()
    }

    fn closed(&self) -> Failure {
        Failure::Closed(closing_words(&self.stderr(), None))
    }
}

/// The last three lines it wrote to stderr, or that it stopped.
fn closing_words(stderr: &str, code: Option<i32>) -> String {
    let lines = stderr.trim().lines().collect::<Vec<_>>();
    let tail = lines[lines.len().saturating_sub(3)..].join("\n");
    if !tail.is_empty() {
        return tail;
    }
    match code {
        Some(code) => format!("The agent stopped (exit code {code})."),
        None => "The agent isn't running.".into(),
    }
}

struct Connection {
    options: Options,
    writes: mpsc::Sender<Write>,
    incoming: mpsc::Sender<Incoming>,
    stderr: watch::Sender<String>,
    next: u64,
    pending: HashMap<u64, oneshot::Sender<Result<Value, Failure>>>,
    seen: u64,
    acked: u64,
    closed: bool,
    fenced: bool,
}

impl Connection {
    async fn run(mut self, mut frames: mpsc::Receiver<Output>, mut commands: mpsc::Receiver<Command>) {
        let mut ack_at: Option<Instant> = None;
        let mut reading = true;
        loop {
            tokio::select! {
                frame = frames.recv(), if reading => match frame {
                    Some(frame) => {
                        if self.frame(frame).await && ack_at.is_none() {
                            ack_at = Some(Instant::now() + ACK_EVERY);
                        }
                    }
                    None => {
                        reading = false;
                        // Gone with no word of its agent's end: lost, unless a
                        // newer resident took it over (then it's that one's).
                        if !self.fenced && !self.closed {
                            self.close(Some(-1));
                            let _ = self
                                .incoming
                                .send(Incoming::Exited { code: -1, signal: Some("lost".into()) })
                                .await;
                        }
                    }
                },
                () = async { sleep_until(ack_at.expect("an ack is due")).await }, if ack_at.is_some() => {
                    ack_at = None;
                    self.acknowledge().await;
                }
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => return,
                },
            }
        }
    }

    /// Handles one holder frame. True when it took in a new line.
    async fn frame(&mut self, frame: Output) -> bool {
        match frame {
            Output::Line {
                sequence,
                line,
                again,
            } => {
                // Each line once: what comes again is only what it asked and
                // wasn't answered.
                if sequence <= self.seen && !again {
                    return false;
                }
                self.seen = self.seen.max(sequence);
                let line = line.strip_suffix('\r').unwrap_or(&line);
                if !line.trim().is_empty()
                    && self.options.filter.as_ref().is_none_or(|keep| keep(line))
                {
                    self.receive(line).await;
                }
                true
            }
            Output::Stderr { text } => {
                self.stderr_text(text);
                false
            }
            Output::Exited { exit } => {
                let lost = matches!(exit.signal.as_deref(), Some("fenced" | "lost" | "detached"));
                let code = exit.code.unwrap_or(if lost { -1 } else { 1 });
                self.close(Some(code));
                self.acknowledge().await;
                let _ = self.writes.send(Write::Bye).await;
                let _ = self
                    .incoming
                    .send(Incoming::Exited {
                        code,
                        signal: exit.signal,
                    })
                    .await;
                false
            }
            Output::Fenced { .. } => {
                // A newer resident took the agent: its answers are that one's.
                self.closed = true;
                self.fenced = true;
                self.pending.clear();
                false
            }
            Output::Prepared { .. } | Output::Refused { .. } => false,
        }
    }

    async fn receive(&mut self, line: &str) {
        if self.closed {
            return;
        }
        let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = message.remove("id");
        let params = message.remove("params").unwrap_or(Value::Null);
        let event = match (method, id) {
            (Some(method), Some(id)) => Incoming::Request { id, method, params },
            (Some(method), None) => Incoming::Notification { method, params },
            (None, Some(id)) => {
                let answer = match message.get("error") {
                    Some(error) if !error.is_null() => Err(RpcError::read(error)),
                    _ => Ok(message.remove("result").unwrap_or(Value::Null)),
                };
                let Some(waiting) = id.as_u64().and_then(|own| self.pending.remove(&own)) else {
                    let _ = self.incoming.send(Incoming::Stray { id, answer }).await;
                    return;
                };
                let _ = waiting.send(answer.map_err(Failure::Refused));
                return;
            }
            (None, None) => return,
        };
        let _ = self.incoming.send(event).await;
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Request {
                method,
                params,
                answer,
                id,
            } => {
                if self.closed {
                    let _ = answer.send(Err(Failure::Closed(closing_words(
                        &self.stderr.borrow(),
                        None,
                    ))));
                    return;
                }
                let own = self.next;
                self.next += 1;
                let _ = id.send(own);
                self.pending.insert(own, answer);
                let mut message = Map::new();
                message.insert("id".into(), own.into());
                message.insert("method".into(), method.into());
                if let Some(params) = params {
                    message.insert("params".into(), params);
                }
                self.write(message).await;
            }
            Command::Forget(id) => {
                self.pending.remove(&id);
            }
            Command::Notify { method, params } => {
                let mut message = Map::new();
                message.insert("method".into(), method.into());
                if let Some(params) = params {
                    message.insert("params".into(), params);
                }
                self.write(message).await;
            }
            Command::Respond { id, answer } => {
                let mut message = Map::new();
                message.insert("id".into(), id);
                match answer {
                    Ok(result) => {
                        message.insert("result".into(), result);
                    }
                    Err(error) => {
                        message.insert(
                            "error".into(),
                            json!({ "code": error.code, "message": error.message }),
                        );
                    }
                }
                self.write(message).await;
            }
            Command::Stop => {
                if !self.closed {
                    let _ = self.writes.send(Write::Signal("SIGTERM".into())).await;
                }
            }
        }
    }

    async fn write(&mut self, message: Map<String, Value>) {
        if self.closed {
            return;
        }
        let message = match self.options.framing {
            Framing::Bare => message,
            Framing::JsonRpc2 => {
                let mut framed = Map::new();
                framed.insert("jsonrpc".into(), "2.0".into());
                framed.extend(message);
                framed
            }
        };
        let line = Value::Object(message).to_string();
        if self.writes.send(Write::Line(line)).await.is_err() {
            self.close(None);
        }
    }

    async fn acknowledge(&mut self) {
        if self.seen > self.acked {
            self.acked = self.seen;
            let _ = self.writes.send(Write::Ack(self.seen)).await;
        }
    }

    fn close(&mut self, code: Option<i32>) {
        self.closed = true;
        let why = closing_words(&self.stderr.borrow(), code);
        for (_, waiting) in self.pending.drain() {
            let _ = waiting.send(Err(Failure::Closed(why.clone())));
        }
    }

    fn stderr_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.stderr.send_modify(|tail| {
            tail.push_str(&text);
            let mut cut = tail.len().saturating_sub(STDERR_TAIL);
            while !tail.is_char_boundary(cut) {
                cut += 1;
            }
            tail.drain(..cut);
        });
    }
}

#[cfg(test)]
mod tests;
