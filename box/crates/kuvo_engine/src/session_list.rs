//! A local session list that can be followed by the line and read by the peers crate.

mod activity;
mod engine_facts;
mod work;

use engine_facts::EngineFacts;

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    sync::Arc,
};

use http::{Method, StatusCode};
use kuvo_os::watch::{Notice, Options, Subscription, WatchHub};
use kuvo_protocol::routes::{
    get_kuvo_sessions_list as read, get_kuvo_sessions_list_events as events,
    get_kuvo_sessions_page as page,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Engine, api,
    lifetime::{Demand, Lease},
    sessions::{Session, typed},
};

pub use activity::{Activity, ActivityState};

#[derive(Clone)]
pub struct LocalSessions {
    snapshot: watch::Receiver<Option<Arc<Snapshot>>>,
    refresh: mpsc::Sender<()>,
    ask_changes: watch::Receiver<Arc<crate::asks::Book>>,
    _tasks: Arc<ListTasks>,
}

struct ListTasks {
    list: tokio::task::JoinHandle<()>,
    feed: tokio::task::JoinHandle<()>,
    models: tokio::task::JoinHandle<()>,
    limits: tokio::task::JoinHandle<()>,
    stop: CancellationToken,
}

impl Drop for ListTasks {
    fn drop(&mut self) {
        self.stop.cancel();
        self.feed.abort();
        self.list.abort();
        self.models.abort();
        self.limits.abort();
    }
}

impl LocalSessions {
    pub(crate) fn names(&self, here: Option<&str>) -> Vec<crate::naming::Name> {
        use crate::naming::{Kind as AgentKind, Name};
        let current = self.snapshot.borrow();
        let Some(snapshot) = current.as_deref() else {
            return Vec::new();
        };
        let mut rows = snapshot.sessions.iter().collect::<Vec<_>>();
        rows.sort_by(|(a, one), (b, other)| {
            other
                .facts
                .updated
                .cmp(&one.facts.updated)
                .then_with(|| a.cmp(b))
        });
        rows.into_iter()
            .filter_map(|(id, row)| {
                let facts = &row.facts;
                if facts.flag("archived") || facts.flag("away") {
                    return None;
                }
                let title = facts
                    .extra
                    .get("title")?
                    .as_str()
                    .filter(|title| !title.is_empty() && *title != "New session")?;
                let kind = match facts.kind {
                    Kind::Bot => AgentKind::Bot,
                    Kind::Project | Kind::Chat => AgentKind::Session,
                    Kind::Hidden | Kind::Seat | Kind::Subagent => return None,
                };
                let project = facts
                    .project()
                    .and_then(|folder| {
                        snapshot
                            .projects
                            .get(folder)
                            .and_then(|project| project["name"].as_str())
                            .or_else(|| {
                                Path::new(folder).file_name().and_then(|name| name.to_str())
                            })
                    })
                    .map(str::to_owned);
                Some(Name {
                    session: id.clone(),
                    name: title.to_owned(),
                    kind,
                    project,
                    machine: here.map(str::to_owned),
                    about: None,
                })
            })
            .collect()
    }

    /// Only changes to the archived inventory need another owner-menu read.
    pub fn archive_revision(&self) -> Option<ArchiveRevision> {
        let snapshot = self.snapshot.borrow();
        let snapshot = snapshot.as_deref()?;
        let rows = snapshot
            .archived
            .iter()
            .flat_map(|group| group["sessions"].as_array().into_iter().flatten())
            .filter_map(|id| snapshot.sessions.get(id.as_str()?).map(|row| row.seq))
            .collect();
        Some(ArchiveRevision {
            groups: snapshot.archived.clone(),
            rows,
            pending: snapshot.pending.clone(),
            epoch: snapshot.epoch.clone(),
        })
    }

    /// The compatible on-disk representation, passed in memory to the peer publisher.
    pub fn latest_record(&self) -> Option<Value> {
        serde_json::to_value(self.snapshot.borrow().as_deref().as_ref()?).ok()
    }

    pub(crate) fn snapshot_now(&self) -> Option<Arc<Snapshot>> {
        self.snapshot.borrow().clone()
    }

    pub(crate) fn knows_folder(&self, matches: impl Fn(&str) -> bool) -> bool {
        self.snapshot.borrow().as_ref().is_some_and(|snapshot| {
            snapshot
                .sessions
                .values()
                .any(|row| matches(&row.facts.directory))
        })
    }

    pub async fn changed(&mut self) -> bool {
        self.snapshot.changed().await.is_ok()
    }

