//! Session lists (PROTOCOL.md, "Session lists", `sessions.list`): what each session is and the
//! project it's filed under, which the box works out, so a client draws its sidebar from facts and
//! works nothing out. What's here is only taking the box's lines in and the drawing's order, held
//! to `protocol/vectors/session-lists.json` as KuvoClient's `SessionLists` is.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// The list now (`since`: only what changed after it, where the box can still say).
pub fn route(since: Option<&str>) -> String {
    format!("kuvo/sessions/list{}", query(since))
}

/// The list now, then a line for each change.
pub fn events_route(since: Option<&str>) -> String {
    format!("kuvo/sessions/list/events{}", query(since))
}

fn query(since: Option<&str>) -> String {
    match since.filter(|since| !since.is_empty()) {
        Some(since) => format!("?since={}", crate::paths::query_value(since)),
        None => String::new(),
    }
}

/// What a session is. One a newer box names that this doesn't know shows nowhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Kind(pub String);

impl Kind {
    /// A project's session.
    pub fn is_project(&self) -> bool {
        self.0 == "project"
    }
    /// A conversation of no project.
    pub fn is_chat(&self) -> bool {
        self.0 == "chat"
    }
}

/// One session, as its box says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    pub kind: Kind,
    /// The project it's filed under (a project's session's), the folder as its box names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub created: f64,
    /// When it last changed (ms): the lists' order.
    pub updated: f64,
    /// The session it works for (a subagent's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Its subagents' sessions, in the order its box lists them (those at work or waiting on you
    /// first, then the newest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<String>>,
    /// How a subagent takes your messages, as its box says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Messages>,
    /// The box's work lineage, from the lead to this session's immediate starter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<Vec<String>>,
    #[serde(default, rename = "reviewPart", skip_serializing_if = "Option::is_none")]
    pub review_part: Option<bool>,
    #[serde(default, rename = "childOnly", skip_serializing_if = "Option::is_none")]
    pub child_only: Option<bool>,
    #[serde(default, rename = "ownAsk", skip_serializing_if = "Option::is_none")]
    pub own_ask: Option<bool>,
    #[serde(default, rename = "childAsk", skip_serializing_if = "Option::is_none")]
    pub child_ask: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    /// Since when it's waited on you (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<f64>,
    /// This copy isn't its box's own (it moved, or is on its way there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away: Option<bool>,
    /// The bot it is, or works for, by its id (as `kuvo/bots` lists it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    /// Its model is reasoning now, as the box debounces it (`bots.state`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    /// It stopped on an error or a limit you haven't seen, or retries one for long: which (`error`,
    /// `limit`; a word a newer box may add to).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    /// It's retrying what stopped it (a limit, an error), as its engine says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrying: Option<bool>,
    /// It ended a turn you haven't read: when (ms, its `time.idle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<f64>,    /// Another of your computers' session, as the setup's list says (none: this computer's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<OnComputer>,
}

/// Where a session or project from the setup's list runs, when it isn't this computer, and how
/// that computer stands, as the nearest box says (PROTOCOL.md, "Setup-wide list topics").
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OnComputer {
    /// Its public key, to address its routes.
    pub id: String,
    pub name: String,
    /// Not answering now: its rows are its last copy.
    pub away: bool,
    /// When its last copy came (ms), while it's away.
    #[serde(default, rename = "lastSeen")]
    pub last_seen: Option<f64>,
    /// Away only to switch to a new release.
    #[serde(default)]
    pub updating: bool,
    /// What can be done with the row now (`open`, `manage`): none while it's away.
    pub actions: Vec<String>,
}

impl OnComputer {
    /// Whether the row can be opened now.
    pub fn opens(&self) -> bool {
        self.actions.iter().any(|action| action == "open")
    }

    /// Whether its menu's actions can be used now.
    pub fn manages(&self) -> bool {
        self.actions.iter().any(|action| action == "manage")
    }
}

/// How a subagent takes your messages: `direct`, through its `parent` (which passes them on), or
/// `none`, with `note` saying why in words to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Messages {
    pub via: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Messages {
    /// Whether a message can't reach it at all.
    pub fn refused(&self) -> bool {
        self.via == "none"
    }
}

impl Facts {
    /// Whether it's at work or waiting on you.
    pub fn is_ongoing(&self) -> bool {
        self.is_running() || self.asked.is_some()
    }
    pub fn is_pinned(&self) -> bool {
        self.pinned == Some(true)
    }
    pub fn is_archived(&self) -> bool {
        self.archived == Some(true)
    }
    pub fn is_running(&self) -> bool {
        self.running == Some(true)
    }
}

