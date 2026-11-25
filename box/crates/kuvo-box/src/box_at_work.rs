//! What's at work here and on the other enrolled computers, for the Helper menu.

use kuvo_os::timer_fires::{self, Timer};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use kuvo_engine::{Engine, LocalSessions};
use kuvo_peers::{lists::LocalList, setup_lists::Reader};
use kuvo_protocol::routes::{
    get_kuvo_box_at_work as read, get_kuvo_box_at_work_events as events,
    types::{
        GetKuvoBotsStatus200ElsewhereItemKind3 as Kind,
        GetKuvoBoxAtWorkStatus200ComputersItem as Computer,
        GetKuvoBoxAtWorkStatus200ComputersItemSessionsItem as Session,
        GetKuvoBoxAtWorkStatus200Waiting as Waiting,
        GetKuvoComputersStatus200ComputersItemReach4 as Reach,
    },
};
use kuvo_server::{Access, JsonLine, Registry, RouteReply};
use kuvo_services::limits_menu::MenuLimits;
use kuvo_store::{
    background,
    files::{FileStamp, file_stamp},
};
use kuvo_work::{bots::here::BotHere, wakes::folder::names, work::watch::told};
use tokio_util::sync::CancellationToken;

const MOST_NAMED: usize = 20;
const WATCHED: &[&str] = &[
    "work/background.json",
    "peer-agents.json",
    "peer-reach.json",
    "reach-again.json",
    "work-queue/queues",
    "mac-link.json",
    #[cfg(feature = "agent-runtime-fixture")]
    "plan-limit-rows-fixture.json",
];

struct Menu {
    home: PathBuf,
    applies: bool,
    is_mac: bool,
    engine: Engine,
    peers: Reader,
    limits: MenuLimits,
}

fn title(text: &str) -> Option<String> {
    let text = text
        .split(|c: char| c == '\u{feff}' || c.is_whitespace() && c != '\u{85}')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut units = 0;
    let shortened = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 200
        })
        .collect::<String>();
    (!shortened.is_empty()).then_some(shortened)
}

impl Menu {
    fn stamps(&self) -> Vec<Option<FileStamp>> {
        let run = self.home.join("run");
        WATCHED
            .iter()
            .map(|name| file_stamp(&run.join(name)).ok().flatten())
            .collect()
    }

    async fn read(
        &self,
        sessions: &mut LocalSessions,
        cancel: &CancellationToken,
    ) -> read::Status200 {
        let (record, waiting) = self.engine.waiting_snapshot(sessions, cancel).await;
        let list = record
            .and_then(|record| serde_json::from_value::<LocalList>(record).ok())
            .unwrap_or_default();
        let working = list.working();
        let working_ids: HashSet<_> = working.iter().map(|(id, _)| id.as_str()).collect();
        let working_count = working.len();
        let run = self.home.join("run");
        let background = background::active_sessions(&run, told::now())
            .iter()
            .filter(|id| !working_ids.contains(id.as_str()))
            .count();
        let folder = self.home.join("bots");
        let bots: Vec<_> = names(&folder)
            .into_iter()
            .filter_map(|id| BotHere::read(&folder, &id))
            .collect();
        let sessions: Vec<_> = working
            .into_iter()
            .take(MOST_NAMED)
            .map(|(id, facts)| Session {
                id: id.clone(),
                title: facts.title.as_deref().and_then(title),
                bot: bots
                    .iter()
                    .find(|bot| bot.session == *id)
                    .or_else(|| {
                        bots.iter()
                            .find(|bot| facts.bot.as_deref() == Some(bot.id.as_str()))
                    })
                    .map(|bot| bot.name())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
            })
            .collect();
        let mut asking = list.asking();
        let asking_count = asking.len();
        asking.truncate(MOST_NAMED);
        let view = self.peers.view();
        let key = view
            .roster
            .here
            .as_ref()
            .map(|key| kuvo_wire::keys::encode(key))
            .unwrap_or_default();
        #[cfg(feature = "agent-runtime-fixture")]
        if let Some(rows) = kuvo_store::files::text_of(&run.join("plan-limit-rows-fixture.json"))
            .and_then(|text| serde_json::from_str::<Vec<serde_json::Value>>(&text).ok())
        {
            self.limits.publish(&rows);
        }
        let rings = self.limits.rings();
        let name = if self.is_mac {
            kuvo_services::mac_app::legacy_name(&self.home)
                .map(|name| kuvo_peers::calls::plain(&name, usize::MAX))
                .filter(|name| !name.is_empty())
        } else {
            None
        }
        .unwrap_or_else(|| view.roster.name.clone());
        let mut computers = vec![Computer {
            key,
            name,
            kind: if self.is_mac {
                Kind::Mac
            } else {
                Kind::Machine
            },
            here: Some(Default::default()),
            reach: Reach::Connected,
            working: Some(working_count.into()),
            waiting: Some(asking_count.into()),
            sessions: sessions.clone(),
            asking: asking.clone(),
            rings: vec![],
        }];
        computers.extend(view.helper_peers(&run, self.peers.now()));
        for computer in &mut computers {
            computer.rings = rings
                .iter()
                .filter(|row| row.on(&computer.key, computer.here.is_some()))
                .map(|row| row.rings.clone())
                .collect();
        }
        read::Status200 {
            applies: self.applies,
            working: working_count.into(),
            sessions,
            asking,
            waiting: Waiting {
                sessions: waiting.sessions,
                bots: waiting.bots,
                queue: waiting.queue,
            },
            background: background.into(),
            busy: working_count != 0 || background != 0,
            rings: rings.iter().map(|row| row.rings.clone()).collect(),
            computers: Some(computers),
        }
    }
}

