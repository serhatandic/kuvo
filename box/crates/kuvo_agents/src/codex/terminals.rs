//! The commands Codex left running in background terminals, as its app-server last listed them.
//! Each with a command goes in the conversation's commands, with what it printed and Stop through
//! Codex; one without is a task. Codex doesn't say when one ends: it's listed again every while,
//! until none is left. A list that fails isn't taken for none.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex as Plain},
};

use futures_util::future::BoxFuture;
use serde_json::Value;

use super::projection::{OUTPUT_LIMIT, tail, unwrap_shell};
use crate::background::{
    AT_WORK_LIMIT, Background, CommandSource, Helper, Printed, WorkOrder, now,
};

/// Command starts kept for commands not listed yet.
const STARTS_KEPT: usize = 200;

/// A terminal as Codex lists it.
pub(super) struct Listed {
    pub id: String,
    pub item: Option<String>,
    pub command: Option<String>,
}

impl Listed {
    /// One entry of `thread/backgroundTerminals/list`; none without a process.
    pub fn from(terminal: &Value) -> Option<Self> {
        let text = |key: &str| terminal[key].as_str().map(str::to_owned);
        Some(Self {
            id: text("processId")?,
            item: text("itemId"),
            command: text("command"),
        })
    }
}

/// Stops one terminal through its app-server.
pub(super) type Stop = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

struct Terminal {
    id: String,
    item: Option<String>,
    since: u64,
    order: WorkOrder,
    /// Its id in the bar, when it has a command.
    shell: Option<String>,
}

type Outputs = Arc<Plain<HashMap<String, Printed>>>;

pub(super) struct Terminals {
    session: String,
    cwd: String,
    background: Background,
    listed: Vec<Terminal>,
    /// When each command began, by its item, including ones not listed yet (oldest first).
    starts: HashMap<String, (u64, WorkOrder)>,
    started: VecDeque<String>,
    outputs: Outputs,
    /// The list after a turn failed: what it left running is still to be found.
    unknown: Option<Helper>,
}

impl Terminals {
    pub fn new(session: &str, cwd: &str, background: Background) -> Self {
        Self {
            session: session.to_owned(),
            cwd: cwd.to_owned(),
            background,
            listed: Vec::new(),
            starts: HashMap::new(),
            started: VecDeque::new(),
            outputs: Arc::default(),
            unknown: None,
        }
    }

    /// Commands it left running (or not known yet): its app-server is kept for them.
    pub fn keeps_working(&self) -> bool {
        !self.listed.is_empty() || self.unknown.is_some()
    }

    /// What its app-server said of a command: when it began, and what it printed.
    pub fn heard(&mut self, method: &str, params: &Value) {
        let item = &params["item"];
        let command = item["type"] == "commandExecution";
        match (method, item["id"].as_str()) {
            ("item/started", Some(id)) if command && !self.starts.contains_key(id) => {
                self.starts
                    .insert(id.to_owned(), (now(), WorkOrder::next()));
                self.started.push_back(id.to_owned());
            }
            ("item/completed", Some(id)) if command => {
                let Some(all) = item["aggregatedOutput"].as_str() else {
                    return;
                };
                let mut outputs = lock(&self.outputs);
                let printed = outputs.entry(id.to_owned()).or_default();
                let total = all.len() as u64;
                if total >= printed.total {
                    printed.output = tail(all, OUTPUT_LIMIT).to_owned();
                    printed.total = total;
                }
            }
            ("item/commandExecution/outputDelta", _) => {
                let (Some(id), Some(delta)) = (params["itemId"].as_str(), params["delta"].as_str())
                else {
                    return;
                };
                let mut outputs = lock(&self.outputs);
                let printed = outputs.entry(id.to_owned()).or_default();
                printed.output.push_str(delta);
                printed.output = tail(&printed.output, OUTPUT_LIMIT).to_owned();
                printed.total += delta.len() as u64;
            }
            _ => {}
        }
    }

