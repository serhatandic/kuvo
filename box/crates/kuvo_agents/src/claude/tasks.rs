//! Claude Code's background tasks, as the bar shows them: a command it left running (or moved to
//! the background) goes in the conversation's commands while it runs, with what it printed and a
//! way to stop it; what else it keeps going is a task. Each ends as Claude Code says it did, or with
//! Claude Code itself.

use std::{
    collections::{HashMap, HashSet},
    io,
    sync::{Arc, Mutex},
};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::background::{Background, CommandSource, Helper, Printed, WorkOrder, now};

/// How much of a command's output file is read: its end.
const OUTPUT_END: u64 = 64_000;

/// A control request to the Claude Code these tasks are in, and its answer's `response`.
pub(super) trait Calls: Send + Sync {
    fn call(&self, request: Value) -> BoxFuture<'_, io::Result<Value>>;
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Task {
    kind: String,
    description: String,
    tool_use: Option<String>,
    since: u64,
    order: Option<WorkOrder>,
    /// Its id in the bar, for a command.
    shell: Option<String>,
    output_file: Option<String>,
    /// What it printed when last read.
    last: Option<Printed>,
}

#[derive(Default, Deserialize, Serialize)]
struct Known {
    tasks: HashMap<String, Task>,
    /// Those Claude Code says it has going now.
    live: HashSet<String>,
    /// Each Bash call's command, by its call.
    commands: HashMap<String, String>,
}

/// One Claude Code's tasks, for its conversation.
pub(super) struct Tasks {
    session: String,
    cwd: String,
    background: Background,
    known: Arc<Mutex<Known>>,
    calls: Arc<dyn Calls>,
}

impl Tasks {
    pub fn new(session: &str, cwd: &str, background: Background, calls: Arc<dyn Calls>) -> Self {
        Self {
            session: session.to_owned(),
            cwd: cwd.to_owned(),
            background,
            known: Arc::default(),
            calls,
        }
    }

    fn known(&self) -> std::sync::MutexGuard<'_, Known> {
        self.known.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Commands or tasks it keeps going: closing it would end them.
    pub fn keeps_working(&self) -> bool {
        !self.known().live.is_empty()
    }

    pub fn snapshot(&self) -> Value {
        let known = self.known();
        if known.live.is_empty() {
            return Value::Null;
        }
        let tasks: HashMap<_, _> = known
            .tasks
            .iter()
            .filter(|(id, _)| known.live.contains(*id))
            .map(|(id, task)| (id.clone(), task.clone()))
            .collect();
        let commands = tasks
            .values()
            .filter_map(|task| task.tool_use.as_ref())
            .filter_map(|id| {
                known
                    .commands
                    .get(id)
                    .map(|command| (id.clone(), command.clone()))
            })
            .collect();
        json!(Known {
            tasks,
            live: known.live.clone(),
            commands
        })
    }

    pub fn restore(&self, saved: Value) {
        let Ok(known) = serde_json::from_value::<Known>(saved) else {
            return;
        };
        *self.known() = known;
        let jobs: Vec<_> = {
            let known = self.known();
            known
                .tasks
                .iter()
                .filter_map(|(id, task)| {
                    let shell = task.shell.clone()?;
                    Some((
                        id.clone(),
                        crate::background::Command {
                            id: shell,
                            running: true,
                            command: task
                                .tool_use
                                .as_ref()
                                .and_then(|call| known.commands.get(call))
                                .cloned()
                                .unwrap_or_else(|| task.description.clone()),
                            cwd: self.cwd.clone(),
                            exit: None,
                            session: self.session.clone(),
                            started: task.since,
                            order: task.order.clone().unwrap_or_else(WorkOrder::next),
                            foreground: false,
                        },
                    ))
                })
                .collect()
        };
        for (id, job) in jobs {
            self.background.restore(
                job,
                Arc::new(ClaudeCommand {
                    task: id,
                    known: self.known.clone(),
                    calls: self.calls.clone(),
                    background: self.background.clone(),
                }),
            );
        }
        self.tell_tasks();
    }

    /// What it said, for what it has going.
    pub fn frame(&self, message: &Value) -> bool {
        match (message["type"].as_str(), message["subtype"].as_str()) {
            (Some("assistant"), _) => self.calls_in(message),
            (Some("system"), Some(subtype)) => match subtype {
                "task_started" => self.started(message),
                "task_updated" => self.updated(message),
                "task_notification" => self.notified(message),
                "background_tasks_changed" => self.changed(message),
                _ => return false,
            },
            _ => return false,
        }
        self.tell_tasks();
        true
    }

    /// Each Bash call's command, for the bar to name it by.
    fn calls_in(&self, message: &Value) {
        let Some(content) = message["message"]["content"].as_array() else {
            return;
        };
        let mut known = self.known();
        for part in content {
            if part["type"] == "tool_use"
                && part["name"] == "Bash"
                && let (Some(id), Some(command)) =
                    (part["id"].as_str(), part["input"]["command"].as_str())
            {
                known.commands.insert(id.to_owned(), command.to_owned());
            }
        }
    }