pub fn applies(home: &Path, is_mac: bool) -> bool {
    cfg!(target_os = "macos") && kuvo_services::config::is_own_home_mac(home, is_mac)
}

pub fn register(
    routes: &mut Registry,
    home: &Path,
    is_mac: bool,
    engine: Engine,
    sessions: LocalSessions,
    peers: Reader,
    limits: MenuLimits,
) {
    let menu = Arc::new(Menu {
        home: home.to_owned(),
        applies: applies(home, is_mac),
        is_mac: is_mac
            || kuvo_services::config::box_env(home, "KUVO_BOX").as_deref() == Some("mac"),
        engine,
        peers,
        limits,
    });
    let source = sessions.clone();
    let here = menu.clone();
    routes.register::<read::Spec, _, _>(Access::Any, move |call| {
        let menu = here.clone();
        let mut list = source.clone();
        async move {
            Ok(RouteReply::json::<200>(
                menu.read(&mut list, &call.cancel).await,
            ))
        }
    });
    routes.register::<events::Spec, _, _>(Access::Any, move |call| {
        let menu = menu.clone();
        let mut list = sessions.clone();
        let mut peers = menu.peers.clone();
        let mut limits = menu.limits.subscribe();
        async move {
            menu.limits.requested();
            Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                let mut last = None;
                let mut refresh = timer_fires::interval(Timer::WorkAtWorkFollow, Duration::from_secs(1));
                refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut stamps = menu.stamps();
                let mut read_at = tokio::time::Instant::now();
                let mut due = true;
                loop {
                    if due {
                        let current = menu.read(&mut list, &call.cancel).await;
                        read_at = tokio::time::Instant::now();
                        if last.as_ref() != Some(&current) {
                            yield JsonLine::new(current.clone())?;
                            last = Some(current);
                        }
                    }
                    due = tokio::select! {
                        changed = list.changed() => { if !changed { break; } true },
                        changed = peers.changed() => { if changed.is_err() { break; } true },
                        changed = limits.changed() => { if changed.is_err() { break; } true },
                        () = call.cancel.cancelled() => break,
                        _ = refresh.tick() => {
                            let next = menu.stamps();
                            let changed = stamps != next;
                            stamps = next;
                            changed || read_at.elapsed() >= Duration::from_secs(15)
                        },
                    };
                }
            }))
        }
    });
}