    pub(crate) fn start(engine: Engine) -> Self {
        let (refresh, mut requests) = mpsc::channel(1);
        let (updates, snapshot) = watch::channel(None);
        let stop = CancellationToken::new();
        let (changed, revisions) = watch::channel(0);
        let agent_revision = changed.clone();
        let (ask_updates, mut ask_changes) = watch::channel(Arc::new(crate::asks::Book::default()));
        let (thinking, mut thought) = watch::channel(Arc::new(BTreeSet::new()));
        let (models_changed, models) = crate::model_refresh::watch(engine.clone());
        let limits = crate::queue::watch_limits(engine.clone(), stop.clone());
        let feed = crate::conversation::watch(
            engine.clone(),
            refresh.downgrade(),
            changed,
            ask_updates,
            models_changed,
            thinking,
            stop.clone(),
        );
        let known_asks = ask_changes.clone();
        let mut external = engine.external_asks.as_ref().map(|source| source.waiting());
        let mut agent_runs = engine
            .agent_sessions
            .as_ref()
            .and_then(|source| source.running_changes());
        let list = tokio::spawn(async move {
            let kept = load(&engine.home).await;
            let existed = kept.is_some();
            let mut last = kept.unwrap_or_else(Snapshot::new);
            if last.owner.is_some() {
                last.seq += 1_000_000;
                last.floor = last.seq;
            }
            last.owner = Some(std::process::id());
            last.started = Some(format!("{:x}-{:x}", crate::disk::now(), std::process::id()));
            if existed {
                let mut cached = last.clone();
                // The coding agents' rows are in-process and come with the
                // first read; only the engine's may still be starting.
                cached.pending = vec!["engine".to_owned()];
                updates.send_replace(Some(Arc::new(cached)));
            }
            let mut first = true;
            let mut state = engine.states();
            let files = WatchHub::default();
            let mut edits = files
                .subscribe(
                    engine.home.join("box"),
                    Options {
                        create: true,
                        ..Options::default()
                    },
                )
                .await
                .ok();
            let mut background_edits = files
                .subscribe(
                    engine.home.join("run/work"),
                    Options {
                        create: true,
                        ..Options::default()
                    },
                )
                .await
                .ok();
            let mut agent_edits = if engine.agent_sessions.is_some() {
                files
                    .subscribe(
                        engine.home.join("agents/sessions"),
                        Options {
                            create: true,
                            recursive: true,
                        },
                    )
                    .await
                    .ok()
            } else {
                None
            };
            // Each subscription begins with a synthetic Rescan. The initial inventory is
            // taken after the watches are armed, so that notice is already accounted for.
            // Later rescan/failure notices really do mean a possibly missed change.
            for subscription in [&mut edits, &mut background_edits, &mut agent_edits]
                .into_iter()
                .flatten()
            {
                let _ = subscription.recv().await;
            }
            // The agents' saved rows have their own source. Keep their baseline even
            // while the first engine inventory is in flight: a watcher rescan of an
            // unchanged agent directory must not invalidate that engine page.
            let mut agents = agent_rows(&engine).await.map(Arc::new);
            let mut inventory: Option<(u64, Inventory, EngineFacts)> = None;
            let mut reads = JoinSet::new();
            let mut read_again = true;
            loop {
                if read_again && reads.is_empty() {
                    read_again = false;
                    let at = *revisions.borrow();
                    let reader = engine.clone();
                    let previous = inventory
                        .as_ref()
                        .map(|(_, _, facts)| Arc::clone(&facts.databases));
                    let bridge_rows = agents.clone();
                    reads.spawn(async move {
                        let (rows, mut facts) = tokio::join!(
                            Inventory::read(&reader, bridge_rows),
                            engine_facts::read(&reader.home, previous)
                        );
                        let rows = rows?;
                        facts.add_linked(rows.rows()).await;
                        Some((at, rows, facts))
                    });
                }
                tokio::select! {
                    biased;
                    next = requests.recv() => {
                        if next.is_none() { break; }
                        refresh_agents(&engine, &mut agents, &agent_revision).await;
                        read_again = true;
                        continue;
                    },
                    changed = ask_changes.changed() => {
                        if changed.is_err() { break; }
                    },
                    changed = thought.changed() => {
                        if changed.is_err() { break; }
                        if !first && last.apply_thinking(&thought.borrow_and_update()) {
                            let listed = publish(&updates, &last);
                            save(&engine.home, listed).await;
                        }
                        continue;
                    },
                    changed = async {
                        match &mut external {
                            Some(external) => external.changed().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if changed.is_err() { external = None; }
                    },
                    changed = state.changed() => {
                        if changed.is_err() { break; }
                        refresh_agents(&engine, &mut agents, &agent_revision).await;
                        read_again = true;
                        continue;
                    },
                    changed = project_changes(&mut edits) => match changed {
                        None => { edits = None; continue; },
                        Some(()) => {},
                    },
                    changed = background_changes(&mut background_edits) => match changed {
                        None => { background_edits = None; continue; },
                        Some(()) => {},
                    },
                    changed = agent_changes(&mut agent_edits) => {
                        match changed {
                            Some(()) => {
                                // A watch can report a stream reconfiguration, a
                                // rewritten identical record or another event in
                                // the agent folder. Recheck the agent rows, but
                                // don't invalidate OpenCode's unchanged inventory.
                                read_again |= refresh_agents(&engine, &mut agents, &agent_revision).await;
                            }
                            None => agent_edits = None,
                        }
                        continue;
                    },
                    changed = async {
                        match &mut agent_runs {
                            Some(changes) => changes.changed().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if changed.is_ok() {
                            read_again |= refresh_agents(&engine, &mut agents, &agent_revision).await;
                        } else {
                            agent_runs = None;
                        }
                        continue;
                    },
                    finished = reads.join_next(), if !reads.is_empty() => {
                        let Some(Ok(Some((at, rows, facts)))) = finished else { continue; };
                        if at != *revisions.borrow() { continue; }
                        inventory = Some((at, rows, facts));
                    },
                }
                let Some((began, inventory, facts)) = &inventory else {
                    // Mark an ask heard even while there is no complete engine inventory.
                    ask_changes.borrow_and_update();
                    continue;
                };
                let mut book = ask_changes.borrow_and_update().clone();
                if let Some(external) = &external {
                    crate::asks::include_external(Arc::make_mut(&mut book), &external.borrow());
                }
                let (projects, worktrees) = tokio::join!(
                    read_projects(&engine.home),
                    crate::worktree_notes::read(&engine.home)
                );
                let Ok(worktrees) = worktrees else {
                    continue;
                };
                let (mut sessions, projects, archived, needs_you) = measure(
                    inventory.rows(),
                    inventory.retained(&last),
                    &engine.home,
                    &projects,
                    &worktrees.filed,
                    facts,
                    &book,
                );
                let published = {
                    let revision = revisions.borrow();
                    if *revision != *began {
                        // The feed queued another read. A stale page cannot revive a deleted row.
                        continue;
                    }
                    let thinking = thought.borrow_and_update();
                    for (id, facts) in &mut sessions {
                        facts.thinking(thinking.contains(id));
                    }
                    let write_first = std::mem::take(&mut first);
                    (last.commit(sessions, projects, archived, needs_you, inventory) || write_first)
                        .then(|| publish(&updates, &last))
                };
                if let Some(saved) = published {
                    save(&engine.home, saved).await;
                }
            }
        });
        Self {
            snapshot,
            refresh,
            ask_changes: known_asks,
            _tasks: Arc::new(ListTasks {
                list,
                feed,
                models,
                limits,
                stop,
            }),
        }
    }

    pub(crate) fn ask_changes(&self) -> watch::Receiver<Arc<crate::asks::Book>> {
        let mut changes = self.ask_changes.clone();
        changes.mark_unchanged();
        changes
    }

    pub(crate) fn refresh(&self) {
        let _ = self.refresh.try_send(());
    }

    pub(crate) async fn current(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<Arc<Snapshot>, Refusal> {
        let current = self.snapshot.borrow_and_update().clone();
        if let Some(current) = current {
            return Ok(current);
        }
        self.refresh();
        tokio::select! {
            result = time::timeout(api::WAIT, self.snapshot.wait_for(Option::is_some)) => {
                if let Ok(Ok(snapshot)) = result {
                    return Ok(Arc::clone(snapshot.as_ref().expect("waited for a snapshot")));
                }
            }
            () = cancel.cancelled() => {}
        }
        Err(Refusal::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The engine isn’t running.",
        ))
    }
}

fn publish(updates: &watch::Sender<Option<Arc<Snapshot>>>, snapshot: &Snapshot) -> Arc<Snapshot> {
    let published = Arc::new(snapshot.clone());
    updates.send_replace(Some(published.clone()));
    published
}

async fn save(home: &Path, snapshot: Arc<Snapshot>) {
    let path = home.join("run/lists/sessions.json");
    let _ = tokio::task::spawn_blocking(move || crate::disk::write(&path, snapshot.as_ref())).await;
}

#[derive(PartialEq, Eq)]
pub struct ArchiveRevision {
    groups: Vec<Value>,
    rows: Vec<u64>,
    pending: Vec<String>,
    epoch: String,
}

type Sessions = BTreeMap<String, Facts>;
type Projects = BTreeMap<String, Value>;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    epoch: String,
    seq: u64,
    floor: u64,
    sessions: BTreeMap<String, Listed>,
    gone: BTreeMap<String, Gone>,
    projects: Projects,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<String>,
    // Older homes need neither of these derived fields in order to open the list.
    #[serde(default, skip_serializing)]
    archived: Vec<Value>,
    #[serde(default, rename = "needsYou")]
    needs_you: Option<Vec<Value>>,
    #[serde(default, rename = "workByBot")]
    work_by_bot: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Listed {
    facts: Facts,
    seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    place: Option<String>,
    /// A bot's work's or a subagent's work fields (`work::listed_work`), beside its facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work: Option<Value>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Gone {
    seq: u64,
    at: u64,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Project,
    Chat,
    Subagent,
    Bot,
    Seat,
    Hidden,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
struct Facts {
    kind: Kind,
    directory: String,
    created: u64,
    updated: u64,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
    /// Kept beside the facts in the list (`Listed::work`), never among them.
    #[serde(skip)]
    work: Option<Value>,
}

impl Facts {
    fn flag(&self, field: &str) -> bool {
        self.extra.get(field) == Some(&Value::Bool(true))
    }

    fn thinking(&mut self, should_show: bool) -> bool {
        let thinking = should_show && self.flag("running");
        if thinking == self.flag("thinking") {
            return false;
        }
        if thinking {
            self.extra.insert("thinking".into(), json!(true));
        } else {
            self.extra.remove("thinking");
        }
        true
    }

    fn waiting(&self) -> bool {
        matches!(self.kind, Kind::Project | Kind::Chat | Kind::Bot)
            && self.extra.contains_key("asked")
            && !self.flag("archived")
            && !self.flag("away")
    }

    fn project(&self) -> Option<&str> {
        self.extra.get("project").and_then(Value::as_str)
    }
}

impl Snapshot {
    fn apply_thinking(&mut self, thinking: &BTreeSet<String>) -> bool {
        let next = self.seq + 1;
        let mut changed = false;
        for (session, row) in &mut self.sessions {
            if row.facts.thinking(thinking.contains(session)) {
                row.seq = next;
                changed = true;
            }
        }
        if changed {
            self.seq = next;
        }
        changed
    }

    pub(crate) fn has_local(&self, id: &str) -> bool {
        self.sessions
            .get(id)
            .is_some_and(|row| !row.facts.flag("away"))
    }

    pub(crate) fn waiting_sessions(&self) -> usize {
        self.sessions
            .values()
            .filter(|row| row.facts.waiting())
            .count()
    }

    pub(crate) fn asking(&self, id: &str) -> bool {
        self.sessions.get(id).is_some_and(|row| {
            row.facts.extra.contains_key("asked")
                && !row.facts.flag("archived")
                && !row.facts.flag("away")
        })
    }

    pub(crate) fn bot_waits(&self, root: &str, bot: &str) -> bool {
        self.sessions.iter().any(|(id, row)| {
            let facts = &row.facts;
            if facts.flag("archived")
                || facts.flag("away")
                || facts.kind == Kind::Hidden
                || facts.flag("reviewPart")
                || !(facts.flag("ownAsk")
                    || facts.extra.contains_key("asked") && !facts.flag("childAsk"))
            {
                return false;
            }
            let belongs =
                |facts: &Facts| facts.extra.get("bot").and_then(Value::as_str) == Some(bot);
            if id == root || belongs(facts) {
                return true;
            }
            let parents = |facts: &Facts| {
                facts
                    .extra
                    .get("parent")
                    .and_then(Value::as_str)
                    .into_iter()
                    .chain(
                        facts
                            .extra
                            .get("line")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str),
                    )
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            let mut ancestors = std::collections::VecDeque::from(parents(facts));
            let mut seen = BTreeSet::new();
            while seen.len() < 24 {
                let Some(parent) = ancestors.pop_front() else {
                    break;
                };
                if parent == *id || !seen.insert(parent.clone()) {
                    continue;
                }
                if parent == root {
                    return true;
                }
                if let Some(above) = self.sessions.get(&parent) {
                    if belongs(&above.facts) {
                        return true;
                    }
                    ancestors.extend(parents(&above.facts));
                }
            }
            false
        })
    }

    fn new() -> Self {
        Self {
            epoch: format!("{:x}-{:x}", crate::disk::now(), std::process::id()),
            seq: 0,
            floor: 0,
            sessions: BTreeMap::new(),
            gone: BTreeMap::new(),
            projects: BTreeMap::new(),
            pending: Vec::new(),
            archived: Vec::new(),
            needs_you: Some(Vec::new()),
            work_by_bot: BTreeMap::new(),
            owner: None,
            started: None,
        }
    }

    fn commit(
        &mut self,
        fresh: Sessions,
        projects: Projects,
        archived: Vec<Value>,
        needs_you: Vec<Value>,
        inventory: &Inventory,
    ) -> bool {
        let pending = inventory.pending();
        let mut changed = self.seq == 0
            || self.needs_you.as_ref() != Some(&needs_you)
            || self.pending != pending
            || self.projects != projects
            || self.archived != archived;
        let next = self.seq + 1;
        self.pending = pending;
        self.needs_you = Some(needs_you);
        let removed = self
            .sessions
            .keys()
            .filter(|id| !fresh.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed {
            self.sessions.remove(&id);
            self.gone.insert(
                id,
                Gone {
                    seq: next,
                    at: crate::disk::now(),
                },
            );
            changed = true;
        }
        for (id, facts) in fresh {
            let place = inventory.place(&id, self.sessions.get(&id));
            if self
                .sessions
                .get(&id)
                .is_some_and(|row| row.facts == facts && row.place.as_deref() == Some(place))
            {
                continue;
            }
            self.gone.remove(&id);
            self.sessions.insert(
                id,
                Listed {
                    work: facts.work.clone(),
                    facts,
                    seq: next,
                    place: Some(place.into()),
                },
            );
            changed = true;
        }
        if changed {
            self.seq = next;
            self.projects = projects;
            self.archived = archived;
            self.work_by_bot = work_by_bot(&self.sessions);
            let now = crate::disk::now();
            let mut disappeared = self.gone.iter().collect::<Vec<_>>();
            disappeared.sort_by_key(|(_, gone)| std::cmp::Reverse(gone.seq));
            let expired = disappeared
                .into_iter()
                .enumerate()
                .filter(|(index, (_, gone))| {
                    *index >= 2000 || now.saturating_sub(gone.at) >= 24 * 3_600_000
                })
                .map(|(_, (id, gone))| (id.clone(), gone.seq))
                .collect::<Vec<_>>();
            for (id, seq) in expired {
                self.floor = self.floor.max(seq);
                self.gone.remove(&id);
            }
        }
        changed
    }

    fn cursor(&self) -> String {
        format!("{}.{}", self.epoch, self.seq)
    }

    fn answer(&self, since: Option<&str>) -> Value {
        let since = since
            .and_then(|since| since.rsplit_once('.'))
            .filter(|(epoch, _)| *epoch == self.epoch)
            .and_then(|(_, seq)| seq.parse::<u64>().ok())
            .filter(|seq| (self.floor..=self.seq).contains(seq));
        let sessions = self
            .sessions
            .iter()
            .filter(|(_, row)| since.is_none_or(|since| row.seq > since))
            .map(|(id, row)| (id, &row.facts))
            .collect::<BTreeMap<_, _>>();
        let mut projects = self.projects.iter().collect::<Vec<_>>();
        projects.sort_by(|(left_id, left), (right_id, right)| {
            let updated = |row: &Value| row["updated"].as_u64().unwrap_or_default();
            updated(right)
                .cmp(&updated(left))
                .then_with(|| {
                    by_project_name(project_name(left_id, left), project_name(right_id, right))
                })
                .then_with(|| left_id.cmp(right_id))
        });
        let order = projects.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        let mut answer = json!({ "at": self.cursor(), "sessions": sessions, "projects": self.projects,
            "projectOrder": order, "startProject": order.first(), "archivedGroups": self.archived,
            "needsYou": self.needs_you.as_deref().unwrap_or_default() });
        if let Some(since) = since {
            let gone = self
                .gone
                .iter()
                .filter(|(_, row)| row.seq > since)
                .map(|(id, _)| id)
                .collect::<Vec<_>>();
            if !gone.is_empty() {
                answer["gone"] = json!(gone);
            }
        } else {
            answer["full"] = json!(true);
        }
        if !self.pending.is_empty() {
            answer["pending"] = json!(self.pending);
        }
        answer
    }
}

fn project_name<'a>(directory: &'a str, row: &'a Value) -> &'a str {
    row["name"].as_str().unwrap_or_else(|| {
        Path::new(directory)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(directory)
    })
}

fn name_parts(mut name: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        let first = name.chars().next()?;
        let end = if first.is_ascii_digit() {
            name.find(|character: char| !character.is_ascii_digit())
                .unwrap_or(name.len())
        } else {
            first.len_utf8()
        };
        let (part, rest) = name.split_at(end);
        name = rest;
        Some(part)
    })
}

fn by_project_name(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left = name_parts(left);
    let mut right = name_parts(right);
    loop {
        let (a, b) = (left.next(), right.next());
        let (Some(a), Some(b)) = (a, b) else {
            return a.is_some().cmp(&b.is_some());
        };
        let part = if a.as_bytes()[0].is_ascii_digit() && b.as_bytes()[0].is_ascii_digit() {
            let a = a.trim_start_matches('0');
            let b = b.trim_start_matches('0');
            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
        } else {
            a.to_lowercase().cmp(&b.to_lowercase())
        };
        if part != std::cmp::Ordering::Equal {
            return part;
        }
    }
}

fn work_by_bot(sessions: &BTreeMap<String, Listed>) -> BTreeMap<String, Vec<String>> {
    let mut work = BTreeMap::<String, Vec<String>>::new();
    for (id, row) in sessions {
        let facts = &row.facts;
        if facts.flag("archived")
            || facts.flag("away")
            || facts.extra.contains_key("parent")
            || !facts
                .extra
                .get("line")
                .and_then(Value::as_array)
                .is_some_and(|line| !line.is_empty())
        {
            continue;
        }
        if let Some(bot) = facts
            .extra
            .get("bot")
            .and_then(Value::as_str)
            .filter(|bot| {
                bot.strip_prefix("bot_").is_some_and(|name| {
                    !name.is_empty() && name.bytes().all(|letter| letter.is_ascii_alphanumeric())
                })
            })
        {
            work.entry(bot.into()).or_default().push(id.clone());
        }
    }
    work
}

async fn load(home: &Path) -> Option<Snapshot> {
    let mut saved: Snapshot = serde_json::from_slice(
        &tokio::fs::read(home.join("run/lists/sessions.json"))
            .await
            .ok()?,
    )
    .ok()?;
    for row in saved.sessions.values_mut() {
        row.facts.work.clone_from(&row.work);
    }
    for project in saved.projects.values_mut().filter_map(Value::as_object_mut) {
        for field in ["asking", "working"] {
            project.entry(field).or_insert_with(|| json!(0));
        }
    }
    saved.needs_you.get_or_insert_with(|| {
        let mut waiting: Vec<_> = saved
            .sessions
            .iter()
            .filter(|(_, row)| matches!(row.facts.kind, Kind::Project | Kind::Chat) && row.facts.waiting())
            .map(|(session, row)| (session, row.facts.extra.get("asked").and_then(Value::as_u64)))
            .collect();
        waiting.sort_by(|(a, one), (b, other)| other.cmp(one).then_with(|| a.cmp(b)));
        waiting
            .into_iter()
            .map(|(session, _)| json!({"session":session,"asks":{"kind":"other","summary":"waits on you"}}))
            .collect()
    });
    if saved
        .sessions
        .values()
        .any(|session| session.facts.flag("archived"))
    {
        let facts = saved
            .sessions
            .iter()
            .map(|(id, row)| (id.clone(), row.facts.clone()))
            .collect();
        saved.archived = archived_groups(&facts, &saved.projects, &read_projects(home).await);
    }
    Some(saved)
}

#[derive(Deserialize)]
struct ProjectFile {
    projects: Vec<Project>,
}

#[derive(Deserialize)]
struct Project {
    directory: String,
    name: Option<String>,
    #[serde(default)]
    folders: Vec<String>,
    #[serde(default)]
    removed: bool,
}

async fn read_projects(home: &Path) -> Vec<Project> {
    tokio::fs::read(home.join("box/projects.json"))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ProjectFile>(&bytes).ok())
        .map_or_else(Vec::new, |file| file.projects)
}

#[derive(PartialEq)]
pub(crate) struct Observed {
    pub(crate) session: Session,
    pub(crate) running: bool,
}

struct Inventory {
    engine_rows: Vec<Observed>,
    bridge_rows: Option<Arc<Vec<Observed>>>,
    agent_ids: BTreeSet<String>,
    engine_known: bool,
    bridge_known: Option<bool>,
}

impl Inventory {
    async fn read(engine: &Engine, bridge_rows: Option<Arc<Vec<Observed>>>) -> Option<Self> {
        let engine_rows = async {
            let lease = engine.acquire(Demand::Passive).await?;
            read_inventory(engine, &lease).await.ok()
        }
        .await;
        if engine_rows.is_none() && bridge_rows.is_none() {
            return None;
        }
        let agent_ids = bridge_rows
            .iter()
            .flat_map(|rows| rows.iter())
            .map(|row| row.session.id.clone())
            .collect();
        let engine_known = engine_rows.is_some();
        let bridge_known = engine
            .agent_sessions
            .as_ref()
            .map(|_| bridge_rows.is_some());
        Some(Self {
            engine_rows: engine_rows.unwrap_or_default(),
            bridge_rows,
            agent_ids,
            engine_known,
            bridge_known,
        })
    }

