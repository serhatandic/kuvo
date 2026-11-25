//! The resident owns peer copies, independently from local engine state. Readers subscribe to
//! immutable views. Loading the kept copies precedes publishing the first view.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kuvo_protocol::routes::types::{
    GetKuvoComputersStatus200ComputersItemAtWorkItem, GetKuvoComputersStatus200FleetUpdate,
    PostKuvoUpdateRetryStatus202 as FleetRow,
};
use kuvo_store::files::{FileStamp, file_stamp};
use kuvo_wire::{
    keys::{Host, encode},
    mail::{Envelope, MAX_PLAINTEXT},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{
    activity,
    devices::Devices,
    directory::{
        AgentKind, AnnouncedAgent, Announcement, Announcements, ComputerKey, Owner, Ownership,
        PreparedRelease, RecordScope, Roster, UpdateFacts, owned_digest,
    },
    kept,
    lists::{self, Computer, Lists, Local},
    notices::{self, History},
    parts::{self, Part, Parts},
    reach::{Millis, Reach, Standing, UpdatePresence},
    relay::{self, Relay},
    relay_routes, remote_work,
};

const LIST_FILE: &str = "lists-v1.json";
const NOTICE_FILE: &str = "notices-v1.json";
const PEER_ROUND: Duration = Duration::from_secs(3);
const HEARD_EVERY: Millis = 300_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Copy<T> {
    pub at: Millis,
    pub seen: Millis,
    #[serde(flatten)]
    pub value: T,
}

#[derive(Clone)]
pub struct View {
    pub roster: Arc<Roster>,
    pub local: Arc<Local>,
    pub(crate) local_lists: Option<Arc<Lists>>,
    pub(crate) reach: Arc<Reach>,
    pub(crate) announced: Arc<Announcements>,
    pub(crate) lists: BTreeMap<ComputerKey, Arc<Copy<Lists>>>,
    pub(crate) notice_copies: BTreeMap<ComputerKey, Arc<Copy<History>>>,
    pub(crate) notices: Arc<History>,
    pub(crate) move_notices: Arc<crate::moves::Notices>,
    pub(crate) move_waiting: Arc<BTreeSet<String>>,
    pub(crate) activities: BTreeMap<ComputerKey, Arc<activity::Saved>>,
    pub relay_connected: bool,
    bridge: Option<Arc<BridgeAdmission>>,
    update: Arc<UpdatePublication>,
    work_calls: bool,
    transfers: bool,
    submission_recovery: bool,
    conversation_search: bool,
    backup_receiver: bool,
    pub(crate) endpoint: Option<Arc<relay::Endpoint>>,
    pub(crate) system: Option<Arc<str>>,
    pub(crate) versions: LocalVersions,
}

impl View {
    pub(crate) fn local_running_version(&self) -> Option<&str> {
        self.update
            .facts
            .kuvo
            .as_deref()
            .or(self.versions.running.as_deref())
    }

    pub(crate) fn in_bridge_handoff(&self, peer: &str) -> bool {
        self.bridge.as_ref().is_some_and(|bridge| {
            self.announced
                .get(peer)
                .and_then(|entry| entry.computer.as_ref())
                .is_some_and(|computer| computer.in_bridge_handoff(&bridge.release))
        })
    }

    pub(crate) fn local_work_admitted(&self) -> bool {
        self.bridge.as_ref().is_none_or(|bridge| {
            !bridge.local_trial
                || self
                    .update
                    .facts
                    .kuvo
                    .as_deref()
                    .is_some_and(|version| version != bridge.release.version)
        })
    }

    pub(crate) fn work_admitted(&self, peer: &str) -> bool {
        self.local_work_admitted() && !self.in_bridge_handoff(peer)
    }

    pub(crate) fn local_update_row(&self) -> Option<&FleetRow> {
        self.update.local_row.as_ref()
    }

    pub(crate) fn peer_update_row(&self, key: &ComputerKey) -> Option<&FleetRow> {
        self.update.peer_rows.get(key)
    }

    pub(crate) fn fleet_summary(&self) -> Option<&GetKuvoComputersStatus200FleetUpdate> {
        self.update.summary.as_ref()
    }

    /// Check for Updates uses the already persisted observation even while the
    /// receiver is still publishing its new in-memory view.
    pub fn kept_peer_is_away(&self, run: &Path, key: &ComputerKey, now: Millis) -> bool {
        let key = encode(key);
        let heard = self.announced.get(&key).and_then(|entry| entry.heard);
        Reach::load(run).peer_is_away(&key, heard, now)
    }

    pub fn peer_updates(&self, key: &ComputerKey, now: Millis) -> (bool, UpdateFacts) {
        let (presence, facts) = self.peer_update(key, now);
        (presence.away_for.is_some(), facts.unwrap_or_default())
    }

    pub fn peer_update(
        &self,
        key: &ComputerKey,
        now: Millis,
    ) -> (UpdatePresence, Option<UpdateFacts>) {
        let key = encode(key);
        let announcement = self.announced.get(&key);
        let presence =
            self.reach
                .update_presence(&key, announcement.and_then(|entry| entry.heard), now);
        let facts = announcement
            .and_then(|entry| entry.computer.as_ref())
            .map(|computer| computer.updates());
        (presence, facts)
    }

    pub fn pending(&self, feature: &str, kept: impl Fn(&ComputerKey) -> bool) -> Vec<Computer> {
        self.roster
            .pending(&self.reach, &self.announced, feature, kept)
    }

    /// The agents a computer last told this one of.
    pub fn told_agents(&self, key: &ComputerKey) -> Vec<AnnouncedAgent> {
        self.announced
            .get(&encode(key))
            .map(|copy| copy.list.clone())
            .unwrap_or_default()
    }

    /// Whether the relay says a computer isn't there now.
    pub fn away(&self, key: &ComputerKey) -> bool {
        self.reach.away_since(&encode(key)).is_some()
    }

    pub fn advertises(&self, key: &ComputerKey, feature: &str) -> bool {
        self.announced
            .get(&encode(key))
            .and_then(|announcement| announcement.features.as_ref())
            .is_some_and(|features| features.iter().any(|name| name == feature))
    }

    pub fn announcing(&self, scope: RecordScope, id: &str) -> Vec<ComputerKey> {
        self.roster
            .members
            .iter()
            .filter(|member| {
                self.announced
                    .get(&encode(&member.key))
                    .is_some_and(|copy| copy.announces(scope, id))
            })
            .map(|member| member.key)
            .collect()
    }

    pub fn peer_session_name(&self, key: &ComputerKey, session: &str) -> Option<&str> {
        self.announced
            .get(&encode(key))
            .and_then(|copy| copy.list.iter().find(|agent| agent.session == session))
            .map(|agent| agent.name.as_str())
            .or_else(|| {
                self.lists
                    .get(key)?
                    .value
                    .sessions
                    .iter()
                    .find(|row| row.id == session)?
                    .facts
                    .title
                    .as_deref()
            })
    }

    pub fn peer_bot_named(&self, name: &str) -> Option<&AnnouncedAgent> {
        let lowered = name.to_lowercase();
        let mut candidates = self
            .roster
            .members
            .iter()
            .filter_map(|member| self.announced.get(&encode(&member.key)))
            .flat_map(|copy| &copy.list)
            .filter(|agent| agent.kind == AgentKind::Bot && agent.name.to_lowercase() == lowered);
        let found = candidates.next()?;
        candidates
            .all(|agent| agent.session == found.session)
            .then_some(found)
    }

    pub fn announced_sessions(&self) -> Vec<String> {
        self.roster
            .members
            .iter()
            .filter_map(|member| self.announced.get(&encode(&member.key)))
            .flat_map(|copy| copy.list.iter().map(|agent| agent.session.clone()))
            .filter(|session| session.starts_with("ses_"))
            .collect()
    }

    pub fn standing(&self, key: ComputerKey, now: Millis) -> Standing {
        self.copy_standing(key, self.lists.get(&key).map(|copy| copy.seen), now)
    }

    pub(crate) fn copy_standing(
        &self,
        key: ComputerKey,
        seen: impl Into<Option<Millis>>,
        now: Millis,
    ) -> Standing {
        let key = encode(&key);
        let release = self.announced.get(&key).and_then(Announcement::release);
        self.reach.copy_standing(&key, seen.into(), release, now)
    }

    pub fn owner_of(&self, scope: RecordScope, id: &str, now: Millis) -> Option<Owner> {
        let mut ownership = self.local.ownership.clone();
        if scope == RecordScope::Session && self.local.holds(id) {
            ownership.held.insert((scope, id.into()));
        }
        ownership.owner_of(scope, id, &self.roster, &self.announced, &self.reach, now)
    }

    pub(crate) fn features(&self) -> Vec<&'static str> {
        let mut features = crate::routes::FEATURES.to_vec();
        features.push("box.atWork");
        features.push("data.queued-device-authority");
        if self.backup_receiver {
            features.push("box.backups.relay");
        }
        if self.work_calls {
            features.extend([crate::calls::FEATURE, "work.elsewhere.start"]);
        }
        if self.transfers {
            features.extend([
                "moves",
                "moves.check",
                "moves.two-computers",
                "letters-read",
                "data.session-notes",
                "data.forward-evidence",
            ]);
        }
        if self.submission_recovery {
            features.push("typed.submission-recovery");
        }
        if self.conversation_search {
            features.extend([
                crate::memory_read::FEATURE,
                crate::memory_read::SEARCH_FEATURE,
                crate::owner_read::FEATURE,
            ]);
        }
        if self
            .local_lists
            .as_ref()
            .is_some_and(|lists| lists.bots.is_some())
        {
            features.push(lists::BOT_FEATURE);
        }
        features
    }

    pub(crate) fn at_work(&self) -> Vec<GetKuvoComputersStatus200ComputersItemAtWorkItem> {
        self.local
            .list
            .as_ref()
            .into_iter()
            .flat_map(lists::LocalList::working)
            .map(
                |(id, facts)| GetKuvoComputersStatus200ComputersItemAtWorkItem {
                    id: id.clone(),
                    title: facts
                        .title
                        .as_deref()
                        .and_then(crate::directory::work_title),
                },
            )
            .collect()
    }

    pub(crate) fn announced_agents(&self) -> Vec<AnnouncedAgent> {
        let mut agents = vec![];
        if let Some(lists) = &self.local_lists {
            let mut sessions: Vec<_> = lists
                .sessions
                .iter()
                .filter(|row| {
                    row.facts.parent.is_none()
                        && row.facts.archived != Some(true)
                        && !row.actions.is_empty()
                })
                .collect();
            sessions.sort_by(|a, b| {
                b.facts
                    .updated
                    .as_f64()
                    .unwrap_or_default()
                    .total_cmp(&a.facts.updated.as_f64().unwrap_or_default())
            });
            agents = sessions
                .into_iter()
                .filter_map(|row| {
                    Some(AnnouncedAgent {
                        session: row.id.clone(),
                        name: row.facts.title.clone()?,
                        kind: if row.facts.kind == lists::Kind::Bot {
                            AgentKind::Bot
                        } else {
                            AgentKind::Session
                        },
                        id: row.facts.bot.clone(),
                    })
                })
                .take(120)
                .collect();
        }
        agents
    }

    pub(crate) fn moved(&self, id: &str, now: Millis) -> Option<String> {
        self.moved_with(id, now, &self.local.ownership)
    }

    pub(crate) fn moved_with(
        &self,
        id: &str,
        now: Millis,
        ownership: &Ownership,
    ) -> Option<String> {
        self.move_address(id, now, ownership).map(|(key, _)| key)
    }

    pub(crate) fn move_address(
        &self,
        id: &str,
        now: Millis,
        ownership: &Ownership,
    ) -> Option<(String, Option<Millis>)> {
        let recorded = ownership.journal.get(&(RecordScope::Session, id.into()));
        let recorded_key = || {
            recorded.map(|location| {
                let key = &location.key;
                (
                    encode(ownership.replacements.get(key).unwrap_or(key)),
                    location.departed_at,
                )
            })
        };
        if self.local.holds(id) || recorded.is_some_and(|location| location.departed_at.is_none()) {
            return recorded_key();
        }
        let since = recorded.and_then(|location| location.departed_at);
        self.roster
            .members
            .iter()
            .filter_map(|peer| self.announced.get(&encode(&peer.key)))
            .flat_map(|entry| &entry.moved)
            .filter(|hint| {
                hint.session == id
                    && since.is_none_or(|at| hint.at >= at)
                    && now.saturating_sub(hint.at) < crate::moves::ROUTING_FOR
            })
            .max_by_key(|hint| hint.at)
            .map(|hint| (hint.key.clone(), Some(hint.at)))
            .or_else(recorded_key)
    }

    pub(crate) fn lists_owner(&self, id: &str, computer: &str, now: Millis) -> bool {
        self.moved(id, now).is_none_or(|owner| owner == computer)
    }
}

