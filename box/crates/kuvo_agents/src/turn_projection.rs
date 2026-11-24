//! The portable transcript of a running adapter turn.

use crate::{
    runtime::AdapterCall,
    session::{
        EventEnvelope, ModelRef, Outcome, Prompt, TextKind, ToolOutput, ToolState, TurnError,
        TurnEvent, TurnInput, TurnKind, TurnResult, Usage,
    },
    session_store::{SessionStore, close_rows},
    turn_runtime::TurnWriter,
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, io, time::SystemTime};

pub(crate) struct Projection {
    store: SessionStore,
    session: String,
    turn: String,
    agent: String,
    model: ModelRef,
    next: u64,
    current: Option<usize>,
    steps: Vec<Step>,
    calls: BTreeMap<String, (usize, usize)>,
    compact: Option<Value>,
    /// A summary just ended, before the model's next step: a second word of it is the same one.
    just_compacted: bool,
    /// The note written last, when nothing has come since: the same again isn't written twice.
    last_notice: Option<String>,
    /// A recovered turn's rows as an earlier resident wrote them. Its
    /// replay writes them again, by the same IDs.
    prior: BTreeMap<String, Value>,
    /// What this turn used and cost, added to the session's as it ends.
    spent: Usage,
}

impl Projection {
    pub(crate) async fn start(
        store: SessionStore,
        session: String,
        agent: String,
        input: &TurnInput,
        shown: Option<&Prompt>,
        note: Option<Map<String, Value>>,
    ) -> io::Result<Self> {
        let prior = if matches!(input.kind, TurnKind::Recover { .. }) {
            store
                .turn_messages(&session, row_prefix(&input.id))
                .await?
                .into_iter()
                .filter_map(|row| Some((row["id"].as_str()?.to_owned(), row)))
                .collect()
        } else {
            BTreeMap::new()
        };
        let projection = Self {
            store,
            session,
            turn: input.id.clone(),
            agent,
            model: input.model.clone(),
            next: 0,
            current: None,
            steps: Vec::new(),
            calls: BTreeMap::new(),
            compact: None,
            just_compacted: false,
            last_notice: None,
            prior,
            spent: Usage::default(),
        };
        if let TurnKind::Prompt(prompt) = &input.kind {
            let mut row = user(shown.unwrap_or(prompt));
            if let Some(note) = note {
                row["metadata"] = Value::Object(note);
            }
            projection.write(vec![row]).await?;
        }
        Ok(projection)
    }

