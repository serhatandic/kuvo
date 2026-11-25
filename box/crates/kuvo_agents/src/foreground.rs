//! The commands a turn waits on in the foreground, listed while they run with what they've
//! printed, as those an agent left running are (`background`). Stop on one stops its turn, as in
//! each agent's own app: none can stop one foreground command alone. Any agent's `shell` step.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex as Plain},
};

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::{
    background::{Background, CommandSource, Printed, WorkOrder, now},
    session::{ToolOutput, ToolState, TurnEvent},
};

/// Stops what the turn's commands are part of: the turn.
pub(crate) type StopTurn = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// How much of what it printed is looked back on to find where the new part begins.
const OVERLAP: usize = 200;

pub(crate) struct Foreground {
    session: String,
    cwd: String,
    background: Background,
    stop: StopTurn,
    /// Each command running, by its step: its listing, and what it has printed.
    commands: HashMap<String, (String, Arc<Plain<Printed>>)>,
}

impl Foreground {
    pub fn new(session: &str, cwd: &str, background: Background, stop: StopTurn) -> Self {
        Self {
            session: session.to_owned(),
            cwd: cwd.to_owned(),
            background,
            stop,
            commands: HashMap::new(),
        }
    }

    /// What the turn said: a command it now waits on is listed, what it printed kept, and one
    /// that ended goes (what it printed still read a while).
    pub fn heard(&mut self, event: &TurnEvent) {
        match event {
            TurnEvent::Tool {
                id,
                name,
                state,
                metadata,
            } => match state {
                ToolState::Running { input } => {
                    self.list(id, name, input);
                    if let Some(output) = metadata.get("output").and_then(Value::as_str) {
                        self.printed(id, output);
                    }
                }
                ToolState::Completed { content, .. } => self.end(id, content, Some(0)),
                ToolState::Failed { content, .. } => self.end(id, content, None),
                ToolState::Streaming(_) => {}
            },
            TurnEvent::Finished(_) => self.end_all(),
            _ => {}
        }
    }

    /// The turn is over: what's still listed goes with it.
    pub fn end_all(&mut self) {
        for (_, (shell, _)) in self.commands.drain() {
            self.background.end(&shell, None);
        }
    }

    // (One sent to the background returns at once: it's its agent's to list.)
    fn list(&mut self, id: &str, name: &str, input: &Value) {
        let command = input["command"].as_str().filter(|command| !command.trim().is_empty());
        let Some(command) = command.filter(|_| {
            name == "shell" && input["background"] != true && !self.commands.contains_key(id)
        }) else {
            return;
        };
        let printed = Arc::new(Plain::new(Printed::default()));
        let source = Arc::new(Waited {
            printed: printed.clone(),
            stop: self.stop.clone(),
        });
        let shell = self.background.add(
            &self.session,
            &self.cwd,
            command,
            source,
            now(),
            WorkOrder::next(),
            true,
        );
        self.commands.insert(id.to_owned(), (shell, printed));
    }

    /// What it has printed now, whole or its end (an agent keeps the last of it): how much is new
    /// is what follows the end of what it had before.
    fn printed(&self, id: &str, output: &str) {
        let Some((_, printed)) = self.commands.get(id) else {
            return;
        };
        let mut printed = lock(printed);
        if printed.output == output {
            return;
        }
        let before = &printed.output;
        let added = match output.strip_prefix(before.as_str()) {
            Some(added) => added,
            None => {
                let mut from = before.len().saturating_sub(OVERLAP);
                while !before.is_char_boundary(from) {
                    from += 1;
                }
                let tail = &before[from..];
                match output.rfind(tail).filter(|_| !tail.is_empty()) {
                    Some(at) => &output[at + tail.len()..],
                    None => output,
                }
            }
        };
        let total = printed.total + added.len() as u64;
        printed.total = total.max(output.len() as u64);
        printed.output = output.to_owned();
    }

    fn end(&mut self, id: &str, content: &[ToolOutput], exit: Option<i32>) {
        let words: Vec<&str> = content
            .iter()
            .filter_map(|part| match part {
                ToolOutput::Text(text) => Some(text.as_str()),
                ToolOutput::File { .. } => None,
            })
            .collect();
        if !words.is_empty() {
            self.printed(id, &words.join("\n"));
        }
        if let Some((shell, _)) = self.commands.remove(id) {
            self.background.end(&shell, exit);
        }
    }
}