/// A project the lists show: its name where its list gives one, when its newest session changed
/// (none for one with none), and its sums as the box counts them: how many of its sessions wait on
/// you, and how many are at work (never one that waits).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Project {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<f64>,
    #[serde(default, deserialize_with = "count")]
    pub asking: usize,
    #[serde(default, deserialize_with = "count")]
    pub working: usize,
    /// Its list's first sessions, in the order its box shows them; none from a box before it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<Vec<String>>,
    /// How many sessions its list has in all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// What "Show more" adds next; none once all show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<ShowMore>,    /// Another of your computers' project, as the setup's list says (none: this computer's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<OnComputer>,
}

/// What "Show more" under a project's list adds next: `count` more, asked for with `cursor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowMore {
    pub cursor: String,
    pub count: u64,
}

/// A project's list as far as a cursor shows it (`kuvo/sessions/page`): its sessions in the box's
/// order, how many it has in all, and what "Show more" adds next.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub project: String,
    pub sessions: Vec<String>,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub more: Option<ShowMore>,
}

/// Whether a box pages its projects' lists.
pub const LIST_PAGES: &str = "sessions.list.pages";

/// Project `project`'s list as far as `cursor` (its `more`'s) shows it.
pub fn page_route(project: &str, cursor: &str) -> String {
    format!("kuvo/sessions/page?project={}&cursor={}", crate::paths::query_value(project), crate::paths::query_value(cursor))
}

/// A count as the box says it; one a newer box says in a way this can't read is none, not the
/// whole list lost.
fn count<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<usize, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_u64().map(|count| count as usize).unwrap_or(0))
}

/// What a session waiting on you asks, in a few words: its kind (`permission`, `question`,
/// `sandbox`, `vault`, `other`; one a newer box names is shown the same way), and the box's words
/// for it, written to follow a name ("asks to run a command").
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Asks {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub summary: String,
}

/// A session waiting on you, as "Needs you" lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NeedsYou {
    pub session: String,
    #[serde(default)]
    pub asks: Asks,
}

/// Archived rows in the box's grouping and order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArchivedGroup {
    pub directory: String,
    pub title: String,
    pub kind: String,
    pub sessions: Vec<String>,
}

/// A line from the box: whole (`full`), or what changed since the line before.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Line {
    pub at: String,
    #[serde(rename = "archivedGroups")]
    pub archived_groups: Vec<ArchivedGroup>,
    #[serde(default)]
    pub full: Option<bool>,
    #[serde(default)]
    pub sessions: HashMap<String, Facts>,
    #[serde(default)]
    pub gone: Option<Vec<String>>,
    #[serde(default)]
    pub projects: HashMap<String, Project>,
    #[serde(default, rename = "projectOrder")]
    pub project_order: Option<Vec<String>>,
    #[serde(default, rename = "startProject", deserialize_with = "start_project")]
    pub start_project: Option<Option<String>>,
    /// The sessions waiting on you that the lists show, the one that asked last first: whole in
    /// every line.
    #[serde(default, rename = "needsYou", deserialize_with = "crate::accounts::readable")]
    pub needs_you: Vec<NeedsYou>,
    /// What the box couldn't read yet (`bridge`): its sessions are as last read.
    #[serde(default)]
    pub pending: Option<Vec<String>>,
}

pub const LIST_START: &str = "sessions.list.start";

/// Whether a box serves the setup-wide lists on its line.
pub const SETUP_LISTS: &str = "line.setup-lists";

/// The setup's sessions and projects, across your computers.
pub const MERGED_SESSIONS: &str = "line.merged-sessions";
pub const MERGED_SESSIONS_ROUTE: &str = "kuvo/setup/sessions/list/events";

fn start_project<'de, D: serde::Deserializer<'de>>(decoder: D) -> std::result::Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(decoder).map(Some)
}

impl Line {
    pub(crate) fn check_start(self, required: bool) -> Result<Self, crate::Error> {
        if required && (self.start_project.is_none() || self.project_order.is_none()) {
            return Err(crate::Error::BadAnswer("The session list didn’t say its project order or where a new one starts.".into()));
        }
        Ok(self)
    }
}

/// A box's list as a client keeps it, line after line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Listing {
    at: Option<String>,
    sessions: HashMap<String, Facts>,
    projects: HashMap<String, Project>,
    project_order: Option<Vec<String>>,
    start_project: Option<Option<String>>,
    needs_you: Vec<NeedsYou>,
    archived_groups: Vec<ArchivedGroup>,
    /// What it said of sessions it has let go since (deleted there, or moved away), for a row a
    /// client still holds as it goes.
    former: HashMap<String, Facts>,
    pending: Vec<String>,
}