    async fn write(&self, rows: Vec<Value>) -> io::Result<()> {
        if rows.is_empty() || self.store.write_messages(&self.session, rows).await? {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The coding-agent session has ended.",
            ))
        }
    }

    fn id(&mut self) -> String {
        self.next += 1;
        format!("{}{}", row_prefix(&self.turn), self.next)
    }

    fn start_step(&mut self, model: Option<ModelRef>) -> usize {
        let index = self.steps.len();
        let id = self.id();
        let created = saved_time(self.prior.get(&id), "created");
        self.steps.push(Step {
            id,
            model: model.unwrap_or_else(|| self.model.clone()),
            created,
            completed: None,
            parts: Vec::new(),
            usage: None,
            finish: None,
            error: None,
        });
        self.current = Some(index);
        index
    }

    fn step(&mut self) -> usize {
        self.current.unwrap_or_else(|| self.start_step(None))
    }

    fn finish_step(
        &mut self,
        usage: Option<Usage>,
        finish: Option<String>,
        error: Option<TurnError>,
    ) -> Option<Value> {
        let index = self.current.take()?;
        let step = &mut self.steps[index];
        step.completed = Some(saved_time(self.prior.get(&step.id), "completed"));
        step.usage = usage;
        step.finish = finish;
        step.error = error;
        Some(step.render(&self.agent))
    }

    fn text(&mut self, kind: TextKind, block: Option<String>, delta: &str) -> Value {
        let at = self.step();
        let step = &mut self.steps[at];
        let matched = match &block {
            Some(key) => step.parts.iter_mut().find(|part| matches!(part,
                Part::Text { kind: other, block: Some(other_key), .. } if kind == *other && key == other_key
            )),
            None => step.parts.last_mut().filter(|part| matches!(part,
                Part::Text { kind: other, block: None, .. } if kind == *other
            )),
        };
        let time = now();
        if let Some(Part::Text { text, streamed, .. }) = matched {
            text.push_str(delta);
            *streamed = time;
        } else {
            step.parts.push(Part::Text {
                kind,
                block,
                text: delta.into(),
                created: time,
                streamed: time,
            });
        }
        step.render(&self.agent)
    }

    fn tool(
        &mut self,
        id: String,
        name: String,
        state: ToolState,
        metadata: &Map<String, Value>,
    ) -> Value {
        if let Some(&(step, part)) = self.calls.get(&id) {
            let step = &mut self.steps[step];
            if let Part::Tool { body } = &mut step.parts[part] {
                let created = body["time"]["created"].as_u64().unwrap_or_else(now);
                let ran = body["time"]["ran"].as_u64();
                *body = tool(&id, &name, state, metadata, created, ran);
            }
            return step.render(&self.agent);
        }
        let index = self.step();
        let step = &mut self.steps[index];
        self.calls.insert(id.clone(), (index, step.parts.len()));
        step.parts.push(Part::Tool {
            body: tool(&id, &name, state, metadata, now(), None),
        });
        step.render(&self.agent)
    }

    fn add(&mut self, usage: &Usage) {
        let spent = &mut self.spent;
        spent.input = spent.input.saturating_add(usage.input);
        spent.output = spent.output.saturating_add(usage.output);
        spent.reasoning = spent.reasoning.saturating_add(usage.reasoning);
        spent.cache_read = spent.cache_read.saturating_add(usage.cache_read);
        spent.cache_write = spent.cache_write.saturating_add(usage.cache_write);
        if let Some(cost) = usage.cost.filter(|cost| cost.is_finite() && *cost > 0.0) {
            spent.cost = Some(spent.cost.unwrap_or_default() + cost);
        }
    }

    /// Tools still running as the turn ends are stopped (`aborted`, when it was) or failed.
    fn stop_tools(&mut self, interrupted: bool) -> Vec<Value> {
        let (kind, message) = if interrupted {
            ("aborted", "Stopped.")
        } else {
            ("tool", "")
        };
        let mut touched = std::collections::BTreeSet::new();
        for &(step, part) in self.calls.values() {
            let Part::Tool { body } = &mut self.steps[step].parts[part] else {
                continue;
            };
            let state = &body["state"];
            let input = match state["status"].as_str() {
                Some("running") => state["input"].clone(),
                Some("streaming") => state["input"]
                    .as_str()
                    .and_then(|input| serde_json::from_str(input).ok())
                    .unwrap_or_else(|| json!({})),
                _ => continue,
            };
            let mut stopped = json!({
                "status":"error", "input":input, "error":{"type":kind,"message":message},
                "content": if message.is_empty() { json!([]) } else { json!([{"type":"text","text":message}]) },
            });
            if let Some(metadata) = state.get("metadata").filter(|value| value.is_object()) {
                stopped["metadata"] = metadata.clone();
            }
            body["state"] = stopped;
            body["time"]["completed"] = json!(now());
            touched.insert(step);
        }
        touched
            .into_iter()
            .map(|step| self.steps[step].render(&self.agent))
            .collect()
    }

    fn finish_compaction(&mut self, success: bool) -> Option<Value> {
        let mut row = self.compact.take()?;
        row["status"] = json!(if success { "completed" } else { "failed" });
        row["time"]["completed"] = json!(now());
        Some(row)
    }

    fn rows(&mut self, event: &TurnEvent) -> Vec<Value> {
        let rows = self.event_rows(event);
        if !rows.is_empty() && !matches!(event, TurnEvent::Notice(_)) {
            self.last_notice = None;
        }
        rows
    }

    fn compaction_row(&mut self) -> Value {
        let row = json!({
            "type":"compaction", "id":self.id(), "reason":"auto",
            "status":"running", "time":{"created":now()}
        });
        self.compact = Some(row.clone());
        row
    }

    fn event_rows(&mut self, event: &TurnEvent) -> Vec<Value> {
        match event {
            TurnEvent::StepStarted { model } => {
                let mut rows = self
                    .finish_step(None, None, None)
                    .into_iter()
                    .collect::<Vec<_>>();
                // A summary still under way as the model answers again: it's over, and it wasn't
                // said to have gone through, so it doesn't stay running.
                rows.extend(self.finish_compaction(false));
                self.just_compacted = false;
                let step = self.start_step(model.clone());
                rows.push(self.steps[step].render(&self.agent));
                rows
            }
            TurnEvent::TextDelta { kind, block, delta } => {
                vec![self.text(*kind, block.clone(), delta)]
            }
            TurnEvent::Tool {
                id,
                name,
                state,
                metadata,
            } => vec![self.tool(id.clone(), name.clone(), state.clone(), metadata)],
            TurnEvent::StepEnded {
                usage,
                finish,
                error,
            } => {
                if let Some(usage) = usage {
                    self.add(usage);
                }
                self.finish_step(usage.clone(), finish.clone(), error.clone())
                    .into_iter()
                    .collect()
            }
            TurnEvent::Cost(cost) => {
                if cost.is_finite() && *cost > 0.0 {
                    self.spent.cost = Some(self.spent.cost.unwrap_or_default() + cost);
                }
                Vec::new()
            }
            TurnEvent::Notice(text) => {
                // Said again with nothing since (each hook failing the same way): once is enough.
                if self.last_notice.as_deref() == Some(text.as_str()) {
                    return Vec::new();
                }
                let mut rows = self
                    .finish_step(None, None, None)
                    .into_iter()
                    .collect::<Vec<_>>();
                rows.push(json!({
                    "type":"synthetic", "id":self.id(), "text":text, "time":{"created":now()},
                    "metadata":{"kuvo":{"notice":true}}
                }));
                self.last_notice = Some(text.clone());
                rows
            }
            TurnEvent::CompactionStarted => {
                // Said again while it runs, it's the same one.
                if self.compact.is_some() {
                    return Vec::new();
                }
                self.just_compacted = false;
                let mut rows = self
                    .finish_step(None, None, None)
                    .into_iter()
                    .collect::<Vec<_>>();
                rows.push(self.compaction_row());
                rows
            }
            TurnEvent::CompactionEnded { succeeded } => {
                let mut rows = Vec::new();
                // One told only once it's done gets its row then; a second word about the one
                // just ended is the same one.
                if self.compact.is_none() {
                    if self.just_compacted {
                        return rows;
                    }
                    rows.extend(self.finish_step(None, None, None));
                    self.compaction_row();
                }
                self.just_compacted = true;
                rows.extend(self.finish_compaction(*succeeded));
                rows
            }
            TurnEvent::Finished(result) => {
                let error = match &result.outcome {
                    Outcome::Failed(error) => Some(error.clone()),
                    _ => None,
                };
                let interrupted = matches!(
                    result.outcome,
                    Outcome::Interrupted | Outcome::AgentExited(_)
                );
                let mut rows = self.finish_compaction(false).into_iter().collect::<Vec<_>>();
                rows.extend(self.finish_step(None, None, error));
                rows.extend(self.stop_tools(interrupted));
                let id = self.id();
                let at = saved_time(self.prior.get(&id), "created");
                rows.push(idle(id, at, result));
                rows
            }
            TurnEvent::Context { .. }
            | TurnEvent::ContextCleared
            | TurnEvent::RanOn(_)
            | TurnEvent::ReplayCaughtUp => Vec::new(),
        }
    }
}

