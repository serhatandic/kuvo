//! A bot's work and what's asked before it (PROTOCOL.md, "Work"): where new projects go here
//! (`kuvo/work/settings`), this Mac kept awake while agents work, what bots just did to their
//! sessions (`kuvo/work/oversight`) and when each bot's work last changed (`kuvo/work/events`).

pub mod records;
pub mod settings;

use std::{collections::HashMap, path::PathBuf, time::Duration};

use http::StatusCode;
use kuvo_engine::LocalSessions;
use kuvo_peers::setup_lists::Reader;
use kuvo_protocol::routes::{
    get_kuvo_work_events as changed_events, get_kuvo_work_oversight as oversight_get,
    get_kuvo_work_oversight_events as oversight_events, get_kuvo_work_settings as settings_get,
    patch_kuvo_work_settings as settings_patch,
};
use kuvo_server::{Access, JsonLine, Refusal, Registry, RouteReply};
use serde_json::{Value, json};

pub use records::Records;
pub use settings::WorkSettings;

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
    bots: PathBuf,
    sessions: LocalSessions,
    peers: Reader,
}

impl Work {
    pub fn new(
        settings: WorkSettings,
        home: &std::path::Path,
        sessions: LocalSessions,
        peers: Reader,
    ) -> Self {
        Self {
            settings,
            records: Records::new(&home.join("run")),
            bots: home.join("bots"),
            sessions,
            peers,
        }
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
