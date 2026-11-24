//! A bot's work and what's asked before it (PROTOCOL.md, "Work"): where new projects go here
//! (`kuvo/work/settings`), this Mac kept awake while agents work, what bots just did to their
//! sessions (`kuvo/work/oversight`) and when each bot's work last changed (`kuvo/work/events`).

pub mod answers;
pub mod asks;
pub mod bot;
pub mod projects;
pub mod records;
pub mod settings;
pub mod unanswering;

use std::{collections::HashMap, path::PathBuf, time::Duration};

use http::StatusCode;
use kuvo_engine::LocalSessions;
use kuvo_peers::setup_lists::Reader;
use kuvo_protocol::routes::{
    get_kuvo_work as work_get, get_kuvo_work_asks as asks_get,
    get_kuvo_work_asks_events as asks_events, get_kuvo_work_events as changed_events,
    get_kuvo_work_oversight as oversight_get, get_kuvo_work_oversight_events as oversight_events,
    get_kuvo_work_settings as settings_get, get_kuvo_work_unanswering as unanswering_get,
    patch_kuvo_work_settings as settings_patch, post_kuvo_projects_new as project_new,
    post_kuvo_work_asks_id as ask_answer,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Value, json};

pub use records::Records;
pub use settings::WorkSettings;
pub use unanswering::Unanswerings;

use crate::bots::here::BotHere;

/// How often a followed list is read again with nothing said to change (another process's writes).
const LOOK: Duration = Duration::from_secs(2);

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
    bots: PathBuf,
    list: crate::bots::Bots,
    sessions: LocalSessions,
    peers: Reader,
    setup: std::sync::Arc<dyn crate::bots::setup::Setup>,
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
        Self {
            settings,
            records: Records::new(&home.join("run")),
            unanswering: Unanswerings::new(&home.join("run")),
            home: home.to_owned(),
            bots: bots.bots_folder().to_owned(),
            list: bots,
            sessions,
            peers,
            setup,
        }
    }

    /// Follows the engine's events for what they change here: which models can't answer.
    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        let (work, mut events) = (self.clone(), self.list.engine().events());
        tokio::spawn(async move {
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                };
                let engine = work.list.engine().running_bot_engine().await;
                work.unanswering.follow(&event, engine.as_ref()).await;
            }
        })
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

    /// Lines of `read` now and again as it changes: on the session list changing, and a look every
    /// two seconds for what other processes write.
    fn followed<F>(
        &self,
        mut read: F,
        cancel: tokio_util::sync::CancellationToken,
    ) -> impl futures_core::Stream<Item = std::io::Result<Value>> + Send + 'static
    where
        F: FnMut() -> Value + Send + 'static,
    {
        let mut sessions = self.sessions.clone();
        async_stream::try_stream! {
            let mut previous: Option<Value> = None;
            loop {
                let value = read();
                if previous.as_ref() != Some(&value) {
                    previous = Some(value.clone());
                    yield value;
                }
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = sessions.changed() => {}
                    () = tokio::time::sleep(LOOK) => {}
                }
            }
        }
    }

    pub fn register(&self, routes: &mut Registry) {
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
                    move || json!({ "asks": reading.asks() }),
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
                    move || json!({ "marks": records.oversight() }),
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
                    move || {
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
