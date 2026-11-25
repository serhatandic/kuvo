//! What a Codex thread says, as the conversation's steps: words, thinking, its commands, changes
//! and tools, its to-do list, and what Codex says for you to read.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::{
    file_change::{file_change, patch_change},
    pictures::{picture_data, picture_file, picture_url},
    session::{TextKind, ToolOutput, ToolState, TurnError, TurnEvent, Usage},
};

use super::citations::{Citations, Said};

pub(super) const OUTPUT_LIMIT: usize = 30_000;
pub(super) const TOOLS_SERVER: &str = "kuvo-browser";

/// Codex's items shown here, or left out on purpose (your own message, a hook's prompt to the
/// model). Any other is shown as a step of its own, so a newer Codex's isn't lost.
const KNOWN_ITEMS: [&str; 19] = [
    "userMessage",
    "hookPrompt",
    "agentMessage",
    "reasoning",
    "plan",
    "commandExecution",
    "fileChange",
    "mcpToolCall",
    "dynamicToolCall",
    "collabAgentToolCall",
    "subAgentActivity",
    "webSearch",
    "imageView",
    "sleep",
    "imageGeneration",
    "functionCallOutput",
    "enteredReviewMode",
    "exitedReviewMode",
    "contextCompaction",
];

struct Tool {
    name: String,
    input: Value,
    done: bool,
    metadata: Map<String, Value>,
}

/// A Codex file change as kept: a rename says where it went.
#[derive(Clone)]
pub(super) struct Change {
    pub path: String,
    kind: String,
    diff: String,
    pub moved_to: Option<String>,
}

/// One turn's steps, as its items come.
pub(super) struct Lane {
    /// The turn's own ID: synthetic tool IDs from it come out the same when its lines come again.
    turn: String,
    synthetic: u64,
    pub model: String,
    /// Asked to summarize: no step, and no note saying it summarized.
    pub compacting: bool,
    streamed: HashSet<String>,
    pub changes: HashMap<String, Vec<Change>>,
    output: HashMap<String, String>,
    tools_since_text: bool,
    summarized: HashSet<String>,
    reasoned: HashSet<String>,
    pub plan: Option<(String, Value)>,
    noticed: HashSet<String>,
    citations: HashMap<String, Citations>,
    tools: HashMap<String, Tool>,
    block: Option<(TextKind, u64)>,
    blocks: u64,
    pub out: Vec<TurnEvent>,
}

impl Lane {
    pub fn new(turn: &str, model: &str, compacting: bool) -> Self {
        Self {
            turn: turn.to_owned(),
            synthetic: 0,
            model: model.to_owned(),
            compacting,
            streamed: HashSet::new(),
            changes: HashMap::new(),
            output: HashMap::new(),
            tools_since_text: false,
            summarized: HashSet::new(),
            reasoned: HashSet::new(),
            plan: None,
            noticed: HashSet::new(),
            citations: HashMap::new(),
            tools: HashMap::new(),
            block: None,
            blocks: 0,
            out: Vec::new(),
        }
    }

    pub fn take(&mut self) -> Vec<TurnEvent> {
        std::mem::take(&mut self.out)
    }

    fn synthetic_id(&mut self, prefix: &str) -> String {
        self.synthetic += 1;
        format!("{prefix}call_{}_{}", self.turn, self.synthetic)
    }

    pub fn start_step(&mut self) {
        self.end_step(None);
        self.out.push(TurnEvent::StepStarted { model: None });
    }

    pub fn end_step(&mut self, usage: Option<Usage>) {
        self.end_block();
        self.out.push(TurnEvent::StepEnded {
            usage,
            finish: None,
            error: None,
        });
    }

    pub fn end_block(&mut self) {
        self.block = None;
    }

    fn write(&mut self, kind: TextKind, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let key = match self.block {
            Some((current, key)) if current == kind => key,
            _ => {
                self.blocks += 1;
                self.block = Some((kind, self.blocks));
                self.blocks
            }
        };
        self.out.push(TurnEvent::TextDelta {
            kind,
            block: Some(format!("b{key}")),
            delta: delta.to_owned(),
        });
    }

    pub fn text(&mut self, delta: &str) {
        self.write(TextKind::Text, delta);
    }

    fn reasoning(&mut self, delta: &str) {
        self.write(TextKind::Reasoning, delta);
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.out.push(TurnEvent::Notice(text.into()));
    }

    /// A notice for this turn, once.
    pub fn notice_once(&mut self, key: &str, text: impl Into<String>) {
        if self.noticed.insert(key.to_owned()) {
            self.notice(text);
        }
    }

    pub fn has_tool(&self, id: &str) -> bool {
        self.tools.contains_key(id)
    }

    fn emit(&mut self, id: &str, state: impl FnOnce(Value) -> ToolState) {
        let tool = &self.tools[id];
        self.out.push(TurnEvent::Tool {
            id: id.to_owned(),
            name: tool.name.clone(),
            state: state(tool.input.clone()),
            metadata: tool.metadata.clone(),
        });
    }