/// A message whose agent couldn't start: it's in the conversation with the
/// turn that failed, as one whose agent broke at once. A recovered turn's
/// rows stay as they were shown, closed, before its end.
pub(crate) async fn record_failed_start(
    store: SessionStore,
    session: String,
    agent: String,
    input: &TurnInput,
    shown: Option<&Prompt>,
    note: Option<Map<String, Value>>,
    error: &io::Error,
) -> io::Result<()> {
    let mut projection = Projection::start(store, session, agent, input, shown, note).await?;
    projection.abandon(error).await
}

/// A turn's end as failed with `error`, as one whose agent broke.
fn failed(error: &io::Error) -> EventEnvelope {
    let message = error.to_string();
    EventEnvelope {
        position: None,
        delivery: crate::session::Delivery::Live,
        event: TurnEvent::Finished(TurnResult {
            outcome: Outcome::Failed(TurnError {
                kind: "agent".into(),
                message: if message.is_empty() {
                    "The agent stopped with an error.".into()
                } else {
                    message
                },
                status: None,
                limit: None,
            }),
            took_prompt: false,
            missed: Vec::new(),
        }),
    }
}

fn row_prefix(turn: &str) -> String {
    format!("msg_{turn}_")
}

fn saved_time(row: Option<&Value>, kind: &str) -> u64 {
    row.and_then(|row| row["time"][kind].as_u64())
        .unwrap_or_else(now)
}

impl TurnWriter for Projection {
    fn apply<'a>(&'a mut self, event: &'a EventEnvelope) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            if matches!(event.event, TurnEvent::Finished(_)) {
                self.drop_unreplayed().await?;
            }
            let rows = self.rows(&event.event);
            self.write(rows).await?;
            self.update_header(&event.event).await
        })
    }

    fn abandon<'a>(&'a mut self, error: &'a io::Error) -> AdapterCall<'a, ()> {
        Box::pin(async move {
            if self.next == 0 && !self.prior.is_empty() {
                // A recovered turn never taken up: what was shown of it
                // stays, closed as a crash leaves it, and its end follows.
                let mut rows = self.prior.values().cloned().collect::<Vec<_>>();
                let closed = close_rows(&mut rows);
                self.write(closed).await?;
                self.next = self.prior.keys().filter_map(|id| self.index(id)).max().unwrap_or(0);
                self.prior.clear();
            }
            self.apply(&failed(error)).await
        })
    }
}