/// A command its turn waits on: read from what its agent said it printed, stopped with its turn.
struct Waited {
    printed: Arc<Plain<Printed>>,
    stop: StopTurn,
}

impl CommandSource for Waited {
    fn output(&self) -> BoxFuture<'_, Printed> {
        let printed = lock(&self.printed).clone();
        Box::pin(async move { printed })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        (self.stop)()
    }
}

fn lock<T>(plain: &Plain<T>) -> std::sync::MutexGuard<'_, T> {
    plain.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn running(id: &str, command: Value, output: Option<&str>) -> TurnEvent {
        let mut metadata = Map::new();
        if let Some(output) = output {
            metadata.insert("output".into(), json!(output));
        }
        TurnEvent::Tool {
            id: id.into(),
            name: "shell".into(),
            state: ToolState::Running { input: command },
            metadata,
        }
    }

    fn tracker(stops: Arc<AtomicUsize>) -> (Foreground, Background) {
        let background = Background::default();
        let stop: StopTurn = Arc::new(move || {
            stops.fetch_add(1, Ordering::Relaxed);
            Box::pin(async {})
        });
        (Foreground::new("ses_a", "/work", background.clone(), stop), background)
    }

    #[tokio::test]
    async fn a_command_it_waits_on_is_listed_with_what_it_printed_and_its_stop_stops_the_turn() {
        let stops = Arc::new(AtomicUsize::new(0));
        let (mut foreground, background) = tracker(stops.clone());
        foreground.heard(&running("c1", json!({"command":"npm test"}), None));
        foreground.heard(&running("c1", json!({"command":"npm test"}), Some("3 passing\n")));
        let listed = background.running("ses_a", true);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].command, "npm test");
        assert!(listed[0].foreground);
        // Not something it left running: nothing waits on it past the turn.
        assert!(background.running("ses_a", false).is_empty());
        let read = background.output(&listed[0].id, 0, None).await.unwrap();
        assert_eq!(read.output, "3 passing\n");
        assert!(background.stop(&listed[0].id).await);
        assert_eq!(stops.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn one_that_ends_leaves_the_list_and_what_it_printed_stays() {
        let (mut foreground, background) = tracker(Arc::default());
        foreground.heard(&running("c1", json!({"command":"npm run build"}), None));
        let id = background.running("ses_a", true)[0].id.clone();
        foreground.heard(&TurnEvent::Tool {
            id: "c1".into(),
            name: "shell".into(),
            state: ToolState::Completed {
                input: json!({"command":"npm run build"}),
                content: vec![ToolOutput::Text("built in 2s\n".into())],
            },
            metadata: Map::new(),
        });
        assert!(background.running("ses_a", true).is_empty());
        assert_eq!(background.command(&id).unwrap().exit, Some(0));
        let read = background.output(&id, 0, None).await.unwrap();
        assert_eq!(read.output, "built in 2s\n");
    }

    #[test]
    fn one_in_the_background_or_another_step_isnt_listed() {
        let (mut foreground, background) = tracker(Arc::default());
        foreground.heard(&running("c1", json!({"command":"npm run dev","background":true}), None));
        foreground.heard(&running("c2", json!({"command":"  "}), None));
        foreground.heard(&TurnEvent::Tool {
            id: "r1".into(),
            name: "read".into(),
            state: ToolState::Running {
                input: json!({"command":"cat"}),
            },
            metadata: Map::new(),
        });
        assert!(background.running("ses_a", true).is_empty());
    }

    #[test]
    fn what_it_printed_counts_what_came_past_a_kept_end() {
        let (mut foreground, background) = tracker(Arc::default());
        let long = "x".repeat(300);
        foreground.heard(&running("c1", json!({"command":"yes"}), Some(&format!("a{long}"))));
        // Its agent keeps only the end: what follows the end it had before is new.
        foreground.heard(&running("c1", json!({"command":"yes"}), Some(&format!("{long}bc"))));
        let (_, printed) = &foreground.commands["c1"];
        assert_eq!(lock(printed).total, 303);
        foreground.end_all();
        assert!(background.running("ses_a", true).is_empty());
    }
}