    pub fn tool_start(&mut self, id: &str, name: &str, input: Value) {
        if self.tools.contains_key(id) {
            return self.tool_called(id, input);
        }
        self.end_block();
        self.tools.insert(
            id.to_owned(),
            Tool {
                name: name.to_owned(),
                input,
                done: false,
                metadata: Map::new(),
            },
        );
        self.emit(id, |input| ToolState::Running { input });
    }

    fn tool_called(&mut self, id: &str, input: Value) {
        let Some(tool) = self.tools.get_mut(id) else {
            return self.tool_start(id, "tool", input);
        };
        if tool.done {
            return;
        }
        tool.input = input;
        self.emit(id, |input| ToolState::Running { input });
    }

    fn tool_progress(&mut self, id: &str, metadata: Map<String, Value>) {
        let Some(tool) = self.tools.get_mut(id).filter(|tool| !tool.done) else {
            return;
        };
        tool.metadata.extend(metadata);
        self.emit(id, |input| ToolState::Running { input });
    }

    pub fn tool_succeeded(
        &mut self,
        id: &str,
        content: Vec<ToolOutput>,
        metadata: Option<Map<String, Value>>,
    ) {
        let Some(tool) = self.tools.get_mut(id).filter(|tool| !tool.done) else {
            return;
        };
        tool.done = true;
        if let Some(metadata) = metadata {
            tool.metadata = metadata;
        }
        self.emit(id, |input| ToolState::Completed { input, content });
    }

    fn tool_failed(&mut self, id: &str, message: &str, metadata: Option<Map<String, Value>>) {
        let Some(tool) = self.tools.get_mut(id).filter(|tool| !tool.done) else {
            return;
        };
        tool.done = true;
        if let Some(metadata) = metadata {
            tool.metadata = metadata;
        }
        let content = if message.is_empty() {
            Vec::new()
        } else {
            vec![ToolOutput::Text(message.to_owned())]
        };
        let error = TurnError {
            kind: "tool".into(),
            message: message.to_owned(),
            status: None,
            limit: None,
        };
        self.emit(id, |input| ToolState::Failed {
            input,
            content,
            error,
        });
    }

    /// Words after a tool begin a new step, as a model's next answer does.
    fn words(&mut self) {
        if self.tools_since_text {
            self.tools_since_text = false;
            self.end_step(None);
            self.out.push(TurnEvent::StepStarted { model: None });
        }
    }

    /// Its words, with a file it cites as what it made shown in the conversation.
    fn say(&mut self, said: Vec<Said>) {
        for each in said {
            match each {
                Said::Text(text) => self.text(&text),
                Said::Made(path) => {
                    // Part of its message, not a step of its own.
                    let name = format!("{TOOLS_SERVER}_show_file");
                    let id = self.synthetic_id("cited-");
                    self.tool_start(&id, &name, json!({ "path": path }));
                    self.tool_succeeded(&id, text("Shown in the conversation."), None);
                }
            }
        }
    }

    /// The turn ends: its to-do list is done being brought up to date.
    pub fn close_plan(&mut self) {
        if let Some((id, todos)) = self.plan.clone() {
            self.tool_succeeded(&id, text(""), Some(object(json!({ "todos": todos }))));
        }
    }

