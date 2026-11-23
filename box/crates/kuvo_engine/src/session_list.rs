//! A local session list that can be followed by the line and read by the peers crate.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    sync::Arc,
    time::Duration,
};

use http::{Method, StatusCode};
use kuvo_protocol::routes::{
    get_kuvo_sessions_list as read, get_kuvo_sessions_list_events as events,
    get_kuvo_sessions_page as page,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, watch},
    time,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Engine, api,
    lifetime::{Demand, Lease},
    sessions::{Session, typed},
};

#[derive(Clone)]
pub struct LocalSessions {
    snapshot: watch::Receiver<Option<Arc<Snapshot>>>,
    refresh: mpsc::Sender<()>,
}

impl LocalSessions {
    /// The most recent local list. A consumer can subscribe to full snapshots
    /// without starting the engine or joining the per-client cursor history.
    pub fn latest(&self) -> Option<read::Status200> {
        self.snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| typed(snapshot.answer(None)).ok())
    }

    pub async fn changed(&mut self) -> bool {
        self.snapshot.changed().await.is_ok()
    }

    pub(crate) fn start(engine: Engine) -> Self {
        let (refresh, mut requests) = mpsc::channel(1);
        let (updates, snapshot) = watch::channel(None);
        crate::conversation::watch(engine.clone(), refresh.downgrade());
        tokio::spawn(async move {
            let mut last = load(&engine.home).await.unwrap_or_else(Snapshot::new);
            if !last.sessions.is_empty() {
                last.seq += 1_000_000;
                last.floor = last.seq;
                last.pending = vec!["engine".to_owned()];
                updates.send_replace(Some(Arc::new(last.clone())));
            }
            let mut state = engine.states();
            let mut check = time::interval(Duration::from_secs(60));
            check.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    next = requests.recv() => if next.is_none() { break; },
                    changed = state.changed() => if changed.is_err() { break; },
                    _ = check.tick() => {},
                }
                let Some(lease) = engine.acquire(Demand::Passive).await else {
                    continue;
                };
                let Some((sessions, projects, archived)) = measure(&engine, &lease).await.ok()
                else {
                    continue;
                };
                if last.commit(sessions, projects, archived) || updates.borrow().is_none() {
                    updates.send_replace(Some(Arc::new(last.clone())));
                    let path = engine.home.join("run/lists/sessions.json");
                    let saved = last.clone();
                    let _ = tokio::task::spawn_blocking(move || crate::disk::write(&path, &saved))
                        .await;
                }
            }
        });
        Self { snapshot, refresh }
    }

    pub(crate) fn refresh(&self) {
        let _ = self.refresh.try_send(());
    }

    async fn current(&mut self, cancel: &CancellationToken) -> Result<Arc<Snapshot>, Refusal> {
        self.refresh();
        let current = self.snapshot.borrow_and_update().clone();
        if let Some(current) = current {
            return Ok(current);
        }
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

type Sessions = BTreeMap<String, Facts>;
type Projects = BTreeMap<String, Value>;

#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
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
    needs_you: Vec<Value>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Listed {
    facts: Facts,
    seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    place: Option<String>,
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
}

impl Facts {
    fn flag(&self, field: &str) -> bool {
        self.extra.get(field) == Some(&Value::Bool(true))
    }

    fn project(&self) -> Option<&str> {
        self.extra.get("project").and_then(Value::as_str)
    }
}

impl Snapshot {
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
            needs_you: Vec::new(),
        }
    }

    fn commit(&mut self, fresh: Sessions, projects: Projects, archived: Vec<Value>) -> bool {
        let mut changed =
            !self.pending.is_empty() || self.projects != projects || self.archived != archived;
        self.pending.clear();
        let removed = self
            .sessions
            .keys()
            .filter(|id| !fresh.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in removed {
            self.seq += 1;
            self.sessions.remove(&id);
            self.gone.insert(
                id,
                Gone {
                    seq: self.seq,
                    at: crate::disk::now(),
                },
            );
            changed = true;
        }
        for (id, facts) in fresh {
            if self.sessions.get(&id).is_some_and(|row| row.facts == facts) {
                continue;
            }
            self.seq += 1;
            self.gone.remove(&id);
            self.sessions.insert(
                id,
                Listed {
                    facts,
                    seq: self.seq,
                    place: Some("engine".into()),
                },
            );
            changed = true;
        }
        if changed {
            self.seq += 1;
            self.projects = projects;
            self.archived = archived;
            while self.gone.len() > 2000 {
                let Some((oldest, gone)) = self.gone.iter().min_by_key(|(_, gone)| gone.seq) else {
                    break;
                };
                self.floor = self.floor.max(gone.seq);
                let oldest = oldest.clone();
                self.gone.remove(&oldest);
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
            right["updated"]
                .as_u64()
                .cmp(&left["updated"].as_u64())
                .then_with(|| left_id.cmp(right_id))
        });
        let order = projects.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        let mut answer = json!({ "at": self.cursor(), "sessions": sessions, "projects": self.projects,
            "projectOrder": order, "startProject": order.first(), "archivedGroups": self.archived,
            "needsYou": self.needs_you });
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

async fn load(home: &Path) -> Option<Snapshot> {
    let mut saved: Snapshot = serde_json::from_slice(
        &tokio::fs::read(home.join("run/lists/sessions.json"))
            .await
            .ok()?,
    )
    .ok()?;
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
        saved.archived = archived_groups(&facts, &saved.projects);
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

#[derive(Deserialize)]
struct EnginePage {
    data: Vec<Value>,
    cursor: Option<Cursor>,
}

#[derive(Deserialize)]
struct Cursor {
    next: Option<String>,
}

async fn measure(
    engine: &Engine,
    lease: &Lease,
) -> Result<(Sessions, Projects, Vec<Value>), Box<dyn std::error::Error + Send + Sync>> {
    let (rows, active, projects) = tokio::join!(
        all_sessions(engine, lease),
        engine.call::<Value>(lease, Method::GET, "api/session/active", None),
        tokio::fs::read(engine.home.join("box/projects.json"))
    );
    let rows = rows?;
    let active = active?;
    let projects = projects
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ProjectFile>(&bytes).ok())
        .map_or_else(Vec::new, |file| file.projects);
    let mut sessions = Sessions::new();
    for session in rows {
        let running = active
            .as_array()
            .is_some_and(|active| active.iter().any(|row| row == &session.id))
            || active
                .get(&session.id)
                .is_some_and(|value| !value.is_null());
        let facts = facts(&session, &engine.home, &projects, running);
        sessions.insert(session.id, facts);
    }

    let children = sessions
        .iter()
        .filter(|(_, facts)| facts.kind == Kind::Subagent)
        .filter_map(|(id, facts)| {
            Some((
                facts.extra.get("parent")?.as_str()?.to_owned(),
                id.clone(),
                facts.flag("running"),
                facts.updated,
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

    let mut listed = Projects::new();
    let directories = projects
        .iter()
        .filter(|project| !project.removed)
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
        let mut project = json!({ "asking": 0, "working": rows.iter().filter(|(_, facts)| facts.flag("running")).count(),
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
    let archived = archived_groups(&sessions, &listed);
    Ok((sessions, listed, archived))
}

async fn all_sessions(
    engine: &Engine,
    lease: &Lease,
) -> Result<Vec<Session>, Box<dyn std::error::Error + Send + Sync>> {
    let mut path = "api/session?limit=100&order=desc".to_owned();
    let mut seen = BTreeSet::new();
    let mut sessions = Vec::new();
    for _ in 0..1000 {
        let response = engine
            .upstream
            .request(&lease.record, Method::GET, &path)
            .timeout(api::WAIT)
            .send()
            .await?
            .error_for_status()?
            .json::<EnginePage>()
            .await?;
        let count = response.data.len();
        for value in response.data {
            sessions.push(serde_json::from_value(value)?);
        }
        let Some(next) = response
            .cursor
            .and_then(|cursor| cursor.next)
            .filter(|_| count >= 100)
        else {
            return Ok(sessions);
        };
        if !seen.insert(next.clone()) {
            return Err(io::Error::other("The session page cursor repeated.").into());
        }
        path = format!("api/session?limit=100&cursor={}", api::segment(&next));
    }
    Err(io::Error::other("Could not read the full session list.").into())
}

fn facts(session: &Session, home: &Path, projects: &[Project], running: bool) -> Facts {
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
        updated: session
            .time
            .activity
            .or(session.time.updated)
            .unwrap_or_default(),
        extra: BTreeMap::new(),
    };
    let mut set = |key: &str, value: Value| {
        facts.extra.insert(key.to_owned(), value);
    };
    if kind == Kind::Project {
        let project = projects
            .iter()
            .filter(|project| !project.removed)
            .filter(|project| {
                Path::new(directory).starts_with(&project.directory)
                    || project
                        .folders
                        .iter()
                        .any(|folder| Path::new(directory).starts_with(folder))
            })
            .max_by_key(|project| project.directory.len())
            .map(|project| project.directory.as_str())
            .or_else(|| {
                (note("filed")["folder"] == directory)
                    .then(|| note("filed")["project"].as_str())
                    .flatten()
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
    let rank = |facts: &Facts| (!facts.flag("pinned"), !facts.flag("running"));
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

fn archived_groups(sessions: &Sessions, projects: &Projects) -> Vec<Value> {
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

pub(crate) fn register(list: LocalSessions, routes: &mut Registry) {
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
    routes.register::<page::Spec, _, _>(Access::Any, move |call| {
        let mut local = list.clone();
        async move {
            let project = crate::query(&call.request.path, "project").filter(|project| !project.is_empty())
                .ok_or_else(|| Refusal::new(StatusCode::BAD_REQUEST, "Say which project (project, its folder)."))?;
            let cursor = crate::query(&call.request.path, "cursor");
            let (shown, step) = cursor.as_deref().and_then(parse_cursor).unwrap_or((6, 0));
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
    let (shown, step) = (shown.parse().ok()?, step.parse().ok()?);
    ((6..=5000).contains(&shown) && step <= 20).then_some((shown, step))
}
