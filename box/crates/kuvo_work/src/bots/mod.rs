//! This box's bots (PROTOCOL.md, "Per area"): listed with what each is doing now, and followed.

mod edit;
pub mod here;
pub mod job;
pub mod kit;
pub mod list;
mod make;
pub mod now;
pub mod own_keys;
pub(crate) mod reach;
mod routes;
mod seen;
pub mod setup;
pub(crate) mod tell;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use kuvo_engine::{Engine, LocalSessions};
use kuvo_peers::setup_lists::Reader;
use kuvo_server::Registry;
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use crate::{approval::Approvals, wakes::Wakes};
use kit::Kit;
use list::{FolderGoes, Place};
use now::{Rest, Told};
use setup::{Setup, ZoneFiles};

/// What these routes answer whole (held edits and the bots elsewhere come with a later part).
pub const FEATURES: &[&str] = &["bots.make", "bots.state"];

/// What's said while a bot runs with the coding agents, whose bridge isn't in this box yet.
pub const AGENTS_DOWN: &str = "The coding agents are restarting. This takes a few seconds.";

/// This computer's bots.
#[derive(Clone)]
pub struct Bots {
    inner: Arc<Inner>,
}

struct Inner {
    home: PathBuf,
    is_mac: bool,
    bots: PathBuf,
    run: PathBuf,
    engine: Engine,
    setup: Arc<dyn Setup>,
    approvals: Approvals,
    conversations: crate::approval::ApprovalService,
    requests: make::SharedRequests,
    telling: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    wakes: Wakes,
    peers: Reader,
    sessions: LocalSessions,
    machine: Option<String>,
    computer: String,
    goes: FolderGoes,
    told: Mutex<Told>,
    changes: watch::Sender<u64>,
}

/// Where the bots live and how this computer is named.
pub struct Setting {
    pub home: PathBuf,
    /// Your own folder (`~`), as a bot's trash folder is written for you.
    pub account: PathBuf,
    pub name: String,
    pub is_mac: bool,
}

impl Bots {
    pub fn new(
        setting: Setting,
        wakes: Wakes,
        peers: Reader,
        sessions: LocalSessions,
        engine: Engine,
        setup: Arc<dyn Setup>,
    ) -> Self {
        let Setting {
            home,
            account,
            name,
            is_mac,
        } = setting;
        let here = if is_mac { "this Mac" } else { "this machine" };
        let computer = if name.is_empty() {
            here.to_owned()
        } else {
            name.clone()
        };
        Self {
            inner: Arc::new(Inner {
                is_mac,
                engine: engine.clone(),
                setup,
                approvals: Approvals::new(&home, is_mac),
                conversations: crate::approval::ApprovalService::new(
                    Approvals::new(&home, is_mac),
                    engine.clone(),
                    home.join("bots"),
                    sessions.clone(),
                ),
                requests: Arc::default(),
                telling: Mutex::default(),
                bots: home.join("bots"),
                run: home.join("run"),
                goes: FolderGoes::of(&home, &account),
                machine: (!is_mac && !name.is_empty()).then_some(name),
                computer,
                wakes,
                peers,
                sessions,
                told: Mutex::new(Told::default()),
                changes: watch::channel(0).0,
                home,
            }),
        }
    }

    pub fn register(&self, routes: &mut Registry) {
        routes::register(self.clone(), routes);
    }

