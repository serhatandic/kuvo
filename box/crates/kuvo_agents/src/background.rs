//! What still runs for a conversation besides its own turn, as the coding agents say it: the
//! commands an agent left running (and those its turn waits on, `foreground`, which end with their
//! tool call), each with what it has printed so far and a way to stop it; and the other tasks an
//! agent keeps going. The agent that started each says when it ends. Subagents at work under a
//! conversation are 4.6's to add. The resident publishes what's here as its place in
//! `run/work/background.json`, which the work routes read.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// How long unseen or unending background work can hold a finish (ms).
pub use kuvo_store::background::AT_WORK_LIMIT;

/// How long an ended command's output can still be read.
const KEPT_AFTER: Duration = Duration::from_secs(10 * 60);

/// The order of what this process saw, independent of its wall clock: `[origin, sequence]`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkOrder(pub String, pub u64);

impl WorkOrder {
    pub fn next() -> Self {
        static ORIGIN: OnceLock<String> = OnceLock::new();
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let origin = ORIGIN.get_or_init(|| uuid::Uuid::new_v4().to_string());
        Self(origin.clone(), SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1)
    }
}

/// Now, in ms since 1970.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// A command an agent runs, as the bar lists it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Command {
    pub id: String,
    pub running: bool,
    pub command: String,
    /// The folder it runs in.
    pub cwd: String,
    pub exit: Option<i32>,
    pub session: String,
    pub started: u64,
    pub order: WorkOrder,
    /// Its turn waits on it: it ends with its tool call, never after the turn.
    pub foreground: bool,
}

/// The end of what a command printed, and how much it printed in all (bytes).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Printed {
    pub output: String,
    pub total: u64,
}

/// What it printed from byte `cursor` on, as the engine reads a command's output.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub output: String,
    pub cursor: u64,
    pub size: u64,
}

/// Where a command's output is read and how it's stopped: its agent's.
pub trait CommandSource: Send + Sync {
    fn output(&self) -> BoxFuture<'_, Printed>;
    fn stop(&self) -> BoxFuture<'_, ()>;
    fn pushes_output(&self) -> bool { false }
}

/// A subagent or task at work, since when (ms).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Helper {
    pub id: String,
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<WorkOrder>,
}

/// A command as the work file keeps it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Running {
    pub id: String,
    pub command: String,
    pub started: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<WorkOrder>,
}

/// What runs for a session besides its own turn.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Work {
    #[serde(default)]
    pub commands: Vec<Running>,
    #[serde(default)]
    pub subagents: Vec<Helper>,
    /// What else its agent keeps going.
    #[serde(default)]
    pub tasks: Vec<Helper>,
}

impl Work {
    fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.subagents.is_empty() && self.tasks.is_empty()
    }
}

struct Kept {
    job: Command,
    source: Arc<dyn CommandSource>,
    ended: Option<Instant>,
}

#[derive(Default)]
struct State {
    commands: HashMap<String, Kept>,
    /// Each session's tasks, by the agent thread that keeps them.
    tasks: HashMap<String, Vec<Helper>>,
}

/// The commands and tasks the agents here have going: one for the resident, shared with each
/// conversation's thread.
#[derive(Clone)]
pub struct Background {
    state: Arc<Mutex<State>>,
    changed: Arc<watch::Sender<u64>>,
    output_changed: Arc<watch::Sender<u64>>,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            changed: Arc::new(watch::channel(0).0),
            output_changed: Arc::new(watch::channel(0).0),
        }
    }
}