fn system_name() -> Option<Arc<str>> {
    match std::env::consts::OS {
        "macos" => {
            let plist = fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok();
            let version = plist.as_deref().and_then(|text| {
                text.split_once("<key>ProductVersion</key>")?
                    .1
                    .trim_start()
                    .strip_prefix("<string>")?
                    .split_once("</string>")
                    .map(|(version, _)| version)
                    .filter(|version| {
                        !version.is_empty() && version.len() <= 40 && !version.contains('<')
                    })
            });
            Some(version.map_or_else(
                || "macOS".into(),
                |version| format!("macOS {version}").into(),
            ))
        }
        "linux" => {
            let release = fs::read_to_string("/etc/os-release").ok();
            let name = release.as_deref().and_then(|text| {
                let value = text
                    .lines()
                    .find_map(|line| line.strip_prefix("PRETTY_NAME="))?;
                let value = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .unwrap_or(value);
                (!value.is_empty() && value.encode_utf16().count() <= 80 && !value.contains('"'))
                    .then_some(value)
            });
            Some(name.unwrap_or("Linux").into())
        }
        _ => None,
    }
}

pub type Clock = Arc<dyn Fn() -> Millis + Send + Sync>;

pub fn system_clock() -> Millis {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| {
            i64::try_from(time.as_millis()).unwrap_or(i64::MAX)
        })
}