    /// Each conversation once: one the coding agents list (OpenCode's older ones among them) is
    /// theirs, never the engine's too.
    fn rows(&self) -> impl Iterator<Item = &Observed> {
        self.engine_rows
            .iter()
            .filter(|row| !self.agent_ids.contains(&row.session.id))
            .chain(self.bridge_rows.iter().flat_map(|rows| rows.iter()))
    }

    fn pending(&self) -> Vec<String> {
        let mut sources = Vec::new();
        if !self.engine_known {
            sources.push("engine".to_owned());
        }
        if self.bridge_known == Some(false) {
            sources.push("bridge".to_owned());
        }
        sources
    }

    fn retained(&self, previous: &Snapshot) -> Sessions {
        previous
            .sessions
            .iter()
            .filter(|(_, row)| match row.place.as_deref() {
                Some("bridge") => self.bridge_known == Some(false),
                _ => !self.engine_known,
            })
            .map(|(id, row)| (id.clone(), row.facts.clone()))
            .collect()
    }

    fn place(&self, id: &str, previous: Option<&Listed>) -> &'static str {
        if self.agent_ids.contains(id)
            || (self.bridge_known == Some(false)
                && previous.is_some_and(|row| row.place.as_deref() == Some("bridge")))
        {
            "bridge"
        } else {
            "engine"
        }
    }
}

