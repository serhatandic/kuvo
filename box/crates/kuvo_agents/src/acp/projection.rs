//! A turn as an ACP agent tells it, made into the turn's events: its words and thinking in steps,
//! its tool calls merged update by update, its to-do list, how full its context is and what it
//! cost.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use crate::{
    file_change::file_change,
    session::{TextKind, ToolOutput, ToolState, TurnError, TurnEvent, Usage},
};

use super::{
    flavour::Flavour,
    tools::{self, Call, KnownTools},
    wire::Update,
};

/// How a read Kuvo did for the agent went: what it read, or why it couldn't.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Served {
    Text(String),
    Error(String),
}

/// What the projection needs of the conversation.
pub(super) struct Context<'a> {
    pub flavour: &'a dyn Flavour,
    pub directory: &'a Path,
    pub model: &'a str,
    /// The agent's session, for files it made.
    pub native: &'a str,
    pub known: &'a KnownTools,
}

/// A subagent's session the agent keeps on the same connection: its task, then its row.
enum Child {
    Task(String),
    Row(String),
}

#[derive(Default)]
pub(super) struct Projection {
    /// When the turn began (ms).
    pub started_ms: u64,
    /// The turn asked for a summary (`/compact`).
    pub compacting: bool,
    calls: HashMap<String, Call>,
    shown: HashSet<String>,
    /// Calls whose end Kuvo showed already (a read it did for the agent).
    settled: HashSet<String>,
    tools_since_text: bool,
    /// What it said since its last tool, the end of it, and how long that was.
    pub said: String,
    pub said_length: usize,
    /// It said something of this turn.
    pub heard: bool,
    plan: Option<(String, Vec<Value>)>,
    used: Option<u64>,
    window: Option<u64>,
    compacted: bool,
    served: HashMap<PathBuf, VecDeque<Served>>,
    /// Calls Kuvo said no to, failed with this unless the agent says how they ended.
    pub declined: HashMap<String, String>,
    children: HashMap<String, Child>,
    synthetic: u64,
    /// What it cost the conversation as it last said, and for which session.
    pub cost: Option<(String, f64)>,
}

impl Projection {
    pub fn new(started_ms: u64, compacting: bool) -> Self {
        Self {
            started_ms,
            compacting,
            ..Self::default()
        }
    }

    pub fn has_tools(&self) -> bool {
        !self.calls.is_empty()
    }

    pub fn call(&self, id: &str) -> Option<&Call> {
        self.calls.get(id)
    }

    fn synthetic_id(&mut self, name: &str) -> String {
        self.synthetic += 1;
        format!("acp-{name}-{}", self.synthetic)
    }

    /// A step's usage, as the context gauge reads it: what the conversation fills.
    fn step_usage(&self) -> Option<Usage> {
        self.used.map(|input| Usage {
            input,
            ..Usage::default()
        })
    }