impl Background {
    fn change<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        // (An ended one's output stays readable a while.)
        state
            .commands
            .retain(|_, kept| kept.ended.is_none_or(|at| at.elapsed() < KEPT_AFTER));
        let result = change(&mut state);
        drop(state);
        self.changed.send_modify(|count| *count += 1);
        result
    }

    fn read<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        read(&self.state.lock().unwrap_or_else(|error| error.into_inner()))
    }

    /// Told each time something here changes.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub fn output_changes(&self) -> watch::Receiver<u64> { self.output_changed.subscribe() }
    pub(crate) fn note_output(&self) { self.output_changed.send_modify(|count| *count = count.wrapping_add(1)); }
    pub fn output_is_pushed(&self, id: &str) -> bool {
        self.read(|state| state.commands.get(id).is_some_and(|kept| kept.source.pushes_output()))
    }

    /// A command an agent left running (or, `foreground`, one its turn waits on): its id here.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        session: &str,
        cwd: &str,
        command: &str,
        source: Arc<dyn CommandSource>,
        started: u64,
        order: WorkOrder,
        foreground: bool,
    ) -> String {
        let id = format!("shl_{}", uuid::Uuid::new_v4().simple());
        let job = Command {
            id: id.clone(),
            running: true,
            command: command.to_owned(),
            cwd: cwd.to_owned(),
            exit: None,
            session: session.to_owned(),
            started,
            order,
            foreground,
        };
        self.restore(job, source);
        id
    }

    /// Rebind a running command to its adopted agent, preserving the public id.
    pub(crate) fn restore(&self, job: Command, source: Arc<dyn CommandSource>) {
        self.change(|state| {
            state.commands.insert(
                job.id.clone(),
                Kept {
                    job,
                    source,
                    ended: None,
                },
            )
        });
    }

    /// It ended (or its agent did): the bar lets it go.
    pub fn end(&self, id: &str, exit: Option<i32>) {
        self.change(|state| {
            if let Some(kept) = state.commands.get_mut(id)
                && kept.job.running
            {
                kept.job.running = false;
                kept.job.exit = exit.or(kept.job.exit);
                kept.ended = Some(Instant::now());
            }
        });
    }

    pub fn command(&self, id: &str) -> Option<Command> {
        self.read(|state| state.commands.get(id).map(|kept| kept.job.clone()))
    }

    /// Every command known, running or lately ended.
    pub fn commands(&self) -> Vec<Command> {
        self.read(|state| {
            let mut all: Vec<_> = state
                .commands
                .values()
                .map(|kept| kept.job.clone())
                .collect();
            all.sort_by_key(|job| job.started);
            all
        })
    }

    /// A session's commands running now that it left running (with `foreground`, its turn's too).
    pub fn running(&self, session: &str, foreground: bool) -> Vec<Command> {
        self.commands()
            .into_iter()
            .filter(|job| job.running && job.session == session && (foreground || !job.foreground))
            .collect()
    }

    /// What it printed from byte `cursor` on, `limit` bytes at most. Only its end is known: what's
    /// asked from before that comes from where it begins.
    pub async fn output(&self, id: &str, cursor: u64, limit: Option<u64>) -> Option<Read> {
        let source = self.read(|state| state.commands.get(id).map(|kept| kept.source.clone()))?;
        let printed = source.output().await;
        let bytes = printed.output.as_bytes();
        let total = printed.total.max(bytes.len() as u64);
        let known = total - bytes.len() as u64;
        let start = cursor.max(known);
        let end = limit.map_or(total, |limit| total.min(cursor + limit));
        let output = if start < end {
            String::from_utf8_lossy(&bytes[(start - known) as usize..(end - known) as usize])
                .into_owned()
        } else {
            String::new()
        };
        Some(Read {
            output,
            cursor: start.max(end),
            size: total,
        })
    }

    /// Stop it, if it still runs. False for one not known here.
    pub async fn stop(&self, id: &str) -> bool {
        let Some((source, running)) = self.read(|state| {
            state
                .commands
                .get(id)
                .map(|kept| (kept.source.clone(), kept.job.running))
        }) else {
            return false;
        };
        if running {
            source.stop().await;
        }
        true
    }

    /// The tasks an agent keeps going for `session` now (neither commands nor subagents).
    pub fn set_tasks(&self, session: &str, tasks: Vec<Helper>) {
        if self.read(|state| {
            state.tasks.get(session) == Some(&tasks)
                || (tasks.is_empty() && !state.tasks.contains_key(session))
        }) {
            return;
        }
        self.change(|state| {
            if tasks.is_empty() {
                state.tasks.remove(session);
            } else {
                state.tasks.insert(session.to_owned(), tasks);
            }
        });
    }

    /// What still runs for each session besides its own turn: commands it left running and other
    /// tasks its agent keeps going. With `foreground` (what clients list), the commands its turn
    /// waits on too.
    pub fn work(&self, foreground: bool) -> BTreeMap<String, Work> {
        let mut all: BTreeMap<String, Work> = BTreeMap::new();
        for job in self.commands() {
            if job.running && (foreground || !job.foreground) {
                all.entry(job.session.clone())
                    .or_default()
                    .commands
                    .push(Running {
                        id: job.id,
                        command: job.command,
                        started: job.started,
                        order: Some(job.order),
                    });
            }
        }
        self.read(|state| {
            for (session, tasks) in &state.tasks {
                all.entry(session.clone())
                    .or_default()
                    .tasks
                    .extend(tasks.iter().cloned());
            }
        });
        all.retain(|_, work| !work.is_empty());
        all
    }
}