impl Projection {
    fn index(&self, id: &str) -> Option<u64> {
        id.strip_prefix(&row_prefix(&self.turn))?.parse().ok()
    }

    /// A recovered turn's rows its replay didn't write again: gone, so its
    /// end and what it said are as the agent made them, once.
    async fn drop_unreplayed(&mut self) -> io::Result<()> {
        let stale = self
            .prior
            .keys()
            .filter(|id| self.index(id).is_some_and(|index| index > self.next))
            .cloned()
            .collect::<Vec<_>>();
        if stale.is_empty() {
            return Ok(());
        }
        for id in &stale {
            self.prior.remove(id);
        }
        self.store.remove_messages(&self.session, stale).await?;
        Ok(())
    }
}

impl Projection {
    async fn update_header(&self, event: &TurnEvent) -> io::Result<()> {
        if !matches!(
            event,
            TurnEvent::Context { .. }
                | TurnEvent::ContextCleared
                | TurnEvent::RanOn(_)
                | TurnEvent::Finished(_)
        ) {
            return Ok(());
        }
        let event = event.clone();
        let model = model(&self.model);
        let spent = self.spent.clone();
        let kept = self
            .store
            .change_header(&self.session, move |header| match event {
                TurnEvent::Context { used, window } => {
                    if let Some(window) =
                        window.filter(|&window| window > 0 && window < (1 << 53))
                    {
                        header.insert("contextWindow".into(), json!(window));
                    }
                    let window = header.get("contextWindow").cloned();
                    let context = object(header, "context");
                    if let Some(used) = used.filter(|&used| used < (1 << 53)) {
                        context.insert("used".into(), json!(used));
                    }
                    if let Some(window) = window {
                        context.insert("window".into(), window);
                    }
                    header.insert("contextModel".into(), model);
                }
                TurnEvent::ContextCleared => {
                    header.remove("context");
                }
                TurnEvent::RanOn(name) if !name.trim().is_empty() => {
                    let kuvo = object(object(header, "metadata"), "kuvo");
                    kuvo.insert("ranOn".into(), json!(name));
                }
                TurnEvent::Finished(result) => {
                    let outcome = match result.outcome {
                        Outcome::Succeeded => "succeeded",
                        Outcome::Failed(_) => "failed",
                        Outcome::Interrupted | Outcome::AgentExited(_) => "interrupted",
                    };
                    header.insert("outcome".into(), json!(outcome));
                    let time = object(header, "time");
                    let at = now();
                    for key in ["activity", "updated", "idle"] {
                        time.insert(key.into(), json!(at));
                    }
                    let tokens = object(header, "tokens");
                    for (key, value) in [
                        ("input", spent.input),
                        ("output", spent.output),
                        ("reasoning", spent.reasoning),
                    ] {
                        let total = tokens.get(key).and_then(Value::as_u64).unwrap_or_default();
                        tokens.insert(key.into(), json!(total.saturating_add(value)));
                    }
                    let cache = object(tokens, "cache");
                    for (key, value) in [("read", spent.cache_read), ("write", spent.cache_write)] {
                        let total = cache.get(key).and_then(Value::as_u64).unwrap_or_default();
                        cache.insert(key.into(), json!(total.saturating_add(value)));
                    }
                    let cost = header.get("cost").and_then(Value::as_f64).unwrap_or_default();
                    header.insert("cost".into(), json!(cost + spent.cost.unwrap_or_default()));
                }
                _ => {}
            })
            .await?;
        if kept {
            Ok(())
        } else {
            Err(io::ErrorKind::NotFound.into())
        }
    }
}

fn object<'a>(fields: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let entry = fields.entry(key.to_owned()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().expect("ensured an object")
}

struct Step {
    id: String,
    model: ModelRef,
    created: u64,
    completed: Option<u64>,
    parts: Vec<Part>,
    finish: Option<String>,
    usage: Option<Usage>,
    error: Option<TurnError>,
}

