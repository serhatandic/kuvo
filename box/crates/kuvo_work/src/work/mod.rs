//! A bot's work and what's asked before it (PROTOCOL.md, "Work"): where new projects go here
//! (`kuvo/work/settings`), this Mac kept awake while agents work, what bots just did to their
//! sessions (`kuvo/work/oversight`) and when each bot's work last changed (`kuvo/work/events`).

pub mod agent_tools;
pub mod answers;
pub mod asks;
pub mod bot;
pub mod elsewhere;
pub mod policy;
pub mod projects;
pub mod records;
pub mod sent;
pub mod settings;
pub mod starts;
pub mod unanswering;
pub mod watch;

use kuvo_os::timer_fires::{self, Timer};
use std::{collections::HashMap, path::PathBuf, time::Duration};

use http::StatusCode;
use kuvo_engine::LocalSessions;
use kuvo_peers::setup_lists::Reader;
use kuvo_protocol::routes::{
    get_kuvo_work as work_get, get_kuvo_work_asks as asks_get,
    get_kuvo_work_asks_events as asks_events, get_kuvo_work_events as changed_events,
    get_kuvo_work_oversight as oversight_get, get_kuvo_work_oversight_events as oversight_events,
    get_kuvo_work_settings as settings_get, get_kuvo_work_unanswering as unanswering_get,
    patch_kuvo_work_settings as settings_patch, post_kuvo_bots_id_stop_all as stop_all,
    post_kuvo_projects_new as project_new, post_kuvo_work_asks_id as ask_answer,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Value, json};

pub use records::Records;
pub use settings::WorkSettings;
pub use unanswering::Unanswerings;