    /// Its app-server listed `listed`: those gone have ended. `stop` stops one by its process.
    pub fn listed(&mut self, listed: Vec<Listed>, stop: impl Fn(&str) -> Stop) {
        self.unknown = None;
        let mut before: HashMap<String, Terminal> = self
            .listed
            .drain(..)
            .map(|terminal| (terminal.id.clone(), terminal))
            .collect();
        for Listed { id, item, command } in listed {
            // (Another command in a terminal of the same id isn't the one known.)
            let same = match before.remove(&id) {
                Some(known) if known.item == item => Some(known),
                Some(other) => {
                    before.insert(format!("{id} ended"), other);
                    None
                }
                None => None,
            };
            let start = item
                .as_ref()
                .and_then(|item| self.starts.get(item))
                .cloned();
            let (since, order) = match (start, &same) {
                (Some(start), _) => start,
                (None, Some(known)) => (known.since, known.order.clone()),
                (None, None) => (now(), WorkOrder::next()),
            };
            let mut shell = same.and_then(|known| known.shell);
            if shell.is_none()
                && let Some(command) = command
            {
                let source = Arc::new(Source {
                    outputs: self.outputs.clone(),
                    item: item.clone(),
                    stop: stop(&id),
                });
                shell = Some(self.background.add(
                    &self.session,
                    &self.cwd,
                    &unwrap_shell(&command),
                    source,
                    since,
                    order.clone(),
                    false,
                ));
            }
            self.listed.push(Terminal {
                id,
                item,
                since,
                order,
                shell,
            });
        }
        for terminal in before.into_values() {
            if let Some(shell) = terminal.shell {
                self.background.end(&shell, None);
            }
        }
        // A recent history for commands not listed yet: listed ones keep their own times.
        while self.started.len() > STARTS_KEPT {
            if let Some(old) = self.started.pop_front() {
                self.starts.remove(&old);
            }
        }
        let live: Vec<&String> = self.listed.iter().filter_map(|t| t.item.as_ref()).collect();
        lock(&self.outputs)
            .retain(|item, _| live.contains(&item) || self.starts.contains_key(item));
        self.tell();
    }

    /// Listing failed. After a turn, what it left running is unknown (and kept a while); later,
    /// those known stay as they were.
    pub fn failed(&mut self, thread: &str, initial: bool) {
        let at = now();
        let fresh = self
            .unknown
            .take()
            .filter(|before| at.saturating_sub(before.since) < AT_WORK_LIMIT);
        self.unknown = if initial {
            Some(Helper {
                id: format!("unknown:{thread}"),
                since: fresh.map_or(at, |before| before.since),
                order: Some(WorkOrder::next()),
            })
        } else {
            fresh
        };
        self.tell();
    }

    /// Its app-server or thread changed, or it closed: what was listed went with it.
    pub fn reset(&mut self) {
        for terminal in self.listed.drain(..) {
            if let Some(shell) = terminal.shell {
                self.background.end(&shell, None);
            }
        }
        lock(&self.outputs).clear();
        self.unknown = None;
        self.starts.clear();
        self.started.clear();
        self.tell();
    }

    /// Terminals without a command, and what isn't known yet, are its tasks.
    fn tell(&self) {
        let mut tasks: Vec<Helper> = self
            .listed
            .iter()
            .filter(|terminal| terminal.shell.is_none())
            .map(|terminal| Helper {
                id: terminal.id.clone(),
                since: terminal.since,
                order: Some(terminal.order.clone()),
            })
            .collect();
        tasks.extend(self.unknown.clone());
        self.background.set_tasks(&self.session, tasks);
    }
}

/// A terminal's command, read from what Codex said it printed and stopped through Codex.
struct Source {
    outputs: Outputs,
    item: Option<String>,
    stop: Stop,
}

impl CommandSource for Source {
    fn output(&self) -> BoxFuture<'_, Printed> {
        let printed = self
            .item
            .as_ref()
            .and_then(|item| lock(&self.outputs).get(item).cloned())
            .unwrap_or_default();
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
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn terminal(id: &str, item: Option<&str>, command: Option<&str>) -> Listed {
        Listed {
            id: id.into(),
            item: item.map(str::to_owned),
            command: command.map(str::to_owned),
        }
    }

    fn never(_: &str) -> Stop {
        Arc::new(|| Box::pin(async {}))
    }

    fn started(item: &str, command: &str) -> Value {
        json!({ "item": { "type": "commandExecution", "id": item, "command": command } })
    }

    #[tokio::test]
    async fn a_terminal_it_leaves_running_is_a_command_with_what_it_printed() {
        let background = Background::default();
        let mut terminals = Terminals::new("ses_a", "/work", background.clone());
        terminals.heard("item/started", &started("c-1", "npm run dev"));
        let delta = json!({ "itemId": "c-1", "delta": "Listening on :3000\n" });
        terminals.heard("item/commandExecution/outputDelta", &delta);
        terminals.listed(
            vec![terminal("p1", Some("c-1"), Some("npm run dev"))],
            never,
        );
        let [job] = &background.running("ses_a", false)[..] else {
            panic!("one command");
        };
        assert_eq!(job.command, "npm run dev");
        let read = background.output(&job.id, 0, None).await.unwrap();
        assert_eq!(read.output, "Listening on :3000\n");
        assert!(terminals.keeps_working());

        // It ended: the next list hasn't it, and nothing keeps the app-server.
        terminals.listed(Vec::new(), never);
        assert!(background.running("ses_a", false).is_empty());
        assert!(!terminals.keeps_working());
    }

