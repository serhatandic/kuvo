//! What still runs for the engine's sessions once their own turn is over: a command one left
//! running, or a subagent it sent off. Kept as the engine's place in `run/work/background.json`
//! (the coding agents keep theirs, `bridge`, from `kuvo_agents::background`), read by what a lead
//! hears, by a bot's work and by every client. The engine lists its commands a folder at a time,
//! and asking about a folder starts that folder's tools there, so only folders at work are asked
//! about: those its events name, a session's that just ran, and those of the last hour as the box
//! starts. Its events say when to look again; what's found is what the engine says then.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use http::Method;
use kuvo_engine::{BotEngine, EngineEvent};
use kuvo_store::locks::{OnBusy, StaleLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{services::listening_ports, told::now, works_for};
use crate::work::Work;

/// How long a command or subagent that never ends counts as work, from when it started (ms).
pub use kuvo_store::background::AT_WORK_LIMIT;
// As the box starts: the sessions updated this recently have their folders' tools running still.
const RECENT: u64 = 3_600_000;
const WAIT: Duration = Duration::from_secs(10);
const PLACE: &str = "engine";

/// Where in its run something was first seen (`[run, step]`), to tell what began since a question.
pub type WorkOrder = (String, u64);

/// A command a session left running; `ports`: those it listens on, a service's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub command: String,
    pub started: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<WorkOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
}

/// A subagent or another task at work for a session, since when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Helper {
    pub id: String,
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<WorkOrder>,
}

/// What runs for a session besides its own turn.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Running {
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub subagents: Vec<Helper>,
    #[serde(default)]
    pub tasks: Vec<Helper>,
}

impl Running {
    fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.subagents.is_empty() && self.tasks.is_empty()
    }

    /// What of it still counts at `now`; none for nothing.
    pub fn counting(&self, now: u64) -> Option<Running> {
        let fresh = |at: u64| now.saturating_sub(at) < AT_WORK_LIMIT;
        let kept = Running {
            commands: self
                .commands
                .iter()
                .filter(|each| fresh(each.started))
                .cloned()
                .collect(),
            subagents: self
                .subagents
                .iter()
                .filter(|each| fresh(each.since))
                .cloned()
                .collect(),
            tasks: self
                .tasks
                .iter()
                .filter(|each| fresh(each.since))
                .cloned()
                .collect(),
        };
        (!kept.is_empty()).then_some(kept)
    }

    /// What of it its session waits on, services left out; none for nothing.
    pub fn awaited(&self) -> Option<Running> {
        let commands = self
            .commands
            .iter()
            .filter(|each| each.ports.is_empty())
            .cloned()
            .collect();
        let left = Running {
            commands,
            ..self.clone()
        };
        (!left.is_empty()).then_some(left)
    }

    /// Whether any of it started at `at` (ms) or since (in its run's order, where both say one).
    pub fn started_since(&self, at: u64, order: Option<&WorkOrder>) -> bool {
        let since = |started: u64, seen: Option<&WorkOrder>| match (order, seen) {
            (Some(order), Some(seen)) if order.0 == seen.0 => seen.1 >= order.1,
            _ => started >= at,
        };
        self.commands
            .iter()
            .any(|each| since(each.started, each.order.as_ref()))
            || self
                .subagents
                .iter()
                .any(|each| since(each.since, each.order.as_ref()))
            || self
                .tasks
                .iter()
                .any(|each| since(each.since, each.order.as_ref()))
    }

    /// The services among it, in words: "`npm run dev` on :5173".
    pub fn services_words(&self) -> Vec<String> {
        let services = self
            .commands
            .iter()
            .filter(|each| !each.ports.is_empty())
            .take(3);
        let word = |each: &Command| {
            let first: String = each
                .command
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect();
            let ports: Vec<String> = each.ports.iter().map(|port| format!(":{port}")).collect();
            format!("`{first}` on {}", ports.join(", "))
        };
        services.map(word).collect()
    }
}

/// `place`'s part of the work file, the others' as they are, under the file's lock (the TS box's).
pub fn write_place(file: &Path, place: &str, value: Value) {
    let Some(folder) = file.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(folder);
    let (stale, wait) = (Duration::from_secs(10), Duration::from_secs(3));
    let _lock = StaleLock::acquire(
        &folder.join(".background.lock"),
        stale,
        wait,
        OnBusy::Proceed,
    );
    let kept =
        kuvo_store::files::text_of(file).and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let mut all = kept
        .and_then(|kept| kept.as_object().cloned())
        .unwrap_or_default();
    all.insert(place.to_owned(), value);
    let text = serde_json::to_string(&all).expect("JSON");
    let _ = kuvo_store::files::write_secret(file, text.as_bytes());
}