/// Invalidate an in-flight page only for a different agent projection, independently of
/// whether the engine has finished its first inventory.
async fn refresh_agents(
    engine: &Engine,
    kept: &mut Option<Arc<Vec<Observed>>>,
    revision: &watch::Sender<u64>,
) -> bool {
    let current = agent_rows(engine).await;
    if kept.as_deref() == current.as_ref() {
        return false;
    }
    *kept = current.map(Arc::new);
    revision.send_modify(|revision| *revision += 1);
    true
}

/// The coding agents' conversations, or `None` when they can't be read.
pub(crate) async fn agent_rows(engine: &Engine) -> Option<Vec<Observed>> {
    let source = engine.agent_sessions.as_ref()?;
    let rows = source.inventory().await.ok()?;
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter_map(|row| {
            let session = serde_json::from_value(Value::Object(row.header)).ok()?;
            Some(Observed {
                session,
                running: row.running,
            })
        })
        .collect();
    rows.sort_unstable_by(|left, right| left.session.id.cmp(&right.session.id));
    Some(rows)
}

pub(crate) async fn read_inventory(
    engine: &Engine,
    lease: &Lease,
) -> Result<Vec<Observed>, Box<dyn std::error::Error + Send + Sync>> {
    let (rows, active) = tokio::join!(
        all_sessions(engine, lease),
        engine.call::<Value>(lease, Method::GET, "api/session/active", None)
    );
    let active = active?;
    if (!active.is_object() && !active.is_array()) || active.get("data").is_some() {
        return Err(io::Error::other("The engine did not supply a complete run list.").into());
    }
    Ok(rows?
        .into_iter()
        .map(|session| {
            let running = active
                .as_array()
                .is_some_and(|active| active.iter().any(|row| row == &session.id))
                || active
                    .get(&session.id)
                    .is_some_and(|value| !value.is_null());
            Observed { session, running }
        })
        .collect())
}