use crate::bots::here::BotHere;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow { Asks, Oversight, Changes }

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value)
        .map_err(|error| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn refused((status, message): (u16, String)) -> Refusal {
    Refusal::new(
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
        message,
    )
}

/// This box's bots' work.
#[derive(Clone)]
pub struct Work {
    pub settings: WorkSettings,
    pub records: Records,
    pub unanswering: Unanswerings,
    home: PathBuf,
    late_delivery: std::sync::Arc<tokio::sync::Mutex<()>>,
    bots: PathBuf,
    list: crate::bots::Bots,
    sessions: LocalSessions,
    peers: Reader,
    setup: std::sync::Arc<dyn crate::bots::setup::Setup>,
    /// Starters with lines added from outside the watcher, for it to send.
    wakes: tokio::sync::mpsc::UnboundedSender<String>,
    added: std::sync::Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>>,
    /// Calls to the computers this one's leads have sessions on, once the peers run.
    calls: std::sync::Arc<std::sync::OnceLock<kuvo_peers::remote_work::WorkHandle>>,
    /// The Work Queue's link for the sessions its tools start, once the queue runs.
    links: std::sync::Arc<std::sync::OnceLock<crate::queue::StartLink>>,
    /// What this box told you lately (`run/notices/told.json`).
    pub notices: crate::notices::Notices,
    limits: Option<kuvo_services::usage::limits::Service>,
    resets: Option<kuvo_services::usage::resets::Resets>,
    names: kuvo_services::git::ProjectNames,
}

impl Work {
    pub fn new(
        settings: WorkSettings,
        home: &std::path::Path,
        sessions: LocalSessions,
        peers: Reader,
        setup: std::sync::Arc<dyn crate::bots::setup::Setup>,
        bots: crate::bots::Bots,
    ) -> Self {
        let (wakes, added) = tokio::sync::mpsc::unbounded_channel();
        Self {
            wakes,
            added: std::sync::Arc::new(std::sync::Mutex::new(Some(added))),
            calls: std::sync::Arc::default(),
            links: std::sync::Arc::default(),
            notices: crate::notices::Notices::new(home),
            limits: None,
            resets: None,
            names: kuvo_services::git::ProjectNames::default(),
            settings,
            records: Records::new(&home.join("run")),
            unanswering: Unanswerings::new(&home.join("run")),
            home: home.to_owned(),
            late_delivery: std::sync::Arc::default(),
            bots: bots.bots_folder().to_owned(),
            list: bots,
            sessions,
            peers,
            setup,
        }
    }

    pub fn with_limits(mut self, limits: kuvo_services::usage::limits::Service) -> Self {
        self.limits = Some(limits);
        self
    }

    pub fn with_resets(mut self, resets: kuvo_services::usage::resets::Resets) -> Self {
        self.resets = Some(resets);
        self
    }

    /// Follows the engine's events for what they change here: which models can't answer, and what
    /// each conversation hears of the sessions it started (the work watcher).
    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        struct Stops(Option<tokio::task::JoinHandle<()>>);
        impl Drop for Stops {
            fn drop(&mut self) {
                self.0.take().inspect(tokio::task::JoinHandle::abort);
            }
        }
        let added = self.added.lock().expect("added").take();
        let watcher = added.map(|added| watch::Watcher::new(self.clone()).start(added));
        let (work, mut events) = (self.clone(), self.list.engine().events());
        // The coding agents' conversations' turns that ended: news as the engine's are.
        let agents = self.list.engine().agent_events().map(|mut agents| {
            let work = self.clone();
            tokio::spawn(async move {
                loop {
                    match agents.recv().await {
                        Ok(event) => work.news(event.kind, event.data, Value::Null),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => return,
                    }
                }
            })
        });
        tokio::spawn(async move {
            let _watcher = Stops(watcher);
            let _agents = Stops(agents);
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                };
                let engine = work.list.engine().running_bot_engine().await;
                work.unanswering.follow(&event, engine.as_ref()).await;
                work.news(&event.kind, event.data.clone(), event.location.clone());
            }
        })
    }

    /// An event's news told, where it's news (`notices::news`), as it's looked up.
    fn news(&self, kind: &str, data: Value, location: Value) {
        if !crate::notices::news::NEWS.contains(&kind) {
            return;
        }
        let (work, kind) = (self.clone(), kind.to_owned());
        tokio::spawn(async move {
            let engine = work.list.engine().work_engine().await;
            let news = crate::notices::news::News {
                engine: &engine,
                home: &work.home,
                names: &work.names,
            };
            if let Some(notice) = news.notice_for(&kind, &data, &location).await {
                work.notices.tell(notice);
            }
        });
    }

    /// What each bot's work looks like now, to tell when it changed: what's at work under it,
    /// and its sessions as the session list has them.
    fn work_keys(&self) -> Vec<(String, String)> {
        let record = self.sessions.latest_record().unwrap_or_default();
        let view = self.peers.view();
        crate::wakes::folder::names(&self.bots)
            .into_iter()
            .filter_map(|id| BotHere::read(&self.bots, &id))
            .map(|bot| {
                let activity =
                    view.activity(Some(&bot.session), Some(&bot.id), false, self.peers.now());
                let work = record["workByBot"][&bot.id]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let seq = work
                    .iter()
                    .filter_map(|session| record["sessions"][session.as_str()?]["seq"].as_u64())
                    .max()
                    .unwrap_or(0);
                let key = json!([activity, record.get("workByBot").is_some(), work.len(), seq])
                    .to_string();
                (bot.id, key)
            })
            .collect()
    }

    /// Watch only the files and live facts the selected page uses.
    fn followed<F>(
        &self,
        kind: Follow,
        mut read: F,
        cancel: tokio_util::sync::CancellationToken,
    ) -> impl futures_core::Stream<Item = std::io::Result<Value>> + Send + 'static
    where
        F: FnMut() -> Value + Send + 'static,
    {
        let mut sessions = self.sessions.clone();
        let mut peers = self.peers.clone();
        let mut bots = self.list.changes();
        let settings = self.settings.clone();
        let root = self.records.folder().to_owned();
        let hub = kuvo_os::watch::WatchHub::default();
        let mut files = kuvo_os::watch::FileSet::new(hub.clone());
        let mut bot_files = kuvo_os::watch::FileSet::new(hub);
        if kind != Follow::Oversight { bot_files.set_child_files(self.bots.clone(), ["bot.json", "session"]); }
        async_stream::try_stream! {
            let mut previous: Option<Value> = None;
            loop {
                let paths = match kind {
                    Follow::Asks => { let mut paths = settings.follow_paths(); paths.push(root.join("asks.json")); paths },
                    Follow::Oversight => vec![root.join("oversight.json")],
                    Follow::Changes => vec![root.join("changes.json")],
                };
                files.set_files(paths);
                let value = read();
                let now = jiff::Timestamp::now().as_millisecond();
                let due = match kind {
                    Follow::Asks => None,
                    Follow::Oversight => value["marks"].as_array().into_iter().flatten()
                        .filter_map(|mark| mark["at"].as_f64()).map(|at| (at.ceil() as i64).saturating_add(60_000)).min(),
                    Follow::Changes => {
                        let peer_now = peers.now();
                        peers.view().next_activity_change(peer_now).map(|at| now.saturating_add(at.saturating_sub(peer_now)))
                    }
                };
                if previous.as_ref() != Some(&value) {
                    previous = Some(value.clone());
                    yield value;
                }
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = files.changed() => {},
                    _ = bot_files.changed(), if kind != Follow::Oversight => {},
                    changed = bots.changed(), if kind != Follow::Oversight => if changed.is_err() { break; },
                    changed = sessions.changed(), if kind == Follow::Changes => if !changed { break; },
                    changed = peers.changed(), if kind == Follow::Changes => if changed.is_err() { break; },
                    () = async { match due {
                        Some(at) => timer_fires::sleep(Timer::WorkFollow, Duration::from_millis(at.saturating_sub(jiff::Timestamp::now().as_millisecond()).max(1) as u64)).await,
                        None => std::future::pending().await,
                    } } => {},
                }
            }
        }
    }

    /// The Work Queue's link for the sessions the agents' tools start (5.2's hook).
    pub fn attach_start_link(&self, link: crate::queue::StartLink) {
        let _ = self.links.set(link);
    }

    pub fn register(&self, routes: &mut Registry) {
        self.register_tools(routes);
        let stops = std::sync::Arc::new(watch::Stops(self.clone()));
        self.list.engine().attach_work_stops(stops);
        let work = self.clone();
        routes.register::<stop_all::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let bot =
                    percent_encoding::percent_decode_str(call.param("id").unwrap_or_default())
                        .decode_utf8_lossy()
                        .into_owned();
                let stopped = work.stop_bot_work(&bot).await?;
                Ok(RouteReply::json::<200>(typed(
                    json!({ "stopped": stopped }),
                )?))
            }
        });
        let here = self.settings.clone();
        routes.register::<settings_get::Spec, _, _>(Access::Any, move |_| {
            let settings = here.clone();
            async move { Ok(RouteReply::json::<200>(typed(settings.shown())?)) }
        });
        let here = self.settings.clone();
        routes.register::<settings_patch::Spec, _, _>(Access::Any, move |call| {
            let settings = here.clone();
            async move {
                let given = serde_json::from_slice(&call.request.body).unwrap_or(Value::Null);
                let changed = settings.change(&given).map_err(refused)?;
                Ok(RouteReply::json::<200>(typed(changed)?))
            }
        });
        let work = self.clone();
        routes.register::<project_new::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let given = serde_json::from_slice::<Value>(&call.request.body).ok();
                let Some(Value::Object(given)) = given else {
                    return Err(refused((400, "Send {name}, and git or clone.".into())));
                };
                let (settings, home) = (&work.settings, &work.home);
                let made = match projects::make_project(&given, None, settings, home).await {
                    Err((403, message)) => {
                        let needs = json!({ "message": message, "needs": "projectsFolder" });
                        return Err(Refusal::json(StatusCode::FORBIDDEN, needs));
                    }
                    made => made.map_err(refused)?,
                };
                let text = projects::made_words(&made, settings.account(), settings.here());
                let answer = json!({ "project": made.project, "text": text });
                Ok(RouteReply::json_as_kept::<201>(answer))
            }
        });
        let work = self.clone();
        routes.register::<ask_answer::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let id = percent_encoding::percent_decode_str(call.param("id").unwrap_or_default())
                    .decode_utf8_lossy()
                    .into_owned();
                let given = serde_json::from_slice::<Value>(&call.request.body).ok();
                let given = given
                    .and_then(|given| given.as_object().cloned())
                    .unwrap_or_default();
                let mut answered = work.answer_work(&id, &given).await.map_err(refused)?;
                answered.as_object_mut().map(|said| said.shift_remove("at"));
                Ok(RouteReply::json::<200>(typed(answered)?))
            }
        });
        let work = self.clone();
        routes.register::<unanswering_get::Spec, _, _>(Access::Any, move |_| {
            let work = work.clone();
            async move {
                let held = work.unanswering.now();
                // (Which of an account's models a hold on its paid or free ones covers: as its
                // engine prices them.)
                let mut listed = None;
                if held.iter().any(|each| each.paid || each.free)
                    && let Some(engine) = work.list.engine().running_bot_engine().await
                {
                    let wait = Duration::from_secs(10);
                    let models = engine
                        .call(http::Method::GET, "api/model", None, wait)
                        .await;
                    listed = models.ok().and_then(|models| models.as_array().cloned());
                }
                let list = Unanswerings::list(&held, listed.as_deref());
                Ok(RouteReply::json::<200>(typed(list)?))
            }
        });
        let work = self.clone();
        routes.register::<work_get::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let bot = call
                    .request
                    .path
                    .split_once('?')
                    .map(|(_, query)| query)
                    .unwrap_or_default()
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("bot="))
                    .map(|bot| {
                        percent_encoding::percent_decode_str(&bot.replace('+', " "))
                            .decode_utf8_lossy()
                            .into_owned()
                    })
                    .unwrap_or_default();
                let answer = tokio::task::spawn_blocking(move || work.bot_work(&bot))
                    .await
                    .map_err(|error| {
                        Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
                    })?
                    .map_err(refused)?;
                Ok(RouteReply::json::<200>(typed(answer)?))
            }
        });
        let work = self.clone();
        routes.register::<asks_get::Spec, _, _>(Access::Any, move |_| {
            let work = work.clone();
            async move {
                Ok(RouteReply::json::<200>(typed(
                    json!({ "asks": work.asks() }),
                )?))
            }
        });
        let work = self.clone();
        routes.register::<asks_events::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let reading = work.clone();
                let lines = work.followed(
                    Follow::Asks, move || json!({ "asks": reading.asks() }),
                    call.cancel.clone(),
                );
                Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                    for await value in lines {
                        yield JsonLine::new(serde_json::from_value(value?)?)?;
                    }
                }))
            }
        });
        let here = self.records.clone();
        routes.register::<oversight_get::Spec, _, _>(Access::Any, move |_| {
            let records = here.clone();
            async move {
                Ok(RouteReply::json::<200>(typed(
                    json!({ "marks": records.oversight() }),
                )?))
            }
        });
        let work = self.clone();
        routes.register::<oversight_events::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let records = work.records.clone();
                let lines = work.followed(
                    Follow::Oversight, move || json!({ "marks": records.oversight() }),
                    call.cancel.clone(),
                );
                Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                    for await value in lines {
                        yield JsonLine::new(serde_json::from_value(value?)?)?;
                    }
                }))
            }
        });
        let work = self.clone();
        routes.register::<changed_events::Spec, _, _>(Access::Any, move |call| {
            let work = work.clone();
            async move {
                let reading = work.clone();
                let mut seen: HashMap<String, (String, i64)> = HashMap::new();
                let lines = work.followed(
                    Follow::Changes, move || {
                        let mut changed = reading.records.changes();
                        for (id, key) in reading.work_keys() {
                            let at = match seen.get(&id) {
                                Some((was, at)) if *was == key => *at,
                                old => jiff::Timestamp::now()
                                    .as_millisecond()
                                    .max(old.map_or(0, |(_, at)| *at) + 1),
                            };
                            seen.insert(id.clone(), (key, at));
                            let kept = changed.get(&id).and_then(Value::as_i64).unwrap_or(0);
                            changed.insert(id, json!(kept.max(at)));
                        }
                        json!({ "changed": changed })
                    },
                    call.cancel.clone(),
                );
                Ok(RouteReply::ndjson::<200>(async_stream::try_stream! {
                    for await value in lines {
                        yield JsonLine::new(serde_json::from_value(value?)?)?;
                    }
                }))
            }
        });
    }
}