    fn started(&self, message: &Value) {
        let id = string(&message["task_id"]);
        {
            let mut known = self.known();
            let was = known.tasks.get(&id).cloned().unwrap_or_default();
            let kind = message["task_type"]
                .as_str()
                .map_or(was.kind.clone(), str::to_owned);
            known.tasks.insert(
                id.clone(),
                Task {
                    kind,
                    description: message["description"]
                        .as_str()
                        .map_or(was.description.clone(), str::to_owned),
                    tool_use: message["tool_use_id"]
                        .as_str()
                        .map(str::to_owned)
                        .or(was.tool_use.clone()),
                    since: if was.since > 0 { was.since } else { now() },
                    order: was.order.clone().or_else(|| Some(WorkOrder::next())),
                    ..was
                },
            );
        }
        if message["is_backgrounded"] == true && message["ambient"] != true {
            self.shell_for(&id);
        }
    }

    fn updated(&self, message: &Value) {
        if message["patch"]["is_backgrounded"] == true {
            self.shell_for(&string(&message["task_id"]));
        }
    }

    fn notified(&self, message: &Value) {
        let id = string(&message["task_id"]);
        let ended = {
            let mut known = self.known();
            known.live.remove(&id);
            let Some(task) = known.tasks.get_mut(&id) else {
                return;
            };
            if let Some(file) = message["output_file"].as_str() {
                task.output_file = Some(file.to_owned());
            }
            task.shell.clone()
        };
        if let Some(shell) = ended {
            let summary = message["summary"].as_str().unwrap_or_default();
            let exit = exit_code(summary).or((message["status"] == "completed").then_some(0));
            self.background.end(&shell, exit);
        }
    }

    fn changed(&self, message: &Value) {
        let listed: Vec<&Value> = message["tasks"]
            .as_array()
            .map(|tasks| {
                tasks
                    .iter()
                    .filter(|task| task["ambient"] != true)
                    .collect()
            })
            .unwrap_or_default();
        let ids: Vec<String> = listed.iter().map(|task| string(&task["task_id"])).collect();
        let gone: Vec<String> = {
            let mut known = self.known();
            known.live = ids.iter().cloned().collect();
            for task in &listed {
                let id = string(&task["task_id"]);
                let was = known.tasks.get(&id).cloned().unwrap_or_default();
                known.tasks.insert(
                    id,
                    Task {
                        kind: task["task_type"]
                            .as_str()
                            .map_or(was.kind.clone(), str::to_owned),
                        description: task["description"]
                            .as_str()
                            .map_or(was.description.clone(), str::to_owned),
                        since: if was.since > 0 { was.since } else { now() },
                        order: was.order.clone().or_else(|| Some(WorkOrder::next())),
                        ..was
                    },
                );
            }
            known
                .tasks
                .iter()
                .filter(|(id, task)| task.shell.is_some() && !known.live.contains(*id))
                .filter_map(|(_, task)| task.shell.clone())
                .collect()
        };
        for id in &ids {
            self.shell_for(id);
        }
        for shell in gone {
            self.background.end(&shell, None);
        }
    }

    /// A command it left running, in the bar.
    fn shell_for(&self, id: &str) {
        let (command, since, order) = {
            let known = self.known();
            let Some(task) = known.tasks.get(id) else {
                return;
            };
            if task.shell.is_some() || task.kind != "local_bash" {
                return;
            }
            let command = task
                .tool_use
                .as_ref()
                .and_then(|call| known.commands.get(call))
                .cloned()
                .unwrap_or_else(|| task.description.clone());
            (
                command,
                task.since,
                task.order.clone().unwrap_or_else(WorkOrder::next),
            )
        };
        let source = Arc::new(ClaudeCommand {
            task: id.to_owned(),
            known: self.known.clone(),
            calls: self.calls.clone(),
            background: self.background.clone(),
        });
        let shell = self.background.add(
            &self.session,
            &self.cwd,
            &command,
            source,
            since,
            order,
            false,
        );
        if let Some(task) = self.known().tasks.get_mut(id) {
            task.shell = Some(shell);
        }
    }

    /// What it keeps going that's neither a command (in the bar) nor a subagent's session.
    fn tell_tasks(&self) {
        let tasks = {
            let known = self.known();
            let mut tasks: Vec<Helper> = known
                .live
                .iter()
                .filter_map(|id| {
                    let task = known.tasks.get(id)?;
                    (task.shell.is_none() && task.kind != "local_bash").then(|| Helper {
                        id: id.clone(),
                        since: task.since,
                        order: task.order.clone(),
                    })
                })
                .collect();
            tasks.sort_by_key(|task| task.since);
            tasks
        };
        self.background.set_tasks(&self.session, tasks);
    }

    /// Claude Code stopped (or crashed): what it left running went with it.
    pub fn gone(&self) {
        let shells: Vec<String> = {
            let mut known = self.known();
            known.live.clear();
            known
                .tasks
                .values()
                .filter_map(|task| task.shell.clone())
                .collect()
        };
        for shell in shells {
            self.background.end(&shell, None);
        }
        self.background.set_tasks(&self.session, Vec::new());
    }
}

/// A command Claude Code left running, read and stopped through it.
struct ClaudeCommand {
    task: String,
    known: Arc<Mutex<Known>>,
    calls: Arc<dyn Calls>,
    background: Background,
}

impl ClaudeCommand {
    fn task(&self) -> Option<Task> {
        self.known
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .tasks
            .get(&self.task)
            .cloned()
    }