/// The computer and native folder from a remote project key in the merged list.
pub fn project_elsewhere(key: &str) -> Option<(&str, &str)> {
    let (computer, directory) = key.split_once(':')?;
    (directory.starts_with('/') && !computer.is_empty() && !computer.contains('/')).then_some((computer, directory))
}

impl Listing {
    pub fn new() -> Listing {
        Listing::default()
    }

    /// Where it's at: given back as `since` to resume after a dropped stream.
    pub fn at(&self) -> Option<&str> {
        self.at.as_deref()
    }

    /// Whether its box has said it yet.
    pub fn is_read(&self) -> bool {
        self.at.is_some()
    }

    pub fn sessions(&self) -> &HashMap<String, Facts> {
        &self.sessions
    }

    pub fn projects(&self) -> &HashMap<String, Project> {
        &self.projects
    }

    /// Where a new conversation starts: a project or Chats. None before the list is read or on an older box.
    pub fn start_project(&self) -> Option<Option<&str>> {
        self.start_project.as_ref().map(Option::as_deref)
    }

    /// "Needs you": the sessions waiting on you, in the box's order, each with what it asks.
    pub fn archived_groups(&self) -> &[ArchivedGroup] {
        &self.archived_groups
    }

    pub fn needs_you(&self) -> &[NeedsYou] {
        &self.needs_you
    }

    /// What the box couldn't read yet (`["bridge"]`): its sessions there are as last read.
    pub fn pending(&self) -> &[String] {
        &self.pending
    }

    /// What its box says of session `id`: as it's listed, else as it was before it was let go.
    pub fn facts(&self, id: &str) -> Option<&Facts> {
        self.sessions.get(id).or_else(|| self.former.get(id))
    }

    /// Project `key` as the box lists it.
    pub fn project(&self, key: &str) -> Option<&Project> {
        self.projects.get(key)
    }

    /// The computer session `id` is on, where it's another of yours.
    pub fn computer_of(&self, id: &str) -> Option<&OnComputer> {
        self.facts(id)?.on.as_ref()
    }

    /// `line` taken in: a whole one in place of what was, else what changed. Whether anything did.
    pub fn take(&mut self, line: Line) -> bool {
        let was_read = self.at.is_some();
        let before = (self.sessions.clone(), self.projects.clone(), std::mem::take(&mut self.pending), std::mem::take(&mut self.needs_you));
        let archived_changed = self.archived_groups != line.archived_groups;
        let start_changed = self.start_project != line.start_project;
        let order_changed = self.project_order != line.project_order;
        self.project_order = line.project_order;
        self.start_project = line.start_project;
        self.archived_groups = line.archived_groups;
        self.at = Some(line.at);
        if line.full == Some(true) {
            for (id, facts) in self.sessions.drain() {
                if !line.sessions.contains_key(&id) {
                    self.former.insert(id, facts);
                }
            }
            self.sessions = line.sessions.clone();
        } else {
            for (id, facts) in &line.sessions {
                self.sessions.insert(id.clone(), facts.clone());
            }
            for id in line.gone.iter().flatten() {
                if let Some(facts) = self.sessions.remove(id) {
                    self.former.insert(id.clone(), facts);
                }
            }
        }
        for id in line.sessions.keys() {
            self.former.remove(id);
        }
        if self.former.len() > 500 {
            self.former.clear();
        }
        self.projects = line.projects;
        self.needs_you = line.needs_you;
        self.pending = line.pending.unwrap_or_default();
        archived_changed || start_changed || order_changed || !was_read || before.0 != self.sessions || before.1 != self.projects || before.2 != self.pending || before.3 != self.needs_you
    }

    /// The sidebar a client draws from it ([`sidebar`]).
    pub fn sidebar(&self) -> Sidebar {
        sidebar_with_order(&self.sessions, &self.projects, self.project_order.as_deref())
    }
}

/// Where a session shows in the lists: under its project, or in Chats.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    Project(String),
    Chat,
}

/// Where `facts`' session shows: a project's session under its project, a chat in Chats, unless
/// its box says it's archived; any other kind, or one its box hasn't listed, nowhere.
pub fn place(facts: Option<&Facts>) -> Option<Place> {
    let facts = facts?;
    if facts.is_archived() {
        return None;
    }
    if facts.kind.is_project() {
        Some(Place::Project(facts.project.clone().unwrap_or_else(|| facts.directory.clone())))
    } else if facts.kind.is_chat() {
        Some(Place::Chat)
    } else {
        None
    }
}