    /// A change to the bots made here, as it's made.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    pub fn changed(&self) {
        self.inner.changes.send_modify(|count| *count += 1);
    }

    /// This computer, as what the box says names it.
    fn here(&self) -> &'static str {
        if self.inner.is_mac {
            "this Mac"
        } else {
            "this machine"
        }
    }

    /// What this box makes bots with now: its words, with your zone and its own facts.
    fn kit(&self) -> Option<Kit> {
        let zone = self.inner.setup.time_zone();
        let time_zone = zone
            .as_ref()
            .map_or_else(setup::own_clock, |(zone, _)| zone.clone());
        let machine = self.inner.setup.link_id().unwrap_or_else(|| {
            hostname::get()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        Kit::read(
            &self.inner.home,
            &kit::Place {
                is_mac: self.inner.is_mac,
                zone_yours: zone.is_some(),
                machine: &machine,
                time_zone: &time_zone,
                system: kit::system_here(),
            },
        )
    }

    /// The zone files a bot's folder has here.
    fn zone_files(&self) -> ZoneFiles {
        ZoneFiles::of(self.inner.setup.time_zone(), self.inner.is_mac)
    }

    /// One telling of a bot at a time, by its conversation.
    async fn one_at_a_time(&self, session: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let turn = {
            let mut telling = self.inner.telling.lock().expect("bot turns");
            telling.retain(|_, turn| Arc::strong_count(turn) > 1);
            telling.entry(session.to_owned()).or_default().clone()
        };
        turn.lock_owned().await
    }

    /// A bot here as `kuvo/bots` lists it, without what it's doing now; none should it not list.
    fn listed_one(&self, id: &str) -> Option<Value> {
        let here = here::BotHere::read(&self.inner.bots, id)?;
        Some(Value::Object(list::listed(&self.place(), here).value))
    }

    /// As the engine starts: every bot on it told what it is now and its rules put right, and
    /// what deleted bots left behind gone.
    pub fn follow_engine(&self) -> tokio::task::JoinHandle<()> {
        let bots = self.clone();
        tokio::spawn(async move {
            let mut states = bots.inner.engine.states();
            let mut last = None;
            loop {
                let started = match &*states.borrow_and_update() {
                    kuvo_engine::State::Running { started, .. } => Some(*started),
                    _ => None,
                };
                if let Some(started) = started
                    && last != Some(started)
                {
                    last = Some(started);
                    bots.finish_deletes().await;
                    bots.freshen_all().await;
                }
                if states.changed().await.is_err() {
                    return;
                }
            }
        })
    }

    fn place(&self) -> Place<'_> {
        Place {
            bots: &self.inner.bots,
            machine: self.inner.machine.as_deref(),
            computer: &self.inner.computer,
            goes: &self.inner.goes,
        }
    }

    /// When you last saw each bot here (ms), from `run/bots-seen/seen.json`.
    fn seen(&self) -> HashMap<String, i64> {
        let kept: Value = kuvo_store::files::text_of(&self.inner.run.join("bots-seen/seen.json"))
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        kept["bots"]
            .as_object()
            .map(|bots| {
                bots.iter()
                    .filter(|(id, _)| here::bot_id(id))
                    .filter_map(|(id, at)| {
                        Some((id.clone(), at.as_f64().filter(|at| at.is_finite())? as i64))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The bots here, each with what it's doing now (`now`), and when the first may change state
    /// with nothing else changing.
    pub fn with_now(&self) -> (Vec<Map<String, Value>>, Option<i64>) {
        let at = jiff::Timestamp::now().as_millisecond();
        let arriving = list::arriving(&self.inner.run);
        let listed: Vec<_> = list::list(&self.place())
            .into_iter()
            .filter(|bot| !arriving.contains(&bot.here.id))
            .collect();
        let record = self.inner.sessions.latest_record().unwrap_or_default();
        let empty = Map::new();
        let mut counts = now::counts(
            record["sessions"].as_object().unwrap_or(&empty),
            &self.seen(),
        );
        let view = self.inner.peers.view();
        let mut told = self.inner.told.lock().expect("bot states");
        told.keep_only(
            &listed
                .iter()
                .map(|bot| bot.here.id.clone())
                .collect::<Vec<_>>(),
        );
        let mut turns: Option<i64> = None;
        let bots = listed
            .into_iter()
            .map(|bot| {
                let activity = view.activity(
                    Some(&bot.here.session),
                    Some(&bot.here.id),
                    false,
                    self.inner.peers.now(),
                );
                let mut count = counts.remove(&bot.here.id).unwrap_or_default();
                count.working = activity.working as u64;
                count.asking = activity.asking as u64;
                if count.working > 0 || count.asking > 0 {
                    count.finished = None;
                }
                let rest = Rest {
                    paused: bot.paused,
                    wakes: bot.wakes,
                };
                if let Some(turn) = now::turns(rest, at) {
                    turns = Some(turns.map_or(turn, |soonest| soonest.min(turn)));
                }
                let mut value = bot.value;
                value.insert(
                    "now".into(),
                    json!(told.now_of(&bot.here.id, count, rest, at)),
                );
                value
            })
            .collect();
        (bots, turns)
    }

    /// What `kuvo/bots` says.
    pub fn said(&self) -> (Value, Option<i64>) {
        let (bots, turns) = self.with_now();
        let pending = self.inner.peers.view().pending_bots();
        (json!({ "pendingComputers": pending, "bots": bots }), turns)
    }

    /// What this computer tells your other computers its bots are doing (`bots` in its lists).
    pub fn peer_activity(&self) -> BTreeMap<String, Value> {
        self.with_now()
            .0
            .into_iter()
            .filter_map(|bot| {
                let id = bot.get("id")?.as_str()?.to_owned();
                let mut activity = Map::new();
                for field in ["session", "now", "paused", "wakes"] {
                    if let Some(value) = bot.get(field) {
                        activity.insert(field.into(), value.clone());
                    }
                }
                Some((id, Value::Object(activity)))
            })
            .collect()
    }

    /// This computer's wake-ups.
    pub(crate) fn wakes(&self) -> &Wakes {
        &self.inner.wakes
    }

    /// This computer's settings, as its integrations are kept there.
    pub(crate) fn config(&self) -> kuvo_services::config::ConfigPaths {
        kuvo_services::config::ConfigPaths::for_home(&self.inner.home, self.inner.is_mac)
    }

    pub fn bots_folder(&self) -> &Path {
        &self.inner.bots
    }

    pub fn engine(&self) -> &Engine {
        &self.inner.engine
    }
}