    fn remember(&self, printed: &Printed) {
        if let Some(task) = self
            .known
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .tasks
            .get_mut(&self.task)
        {
            task.last = Some(printed.clone());
        }
    }
}

impl CommandSource for ClaudeCommand {
    /// The end of what it printed: from its file once Claude Code names it, or as Claude Code
    /// reads it.
    fn output(&self) -> BoxFuture<'_, Printed> {
        Box::pin(async move {
            let task = self.task();
            if let Some(file) = task.as_ref().and_then(|task| task.output_file.clone())
                && let Ok(printed) = file_end(&file).await
                && printed.total > 0
            {
                self.remember(&printed);
                return printed;
            }
            let request = json!({"subtype":"get_task_output","task_id":self.task});
            if let Ok(response) = self.calls.call(request).await {
                let printed = Printed {
                    output: response["output"].as_str().unwrap_or_default().to_owned(),
                    total: response["total_bytes"].as_u64().unwrap_or_default(),
                };
                if printed.total > 0 || !printed.output.is_empty() {
                    self.remember(&printed);
                    return printed;
                }
            }
            task.and_then(|task| task.last).unwrap_or_default()
        })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let request = json!({"subtype":"stop_task","task_id":self.task});
            if self.calls.call(request).await.is_err() {
                // Claude Code is gone, and the command with it.
                let known = self.task();
                if let Some(shell) = known.and_then(|task| task.shell) {
                    self.background.end(&shell, None);
                }
            }
        })
    }
}

async fn file_end(file: &str) -> io::Result<Printed> {
    let mut file = tokio::fs::File::open(file).await?;
    let size = file.metadata().await?.len();
    let from = size.saturating_sub(OUTPUT_END);
    file.seek(io::SeekFrom::Start(from)).await?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await?;
    Ok(Printed {
        output: String::from_utf8_lossy(&bytes).into_owned(),
        total: from + bytes.len() as u64,
    })
}

fn string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The exit code a summary names ("exit code 1").
fn exit_code(summary: &str) -> Option<i32> {
    let lower = summary.to_lowercase();
    let at = lower.find("exit code")?;
    let rest = summary[at + "exit code".len()..].trim_start();
    let end = rest
        .char_indices()
        .find(|(index, char)| !(char.is_ascii_digit() || (*index == 0 && *char == '-')))
        .map_or(rest.len(), |(index, _)| index);
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Answers;

    impl Calls for Answers {
        fn call(&self, request: Value) -> BoxFuture<'_, io::Result<Value>> {
            Box::pin(async move {
                Ok(if request["subtype"] == "get_task_output" {
                    json!({"output":"ready on :3000\n","total_bytes":15})
                } else {
                    json!({})
                })
            })
        }
    }

    #[tokio::test]
    async fn a_command_it_leaves_running_shows_in_the_bar_until_it_ends() {
        let background = Background::default();
        let tasks = Tasks::new("ses_a", "/work", background.clone(), Arc::new(Answers));
        tasks.frame(&json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"tool1","name":"Bash","input":{"command":"npm run dev","run_in_background":true}}
        ]}}));
        tasks.frame(&json!({"type":"system","subtype":"task_started","task_id":"t1","task_type":"local_bash",
            "description":"Start the dev server","tool_use_id":"tool1","is_backgrounded":true}));
        tasks.frame(
            &json!({"type":"system","subtype":"background_tasks_changed","tasks":[
                {"task_id":"t1","task_type":"local_bash","description":"Start the dev server"},
                {"task_id":"t2","task_type":"remote_agent","description":"Review"}
            ]}),
        );
        let work = background.work(false);
        assert_eq!(work["ses_a"].commands[0].command, "npm run dev");
        assert_eq!(
            work["ses_a"]
                .tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["t2"]
        );
        assert!(tasks.keeps_working());
        let shell = work["ses_a"].commands[0].id.clone();
        assert_eq!(
            background.output(&shell, 0, None).await.unwrap().output,
            "ready on :3000\n"
        );
        // It ends: the bar lets it go, with its exit.
        tasks.frame(&json!({"type":"system","subtype":"task_notification","task_id":"t1","status":"completed",
            "tool_use_id":"tool1","summary":"exit code 3"}));
        assert_eq!(background.command(&shell).unwrap().exit, Some(3));
        assert!(background.work(false)["ses_a"].commands.is_empty());
        tasks.gone();
        assert!(background.work(false).is_empty());
        assert!(!tasks.keeps_working());
    }

    #[test]
    fn exit_codes_as_summaries_say_them() {
        assert_eq!(exit_code("exit code 0"), Some(0));
        assert_eq!(exit_code("Failed with Exit Code -1."), Some(-1));
        assert_eq!(exit_code("done"), None);
    }
}