    /// An update of the agent's own session.
    pub fn update(&mut self, update: &Update, cx: &Context) -> Vec<TurnEvent> {
        self.heard = true;
        let mut events = Vec::new();
        match update {
            Update::Chunk { thought, content } => {
                let text = tools::content_text(content);
                if !thought {
                    if self.tools_since_text {
                        self.said.clear();
                        self.said_length = 0;
                    }
                    self.said_length += text.len();
                    self.said.push_str(&text);
                    let keep = self.said.len().saturating_sub(2_000);
                    let keep = (keep..=self.said.len())
                        .find(|at| self.said.is_char_boundary(*at))
                        .unwrap_or(0);
                    self.said.drain(..keep);
                }
                self.words(&mut events, cx);
                self.content(
                    &mut events,
                    content,
                    if *thought {
                        TextKind::Reasoning
                    } else {
                        TextKind::Text
                    },
                );
            }
            Update::Tool(update) => self.tool(&mut events, update, cx),
            Update::Usage { size, used, cost } => {
                if let Some(size) = size {
                    self.window = Some(*size);
                    cx.flavour.note_context(cx.model, *size);
                }
                if used.is_some() {
                    self.used = *used;
                }
                events.push(TurnEvent::Context {
                    used: *used,
                    window: self.window,
                });
                if let Some(total) = cost {
                    events.push(TurnEvent::Cost(self.cost_now(cx.native, *total)));
                }
            }
            Update::Mode(mode) => events.push(TurnEvent::Notice(format!(
                "{} switched to its {mode} mode.",
                cx.flavour.name()
            ))),
            Update::Notice(said) => events.push(TurnEvent::Notice(said.clone())),
            Update::Compacted => {
                if !self.compacted && !self.compacting {
                    events.extend([
                        TurnEvent::ContextCleared,
                        TurnEvent::CompactionEnded { succeeded: true },
                    ]);
                }
                self.compacted = true;
            }
            Update::AutoCompactStarted { window, used } => {
                if let Some(window) = window {
                    self.window = Some(*window);
                    cx.flavour.note_context(cx.model, *window);
                }
                events.push(TurnEvent::Context {
                    used: *used,
                    window: self.window,
                });
                if !self.compacting {
                    events.push(TurnEvent::CompactionStarted);
                }
            }
            Update::AutoCompactCompleted { after } => {
                events.push(match after {
                    Some(after) => TurnEvent::Context {
                        used: Some(*after),
                        window: self.window,
                    },
                    None => TurnEvent::ContextCleared,
                });
                if !self.compacting {
                    events.push(TurnEvent::CompactionEnded { succeeded: true });
                }
            }
            Update::AutoCompactEnded => {
                if !self.compacting {
                    events.push(TurnEvent::CompactionEnded { succeeded: false });
                }
            }
            Update::Plan(entries) => {
                let todos: Vec<Value> = entries
                    .iter()
                    .map(|todo| json!({"content": todo.content, "status": todo.status}))
                    .collect();
                let id = match &self.plan {
                    Some((id, _)) => id.clone(),
                    None => self.synthetic_id("todowrite"),
                };
                events.push(tool_event(
                    &id,
                    "todowrite",
                    ToolState::Running {
                        input: json!({"todos": todos}),
                    },
                    Map::new(),
                ));
                self.shown.insert(id.clone());
                self.plan = Some((id, todos));
            }
            Update::Unknown { kind, words } => {
                if !words.is_empty() {
                    events.push(TurnEvent::Notice(words.clone()));
                }
                eprintln!(
                    "{} sent {kind}, which Kuvo Code doesn't know.",
                    cx.flavour.name()
                );
            }
            Update::UserChunk(_) | Update::ConfigOptions(_) | Update::Ignored => {}
        }
        events
    }

    /// What the conversation cost since it last said (its total, said again as it grows).
    fn cost_now(&mut self, native: &str, total: f64) -> f64 {
        let before = match &self.cost {
            Some((session, amount)) if session == native => *amount,
            _ => 0.0,
        };
        self.cost = Some((native.to_owned(), total));
        if total >= before {
            total - before
        } else {
            total
        }
    }

    /// What it said, with pictures shown as such, and links as links.
    fn content(&mut self, events: &mut Vec<TurnEvent>, content: &Value, kind: TextKind) {
        if let Some(picture) = tools::content_picture(content) {
            let id = self.synthetic_id("generate_image");
            self.tools_since_text = true;
            events.push(tool_event(
                &id,
                "generate_image",
                ToolState::Running { input: json!({}) },
                Map::new(),
            ));
            events.push(tool_event(
                &id,
                "generate_image",
                ToolState::Completed {
                    input: json!({}),
                    content: vec![picture],
                },
                Map::new(),
            ));
            return;
        }
        let text = match content["type"].as_str() {
            Some("resource_link") => {
                let name = [&content["name"], &content["title"], &content["uri"]]
                    .into_iter()
                    .find_map(Value::as_str)
                    .unwrap_or("");
                format!("[{name}]({})", content["uri"].as_str().unwrap_or(""))
            }
            Some("audio") => "(a sound clip)".into(),
            _ => tools::content_text(content),
        };
        if !text.is_empty() {
            events.push(TurnEvent::TextDelta {
                kind,
                block: None,
                delta: text,
            });
        }
    }

    /// Words after a tool begin a new step.
    fn words(&mut self, events: &mut Vec<TurnEvent>, cx: &Context) {
        if !self.tools_since_text {
            return;
        }
        self.tools_since_text = false;
        self.settle_reads(events, false, cx);
        events.push(TurnEvent::StepEnded {
            usage: self.step_usage(),
            finish: None,
            error: None,
        });
        events.push(TurnEvent::StepStarted { model: None });
    }