    #[test]
    fn a_list_that_fails_keeps_what_was_listed() {
        let background = Background::default();
        let mut terminals = Terminals::new("ses_a", "/work", background.clone());
        terminals.listed(vec![terminal("p1", None, Some("npm run dev"))], never);
        terminals.failed("thr", false);
        terminals.failed("thr", false);
        assert_eq!(background.running("ses_a", false).len(), 1);
        assert!(terminals.keeps_working());
    }

    #[test]
    fn a_failed_first_list_is_an_unknown_task_until_a_list_comes() {
        let background = Background::default();
        let mut terminals = Terminals::new("ses_a", "/work", background.clone());
        terminals.failed("thr", true);
        let work = background.work(false);
        assert_eq!(work["ses_a"].tasks[0].id, "unknown:thr");
        let since = work["ses_a"].tasks[0].since;
        // Failing again keeps its start.
        terminals.failed("thr", true);
        assert_eq!(background.work(false)["ses_a"].tasks[0].since, since);
        assert!(terminals.keeps_working());

        terminals.listed(vec![terminal("p1", None, Some("npm test"))], never);
        let work = background.work(false);
        assert!(work["ses_a"].tasks.is_empty());
        assert_eq!(work["ses_a"].commands[0].command, "npm test");
    }

    #[test]
    fn a_terminal_without_a_command_is_a_task() {
        let background = Background::default();
        let mut terminals = Terminals::new("ses_a", "/work", background.clone());
        terminals.listed(vec![terminal("p1", None, None)], never);
        assert_eq!(background.work(false)["ses_a"].tasks[0].id, "p1");
        terminals.listed(Vec::new(), never);
        assert!(background.work(false).is_empty());
    }

    #[test]
    fn a_command_keeps_when_it_began_across_lists_and_a_replacement_doesnt() {
        let background = Background::default();
        let mut terminals = Terminals::new("ses_a", "/work", background.clone());
        terminals.listed(vec![terminal("p1", Some("c-1"), Some("npm test"))], never);
        let first = background.running("ses_a", false)[0].clone();
        std::thread::sleep(std::time::Duration::from_millis(5));
        terminals.listed(vec![terminal("p1", Some("c-1"), Some("npm test"))], never);
        assert_eq!(background.running("ses_a", false)[0], first);

        // The same process with another item: a new command, with a time of its own.
        terminals.listed(vec![terminal("p1", Some("c-2"), Some("npm test"))], never);
        let [job] = &background.running("ses_a", false)[..] else {
            panic!("one command");
        };
        assert_ne!(job.id, first.id);
        assert!(job.started > first.started);
    }

    #[test]
    fn a_reset_ends_them_and_forgets_their_times() {
        let background = Background::default();
        let mut terminals = Terminals::new("ses_a", "/work", background.clone());
        terminals.listed(vec![terminal("p1", Some("c-1"), Some("npm test"))], never);
        let since = background.running("ses_a", false)[0].started;
        terminals.reset();
        assert!(background.running("ses_a", false).is_empty());
        assert!(!terminals.keeps_working());
        std::thread::sleep(std::time::Duration::from_millis(5));
        // A next app-server can reuse the process and item: a fresh start.
        terminals.listed(vec![terminal("p1", Some("c-1"), Some("npm test"))], never);
        assert!(background.running("ses_a", false)[0].started > since);
    }

    #[tokio::test]
    async fn stop_goes_through_codex() {
        let background = Background::default();
        let mut terminals = Terminals::new("ses_a", "/work", background.clone());
        let stops = Arc::new(AtomicUsize::new(0));
        let counted = stops.clone();
        terminals.listed(
            vec![terminal("p1", None, Some("sleep 9"))],
            move |process| {
                assert_eq!(process, "p1");
                let counted = counted.clone();
                Arc::new(move || {
                    counted.fetch_add(1, Ordering::Relaxed);
                    Box::pin(async {})
                })
            },
        );
        let id = background.running("ses_a", false)[0].id.clone();
        assert!(background.stop(&id).await);
        assert_eq!(stops.load(Ordering::Relaxed), 1);
    }
}