/// What a client's sidebar shows, as the box's facts say: pinned and Chats newest first; the
/// projects by their newest session shown, those with none last, ties by name (its list's name,
/// else its folder's last part; Finder's order); each project's sessions newest first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sidebar {
    pub pinned: Vec<String>,
    pub chats: Vec<String>,
    pub projects: Vec<ProjectGroup>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectGroup {
    pub directory: String,
    pub name: String,
    pub sessions: Vec<String>,
    /// The computer it's on, where that tells it from another group of the same name.
    pub computer: Option<String>,
    /// Another computer's project (`directory` is the list's key for it, not a folder here).
    pub elsewhere: bool,
}

/// The sidebar for `sessions` and `projects` as a box lists them.
pub fn sidebar(sessions: &HashMap<String, Facts>, projects: &HashMap<String, Project>) -> Sidebar {
    sidebar_with_order(sessions, projects, None)
}

fn sidebar_with_order(sessions: &HashMap<String, Facts>, projects: &HashMap<String, Project>, order: Option<&[String]>) -> Sidebar {
    let shown: Vec<(&String, &Facts, Place)> = sessions.iter().filter_map(|(id, facts)| place(Some(facts)).map(|place| (id, facts, place))).collect();
    let newest = |mut rows: Vec<(&String, &Facts)>| -> Vec<String> {
        rows.sort_by(|a, b| b.1.updated.partial_cmp(&a.1.updated).unwrap_or(Ordering::Equal).then_with(|| a.0.cmp(b.0)));
        rows.into_iter().map(|(id, _)| id.clone()).collect()
    };
    let pinned = newest(shown.iter().filter(|(_, facts, _)| facts.is_pinned()).map(|(id, facts, _)| (*id, *facts)).collect());
    let chats = newest(shown.iter().filter(|(_, _, place)| *place == Place::Chat).map(|(id, facts, _)| (*id, *facts)).collect());

    let mut grouped: BTreeMap<String, Vec<(&String, &Facts)>> = BTreeMap::new();
    for (id, facts, place) in &shown {
        if let Place::Project(directory) = place {
            grouped.entry(directory.clone()).or_default().push((id, facts));
        }
    }
    for directory in projects.keys() {
        grouped.entry(directory.clone()).or_default();
    }
    let mut groups: Vec<(f64, ProjectGroup)> = grouped
        .into_iter()
        .map(|(directory, rows)| {
            let top = rows.iter().map(|(_, facts)| facts.updated).fold(None, |most: Option<f64>, updated| Some(most.map_or(updated, |most| most.max(updated))));
            let name = project_name(&directory, projects);
            let on = projects.get(&directory).and_then(|project| project.on.as_ref()).or_else(|| rows.first().and_then(|(_, facts)| facts.on.as_ref())).cloned();
            (top.unwrap_or(0.0), ProjectGroup { name, sessions: newest(rows), elsewhere: on.is_some(), computer: on.map(|on| on.name), directory })
        })
        .collect();
    if let Some(order) = order {
        // (Another computer's projects aren't in this one's order: after it, newest first.)
        let ranks: HashMap<&str, usize> = order.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
        groups.sort_by(|(left, a), (right, b)| {
            let rank = |group: &ProjectGroup| ranks.get(group.directory.as_str()).copied().unwrap_or(usize::MAX);
            rank(a).cmp(&rank(b)).then_with(|| right.partial_cmp(left).unwrap_or(Ordering::Equal)).then_with(|| finder_order(&a.name, &b.name))
        });
    } else {
        groups.sort_by(|(left, a), (right, b)| {
            right.partial_cmp(left).unwrap_or(Ordering::Equal).then_with(|| finder_order(&a.name, &b.name)).then_with(|| a.directory.cmp(&b.directory))
        });
    }
    let mut projects: Vec<ProjectGroup> = groups.into_iter().map(|(_, group)| group).collect();
    // A group names its computer only where another group has the same name.
    let names: Vec<String> = projects.iter().map(|group| group.name.clone()).collect();
    for group in &mut projects {
        if names.iter().filter(|name| **name == group.name).count() < 2 {
            group.computer = None;
        }
    }
    Sidebar { pinned, chats, projects }
}

/// A project's name: as its box's list names it, else its folder's.
pub fn project_name(directory: &str, projects: &HashMap<String, Project>) -> String {
    if let Some(name) = projects.get(directory).and_then(|project| project.name.clone()) {
        return name;
    }
    let trimmed = directory.trim_end_matches('/');
    trimmed.rsplit('/').next().filter(|last| !last.is_empty()).unwrap_or(directory).to_owned()
}

/// Finder's order for names: case-insensitive, numbers by their value ("web2" before "web10").
pub fn finder_order(a: &str, b: &str) -> Ordering {
    let (mut left, mut right) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(digit) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(digit);
                        chars.next();
                    }
                    digits.trim_start_matches('0').to_owned()
                };
                let (x, y) = (number(&mut left), number(&mut right));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                left.next();
                right.next();
            }
        }
    }
}