async fn project_changes(edits: &mut Option<Subscription>) -> Option<()> {
    let Some(edits) = edits else {
        return std::future::pending().await;
    };
    while let Some(notice) = edits.recv().await {
        match notice {
            Notice::Rescan | Notice::Failed(_) => return Some(()),
            Notice::Changed { paths }
                if paths.iter().any(|path| {
                    path == Path::new("projects.json")
                        || path == Path::new(crate::worktree_notes::FILE)
                }) =>
            {
                return Some(());
            }
            _ => {}
        }
    }
    None
}

async fn background_changes(edits: &mut Option<Subscription>) -> Option<()> {
    let Some(edits) = edits else {
        return std::future::pending().await;
    };
    while let Some(notice) = edits.recv().await {
        match notice {
            Notice::Rescan | Notice::Failed(_) => return Some(()),
            Notice::Changed { paths }
                if paths
                    .iter()
                    .any(|path| path == Path::new("background.json")) =>
            {
                return Some(());
            }
            _ => {}
        }
    }
    None
}

async fn agent_changes(edits: &mut Option<Subscription>) -> Option<()> {
    let Some(edits) = edits else {
        return std::future::pending().await;
    };
    while let Some(notice) = edits.recv().await {
        match notice {
            Notice::Rescan | Notice::Failed(_) => return Some(()),
            Notice::Changed { paths }
                if paths.iter().any(|path| {
                    path.file_name().is_some_and(|name| name == "session.json")
                        || path.components().count() == 1
                }) =>
            {
                return Some(());
            }
            _ => {}
        }
    }
    None
}