#[derive(Clone)]
pub struct Reader {
    views: watch::Receiver<Arc<View>>,
    agent_mail: watch::Receiver<AgentMail>,
    clock: Clock,
    pub(crate) run: Arc<PathBuf>,
}

impl Reader {
    pub fn view(&self) -> Arc<View> {
        self.views.borrow().clone()
    }
    pub fn now(&self) -> Millis {
        (self.clock)()
    }
    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.views.changed().await
    }

    /// Whether this computer's agent publication named the sender to this peer.
    pub(crate) fn agent_sent_to(&self, peer: &ComputerKey, session: &str) -> Option<bool> {
        let sent = self.agent_mail.borrow();
        let list = sent.get(peer)?;
        if list.sessions.contains(session) {
            Some(true)
        } else {
            list.complete.then_some(false)
        }
    }
}

type AgentMail = BTreeMap<ComputerKey, SentAgents>;

#[derive(PartialEq, Eq)]
struct SentAgents {
    sessions: Arc<BTreeSet<String>>,
    complete: bool,
}

enum Change {
    Update(Arc<UpdatePublication>),
    Versions(LocalVersions, oneshot::Sender<()>),
    Local(Arc<Local>),
    Notices(Arc<History>),
    Dispatch(kuvo_server::Dispatcher),
    WorkCalls,
    TransferWorker,
    SubmissionRecovery,
    ConversationSearch,
    BackupReceiver,
    SetupRosters(watch::Receiver<Option<Arc<Roster>>>),
}

struct PublishedViews {
    views: watch::Sender<Arc<View>>,
    sent_agents: watch::Sender<AgentMail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeUpdate {
    pub recipients: BTreeSet<ComputerKey>,
    pub facts: UpdateFacts,
}

pub struct BridgeAdmission {
    pub release: PreparedRelease,
    pub local_trial: bool,
}

/// The local row's installed versions and the desktop's private update check.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct LocalVersions {
    pub version: Option<String>,
    pub running: Option<String>,
    pub app_check: Option<String>,
    pub is_mac: bool,
}

#[derive(Default, PartialEq)]
pub struct UpdatePublication {
    pub facts: UpdateFacts,
    pub bridge: Option<BridgeUpdate>,
    pub local_row: Option<FleetRow>,
    pub peer_rows: BTreeMap<ComputerKey, FleetRow>,
    pub summary: Option<GetKuvoComputersStatus200FleetUpdate>,
}

pub struct ResidentPeers {
    reader: Reader,
    changes: mpsc::Sender<Change>,
    relay: Option<relay::RelayHandle>,
    work_task: Option<JoinHandle<()>>,
    transfers: Option<(crate::moves::Handle, JoinHandle<()>)>,
    move_waiting: watch::Sender<Arc<BTreeSet<String>>>,
    letters: Option<JoinHandle<()>>,
    task: JoinHandle<()>,
}

impl Drop for ResidentPeers {
    fn drop(&mut self) {
        if let Some(work) = &self.work_task {
            work.abort();
        }
        if let Some((_, transfers)) = &self.transfers {
            transfers.abort();
        }
        if let Some(letters) = &self.letters {
            letters.abort();
        }
        self.task.abort();
    }
}

impl ResidentPeers {
    /// `roster` is the accepted setup view. The caller owns enrollment and validates membership.
    pub async fn start(
        home: &Path,
        roster: Roster,
        relay: Option<Relay>,
        devices: Option<Devices>,
        telling: bool,
        clock: Clock,
    ) -> io::Result<Self> {
        Self::start_with_bridge(home, roster, relay, devices, telling, clock, None).await
    }

    pub async fn start_with_bridge(
        home: &Path,
        roster: Roster,
        relay: Option<Relay>,
        devices: Option<Devices>,
        telling: bool,
        clock: Clock,
        bridge: Option<BridgeAdmission>,
    ) -> io::Result<Self> {
        let run = home.join("run");
        let files = Arc::new(run.clone());
        let endpoint = relay.as_ref().map(|relay| Arc::new(relay.endpoint.clone()));
        let mut resident =
            tokio::task::spawn_blocking(move || Resident::load(run, roster, telling, endpoint))
                .await?;
        resident.view.bridge = bridge.map(Arc::new);
        let (publish, views) = watch::channel(Arc::new(resident.view.clone()));
        let (sent_agents, agent_mail) = watch::channel(AgentMail::new());
        let (changes, change_rx) = mpsc::channel(16);
        let (move_waiting, waiting_updates) = watch::channel(resident.view.move_waiting.clone());
        let handle = relay.as_ref().map(Relay::handle);
        let task = tokio::spawn(resident.run(
            relay,
            devices,
            PublishedViews {
                views: publish,
                sent_agents,
            },
            change_rx,
            waiting_updates,
            clock.clone(),
        ));
        Ok(Self {
            reader: Reader {
                views,
                agent_mail,
                clock,
                run: files,
            },
            changes,
            relay: handle,
            work_task: None,
            transfers: None,
            move_waiting,
            letters: None,
            task,
        })
    }