    /// What a thread says that's the same for the turn's own and a spawned agent's.
    pub fn notification(&mut self, method: &str, params: &Value) {
        let item_id = params["itemId"].as_str().unwrap_or_default();
        let delta = params["delta"].as_str().unwrap_or_default();
        match method {
            "item/agentMessage/delta" => {
                self.words();
                self.streamed.insert(item_id.to_owned());
                let said = self
                    .citations
                    .entry(item_id.to_owned())
                    .or_default()
                    .read(delta);
                self.say(said);
            }
            "item/reasoning/summaryTextDelta" => {
                self.words();
                self.summarized.insert(item_id.to_owned());
                self.reasoning(delta);
            }
            "item/reasoning/summaryPartAdded" => {
                if params["summaryIndex"].as_u64().unwrap_or(0) > 0 {
                    self.reasoning("\n\n");
                }
            }
            "item/reasoning/textDelta" => {
                // Its own thinking, for a model that sends no summary of it.
                if self.summarized.contains(item_id) {
                    return;
                }
                self.words();
                self.reasoned.insert(item_id.to_owned());
                self.reasoning(delta);
            }
            "item/plan/delta" => {
                self.words();
                self.streamed.insert(item_id.to_owned());
                self.text(delta);
            }
            "item/commandExecution/outputDelta" | "item/commandExecution/terminalInteraction" => {
                let added = if method.ends_with("outputDelta") {
                    delta.to_owned()
                } else {
                    // What it typed into the command while it ran.
                    format!("\n> {}\n", params["stdin"].as_str().unwrap_or_default())
                };
                let output = self.output.entry(item_id.to_owned()).or_default();
                output.push_str(&added);
                *output = tail(output, OUTPUT_LIMIT).to_owned();
                let output = output.clone();
                self.tool_progress(item_id, object(json!({ "output": output })));
            }
            "item/fileChange/patchUpdated" => {
                let changes = changes(&params["changes"]);
                let files: Vec<_> = changes.iter().map(change_diff).collect();
                self.changes.insert(item_id.to_owned(), changes);
                self.tool_progress(item_id, object(json!({ "files": files })));
            }
            "item/mcpToolCall/progress" => {
                if let Some(message) = params["message"].as_str() {
                    self.tool_progress(item_id, object(json!({ "output": message })));
                }
            }
            "item/started" => self.item_started(&params["item"]),
            "item/completed" => self.item_completed(&params["item"]),
            "turn/plan/updated" => {
                // Its to-do list, one a turn, brought up to date as it works through it.
                let todos: Vec<Value> = params["plan"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|entry| {
                        let status = match entry["status"].as_str() {
                            Some("inProgress") => "in_progress",
                            Some(other) => other,
                            None => "",
                        };
                        json!({ "content": entry["step"], "status": status })
                    })
                    .collect();
                let id = match &self.plan {
                    Some((id, _)) => id.clone(),
                    None => self.synthetic_id(""),
                };
                self.plan = Some((id.clone(), json!(todos)));
                if self.has_tool(&id) {
                    self.tool_called(&id, json!({ "todos": todos }));
                } else {
                    self.tool_start(&id, "todowrite", json!({ "todos": todos }));
                }
            }
            "thread/tokenUsage/updated" => self.token_usage(params),
            "hook/started" | "hook/completed" => {
                // A hook of yours ran (Codex's settings): a step, with what it said.
                let run = &params["run"];
                let id = match run["id"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| run["id"].as_u64().map(|id| id.to_string()))
                {
                    Some(id) => format!("hook:{id}"),
                    None => self.synthetic_id("hook:"),
                };
                if !self.has_tool(&id) {
                    self.tools_since_text = true;
                    let event = run["eventName"].as_str().unwrap_or("hook");
                    self.tool_start(&id, "hook", json!({ "description": format!("{event} hook"), "source": run["sourcePath"] }));
                }
                if method == "hook/started" {
                    return;
                }
                let said = run["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|entry| {
                        let text = entry["text"].as_str().unwrap_or_default();
                        match entry["kind"].as_str() {
                            Some(kind) if kind != "context" => format!("{kind}: {text}"),
                            _ => text.to_owned(),
                        }
                    })
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let status = run["status"].as_str().unwrap_or_default();
                let message = run["statusMessage"].as_str();
                if status == "failed" || status == "blocked" {
                    let fallback =
                        message.map_or_else(|| format!("The hook {status}."), str::to_owned);
                    self.tool_failed(&id, if said.is_empty() { &fallback } else { &said }, None);
                } else {
                    let said = if said.is_empty() {
                        message.unwrap_or_default().to_owned()
                    } else {
                        said
                    };
                    self.tool_succeeded(&id, text(&said), None);
                }
            }
            "model/rerouted" => {
                let to = params["toModel"].as_str().unwrap_or("Another model");
                let from = params["fromModel"].as_str().unwrap_or("the one you picked");
                let why = if params["reason"] == "highRiskCyberActivity" {
                    " (Codex moves security-sensitive work to it)"
                } else {
                    ""
                };
                self.notice(format!("{to} answered instead of {from}{why}."));
            }
            "model/verification" => {
                let asked: Vec<String> = params["verifications"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|each| {
                        each.as_str()
                            .map_or_else(|| each.to_string(), str::to_owned)
                    })
                    .collect();
                if !asked.is_empty() {
                    self.notice(format!(
                        "Codex asks for verification to go on: {}.",
                        asked.join(", ")
                    ));
                }
            }
            "model/safetyBuffering/updated" => {
                if params["showBufferingUi"] == true {
                    let model = params["model"].as_str().unwrap_or("this model");
                    let faster = params["fasterModel"]
                        .as_str()
                        .map(|faster| format!("; {faster} would be faster"))
                        .unwrap_or_default();
                    self.notice_once(
                        "buffering",
                        format!("Codex is checking {model}'s answer before showing it{faster}."),
                    );
                }
            }
            "modelProvider/authRecoveryStarted"
            | "modelProvider/authRecoveryCompleted"
            | "warning"
            | "guardianWarning" => {
                if let Some(message) = params["message"]
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    self.notice(message);
                }
            }
            "deprecationNotice" | "configWarning" => {
                if let Some(summary) = params["summary"].as_str().filter(|text| !text.is_empty()) {
                    let parts: Vec<&str> = [
                        Some(summary),
                        params["details"].as_str(),
                        params["path"].as_str(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|text| !text.is_empty())
                    .collect();
                    self.notice(parts.join(" — "));
                }
            }
            // Kuvo Code's own tools, there or not: the agent works without them.
            "mcpServer/startupStatus/updated"
                if params["name"] == TOOLS_SERVER && params["status"] == "failed" =>
            {
                let why = params["error"]
                    .as_str()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default();
                self.notice(format!("Kuvo Code's tools didn't start for Codex{why}."));
            }
            _ => {}
        }
    }

    /// Told as a model call ends: its step ends with what it used, and what comes next is the next
    /// call's step. (One with nothing read or written, Codex's estimate after it compacted, isn't.)
    fn token_usage(&mut self, params: &Value) {
        let usage = &params["tokenUsage"];
        let window = usage["modelContextWindow"]
            .as_u64()
            .filter(|window| *window > 0);
        let last = &usage["last"];
        let count = |key: &str| last[key].as_u64();
        let used =
            count("totalTokens").or_else(|| Some(count("inputTokens")? + count("outputTokens")?));
        self.out.push(TurnEvent::Context { used, window });
        let (input, output) = (
            count("inputTokens").unwrap_or(0),
            count("outputTokens").unwrap_or(0),
        );
        if input == 0 && output == 0 {
            return;
        }
        self.tools_since_text = false;
        let cached = count("cachedInputTokens").unwrap_or(0);
        let reasoning = count("reasoningOutputTokens").unwrap_or(0);
        self.end_step(Some(Usage {
            input: input.saturating_sub(cached),
            output: output.saturating_sub(reasoning),
            reasoning,
            cache_read: cached,
            cache_write: count("cacheWriteInputTokens").unwrap_or(0),
            cost: None,
        }));
    }

    fn item_started(&mut self, item: &Value) {
        let id = item["id"].as_str().unwrap_or_default();
        let kind = item["type"].as_str().unwrap_or_default();
        match kind {
            "contextCompaction" => {
                // It summarizes by itself, until the item completes. (One you asked for shows as that.)
                if !self.compacting {
                    self.out.push(TurnEvent::CompactionStarted);
                }
            }
            "sleep" => {
                self.tools_since_text = true;
                let seconds = (item["durationMs"].as_f64().unwrap_or(0.0) / 1000.0).round();
                self.tool_start(id, "sleep", json!({ "seconds": seconds as u64 }));
            }
            "collabAgentToolCall" => {
                self.tools_since_text = true;
                match item["tool"].as_str() {
                    // Waiting for the agents it spawned.
                    Some("wait") => self.tool_start(
                        id,
                        "wait",
                        json!({ "description": "Waiting for its subagents" }),
                    ),
                    // Another agent spawned for part of the work, beside this one.
                    Some("spawnAgent") => {
                        let prompt = item["prompt"].as_str().unwrap_or_default();
                        let title = first_line(prompt);
                        let title = if title.is_empty() {
                            "A spawned agent's task".into()
                        } else {
                            title
                        };
                        self.tool_start(
                            id,
                            "subagent",
                            json!({ "description": title, "prompt": prompt, "background": true }),
                        );
                    }
                    tool => {
                        let mut input = json!({ "description": collab_words(tool) });
                        if let Some(prompt) = item["prompt"].as_str() {
                            input["prompt"] = json!(prompt);
                        }
                        if !item["receiverThreadIds"].is_null() {
                            input["agents"] = item["receiverThreadIds"].clone();
                        }
                        self.tool_start(id, &format!("agent_{}", tool.unwrap_or("action")), input);
                    }
                }
            }
            "subAgentActivity" => {}
            "commandExecution" => {
                self.tools_since_text = true;
                let command = unwrap_shell(item["command"].as_str().unwrap_or_default());
                let mut input = json!({ "command": command });
                if !item["cwd"].is_null() {
                    input["cwd"] = item["cwd"].clone();
                }
                self.tool_start(id, "shell", input);
            }
            "fileChange" => {
                self.tools_since_text = true;
                let changes = changes(&item["changes"]);
                let files: Vec<&str> = changes
                    .iter()
                    .flat_map(|change| {
                        std::iter::once(change.path.as_str()).chain(change.moved_to.as_deref())
                    })
                    .collect();
                let input = json!({ "files": files });
                self.changes.insert(id.to_owned(), changes);
                self.tool_start(id, "patch", input);
            }
            "functionCallOutput" => {
                // A tool of Codex's own with no item of its own: what it came back with.
                self.tools_since_text = true;
                let name: Vec<&str> = [item["namespace"].as_str(), item["name"].as_str()]
                    .into_iter()
                    .flatten()
                    .filter(|part| !part.is_empty())
                    .collect();
                let name = if name.is_empty() {
                    "tool".into()
                } else {
                    name.join(".")
                };
                self.tool_start(id, &name, json!({}));
            }
            "mcpToolCall" => {
                self.tools_since_text = true;
                let name = format!("{}_{}", text_of(&item["server"]), text_of(&item["tool"]));
                self.tool_start(id, &name, or_object(&item["arguments"]));
            }
            "dynamicToolCall" => {
                self.tools_since_text = true;
                self.tool_start(id, &text_of(&item["tool"]), or_object(&item["arguments"]));
            }
            "webSearch" => {
                self.tools_since_text = true;
                let query = item["query"]
                    .as_str()
                    .or_else(|| item["action"]["query"].as_str())
                    .unwrap_or_default();
                self.tool_start(id, "websearch", json!({ "query": query }));
            }
            "imageGeneration" => {
                self.tools_since_text = true;
                self.tool_start(
                    id,
                    "generate_image",
                    json!({ "prompt": item["revisedPrompt"].as_str().unwrap_or_default() }),
                );
            }
            "imageView" => {
                self.tools_since_text = true;
                self.tool_start(id, "read", json!({ "path": item["path"] }));
            }
            _ => {
                // One this resident doesn't know (a newer Codex's): shown as a step, not lost.
                if !KNOWN_ITEMS.contains(&kind) && item["id"].is_string() {
                    self.tools_since_text = true;
                    let name = item["type"].as_str().unwrap_or("tool").to_owned();
                    self.tool_start(id, &name, unknown_input(item));
                }
            }
        }
    }

    fn item_completed(&mut self, item: &Value) {
        let id = item["id"].as_str().unwrap_or_default();
        let kind = item["type"].as_str().unwrap_or_default();
        // A step whose start wasn't told (it came whole): started now, so its end isn't lost.
        let whole = [
            "commandExecution",
            "fileChange",
            "mcpToolCall",
            "dynamicToolCall",
            "webSearch",
            "functionCallOutput",
        ];
        if whole.contains(&kind) && item["id"].is_string() && !self.has_tool(id) {
            self.item_started(item);
        }
        match kind {
            "sleep" => {
                let seconds = (item["durationMs"].as_f64().unwrap_or(0.0) / 1000.0).round();
                self.tool_succeeded(id, text(&format!("Waited {seconds} seconds.")), None);
            }
            "collabAgentToolCall" => match item["tool"].as_str() {
                Some("wait") => {
                    // What each agent it waited for said.
                    let states = item["agentsStates"]
                        .as_object()
                        .into_iter()
                        .flat_map(|states| states.values())
                        .map(|state| {
                            [state["status"].as_str(), state["message"].as_str()]
                                .into_iter()
                                .flatten()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(": ")
                        })
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if item["status"] == "failed" {
                        self.tool_failed(
                            id,
                            if states.is_empty() {
                                "It stopped waiting."
                            } else {
                                &states
                            },
                            None,
                        );
                    } else {
                        self.tool_succeeded(
                            id,
                            text(if states.is_empty() {
                                "Done waiting."
                            } else {
                                &states
                            }),
                            None,
                        );
                    }
                }
                Some("spawnAgent") => {
                    if item["status"] == "failed" || item["receiverThreadIds"][0].is_null() {
                        self.tool_failed(id, "The agent couldn't be started.", None);
                    } else {
                        self.tool_succeeded(id, text("Started."), None);
                    }
                }
                tool => {
                    if !self.has_tool(id) {
                        let name = format!("agent_{}", tool.unwrap_or("action"));
                        self.tool_start(id, &name, json!({ "description": collab_words(tool) }));
                    }
                    if item["status"] == "failed" {
                        self.tool_failed(id, "It didn't go through.", None);
                    } else {
                        self.tool_succeeded(id, text(""), None);
                    }
                }
            },
            "agentMessage" => {
                let mut citations = self.citations.remove(id).unwrap_or_default();
                if !self.streamed.contains(id)
                    && let Some(words) = item["text"].as_str().filter(|words| !words.is_empty())
                {
                    self.words();
                    let said = citations.read(words);
                    self.say(said);
                }
                let said = citations.end();
                self.say(said);
                // Questions it leaves for you, and what it remembered and where from.
                let questions: Vec<String> = item["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|question| {
                        let options: Vec<&str> = question["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect();
                        let choices = if options.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", options.join(" / "))
                        };
                        format!(
                            "- {}{choices}",
                            question["title"].as_str().unwrap_or_default()
                        )
                    })
                    .collect();
                if !questions.is_empty() {
                    self.text(&format!("\n\n{}", questions.join("\n")));
                }
                let cited: Vec<String> = item["memoryCitation"]["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|entry| {
                        let note = entry["note"]
                            .as_str()
                            .map(|note| format!(" ({note})"))
                            .unwrap_or_default();
                        format!(
                            "{}:{}-{}{note}",
                            text_of(&entry["path"]),
                            text_of(&entry["lineStart"]),
                            text_of(&entry["lineEnd"])
                        )
                    })
                    .collect();
                if !cited.is_empty() {
                    self.text(&format!("\n\nFrom memory: {}", cited.join("; ")));
                }
                self.end_block();
            }
            "reasoning" => {
                // Thinking that wasn't told as it came: told now.
                if !self.summarized.contains(id) && !self.reasoned.contains(id) {
                    let summary = item["summary"].as_array().filter(|parts| !parts.is_empty());
                    let parts = summary.or_else(|| item["content"].as_array());
                    let said = parts
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter(|part| !part.trim().is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if !said.is_empty() {
                        self.words();
                        self.reasoning(&said);
                    }
                }
                self.end_block();
            }
            "plan" => {
                // Plan mode's proposal, in words (unless it came as it was written).
                if let Some(words) = item["text"].as_str().filter(|words| !words.is_empty())
                    && !self.streamed.contains(id)
                {
                    self.words();
                    self.text(words);
                }
                self.end_block();
            }
            "commandExecution" => {
                let output = item["aggregatedOutput"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| self.output.get(id).cloned())
                    .unwrap_or_default();
                let output = tail(&output, OUTPUT_LIMIT).to_owned();
                if item["status"] == "declined" {
                    self.tool_failed(id, "Declined.", None);
                } else if item["status"] == "failed" && item["exitCode"].is_null() {
                    self.tool_failed(
                        id,
                        if output.is_empty() {
                            "The command failed."
                        } else {
                            &output
                        },
                        None,
                    );
                } else {
                    let exit = item["exitCode"].as_i64().unwrap_or(0);
                    let metadata = object(json!({ "output": output, "exit": exit }));
                    self.tool_succeeded(id, text(&output), Some(metadata));
                }
            }
            "fileChange" => {
                let changes = changes(&item["changes"]);
                let files: Vec<Value> = changes.iter().map(change_diff).collect();
                let said = changes
                    .iter()
                    .map(|change| match &change.moved_to {
                        Some(to) => format!("{} → {to}", change.path),
                        None => change.path.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let metadata = Some(object(json!({ "files": files })));
                match item["status"].as_str() {
                    Some("declined") => self.tool_failed(id, "Declined.", metadata),
                    Some("failed") => {
                        self.tool_failed(id, "The change couldn't be made.", metadata)
                    }
                    _ => self.tool_succeeded(id, text(&said), metadata),
                }
            }
            "mcpToolCall" => {
                let (words, pictures) = mcp_parts(&item["result"]["content"]);
                let structured = &item["result"]["structuredContent"];
                let words = if words.is_empty() && !structured.is_null() {
                    serde_json::to_string_pretty(structured).unwrap_or_default()
                } else {
                    words
                };
                if !item["error"].is_null() || item["status"] == "failed" {
                    let message = item["error"]["message"]
                        .as_str()
                        .filter(|text| !text.is_empty());
                    let message = message.map(str::to_owned).unwrap_or_else(|| {
                        if words.is_empty() {
                            "The tool failed.".into()
                        } else {
                            words.clone()
                        }
                    });
                    self.tool_failed(id, &message, None);
                } else {
                    self.tool_succeeded(
                        id,
                        with_pictures(head(&words, OUTPUT_LIMIT), pictures),
                        None,
                    );
                }
            }
            "dynamicToolCall" => {
                let parts = item["contentItems"].as_array().cloned().unwrap_or_default();
                let words = parts
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let pictures = parts
                    .iter()
                    .filter(|part| part["type"] == "inputImage")
                    .filter_map(|part| picture_url(part["imageUrl"].as_str()?))
                    .collect();
                if item["success"] == false {
                    self.tool_failed(
                        id,
                        if words.is_empty() {
                            "The tool failed."
                        } else {
                            &words
                        },
                        None,
                    );
                } else {
                    self.tool_succeeded(
                        id,
                        with_pictures(head(&words, OUTPUT_LIMIT), pictures),
                        None,
                    );
                }
            }
            "webSearch" => {
                // What it searched for or opened, and what came back.
                let action = &item["action"];
                let mut lines: Vec<String> = Vec::new();
                if let Some(url) = action["url"].as_str() {
                    lines.push(format!("Opened {url}"));
                }
                if let Some(pattern) = action["pattern"].as_str() {
                    lines.push(format!("Looked for \"{pattern}\""));
                }
                for query in action["queries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    lines.push(format!("Searched for \"{query}\""));
                }
                for result in item["results"].as_array().into_iter().flatten() {
                    let found: Vec<&str> = [result["title"].as_str(), result["url"].as_str()]
                        .into_iter()
                        .flatten()
                        .filter(|part| !part.is_empty())
                        .collect();
                    lines.push(found.join(" — "));
                }
                let said = lines
                    .into_iter()
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.tool_succeeded(id, text(head(&said, OUTPUT_LIMIT)), None);
            }
            "imageGeneration" => {
                if !self.has_tool(id) {
                    self.tool_start(
                        id,
                        "generate_image",
                        json!({ "prompt": item["revisedPrompt"].as_str().unwrap_or_default() }),
                    );
                }
                let failure = &item["failure"];
                if item["status"] == "failed" || !failure.is_null() {
                    let message = if failure["type"] == "usageLimitExceeded" {
                        let resets = failure["resetsAt"]
                            .as_i64()
                            .and_then(|at| jiff::Timestamp::from_second(at).ok())
                            .map(|at| format!(" It resets {}.", at.strftime("%Y-%m-%d %H:%M UTC")))
                            .unwrap_or_default();
                        format!("You've reached your limit for making images.{resets}")
                    } else {
                        "The image couldn't be made.".into()
                    };
                    self.tool_failed(id, &message, None);
                } else {
                    let picture = item["savedPath"]
                        .as_str()
                        .and_then(picture_file)
                        .or_else(|| picture_data(item["result"].as_str()?, "image/png"));
                    self.tool_succeeded(id, with_pictures("", picture.into_iter().collect()), None);
                }
            }
            "functionCallOutput" => {
                let output = &item["output"];
                if let Some(words) = output.as_str() {
                    self.tool_succeeded(id, text(head(words, OUTPUT_LIMIT)), None);
                } else {
                    let parts = output.as_array().cloned().unwrap_or_default();
                    let words = parts
                        .iter()
                        .filter_map(|part| part["text"].as_str())
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let pictures = parts
                        .iter()
                        .filter_map(|part| picture_url(part["image_url"].as_str()?))
                        .collect();
                    self.tool_succeeded(
                        id,
                        with_pictures(head(&words, OUTPUT_LIMIT), pictures),
                        None,
                    );
                }
            }
            "imageView" => {
                if !self.has_tool(id) {
                    self.tool_start(id, "read", json!({ "path": item["path"] }));
                }
                let picture = item["path"].as_str().and_then(picture_file);
                self.tool_succeeded(id, with_pictures("", picture.into_iter().collect()), None);
            }
            "contextCompaction" => {
                if !self.compacting {
                    self.out
                        .push(TurnEvent::CompactionEnded { succeeded: true });
                }
            }
            "enteredReviewMode" => {
                let review = item["review"].as_str().filter(|review| !review.is_empty());
                self.notice(review.map_or_else(
                    || "Reviewing".into(),
                    |review| format!("Reviewing: {review}"),
                ));
            }
            "exitedReviewMode" => {
                if let Some(review) = item["review"]
                    .as_str()
                    .filter(|review| !review.trim().is_empty())
                {
                    self.words();
                    self.text(review);
                    self.end_block();
                }
            }
            _ => {
                // One this resident doesn't know: its step ends, with what it says.
                if !KNOWN_ITEMS.contains(&kind) && item["id"].is_string() {
                    if !self.has_tool(id) {
                        let name = item["type"].as_str().unwrap_or("tool").to_owned();
                        self.tool_start(id, &name, unknown_input(item));
                    }
                    let said = unknown_output(item);
                    if item["status"] == "failed" || item["success"] == false {
                        self.tool_failed(
                            id,
                            if said.is_empty() { "It failed." } else { &said },
                            None,
                        );
                    } else {
                        self.tool_succeeded(id, text(&said), None);
                    }
                }
            }
        }
    }
}