struct Filing<'a> {
    saved: &'a BTreeMap<String, String>,
    engine: &'a EngineFacts,
    other_folders: BTreeMap<&'a str, &'a str>,
}

impl<'a> Filing<'a> {
    fn new(
        projects: &'a [Project],
        saved: &'a BTreeMap<String, String>,
        engine: &'a EngineFacts,
    ) -> Self {
        let own = projects
            .iter()
            .filter(|project| !project.removed)
            .map(|project| project.directory.as_str())
            .collect::<BTreeSet<_>>();
        let mut other_folders = BTreeMap::new();
        for project in projects.iter().filter(|project| !project.removed) {
            for folder in &project.folders {
                if !own.contains(folder.as_str()) {
                    other_folders
                        .entry(folder.as_str())
                        .or_insert(project.directory.as_str());
                }
            }
        }
        Self {
            saved,
            engine,
            other_folders,
        }
    }

    fn known(&self, folder: &str) -> Option<&str> {
        let project = self
            .saved
            .get(folder)
            .map(String::as_str)
            .or_else(|| self.other_folders.get(folder).copied())
            .or_else(|| self.engine.worktree(folder).and_then(Path::to_str))?;
        Some(self.other_folders.get(project).copied().unwrap_or(project))
    }
}

fn measure<'a>(
    rows: impl Iterator<Item = &'a Observed>,
    mut sessions: Sessions,
    home: &Path,
    projects: &[Project],
    worktrees: &BTreeMap<String, String>,
    engine_facts: &EngineFacts,
    asks: &crate::asks::Book,
) -> (Sessions, Projects, Vec<Value>, Vec<Value>) {
    let filing = Filing::new(projects, worktrees, engine_facts);
    let background = kuvo_store::background::active_sessions(&home.join("run"), crate::disk::now());
    sessions.extend(rows.map(|row| {
        (
            row.session.id.clone(),
            facts(
                &row.session,
                home,
                &filing,
                row.running || background.contains(&row.session.id),
            ),
        )
    }));

    let mut busy_ancestors = BTreeSet::new();
    for facts in sessions
        .values()
        .filter(|facts| facts.kind == Kind::Subagent && facts.flag("running"))
    {
        let mut current = facts;
        for _ in 0..100 {
            let Some(parent) = current.extra.get("parent").and_then(Value::as_str) else {
                break;
            };
            let Some(next) = sessions.get(parent) else {
                break;
            };
            if !busy_ancestors.insert(parent.to_owned()) {
                break;
            }
            current = next;
        }
    }
    for id in busy_ancestors {
        if let Some(parent) = sessions.get_mut(&id) {
            parent.extra.insert("running".into(), json!(true));
        }
    }

    let children = sessions
        .iter()
        .filter(|(_, facts)| facts.kind == Kind::Subagent)
        .filter_map(|(id, facts)| {
            Some((
                facts.extra.get("parent")?.as_str()?.to_owned(),
                id.clone(),
                facts.flag("running"),
                facts.created,
            ))
        })
        .collect::<Vec<_>>();
    let mut by_parent: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for (parent, id, running, updated) in children {
        by_parent
            .entry(parent)
            .or_default()
            .push((id, running, updated));
    }
    for (parent, mut children) in by_parent {
        if let Some(facts) = sessions.get_mut(&parent) {
            children.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| b.2.cmp(&a.2))
                    .then_with(|| a.0.cmp(&b.0))
            });
            facts.extra.insert(
                "subagents".into(),
                json!(
                    children
                        .into_iter()
                        .map(|(id, _, _)| id)
                        .collect::<Vec<_>>()
                ),
            );
        }
    }

    let mut asked = BTreeMap::<String, &crate::asks::Pending>::new();
    let direct = asks
        .values()
        .map(|ask| ask.session.as_str())
        .collect::<BTreeSet<_>>();
    for ask in asks.values() {
        let mut root = ask.session.as_str();
        for _ in 0..10 {
            let Some(parent) = sessions
                .get(root)
                .and_then(|row| row.extra.get("parent"))
                .and_then(Value::as_str)
            else {
                break;
            };
            root = parent;
        }
        let entry = asked.entry(root.to_owned()).or_insert(ask);
        if ask.at < entry.at {
            *entry = ask;
        }
    }
    let mut needs = Vec::new();
    for (session, facts) in &mut sessions {
        match asked.get(session) {
            Some(ask) => {
                facts.extra.insert("asked".into(), json!(ask.at));
                if !direct.contains(session.as_str()) {
                    facts.extra.insert("childAsk".into(), json!(true));
                }
                if facts.waiting() && matches!(facts.kind, Kind::Project | Kind::Chat) {
                    needs.push((session, ask));
                }
            }
            None if direct.contains(session.as_str()) => {
                facts.extra.insert("ownAsk".into(), json!(true));
            }
            None => {}
        }
    }
    needs.sort_by(|(a, one), (b, other)| other.at.cmp(&one.at).then(a.cmp(b)));
    let needs_you = needs
        .into_iter()
        .map(|(session, ask)| json!({"session":session,"asks":ask.asks}))
        .collect();

    let mut listed = Projects::new();
    let directories = projects
        .iter()
        .filter(|project| !project.removed && filing.known(&project.directory).is_none())
        .map(|project| project.directory.as_str())
        .chain(
            sessions
                .values()
                .filter(|facts| !facts.flag("archived"))
                .filter_map(Facts::project),
        )
        .collect::<BTreeSet<_>>();
    for directory in directories {
        let rows = project_rows(&sessions, directory);
        let (shown, more) = project_page(&rows, 6, 0);
        let mut project = json!({ "asking": rows.iter().filter(|(_, facts)| facts.waiting()).count(), "working": rows.iter().filter(|(_, facts)| !facts.flag("away") && !facts.waiting() && facts.flag("running")).count(),
            "shown": shown, "total": rows.len() });
        if let Some(updated) = rows.iter().map(|(_, facts)| facts.updated).max() {
            project["updated"] = json!(updated);
        }
        if let Some(name) = projects
            .iter()
            .find(|project| project.directory == directory)
            .and_then(|project| project.name.as_ref())
        {
            project["name"] = json!(name);
        }
        if let Some(more) = more {
            project["more"] = more;
        }
        listed.insert(directory.to_owned(), project);
    }
    let archived = archived_groups(&sessions, &listed, projects);
    (sessions, listed, archived, needs_you)
}