    pub fn relay_handle(&self) -> Option<relay::RelayHandle> {
        self.relay.clone()
    }

    /// Enable remote work on this resident's relay once the session engine is available.
    pub async fn attach_work(
        &mut self,
        work: Arc<dyn crate::calls::RemoteWork>,
    ) -> io::Result<remote_work::WorkHandle> {
        if self.work_task.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Remote work already attached",
            ));
        }
        let relay = self.relay_handle().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "Remote work requires the resident relay",
            )
        })?;
        let (handle, task) = remote_work::start(self.reader(), relay, work)?;
        self.work_task = Some(task);
        self.changes
            .send(Change::WorkCalls)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Peer resident stopped"))?;
        Ok(handle)
    }

    pub async fn advertise_backup_receiver(&self) {
        let _ = self.changes.send(Change::BackupReceiver).await;
    }

    pub fn reader(&self) -> Reader {
        self.reader.clone()
    }

    /// Attach the session engine to the resident's move worker. The worker
    /// keeps its transfer state after the initiating request disconnects.
    pub async fn attach_moves(
        &mut self,
        engine: kuvo_engine::Engine,
        ownership: crate::ownership::Handle,
    ) -> io::Result<()> {
        self.attach_moves_with_queue(engine, ownership, None).await
    }

    pub async fn attach_moves_with_queue(
        &mut self,
        engine: kuvo_engine::Engine,
        ownership: crate::ownership::Handle,
        queue: Option<Arc<dyn crate::moves::work_queue::WorkQueueTransfer>>,
    ) -> io::Result<()> {
        let Some(relay) = self.relay_handle() else {
            return Ok(());
        };
        self.changes
            .send(Change::TransferWorker)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Peer resident stopped"))?;
        self.transfers = Some(crate::moves::start(
            self.reader(),
            relay.clone(),
            engine.clone(),
            ownership.clone(),
            self.move_waiting.clone(),
            queue,
        )?);
        self.letters = Some(crate::letters::start(
            self.reader(),
            relay,
            engine,
            ownership,
        )?);
        Ok(())
    }

    pub fn register_moves(
        &self,
        registry: &mut kuvo_server::Registry,
        engine: kuvo_engine::Engine,
    ) {
        if let Some((moves, _)) = &self.transfers {
            crate::moves::register_start(registry, &self.reader, moves.clone(), engine);
        }
    }

    pub async fn dispatch_with(&self, dispatcher: kuvo_server::Dispatcher) {
        let _ = self.changes.send(Change::Dispatch(dispatcher)).await;
    }

    /// Advertise submission recovery once the engine has connected its owner handler.
    pub async fn advertise_engine_submissions(&self) -> io::Result<()> {
        self.changes
            .send(Change::SubmissionRecovery)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Peer resident stopped"))
    }

    pub async fn advertise_conversation_search(&self) -> io::Result<()> {
        self.changes
            .send(Change::ConversationSearch)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Peer resident stopped"))
    }

    /// Track the roster handed down by setup authority after a durable membership or epoch change.
    pub async fn follow_setup_roster(&self, rosters: watch::Receiver<Option<Arc<Roster>>>) {
        let _ = self.changes.send(Change::SetupRosters(rosters)).await;
    }

    pub async fn local(&self, local: Local) {
        let _ = self.changes.send(Change::Local(Arc::new(local))).await;
    }

    pub async fn notices(&self, notices: History) {
        let _ = self.changes.send(Change::Notices(Arc::new(notices))).await;
    }

    pub async fn announce_versions(&self, versions: LocalVersions) -> io::Result<()> {
        let (done, published) = oneshot::channel();
        let unavailable = || {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "The computer list is not running.",
            )
        };
        self.changes
            .send(Change::Versions(versions, done))
            .await
            .map_err(|_| unavailable())?;
        published.await.map_err(|_| unavailable())
    }

    pub async fn announce_update(&self, update: Arc<UpdatePublication>) {
        let _ = self.changes.send(Change::Update(update)).await;
    }
}

struct Resident {
    run: PathBuf,
    move_files: Option<FileStamp>,
    view: View,
    list_parts: BTreeMap<ComputerKey, Parts>,
    notice_parts: BTreeMap<ComputerKey, Parts>,
    activity_parts: BTreeMap<ComputerKey, activity::Receiving>,
    move_parts: crate::moves::transfer::Receiving,
}

impl Resident {
    fn load(
        run: PathBuf,
        roster: Roster,
        telling: bool,
        endpoint: Option<Arc<relay::Endpoint>>,
    ) -> Self {
        let lists = read_copies(&run, &roster, LIST_FILE);
        let notice_copies = read_copies(&run, &roster, NOTICE_FILE);
        let activities = roster
            .members
            .iter()
            .filter_map(|peer| Some((peer.key, Arc::new(activity::load(&run, &peer.key)?))))
            .collect();
        let local = Local::load(&run);
        let view = View {
            roster: Arc::new(roster),
            local_lists: local.publication().map(Arc::new),
            local: Arc::new(local),
            reach: Arc::new(Reach::load(&run)),
            lists,
            notice_copies,
            activities,
            announced: Arc::new(kept::read(&run.join("peer-agents.json")).unwrap_or_default()),
            notices: Arc::new(notices::load(&run, telling)),
            move_notices: Arc::new(crate::moves::Notices::load(&run)),
            move_waiting: Arc::default(),
            relay_connected: false,
            bridge: None,
            update: Arc::default(),
            work_calls: false,
            transfers: false,
            submission_recovery: false,
            conversation_search: false,
            backup_receiver: false,
            endpoint,
            system: system_name(),
            versions: LocalVersions::default(),
        };
        Self {
            run,
            move_files: None,
            view,
            list_parts: BTreeMap::new(),
            notice_parts: BTreeMap::new(),
            activity_parts: BTreeMap::new(),
            move_parts: crate::moves::transfer::Receiving::default(),
        }
    }