fn text(words: &str) -> Vec<ToolOutput> {
    vec![ToolOutput::Text(words.to_owned())]
}

/// A step's words, with its pictures after them.
fn with_pictures(words: &str, pictures: Vec<ToolOutput>) -> Vec<ToolOutput> {
    std::iter::once(ToolOutput::Text(words.to_owned()))
        .chain(pictures)
        .collect()
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn or_object(value: &Value) -> Value {
    if value.is_null() {
        json!({})
    } else {
        value.clone()
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "undefined".into(),
        other => other.to_string(),
    }
}

/// The last `limit` characters of `text`.
pub(super) fn tail(text: &str, limit: usize) -> &str {
    let count = text.chars().count();
    if count <= limit {
        return text;
    }
    let skip = text
        .char_indices()
        .nth(count - limit)
        .map_or(0, |(at, _)| at);
    &text[skip..]
}

/// The first `limit` characters of `text`.
fn head(text: &str, limit: usize) -> &str {
    text.char_indices()
        .nth(limit)
        .map_or(text, |(at, _)| &text[..at])
}

pub(super) fn changes(raw: &Value) -> Vec<Change> {
    raw.as_array()
        .into_iter()
        .flatten()
        .map(|change| Change {
            path: change["path"].as_str().unwrap_or_default().to_owned(),
            kind: change["kind"]["type"]
                .as_str()
                .unwrap_or("update")
                .to_owned(),
            diff: change["diff"].as_str().unwrap_or_default().to_owned(),
            moved_to: change["kind"]["move_path"].as_str().map(str::to_owned),
        })
        .collect()
}

/// A change as the app draws it: Codex gives a new or deleted file's whole text, and an edit's
/// unified diff.
pub(super) fn change_diff(change: &Change) -> Value {
    match change.kind.as_str() {
        "add" => file_change(&change.path, None, Some(&change.diff)),
        "delete" => file_change(&change.path, Some(&change.diff), None),
        _ => patch_change(
            change.moved_to.as_deref().unwrap_or(&change.path),
            &change.diff,
            Some("modified"),
        ),
    }
}

/// An MCP tool's result, as words and pictures (resources and links named, audio said).
fn mcp_parts(content: &Value) -> (String, Vec<ToolOutput>) {
    let mut lines = Vec::new();
    let mut pictures = Vec::new();
    for part in content.as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("text") => {
                if let Some(words) = part["text"].as_str().filter(|words| !words.is_empty()) {
                    lines.push(words.to_owned());
                }
            }
            Some("image") => pictures.extend(picture_data(
                part["data"].as_str().unwrap_or_default(),
                part["mimeType"].as_str().unwrap_or_default(),
            )),
            Some("resource") => {
                let resource = &part["resource"];
                if let Some(words) = resource["text"].as_str() {
                    lines.push(words.to_owned());
                } else if let Some(uri) = resource["uri"].as_str().filter(|uri| !uri.is_empty()) {
                    lines.push(format!("Resource: {uri}"));
                }
                if let Some(blob) = resource["blob"].as_str() {
                    pictures.extend(picture_data(
                        blob,
                        resource["mimeType"].as_str().unwrap_or_default(),
                    ));
                }
            }
            Some("resource_link") => {
                let name = part["name"]
                    .as_str()
                    .or_else(|| part["title"].as_str())
                    .unwrap_or("Link");
                lines.push(format!(
                    "{name}: {}",
                    part["uri"].as_str().unwrap_or_default()
                ));
            }
            Some("audio") => lines.push("(It sent a sound clip.)".into()),
            _ if !part.is_null() => lines.push(
                part["text"]
                    .as_str()
                    .map_or_else(|| part.to_string(), str::to_owned),
            ),
            _ => {}
        }
    }
    (lines.join("\n"), pictures)
}