async fn all_sessions(
    engine: &Engine,
    lease: &Lease,
) -> Result<Vec<Session>, Box<dyn std::error::Error + Send + Sync>> {
    api::Scan::Sessions
        .read(engine, lease, "api/session")
        .await?
        .into_iter()
        .map(|row| serde_json::from_value(row).map_err(Into::into))
        .collect()
}

fn facts(session: &Session, home: &Path, filing: &Filing<'_>, running: bool) -> Facts {
    let directory = session.directory();
    let notes = session.notes();
    let note = |key| {
        notes
            .and_then(|notes| notes.get(key))
            .unwrap_or(&Value::Null)
    };
    let kind = if session.parent.as_ref().is_some_and(|id| !id.is_empty()) {
        Kind::Subagent
    } else if !note("seat").is_null() {
        Kind::Seat
    } else if !note("engineFor").is_null() || !note("reviewPart").is_null() {
        Kind::Hidden
    } else if Path::new(directory).starts_with(home.join("bots")) {
        Kind::Bot
    } else if note("chat").as_str() == Some(directory)
        || Path::new(directory).starts_with(home.join("chats"))
    {
        Kind::Chat
    } else {
        Kind::Project
    };

    let mut facts = Facts {
        kind,
        directory: directory.to_owned(),
        created: session.time.created.unwrap_or_default(),
        updated: filing
            .engine
            .activity(&session.id)
            .or(session.time.activity)
            .or(session.time.updated)
            .unwrap_or_default(),
        extra: BTreeMap::new(),
        work: work::listed_work(session),
    };
    let mut set = |key: &str, value: Value| {
        facts.extra.insert(key.to_owned(), value);
    };
    if kind == Kind::Project {
        let mark = note("filed");
        let project = filing
            .known(directory)
            .or_else(|| {
                let named = mark["project"]
                    .as_str()
                    .filter(|named| !named.is_empty() && *named != directory)?;
                (mark["folder"] == directory).then(|| filing.known(named).unwrap_or(named))
            })
            .unwrap_or(directory);
        set("project", json!(project));
    }
    if !session.title.is_empty() {
        set("title", json!(session.title));
    }
    if kind == Kind::Subagent {
        set("parent", json!(session.parent));
        let messages = note("messages");
        set(
            "messages",
            if messages.is_object() {
                messages.clone()
            } else {
                json!({ "via": "direct" })
            },
        );
    }
    if let Some(line) = note("line").as_array().filter(|line| !line.is_empty()) {
        let mut line = line
            .iter()
            .filter_map(Value::as_str)
            .rev()
            .take(20)
            .collect::<Vec<_>>();
        line.reverse();
        set("line", json!(line));
    } else if let Some(starter) = note("startedBy").as_str().filter(|id| !id.is_empty()) {
        set("line", json!([starter]));
    }
    let bot = note("bot")
        .as_str()
        .filter(|id| !id.is_empty())
        .or_else(|| {
            if kind == Kind::Subagent {
                return None;
            }
            note("by")["bot"].as_str().filter(|id| !id.is_empty())
        });
    if let Some(bot) = bot {
        set("bot", json!(bot));
    }
    if running {
        set("running", json!(true));
    }
    if session.pinned() {
        set("pinned", json!(true));
    }
    if session.archived() {
        set("archived", json!(true));
    }
    if let Some(idle) = session
        .time
        .idle
        .filter(|idle| *idle > session.time.viewed.unwrap_or_default())
    {
        set("unread", json!(idle));
    }
    facts
}