    async fn run(
        mut self,
        mut relay: Option<Relay>,
        mut devices: Option<Devices>,
        publication: PublishedViews,
        mut changes: mpsc::Receiver<Change>,
        mut waiting_updates: watch::Receiver<Arc<BTreeSet<String>>>,
        clock: Clock,
    ) {
        let PublishedViews {
            views: publish,
            sent_agents,
        } = publication;
        let mut paired = relay_routes::Routes::default();
        let mut dispatch = None;
        let mut round = tokio::time::interval(PEER_ROUND);
        round.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut publisher = relay
            .as_ref()
            .map(|relay| Publisher::start(relay.sender.clone(), self.run.clone(), sent_agents));
        let mut grants = devices.as_ref().map_or_else(HashMap::new, Devices::grants);
        let mut pending_trust = None;
        let mut setup_rosters = None;
        loop {
            tokio::select! {
                Ok(()) = waiting_updates.changed() => {
                    self.view.move_waiting = waiting_updates.borrow_and_update().clone();
                    if let Some(publisher) = &mut publisher {
                        publisher.request(self.view.clone(), clock(), Refresh::None);
                    }
                }
                change = changes.recv() => match change {
                    Some(Change::Update(update)) => {
                        let announce = self.view.update.facts != update.facts
                            || self.view.update.bridge != update.bridge;
                        self.view.update = update;
                        if announce && let Some(publisher) = &mut publisher {
                            publisher.request(self.view.clone(), clock(), Refresh::None);
                        }
                    }
                    Some(Change::Versions(versions, done)) => {
                        if self.view.versions != versions {
                            self.view.versions = versions;
                            if let Some(publisher) = &mut publisher {
                                publisher.request(self.view.clone(), clock(), Refresh::None);
                            }
                            publish.send_replace(Arc::new(self.view.clone()));
                        }
                        let _ = done.send(());
                        continue;
                    }
                    Some(Change::Local(mut local)) => {
                        let incoming = Arc::make_mut(&mut local);
                        incoming.ownership.frozen.clone_from(&self.view.local.ownership.frozen);
                        incoming.ownership.journal.clone_from(&self.view.local.ownership.journal);
                        self.view.local_lists = local.publication().map(Arc::new);
                        self.view.local = local;
                    }
                    Some(Change::Notices(notices)) => self.view.notices = notices,
                    Some(Change::Dispatch(dispatcher)) => dispatch = Some(dispatcher),
                    Some(Change::WorkCalls) => {
                        self.view.work_calls = true;
                        if let Some(publisher) = &mut publisher {
                            publisher.request(self.view.clone(), clock(), Refresh::All);
                        }
                    },
                    Some(Change::TransferWorker) => {
                        self.view.transfers = true;
                        if let Some(publisher) = &mut publisher {
                            publisher.request(self.view.clone(), clock(), Refresh::All);
                        }
                    },
                    Some(Change::SubmissionRecovery) => {
                        self.view.submission_recovery = true;
                        if let Some(publisher) = &mut publisher {
                            publisher.request(self.view.clone(), clock(), Refresh::All);
                        }
                    }
                    Some(Change::ConversationSearch) => {
                        self.view.conversation_search = true;
                        if let Some(publisher) = &mut publisher {
                            publisher.request(self.view.clone(), clock(), Refresh::All);
                        }
                    }
                    Some(Change::BackupReceiver) => {
                        self.view.backup_receiver = true;
                        publish.send_replace(Arc::new(self.view.clone()));
                    }
                    Some(Change::SetupRosters(mut rosters)) => {
                        if let Some(roster) = rosters.borrow_and_update().clone() {
                            self.view.roster = roster;
                        }
                        setup_rosters = Some(rosters);
                    },
                    None => return,
                },
                roster = setup_roster_changed(&mut setup_rosters) => {
                    if let Some(roster) = roster {
                        self.view.roster = roster;
                        if let Some(publisher) = &mut publisher {
                            publisher.request(self.view.clone(), clock(), Refresh::All);
                        }
                    }
                }
                () = grants_changed(&mut devices) => {
                    if let (Some(relay), Some(devices)) = (&relay, &devices) {
                        let next = devices.grants();
                        if grants.iter().any(|(key, pairing)| next.get(key) != Some(pairing)) {
                            // Let a device that removed itself receive its reply. Route admission
                            // already checks the newer device view while the transport drains.
                            pending_trust = Some(tokio::time::Instant::now() + Duration::from_millis(200));
                        } else if pending_trust.is_none() {
                            let members = self.view.roster.members.iter().filter_map(|member| member.host()).collect();
                            let _ = relay.sender.trust(next.clone(), members).await;
                        }
                        grants = next;
                    }
                }
                () = wait_for_trust(pending_trust) => {
                    pending_trust = None;
                    if let Some(relay) = &relay {
                        let members = self.view.roster.members.iter().filter_map(|member| member.host()).collect();
                        let _ = relay.sender.trust(grants.clone(), members).await;
                    }
                }
                () = paired.reap() => continue,
                event = relay_event(&mut relay) => match event {
                    Some(relay::Event::Connected(_)) => {
                        self.view.relay_connected = true;
                        if let Some(publisher) = &mut publisher {
                            publisher.request(self.view.clone(), clock(), Refresh::All);
                        }
                    }
                    Some(event @ (relay::Event::Mail { .. } | relay::Event::Unreachable(_))) => {
                        let now = clock();
                        if let (relay::Event::Mail { key, envelope, .. }, Some(devices), Some(relay)) = (&event, &devices, &relay) {
                            match devices.legacy_mail(*key, &envelope.0, now).await {
                                Ok(Some(members)) => {
                                    Arc::make_mut(&mut self.view.roster).members = members;
                                    let hosts = self.view.roster.members.iter().filter_map(|member| member.host()).collect();
                                    let _ = relay.sender.trust(grants.clone(), hosts).await;
                                    if let Some(publisher) = &mut publisher {
                                        publisher.request(self.view.clone(), now, Refresh::All);
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => eprintln!("Keeping released Mac roster: {error}"),
                            }
                        }
                        match tokio::task::spawn_blocking(move || {
                            let result = self.receive(event, now);
                            (self, result)
                        }).await {
                            Ok((resident, result)) => {
                                self = resident;
                                match result {
                                    Ok(received) => {
                                        if let (Some(key), Some(publisher)) = (received.refresh, &mut publisher) {
                                            publisher.request(self.view.clone(), now, Refresh::Peer { key, ask: received.ask });
                                        }
                                        if let (Some((host, reply)), Some(relay)) = (received.reply, &relay) {
                                            let _ = relay.sender.mail(host, reply).await;
                                        }
                                    },
                                    Err(error) => eprintln!("Keeping peer state: {error}"),
                                }
                            }
                            Err(error) => { eprintln!("Keeping peer state: {error}"); return; }
                        }
                    }
                    Some(relay::Event::Session(event)) => {
                        if let Some(relay) = &relay {
                            paired.event(event, &relay.sender, dispatch.as_ref(), devices.as_ref());
                        }
                        continue;
                    }
                    Some(relay::Event::Disconnected(_)) => self.view.relay_connected = false,
                    Some(_) => continue,
                    None => {
                        self.view.relay_connected = false;
                        relay = None;
                        publisher = None;
                    }
                },
                _ = round.tick() => {
                    let now = clock();
                    if let Err(error) = self.refresh_move_ownership().await {
                        eprintln!("Refreshing move ownership: {error}");
                    }
                    if let Err(error) = Arc::make_mut(&mut self.view.move_notices).expire(&self.run, now) {
                        eprintln!("Keeping moves on other computers: {error}");
                    }
                    if let Some(publisher) = &mut publisher {
                        publisher.request(self.view.clone(), now, Refresh::None);
                    }
                },
            }
            publish.send_replace(Arc::new(self.view.clone()));
        }
    }

    /// A move can activate after OpenCode already published the imported row.
    /// Use the durable ownership change to publish it even if that row stays the same.
    async fn refresh_move_ownership(&mut self) -> io::Result<()> {
        let run = self.run.clone();
        let last = self.move_files.clone();
        let refreshed = tokio::task::spawn_blocking(move || {
            let stamp = file_stamp(&run.join("moves"))?;
            if stamp == last {
                return Ok(None);
            }
            let mut ownership = Ownership::default();
            crate::moves::locate_checked(&run, &mut ownership)?;
            Ok::<_, io::Error>(Some((stamp, ownership)))
        })
        .await??;
        if let Some((stamp, ownership)) = refreshed {
            self.move_files = stamp;
            let local = Arc::make_mut(&mut self.view.local);
            local.ownership.frozen = ownership.frozen;
            local.ownership.journal = ownership.journal;
            self.view.local_lists = local.publication().map(Arc::new);
        }
        Ok(())
    }

    fn receive(&mut self, event: relay::Event, now: Millis) -> io::Result<Received> {
        match event {
            relay::Event::Unreachable(address) => {
                if let Some(peer) = self.view.roster.at_address(address) {
                    Arc::make_mut(&mut self.view.reach).unreachable(
                        &self.run,
                        &encode(&peer.key),
                        now,
                    )?;
                }
            }
            relay::Event::Mail {
                key,
                envelope: Envelope(mut payload),
                ..
            } => {
                let Some(peer) = self.view.roster.member(&key) else {
                    return Ok(Received::default());
                };
                // Conversation stream parts and in-memory queue reads do
                // not change the durable reach projection.
                if payload
                    .get("sessionFollow")
                    .and_then(|value| value.get("part"))
                    .is_some_and(Value::is_object)
                    || payload
                        .get("workQueue")
                        .and_then(|value| value.get("memory"))
                        .and_then(Value::as_bool)
                        == Some(true)
                {
                    return Ok(Received::default());
                }
                let id = encode(&key);
                let back = Arc::make_mut(&mut self.view.reach).heard(&self.run, &id)?;
                let before = self.view.announced.get(&id);
                let was_quiet = before
                    .and_then(|entry| entry.heard)
                    .is_some_and(|heard| self.view.reach.peer_is_away(&id, Some(heard), now));
                let announced = payload
                    .remove("agents")
                    .and_then(|value| Announcement::received(value, peer, now))
                    .filter(|entry| entry.at > before.map_or(0, |entry| entry.at));
                if let Some(entry) = announced.or_else(|| {
                    let before = before?;
                    let heard = before.heard?;
                    if now.saturating_sub(heard) < HEARD_EVERY {
                        return None;
                    }
                    Some(Announcement {
                        heard: Some(now),
                        ..before.clone()
                    })
                }) {
                    let mut all = (*self.view.announced).clone();
                    all.insert(id.clone(), entry);
                    kept::write(&self.run.join("peer-agents.json"), &all)?;
                    self.view.announced = Arc::new(all);
                }
                take_copy(
                    &self.run,
                    key,
                    LIST_FILE,
                    payload.remove("setupLists"),
                    now,
                    &mut self.list_parts,
                    &mut self.view.lists,
                )?;
                take_copy(
                    &self.run,
                    key,
                    NOTICE_FILE,
                    payload.remove("noticeCopy"),
                    now,
                    &mut self.notice_parts,
                    &mut self.view.notice_copies,
                )?;
                if let Some(value) = payload.remove("activity") {
                    activity::take(
                        &self.run,
                        key,
                        now,
                        value,
                        &mut self.view.activities,
                        &mut self.activity_parts,
                    )?;
                }
                if let Some(moving) = payload.remove("moving") {
                    Arc::make_mut(&mut self.view.move_notices).take(
                        &self.run,
                        &self.view.roster,
                        key,
                        moving,
                        now,
                    )?;
                }
                let ask = payload.get("ask").and_then(Value::as_str) == Some("agents");
                let reply = if !self.view.transfers
                    && let (Some(movement), Some(host)) = (payload.get("move"), peer.host())
                {
                    let reply = if movement["op"].as_str() == Some("part") {
                        self.move_parts.take(&self.run, &id, movement, now)?
                    } else {
                        crate::moves::receipt::replay(&self.run, &id, movement, now)
                            .map_err(io::Error::other)?
                    };
                    reply.map(|reply| (host, reply))
                } else {
                    None
                };
                return Ok(Received {
                    refresh: (back || was_quiet || ask).then_some(key),
                    ask,
                    reply,
                });
            }
            _ => {}
        }
        Ok(Received::default())
    }
}

async fn wait_for_trust(until: Option<tokio::time::Instant>) {
    if let Some(until) = until {
        tokio::time::sleep_until(until).await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn grants_changed(devices: &mut Option<Devices>) {
    if let Some(device) = devices
        && device.changed().await.is_ok()
    {
        return;
    }
    std::future::pending().await
}

async fn setup_roster_changed(
    rosters: &mut Option<watch::Receiver<Option<Arc<Roster>>>>,
) -> Option<Arc<Roster>> {
    let Some(receiver) = rosters.as_mut() else {
        return std::future::pending().await;
    };
    if receiver.changed().await.is_err() {
        *rosters = None;
        return None;
    }
    receiver.borrow_and_update().clone()
}

async fn relay_event(relay: &mut Option<Relay>) -> Option<relay::Event> {
    if let Some(relay) = relay {
        relay.events.recv().await
    } else {
        std::future::pending().await
    }
}

fn copy_path(run: &Path, key: &ComputerKey, file: &str) -> PathBuf {
    run.join("peers").join(encode(key)).join(file)
}

fn read_copies<T: DeserializeOwned>(
    run: &Path,
    roster: &Roster,
    file: &str,
) -> BTreeMap<ComputerKey, Arc<Copy<T>>> {
    roster
        .members
        .iter()
        .filter_map(|peer| {
            Some((
                peer.key,
                Arc::new(kept::read(&copy_path(run, &peer.key, file))?),
            ))
        })
        .collect()
}

#[derive(Deserialize)]
struct Told<T> {
    at: Millis,
    #[serde(flatten)]
    value: T,
}

fn take_copy<T: DeserializeOwned + Serialize>(
    run: &Path,
    key: ComputerKey,
    file: &str,
    value: Option<Value>,
    seen: Millis,
    pending: &mut BTreeMap<ComputerKey, Parts>,
    copies: &mut BTreeMap<ComputerKey, Arc<Copy<T>>>,
) -> io::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let told: Option<Told<T>> = if let Some(part) = value.get("part") {
        serde_json::from_value::<Part>(part.clone())
            .ok()
            .and_then(|part| pending.entry(key).or_default().add(part))
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    } else {
        serde_json::from_value(value).ok()
    };
    let Some(told) = told else {
        return Ok(());
    };
    if copies.get(&key).is_some_and(|before| before.at >= told.at) {
        return Ok(());
    }
    let copy = Copy {
        at: told.at,
        seen,
        value: told.value,
    };
    kept::write(&copy_path(run, &key, file), &copy)?;
    copies.insert(key, Arc::new(copy));
    Ok(())
}

#[derive(Default)]
struct Received {
    refresh: Option<ComputerKey>,
    ask: bool,
    reply: Option<(Host, Envelope)>,
}

enum Refresh {
    None,
    All,
    Peer { key: ComputerKey, ask: bool },
}

struct Publisher {
    tx: mpsc::Sender<(View, Millis, Refresh)>,
    task: JoinHandle<()>,
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Publisher {
    fn start(relay: relay::Sender, run: PathBuf, sent_agents: watch::Sender<AgentMail>) -> Self {
        let (tx, mut rx) = mpsc::channel::<(View, Millis, Refresh)>(16);
        let task = tokio::spawn(async move {
            let mut previous = BTreeMap::new();
            let mut announcements = BTreeMap::new();
            let mut moves = crate::moves::Announcer::default();
            while let Some((view, now, refresh)) = rx.recv().await {
                match refresh {
                    Refresh::None => {
                        moves
                            .announce(&run, &view.roster, &view.move_waiting, &relay, now)
                            .await
                    }
                    Refresh::Peer { ask: true, .. } => moves.repeat(),
                    _ => {}
                }
                publish_round(
                    &relay,
                    view,
                    now,
                    refresh,
                    &mut previous,
                    &mut announcements,
                    &sent_agents,
                )
                .await;
            }
        });
        Self { tx, task }
    }

    fn request(&mut self, view: View, now: Millis, refresh: Refresh) {
        let _ = self.tx.try_send((view, now, refresh));
    }
}

async fn publish_round(
    relay: &relay::Sender,
    view: View,
    now: Millis,
    refresh: Refresh,
    previous: &mut BTreeMap<(ComputerKey, &'static str), ([u8; 32], Millis)>,
    announcements: &mut BTreeMap<ComputerKey, ([u8; 32], Millis)>,
    sent_agents: &watch::Sender<AgentMail>,
) {
    let lists = &view.local_lists;
    let mut older_lists = None;
    let activity = activity::local(&view.local);
    let activity_digest: [u8; 32] =
        Sha256::digest(serde_json::to_vec(&activity).expect("activity serializes")).into();
    let notice = serde_json::to_value(view.notices(now)).expect("notice history serializes");
    let owns_here = |record: &(RecordScope, String)| {
        !view.local.ownership.frozen.contains(&record.1)
            && view
                .local
                .ownership
                .journal
                .get(record)
                .is_none_or(|owner| view.roster.here == Some(owner.key))
    };
    let mut owned: Vec<_> = view
        .local
        .ownership
        .held
        .iter()
        .filter(|record| owns_here(record))
        .map(|(_, id)| id.as_str())
        .collect();
    if let Some(lists) = lists {
        let mut sessions: Vec<_> = lists
            .sessions
            .iter()
            .filter(|row| {
                row.facts.parent.is_none()
                    && !row.actions.is_empty()
                    && owns_here(&(RecordScope::Session, row.id.clone()))
            })
            .collect();
        sessions.sort_by(|a, b| {
            b.facts
                .updated
                .as_f64()
                .unwrap_or_default()
                .total_cmp(&a.facts.updated.as_f64().unwrap_or_default())
        });
        owned.extend(sessions.into_iter().map(|row| row.id.as_str()));
    }
    let owns: String = owned.into_iter().take(5000).map(owned_digest).collect();
    let features = view.features();
    let agents = view.announced_agents();
    let sessions = Arc::new(
        agents
            .iter()
            .map(|agent| agent.session.clone())
            .collect::<BTreeSet<_>>(),
    );
    let mut active = view.at_work();
    let working = active.len();
    active.truncate(20);
    let mut asking = view
        .local
        .list
        .as_ref()
        .map_or_else(Vec::new, lists::LocalList::asking);
    let waiting = asking.len();
    asking.truncate(10);
    let mut state = json!({"list": agents, "owns": owns, "features": features, "computer": {
        "working":working, "atWork":active, "waiting":waiting, "asking":asking,
    }});
    if let Some(system) = view.system.as_deref() {
        state["computer"]["system"] = json!(system);
    }
    if let Some(engine) = view
        .endpoint
        .as_ref()
        .and_then(|endpoint| endpoint.hello.engine.as_ref())
    {
        state["computer"]["engine"] = json!(engine);
    }
    let offer = |update: &UpdateFacts, normal: bool| {
        // A legacy recipient must get only the bridge's signed facts. Do not
        // expose a selected Rust version while the first update facts are pending.
        let metadata = normal && (view.bridge.is_none() || update.kuvo.is_some());
        let mut state = state.clone();
        if let (Some(computer), Ok(Value::Object(update))) = (
            state["computer"].as_object_mut(),
            serde_json::to_value(update),
        ) {
            computer.extend(update);
            if metadata {
                if let Some(version) = &view.versions.version {
                    computer.insert("version".into(), json!(version));
                }
                if !computer.contains_key("kuvo")
                    && let Some(running) = &view.versions.running
                {
                    computer.insert("kuvo".into(), json!(running));
                }
            }
        }
        let digest: [u8; 32] = Sha256::digest(state.to_string()).into();
        (state, digest)
    };
    let current = offer(&view.update.facts, true);
    let bridge = view
        .update
        .bridge
        .as_ref()
        .map(|bridge| (bridge, offer(&bridge.facts, false)));
    for peer in &view.roster.members {
        let Some(host) = peer.host() else {
            continue;
        };
        let forced = matches!(refresh, Refresh::All)
            || matches!(refresh, Refresh::Peer { key, .. } if key == peer.key);
        let known = view.announced.get(&encode(&peer.key));
        let (state, announcement_digest) = bridge
            .as_ref()
            .filter(|(bridge, _)| bridge.recipients.contains(&peer.key))
            .map_or(&current, |(_, offer)| offer);
        let before = announcements.get(&peer.key);
        if forced
            || before.is_none_or(|(digest, at)| {
                digest != announcement_digest || now.saturating_sub(*at) >= 600_000
            })
        {
            let at = before.map_or(now, |(_, before)| now.max(before.saturating_add(1)));
            let mut fields = json!({"at": now, "agents": state});
            fields["agents"]["at"] = json!(at);
            if matches!(refresh, Refresh::All)
                || (known.is_none() && matches!(refresh, Refresh::None))
            {
                fields["ask"] = json!("agents");
            }
            if relay.mail(host.clone(), envelope(fields)).await.is_ok() {
                announcements.insert(peer.key, (*announcement_digest, at));
            }
        }
        if announcements
            .get(&peer.key)
            .is_some_and(|(digest, _)| digest == announcement_digest)
        {
            let sent = SentAgents {
                sessions: sessions.clone(),
                complete: lists.is_some(),
            };
            sent_agents.send_if_modified(|known| {
                if known.get(&peer.key) == Some(&sent) {
                    return false;
                }
                known.insert(peer.key, sent);
                true
            });
        }
        let activity_key = (peer.key, "activity");
        let before = previous.get(&activity_key);
        if forced
            || before.is_none_or(|(hash, at)| {
                *hash != activity_digest || now.saturating_sub(*at) >= 60_000
            })
        {
            let at = before.map_or(now, |(_, at)| now.max(at.saturating_add(1)));
            let mut sent = false;
            for part in activity::split(&activity, at) {
                sent = relay
                    .mail(host.clone(), envelope(json!({"at":now,"activity":part})))
                    .await
                    .is_ok();
                if !sent {
                    break;
                }
            }
            if sent {
                previous.insert(activity_key, (activity_digest, at));
            }
        }
        let Some(known) = known else {
            continue;
        };
        if known.supports(lists::FEATURE)
            && let Some(lists) = lists
        {
            let content = if known.supports(lists::BOT_FEATURE) {
                serde_json::to_value(lists.as_ref()).expect("lists serialize")
            } else {
                older_lists
                    .get_or_insert_with(|| {
                        let mut copy =
                            serde_json::to_value(lists.as_ref()).expect("lists serialize");
                        copy.as_object_mut().expect("lists object").remove("bots");
                        copy
                    })
                    .clone()
            };
            publish_copy(
                relay,
                &host,
                Publication {
                    field: "setupLists",
                    content,
                    now,
                    forced,
                    spacing: 15_000,
                },
                previous,
            )
            .await;
        }
        if known.supports(notices::FEATURE) {
            publish_copy(
                relay,
                &host,
                Publication {
                    field: "noticeCopy",
                    content: notice.clone(),
                    now,
                    forced,
                    spacing: 0,
                },
                previous,
            )
            .await;
        }
    }
}

struct Publication {
    field: &'static str,
    content: Value,
    now: Millis,
    forced: bool,
    spacing: Millis,
}
type Sent = BTreeMap<(ComputerKey, &'static str), ([u8; 32], Millis)>;

async fn publish_copy(relay: &relay::Sender, host: &Host, item: Publication, previous: &mut Sent) {
    let Publication {
        field,
        mut content,
        now,
        forced,
        spacing,
    } = item;
    let key = (host.key, field);
    let digest: [u8; 32] = Sha256::digest(content.to_string()).into();
    if let Some((old, at)) = previous.get(&key) {
        let age = now.saturating_sub(*at);
        if !forced && (age < spacing || (age < 60_000 && old == &digest)) {
            return;
        }
    }
    let at = previous
        .get(&key)
        .map_or(now, |(_, at)| now.max(at.saturating_add(1)));
    content["at"] = json!(at);
    let single = envelope(json!({"at":now,field:&content}));
    if single.encode().len() <= MAX_PLAINTEXT {
        if relay.mail(host.clone(), single).await.is_err() {
            return;
        }
    } else {
        let Some(parts) = parts::split(content.to_string().as_bytes()) else {
            return;
        };
        for part in parts.iter() {
            if relay
                .mail(
                    host.clone(),
                    envelope(json!({"at":now,field:{"part":part}})),
                )
                .await
                .is_err()
            {
                return;
            }
        }
    }
    previous.insert(key, (digest, at));
}

fn envelope(value: Value) -> Envelope {
    Envelope(value.as_object().expect("mail object").clone())
}