impl Background {
    /// Keep `file` (`run/work/background.json`) saying what runs here, as its `bridge` place,
    /// what clients list (commands its turns wait on too): now, and as it changes. Other places
    /// there stay as they are.
    pub fn publish(&self, file: std::path::PathBuf) -> tokio::task::JoinHandle<()> {
        let background = self.clone();
        let mut changes = self.changes();
        tokio::spawn(async move {
            let mut written = None;
            loop {
                let work = background.work(true);
                if written.as_ref() != Some(&work) {
                    let place = serde_json::json!({ "sessions": &work, "folders": [] });
                    let file = file.clone();
                    let saved =
                        tokio::task::spawn_blocking(move || write_place(&file, "bridge", place))
                            .await;
                    match saved {
                        Ok(Ok(())) => written = Some(work),
                        Ok(Err(error)) => {
                            eprintln!("Kuvo could not say what its agents keep going: {error}")
                        }
                        Err(_) => return,
                    }
                }
                if changes.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}

/// `place`'s part of the work file, the others' as they are.
fn write_place(
    file: &std::path::Path,
    from: &str,
    place: serde_json::Value,
) -> std::io::Result<()> {
    // (Under the file's lock, as the engine's place is written beside it.)
    // Do not create the work directory or a lock for an empty pristine place.
    let empty = place["sessions"].as_object().is_some_and(|rows| rows.is_empty())
        && place["folders"].as_array().is_some_and(|rows| rows.is_empty());
    if empty && !file.exists() { return Ok(()); }
    let lock = file.with_file_name(".background.lock");
    let (stale, wait) = (
        std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(3),
    );
    let _lock = kuvo_store::locks::StaleLock::acquire(
        &lock,
        stale,
        wait,
        kuvo_store::locks::OnBusy::Proceed,
    );
    let mut all = std::fs::read(file)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).ok()
        })
        .unwrap_or_default();
    if all.get(from) == Some(&place) || empty && !all.contains_key(from) { return Ok(()); }
    all.insert(from.to_owned(), place);
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    kuvo_store::files::write_atomic(file, &serde_json::to_vec(&all)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Printing(&'static str, u64);

    impl CommandSource for Printing {
        fn output(&self) -> BoxFuture<'_, Printed> {
            Box::pin(async move {
                Printed {
                    output: self.0.to_owned(),
                    total: self.1,
                }
            })
        }
        fn stop(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn a_command_left_running_is_listed_read_and_let_go() {
        let background = Background::default();
        let mut changes = background.changes();
        let id = background.add(
            "ses_a",
            "/work",
            "npm run dev",
            Arc::new(Printing("ready on :3000\n", 115)),
            10,
            WorkOrder::next(),
            false,
        );
        let waited = background.add(
            "ses_a",
            "/work",
            "npm test",
            Arc::new(Printing("", 0)),
            20,
            WorkOrder::next(),
            true,
        );
        assert!(changes.has_changed().unwrap());
        changes.mark_unchanged();
        // What its turn waits on isn't left running; clients list it.
        assert_eq!(background.work(false)["ses_a"].commands.len(), 1);
        assert_eq!(background.work(true)["ses_a"].commands.len(), 2);
        // Only the end of what it printed is known: what's asked from before comes from there.
        let read = background.output(&id, 0, Some(24_000)).await.unwrap();
        assert_eq!(
            (read.output.as_str(), read.cursor, read.size),
            ("ready on :3000\n", 115, 115)
        );
        let tail = background.output(&id, 110, None).await.unwrap();
        assert_eq!(tail.output, "3000\n");
        background.end(&id, Some(0));
        background.end(&waited, None);
        assert!(changes.has_changed().unwrap());
        assert!(background.work(true).is_empty());
        assert_eq!(background.command(&id).unwrap().exit, Some(0));
        assert!(background.output(&id, 0, None).await.is_some());
        background.set_tasks(
            "ses_b",
            vec![Helper {
                id: "t1".into(),
                since: 5,
                order: None,
            }],
        );
        assert_eq!(background.work(false)["ses_b"].tasks.len(), 1);
        background.set_tasks("ses_b", Vec::new());
        assert!(background.work(false).is_empty());
    }
}