fn project_rows<'a>(sessions: &'a Sessions, project: &str) -> Vec<(&'a str, &'a Facts)> {
    let mut rows = sessions
        .iter()
        .filter(|(_, facts)| facts.project() == Some(project) && !facts.flag("archived"))
        .map(|(id, facts)| (id.as_str(), facts))
        .collect::<Vec<_>>();
    let rank = |facts: &Facts| {
        if facts.flag("pinned") {
            0
        } else if !facts.flag("away")
            && (facts.flag("running") || facts.extra.contains_key("asked"))
        {
            1
        } else {
            2
        }
    };
    rows.sort_by(|(a, one), (b, other)| {
        rank(one)
            .cmp(&rank(other))
            .then_with(|| other.updated.cmp(&one.updated))
            .then_with(|| a.cmp(b))
    });
    rows
}

fn project_page<'a>(
    rows: &[(&'a str, &Facts)],
    shown: usize,
    step: u32,
) -> (Vec<&'a str>, Option<Value>) {
    let count = rows
        .len()
        .saturating_sub(shown)
        .min(10 << step)
        .min(5000 - shown);
    let more = (count > 0)
        .then(|| json!({"cursor": format!("s{}.{}", shown + count, step + 1), "count": count}));
    (rows.iter().take(shown).map(|(id, _)| *id).collect(), more)
}

fn archived_groups(sessions: &Sessions, projects: &Projects, saved: &[Project]) -> Vec<Value> {
    let mut rows = sessions
        .iter()
        .filter(|(_, facts)| {
            facts.flag("archived")
                && !facts.flag("away")
                && matches!(facts.kind, Kind::Project | Kind::Chat | Kind::Bot)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|(a, one), (b, other)| other.updated.cmp(&one.updated).then_with(|| a.cmp(b)));
    let mut groups: Vec<(&str, Kind, Vec<&str>)> = Vec::new();
    for (id, facts) in rows {
        let directory = facts.project().unwrap_or(&facts.directory);
        match groups
            .iter_mut()
            .find(|(folder, kind, _)| *folder == directory && *kind == facts.kind)
        {
            Some((_, _, ids)) => ids.push(id),
            None => groups.push((directory, facts.kind, vec![id])),
        }
    }
    groups
        .into_iter()
        .map(|(directory, kind, sessions)| {
            let title = if kind == Kind::Chat {
                "Chats"
            } else {
                projects
                    .get(directory)
                    .and_then(|project| project["name"].as_str())
                    .or_else(|| {
                        saved
                            .iter()
                            .find(|project| project.directory == directory)
                            .and_then(|project| project.name.as_deref())
                    })
                    .or_else(|| {
                        Path::new(directory)
                            .file_name()
                            .and_then(|name| name.to_str())
                    })
                    .unwrap_or(directory)
            };
            json!({ "directory": directory, "kind": kind, "sessions": sessions, "title": title })
        })
        .collect()
}

pub(crate) fn register(
    list: LocalSessions,
    routes: &mut Registry,
    merged_page: impl Fn(&str, usize, u32) -> Option<page::Status200> + Send + Sync + 'static,
) {
    let local = list.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let mut local = local.clone();
        async move {
            let since = crate::query(&call.request.path, "since");
            let snapshot = local.current(&call.cancel).await?;
            Ok(RouteReply::json::<200>(typed(
                snapshot.answer(since.as_deref()),
            )?))
        }
    });
    let local = list.clone();
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let mut local = local.clone();
        async move {
            let mut since = crate::query(&call.request.path, "since");
            let mut snapshot = local.current(&call.cancel).await?;
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                loop {
                    let answer: events::Status200 = serde_json::from_value(snapshot.answer(since.as_deref())).map_err(io::Error::other)?;
                    yield JsonLine::new(answer)?;
                    since = Some(snapshot.cursor());
                    tokio::select! {
                        changed = local.snapshot.changed() => if changed.is_err() { break; },
                        () = call.cancel.cancelled() => break,
                    }
                    if let Some(current) = local.snapshot.borrow_and_update().clone() { snapshot = current; }
                }
            }))
        }
    });
    let merged_page = Arc::new(merged_page);
    routes.register::<page::Spec, _, _>(Access::Any, move |call| {
        let mut local = list.clone();
        let merged_page = Arc::clone(&merged_page);
        async move {
            let project = crate::query(&call.request.path, "project")
                .filter(|project| !project.is_empty())
                .ok_or_else(|| {
                    Refusal::new(StatusCode::BAD_REQUEST, "Name the project: project=<its key>.")
                })?;
            let (shown, step) = crate::query(&call.request.path, "cursor")
                .filter(|value| !value.is_empty())
                .map_or(Some((6, 0)), |cursor| parse_cursor(&cursor))
                .ok_or_else(|| {
                    Refusal::new(StatusCode::BAD_REQUEST, "That cursor isn't one this box gave. Read the list again.")
                })?;
            let peer_project = project.split_once(':').filter(|(key, _)| {
                key.len() == 43
                    && key.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            });
            let name = peer_project.map_or(project.as_str(), |(_, name)| name);
            if name
                .split(['/', '\\'])
                .any(|part| part.chars().filter(char::is_ascii).eq("..".chars()))
            {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Name the project: project=<its key>.",
                ));
            }
            // Page keys only select facts from the local or merged inventory.
            if peer_project.is_some() {
                return merged_page(&project, shown, step)
                    .map(RouteReply::json::<200>)
                    .ok_or_else(|| {
                        Refusal::new(StatusCode::SERVICE_UNAVAILABLE, "The session list isn’t ready.")
                    });
            }
            let snapshot = local.current(&call.cancel).await?;
            let facts = snapshot.sessions.iter().map(|(id, row)| (id.clone(), row.facts.clone())).collect();
            let rows = project_rows(&facts, &project);
            let (sessions, more) = project_page(&rows, shown, step);
            Ok(RouteReply::json::<200>(typed(json!({ "project": project, "sessions": sessions, "total": rows.len(), "more": more }))?))
        }
    });
}

fn parse_cursor(cursor: &str) -> Option<(usize, u32)> {
    let (shown, step) = cursor.strip_prefix('s')?.split_once('.')?;
    if !(1..=5).contains(&shown.len())
        || !(1..=2).contains(&step.len())
        || !shown
            .bytes()
            .chain(step.bytes())
            .all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let (shown, step) = (shown.parse().ok()?, step.parse().ok()?);
    ((6..=5000).contains(&shown) && step <= 20).then_some((shown, step))
}