    fn tool(&mut self, events: &mut Vec<TurnEvent>, update: &Call, cx: &Context) {
        let id = tools::id(update).to_owned();
        if id.is_empty() {
            return;
        }
        let call = self.calls.entry(id.clone()).or_default();
        tools::merge(call, update);
        let call = call.clone();
        if cx.flavour.hides(&call) || self.settled.contains(&id) {
            return;
        }
        let (name, input) = tools::kuvo_tool(&call, cx.known);
        let new = self.shown.insert(id.clone());
        if new {
            // It went on to something else: the reads before are over.
            if tools::kind(&call) != "read" {
                self.settle_reads(events, false, cx);
            }
            self.tools_since_text = true;
        }
        let (output, mut pictures) = tools::tool_output(&call);
        let mut metadata = Map::new();
        let files: Vec<Value> = call
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "diff" && part["path"].is_string())
            .map(|part| {
                file_change(
                    part["path"].as_str().unwrap_or(""),
                    part["oldText"].as_str(),
                    Some(part["newText"].as_str().unwrap_or("")),
                )
            })
            .collect();
        if !files.is_empty() {
            metadata.insert("files".into(), files.into());
        }
        let raw_output = call.get("rawOutput").unwrap_or(&Value::Null);
        if let Some(exit) = raw_output["exit_code"]
            .as_i64()
            .or_else(|| raw_output["exitCode"].as_i64())
        {
            metadata.insert("exit".into(), exit.into());
        }
        if !output.is_empty() {
            metadata.insert("output".into(), output.clone().into());
        }
        let text = |content: Vec<ToolOutput>| -> Vec<ToolOutput> {
            if output.is_empty() {
                content
            } else {
                [vec![ToolOutput::Text(output.clone())], content].concat()
            }
        };
        let state = match tools::status(&call) {
            "completed" => {
                pictures.extend(cx.flavour.made_files(&call, cx.native, self.started_ms));
                ToolState::Completed {
                    input,
                    content: text(pictures),
                }
            }
            "failed" => failed(
                input,
                if output.is_empty() {
                    "The tool failed."
                } else {
                    &output
                },
            ),
            "cancelled" => {
                let why = self.declined.get(&id).map_or("Stopped.", String::as_str);
                failed(input, if output.is_empty() { why } else { &output })
            }
            _ => ToolState::Running { input },
        };
        let ended = !matches!(state, ToolState::Running { .. });
        events.push(tool_event(&id, &name, state, metadata));
        if ended {
            self.declined.remove(&id);
        }
        // A file Kuvo read for it before it said which call read it.
        if tools::open_read(&call)
            && tools::client_read(&call)
            && let Some(path) = self.read_file(&call, cx)
            && let Some(served) = self.served.get_mut(&path).and_then(VecDeque::pop_front)
        {
            self.settle_read(events, &id, served);
        }
    }

    fn read_file(&self, call: &Call, cx: &Context) -> Option<PathBuf> {
        tools::read_path(call).map(|path| cx.directory.join(path))
    }

    /// Its reads are over by now (Antigravity says so only as the turn ends): one Kuvo read for it
    /// shows what was read, one of a file that isn't there failed, the rest are done; one it
    /// hasn't named the file of waits (`all`: not as the turn ends).
    fn settle_reads(&mut self, events: &mut Vec<TurnEvent>, all: bool, cx: &Context) {
        if !cx.flavour.reads_unsaid() {
            return;
        }
        let open: Vec<(String, Call)> = self
            .calls
            .iter()
            .filter(|(id, call)| {
                tools::open_read(call) && !self.settled.contains(*id) && self.shown.contains(*id)
            })
            .map(|(id, call)| (id.clone(), call.clone()))
            .collect();
        for (id, call) in open {
            let path = self.read_file(&call, cx);
            if path.is_none() && !all {
                continue;
            }
            let served = path
                .as_ref()
                .filter(|_| tools::client_read(&call))
                .and_then(|path| self.served.get_mut(path).and_then(VecDeque::pop_front));
            let served = served.unwrap_or_else(|| match &path {
                Some(path) if !path.exists() => Served::Error("There's no such file.".into()),
                _ => Served::Text(String::new()),
            });
            self.settle_read(events, &id, served);
        }
    }

    fn settle_read(&mut self, events: &mut Vec<TurnEvent>, id: &str, served: Served) {
        self.settled.insert(id.to_owned());
        let call = self.calls.get(id).cloned().unwrap_or_default();
        let (name, input) = tools::kuvo_tool(&call, &KnownTools::default());
        let state = match served {
            Served::Error(why) => failed(input, &why),
            Served::Text(text) => ToolState::Completed {
                input,
                content: vec![ToolOutput::Text(text)],
            },
        };
        events.push(tool_event(id, &name, state, Map::new()));
    }

    /// What Kuvo read for the agent: the read it was for shows it, now or once the agent names it.
    pub fn served(&mut self, path: &Path, served: Served, cx: &Context) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        let open = self.calls.iter().find(|(id, call)| {
            tools::open_read(call)
                && !self.settled.contains(*id)
                && tools::client_read(call)
                && self.read_file(call, cx).as_deref() == Some(path)
        });
        match open.map(|(id, _)| id.clone()) {
            Some(id) => self.settle_read(&mut events, &id, served),
            None => self
                .served
                .entry(path.to_owned())
                .or_default()
                .push_back(served),
        }
        events
    }

    /// Another session's update: a subagent's, kept by the agent on the same connection. Its task
    /// comes first, as its "user" message; then its row. (Its own steps are 4.6's.)
    pub fn child(&mut self, session: &str, update: &Update) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        match (self.children.get_mut(session), update) {
            (Some(Child::Row(_)), _) => {}
            (task, Update::UserChunk(content)) => {
                let text = tools::content_text(content);
                match task {
                    Some(Child::Task(prompt)) => prompt.push_str(&text),
                    _ => {
                        self.children.insert(session.to_owned(), Child::Task(text));
                    }
                }
            }
            (task, _) => {
                let prompt = match task {
                    Some(Child::Task(prompt)) => prompt.trim().to_owned(),
                    _ => String::new(),
                };
                let row = format!("acp-subagent:{session}");
                let title = Some(tools::first_line(&prompt))
                    .filter(|line| !line.is_empty())
                    .unwrap_or_else(|| "A subagent's task".into());
                self.tools_since_text = true;
                events.push(tool_event(
                    &row,
                    "subagent",
                    ToolState::Running {
                        input: json!({"description": title, "prompt": prompt}),
                    },
                    Map::new(),
                ));
                self.children.insert(session.to_owned(), Child::Row(row));
            }
        }
        events
    }

    /// The turn ends: its to-do list done, declined calls failed, reads settled, subagents done,
    /// and its last step ended.
    pub fn end(&mut self, cx: &Context) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        if let Some((id, todos)) = self.plan.take() {
            events.push(tool_event(
                &id,
                "todowrite",
                ToolState::Completed {
                    input: json!({"todos": todos}),
                    content: vec![],
                },
                Map::new(),
            ));
        }
        for (_, child) in self.children.drain() {
            if let Child::Row(row) = child {
                events.push(tool_event(
                    &row,
                    "subagent",
                    ToolState::Completed {
                        input: json!({}),
                        content: vec![],
                    },
                    Map::new(),
                ));
            }
        }
        let declined: Vec<_> = self.declined.drain().collect();
        for (id, why) in declined {
            let call = self.calls.get(&id).cloned().unwrap_or_default();
            let (name, input) = tools::kuvo_tool(&call, cx.known);
            events.push(tool_event(&id, &name, failed(input, &why), Map::new()));
        }
        self.settle_reads(&mut events, true, cx);
        events.push(TurnEvent::StepEnded {
            usage: self.step_usage(),
            finish: None,
            error: None,
        });
        events
    }
}

fn failed(input: Value, why: &str) -> ToolState {
    ToolState::Failed {
        input,
        content: vec![ToolOutput::Text(why.into())],
        error: TurnError {
            kind: "tool".into(),
            message: why.into(),
            status: None,
            limit: None,
        },
    }
}

fn tool_event(id: &str, name: &str, state: ToolState, metadata: Map<String, Value>) -> TurnEvent {
    TurnEvent::Tool {
        id: id.into(),
        name: name.into(),
        state,
        metadata,
    }
}

#[cfg(test)]
mod tests;