fn hidden_run(session: &Value) -> bool {
    let kuvo = &session["metadata"]["kuvo"];
    ["engineFor", "seat", "reviewPart"]
        .iter()
        .any(|mark| super::truthy(&kuvo[mark]))
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<String, Running>,
    folders: BTreeSet<String>,
    commands_in: BTreeMap<String, Vec<Value>>,
    helpers: BTreeMap<String, Vec<Helper>>,
    above: HashMap<String, String>,
    hidden: HashSet<String>,
    looked_last: String,
    counted: BTreeSet<String>,
    timed: bool,
    ended: bool,
}

/// The engine's place, followed.
#[derive(Clone)]
pub struct EngineBackground {
    work: Work,
    file: PathBuf,
    state: Arc<Mutex<State>>,
    // One look at a time, in order: each goes by what the engine says then.
    looking: Arc<tokio::sync::Mutex<()>>,
    ran_out: Arc<dyn Fn() + Send + Sync>,
}

impl EngineBackground {
    /// `ran_out`: told after a look as something stops counting by its time.
    pub fn new(work: Work, ran_out: Arc<dyn Fn() + Send + Sync>) -> Self {
        let file = work.home.join("run/work/background.json");
        let kept = kuvo_store::files::text_of(&file)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let place = kept.map(|kept| kept[PLACE].clone()).unwrap_or_default();
        let sessions: BTreeMap<String, Running> =
            serde_json::from_value(place["sessions"].clone()).unwrap_or_default();
        let folders = serde_json::from_value(place["folders"].clone()).unwrap_or_default();
        let counted = sessions
            .iter()
            .filter(|(_, work)| work.counting(now()).is_some())
            .map(|(id, _)| id.clone())
            .collect();
        let state = State {
            sessions,
            folders,
            counted,
            ..State::default()
        };
        let (state, looking) = (Arc::new(Mutex::new(state)), Arc::default());
        Self {
            work,
            file,
            state,
            looking,
            ran_out,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("background")
    }

    /// What's known of `session` as of the last look.
    pub fn known(&self, session: &str) -> Option<Running> {
        self.state()
            .sessions
            .get(session)
            .and_then(|work| work.counting(now()))
    }

    // The session at the top of `id`, as far as its parents go; none for one that can't be read.
    async fn top_of(&self, engine: &BotEngine, id: &str) -> Option<String> {
        // (A conversation the session list has, with none above it, is its own top: the engine
        // isn't asked about it.)
        let record = self.work.sessions.latest_record().unwrap_or_default();
        let facts = &record["sessions"][id]["facts"];
        if facts.is_object() && facts["parent"].as_str().is_none_or(str::is_empty) {
            return Some(id.to_owned());
        }
        let mut at = id.to_owned();
        for _ in 0..8 {
            let known = self.state().above.get(&at).cloned();
            let parent = match known {
                Some(parent) => parent,
                None => {
                    let path = format!("api/session/{}", super::encoded(&at));
                    let data = engine
                        .call(Method::GET, &path, None, WAIT)
                        .await
                        .ok()
                        .filter(|data| data["id"].is_string())?;
                    let parent = data["parentID"].as_str().unwrap_or_default().to_owned();
                    let mut state = self.state();
                    if hidden_run(&data) {
                        state.hidden.insert(at.clone());
                    }
                    state.above.insert(at.clone(), parent.clone());
                    parent
                }
            };
            if parent.is_empty() {
                return Some(at);
            }
            at = parent;
        }
        None
    }

    async fn read_engine(
        &self,
        engine: &BotEngine,
        looked: &[String],
    ) -> BTreeMap<String, Running> {
        for folder in looked {
            let Ok(Value::Array(listed)) =
                engine.call_in(Method::GET, "api/shell", folder, WAIT).await
            else {
                continue;
            };
            let running: Vec<Value> = listed
                .into_iter()
                .filter(|job| job["status"] == "running" && job["id"].is_string())
                .collect();
            let mut state = self.state();
            if running.is_empty() {
                state.commands_in.remove(folder);
                state.folders.remove(folder);
            } else {
                state.commands_in.insert(folder.clone(), running);
            }
        }
        // (One that can't be read now keeps what it was found to have.)
        if let Ok(active) = engine
            .call(Method::GET, "api/session/active", None, WAIT)
            .await
        {
            let ids: Vec<String> = match active {
                Value::Array(ids) => ids
                    .iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect(),
                Value::Object(ids) => ids.keys().cloned().collect(),
                _ => Vec::new(),
            };
            let mut found: BTreeMap<String, Vec<Helper>> = BTreeMap::new();
            for id in ids {
                let Some(top) = self.top_of(engine, &id).await else {
                    continue;
                };
                if top == id || self.state().hidden.contains(&id) {
                    continue;
                }
                // (Since it was first found: kept as the box starts again.)
                let was = self.state().sessions.get(&top).and_then(|work| {
                    work.subagents
                        .iter()
                        .find(|each| each.id == id)
                        .map(|each| each.since)
                });
                found.entry(top).or_default().push(Helper {
                    id,
                    since: was.unwrap_or_else(now),
                    order: None,
                });
            }
            self.state().helpers = found;
        }
        let (helpers, commands_in) = {
            let state = self.state();
            (state.helpers.clone(), state.commands_in.clone())
        };
        let mut all: BTreeMap<String, Running> = BTreeMap::new();
        for (top, each) in helpers {
            all.entry(top).or_default().subagents.extend(each);
        }
        // (Each command leads a process group of its own: what listens in it is a service.)
        let groups: Vec<u32> = commands_in
            .values()
            .flatten()
            .filter_map(|job| job["pid"].as_u64())
            .map(|pid| pid as u32)
            .collect();
        let listening = listening_ports(&groups).await;
        for (folder, jobs) in commands_in {
            for job in jobs {
                let Some(session) = job["metadata"]["sessionID"].as_str() else {
                    continue;
                };
                let Some(top) = self.top_of(engine, session).await else {
                    continue;
                };
                let pid = job["pid"].as_u64().map(|pid| pid as u32);
                let ports = pid
                    .and_then(|pid| listening.get(&pid))
                    .cloned()
                    .unwrap_or_default();
                let started = job["time"]["started"]
                    .as_f64()
                    .map_or_else(now, |at| at as u64);
                let command = job["command"].as_str().unwrap_or_default().to_owned();
                let id = job["id"].as_str().unwrap_or_default().to_owned();
                let folder = Some(folder.clone());
                all.entry(top).or_default().commands.push(Command {
                    id,
                    command,
                    started,
                    order: None,
                    folder,
                    ports,
                });
            }
        }
        all
    }

    async fn look(&self, looked: Vec<String>) {
        let _turn = self.looking.lock().await;
        if self.state().ended {
            return;
        }
        let Some(engine) = self.work.list.engine().running_bot_engine().await else {
            return;
        };
        let found = self.read_engine(&engine, &looked).await;
        let (line, folders) = {
            let mut state = self.state();
            if state.ended {
                return;
            }
            state.sessions = found.clone();
            let line = json!([found, state.folders]).to_string();
            let changed = line != state.looked_last;
            state.looked_last = line;
            (changed, state.folders.clone())
        };
        if line {
            let place = json!({ "sessions": found, "folders": folders });
            let file = self.file.clone();
            let _ = tokio::task::spawn_blocking(move || write_place(&file, PLACE, place)).await;
        }
        self.tell_bots(&engine, &found).await;
        self.look_when_one_runs_out();
    }

    /// Clients read a bot's work again when one of its sessions starts or stops counting.
    async fn tell_bots(&self, engine: &BotEngine, after: &BTreeMap<String, Running>) {
        let counts: BTreeSet<String> = after
            .iter()
            .filter(|(_, work)| work.counting(now()).is_some())
            .map(|(id, _)| id.clone())
            .collect();
        let changed: Vec<String> = {
            let mut state = self.state();
            let changed = state
                .counted
                .symmetric_difference(&counts)
                .cloned()
                .collect();
            state.counted = counts;
            changed
        };
        for id in changed {
            let path = format!("api/session/{}", super::encoded(&id));
            let data = engine
                .call(Method::GET, &path, None, WAIT)
                .await
                .unwrap_or_default();
            if let Some(bot) = works_for(&data) {
                self.work.records.note_changed(bot);
            }
        }
    }

    // The next time something counted runs out, so its session stops showing at work then.
    fn look_when_one_runs_out(&self) {
        let now = now();
        let next = {
            let mut state = self.state();
            let times = state.sessions.values().flat_map(|work| {
                let commands = work.commands.iter().map(|each| each.started);
                commands.chain(
                    work.subagents
                        .iter()
                        .chain(&work.tasks)
                        .map(|each| each.since),
                )
            });
            let next = times
                .map(|at| at + AT_WORK_LIMIT)
                .filter(|at| *at > now)
                .min();
            if state.timed || next.is_none() {
                return;
            }
            state.timed = true;
            next.unwrap_or(now)
        };
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(next - now + 1000)).await;
            this.state().timed = false;
            Box::pin(this.look(Vec::new())).await;
            if !this.state().ended {
                (this.ran_out)();
            }
        });
    }

    /// Looks at what runs as the box starts: the folders of sessions updated in the last hour.
    pub async fn start(&self) {
        let Some(engine) = self.work.list.engine().running_bot_engine().await else {
            return;
        };
        let page = engine
            .call(
                Method::GET,
                "api/session?limit=50&order=desc&parentID=null",
                None,
                WAIT,
            )
            .await;
        let recent = now().saturating_sub(RECENT);
        for each in page
            .ok()
            .and_then(|page| page.as_array().cloned())
            .unwrap_or_default()
        {
            let folder = super::lines::folder_of(&each);
            if !folder.is_empty()
                && each["time"]["updated"]
                    .as_f64()
                    .is_some_and(|at| at as u64 > recent)
            {
                self.state().folders.insert(folder.to_owned());
            }
        }
        let folders = self.state().folders.iter().cloned().collect();
        self.look(folders).await;
    }

    /// The folder's commands and what's at work looked at now: what `session` has running.
    pub async fn of(&self, session: &str, folder: &str) -> Option<Running> {
        let looked: BTreeSet<String> = {
            let mut state = self.state();
            let theirs = state.sessions.get(session).map(|work| {
                work.commands
                    .iter()
                    .filter_map(|each| each.folder.clone())
                    .collect::<Vec<_>>()
            });
            let looked: BTreeSet<String> = std::iter::once(folder.to_owned())
                .chain(theirs.unwrap_or_default())
                .filter(|each| !each.is_empty())
                .collect();
            state.folders.extend(looked.iter().cloned());
            looked
        };
        self.look(looked.into_iter().collect()).await;
        self.known(session)
    }

    /// Each event, looked at again if it says something changed; whether it was.
    pub async fn noted(&self, event: &EngineEvent) -> bool {
        let folder = event.location["directory"]
            .as_str()
            .or(event.data["info"]["cwd"].as_str())
            .map(str::to_owned);
        if ["shell.created", "shell.exited", "shell.deleted"].contains(&event.kind.as_str()) {
            let looked = match folder {
                Some(folder) => {
                    self.state().folders.insert(folder.clone());
                    vec![folder]
                }
                None => self.state().folders.iter().cloned().collect(),
            };
            self.look(looked).await;
            return true;
        }
        let kind = event.kind.as_str();
        if kind != "session.background.updated"
            && kind != "session.deleted"
            && !kind.starts_with("session.execution.")
        {
            return false;
        }
        self.look(Vec::new()).await;
        true
    }

    /// The engine stopped: what ran there went with it.
    pub fn gone(&self) {
        {
            let mut state = self.state();
            state.sessions.clear();
            state.looked_last.clear();
        }
        let kept = kuvo_store::files::text_of(&self.file)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let had = kept
            .as_ref()
            .and_then(|kept| kept[PLACE]["sessions"].as_object())
            .is_some_and(|sessions| !sessions.is_empty());
        if had {
            let folders = self.state().folders.clone();
            write_place(
                &self.file,
                PLACE,
                json!({ "sessions": {}, "folders": folders }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(started: u64, ports: Vec<u16>) -> Command {
        Command {
            id: "cmd".into(),
            command: "npm run dev\nmore".into(),
            started,
            order: None,
            folder: None,
            ports,
        }
    }

    #[test]
    fn what_a_finish_waits_on_and_what_it_says_is_still_serving() {
        let work = Running {
            commands: vec![command(10, vec![5173]), command(20, vec![])],
            ..Running::default()
        };
        assert_eq!(
            work.awaited().map(|left| left.commands.len()),
            Some(1),
            "a service isn't waited on"
        );
        assert_eq!(work.services_words(), vec!["`npm run dev` on :5173"]);
        assert!(work.started_since(15, None));
        assert!(!work.started_since(25, None));
        let ordered = Running {
            tasks: vec![Helper {
                id: "t".into(),
                since: 1,
                order: Some(("run".into(), 4)),
            }],
            ..Running::default()
        };
        assert!(
            ordered.started_since(99, Some(&("run".into(), 3))),
            "its run's order goes before the clock"
        );
        assert_eq!(
            work.counting(20 + AT_WORK_LIMIT),
            None,
            "past its limit it doesn't count"
        );
        let served = Running {
            commands: vec![command(10, vec![80])],
            ..Running::default()
        };
        assert_eq!(served.awaited(), None);
    }
}
