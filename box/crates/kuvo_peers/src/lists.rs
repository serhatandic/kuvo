//! Setup list and catalog snapshots. Origins always come from the receiving computer's roster.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use kuvo_protocol::{
    json::{Null, Nullable},
    routes::{self, types::*},
};
use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::{
    directory::{ComputerKey, Ownership, RecordScope},
    kept,
    reach::{Millis, Standing},
    setup_lists::View,
};

pub type Computer = GetKuvoSetupOutputsStatus200PendingComputersItem;
pub type Facts = GetKuvoSessionsListStatus200SessionsItem;
pub type ProjectFacts = GetKuvoSessionsListStatus200ProjectsItem;
pub type Kind = GetKuvoSessionsListStatus200SessionsItemKind4;
pub type SessionAction = GetKuvoSessionsListStatus200ProjectsItemOnActionsItem2;
pub type RowComputer = GetKuvoSessionsListStatus200ProjectsItemOn;
pub type Ask = GetKuvoSessionsListStatus200NeedsYouItemAsks;
pub type Queue = GetKuvoSessionsQueuesEventsStatus200QueuesItem;
type NeedsYou = GetKuvoSessionsListStatus200NeedsYouItem;

pub const FEATURE: &str = "line.setup-lists";
pub const BOT_FEATURE: &str = "bots.peer-activity";
pub const QUEUE_FEATURE: &str = "work-queue.elsewhere";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session {
    pub id: String,
    pub facts: Facts,
    pub actions: Vec<SessionAction>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Project {
    pub directory: String,
    pub facts: ProjectFacts,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Waiting {
    pub session: String,
    pub since: Number,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub asks: Ask,
    pub actions: Vec<GetKuvoSetupWaitingEventsStatus200WaitingItemActionsItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Queued {
    pub session: String,
    pub queue: Queue,
    pub actions: Vec<GetKuvoSetupQueuesEventsStatus200QueuesItemActionsItem4>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Catalog {
    pub offered: GetKuvoSetupRunsOnEventsStatus200ComputersItemOffered,
    pub agents: Vec<Computer>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            offered: GetKuvoSetupRunsOnEventsStatus200ComputersItemOffered {
                known: GetKuvoSetupRunsOnEventsStatus200ComputersItemOfferedKnown {
                    engine: false,
                    agents: false,
                },
                models: vec![],
                providers: None,
                listing: None,
                note: None,
                generations: None,
                scoped: None,
                starred: None,
                start_choice: None,
                starting: None,
                unavailable: None,
            },
            agents: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BotActivity {
    pub session: String,
    pub now: GetKuvoBotsStatus200BotsItemNow,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wakes: Option<Number>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Lists {
    pub sessions: Vec<Session>,
    pub projects: Vec<Project>,
    pub waiting: Vec<Waiting>,
    pub queues: Vec<Queued>,
    pub catalog: Catalog,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bots: Option<BTreeMap<String, BotActivity>>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Engine,
    Bridge,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LocalSession {
    pub facts: Facts,
    pub place: Option<Place>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalList {
    pub epoch: String,
    pub seq: u64,
    pub sessions: BTreeMap<String, LocalSession>,
    pub projects: BTreeMap<String, ProjectFacts>,
    #[serde(default)]
    pub gone: BTreeMap<String, serde_json::Value>,
    pub needs_you: Option<Vec<NeedsYou>>,
    pub pending: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct Local {
    pub list: Option<LocalList>,
    pub catalog: Catalog,
    pub catalog_pending: bool,
    pub queues: Vec<Queued>,
    pub bots: Option<BTreeMap<String, BotActivity>>,
    pub ownership: Ownership,
    /// Work Queue items linked to sessions, and the peers whose complete copies are known.
    pub queue_sessions: Vec<String>,
    pub queue_copies: BTreeSet<ComputerKey>,
    /// Sessions created before the engine list reports them.
    pub created: BTreeMap<String, (Place, Option<String>)>,
}

impl Local {
    pub fn load(run: &Path) -> Self {
        // Layout 23 predates project counts in some kept files.
        let list = kept::read::<serde_json::Value>(&run.join("lists/sessions.json")).and_then(
            |mut value| {
                for facts in value.get_mut("projects")?.as_object_mut()?.values_mut() {
                    let facts = facts.as_object_mut()?;
                    for field in ["asking", "working"] {
                        facts.entry(field).or_insert(0.into());
                    }
                }
                serde_json::from_value(value).ok()
            },
        );
        let catalog = kept::read(&run.join("lists/runs-on.json"));
        Self {
            list,
            catalog_pending: catalog.is_some(),
            catalog: catalog.unwrap_or_default(),
            ownership: load_ownership(run),
            ..Self::default()
        }
    }

    pub(crate) fn holds(&self, id: &str) -> bool {
        self.list
            .as_ref()
            .and_then(|list| list.sessions.get(id))
            .is_some_and(|row| row.facts.away != Some(true))
    }

    pub fn publication(&self) -> Option<Lists> {
        let list = self.list.as_ref()?;
        let sessions: Vec<_> = list
            .sessions
            .iter()
            .filter(|(_, row)| row.facts.away != Some(true))
            .map(|(id, row)| {
                let actions = if self.ownership.frozen.contains(id) {
                    vec![]
                } else if matches!(row.facts.kind, Kind::Subagent | Kind::Hidden | Kind::Seat) {
                    vec![SessionAction::Open]
                } else {
                    vec![SessionAction::Open, SessionAction::Manage]
                };
                Session {
                    id: id.clone(),
                    facts: row.facts.clone(),
                    actions,
                }
            })
            .collect();
        let asks: BTreeMap<_, _> = list
            .needs_you
            .iter()
            .flatten()
            .map(|row| (&row.session, &row.asks))
            .collect();
        let waiting = sessions
            .iter()
            .filter(|row| waits(&row.facts))
            .map(|row| Waiting {
                session: row.id.clone(),
                since: row.facts.asked.clone().expect("waiting session has an ask"),
                title: row.facts.title.clone(),
                asks: asks
                    .get(&row.id)
                    .map_or_else(default_ask, |ask| (*ask).clone()),
                actions: if row.actions.is_empty() {
                    vec![]
                } else {
                    vec![GetKuvoSetupWaitingEventsStatus200WaitingItemActionsItem::Review]
                },
            })
            .collect();
        let queues = self
            .queues
            .iter()
            .filter(|row| self.holds(&row.session))
            .map(|row| {
                let mut row = row.clone();
                if self.ownership.frozen.contains(&row.session) {
                    row.actions.clear();
                }
                row
            })
            .collect();
        let projects = list
            .projects
            .iter()
            .map(|(directory, facts)| Project {
                directory: directory.clone(),
                facts: facts.clone(),
            })
            .collect();
        Some(Lists {
            sessions,
            projects,
            waiting,
            queues,
            catalog: self.catalog.clone(),
            bots: self.bots.clone(),
        })
    }
}

fn load_ownership(run: &Path) -> Ownership {
    let mut ownership = Ownership::default();
    crate::moves::locate(run, &mut ownership);
    if let Some(home) = run.parent() {
        for (id, folder) in matching_paths(&home.join("bots"), "bot_", None) {
            let Some(bot) = kept::read::<BotDescription>(&folder.join("bot.json")) else {
                continue;
            };
            let Ok(session) = fs::read_to_string(folder.join("session")) else {
                continue;
            };
            let session = session.trim();
            if bot.version == 1 && bot.id == id && !session.is_empty() {
                ownership.held.insert((RecordScope::Bot, id));
                ownership.held.insert((RecordScope::Bot, session.into()));
            }
        }
        ownership.held.extend(
            matching_paths(&home.join("groups"), "grp_", None)
                .filter(|(_, folder)| folder.join("group.json").is_file())
                .map(|(id, _)| (RecordScope::Group, id)),
        );
    }
    ownership
}

#[derive(Deserialize)]
struct BotDescription {
    id: String,
    #[serde(rename = "kuvo")]
    version: u32,
    #[serde(rename = "name")]
    _name: String,
}

pub(crate) fn matching_paths<'a>(
    directory: &Path,
    prefix: &'a str,
    extension: Option<&'a str>,
) -> impl Iterator<Item = (String, PathBuf)> + use<'a> {
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(move |entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            let id = match extension {
                Some(ext) => name.strip_suffix(ext)?,
                None => &name,
            };
            let suffix = id.strip_prefix(prefix)?;
            (!suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()))
                .then(|| (id.to_owned(), entry.path()))
        })
}

pub(crate) fn record(id: &str) -> (RecordScope, String) {
    let scope = if id.starts_with("bot_") {
        RecordScope::Bot
    } else if id.starts_with("grp_") {
        RecordScope::Group
    } else {
        RecordScope::Session
    };
    (scope, id.to_owned())
}

fn default_ask() -> Ask {
    Ask {
        kind: GetKuvoSessionsListStatus200NeedsYouItemAsksKind3::Other,
        summary: "waits on you".into(),
    }
}

#[derive(Clone)]
pub struct Origin {
    pub computer: Computer,
    pub standing: Standing,
    pub here: bool,
}

impl Origin {
    pub fn remote(&self, actions: &[SessionAction]) -> RowComputer {
        RowComputer {
            id: self.computer.id.clone(),
            name: self.computer.name.clone(),
            away: self.standing.away(),
            updating: self.standing.updating(),
            last_seen: self.standing.last_seen().map(Number::from),
            actions: self.actions(actions),
        }
    }

    fn actions<T: Clone>(&self, actions: &[T]) -> Vec<T> {
        if self.standing.away() {
            vec![]
        } else {
            actions.to_vec()
        }
    }

    fn last_seen(&self) -> Option<Number> {
        self.standing.last_seen().map(Number::from)
    }
    fn here(&self) -> Option<bool> {
        self.here.then_some(true)
    }
}

impl View {
    pub fn list_origins(&self, now: Millis) -> impl Iterator<Item = (&Lists, Origin)> {
        self.roster.members.iter().filter_map(move |peer| {
            let copy = self.lists.get(&peer.key)?;
            Some((
                &copy.value,
                Origin {
                    computer: peer.computer(),
                    standing: self.copy_standing(peer.key, copy.seen, now),
                    here: false,
                },
            ))
        })
    }

    fn all_origins<'a>(
        &'a self,
        now: Millis,
        local: &'a Option<std::sync::Arc<Lists>>,
    ) -> impl Iterator<Item = (&'a Lists, Origin)> {
        local
            .iter()
            .map(|lists| {
                (
                    lists.as_ref(),
                    Origin {
                        computer: self.roster.local(),
                        standing: Standing::Live,
                        here: true,
                    },
                )
            })
            .chain(self.list_origins(now))
    }

    pub fn pending_lists(&self) -> Vec<Computer> {
        self.pending(FEATURE, |key| self.lists.contains_key(key))
    }

    pub fn sessions(&self, now: Millis) -> routes::get_kuvo_setup_sessions_events::Status200 {
        let local = &self.local_lists;
        let mut sessions = Vec::new();
        let mut projects = Vec::new();
        for (lists, origin) in self.all_origins(now, local) {
            sessions.extend(
                lists
                    .sessions
                    .iter()
                    .filter(|row| self.lists_owner(&row.id, &origin.computer.id, now))
                    .map(|row| GetKuvoSetupSessionsEventsStatus200SessionsItem {
                        id: row.id.clone(),
                        facts: row.facts.clone(),
                        actions: origin.actions(&row.actions),
                        computer: origin.computer.clone(),
                        away: origin.standing.away(),
                        last_seen: origin.last_seen(),
                        updating: origin.standing.updating(),
                        here: origin.here(),
                    }),
            );
            projects.extend(lists.projects.iter().map(|row| {
                GetKuvoSetupSessionsEventsStatus200ProjectsItem {
                    directory: row.directory.clone(),
                    facts: row.facts.clone(),
                    computer: origin.computer.clone(),
                    away: origin.standing.away(),
                    last_seen: origin.last_seen(),
                    updating: origin.standing.updating(),
                    here: origin.here(),
                }
            }));
        }
        sessions.sort_by(|a, b| newest(&a.facts.updated, &b.facts.updated).then(a.id.cmp(&b.id)));
        routes::get_kuvo_setup_sessions_events::Status200 {
            sessions,
            projects,
            pending_computers: Some(self.pending_lists()),
        }
    }

    pub fn waiting(&self, now: Millis) -> routes::get_kuvo_setup_waiting_events::Status200 {
        let local = &self.local_lists;
        let mut rows = Vec::new();
        for (lists, origin) in self.all_origins(now, local) {
            rows.extend(
                lists
                    .waiting
                    .iter()
                    .filter(|row| self.lists_owner(&row.session, &origin.computer.id, now))
                    .map(|row| GetKuvoSetupWaitingEventsStatus200WaitingItem {
                        session: row.session.clone(),
                        since: row.since.clone(),
                        title: row.title.clone(),
                        asks: row.asks.clone(),
                        actions: origin.actions(&row.actions),
                        computer: origin.computer.clone(),
                        away: origin.standing.away(),
                        last_seen: origin.last_seen(),
                        updating: origin.standing.updating(),
                        here: origin.here(),
                    }),
            );
        }
        rows.sort_by(|a, b| newest(&a.since, &b.since));
        routes::get_kuvo_setup_waiting_events::Status200 {
            waiting: rows,
            pending_computers: Some(self.pending_lists()),
        }
    }

    pub fn queues(&self, now: Millis) -> routes::get_kuvo_setup_queues_events::Status200 {
        let local = &self.local_lists;
        let mut queues = Vec::new();
        for (lists, origin) in self.all_origins(now, local) {
            queues.extend(
                lists
                    .queues
                    .iter()
                    .filter(|row| self.lists_owner(&row.session, &origin.computer.id, now))
                    .map(|row| GetKuvoSetupQueuesEventsStatus200QueuesItem {
                        session: row.session.clone(),
                        queue: row.queue.clone(),
                        actions: origin.actions(&row.actions),
                        computer: origin.computer.clone(),
                        away: origin.standing.away(),
                        last_seen: origin.last_seen(),
                        updating: origin.standing.updating(),
                        here: origin.here(),
                    }),
            );
        }
        routes::get_kuvo_setup_queues_events::Status200 {
            queues,
            pending_computers: Some(self.pending_lists()),
        }
    }

    pub fn runs_on(&self, now: Millis) -> routes::get_kuvo_setup_runs_on_events::Status200 {
        let here = GetKuvoSetupRunsOnEventsStatus200ComputersItem {
            computer: self.roster.local(),
            agents: self.local.catalog.agents.clone(),
            offered: self.local.catalog.offered.clone(),
            here: Some(true),
            pending: self.local.catalog_pending.then_some(true),
            away: None,
            last_seen: None,
            updating: None,
        };
        let computers = std::iter::once(here)
            .chain(self.list_origins(now).map(|(lists, origin)| {
                GetKuvoSetupRunsOnEventsStatus200ComputersItem {
                    computer: origin.computer.clone(),
                    agents: lists.catalog.agents.clone(),
                    offered: lists.catalog.offered.clone(),
                    here: None,
                    pending: None,
                    away: (origin.standing != Standing::Live).then_some(true),
                    last_seen: origin.last_seen(),
                    updating: origin.standing.updating(),
                }
            }))
            .collect();
        routes::get_kuvo_setup_runs_on_events::Status200 {
            computers,
            pending_computers: Some(self.pending_lists()),
        }
    }

    pub fn sidebar(
        &self,
        now: Millis,
    ) -> Option<routes::get_kuvo_setup_sessions_list_events::Status200> {
        let pending_computers = self.pending_lists();
        let local = self.roster.local();
        let local_list = self.local.list.as_ref();
        if local_list.is_none() && self.lists.is_empty() && pending_computers.is_empty() {
            return None;
        }
        let mut sessions: BTreeMap<_, _> = local_list
            .iter()
            .flat_map(|list| list.sessions.iter())
            .map(|(id, row)| {
                let mut facts = row.facts.clone();
                facts.computer = Some(local.clone());
                (id.clone(), facts)
            })
            .collect();
        let mut projects: BTreeMap<_, _> = local_list
            .iter()
            .flat_map(|list| list.projects.iter())
            .map(|(id, facts)| {
                let mut facts = facts.clone();
                facts.computer = Some(local.clone());
                (id.clone(), facts)
            })
            .collect();
        let project_order = local_list
            .map(|_| ordered_projects(&projects))
            .unwrap_or_default();
        let mut archived_groups = archived(&sessions, &projects);
        let mut needs: BTreeMap<_, _> = local_list
            .map(|list| {
                list.needs_you.clone().unwrap_or_else(|| {
                    sessions
                        .iter()
                        .filter(|(_, facts)| needs_you(facts))
                        .map(|(id, _)| NeedsYou {
                            session: id.clone(),
                            asks: default_ask(),
                        })
                        .collect()
                })
            })
            .unwrap_or_default()
            .into_iter()
            .map(|row| (row.session.clone(), row))
            .collect();

        for (lists, origin) in self.list_origins(now) {
            let project_key = |folder: &str| format!("{}:{folder}", origin.computer.id);
            let mut remote = BTreeMap::new();
            for row in &lists.sessions {
                let moved_here = self
                    .moved(&row.id, now)
                    .is_some_and(|owner| owner == origin.computer.id);
                if sessions.get(&row.id).is_some_and(|before| {
                    !(before.away == Some(true) && before.on.is_none() && moved_here)
                }) || !self.lists_owner(&row.id, &origin.computer.id, now)
                {
                    continue;
                }
                let mut facts = row.facts.clone();
                facts.computer = Some(origin.computer.clone());
                facts.on = Some(origin.remote(&row.actions));
                if facts.kind == Kind::Project {
                    facts.project = Some(project_key(
                        facts.project.as_deref().unwrap_or(&facts.directory),
                    ));
                }
                sessions.insert(row.id.clone(), facts.clone());
                remote.insert(row.id.clone(), facts);
            }
            for row in &lists.waiting {
                if remote
                    .get(&row.session)
                    .is_some_and(|facts| matches!(facts.kind, Kind::Chat | Kind::Project))
                {
                    needs.insert(
                        row.session.clone(),
                        NeedsYou {
                            session: row.session.clone(),
                            asks: row.asks.clone(),
                        },
                    );
                }
            }
            let names = lists
                .projects
                .iter()
                .map(|row| {
                    (
                        project_key(&row.directory),
                        row.facts
                            .name
                            .clone()
                            .unwrap_or_else(|| base_name(&row.directory).to_owned()),
                    )
                })
                .collect();
            let remote_projects = peer_projects(&remote, names, &origin);
            archived_groups.extend(archived(&remote, &remote_projects));
            projects.extend(remote_projects);
        }
        let mut needs_you: Vec<_> = needs.into_values().collect();
        needs_you.sort_by(|a, b| {
            let at = |id: &str| {
                sessions
                    .get(id)
                    .and_then(|facts| facts.asked.as_ref())
                    .and_then(Number::as_f64)
                    .unwrap_or_default()
            };
            at(&b.session)
                .total_cmp(&at(&a.session))
                .then(a.session.cmp(&b.session))
        });
        Some(routes::get_kuvo_setup_sessions_list_events::Status200 {
            at: local_list.map_or_else(
                || "peers".into(),
                |list| format!("{}.{}", list.epoch, list.seq),
            ),
            full: Some(true),
            gone: None,
            sessions,
            projects,
            start_project: Some(
                project_order
                    .first()
                    .cloned()
                    .map_or(Nullable::Null(Null), Nullable::Value),
            ),
            project_order: Some(project_order),
            archived_groups,
            needs_you,
            pending: local_list
                .and_then(|list| list.pending.clone())
                .filter(|pending| !pending.is_empty()),
            pending_computers: Some(pending_computers),
        })
    }

    pub fn waiting_counts(
        &self,
        now: Millis,
    ) -> Option<routes::get_kuvo_setup_waiting_counts::Status200> {
        let listing = self.sidebar(now)?;
        let mut bots = BTreeSet::new();
        if let Some(local) = &self.local.bots {
            bots.extend(
                local
                    .iter()
                    .filter(|(_, bot)| number(&bot.now.asking) > 0.0)
                    .map(|(id, _)| id.clone()),
            );
        }
        for (lists, origin) in self.list_origins(now) {
            bots.extend(
                lists
                    .bots
                    .iter()
                    .flatten()
                    .filter(|(_, bot)| {
                        number(&bot.now.asking) > 0.0
                            && listing
                                .sessions
                                .get(&bot.session)
                                .and_then(|facts| facts.computer.as_ref())
                                == Some(&origin.computer)
                    })
                    .map(|(id, _)| id.clone()),
            );
        }
        let waiting: Vec<_> = listing
            .sessions
            .values()
            .filter(|facts| waits(facts))
            .collect();
        let counted: BTreeSet<_> = waiting
            .iter()
            .flat_map(|facts| {
                facts.bot.iter().chain(
                    facts
                        .line
                        .iter()
                        .flatten()
                        .filter_map(|id| listing.sessions.get(id)?.bot.as_ref()),
                )
            })
            .collect();
        let dock = waiting.len() + bots.iter().filter(|id| !counted.contains(id)).count();
        let queue = self
            .local
            .queue_sessions
            .iter()
            .filter(|id| {
                listing.sessions.get(*id).is_some_and(|facts| {
                    facts.asked.is_some()
                        && facts.archived != Some(true)
                        && facts.away != Some(true)
                })
            })
            .count();
        let mut pending: BTreeMap<_, _> = listing
            .pending_computers
            .into_iter()
            .flatten()
            .chain(self.pending(BOT_FEATURE, |key| {
                self.lists
                    .get(key)
                    .is_some_and(|copy| copy.value.bots.is_some())
            }))
            .chain(self.pending(QUEUE_FEATURE, |key| self.local.queue_copies.contains(key)))
            .map(|computer| (computer.id.clone(), computer))
            .collect();
        let mut pending: Vec<_> = std::mem::take(&mut pending).into_values().collect();
        pending.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Some(routes::get_kuvo_setup_waiting_counts::Status200 {
            sessions: waiting.len().into(),
            bots: bots.len().into(),
            queue: queue.into(),
            dock: Some(dock.into()),
            pending_computers: Some(pending),
        })
    }
}

fn number(value: &Number) -> f64 {
    value.as_f64().unwrap_or_default()
}

fn newest(a: &Number, b: &Number) -> std::cmp::Ordering {
    number(b).total_cmp(&number(a))
}

fn needs_you(facts: &Facts) -> bool {
    waits(facts) && matches!(facts.kind, Kind::Chat | Kind::Project)
}

fn waits(facts: &Facts) -> bool {
    facts.asked.is_some()
        && matches!(facts.kind, Kind::Chat | Kind::Project | Kind::Bot)
        && facts.archived != Some(true)
        && facts.away != Some(true)
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(path)
}

fn ordered_projects(projects: &BTreeMap<String, ProjectFacts>) -> Vec<String> {
    let mut rows: Vec<_> = projects.iter().collect();
    rows.sort_by(|(a, one), (b, two)| {
        let one_at = one.updated.as_ref().map_or(0.0, number);
        let two_at = two.updated.as_ref().map_or(0.0, number);
        let one_name = one
            .name
            .as_deref()
            .unwrap_or_else(|| base_name(a))
            .to_lowercase();
        let two_name = two
            .name
            .as_deref()
            .unwrap_or_else(|| base_name(b))
            .to_lowercase();
        two_at
            .total_cmp(&one_at)
            .then(one_name.cmp(&two_name))
            .then(a.cmp(b))
    });
    rows.into_iter().map(|(id, _)| id.clone()).collect()
}

fn archived(
    sessions: &BTreeMap<String, Facts>,
    projects: &BTreeMap<String, ProjectFacts>,
) -> Vec<GetKuvoSessionsListStatus200ArchivedGroupsItem> {
    use GetKuvoSessionsListStatus200ArchivedGroupsItemKind as Archive;
    let mut rows: Vec<_> = sessions
        .iter()
        .filter(|(_, facts)| facts.archived == Some(true) && facts.away != Some(true))
        .collect();
    rows.sort_by(|(id, a), (other, b)| newest(&a.updated, &b.updated).then(id.cmp(other)));
    let mut groups: Vec<GetKuvoSessionsListStatus200ArchivedGroupsItem> = vec![];
    for (id, facts) in rows {
        let kind = match facts.kind {
            Kind::Chat => Archive::Chat,
            Kind::Project => Archive::Project,
            Kind::Bot => Archive::Bot,
            _ => continue,
        };
        let directory = facts.project.as_ref().unwrap_or(&facts.directory);
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.kind == kind && &group.directory == directory)
        {
            group.sessions.push(id.clone());
            continue;
        }
        let title = if kind == Archive::Chat {
            "Chats".into()
        } else {
            projects
                .get(directory)
                .and_then(|project| project.name.clone())
                .unwrap_or_else(|| base_name(directory).into())
        };
        groups.push(GetKuvoSessionsListStatus200ArchivedGroupsItem {
            directory: directory.clone(),
            kind,
            title,
            sessions: vec![id.clone()],
        });
    }
    groups
}

fn rank(facts: &Facts) -> u8 {
    if facts.pinned == Some(true) {
        0
    } else if facts.away != Some(true) && (facts.asked.is_some() || facts.running == Some(true)) {
        1
    } else {
        2
    }
}

pub fn page(
    sessions: &BTreeMap<String, Facts>,
    project: &str,
    shown: usize,
    step: u32,
) -> routes::get_kuvo_sessions_page::Status200 {
    let mut rows: Vec<_> = sessions
        .iter()
        .filter(|(_, facts)| {
            facts.kind == Kind::Project
                && facts.archived != Some(true)
                && facts.project.as_deref() == Some(project)
        })
        .collect();
    rows.sort_by(|(id, a), (other, b)| {
        rank(a)
            .cmp(&rank(b))
            .then(newest(&a.updated, &b.updated))
            .then(id.cmp(other))
    });
    let total = rows.len();
    let count = total
        .saturating_sub(shown)
        .min(10 << step)
        .min(5000 - shown);
    let more = (count > 0).then(|| GetKuvoSessionsListStatus200ProjectsItemMore {
        cursor: format!("s{}.{}", shown + count, step + 1),
        count: count.into(),
    });
    routes::get_kuvo_sessions_page::Status200 {
        project: project.into(),
        sessions: rows
            .into_iter()
            .take(shown)
            .map(|(id, _)| id.clone())
            .collect(),
        total: total.into(),
        more: more.map_or(Nullable::Null(Null), Nullable::Value),
    }
}

fn peer_projects(
    sessions: &BTreeMap<String, Facts>,
    mut names: BTreeMap<String, String>,
    origin: &Origin,
) -> BTreeMap<String, ProjectFacts> {
    let mut totals = BTreeMap::<String, (Option<Number>, usize, usize)>::new();
    for facts in sessions
        .values()
        .filter(|facts| facts.kind == Kind::Project && facts.archived != Some(true))
    {
        let Some(project) = &facts.project else {
            continue;
        };
        let (updated, asking, working) = totals.entry(project.clone()).or_default();
        if updated
            .as_ref()
            .is_none_or(|before| number(before) < number(&facts.updated))
        {
            *updated = Some(facts.updated.clone());
        }
        if facts.away != Some(true) {
            if facts.asked.is_some() {
                *asking += 1;
            } else if facts.running == Some(true) {
                *working += 1;
            }
        }
    }
    let keys: BTreeSet<_> = names.keys().chain(totals.keys()).cloned().collect();
    keys.into_iter()
        .map(|project| {
            let (updated, asking, working) = totals.remove(&project).unwrap_or_default();
            let page = page(sessions, &project, 6, 0);
            let more = match page.more {
                Nullable::Value(more) => Some(more),
                Nullable::Null(_) => None,
            };
            let facts = ProjectFacts {
                name: names.remove(&project),
                updated,
                asking: asking.into(),
                working: working.into(),
                computer: Some(origin.computer.clone()),
                on: Some(origin.remote(&[])),
                shown: Some(page.sessions),
                total: Some(page.total),
                more,
            };
            (project, facts)
        })
        .collect()
}