impl Step {
    fn render(&self, agent: &str) -> Value {
        let mut body = json!({
            "type":"assistant", "id":self.id, "agent":agent, "model":model(&self.model),
            "time":{"created":self.created}, "content":self.parts.iter().map(Part::render).collect::<Vec<_>>()
        });
        if let Some(completed) = self.completed {
            body["time"]["completed"] = json!(completed);
        }
        if let Some(finish) = &self.finish {
            body["finish"] = json!(finish);
        }
        if let Some(usage) = &self.usage {
            body["tokens"] = json!({
                "input":usage.input, "output":usage.output, "reasoning":usage.reasoning,
                "cache":{"read":usage.cache_read, "write":usage.cache_write}
            });
            if let Some(cost) = usage.cost {
                body["cost"] = json!(cost);
            }
        }
        if let Some(error) = &self.error {
            body["error"] = reason(error);
        }
        body
    }
}

enum Part {
    Text {
        kind: TextKind,
        block: Option<String>,
        text: String,
        created: u64,
        streamed: u64,
    },
    Tool {
        body: Value,
    },
}

impl Part {
    fn render(&self) -> Value {
        match self {
            Self::Text {
                kind,
                text,
                created,
                streamed,
                ..
            } => json!({
                "type":match kind { TextKind::Text => "text", TextKind::Reasoning => "reasoning" },
                "text":text, "time":{"created":created,"streamed":streamed}
            }),
            Self::Tool { body } => body.clone(),
        }
    }
}

pub(crate) fn user(prompt: &Prompt) -> Value {
    let mut body = json!({
        "type":if prompt.synthetic { "synthetic" } else { "user" },
        "id": prompt.id, "text": prompt.text, "time":{"created":now()}
    });
    if !prompt.files.is_empty() {
        body["files"] = json!(
            prompt
                .files
                .iter()
                .map(|file| {
                    let mut item = json!({"mime":file.mime});
                    if let Some(name) = &file.name {
                        item["name"] = json!(name);
                    }
                    if let Some(data) = &file.data {
                        item["data"] = json!(data);
                        item["source"] = json!({"type":"inline"});
                    } else {
                        item["source"] = json!({"type":"uri","uri":file.uri});
                    }
                    item
                })
                .collect::<Vec<_>>()
        );
    }
    body
}

fn tool(
    id: &str,
    name: &str,
    state: ToolState,
    metadata: &Map<String, Value>,
    created: u64,
    ran: Option<u64>,
) -> Value {
    let now = now();
    let (mut state, running, completed) = match state {
        ToolState::Streaming(input) => (json!({"status":"streaming","input":input}), false, false),
        ToolState::Running { input } => (json!({"status":"running","input":input}), true, false),
        ToolState::Completed { input, content } => (
            json!({"status":"completed","input":input,"content":outputs(content)}),
            true,
            true,
        ),
        ToolState::Failed {
            input,
            content,
            error,
        } => (
            json!({"status":"error","input":input,"error":reason(&error),"content":outputs(content)}),
            true,
            true,
        ),
    };
    if !metadata.is_empty() {
        state["metadata"] = json!(metadata);
    }
    let mut time = json!({"created":created});
    if running {
        time["ran"] = json!(ran.unwrap_or(now));
    }
    if completed {
        time["completed"] = json!(now);
    }
    json!({"type":"tool", "id":id, "name":name, "state":state, "time":time})
}

fn outputs(content: Vec<ToolOutput>) -> Vec<Value> {
    content
        .into_iter()
        .map(|output| match output {
            ToolOutput::Text(text) => json!({"type":"text","text":text}),
            ToolOutput::File { uri, mime, name } => {
                let mut file = json!({"type":"file","uri":uri,"mime":mime});
                if let Some(name) = name {
                    file["name"] = json!(name);
                }
                file
            }
        })
        .collect()
}

fn idle(id: String, at: u64, result: &TurnResult) -> Value {
    let (outcome, error) = match &result.outcome {
        Outcome::Succeeded => ("succeeded", None),
        Outcome::Failed(error) => ("failed", Some(error)),
        Outcome::Interrupted => ("interrupted", None),
        Outcome::AgentExited(error) => ("interrupted", error.as_ref()),
    };
    let mut row = json!({"type":"idle","id":id,"outcome":outcome,"time":{"created":at}});
    if let Some(error) = error {
        row["error"] = reason(error);
    }
    if matches!(result.outcome, Outcome::AgentExited(_)) {
        row["reason"] = json!("agent-exit");
    }
    row
}

fn model(model: &ModelRef) -> Value {
    let mut body = json!({"providerID":model.provider,"id":model.id});
    if let Some(variant) = &model.variant {
        body["variant"] = json!(variant);
    }
    body
}

fn reason(error: &TurnError) -> Value {
    error.reason()
}

pub(crate) fn now() -> u64 {
    SystemTime::UNIX_EPOCH
        .elapsed()
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