fn collab_words(tool: Option<&str>) -> String {
    match tool {
        Some("sendInput") => "Wrote to a subagent".into(),
        Some("resumeAgent") => "Resumed a subagent".into(),
        Some("closeAgent") => "Closed a subagent".into(),
        other => format!("Subagents: {}", other.unwrap_or_default()),
    }
}

/// An item this resident doesn't know, as a step's input: what it says, without its type and id.
fn unknown_input(item: &Value) -> Value {
    let mut rest = item.as_object().cloned().unwrap_or_default();
    rest.remove("type");
    rest.remove("id");
    Value::Object(rest)
}

fn unknown_output(item: &Value) -> String {
    for key in ["output", "result", "text", "aggregatedOutput", "message"] {
        match &item[key] {
            Value::String(words) if !words.trim().is_empty() => {
                return head(words, OUTPUT_LIMIT).to_owned();
            }
            value @ (Value::Object(_) | Value::Array(_)) => {
                return head(&value.to_string(), OUTPUT_LIMIT).to_owned();
            }
            _ => {}
        }
    }
    String::new()
}

/// The command as you'd type it: Codex runs each in your login shell (`/bin/zsh -lc "…"`).
pub(super) fn unwrap_shell(command: &str) -> String {
    for shell in ["/bin/zsh -lc ", "/bin/bash -lc ", "/bin/sh -lc "] {
        let Some(quoted) = command.strip_prefix(shell) else {
            continue;
        };
        let Some(quote) = quoted
            .chars()
            .next()
            .filter(|quote| matches!(quote, '"' | '\''))
        else {
            return command.to_owned();
        };
        let Some(inner) = quoted[1..]
            .strip_suffix(quote)
            .filter(|_| quoted.len() >= 2)
        else {
            return command.to_owned();
        };
        if quote == '\'' {
            return inner.replace(r#"'"'"'"#, "'");
        }
        let mut out = String::new();
        let mut chars = inner.chars().peekable();
        while let Some(char) = chars.next() {
            if char == '\\'
                && chars
                    .peek()
                    .is_some_and(|next| matches!(next, '"' | '\\' | '$' | '`'))
            {
                out.extend(chars.next());
            } else {
                out.push(char);
            }
        }
        return out;
    }
    command.to_owned()
}

/// The first line of a prompt, short enough for a title.
fn first_line(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or_default();
    if line.chars().count() > 80 {
        line.chars().take(79).collect::<String>() + "…"
    } else {
        line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_shown_as_typed() {
        assert_eq!(
            unwrap_shell(r#"/bin/zsh -lc "echo \"hi\" \$HOME""#),
            r#"echo "hi" $HOME"#
        );
        assert_eq!(unwrap_shell(r#"/bin/bash -lc 'it'"'"'s'"#), "it's");
        assert_eq!(unwrap_shell("ls -la"), "ls -la");
    }

    #[test]
    fn a_tool_ends_once() {
        let mut lane = Lane::new("t", "gpt", false);
        lane.tool_start("c", "shell", json!({"command":"ls"}));
        lane.tool_succeeded("c", text("ok"), None);
        lane.tool_failed("c", "late", None);
        let ends = lane
            .take()
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    TurnEvent::Tool {
                        state: ToolState::Completed { .. } | ToolState::Failed { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(ends, 1);
    }
}
